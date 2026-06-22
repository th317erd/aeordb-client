//! Background connection-health pinger.
//!
//! Periodically calls `ConnectionManager::test_connection` on every
//! configured connection and broadcasts a `connection_health` SSE event
//! whenever a connection flips between `up` and `down`. The renderer
//! uses this to auto-refresh file-browser tabs that were stuck on an
//! "Cannot reach the server" banner when the engine was unreachable —
//! once the engine comes back, the next ping flips state, the SSE event
//! fires, and the affected tabs re-fetch without user action.
//!
//! Polling cadence is a fixed 10s for every connection regardless of
//! state. With a typical 1–5 connections that's at most ~30 outbound
//! pings/minute, which is well inside what the engine handles
//! comfortably (the call is a single GET /system/health). If a future
//! deployment ends up with dozens of connections, this can be split
//! into per-state intervals (faster when down, slower when up).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::config::ConfigStore;
use crate::connections::ConnectionManager;
use crate::server::ServerEvent;

const PING_INTERVAL: Duration = Duration::from_secs(10);

/// Public health status for a connection. `Unknown` is the bootstrap
/// state before the first ping completes — we don't broadcast it; the
/// first real flip is always to either `Up` or `Down`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
  Unknown,
  Up,
  Down,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthSnapshot {
  pub connection_id: String,
  pub status: HealthStatus,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub latency_ms: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub message: Option<String>,
  pub checked_at: i64, // unix millis
}

/// Shared map of `connection_id -> HealthSnapshot`. The pinger task
/// owns writes; readers (REST endpoint) just clone the snapshot.
pub type HealthMap = Arc<Mutex<HashMap<String, HealthSnapshot>>>;

pub fn new_health_map() -> HealthMap {
  Arc::new(Mutex::new(HashMap::new()))
}

/// Spawn the health pinger. Returns the JoinHandle so callers can hold
/// it for shutdown; in practice we let it run for the process lifetime.
pub fn start_health_pinger(
  config_store: Arc<ConfigStore>,
  event_tx: broadcast::Sender<ServerEvent>,
  health_map: HealthMap,
) -> JoinHandle<()> {
  tokio::spawn(async move {
    let mut interval = tokio::time::interval(PING_INTERVAL);

    tick(&config_store, &event_tx, &health_map).await;

    loop {
      interval.tick().await;
      tick(&config_store, &event_tx, &health_map).await;
    }
  })
}

async fn tick(
  config_store: &Arc<ConfigStore>,
  event_tx: &broadcast::Sender<ServerEvent>,
  health_map: &HealthMap,
) {
  let manager = ConnectionManager::new(config_store);
  let connections = match manager.list().await {
    Ok(c) => c,
    Err(e) => {
      tracing::warn!("health pinger: failed to list connections: {}", e);
      return;
    }
  };

  // Ping all connections in parallel; each test_connection has its own
  // 10s timeout so a single dead connection doesn't stall the others.
  let futures = connections.iter().map(|conn| {
    let id = conn.id.clone();
    let manager = ConnectionManager::new(config_store);
    async move {
      let result = manager.test_connection(&id).await;
      (id, result)
    }
  });
  let results = futures_util::future::join_all(futures).await;

  let now = chrono::Utc::now().timestamp_millis();
  let mut map = health_map.lock().await;

  // Drop entries for connections that no longer exist (e.g. user deleted
  // one between ticks).
  let current_ids: std::collections::HashSet<&str> =
    connections.iter().map(|c| c.id.as_str()).collect();
  map.retain(|id, _| current_ids.contains(id.as_str()));

  for (id, result) in results {
    let (status, latency_ms, message) = match result {
      Ok(test) => {
        if test.success {
          (HealthStatus::Up, test.latency_ms, None)
        } else {
          (HealthStatus::Down, test.latency_ms, Some(test.message))
        }
      }
      Err(err) => (HealthStatus::Down, None, Some(err.to_string())),
    };

    let snapshot = HealthSnapshot {
      connection_id: id.clone(),
      status,
      latency_ms,
      message,
      checked_at: now,
    };

    let prev_status = map.get(&id).map(|s| s.status);
    map.insert(id.clone(), snapshot.clone());

    // Only broadcast on transition. The initial Unknown → Up/Down
    // counts as a transition so the UI gets a real status on first
    // tick without needing a separate fetch.
    if prev_status != Some(status) {
      if let Ok(json) = serde_json::to_string(&snapshot) {
        let _ = event_tx.send(ServerEvent::new("connection_health", json));
      }
    }
  }
}
