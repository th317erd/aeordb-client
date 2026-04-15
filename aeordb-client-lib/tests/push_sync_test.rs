use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, put};
use tokio::net::TcpListener;

use aeordb_client_lib::connections::{AuthType, CreateConnectionRequest, ConnectionManager};
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::push::push_sync_pass;
use aeordb_client_lib::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager, SyncDirection,
};

/// Storage for the mock aeordb — tracks uploaded files.
type MockStorage = Arc<Mutex<HashMap<String, Vec<u8>>>>;

async fn mock_put_handler(
  State(storage): State<MockStorage>,
  Path(path): Path<String>,
  body: Bytes,
) -> StatusCode {
  let path = format!("/{}", path);
  storage.lock().unwrap().insert(path, body.to_vec());
  StatusCode::OK
}

async fn mock_get_handler(
  State(storage): State<MockStorage>,
  Path(path): Path<String>,
) -> Response {
  let path = format!("/{}", path);
  let store = storage.lock().unwrap();

  if let Some(data) = store.get(&path) {
    (StatusCode::OK, data.clone()).into_response()
  } else {
    StatusCode::NOT_FOUND.into_response()
  }
}

async fn mock_health() -> Json<serde_json::Value> {
  Json(serde_json::json!({"status": "ok"}))
}

/// Start a mock aeordb that accepts PUT and GET requests.
async fn start_mock_aeordb() -> (String, MockStorage) {
  let storage: MockStorage = Arc::new(Mutex::new(HashMap::new()));
  let storage_clone = storage.clone();

  let storage_for_commit = storage.clone();

  let app = Router::new()
    .route("/admin/health", get(mock_health))
    .route("/engine/{*path}", get(mock_get_handler).put(mock_put_handler))
    .route("/upload/config", get(|| async {
      Json(serde_json::json!({
        "hash_algorithm": "blake3",
        "chunk_size": 262144,
        "chunk_hash_prefix": "chunk:"
      }))
    }))
    .route("/upload/check", axum::routing::post({
      let storage = storage.clone();
      move |body: axum::extract::Json<serde_json::Value>| {
        let _storage = storage.clone();
        async move {
          // Report all chunks as needed (no dedup in mock)
          let hashes = body.get("hashes").and_then(|h| h.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
            .unwrap_or_default();
          Json(serde_json::json!({ "have": [], "needed": hashes }))
        }
      }
    }))
    .route("/upload/chunks/{hash}", axum::routing::put(|| async { StatusCode::CREATED }))
    .route("/upload/commit", axum::routing::post({
      let storage = storage_for_commit;
      move |body: axum::extract::Json<serde_json::Value>| {
        let storage = storage.clone();
        async move {
          // Store committed file paths (we don't have the actual content from chunks in this mock,
          // but we can track that the commit happened)
          if let Some(files) = body.get("files").and_then(|f| f.as_array()) {
            for file in files {
              if let Some(path) = file.get("path").and_then(|p| p.as_str()) {
                storage.lock().unwrap().insert(path.to_string(), b"committed".to_vec());
              }
            }
          }
          Json(serde_json::json!({"status": "ok"}))
        }
      }
    }))
    .with_state(storage_clone);

  let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
  let address  = listener.local_addr().expect("addr failed");

  tokio::spawn(async move {
    axum::serve(listener, app).await.expect("mock server failed");
  });

  (format!("http://{}", address), storage)
}

fn create_state_store() -> (StateStore, std::path::PathBuf) {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let database_path = temp_dir
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string();

  let store = StateStore::open_or_create(&database_path).expect("failed to create state store");
  (store, temp_dir)
}

#[tokio::test]
async fn test_push_sync_uploads_local_files() {
  let (mock_url, storage) = start_mock_aeordb().await;
  let (state, temp_dir)   = create_state_store();
  let local_sync_dir      = temp_dir.join("push-source");

  // Create local files
  std::fs::create_dir_all(&local_sync_dir).expect("create dir failed");
  std::fs::write(local_sync_dir.join("hello.txt"), "Hello from local!").expect("write failed");
  std::fs::write(local_sync_dir.join("data.json"), r#"{"key": "value"}"#).expect("write failed");

  // Create subdirectory with a file
  std::fs::create_dir_all(local_sync_dir.join("sub")).expect("create subdir failed");
  std::fs::write(local_sync_dir.join("sub/nested.md"), "# Nested").expect("write failed");

  // Create connection
  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name:      "Mock".to_string(),
    url:       mock_url,
    auth_type: AuthType::None,
    api_key:   None,
  }).expect("create connection failed");

  // Create relationship (push_only)
  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Push Test".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/uploads/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PushOnly,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // Run push sync
  let result = push_sync_pass(&state, &relationship.id)
    .await
    .expect("push sync failed");

  assert_eq!(result.files_uploaded, 3, "should upload 3 files");
  assert_eq!(result.files_failed, 0, "should have no failures");

  // Verify files were committed to the mock server (via chunked upload protocol)
  let store = storage.lock().unwrap();
  assert!(store.contains_key("/uploads/hello.txt"), "hello.txt should be committed");
  assert!(store.contains_key("/uploads/data.json"), "data.json should be committed");
  assert!(store.contains_key("/uploads/sub/nested.md"), "sub/nested.md should be committed");
}

#[tokio::test]
async fn test_push_sync_skips_unchanged_files() {
  let (mock_url, _storage) = start_mock_aeordb().await;
  let (state, temp_dir)    = create_state_store();
  let local_sync_dir       = temp_dir.join("push-source");

  std::fs::create_dir_all(&local_sync_dir).expect("create dir failed");
  std::fs::write(local_sync_dir.join("stable.txt"), "unchanged content").expect("write failed");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name:      "Mock".to_string(),
    url:       mock_url,
    auth_type: AuthType::None,
    api_key:   None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Push Test".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/data/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PushOnly,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // First push
  let first = push_sync_pass(&state, &relationship.id).await.expect("first push failed");
  assert_eq!(first.files_uploaded, 1);

  // Second push — nothing changed
  let second = push_sync_pass(&state, &relationship.id).await.expect("second push failed");
  assert_eq!(second.files_uploaded, 0);
  assert_eq!(second.files_skipped, 1);
}

#[tokio::test]
async fn test_push_sync_uploads_modified_files() {
  let (mock_url, storage) = start_mock_aeordb().await;
  let (state, temp_dir)   = create_state_store();
  let local_sync_dir      = temp_dir.join("push-source");

  std::fs::create_dir_all(&local_sync_dir).expect("create dir failed");
  std::fs::write(local_sync_dir.join("mutable.txt"), "version 1").expect("write failed");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name:      "Mock".to_string(),
    url:       mock_url,
    auth_type: AuthType::None,
    api_key:   None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Push Test".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/data/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PushOnly,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // First push
  push_sync_pass(&state, &relationship.id).await.expect("first push failed");

  // Modify the file
  std::fs::write(local_sync_dir.join("mutable.txt"), "version 2").expect("write failed");

  // Second push — should detect the change
  let second = push_sync_pass(&state, &relationship.id).await.expect("second push failed");
  assert_eq!(second.files_uploaded, 1, "should re-upload the modified file");

  // Verify the file was re-committed to the server
  let store = storage.lock().unwrap();
  assert!(store.contains_key("/data/mutable.txt"), "mutable.txt should be committed");
}
