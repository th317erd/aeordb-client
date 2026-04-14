use aeordb_client_lib::connections::{AuthType, CreateConnectionRequest};
use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::conflicts::{ConflictManager, ConflictRecord, ConflictResolution};

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
fn test_record_and_list_conflicts() {
  let (state, _temp_dir) = create_state_store();
  let manager            = ConflictManager::new(&state);

  manager.record_conflict(
    "/docs/readme.md",
    "rel-123",
    "local_hash_aaa",
    "remote_hash_bbb",
    "synced_hash_ccc",
    Some(1700000001000),
    Some(1700000002000),
  ).expect("record failed");

  manager.record_conflict(
    "/docs/notes.txt",
    "rel-123",
    "local_hash_ddd",
    "remote_hash_eee",
    "synced_hash_fff",
    None,
    None,
  ).expect("record failed");

  let conflicts = manager.list().expect("list failed");
  assert_eq!(conflicts.len(), 2);
}

#[test]
fn test_list_conflicts_for_relationship() {
  let (state, _temp_dir) = create_state_store();
  let manager            = ConflictManager::new(&state);

  manager.record_conflict("/a.txt", "rel-1", "a", "b", "c", None, None).expect("record failed");
  manager.record_conflict("/b.txt", "rel-2", "d", "e", "f", None, None).expect("record failed");
  manager.record_conflict("/c.txt", "rel-1", "g", "h", "i", None, None).expect("record failed");

  let for_rel1 = manager.list_for_relationship("rel-1").expect("list failed");
  assert_eq!(for_rel1.len(), 2);

  let for_rel2 = manager.list_for_relationship("rel-2").expect("list failed");
  assert_eq!(for_rel2.len(), 1);
}

#[test]
fn test_resolve_conflict() {
  let (state, _temp_dir) = create_state_store();
  let manager            = ConflictManager::new(&state);

  let conflict = manager.record_conflict(
    "/docs/readme.md", "rel-123", "a", "b", "c", None, None,
  ).expect("record failed");

  assert_eq!(manager.list().expect("list failed").len(), 1);

  manager.resolve(&conflict.id).expect("resolve failed");

  assert_eq!(manager.list().expect("list failed").len(), 0);
}

#[test]
fn test_resolve_all_conflicts() {
  let (state, _temp_dir) = create_state_store();
  let manager            = ConflictManager::new(&state);

  manager.record_conflict("/a.txt", "rel-1", "a", "b", "c", None, None).expect("record failed");
  manager.record_conflict("/b.txt", "rel-1", "d", "e", "f", None, None).expect("record failed");
  manager.record_conflict("/c.txt", "rel-1", "g", "h", "i", None, None).expect("record failed");

  let count = manager.resolve_all().expect("resolve_all failed");
  assert_eq!(count, 3);
  assert_eq!(manager.list().expect("list failed").len(), 0);
}

#[test]
fn test_has_conflict_for() {
  let (state, _temp_dir) = create_state_store();
  let manager            = ConflictManager::new(&state);

  assert!(!manager.has_conflict_for("rel-1", "/a.txt").expect("check failed"));

  manager.record_conflict("/a.txt", "rel-1", "a", "b", "c", None, None).expect("record failed");

  assert!(manager.has_conflict_for("rel-1", "/a.txt").expect("check failed"));
  assert!(!manager.has_conflict_for("rel-1", "/b.txt").expect("check failed"));
  assert!(!manager.has_conflict_for("rel-2", "/a.txt").expect("check failed"));
}

#[tokio::test]
async fn test_conflicts_http_api_list_empty() {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let database_path = temp_dir
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string();

  let config = ServerConfig {
    host:          "127.0.0.1".to_string(),
    port:          0,
    database_path,
    auth_token:    None,
  };

  let (address, _handle) = start_server_with_handle(config)
    .await.expect("failed to start server");

  let base_url = format!("http://{}", address);
  let client   = reqwest::Client::new();

  // List conflicts — should be empty
  let response = client.get(format!("{}/api/v1/conflicts", base_url))
    .send().await.expect("list failed");
  assert_eq!(response.status(), 200);

  let conflicts: Vec<ConflictRecord> = response.json().await.expect("parse failed");
  assert!(conflicts.is_empty());

  // Resolve-all on empty queue should succeed
  let response = client.post(format!("{}/api/v1/conflicts/resolve-all", base_url))
    .send().await.expect("resolve-all failed");
  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert_eq!(body["resolved_count"], 0);
}

#[tokio::test]
async fn test_conflicts_http_api_resolve_not_found() {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let database_path = temp_dir
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string();

  let config = ServerConfig {
    host:          "127.0.0.1".to_string(),
    port:          0,
    database_path,
    auth_token:    None,
  };

  let (address, _handle) = start_server_with_handle(config)
    .await.expect("failed to start server");

  let base_url = format!("http://{}", address);
  let client   = reqwest::Client::new();

  // Try to resolve a nonexistent conflict
  let response = client.post(format!("{}/api/v1/conflicts/nonexistent/resolve", base_url))
    .json(&serde_json::json!({ "resolution": "keep_local" }))
    .send().await.expect("resolve failed");

  assert_eq!(response.status(), 404);
}
