use aeordb::engine::{list_conflicts_typed, RequestContext};

use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};
use aeordb_client_lib::state::StateStore;

fn create_state_store() -> (StateStore, std::path::PathBuf) {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let database_path = temp_dir
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string();

  let store = StateStore::open_or_create(&database_path).expect("failed to create state store");
  (store, temp_dir)
}

#[test]
fn test_list_conflicts_empty_database() {
  let (state, _temp_dir) = create_state_store();

  let conflicts = list_conflicts_typed(state.engine())
    .expect("list_conflicts_typed failed");

  assert!(conflicts.is_empty(), "new database should have no conflicts");
}

#[tokio::test]
async fn test_conflicts_http_api_list_empty() {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let data_path = temp_dir.join("test-state.aeordb");
  let config_path = temp_dir.join("config.yaml");

  let config = ServerConfig {
    host:        "127.0.0.1".to_string(),
    port:        0,
    config_path,
    data_path,
    auth_token:  None,
  };

  let (address, _handle) = start_server_with_handle(config)
    .await.expect("failed to start server");

  let base_url = format!("http://{}", address);
  let client   = reqwest::Client::new();

  // List conflicts -- should be empty
  let response = client.get(format!("{}/api/v1/conflicts", base_url))
    .send().await.expect("list failed");
  assert_eq!(response.status(), 200);

  let conflicts: Vec<serde_json::Value> = response.json().await.expect("parse failed");
  assert!(conflicts.is_empty());

  // Dismiss-all on empty queue should succeed
  let response = client.post(format!("{}/api/v1/conflicts/dismiss-all", base_url))
    .send().await.expect("dismiss-all failed");
  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert_eq!(body["dismissed_count"], 0);
}

#[tokio::test]
async fn test_conflicts_http_api_resolve_not_found() {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let data_path = temp_dir.join("test-state.aeordb");
  let config_path = temp_dir.join("config.yaml");

  let config = ServerConfig {
    host:        "127.0.0.1".to_string(),
    port:        0,
    config_path,
    data_path,
    auth_token:  None,
  };

  let (address, _handle) = start_server_with_handle(config)
    .await.expect("failed to start server");

  let base_url = format!("http://{}", address);
  let client   = reqwest::Client::new();

  // Try to resolve a nonexistent conflict
  let response = client.post(format!("{}/api/v1/conflicts/resolve", base_url))
    .json(&serde_json::json!({ "path": "/nonexistent/file.txt", "pick": "winner" }))
    .send().await.expect("resolve failed");

  // Should be 404 or 500 -- the conflict doesn't exist
  assert!(response.status().is_client_error() || response.status().is_server_error());
}
