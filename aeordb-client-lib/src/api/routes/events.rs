use axum::extract::{Path as AxumPath, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{StreamExt, stream::Stream};
use serde::Serialize;
use tokio_stream::wrappers::BroadcastStream;

use crate::connections::ConnectionManager;
use crate::error::ClientError;
use crate::remote::RemoteClient;
use crate::server::AppState;
use crate::sync::relationships::RelationshipManager;
use crate::sync::sse_listener::parse_sse_message;

pub async fn event_stream(
  State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
  let rx = state.event_tx.subscribe();
  let stream = BroadcastStream::new(rx)
    .filter_map(|result| async move { result.ok() })
    .map(|se| Ok(Event::default().event(se.event_name).data(se.data)));

  Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}

#[derive(Debug, Serialize)]
struct RelationshipFileEvent {
  relationship_id: String,
  event_type: String,
  remote_path: String,
  path: String,
}

fn relationship_relative_path(relationship_remote_path: &str, remote_path: &str) -> Option<String> {
  let remote_path = if remote_path.starts_with('/') {
    remote_path.to_string()
  } else {
    format!("/{}", remote_path)
  };
  let base = relationship_remote_path.trim_end_matches('/');

  if base.is_empty() || base == "/" {
    return Some(remote_path);
  }

  if remote_path == base {
    return Some("/".to_string());
  }

  let prefix = format!("{}/", base);
  remote_path
    .strip_prefix(&prefix)
    .map(|relative| format!("/{}", relative))
}

pub async fn relationship_file_events(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, ClientError> {
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager
    .get(&relationship_id)
    .await?
    .ok_or_else(|| ClientError::NotFound(format!("relationship not found: {}", relationship_id)))?;

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager
    .get(&relationship.remote_connection_id)
    .await?
    .ok_or_else(|| {
      ClientError::NotFound(format!(
        "connection not found: {}",
        relationship.remote_connection_id
      ))
    })?;

  let jwt_slot = state.jwt_cache.slot_for(&relationship.remote_connection_id);
  let remote_client = RemoteClient::from_connection_cached(&connection, &state.http_client, jwt_slot);
  let response = remote_client.file_event_stream(&relationship.remote_path).await?;
  let mut remote_stream = response.bytes_stream();
  let remote_prefixes = vec![relationship.remote_path.clone()];
  let relationship_remote_path = relationship.remote_path.clone();
  let relationship_id_for_stream = relationship.id.clone();

  let stream = async_stream::stream! {
    let mut buffer = String::new();

    while let Some(chunk_result) = remote_stream.next().await {
      let chunk = match chunk_result {
        Ok(chunk) => chunk,
        Err(error) => {
          let data = serde_json::json!({ "error": error.to_string() }).to_string();
          yield Ok(Event::default().event("upstream_error").data(data));
          break;
        }
      };

      let text = String::from_utf8_lossy(&chunk);
      buffer.push_str(&text);

      while let Some(boundary) = buffer.find("\n\n") {
        let message = buffer[..boundary].to_string();
        buffer = buffer[boundary + 2..].to_string();

        let Some(changes) = parse_sse_message(&message, &remote_prefixes) else {
          continue;
        };

        for change in changes {
          let Some(relative_path) =
            relationship_relative_path(&relationship_remote_path, &change.path)
          else {
            continue;
          };

          let payload = RelationshipFileEvent {
            relationship_id: relationship_id_for_stream.clone(),
            event_type: change.event_type.clone(),
            remote_path: change.path,
            path: relative_path,
          };
          let data = match serde_json::to_string(&payload) {
            Ok(data) => data,
            Err(_) => continue,
          };

          yield Ok(Event::default().event(change.event_type).data(data));
        }
      }
    }
  };

  Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15))))
}

#[cfg(test)]
mod tests {
  use super::relationship_relative_path;

  #[test]
  fn relationship_relative_path_strips_relationship_root() {
    assert_eq!(
      relationship_relative_path("/workspaces/wyatt/", "/workspaces/wyatt/Pictures/a.jpg"),
      Some("/Pictures/a.jpg".to_string()),
    );
    assert_eq!(
      relationship_relative_path("/workspaces/wyatt", "/workspaces/wyatt"),
      Some("/".to_string()),
    );
    assert_eq!(
      relationship_relative_path("/", "/Pictures/a.jpg"),
      Some("/Pictures/a.jpg".to_string()),
    );
    assert_eq!(
      relationship_relative_path("/workspaces/wyatt", "/other/a.jpg"),
      None,
    );
  }
}
