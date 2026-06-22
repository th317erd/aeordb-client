use std::sync::Arc;

use aeordb_client_lib::config::ConfigStore;
use aeordb_client_lib::connections::{AuthType, RemoteConnection};
use aeordb_client_lib::health::{HealthSnapshot, HealthStatus, new_health_map};
use aeordb_client_lib::jwt_cache::JwtCache;
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::relationships::{DeletePropagation, SyncDirection, SyncRelationship};
use aeordb_client_lib::sync::runner::SyncRunner;
use chrono::Utc;
use std::time::Duration;
use tokio::sync::broadcast;

fn temp_paths() -> (std::path::PathBuf, std::path::PathBuf, tempfile::TempDir) {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  let data_path = temp_dir.path().join("state.aeordb");
  let config_path = temp_dir.path().join("config.yaml");
  (data_path, config_path, temp_dir)
}

#[tokio::test]
async fn runner_status_reports_waiting_when_connection_is_down() {
  let (data_path, config_path, _temp_dir) = temp_paths();
  let state = Arc::new(
    StateStore::open_or_create(data_path.to_str().expect("utf8 path")).expect("state store"),
  );
  let config = Arc::new(ConfigStore::load(&config_path).expect("config store"));

  let now = Utc::now();
  let connection = RemoteConnection {
    id: "conn-1".to_string(),
    name: "Down DB".to_string(),
    url: "http://127.0.0.1:1".to_string(),
    auth_type: AuthType::None,
    api_key: None,
    share_base_url: None,
    created_at: now,
    updated_at: now,
  };
  let relationship = SyncRelationship {
    id: "rel-1".to_string(),
    name: "Pictures".to_string(),
    remote_connection_id: connection.id.clone(),
    remote_path: "/Pictures/".to_string(),
    local_path: temp_path_string(_temp_dir.path().join("Pictures")),
    direction: SyncDirection::PushOnly,
    filter: None,
    delete_propagation: DeletePropagation::default(),
    enabled: true,
    created_at: now,
    updated_at: now,
  };

  config
    .update(|config| {
      config.connections.push(connection.clone());
      config.relationships.push(relationship.clone());
    })
    .await
    .expect("write config");

  let (event_tx, _) = broadcast::channel(8);
  let runner = SyncRunner::new(
    state,
    config,
    reqwest::Client::new(),
    event_tx,
    JwtCache::new(),
  );
  let health_map = new_health_map();
  health_map.lock().await.insert(
    connection.id.clone(),
    HealthSnapshot {
      connection_id: connection.id.clone(),
      status: HealthStatus::Down,
      latency_ms: None,
      message: Some("connection refused".to_string()),
      checked_at: Utc::now().timestamp_millis(),
    },
  );

  let guard = runner.execution_guard(&relationship.id).await;
  let _held = guard.lock().await;

  let statuses = runner.status(&health_map).await;
  let status = statuses
    .iter()
    .find(|status| status.relationship_id == relationship.id)
    .expect("relationship status");

  assert!(status.executing, "raw execution guard should be held");
  assert!(
    !status.syncing,
    "user-facing syncing must be false while the database is down"
  );
  assert_eq!(status.connection_health, HealthStatus::Down);
  assert!(!status.connection_healthy);
  assert_eq!(
    status.connection_message.as_deref(),
    Some("connection refused")
  );
}

#[tokio::test]
async fn runner_auto_start_respects_disabled_setting_without_delay() {
  let (data_path, config_path, _temp_dir) = temp_paths();
  let state = Arc::new(
    StateStore::open_or_create(data_path.to_str().expect("utf8 path")).expect("state store"),
  );
  let config = Arc::new(ConfigStore::load(&config_path).expect("config store"));

  let now = Utc::now();
  let connection = RemoteConnection {
    id: "conn-1".to_string(),
    name: "DB".to_string(),
    url: "http://127.0.0.1:1".to_string(),
    auth_type: AuthType::None,
    api_key: None,
    share_base_url: None,
    created_at: now,
    updated_at: now,
  };
  let relationship = SyncRelationship {
    id: "rel-1".to_string(),
    name: "Pictures".to_string(),
    remote_connection_id: connection.id.clone(),
    remote_path: "/Pictures/".to_string(),
    local_path: temp_path_string(_temp_dir.path().join("Pictures")),
    direction: SyncDirection::PushOnly,
    filter: None,
    delete_propagation: DeletePropagation::default(),
    enabled: true,
    created_at: now,
    updated_at: now,
  };

  config
    .update(|config| {
      config.settings.auto_start_sync = false;
      config.connections.push(connection);
      config.relationships.push(relationship.clone());
    })
    .await
    .expect("write config");

  let (event_tx, _) = broadcast::channel(8);
  let runner = SyncRunner::new(
    state,
    config,
    reqwest::Client::new(),
    event_tx,
    JwtCache::new(),
  );

  tokio::time::timeout(
    Duration::from_millis(100),
    runner.start_all_enabled_if_configured(),
  )
  .await
  .expect("disabled auto-start must not wait for the startup delay");

  assert!(
    !runner.is_running(&relationship.id).await,
    "disabled auto-start must not start enabled relationships",
  );
}

fn temp_path_string(path: std::path::PathBuf) -> String {
  path.to_string_lossy().to_string()
}
