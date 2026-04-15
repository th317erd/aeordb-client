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
use aeordb_client_lib::sync::engine::pull_sync_pass;
use aeordb_client_lib::sync::push::push_sync_pass;
use aeordb_client_lib::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager, SyncDirection,
};

/// Mock aeordb with a directory containing mixed file types.
async fn start_mock_aeordb() -> (String, Arc<Mutex<HashMap<String, Vec<u8>>>>) {
  let storage: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::new(Mutex::new(HashMap::new()));
  let storage_clone = storage.clone();

  let storage_for_upload = storage.clone();

  let app = Router::new()
    .route("/admin/health", get(|| async { Json(serde_json::json!({"status": "ok"})) }))
    .route("/engine/{*path}", get(mock_engine_get).put(mock_engine_put))
    .route("/upload/config", get(|| async {
      Json(serde_json::json!({
        "hash_algorithm": "blake3",
        "chunk_size": 262144,
        "chunk_hash_prefix": "chunk:"
      }))
    }))
    .route("/upload/check", axum::routing::post(|| async {
      Json(serde_json::json!({ "have": [], "needed": [] }))
    }))
    .route("/upload/chunks/{hash}", axum::routing::put(|| async { StatusCode::CREATED }))
    .route("/upload/commit", axum::routing::post({
      let storage = storage_for_upload;
      move |body: axum::extract::Json<serde_json::Value>| {
        let storage = storage.clone();
        async move {
          // Extract files from commit and store them
          if let Some(files) = body.get("files").and_then(|f| f.as_array()) {
            for file in files {
              if let Some(path) = file.get("path").and_then(|p| p.as_str()) {
                storage.lock().unwrap().insert(path.to_string(), vec![]);
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

async fn mock_engine_get(Path(path): Path<String>) -> Response {
  let path = format!("/{}", path);

  match path.as_str() {
    "/docs/" => {
      Json(serde_json::json!([
        { "name": "report.pdf",  "entry_type": 2, "total_size": 5, "created_at": 1700000000000_i64, "updated_at": 1700000001000_i64, "content_type": "application/pdf" },
        { "name": "readme.md",   "entry_type": 2, "total_size": 8, "created_at": 1700000000000_i64, "updated_at": 1700000002000_i64, "content_type": "text/markdown" },
        { "name": "notes.txt",   "entry_type": 2, "total_size": 5, "created_at": 1700000000000_i64, "updated_at": 1700000003000_i64, "content_type": "text/plain" },
        { "name": "cache.tmp",   "entry_type": 2, "total_size": 4, "created_at": 1700000000000_i64, "updated_at": 1700000004000_i64, "content_type": "application/octet-stream" },
        { "name": ".DS_Store",   "entry_type": 2, "total_size": 3, "created_at": 1700000000000_i64, "updated_at": 1700000005000_i64, "content_type": "application/octet-stream" }
      ])).into_response()
    }
    "/docs/report.pdf" => (StatusCode::OK, [("x-updated-at", "1700000001000")], "PDF!!").into_response(),
    "/docs/readme.md"  => (StatusCode::OK, [("x-updated-at", "1700000002000")], "# README").into_response(),
    "/docs/notes.txt"  => (StatusCode::OK, [("x-updated-at", "1700000003000")], "Notes").into_response(),
    "/docs/cache.tmp"  => (StatusCode::OK, [("x-updated-at", "1700000004000")], "tmp!").into_response(),
    "/docs/.DS_Store"  => (StatusCode::OK, [("x-updated-at", "1700000005000")], "mac").into_response(),
    _ => StatusCode::NOT_FOUND.into_response(),
  }
}

async fn mock_engine_put(
  State(storage): State<Arc<Mutex<HashMap<String, Vec<u8>>>>>,
  Path(path): Path<String>,
  body: Bytes,
) -> StatusCode {
  let path = format!("/{}", path);
  storage.lock().unwrap().insert(path, body.to_vec());
  StatusCode::OK
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
async fn test_pull_sync_with_include_filter() {
  let (mock_url, _storage) = start_mock_aeordb().await;
  let (state, temp_dir)    = create_state_store();
  let local_sync_dir       = temp_dir.join("filtered-pull");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name: "Mock".to_string(), url: mock_url, auth_type: AuthType::None, api_key: None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Filtered Pull".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/docs/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PullOnly,
    filter:               Some("*.pdf, *.md".to_string()),
    delete_propagation:   None,
  }).expect("create relationship failed");

  let result = pull_sync_pass(&state, &relationship.id).await.expect("sync failed");

  // Should download only PDFs and markdown (2 files), skip the rest (3 files)
  assert_eq!(result.files_downloaded, 2, "should download 2 filtered files");
  assert_eq!(result.files_skipped, 3, "should skip 3 non-matching files");

  assert!(local_sync_dir.join("report.pdf").exists());
  assert!(local_sync_dir.join("readme.md").exists());
  assert!(!local_sync_dir.join("notes.txt").exists());
  assert!(!local_sync_dir.join("cache.tmp").exists());
  assert!(!local_sync_dir.join(".DS_Store").exists());
}

#[tokio::test]
async fn test_pull_sync_with_exclude_filter() {
  let (mock_url, _storage) = start_mock_aeordb().await;
  let (state, temp_dir)    = create_state_store();
  let local_sync_dir       = temp_dir.join("exclude-pull");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name: "Mock".to_string(), url: mock_url, auth_type: AuthType::None, api_key: None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Exclude Pull".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/docs/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PullOnly,
    filter:               Some("!*.tmp, !.DS_Store".to_string()),
    delete_propagation:   None,
  }).expect("create relationship failed");

  let result = pull_sync_pass(&state, &relationship.id).await.expect("sync failed");

  // Should download 3 files (pdf, md, txt), skip 2 (tmp, .DS_Store)
  assert_eq!(result.files_downloaded, 3);
  assert_eq!(result.files_skipped, 2);

  assert!(local_sync_dir.join("report.pdf").exists());
  assert!(local_sync_dir.join("readme.md").exists());
  assert!(local_sync_dir.join("notes.txt").exists());
  assert!(!local_sync_dir.join("cache.tmp").exists());
  assert!(!local_sync_dir.join(".DS_Store").exists());
}

#[tokio::test]
async fn test_push_sync_with_filter() {
  let (mock_url, storage) = start_mock_aeordb().await;
  let (state, temp_dir)   = create_state_store();
  let local_sync_dir      = temp_dir.join("filtered-push");

  std::fs::create_dir_all(&local_sync_dir).expect("create dir failed");
  std::fs::write(local_sync_dir.join("report.pdf"), "PDF content").expect("write failed");
  std::fs::write(local_sync_dir.join("readme.md"), "# README").expect("write failed");
  std::fs::write(local_sync_dir.join("cache.tmp"), "temp data").expect("write failed");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name: "Mock".to_string(), url: mock_url, auth_type: AuthType::None, api_key: None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Filtered Push".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/uploads/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PushOnly,
    filter:               Some("!*.tmp".to_string()),
    delete_propagation:   None,
  }).expect("create relationship failed");

  let result = push_sync_pass(&state, &relationship.id).await.expect("push failed");

  // Should upload 2 files, skip the .tmp
  assert_eq!(result.files_uploaded, 2);
  assert_eq!(result.files_skipped, 1);

  let store = storage.lock().unwrap();
  assert!(store.contains_key("/uploads/report.pdf"));
  assert!(store.contains_key("/uploads/readme.md"));
  assert!(!store.contains_key("/uploads/cache.tmp"));
}
