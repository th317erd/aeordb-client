use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::state::StateStore;
use crate::sync::relationships::{RelationshipManager, SyncRelationship};
use crate::sync::sse_listener::{RemoteChange, start_sse_listener};

/// Start continuous sync watching for all enabled relationships.
/// This spawns SSE listeners per-connection and processes incoming changes.
///
/// Returns a JoinHandle for the watcher task.
pub fn start_continuous_sync(
  state: Arc<StateStore>,
) -> tokio::task::JoinHandle<()> {
  tokio::spawn(async move {
    if let Err(error) = run_continuous_sync(&state).await {
      tracing::error!("continuous sync failed: {}", error);
    }
  })
}

async fn run_continuous_sync(state: &StateStore) -> Result<()> {
  let relationship_manager = RelationshipManager::new(state);
  let connection_manager   = ConnectionManager::new(state);

  let relationships = relationship_manager.list()?;
  let enabled: Vec<_> = relationships.into_iter().filter(|r| r.enabled).collect();

  if enabled.is_empty() {
    tracing::info!("no enabled sync relationships — continuous sync idle");
    // Just sleep forever; we'll be restarted when config changes
    loop {
      tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
  }

  // Group relationships by connection ID
  let mut by_connection: HashMap<String, Vec<SyncRelationship>> = HashMap::new();
  for relationship in &enabled {
    by_connection
      .entry(relationship.remote_connection_id.clone())
      .or_default()
      .push(relationship.clone());
  }

  // Spawn one SSE listener per connection, collecting path prefixes
  let mut receivers: Vec<(mpsc::Receiver<RemoteChange>, Vec<SyncRelationship>)> = Vec::new();

  for (connection_id, relationships) in by_connection {
    let connection = connection_manager.get(&connection_id)?
      .ok_or_else(|| ClientError::Configuration(
        format!("connection not found: {}", connection_id),
      ))?;

    let path_prefixes: Vec<String> = relationships.iter()
      .map(|r| r.remote_path.clone())
      .collect();

    tracing::info!(
      "starting SSE listener for '{}' watching {} paths",
      connection.name, path_prefixes.len(),
    );

    let receiver = start_sse_listener(connection, path_prefixes);
    receivers.push((receiver, relationships));
  }

  // Process changes from all receivers
  // For now, handle each receiver in its own task
  let mut handles = Vec::new();

  for (mut receiver, relationships) in receivers {
    let relationships_clone: Vec<SyncRelationship> = relationships.clone();

    handles.push(tokio::spawn(async move {
      while let Some(change) = receiver.recv().await {
        tracing::debug!("SSE change: {} {}", change.event_type, change.path);

        // Find which relationship this path belongs to
        for relationship in &relationships_clone {
          if change.path.starts_with(&relationship.remote_path) {
            // Determine local path
            let relative = change.path.strip_prefix(&relationship.remote_path)
              .unwrap_or(&change.path);
            let local_path = format!("{}/{}", relationship.local_path, relative);

            match change.event_type.as_str() {
              "entries_created" => {
                tracing::info!("remote change detected: {} → syncing to {}", change.path, local_path);
                // We'll trigger a targeted download here
                // For now, just log it — a full targeted download requires
                // the RemoteClient and StateStore which we'd need to pass through
              }
              "entries_deleted" => {
                tracing::info!("remote deletion detected: {}", change.path);
                // Handle delete propagation based on relationship config
              }
              _ => {}
            }

            break;
          }
        }
      }
    }));
  }

  // Wait for all handlers (they run indefinitely)
  for handle in handles {
    let _ = handle.await;
  }

  Ok(())
}
