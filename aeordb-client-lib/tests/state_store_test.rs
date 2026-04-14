use aeordb_client_lib::state::StateStore;
use serde::{Deserialize, Serialize};

fn temp_database_path() -> String {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  temp_dir
    .keep()
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string()
}

#[test]
fn test_open_or_create_new_database() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");

  // Should have the directory structure
  assert!(store.exists("/client/").expect("exists check failed"));
  assert!(store.exists("/connections/").expect("exists check failed"));
  assert!(store.exists("/sync/").expect("exists check failed"));
  assert!(store.exists("/sync/relationships/").expect("exists check failed"));
  assert!(store.exists("/sync/state/").expect("exists check failed"));
  assert!(store.exists("/sync/conflicts/").expect("exists check failed"));
  assert!(store.exists("/settings/").expect("exists check failed"));
}

#[test]
fn test_reopen_existing_database() {
  let path = temp_database_path();

  // Create and write something
  {
    let store = StateStore::open_or_create(&path).expect("failed to create");
    store.store_json("/settings/test.json", &serde_json::json!({"key": "value"}))
      .expect("failed to store");
  }

  // Reopen and verify data persisted
  {
    let store = StateStore::open_or_create(&path).expect("failed to reopen");
    let value: Option<serde_json::Value> = store.read_json("/settings/test.json")
      .expect("failed to read");
    assert!(value.is_some());
    assert_eq!(value.unwrap()["key"], "value");
  }
}

#[test]
fn test_client_identity_generated_on_first_run() {
  let path     = temp_database_path();
  let store    = StateStore::open_or_create(&path).expect("failed to create");
  let identity = store.get_or_create_identity().expect("failed to get identity");

  assert!(!identity.id.is_empty());
  assert!(!identity.name.is_empty());

  // Verify it's a valid UUID
  uuid::Uuid::parse_str(&identity.id).expect("should be a valid UUID");
}

#[test]
fn test_client_identity_stable_across_calls() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  let identity_1 = store.get_or_create_identity().expect("failed to get identity");
  let identity_2 = store.get_or_create_identity().expect("failed to get identity");

  assert_eq!(identity_1.id, identity_2.id);
  assert_eq!(identity_1.name, identity_2.name);
}

#[test]
fn test_client_identity_stable_across_reopens() {
  let path = temp_database_path();

  let first_id;
  {
    let store    = StateStore::open_or_create(&path).expect("failed to create");
    let identity = store.get_or_create_identity().expect("failed to get identity");
    first_id     = identity.id;
  }

  {
    let store    = StateStore::open_or_create(&path).expect("failed to reopen");
    let identity = store.get_or_create_identity().expect("failed to get identity");
    assert_eq!(identity.id, first_id);
  }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct TestData {
  name:  String,
  count: u64,
}

#[test]
fn test_store_and_read_json() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  let data = TestData { name: "hello".to_string(), count: 42 };
  store.store_json("/settings/test-data.json", &data)
    .expect("failed to store");

  let read_back: Option<TestData> = store.read_json("/settings/test-data.json")
    .expect("failed to read");

  assert_eq!(read_back, Some(data));
}

#[test]
fn test_read_json_nonexistent_returns_none() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  let result: Option<serde_json::Value> = store.read_json("/settings/nope.json")
    .expect("failed to read");

  assert!(result.is_none());
}

#[test]
fn test_exists_and_delete() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  store.store_json("/settings/deleteme.json", &serde_json::json!({"x": 1}))
    .expect("failed to store");

  assert!(store.exists("/settings/deleteme.json").expect("exists check failed"));

  store.delete("/settings/deleteme.json").expect("failed to delete");

  assert!(!store.exists("/settings/deleteme.json").expect("exists check failed"));
}

#[test]
fn test_list_directory() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  store.store_json("/settings/a.json", &serde_json::json!({}))
    .expect("failed to store");
  store.store_json("/settings/b.json", &serde_json::json!({}))
    .expect("failed to store");
  store.store_json("/settings/c.json", &serde_json::json!({}))
    .expect("failed to store");

  let mut entries = store.list_directory("/settings/").expect("failed to list");
  entries.sort();

  // .keep is from ensure_directory_structure, plus our 3 files
  assert!(entries.contains(&"a.json".to_string()));
  assert!(entries.contains(&"b.json".to_string()));
  assert!(entries.contains(&"c.json".to_string()));
}

#[test]
fn test_store_json_overwrites_existing() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  store.store_json("/settings/data.json", &serde_json::json!({"v": 1}))
    .expect("failed to store v1");

  store.store_json("/settings/data.json", &serde_json::json!({"v": 2}))
    .expect("failed to store v2");

  let value: Option<serde_json::Value> = store.read_json("/settings/data.json")
    .expect("failed to read");

  assert_eq!(value.unwrap()["v"], 2);
}
