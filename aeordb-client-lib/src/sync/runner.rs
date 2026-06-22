use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Sleep, timeout};

use crate::config::ConfigStore;
use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::health::{HealthMap, HealthStatus};
use crate::state::StateStore;
use crate::sync::activity::SyncActivityLog;
use crate::sync::fs_watcher::{FsChangeType, FsWatcherConfig, start_fs_watcher};
use crate::sync::pull::pull_sync;
use crate::sync::push::{PushProgressReporter, PushScanMode, push_sync};
use crate::sync::relationships::{RelationshipManager, SyncDirection, SyncRelationship};
use crate::sync::replication::sync_relationship;
use crate::sync::sse_listener::start_sse_listener;

type SyncExecutionGuards = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;
const WATCHED_SAFETY_NET_MIN_SECONDS: u64 = 60 * 60;
const SYNC_TASK_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_SYNC_DELAY: Duration = Duration::from_secs(60);
const TRANSIENT_SYNC_RETRY_INITIAL_DELAY: Duration = Duration::from_secs(60);
const TRANSIENT_SYNC_RETRY_MAX_DELAY: Duration = Duration::from_secs(15 * 60);

/// Tracks running sync tasks for each relationship.
#[derive(Clone)]
pub struct SyncRunner {
  running: Arc<Mutex<HashMap<String, RunningSync>>>,
  execution_guards: SyncExecutionGuards,
  state: Arc<StateStore>,
  config: Arc<ConfigStore>,
  activity: SyncActivityLog,
  http_client: reqwest::Client,
  event_tx: broadcast::Sender<crate::server::ServerEvent>,
  /// Shared JWT cache (same instance as AppState.jwt_cache) so the
  /// sync loop reuses the cached token instead of re-exchanging on
  /// every push/pull/diff cycle.
  jwt_cache: crate::jwt_cache::JwtCache,
}

struct RunningSync {
  handle: JoinHandle<()>,
  relationship_name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncRunnerStatus {
  pub relationship_id: String,
  pub relationship_name: String,
  pub remote_connection_id: String,
  pub running: bool,
  pub executing: bool,
  pub syncing: bool,
  pub connection_health: HealthStatus,
  pub connection_healthy: bool,
  pub connection_checked_at: Option<i64>,
  pub connection_message: Option<String>,
}

impl SyncRunner {
  pub fn new(
    state: Arc<StateStore>,
    config: Arc<ConfigStore>,
    http_client: reqwest::Client,
    event_tx: broadcast::Sender<crate::server::ServerEvent>,
    jwt_cache: crate::jwt_cache::JwtCache,
  ) -> Self {
    let activity = SyncActivityLog::new(state.clone());

    Self {
      running: Arc::new(Mutex::new(HashMap::new())),
      execution_guards: Arc::new(Mutex::new(HashMap::new())),
      state,
      config,
      activity,
      http_client,
      event_tx,
      jwt_cache,
    }
  }

  /// Get a reference to the event broadcast sender.
  pub fn event_tx(&self) -> &broadcast::Sender<crate::server::ServerEvent> {
    &self.event_tx
  }

  /// Get a reference to the activity log.
  pub fn activity_log(&self) -> &SyncActivityLog {
    &self.activity
  }

  pub async fn execution_guard(&self, relationship_id: &str) -> Arc<Mutex<()>> {
    get_execution_guard(&self.execution_guards, relationship_id).await
  }

  /// Start continuous sync for a relationship.
  pub async fn start(&self, relationship_id: &str) -> Result<()> {
    let mut running = self.running.lock().await;

    if running.contains_key(relationship_id) {
      return Err(ClientError::Configuration(format!(
        "sync already running for relationship {}",
        relationship_id
      )));
    }

    let relationship_manager = RelationshipManager::new(&self.config);
    let relationship = relationship_manager
      .get(relationship_id)
      .await?
      .ok_or_else(|| {
        ClientError::Configuration(format!("sync relationship not found: {}", relationship_id))
      })?;

    if !relationship.enabled {
      return Err(ClientError::Configuration(format!(
        "sync relationship '{}' is disabled",
        relationship.name
      )));
    }

    let connection_manager = ConnectionManager::new(&self.config);
    let connection = connection_manager
      .get(&relationship.remote_connection_id)
      .await?
      .ok_or_else(|| {
        ClientError::Configuration(format!(
          "connection not found: {}",
          relationship.remote_connection_id
        ))
      })?;

    let relationship_name = relationship.name.clone();
    let relationship_id_owned = relationship_id.to_string();
    let state_clone = self.state.clone();
    let activity_clone = self.activity.clone();
    let http_client_clone = self.http_client.clone();
    let config_clone = self.config.clone();
    let event_tx_clone = self.event_tx.clone();
    let jwt_cache_clone = self.jwt_cache.clone();
    let execution_guards_clone = self.execution_guards.clone();

    let sync_interval = self
      .config
      .get()
      .await
      .map(|c| c.settings.sync_interval_seconds)
      .unwrap_or(60);

    tracing::info!(
      "starting sync for '{}' ({:?})",
      relationship.name,
      relationship.direction
    );

    let handle = tokio::spawn(async move {
      run_sync_loop(
        state_clone,
        activity_clone,
        config_clone,
        relationship,
        connection,
        http_client_clone,
        sync_interval,
        event_tx_clone,
        jwt_cache_clone,
        execution_guards_clone,
      )
      .await;
    });

    running.insert(
      relationship_id_owned,
      RunningSync {
        handle,
        relationship_name,
      },
    );

    Ok(())
  }

  /// Stop continuous sync for a relationship.
  pub async fn stop(&self, relationship_id: &str) -> Result<()> {
    let mut running = self.running.lock().await;

    match running.remove(relationship_id) {
      Some(sync) => {
        tracing::info!("stopping sync for '{}'", sync.relationship_name);
        abort_sync_task(relationship_id.to_string(), sync).await;
        Ok(())
      }
      None => Err(ClientError::Configuration(format!(
        "sync not running for relationship {}",
        relationship_id
      ))),
    }
  }

  /// Get status of all sync runners, enriched with the latest database
  /// connection health. `executing` is the raw sync mutex state; `syncing`
  /// is user-facing and only true while execution is active against a
  /// currently healthy connection.
  pub async fn status(&self, health_map: &HealthMap) -> Vec<SyncRunnerStatus> {
    let running = self.running.lock().await;
    let relationship_manager = RelationshipManager::new(&self.config);
    let all_relationships = relationship_manager.list().await.unwrap_or_default();
    let health = health_map.lock().await;

    let mut statuses = Vec::with_capacity(all_relationships.len());
    for relationship in all_relationships {
      let guard = get_execution_guard(&self.execution_guards, &relationship.id).await;
      let executing = guard.try_lock().is_err();
      let snapshot = health.get(&relationship.remote_connection_id);
      let connection_health = snapshot
        .map(|snapshot| snapshot.status)
        .unwrap_or(HealthStatus::Unknown);
      let connection_healthy = connection_health == HealthStatus::Up;
      statuses.push(SyncRunnerStatus {
        relationship_id: relationship.id.clone(),
        relationship_name: relationship.name.clone(),
        remote_connection_id: relationship.remote_connection_id.clone(),
        running: running.contains_key(&relationship.id),
        executing,
        syncing: executing && connection_healthy,
        connection_health,
        connection_healthy,
        connection_checked_at: snapshot.map(|snapshot| snapshot.checked_at),
        connection_message: snapshot.and_then(|snapshot| snapshot.message.clone()),
      });
    }

    statuses
  }

  /// Check if a specific relationship's sync is running.
  pub async fn is_running(&self, relationship_id: &str) -> bool {
    self.running.lock().await.contains_key(relationship_id)
  }

  /// Stop all running syncs.
  pub async fn stop_all(&self) {
    let mut running = self.running.lock().await;
    let syncs: Vec<(String, RunningSync)> = running.drain().collect();
    drop(running);

    for (id, sync) in syncs {
      tracing::info!("stopping sync for '{}' ({})", sync.relationship_name, id);
      abort_sync_task(id, sync).await;
    }
  }

  /// Start all enabled relationships.
  pub async fn start_all_enabled(&self) {
    let relationship_manager = RelationshipManager::new(&self.config);
    for relationship in relationship_manager.list().await.unwrap_or_default() {
      if relationship.enabled {
        if let Err(error) = self.start(&relationship.id).await {
          tracing::warn!(
            "failed to start sync for '{}': {}",
            relationship.name,
            error
          );
        }
      }
    }
  }

  /// Start enabled relationships only when the user's startup setting allows
  /// it. Manual resume paths intentionally call `start_all_enabled()` directly.
  pub async fn start_all_enabled_if_configured(&self) {
    self
      .start_all_enabled_if_configured_after(STARTUP_SYNC_DELAY)
      .await;
  }

  async fn start_all_enabled_if_configured_after(&self, delay: Duration) {
    let auto_start_enabled = match self.config.get().await {
      Ok(config) => config.settings.auto_start_sync,
      Err(error) => {
        tracing::warn!("failed to read sync auto-start setting: {}", error);
        false
      }
    };
    if !auto_start_enabled {
      tracing::info!("sync auto-start is disabled; not starting sync relationships");
      return;
    }

    tracing::info!(
      "sync auto-start is enabled; waiting {:?} before starting enabled sync relationships",
      delay,
    );
    tokio::time::sleep(delay).await;

    match self.config.get().await {
      Ok(config) if config.settings.auto_start_sync => self.start_all_enabled().await,
      Ok(_) => tracing::info!(
        "sync auto-start was disabled during startup delay; not starting sync relationships"
      ),
      Err(error) => tracing::warn!(
        "failed to read sync auto-start setting after delay: {}",
        error
      ),
    }
  }
}

async fn abort_sync_task(relationship_id: String, sync: RunningSync) {
  let relationship_name = sync.relationship_name.clone();
  sync.handle.abort();
  match timeout(SYNC_TASK_STOP_TIMEOUT, sync.handle).await {
    Ok(Ok(())) => {}
    Ok(Err(error)) if error.is_cancelled() => {}
    Ok(Err(error)) => {
      tracing::warn!(
        "sync task for '{}' ({}) ended with error during shutdown: {}",
        relationship_name,
        relationship_id,
        error,
      );
    }
    Err(_) => {
      tracing::warn!(
        "sync task for '{}' ({}) did not stop within {:?}",
        relationship_name,
        relationship_id,
        SYNC_TASK_STOP_TIMEOUT,
      );
    }
  }
}

async fn get_execution_guard(
  execution_guards: &SyncExecutionGuards,
  relationship_id: &str,
) -> Arc<Mutex<()>> {
  let mut guards = execution_guards.lock().await;
  guards
    .entry(relationship_id.to_string())
    .or_insert_with(|| Arc::new(Mutex::new(())))
    .clone()
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
  event_tx: broadcast::Sender<crate::server::ServerEvent>,
  jwt_cache: crate::jwt_cache::JwtCache,
  execution_guards: SyncExecutionGuards,
) {
  let direction = relationship.direction.clone();
  let filter = relationship.filter.clone();
  let relationship_manager = RelationshipManager::new(&config);

  tracing::info!(
    "sync loop active for '{}' ({:?})",
    relationship.name,
    direction
  );

  let mut transient_retry_timer = Box::pin(tokio::time::sleep(TRANSIENT_SYNC_RETRY_INITIAL_DELAY));
  let mut transient_retry_armed = false;
  let mut transient_retry_delay = TRANSIENT_SYNC_RETRY_INITIAL_DELAY;

  // --- Step 1: Initial full sync (push + pull based on direction) ---
  let all_relationships = relationship_manager.list().await.unwrap_or_default();
  let execution_guard = get_execution_guard(&execution_guards, &relationship.id).await;
  let sync_result = {
    let _sync_guard = execution_guard.lock().await;
    let progress =
      PushProgressReporter::new(&relationship.id, &relationship.name, &activity, &event_tx);
    sync_relationship(
      &state,
      &connection,
      &relationship,
      &all_relationships,
      &http_client,
      &jwt_cache,
      startup_push_scan_mode(),
      Some(&progress),
    )
    .await
  };
  match sync_result {
    Ok(result) => {
      transient_retry_delay = TRANSIENT_SYNC_RETRY_INITIAL_DELAY;
      transient_retry_armed = false;
      log_sync_result(&relationship.name, &result);
      if let Err(error) = activity.log_full_sync(&relationship.id, &relationship.name, &result) {
        tracing::warn!(
          "failed to log sync activity for '{}': {}",
          relationship.name,
          error
        );
      }
      broadcast_full_sync(&event_tx, &relationship.id, &relationship.name, &result);
    }
    Err(error) => {
      let retry_delay = arm_transient_retry(
        &relationship.name,
        &error,
        &mut transient_retry_timer,
        &mut transient_retry_armed,
        &mut transient_retry_delay,
      );
      tracing::error!("initial sync failed for '{}': {}", relationship.name, error);
      let error_message = sync_error_activity_message(&error, retry_delay);
      if let Err(log_error) =
        activity.log_error(&relationship.id, &relationship.name, &error_message)
      {
        tracing::warn!(
          "failed to log error activity for '{}': {}",
          relationship.name,
          log_error
        );
      }
      broadcast_error(
        &event_tx,
        &relationship.id,
        &relationship.name,
        &error_message,
      );
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
        tracing::error!(
          "failed to start watcher for '{}': {}",
          relationship.name,
          error
        );
      }
    }
  }

  // SSE listener for pull-capable directions.
  if direction == SyncDirection::PullOnly || direction == SyncDirection::Bidirectional {
    let path_prefixes = vec![relationship.remote_path.clone()];
    sse_receiver = Some(start_sse_listener(connection.clone(), path_prefixes));
    tracing::info!("SSE listener started for '{}'", relationship.name);
  }

  let periodic_safety_net_interval_seconds = periodic_safety_net_interval_seconds(
    sync_interval_seconds,
    fs_receiver.is_some(),
    sse_receiver.is_some(),
  );

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

        // Push local changes to the remote. Refetch the relationship
        // list each cycle so nested-sync exclusions reflect the user's
        // latest config (they may have added or removed a child sync).
        let all_relationships = relationship_manager.list().await.unwrap_or_default();
        let execution_guard = get_execution_guard(&execution_guards, &relationship.id).await;
        let push_result = {
          let _sync_guard = execution_guard.lock().await;
          let progress = PushProgressReporter::new(
            &relationship.id,
            &relationship.name,
            &activity,
            &event_tx,
          );
          push_sync(
            &state,
            &connection,
            &relationship,
            &all_relationships,
            &http_client,
            &jwt_cache,
            PushScanMode::Lite,
            Some(&progress),
          ).await
        };
        match push_result {
          Ok(result) => {
            transient_retry_delay = TRANSIENT_SYNC_RETRY_INITIAL_DELAY;
            transient_retry_armed = false;
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
            broadcast_push(&event_tx, &relationship.id, &relationship.name, &result);
          }
          Err(error) => {
            let retry_delay = arm_transient_retry(
              &relationship.name,
              &error,
              &mut transient_retry_timer,
              &mut transient_retry_armed,
              &mut transient_retry_delay,
            );
            tracing::error!("push failed for '{}': {}", relationship.name, error);
            let error_message = sync_error_activity_message(&error, retry_delay);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error_message) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
            broadcast_error(&event_tx, &relationship.id, &relationship.name, &error_message);
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
        let all_relationships = relationship_manager.list().await.unwrap_or_default();
        let execution_guard = get_execution_guard(&execution_guards, &relationship.id).await;
        let pull_result = {
          let _sync_guard = execution_guard.lock().await;
          pull_sync(&state, &connection, &relationship, &all_relationships, &http_client, &jwt_cache).await
        };
        match pull_result {
          Ok(result) => {
            transient_retry_delay = TRANSIENT_SYNC_RETRY_INITIAL_DELAY;
            transient_retry_armed = false;
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
            broadcast_pull(&event_tx, &relationship.id, &relationship.name, &result);
          }
          Err(error) => {
            let retry_delay = arm_transient_retry(
              &relationship.name,
              &error,
              &mut transient_retry_timer,
              &mut transient_retry_armed,
              &mut transient_retry_delay,
            );
            tracing::error!("pull failed for '{}': {}", relationship.name, error);
            let error_message = sync_error_activity_message(&error, retry_delay);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error_message) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
            broadcast_error(&event_tx, &relationship.id, &relationship.name, &error_message);
          }
        }
      }

      // Short retry after a known transient upstream outage. This is separate
      // from the hourly watched safety net so a server restart or dropped
      // commit request does not leave the user staring at stale activity for
      // nearly an hour.
      _ = &mut transient_retry_timer, if transient_retry_armed => {
        transient_retry_armed = false;
        tracing::info!("retrying sync for '{}' after transient remote issue", relationship.name);

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
            tracing::warn!("connection for '{}' not found, skipping transient retry", relationship.name);
            continue;
          }
        };

        let all_relationships = relationship_manager.list().await.unwrap_or_default();
        let execution_guard = get_execution_guard(&execution_guards, &relationship.id).await;
        let sync_result = {
          let _sync_guard = execution_guard.lock().await;
          let progress = PushProgressReporter::new(
            &current_relationship.id,
            &current_relationship.name,
            &activity,
            &event_tx,
          );
          sync_relationship(
            &state,
            &current_connection,
            &current_relationship,
            &all_relationships,
            &http_client,
            &jwt_cache,
            PushScanMode::Lite,
            Some(&progress),
          ).await
        };
        match sync_result {
          Ok(result) => {
            transient_retry_delay = TRANSIENT_SYNC_RETRY_INITIAL_DELAY;
            log_sync_result(&relationship.name, &result);
            if let Err(error) = activity.log_full_sync(&relationship.id, &relationship.name, &result) {
              tracing::warn!("failed to log sync activity for '{}': {}", relationship.name, error);
            }
            broadcast_full_sync(&event_tx, &relationship.id, &relationship.name, &result);
          }
          Err(error) => {
            let retry_delay = arm_transient_retry(
              &relationship.name,
              &error,
              &mut transient_retry_timer,
              &mut transient_retry_armed,
              &mut transient_retry_delay,
            );
            tracing::error!("transient retry sync failed for '{}': {}", relationship.name, error);
            let error_message = sync_error_activity_message(&error, retry_delay);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error_message) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
            broadcast_error(&event_tx, &relationship.id, &relationship.name, &error_message);
          }
        }
      }

      // Periodic safety net -- Lite sync at a bounded interval. Event-backed
      // relationships rely on watchers/SSE for immediacy; this is only a
      // missed-event safety net.
      _ = tokio::time::sleep(std::time::Duration::from_secs(periodic_safety_net_interval_seconds)) => {
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

        let all_relationships = relationship_manager.list().await.unwrap_or_default();
        let execution_guard = get_execution_guard(&execution_guards, &relationship.id).await;
        let sync_result = {
          let _sync_guard = execution_guard.lock().await;
          let progress = PushProgressReporter::new(
            &current_relationship.id,
            &current_relationship.name,
            &activity,
            &event_tx,
          );
          sync_relationship(
            &state,
            &current_connection,
            &current_relationship,
            &all_relationships,
            &http_client,
            &jwt_cache,
            PushScanMode::Lite,
            Some(&progress),
          ).await
        };
        match sync_result {
          Ok(result) => {
            transient_retry_delay = TRANSIENT_SYNC_RETRY_INITIAL_DELAY;
            transient_retry_armed = false;
            log_sync_result(&relationship.name, &result);
            if let Err(error) = activity.log_full_sync(&relationship.id, &relationship.name, &result) {
              tracing::warn!("failed to log sync activity for '{}': {}", relationship.name, error);
            }
            broadcast_full_sync(&event_tx, &relationship.id, &relationship.name, &result);
          }
          Err(error) => {
            let retry_delay = arm_transient_retry(
              &relationship.name,
              &error,
              &mut transient_retry_timer,
              &mut transient_retry_armed,
              &mut transient_retry_delay,
            );
            tracing::error!("periodic sync failed for '{}': {}", relationship.name, error);
            let error_message = sync_error_activity_message(&error, retry_delay);
            if let Err(log_error) = activity.log_error(&relationship.id, &relationship.name, &error_message) {
              tracing::warn!("failed to log error activity for '{}': {}", relationship.name, log_error);
            }
            broadcast_error(&event_tx, &relationship.id, &relationship.name, &error_message);
          }
        }
      }
    }
  }
}

fn arm_transient_retry(
  relationship_name: &str,
  error: &ClientError,
  retry_timer: &mut Pin<Box<Sleep>>,
  retry_armed: &mut bool,
  retry_delay: &mut Duration,
) -> Option<Duration> {
  let delay = transient_sync_retry_delay(error, *retry_delay)?;
  retry_timer.as_mut().reset(Instant::now() + delay);
  *retry_armed = true;
  *retry_delay = next_transient_sync_retry_delay(delay);
  tracing::warn!(
    "sync for '{}' will retry in {} after transient remote issue: {}",
    relationship_name,
    format_retry_delay(delay),
    error,
  );
  Some(delay)
}

fn transient_sync_retry_delay(error: &ClientError, current_delay: Duration) -> Option<Duration> {
  if error.is_transient_upstream() {
    Some(current_delay.min(TRANSIENT_SYNC_RETRY_MAX_DELAY))
  } else {
    None
  }
}

fn next_transient_sync_retry_delay(current_delay: Duration) -> Duration {
  current_delay
    .saturating_mul(2)
    .min(TRANSIENT_SYNC_RETRY_MAX_DELAY)
}

fn sync_error_activity_message(error: &ClientError, retry_delay: Option<Duration>) -> String {
  match retry_delay {
    Some(delay) => format!(
      "Temporary remote issue; retrying sync in {}: {}",
      format_retry_delay(delay),
      error
    ),
    None => error.to_string(),
  }
}

fn format_retry_delay(delay: Duration) -> String {
  let total_seconds = delay.as_secs();
  if total_seconds < 60 {
    return format!("{}s", total_seconds);
  }

  let minutes = total_seconds / 60;
  let seconds = total_seconds % 60;
  if seconds == 0 {
    format!("{}m", minutes)
  } else {
    format!("{}m {:02}s", minutes, seconds)
  }
}

fn startup_push_scan_mode() -> PushScanMode {
  // Startup should resume politely. A Lite scan still hashes/checks files that
  // have no metadata, so true first-run syncs are covered, but existing large
  // relationships avoid an unconditional boot-time full rescan.
  PushScanMode::Lite
}

fn periodic_safety_net_interval_seconds(
  configured_seconds: u64,
  fs_watcher_active: bool,
  sse_listener_active: bool,
) -> u64 {
  if fs_watcher_active || sse_listener_active {
    return configured_seconds.max(WATCHED_SAFETY_NET_MIN_SECONDS);
  }

  configured_seconds
}

/// Broadcast a SyncEvent as JSON over the event channel.
fn broadcast_event(
  event_tx: &broadcast::Sender<crate::server::ServerEvent>,
  event: &crate::sync::activity::SyncEvent,
) {
  let json = serde_json::to_string(event).unwrap_or_default();
  let _ = event_tx.send(crate::server::ServerEvent::new("sync_activity", json));
}

fn broadcast_push(
  event_tx: &broadcast::Sender<crate::server::ServerEvent>,
  relationship_id: &str,
  relationship_name: &str,
  result: &crate::sync::push::PushResult,
) {
  let summary = crate::sync::activity::summarize_push_result(result);
  let event = crate::sync::activity::SyncEvent {
    id: uuid::Uuid::new_v4().to_string(),
    relationship_id: relationship_id.to_string(),
    relationship_name: relationship_name.to_string(),
    event_type: "push".to_string(),
    summary,
    files_affected: result.files_pushed + result.files_deleted,
    bytes_transferred: result.total_bytes,
    duration_ms: result.duration_ms,
    errors: result.errors.clone(),
    progress_percent: None,
    timestamp: chrono::Utc::now().timestamp_millis(),
  };
  broadcast_event(event_tx, &event);
}

fn broadcast_pull(
  event_tx: &broadcast::Sender<crate::server::ServerEvent>,
  relationship_id: &str,
  relationship_name: &str,
  result: &crate::sync::pull::PullResult,
) {
  let summary = crate::sync::activity::summarize_pull_result(result);
  let event = crate::sync::activity::SyncEvent {
    id: uuid::Uuid::new_v4().to_string(),
    relationship_id: relationship_id.to_string(),
    relationship_name: relationship_name.to_string(),
    event_type: "pull".to_string(),
    summary,
    files_affected: result.files_pulled + result.files_deleted + result.symlinks_pulled,
    bytes_transferred: result.total_bytes,
    duration_ms: result.duration_ms,
    errors: result.errors.clone(),
    progress_percent: None,
    timestamp: chrono::Utc::now().timestamp_millis(),
  };
  broadcast_event(event_tx, &event);
}

fn broadcast_full_sync(
  event_tx: &broadcast::Sender<crate::server::ServerEvent>,
  relationship_id: &str,
  relationship_name: &str,
  result: &crate::sync::replication::SyncResult,
) {
  let mut files_affected: u64 = 0;
  let mut bytes_transferred: u64 = 0;
  let mut duration_ms: u64 = 0;
  let mut errors: Vec<String> = Vec::new();

  if let Some(ref pull) = result.pull {
    files_affected += pull.files_pulled + pull.files_deleted + pull.symlinks_pulled;
    bytes_transferred += pull.total_bytes;
    duration_ms += pull.duration_ms;
    errors.extend(pull.errors.iter().cloned());
  }

  if let Some(ref push) = result.push {
    files_affected += push.files_pushed + push.files_deleted;
    bytes_transferred += push.total_bytes;
    duration_ms += push.duration_ms;
    errors.extend(push.errors.iter().cloned());
  }

  let summary = crate::sync::activity::summarize_full_sync_result(result);

  let event = crate::sync::activity::SyncEvent {
    id: uuid::Uuid::new_v4().to_string(),
    relationship_id: relationship_id.to_string(),
    relationship_name: relationship_name.to_string(),
    event_type: "full_sync".to_string(),
    summary,
    files_affected,
    bytes_transferred,
    duration_ms,
    errors,
    progress_percent: None,
    timestamp: chrono::Utc::now().timestamp_millis(),
  };
  broadcast_event(event_tx, &event);
}

fn broadcast_error(
  event_tx: &broadcast::Sender<crate::server::ServerEvent>,
  relationship_id: &str,
  relationship_name: &str,
  error_message: &str,
) {
  let event = crate::sync::activity::SyncEvent {
    id: uuid::Uuid::new_v4().to_string(),
    relationship_id: relationship_id.to_string(),
    relationship_name: relationship_name.to_string(),
    event_type: "error".to_string(),
    summary: error_message.to_string(),
    files_affected: 0,
    bytes_transferred: 0,
    duration_ms: 0,
    errors: vec![error_message.to_string()],
    progress_percent: None,
    timestamp: chrono::Utc::now().timestamp_millis(),
  };
  broadcast_event(event_tx, &event);
}

/// Log the results of a full sync_relationship call.
fn log_sync_result(name: &str, result: &crate::sync::replication::SyncResult) {
  tracing::info!(
    "sync completed for '{}': {}",
    name,
    crate::sync::activity::summarize_full_sync_result(result),
  );

  if let Some(ref pull) = result.pull {
    if pull.files_pulled > 0 || pull.files_deleted > 0 || pull.files_failed > 0 {
      tracing::info!(
        "pull for '{}': pulled={}, deleted={}, skipped={}, failed={}",
        name,
        pull.files_pulled,
        pull.files_deleted,
        pull.files_skipped,
        pull.files_failed,
      );
    }
  }

  if let Some(ref push) = result.push {
    if push.files_pushed > 0 || push.files_deleted > 0 || push.files_failed > 0 {
      tracing::info!(
        "push for '{}': pushed={}, deleted={}, skipped={}, failed={}",
        name,
        push.files_pushed,
        push.files_deleted,
        push.files_skipped,
        push.files_failed,
      );
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn execution_guard_serializes_same_relationship() {
    let guards: SyncExecutionGuards = Arc::new(Mutex::new(HashMap::new()));
    let first = get_execution_guard(&guards, "rel-1").await;
    let second = get_execution_guard(&guards, "rel-1").await;

    let _held = first.try_lock().expect("first lock should be available");
    assert!(
      second.try_lock().is_err(),
      "second lock attempt for same relationship should be rejected while held",
    );
  }

  #[tokio::test]
  async fn execution_guard_allows_different_relationships() {
    let guards: SyncExecutionGuards = Arc::new(Mutex::new(HashMap::new()));
    let first = get_execution_guard(&guards, "rel-1").await;
    let second = get_execution_guard(&guards, "rel-2").await;

    let _first = first
      .try_lock()
      .expect("first relationship lock should be available");
    let _second = second
      .try_lock()
      .expect("different relationship lock should be available");
  }

  #[test]
  fn watched_push_only_safety_net_uses_hourly_floor() {
    assert_eq!(
      periodic_safety_net_interval_seconds(60, true, false),
      WATCHED_SAFETY_NET_MIN_SECONDS,
    );
  }

  #[test]
  fn push_only_without_watcher_keeps_configured_safety_net() {
    assert_eq!(periodic_safety_net_interval_seconds(60, false, false), 60,);
  }

  #[test]
  fn event_backed_safety_net_uses_hourly_floor_for_remote_or_bidirectional_watchers() {
    assert_eq!(
      periodic_safety_net_interval_seconds(60, false, true),
      WATCHED_SAFETY_NET_MIN_SECONDS,
    );
    assert_eq!(
      periodic_safety_net_interval_seconds(60, true, true),
      WATCHED_SAFETY_NET_MIN_SECONDS,
    );
  }

  #[test]
  fn startup_sync_uses_lite_scan_policy() {
    assert_eq!(startup_push_scan_mode(), PushScanMode::Lite);
  }

  #[test]
  fn startup_sync_delay_is_one_minute() {
    assert_eq!(STARTUP_SYNC_DELAY, Duration::from_secs(60));
  }

  #[test]
  fn transient_upstream_errors_get_short_retry_even_with_hourly_safety_net() {
    let error = ClientError::UpstreamServer {
      status: 503,
      message: "database opening".to_string(),
    };

    assert_eq!(
      periodic_safety_net_interval_seconds(60, true, false),
      WATCHED_SAFETY_NET_MIN_SECONDS,
    );
    assert_eq!(
      transient_sync_retry_delay(&error, TRANSIENT_SYNC_RETRY_INITIAL_DELAY),
      Some(Duration::from_secs(60)),
    );
    assert_eq!(
      sync_error_activity_message(
        &error,
        transient_sync_retry_delay(&error, TRANSIENT_SYNC_RETRY_INITIAL_DELAY),
      ),
      "Temporary remote issue; retrying sync in 1m: upstream server error (HTTP 503): database opening",
    );
  }

  #[test]
  fn transient_retry_backoff_caps_at_fifteen_minutes() {
    assert_eq!(
      next_transient_sync_retry_delay(Duration::from_secs(60)),
      Duration::from_secs(120),
    );
    assert_eq!(
      next_transient_sync_retry_delay(Duration::from_secs(10 * 60)),
      TRANSIENT_SYNC_RETRY_MAX_DELAY,
    );
    assert_eq!(
      next_transient_sync_retry_delay(TRANSIENT_SYNC_RETRY_MAX_DELAY),
      TRANSIENT_SYNC_RETRY_MAX_DELAY,
    );
  }

  #[test]
  fn non_transient_sync_errors_do_not_arm_short_retry() {
    let error = ClientError::UpstreamRejected {
      status: 401,
      message: "expired token".to_string(),
    };

    assert_eq!(
      transient_sync_retry_delay(&error, TRANSIENT_SYNC_RETRY_INITIAL_DELAY),
      None,
    );
    assert_eq!(
      sync_error_activity_message(&error, None),
      "upstream rejected (HTTP 401): expired token",
    );
  }
}
