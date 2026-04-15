use axum::Router;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use tokio::net::TcpListener;

use aeordb_client_lib::connections::{AuthType, CreateConnectionRequest};
use aeordb_client_lib::connections::ConnectionManager;
use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::engine::pull_sync_pass;
use aeordb_client_lib::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager, SyncDirection,
};

/// Mock aeordb server that serves a simple directory structure.
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

/// Mock handler for GET /engine/{path}
/// Serves a fake directory with a couple of files.
async fn mock_engine_handler(Path(path): Path<String>) -> Response {
  let path = format!("/{}", path);

  match path.as_str() {
    "/docs/" => {
      // Directory listing
      Json(serde_json::json!([
        {
          "name": "readme.md",
          "entry_type": 2,
          "total_size": 13,
          "created_at": 1700000000000_i64,
          "updated_at": 1700000001000_i64,
          "content_type": "text/markdown"
        },
        {
          "name": "notes.txt",
          "entry_type": 2,
          "total_size": 11,
          "created_at": 1700000000000_i64,
          "updated_at": 1700000002000_i64,
          "content_type": "text/plain"
        },
        {
          "name": "sub",
          "entry_type": 3,
          "total_size": 0,
          "created_at": 1700000000000_i64,
          "updated_at": 1700000000000_i64,
          "content_type": null
        }
      ])).into_response()
    }
    "/docs/sub/" => {
      Json(serde_json::json!([
        {
          "name": "deep.txt",
          "entry_type": 2,
          "total_size": 14,
          "created_at": 1700000000000_i64,
          "updated_at": 1700000003000_i64,
          "content_type": "text/plain"
        }
      ])).into_response()
    }
    "/docs/readme.md" => {
      (
        StatusCode::OK,
        [
          ("x-path", "/docs/readme.md"),
          ("x-total-size", "13"),
          ("content-type", "text/markdown"),
          ("x-updated-at", "1700000001000"),
        ],
        "Hello, World!",
      ).into_response()
    }
    "/docs/notes.txt" => {
      (
        StatusCode::OK,
        [
          ("x-path", "/docs/notes.txt"),
          ("x-total-size", "11"),
          ("content-type", "text/plain"),
          ("x-updated-at", "1700000002000"),
        ],
        "Some notes.",
      ).into_response()
    }
    "/docs/sub/deep.txt" => {
      (
        StatusCode::OK,
        [
          ("x-path", "/docs/sub/deep.txt"),
          ("x-total-size", "14"),
          ("content-type", "text/plain"),
          ("x-updated-at", "1700000003000"),
        ],
        "Deep file here",
      ).into_response()
    }
    _ => {
      (StatusCode::NOT_FOUND, "not found").into_response()
    }
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
async fn test_pull_sync_downloads_all_files() {
  let mock_url          = start_mock_aeordb().await;
  let (state, temp_dir) = create_state_store();
  let local_sync_dir    = temp_dir.join("synced-docs");

  // Create connection
  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name:      "Mock".to_string(),
    url:       mock_url,
    auth_type: AuthType::None,
    api_key:   None,
  }).expect("create connection failed");

  // Create relationship
  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Docs Sync".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/docs/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PullOnly,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // Run pull sync
  let result = pull_sync_pass(&state, &relationship.id)
    .await
    .expect("sync pass failed");

  assert_eq!(result.files_downloaded, 3, "should download 3 files");
  assert_eq!(result.files_failed, 0, "should have no failures");
  assert_eq!(result.errors.len(), 0, "should have no errors");

  // Verify files on disk
  let readme_content = std::fs::read_to_string(local_sync_dir.join("readme.md"))
    .expect("readme.md should exist");
  assert_eq!(readme_content, "Hello, World!");

  let notes_content = std::fs::read_to_string(local_sync_dir.join("notes.txt"))
    .expect("notes.txt should exist");
  assert_eq!(notes_content, "Some notes.");

  let deep_content = std::fs::read_to_string(local_sync_dir.join("sub/deep.txt"))
    .expect("sub/deep.txt should exist");
  assert_eq!(deep_content, "Deep file here");
}

#[tokio::test]
async fn test_pull_sync_skips_unchanged_files() {
  let mock_url          = start_mock_aeordb().await;
  let (state, temp_dir) = create_state_store();
  let local_sync_dir    = temp_dir.join("synced-docs");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name:      "Mock".to_string(),
    url:       mock_url,
    auth_type: AuthType::None,
    api_key:   None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Docs Sync".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/docs/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PullOnly,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // First sync — downloads everything
  let first_result = pull_sync_pass(&state, &relationship.id)
    .await.expect("first sync failed");
  assert_eq!(first_result.files_downloaded, 3);

  // Second sync — everything unchanged, should skip all
  let second_result = pull_sync_pass(&state, &relationship.id)
    .await.expect("second sync failed");
  assert_eq!(second_result.files_downloaded, 0, "should download nothing on second pass");
  assert_eq!(second_result.files_skipped, 3, "should skip all 3 files");
}

#[tokio::test]
async fn test_pull_sync_disabled_relationship_fails() {
  let mock_url          = start_mock_aeordb().await;
  let (state, temp_dir) = create_state_store();
  let local_sync_dir    = temp_dir.join("synced-docs");

  let connection_manager = ConnectionManager::new(&state);
  let connection = connection_manager.create(CreateConnectionRequest {
    name:      "Mock".to_string(),
    url:       mock_url,
    auth_type: AuthType::None,
    api_key:   None,
  }).expect("create connection failed");

  let relationship_manager = RelationshipManager::new(&state);
  let relationship = relationship_manager.create(CreateSyncRelationshipRequest {
    name:                 "Docs Sync".to_string(),
    remote_connection_id: connection.id,
    remote_path:          "/docs/".to_string(),
    local_path:           local_sync_dir.to_string_lossy().to_string(),
    direction:            SyncDirection::PullOnly,
    filter:               None,
    delete_propagation:   None,
  }).expect("create relationship failed");

  // Disable
  relationship_manager.disable(&relationship.id).expect("disable failed");

  // Sync should fail
  let result = pull_sync_pass(&state, &relationship.id).await;
  assert!(result.is_err());
  assert!(result.unwrap_err().to_string().contains("disabled"));
}

#[tokio::test]
async fn test_pull_sync_via_http_api() {
  let mock_url = start_mock_aeordb().await;

  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let database_path = temp_dir
    .join("test-state.aeordb")
    .to_string_lossy()
    .to_string();

  let local_sync_dir = temp_dir.join("synced-docs");

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

  // Create connection
  let conn_response = client
    .post(format!("{}/api/v1/connections", base_url))
    .json(&CreateConnectionRequest {
      name:      "Mock".to_string(),
      url:       mock_url,
      auth_type: AuthType::None,
      api_key:   None,
    })
    .send().await.expect("create connection failed");

  let connection: aeordb_client_lib::connections::RemoteConnection =
    conn_response.json().await.expect("parse failed");

  // Create relationship
  let sync_response = client
    .post(format!("{}/api/v1/sync", base_url))
    .json(&CreateSyncRelationshipRequest {
      name:                 "Docs".to_string(),
      remote_connection_id: connection.id,
      remote_path:          "/docs/".to_string(),
      local_path:           local_sync_dir.to_string_lossy().to_string(),
      direction:            SyncDirection::PullOnly,
      filter:               None,
      delete_propagation:   None,
    })
    .send().await.expect("create sync failed");

  let relationship: aeordb_client_lib::sync::relationships::SyncRelationship =
    sync_response.json().await.expect("parse failed");

  // Trigger sync via HTTP API
  let trigger_response = client
    .post(format!("{}/api/v1/sync/{}/trigger", base_url, relationship.id))
    .send().await.expect("trigger failed");

  assert_eq!(trigger_response.status(), 200);

  let result: serde_json::Value = trigger_response.json().await.expect("parse failed");
  assert_eq!(result["pull"]["files_downloaded"], 3);
  assert_eq!(result["pull"]["files_failed"], 0);

  // Verify files exist
  assert!(local_sync_dir.join("readme.md").exists());
  assert!(local_sync_dir.join("notes.txt").exists());
  assert!(local_sync_dir.join("sub/deep.txt").exists());
}
