use std::time::Duration;

use serde::Deserialize;
use tokio::sync::mpsc;

use crate::connections::{AuthType, RemoteConnection};
use crate::error::Result;

/// An event received from the remote aeordb SSE stream.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteEvent {
  pub event_id:   String,
  pub event_type: String,
  pub timestamp:  i64,
  pub payload:    serde_json::Value,
}

/// A change detected from SSE — a file was created, modified, or deleted.
#[derive(Debug, Clone)]
pub struct RemoteChange {
  pub event_type: String,
  pub path:       String,
}

/// Start an SSE listener for a remote aeordb connection.
/// Returns a receiver channel that emits RemoteChange events.
///
/// The listener runs in a background task, reconnects on failure with
/// exponential backoff, and filters events by the given path prefixes.
pub fn start_sse_listener(
  connection: RemoteConnection,
  path_prefixes: Vec<String>,
) -> mpsc::Receiver<RemoteChange> {
  let (sender, receiver) = mpsc::channel(256);

  tokio::spawn(async move {
    sse_listener_loop(connection, path_prefixes, sender).await;
  });

  receiver
}

async fn sse_listener_loop(
  connection: RemoteConnection,
  path_prefixes: Vec<String>,
  sender: mpsc::Sender<RemoteChange>,
) {
  let mut backoff = Duration::from_secs(1);
  let max_backoff = Duration::from_secs(60);

  loop {
    match connect_and_listen(&connection, &path_prefixes, &sender).await {
      Ok(()) => {
        // Stream ended cleanly (server closed connection)
        tracing::info!("SSE stream closed for {}, reconnecting...", connection.name);
        backoff = Duration::from_secs(1); // Reset backoff on clean close
      }
      Err(error) => {
        tracing::warn!(
          "SSE connection to '{}' failed: {}. Retrying in {:?}",
          connection.name, error, backoff,
        );
      }
    }

    tokio::time::sleep(backoff).await;
    backoff = (backoff * 2).min(max_backoff);
  }
}

async fn connect_and_listen(
  connection: &RemoteConnection,
  path_prefixes: &[String],
  sender: &mpsc::Sender<RemoteChange>,
) -> Result<()> {
  let mut url = format!(
    "{}/system/events?events=entries_created,entries_deleted",
    connection.url,
  );

  // If we have a single path prefix, use the server-side filter
  if path_prefixes.len() == 1 {
    url = format!("{}&path_prefix={}", url, path_prefixes[0]);
  }

  let client      = reqwest::Client::new();
  let mut request = client.get(&url);

  if connection.auth_type == AuthType::ApiKey {
    if let Some(ref api_key) = connection.api_key {
      request = request.header("Authorization", format!("Bearer {}", api_key));
    }
  }

  let response = request.send().await.map_err(|error| {
    crate::error::ClientError::Server(format!("SSE connect failed: {}", error))
  })?;

  if !response.status().is_success() {
    return Err(crate::error::ClientError::Server(
      format!("SSE returned HTTP {}", response.status()),
    ));
  }

  tracing::info!("SSE connected to '{}'", connection.name);

  let mut stream = response.bytes_stream();
  let mut buffer = String::new();

  use futures_util::StreamExt;

  while let Some(chunk_result) = stream.next().await {
    let chunk = chunk_result.map_err(|error| {
      crate::error::ClientError::Server(format!("SSE read error: {}", error))
    })?;

    let text = String::from_utf8_lossy(&chunk);
    buffer.push_str(&text);

    // Process complete SSE messages (terminated by double newline)
    while let Some(boundary) = buffer.find("\n\n") {
      let message = buffer[..boundary].to_string();
      buffer = buffer[boundary + 2..].to_string();

      if let Some(changes) = parse_sse_message(&message, path_prefixes) {
        for change in changes {
          if sender.send(change).await.is_err() {
            // Receiver dropped — stop listening
            return Ok(());
          }
        }
      }
    }
  }

  Ok(())
}

/// Parse an SSE message and extract RemoteChange events.
fn parse_sse_message(message: &str, path_prefixes: &[String]) -> Option<Vec<RemoteChange>> {
  let mut event_type = None;
  let mut data       = None;

  for line in message.lines() {
    if let Some(value) = line.strip_prefix("event: ") {
      event_type = Some(value.trim().to_string());
    } else if let Some(value) = line.strip_prefix("data: ") {
      data = Some(value.trim().to_string());
    }
  }

  let event_type = event_type?;
  let data       = data?;

  let event: RemoteEvent = serde_json::from_str(&data).ok()?;

  let mut changes = Vec::new();

  // Extract paths from payload.entries[]
  if let Some(entries) = event.payload.get("entries").and_then(|v| v.as_array()) {
    for entry in entries {
      if let Some(path) = entry.get("path").and_then(|v| v.as_str()) {
        // Client-side path filtering (for multiple prefixes)
        let matches = path_prefixes.is_empty() || path_prefixes.iter().any(|prefix| path.starts_with(prefix));

        if matches {
          changes.push(RemoteChange {
            event_type: event_type.clone(),
            path:       path.to_string(),
          });
        }
      }
    }
  }

  if changes.is_empty() {
    None
  } else {
    Some(changes)
  }
}
