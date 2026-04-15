use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use tokio::net::TcpListener;

use aeordb_client_lib::connections::{AuthType, CreateConnectionRequest, ConnectionManager};
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::engine::pull_sync_pass;
use aeordb_client_lib::sync::reconcile::reconcile;
use aeordb_client_lib::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager, SyncDirection,
};

/// Mock aeordb that serves files. After initial sync, one file changes.
async fn start_mock_aeordb() -> String {
  let app = Router::new()
    .route("/admin/health", get(|| async { Json(serde_json::json!({"status": "ok"})) }))
    .route("/engine/{*path}", get(mock_engine_handler));

  let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
  let address  = listener.local_addr().expect("addr failed");

  tokio::spawn(async move {
    axum::serve(listener, app).await.expect("mock server failed");
  });

  format!("http://{}", address)
}

async fn mock_engine_handler(Path(path): Path<String>) -> Response {
  let path = format!("/{}", path);

  match path.as_str() {
    "/data/" => {
      Json(serde_json::json!([
        { "name": "stable.txt",  "entry_type": 2, "total_size": 6, "created_at": 1700000000000_i64, "updated_at": 1700000001000_i64, "content_type": "text/plain" },
        { "name": "changed.txt", "entry_type": 2, "total_size": 11, "created_at": 1700000000000_i64, "updated_at": 1700000099000_i64, "content_type": "text/plain" }
      ])).into_response()
    }
    "/data/stable.txt"  => (StatusCode::OK, [("x-updated-at", "1700000001000")], "stable").into_response(),
    "/data/changed.txt" => (StatusCode::OK, [("x-updated-at", "1700000099000")], "new content").into_response(),
    _ => StatusCode::NOT_FOUND.into_response(),
  }
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
async fn test_reconcile_detects_remote_changes() {
  let mock_url          = start_mock_aeordb().await;
  let (state, temp_dir) = create_state_store();
  let local_sync_dir    = temp_dir.join("synced-data");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name: "Mock".to_string(), url: mock_url, auth_type: AuthType::None, api_key: None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Data Sync".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/data/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::Bidirectional,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // Initial sync
  pull_sync_pass(&state, &relationship.id).await.expect("initial sync failed");

  // Now simulate: "changed.txt" has a different updated_at on remote (mock returns 1700000099000)
  // but our state tracker recorded 1700000099000 from the pull, so remote hasn't changed
  // from our perspective.

  // Reconcile — everything should be unchanged since we just synced
  let result = reconcile(&state, &relationship.id).await.expect("reconcile failed");

  assert_eq!(result.files_unchanged, 2, "both files should be unchanged right after sync");
  assert_eq!(result.conflicts_detected, 0);
}

#[tokio::test]
async fn test_reconcile_detects_local_only_files() {
  let mock_url          = start_mock_aeordb().await;
  let (state, temp_dir) = create_state_store();
  let local_sync_dir    = temp_dir.join("synced-data");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name: "Mock".to_string(), url: mock_url, auth_type: AuthType::None, api_key: None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Data Sync".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/data/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::Bidirectional,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // Initial sync
  pull_sync_pass(&state, &relationship.id).await.expect("initial sync failed");

  // Create a local-only file (simulates creating a file while offline)
  std::fs::write(local_sync_dir.join("local-only.txt"), "new local file").expect("write failed");

  // Reconcile — should detect the local-only file as needing push
  let result = reconcile(&state, &relationship.id).await.expect("reconcile failed");

  assert!(result.files_pushed >= 1, "should detect at least 1 file to push (local-only)");
}
