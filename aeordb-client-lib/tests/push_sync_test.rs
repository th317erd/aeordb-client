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

  let app = Router::new()
    .route("/admin/health", get(mock_health))
    .route("/engine/{*path}", get(mock_get_handler).put(mock_put_handler))
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

  // Verify files landed on the mock server
  let store = storage.lock().unwrap();
  assert_eq!(
    String::from_utf8_lossy(store.get("/uploads/hello.txt").unwrap()),
    "Hello from local!",
  );
  assert_eq!(
    String::from_utf8_lossy(store.get("/uploads/data.json").unwrap()),
    r#"{"key": "value"}"#,
  );
  assert_eq!(
    String::from_utf8_lossy(store.get("/uploads/sub/nested.md").unwrap()),
    "# Nested",
  );
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

  // Verify the new content is on the server
  let store = storage.lock().unwrap();
  assert_eq!(
    String::from_utf8_lossy(store.get("/data/mutable.txt").unwrap()),
    "version 2",
  );
}
