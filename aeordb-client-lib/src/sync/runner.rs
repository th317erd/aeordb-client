use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::state::StateStore;
use crate::sync::filesystem_bridge::{
  WriteSuppressionSet, ingest_directory, ingest_single_file,
  delete_from_aeordb, project_to_filesystem,
};
use crate::sync::fs_watcher::{FsChange, FsChangeType, FsWatcherConfig, start_fs_watcher};
use crate::sync::replication::replicate;
use crate::sync::relationships::{RelationshipManager, SyncDirection, SyncRelationship};
use crate::sync::sse_listener::start_sse_listener;

/// Tracks running sync tasks for each relationship.
#[derive(Clone)]
pub struct SyncRunner {
  running: Arc<Mutex<HashMap<String, RunningSync>>>,
  state:   Arc<StateStore>,
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

    let relationship_name   = relationship.name.clone();
    let relationship_id_owned = relationship_id.to_string();
    let state_clone         = self.state.clone();

    tracing::info!("starting sync for '{}' ({:?})", relationship.name, relationship.direction);

    let handle = tokio::spawn(async move {
      run_sync_loop(state_clone, relationship, connection).await;
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
    let relationship_manager = RelationshipManager::new(&self.state);
    for relationship in relationship_manager.list().unwrap_or_default() {
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
  relationship: SyncRelationship,
  connection: crate::connections::RemoteConnection,
) {
  let direction       = relationship.direction.clone();
  let filter          = relationship.filter.clone();
  let suppression     = WriteSuppressionSet::new();

  tracing::info!("sync loop active for '{}' ({:?})", relationship.name, direction);

  // --- Step 1: Initial ingest — filesystem → local aeordb ---
  if direction == SyncDirection::PushOnly || direction == SyncDirection::Bidirectional {
    if let Err(error) = ingest_directory(
      state.engine(),
      &relationship.local_path,
      &relationship.remote_path,
      filter.as_deref(),
    ) {
      tracing::error!("initial ingest failed for '{}': {}", relationship.name, error);
    }
  }

  // --- Step 2: Initial replication --- local aeordb ↔ remote aeordb ---
  let paths_filter: Option<Vec<String>> = filter.as_ref().map(|f| vec![f.clone()]);
  let paths_ref: Option<&[String]> = paths_filter.as_deref();

  do_replication_cycle(&state, &connection, &relationship, &direction, paths_ref, &suppression).await;

  // --- Step 3: Start watchers based on direction ---
  let mut fs_receiver: Option<mpsc::Receiver<FsChange>> = None;
  let mut sse_receiver: Option<mpsc::Receiver<crate::sync::sse_listener::RemoteChange>> = None;

  // Filesystem watcher for push-capable directions
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

  // SSE listener for pull-capable directions
  if direction == SyncDirection::PullOnly || direction == SyncDirection::Bidirectional {
    let path_prefixes = vec![relationship.remote_path.clone()];
    sse_receiver = Some(start_sse_listener(connection.clone(), path_prefixes));
    tracing::info!("SSE listener started for '{}'", relationship.name);
  }

  // --- Step 4: Event loop — react to changes from either side ---
  loop {
    tokio::select! {
      // Local filesystem change
      Some(change) = async {
        match fs_receiver.as_mut() {
          Some(rx) => rx.recv().await,
          None => std::future::pending().await,
        }
      } => {
        // Skip events caused by our own writes
        if suppression.should_suppress(&change.path) {
          continue;
        }

        // Apply filter
        let filename = change.path.file_name()
          .and_then(|n| n.to_str())
          .unwrap_or("");
        if !crate::sync::filter::matches_filter(filename, filter.as_deref()) {
          continue;
        }

        // Ingest change into local aeordb
        match change.change_type {
          FsChangeType::Created | FsChangeType::Modified => {
            if let Err(error) = ingest_single_file(
              state.engine(),
              &change.path,
              &relationship.local_path,
              &relationship.remote_path,
            ) {
              tracing::error!("failed to ingest {:?}: {}", change.path, error);
              continue;
            }
          }
          FsChangeType::Deleted => {
            if relationship.delete_propagation.local_to_remote {
              if let Err(error) = delete_from_aeordb(
                state.engine(),
                &change.path,
                &relationship.local_path,
                &relationship.remote_path,
              ) {
                tracing::warn!("failed to delete from aeordb: {}", error);
              }
            }
            continue; // Don't need to replicate just for a local delete
          }
        }

        // Replicate to push our change to the remote
        do_replication_cycle(&state, &connection, &relationship, &direction, paths_ref, &suppression).await;
      }

      // Remote SSE change
      Some(_change) = async {
        match sse_receiver.as_mut() {
          Some(rx) => rx.recv().await,
          None => std::future::pending().await,
        }
      } => {
        // Remote changed — replicate to pull their changes
        do_replication_cycle(&state, &connection, &relationship, &direction, paths_ref, &suppression).await;
      }

      // Periodic safety net — replicate every 60 seconds regardless
      _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
        do_replication_cycle(&state, &connection, &relationship, &direction, paths_ref, &suppression).await;
      }
    }
  }
}

/// Execute a replication cycle, respecting direction, then project changes to filesystem.
async fn do_replication_cycle(
  state: &Arc<StateStore>,
  connection: &crate::connections::RemoteConnection,
  relationship: &SyncRelationship,
  direction: &SyncDirection,
  paths_filter: Option<&[String]>,
  suppression: &WriteSuppressionSet,
) {
  // Replicate between local aeordb and remote aeordb
  // Direction control: we always call replicate(), but the replication module
  // handles chunk exchange. For pull-only, we only apply remote chunks locally
  // and don't push ours. For push-only, we only push ours and don't apply theirs.
  //
  // Currently replicate() is always bidirectional. Direction filtering happens
  // at the caller level — we skip the ingest step for pull-only (nothing to push),
  // and skip the project step for push-only (nothing to write locally).

  match replicate(state.engine(), connection, None, paths_filter).await {
    Ok(result) => {
      if result.pulled_chunks > 0 || result.pushed_chunks > 0 || result.conflicts > 0 {
        tracing::info!(
          "replication for '{}': pulled={}, pushed={}, conflicts={}",
          relationship.name, result.pulled_chunks, result.pushed_chunks, result.conflicts,
        );
      }

      // Project remote changes to filesystem (for pull-capable directions)
      if *direction == SyncDirection::PullOnly || *direction == SyncDirection::Bidirectional {
        if result.pulled_chunks > 0 || result.files_changed > 0 {
          if let Err(error) = project_to_filesystem(
            state.engine(),
            &relationship.local_path,
            &relationship.remote_path,
            relationship.filter.as_deref(),
            suppression,
          ) {
            tracing::error!("failed to project changes for '{}': {}", relationship.name, error);
          }
        }
      }
    }
    Err(error) => {
      tracing::error!("replication failed for '{}': {}", relationship.name, error);
    }
  }
}
