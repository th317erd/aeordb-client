use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::config::ConfigStore;
use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::state::StateStore;
use crate::sync::activity::SyncActivityLog;
use crate::sync::fs_watcher::{FsChangeType, FsWatcherConfig, start_fs_watcher};
use crate::sync::pull::pull_sync;
use crate::sync::push::push_sync;
use crate::sync::relationships::{RelationshipManager, SyncDirection, SyncRelationship};
use crate::sync::replication::sync_relationship;
use crate::sync::sse_listener::start_sse_listener;

/// Tracks running sync tasks for each relationship.
#[derive(Clone)]
pub struct SyncRunner {
  running:     Arc<Mutex<HashMap<String, RunningSync>>>,
  state:       Arc<StateStore>,
  config:      Arc<ConfigStore>,
  activity:    SyncActivityLog,
  http_client: reqwest::Client,
}

struct RunningSync {
  handle:            JoinHandle<()>,
  relationship_name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncRunnerStatus {
  pub relationship_id:   String,
  pub relationship_name: String,
  pub running:           bool,
}

impl SyncRunner {
  pub fn new(state: Arc<StateStore>, config: Arc<ConfigStore>, http_client: reqwest::Client) -> Self {
    let activity = SyncActivityLog::new(state.clone());

    Self {
      running:     Arc::new(Mutex::new(HashMap::new())),
      state,
      config,
      activity,
      http_client,
    }
  }

  /// Get a reference to the activity log.
  pub fn activity_log(&self) -> &SyncActivityLog {
    &self.activity
  }

  /// Start continuous sync for a relationship.
  pub async fn start(&self, relationship_id: &str) -> Result<()> {
    let mut running = self.running.lock().await;

    if running.contains_key(relationship_id) {
      return Err(ClientError::Configuration(
        format!("sync already running for relationship {}", relationship_id),
      ));
    }

    let relationship_manager = RelationshipManager::new(&self.config);
    let relationship = relationship_manager.get(relationship_id).await?
      .ok_or_else(|| ClientError::Configuration(
        format!("sync relationship not found: {}", relationship_id),
      ))?;

    if !relationship.enabled {
      return Err(ClientError::Configuration(
        format!("sync relationship '{}' is disabled", relationship.name),
      ));
    }

    let connection_manager = ConnectionManager::new(&self.config);
    let connection = connection_manager.get(&relationship.remote_connection_id).await?
      .ok_or_else(|| ClientError::Configuration(
        format!("connection not found: {}", relationship.remote_connection_id),
      ))?;

    let relationship_name     = relationship.name.clone();
    let relationship_id_owned = relationship_id.to_string();
    let state_clone           = self.state.clone();
    let activity_clone        = self.activity.clone();
    let http_client_clone     = self.http_client.clone();
    let config_clone          = self.config.clone();

    let sync_interval = self.config.get().await
      .map(|c| c.settings.sync_interval_seconds)
      .unwrap_or(60);

    tracing::info!("starting sync for '{}' ({:?})", relationship.name, relationship.direction);

    let handle = tokio::spawn(async move {
      run_sync_loop(state_clone, activity_clone, config_clone, relationship, connection, http_client_clone, sync_interval).await;
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
    let relationship_manager = RelationshipManager::new(&self.config);
    let all_relationships = relationship_manager.list().await.unwrap_or_default();

    all_relationships.iter()
      .map(|relationship| SyncRunnerStatus {
        relationship_id:   relationship.id.clone(),
        relationship_name: relationship.name.clone(),
        running:           running.contains_key(&relationship.id),
      })
      .collect()
  }

  /// Check if a specific relationship's sync is running.
  pub async fn is_running(&self, relationship_id: &str) -> bool {
    self.running.lock().await.contains_key(relationship_id)
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
    let relationship_manager = RelationshipManager::new(&self.config);
    for relationship in relationship_manager.list().await.unwrap_or_default() {
      if relationship.enabled {
        if let Err(error) = self.start(&relationship.id).await {
          tracing::warn!("failed to start sync for '{}': {}", relationship.name, error);
        }
      }
    }
  }
}

/// The main sync loop for a single relationship.
async fn run_sync_loop(
  state: Arc<StateStore>,
  activity: SyncActivityLog,
  config: Arc<ConfigStore>,
  relationship: SyncRelationship,
  connection: crate::connections::RemoteConnection,
  http_client: reqwest::Client,
  sync_interval_seconds: u64,
) {
  let direction = relationship.direction.clone();
  let filter    = relationship.filter.clone();

  tracing::info!("sync loop active for '{}' ({:?})", relationship.name, direction);

  // --- Step 1: Initial full sync (push + pull based on direction) ---
  match sync_relationship(&state, &connection, &relationship, &http_client).await {
    Ok(result) => {
      log_sync_result(&relationship.name, &result);
      if let Err(error) = activity.log_full_sync(&relationship.id, &relationship.name, &result) {
        tracing::warn!("failed to log sync activity for '{}': {}", relationship.name, error);
      }
    }
    Err(error) => {
      tracing::error!("initial sync failed for '{}': {}", relationship.name, error);
      if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error.to_string()) {
        tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
      }
    }
  }

  // --- Step 2: Start watchers based on direction ---
  let mut fs_receiver: Option<mpsc::Receiver<crate::sync::fs_watcher::FsChange>> = None;
  let mut sse_receiver: Option<mpsc::Receiver<crate::sync::sse_listener::RemoteChange>> = None;

  // Filesystem watcher for push-capable directions.
  if direction == SyncDirection::PushOnly || direction == SyncDirection::Bidirectional {
    let local_path = Path::new(&relationship.local_path);
    match start_fs_watcher(local_path, FsWatcherConfig::default()) {
      Ok(receiver) => {
        fs_receiver = Some(receiver);
        tracing::info!("filesystem watcher started for '{}'", relationship.name);
      }
      Err(error) => {
        tracing::error!("failed to start watcher for '{}': {}", relationship.name, error);
      }
    }
  }

  // SSE listener for pull-capable directions.
  if direction == SyncDirection::PullOnly || direction == SyncDirection::Bidirectional {
    let path_prefixes = vec![relationship.remote_path.clone()];
    sse_receiver = Some(start_sse_listener(connection.clone(), path_prefixes));
    tracing::info!("SSE listener started for '{}'", relationship.name);
  }

  // --- Step 3: Event loop -- react to changes from either side ---
  loop {
    tokio::select! {
      // Local filesystem change -- push to remote.
      // The watcher might fire for files we just wrote during pull,
      // but push_sync uses hash comparison and will skip unchanged files.
      Some(change) = async {
        match fs_receiver.as_mut() {
          Some(rx) => rx.recv().await,
          None => std::future::pending().await,
        }
      } => {
        // Apply filter.
        let filename = change.path.file_name()
          .and_then(|n| n.to_str())
          .unwrap_or("");
        if !crate::sync::filter::matches_filter(filename, filter.as_deref()) {
          continue;
        }

        // Skip delete events when delete propagation is disabled.
        if change.change_type == FsChangeType::Deleted
          && !relationship.delete_propagation.local_to_remote
        {
          continue;
        }

        // Push local changes to the remote.
        match push_sync(&state, &connection, &relationship, &http_client).await {
          Ok(result) => {
            if result.files_pushed > 0 || result.files_deleted > 0 || result.files_failed > 0 {
              tracing::info!(
                "push for '{}': pushed={}, deleted={}, skipped={}, failed={}",
                relationship.name, result.files_pushed, result.files_deleted,
                result.files_skipped, result.files_failed,
              );
            }
            if let Err(error) = activity.log_push(&relationship.id, &relationship.name, &result) {
              tracing::warn!("failed to log push activity for '{}': {}", relationship.name, error);
            }
          }
          Err(error) => {
            tracing::error!("push failed for '{}': {}", relationship.name, error);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error.to_string()) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
          }
        }
      }

      // Remote SSE change -- pull from remote.
      Some(_change) = async {
        match sse_receiver.as_mut() {
          Some(rx) => rx.recv().await,
          None => std::future::pending().await,
        }
      } => {
        match pull_sync(&state, &connection, &relationship, &http_client).await {
          Ok(result) => {
            if result.files_pulled > 0 || result.files_deleted > 0 || result.files_failed > 0 {
              tracing::info!(
                "pull for '{}': pulled={}, deleted={}, skipped={}, failed={}",
                relationship.name, result.files_pulled, result.files_deleted,
                result.files_skipped, result.files_failed,
              );
            }
            if let Err(error) = activity.log_pull(&relationship.id, &relationship.name, &result) {
              tracing::warn!("failed to log pull activity for '{}': {}", relationship.name, error);
            }
          }
          Err(error) => {
            tracing::error!("pull failed for '{}': {}", relationship.name, error);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error.to_string()) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
          }
        }
      }

      // Periodic safety net -- full sync at configured interval.
      _ = tokio::time::sleep(std::time::Duration::from_secs(sync_interval_seconds)) => {
        // Re-read config in case it changed
        let relationship_manager = RelationshipManager::new(&config);
        let current_relationship = match relationship_manager.get(&relationship.id).await {
          Ok(Some(r)) if r.enabled => r,
          _ => {
            tracing::info!("relationship '{}' was deleted or disabled, exiting sync loop", relationship.name);
            break;
          }
        };
        let connection_manager = ConnectionManager::new(&config);
        let current_connection = match connection_manager.get(&current_relationship.remote_connection_id).await {
          Ok(Some(c)) => c,
          _ => {
            tracing::warn!("connection for '{}' not found, skipping periodic sync", relationship.name);
            continue;
          }
        };

        match sync_relationship(&state, &current_connection, &current_relationship, &http_client).await {
          Ok(result) => {
            log_sync_result(&relationship.name, &result);
            if let Err(error) = activity.log_full_sync(&relationship.id, &relationship.name, &result) {
              tracing::warn!("failed to log sync activity for '{}': {}", relationship.name, error);
            }
          }
          Err(error) => {
            tracing::error!("periodic sync failed for '{}': {}", relationship.name, error);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error.to_string()) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
          }
        }
      }
    }
  }
}

/// Log the results of a full sync_relationship call.
fn log_sync_result(
  name: &str,
  result: &crate::sync::replication::SyncResult,
) {
  if let Some(ref pull) = result.pull {
    if pull.files_pulled > 0 || pull.files_deleted > 0 || pull.files_failed > 0 {
      tracing::info!(
        "pull for '{}': pulled={}, deleted={}, skipped={}, failed={}",
        name, pull.files_pulled, pull.files_deleted,
        pull.files_skipped, pull.files_failed,
      );
    }
  }

  if let Some(ref push) = result.push {
    if push.files_pushed > 0 || push.files_deleted > 0 || push.files_failed > 0 {
      tracing::info!(
        "push for '{}': pushed={}, deleted={}, skipped={}, failed={}",
        name, push.files_pushed, push.files_deleted,
        push.files_skipped, push.files_failed,
      );
    }
  }
}
