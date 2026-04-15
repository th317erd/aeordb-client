use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::remote::RemoteClient;
use crate::state::StateStore;
use crate::sync::engine::pull_sync_pass;
use crate::sync::fs_watcher::{FsChange, FsChangeType, FsWatcherConfig, start_fs_watcher};
use crate::sync::push::push_sync_pass;
use crate::sync::relationships::{RelationshipManager, SyncDirection, SyncRelationship};
use crate::sync::sse_listener::{RemoteChange, start_sse_listener};

/// Tracks running sync tasks for each relationship.
#[derive(Clone)]
pub struct SyncRunner {
  running: Arc<Mutex<HashMap<String, RunningSync>>>,
  state:   Arc<StateStore>,
}

struct RunningSync {
  handle: JoinHandle<()>,
  relationship_name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncRunnerStatus {
  pub relationship_id:   String,
  pub relationship_name: String,
  pub running:           bool,
}

impl SyncRunner {
  pub fn new(state: Arc<StateStore>) -> Self {
    Self {
      running: Arc::new(Mutex::new(HashMap::new())),
      state,
    }
  }

  /// Start continuous sync for a relationship.
  pub async fn start(&self, relationship_id: &str) -> Result<()> {
    let mut running = self.running.lock().await;

    if running.contains_key(relationship_id) {
      return Err(ClientError::Configuration(
        format!("sync already running for relationship {}", relationship_id),
      ));
    }

    // Load the relationship and connection
    let relationship_manager = RelationshipManager::new(&self.state);
    let relationship = relationship_manager.get(relationship_id)?
      .ok_or_else(|| ClientError::Configuration(
        format!("sync relationship not found: {}", relationship_id),
      ))?;

    if !relationship.enabled {
      return Err(ClientError::Configuration(
        format!("sync relationship '{}' is disabled", relationship.name),
      ));
    }

    let connection_manager = ConnectionManager::new(&self.state);
    let connection = connection_manager.get(&relationship.remote_connection_id)?
      .ok_or_else(|| ClientError::Configuration(
        format!("connection not found: {}", relationship.remote_connection_id),
      ))?;

    let relationship_name = relationship.name.clone();
    let relationship_id_owned = relationship_id.to_string();
    let state_clone = self.state.clone();
    let direction = relationship.direction.clone();

    tracing::info!(
      "starting continuous sync for '{}' ({})",
      relationship.name, relationship_id,
    );

    // Do an initial full sync pass
    match direction {
      SyncDirection::PullOnly | SyncDirection::Bidirectional => {
        if let Err(error) = pull_sync_pass(&self.state, relationship_id).await {
          tracing::warn!("initial pull sync failed for '{}': {}", relationship.name, error);
        }
      }
      _ => {}
    }

    match direction {
      SyncDirection::PushOnly | SyncDirection::Bidirectional => {
        if let Err(error) = push_sync_pass(&self.state, relationship_id).await {
          tracing::warn!("initial push sync failed for '{}': {}", relationship.name, error);
        }
      }
      _ => {}
    }

    // Spawn the continuous sync task
    let handle = tokio::spawn(async move {
      run_continuous_sync(
        state_clone,
        relationship,
        connection,
        direction,
      ).await;
    });

    running.insert(relationship_id_owned, RunningSync {
      handle,
      relationship_name,
    });

    Ok(())
  }

  /// Stop continuous sync for a relationship.
  pub async fn stop(&self, relationship_id: &str) -> Result<()> {
    let mut running = self.running.lock().await;

    match running.remove(relationship_id) {
      Some(sync) => {
        tracing::info!("stopping sync for '{}'", sync.relationship_name);
        sync.handle.abort();
        Ok(())
      }
      None => Err(ClientError::Configuration(
        format!("sync not running for relationship {}", relationship_id),
      )),
    }
  }

  /// Get status of all sync runners.
  pub async fn status(&self) -> Vec<SyncRunnerStatus> {
    let running = self.running.lock().await;
    let relationship_manager = RelationshipManager::new(&self.state);

    let all_relationships = relationship_manager.list().unwrap_or_default();
    let mut statuses = Vec::new();

    for relationship in all_relationships {
      statuses.push(SyncRunnerStatus {
        relationship_id:   relationship.id.clone(),
        relationship_name: relationship.name.clone(),
        running:           running.contains_key(&relationship.id),
      });
    }

    statuses
  }

  /// Check if a specific relationship's sync is running.
  pub async fn is_running(&self, relationship_id: &str) -> bool {
    let running = self.running.lock().await;
    running.contains_key(relationship_id)
  }

  /// Stop all running syncs.
  pub async fn stop_all(&self) {
    let mut running = self.running.lock().await;

    for (id, sync) in running.drain() {
      tracing::info!("stopping sync for '{}' ({})", sync.relationship_name, id);
      sync.handle.abort();
    }
  }

  /// Start all enabled relationships.
  pub async fn start_all_enabled(&self) {
    let relationship_manager = RelationshipManager::new(&self.state);
    let relationships = relationship_manager.list().unwrap_or_default();

    for relationship in relationships {
      if relationship.enabled {
        if let Err(error) = self.start(&relationship.id).await {
          tracing::warn!("failed to start sync for '{}': {}", relationship.name, error);
        }
      }
    }
  }
}

async fn run_continuous_sync(
  state: Arc<StateStore>,
  relationship: SyncRelationship,
  connection: crate::connections::RemoteConnection,
  direction: SyncDirection,
) {
  let relationship_id = relationship.id.clone();
  let relationship_name = relationship.name.clone();

  tracing::info!("continuous sync active for '{}' ({:?})", relationship_name, direction);

  // Start watchers based on direction
  let mut sse_receiver: Option<mpsc::Receiver<RemoteChange>> = None;
  let mut fs_receiver: Option<mpsc::Receiver<FsChange>> = None;

  // SSE listener for pull-capable directions
  if direction == SyncDirection::PullOnly || direction == SyncDirection::Bidirectional {
    let path_prefixes = vec![relationship.remote_path.clone()];
    sse_receiver = Some(start_sse_listener(connection.clone(), path_prefixes));
    tracing::info!("SSE listener started for '{}'", relationship_name);
  }

  // Filesystem watcher for push-capable directions
  if direction == SyncDirection::PushOnly || direction == SyncDirection::Bidirectional {
    let local_path = Path::new(&relationship.local_path);
    match start_fs_watcher(local_path, FsWatcherConfig::default()) {
      Ok(receiver) => {
        fs_receiver = Some(receiver);
        tracing::info!("filesystem watcher started for '{}'", relationship_name);
      }
      Err(error) => {
        tracing::error!("failed to start filesystem watcher for '{}': {}", relationship_name, error);
      }
    }
  }

  // Main event loop — process changes from both sources
  loop {
    tokio::select! {
      // Handle remote changes (SSE)
      Some(change) = async {
        match sse_receiver.as_mut() {
          Some(rx) => rx.recv().await,
          None => std::future::pending().await,
        }
      } => {
        tracing::info!("remote change: {} {}", change.event_type, change.path);

        // Determine the local path for this file
        if let Some(relative) = change.path.strip_prefix(&relationship.remote_path) {
          let local_path = format!("{}/{}", relationship.local_path, relative);
          let remote_client = RemoteClient::from_connection(&connection);

          match change.event_type.as_str() {
            "entries_created" => {
              // Download the changed file
              match remote_client.download_file(&change.path).await {
                Ok((bytes, _metadata)) => {
                  if let Some(parent) = Path::new(&local_path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                  }
                  match std::fs::write(&local_path, &bytes) {
                    Ok(()) => tracing::info!("pulled: {} → {}", change.path, local_path),
                    Err(error) => tracing::error!("failed to write {}: {}", local_path, error),
                  }
                }
                Err(error) => tracing::error!("failed to download {}: {}", change.path, error),
              }
            }
            "entries_deleted" => {
              if relationship.delete_propagation.remote_to_local {
                match std::fs::remove_file(&local_path) {
                  Ok(()) => tracing::info!("deleted local: {}", local_path),
                  Err(error) => tracing::warn!("failed to delete {}: {}", local_path, error),
                }
              }
            }
            _ => {}
          }
        }
      }

      // Handle local changes (filesystem watcher)
      Some(change) = async {
        match fs_receiver.as_mut() {
          Some(rx) => rx.recv().await,
          None => std::future::pending().await,
        }
      } => {
        let entry_path = &change.path;

        // Compute remote path
        if let Ok(relative) = entry_path.strip_prefix(&relationship.local_path) {
          let remote_file_path = format!("{}{}", relationship.remote_path, relative.display());
          let remote_client = RemoteClient::from_connection(&connection);

          // Apply filter
          let filename = entry_path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
          if !crate::sync::filter::matches_filter(filename, relationship.filter.as_deref()) {
            continue;
          }

          match change.change_type {
            FsChangeType::Created | FsChangeType::Modified => {
              if entry_path.is_symlink() {
                if let Ok(target) = std::fs::read_link(entry_path) {
                  let target_str = target.to_string_lossy().to_string();
                  match remote_client.create_symlink(&remote_file_path, &target_str).await {
                    Ok(()) => tracing::info!("pushed symlink: {} → {}", remote_file_path, target_str),
                    Err(error) => tracing::error!("failed to push symlink {}: {}", remote_file_path, error),
                  }
                }
              } else if entry_path.is_file() {
                match std::fs::read(entry_path) {
                  Ok(bytes) => {
                    let content_type = crate::sync::push::mime_from_extension(entry_path);
                    match remote_client.upload_file(&remote_file_path, bytes, content_type.as_deref()).await {
                      Ok(()) => tracing::info!("pushed: {} → {}", entry_path.display(), remote_file_path),
                      Err(error) => tracing::error!("failed to push {}: {}", remote_file_path, error),
                    }
                  }
                  Err(error) => tracing::error!("failed to read {:?}: {}", entry_path, error),
                }
              }
            }
            FsChangeType::Deleted => {
              if relationship.delete_propagation.local_to_remote {
                match remote_client.delete_file(&remote_file_path).await {
                  Ok(()) => tracing::info!("deleted remote: {}", remote_file_path),
                  Err(error) => tracing::warn!("failed to delete remote {}: {}", remote_file_path, error),
                }
              }
            }
          }
        }
      }
    }
  }
}
