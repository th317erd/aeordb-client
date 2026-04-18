use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State as AxumState};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use aeordb_client_lib::server::{ServerConfig, start_server_with_handle};

// ---------------------------------------------------------------------------
// Mock aeordb server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct MockServerState {
  /// Directory listings: directory path -> JSON entries
  directories: Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
  /// File contents: file path -> bytes
  files: Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>,
  /// Uploaded files: path -> bytes
  uploads: Arc<Mutex<HashMap<String, Vec<u8>>>>,
  /// Deleted paths
  deleted: Arc<Mutex<Vec<String>>>,
}

impl MockServerState {
  fn new() -> Self {
    Self {
      directories: Arc::new(std::sync::Mutex::new(HashMap::new())),
      files:       Arc::new(std::sync::Mutex::new(HashMap::new())),
      uploads:     Arc::new(Mutex::new(HashMap::new())),
      deleted:     Arc::new(Mutex::new(Vec::new())),
    }
  }

  fn with_directory(self, path: &str, entries: serde_json::Value) -> Self {
    self.directories.lock().unwrap().insert(path.to_string(), entries);
    self
  }

  fn with_file(self, path: &str, content: &[u8]) -> Self {
    self.files.lock().unwrap().insert(path.to_string(), content.to_vec());
    self
  }
}

/// Handles GET /engine/{*path} — returns directory listing (JSON) or file content.
async fn handle_get_engine(
  Path(path): Path<String>,
  AxumState(state): AxumState<MockServerState>,
) -> impl IntoResponse {
  let remote_path = format!("/{}", path);

  // Check if it's a directory listing request
  {
    let dirs = state.directories.lock().unwrap();
    if let Some(listing) = dirs.get(&remote_path) {
      return axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(listing).unwrap()))
        .unwrap();
    }
  }

  // Check if it's a file download
  {
    let files = state.files.lock().unwrap();
    if let Some(data) = files.get(&remote_path) {
      return axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/octet-stream")
        .header("x-path", &remote_path)
        .header("x-total-size", data.len().to_string())
        .body(axum::body::Body::from(data.clone()))
        .unwrap();
    }
  }

  axum::response::Response::builder()
    .status(StatusCode::NOT_FOUND)
    .body(axum::body::Body::from("not found"))
    .unwrap()
}

/// Handles PUT /engine/{*path} — accept uploads.
async fn handle_put_engine(
  Path(path): Path<String>,
  AxumState(state): AxumState<MockServerState>,
  body: Bytes,
) -> StatusCode {
  let remote_path = format!("/{}", path);
  state.uploads.lock().await.insert(remote_path, body.to_vec());
  StatusCode::OK
}

/// Handles DELETE /engine/{*path} — accept deletes.
async fn handle_delete_engine(
  Path(path): Path<String>,
  AxumState(state): AxumState<MockServerState>,
) -> StatusCode {
  let remote_path = format!("/{}", path);
  state.deleted.lock().await.push(remote_path);
  StatusCode::OK
}

async fn handle_health() -> StatusCode {
  StatusCode::OK
}

async fn start_mock_aeordb(state: MockServerState) -> (SocketAddr, MockServerState) {
  let app = Router::new()
    .route("/admin/health", get(handle_health))
    .route("/engine/{*path}", get(handle_get_engine).put(handle_put_engine).delete(handle_delete_engine))
    .with_state(state.clone());

  let listener = TcpListener::bind("127.0.0.1:0").await.expect("failed to bind mock server");
  let address = listener.local_addr().expect("failed to get address");

  tokio::spawn(async move {
    axum::serve(listener, app).await.expect("mock server failed");
  });

  (address, state)
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

struct TestEnv {
  client_base_url: String,
  mock_state:      MockServerState,
  local_dir:       tempfile::TempDir,
  relationship_id: String,
}

async fn setup_test_env(mock_state: MockServerState) -> TestEnv {
  let (mock_address, mock_state) = start_mock_aeordb(mock_state).await;

  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  let config_dir = temp_dir.path().join("config");
  std::fs::create_dir_all(&config_dir).expect("failed to create config dir");
  let config_path = config_dir.join("config.yaml");
  let data_path = temp_dir.path().join("state.aeordb");

  // Create the local sync directory
  let local_dir = tempfile::tempdir().expect("failed to create local dir");

  let connection_id = "test-conn-001";
  let relationship_id = "test-rel-001";

  // Write the config YAML with connection + relationship
  let config_yaml = format!(
    r#"connections:
  - id: "{connection_id}"
    name: "Test Mock"
    url: "http://{mock_address}"
    auth_type: none
    api_key: null
    created_at: "2024-01-01T00:00:00Z"
    updated_at: "2024-01-01T00:00:00Z"
relationships:
  - id: "{relationship_id}"
    name: "Work Docs"
    remote_connection_id: "{connection_id}"
    remote_path: "/docs/"
    local_path: "{local_path}"
    direction: bidirectional
    filter: null
    delete_propagation:
      local_to_remote: false
      remote_to_local: false
    enabled: true
    created_at: "2024-01-01T00:00:00Z"
    updated_at: "2024-01-01T00:00:00Z"
"#,
    connection_id = connection_id,
    mock_address = mock_address,
    local_path = local_dir.path().to_string_lossy(),
    relationship_id = relationship_id,
  );

  std::fs::write(&config_path, &config_yaml).expect("failed to write config");

  let server_config = ServerConfig {
    host:        "127.0.0.1".to_string(),
    port:        0,
    config_path,
    data_path,
  };

  let (address, _handle) = start_server_with_handle(server_config)
    .await
    .expect("failed to start client server");

  let client_base_url = format!("http://{}", address);

  TestEnv {
    client_base_url,
    mock_state,
    local_dir,
    relationship_id: relationship_id.to_string(),
  }
}

fn sample_directory_listing() -> serde_json::Value {
  serde_json::json!([
    {
      "name": "readme.md",
      "entry_type": 2,
      "total_size": 24576,
      "created_at": 1776288276000i64,
      "updated_at": 1776288276101i64,
      "content_type": "text/markdown",
      "path": "/docs/readme.md",
      "hash": "abc123"
    },
    {
      "name": "images",
      "entry_type": 3,
      "total_size": 0,
      "created_at": 1776288276000i64,
      "updated_at": 1776288276000i64,
      "content_type": null,
      "path": "/docs/images/",
      "hash": null
    }
  ])
}

fn subdirectory_listing() -> serde_json::Value {
  serde_json::json!([
    {
      "name": "logo.png",
      "entry_type": 2,
      "total_size": 102400,
      "created_at": 1776288276000i64,
      "updated_at": 1776288276200i64,
      "content_type": "image/png",
      "path": "/docs/images/logo.png",
      "hash": "def456"
    }
  ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_browse_root_lists_entries() {
  let mock = MockServerState::new()
    .with_directory("/docs/", sample_directory_listing());

  let env = setup_test_env(mock).await;
  let client = reqwest::Client::new();

  let response = client
    .get(format!("{}/api/v1/browse/{}", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert_eq!(body["relationship_id"], env.relationship_id);
  assert_eq!(body["relationship_name"], "Work Docs");
  assert_eq!(body["remote_path"], "/docs/");

  let entries = body["entries"].as_array().expect("entries should be array");
  assert_eq!(entries.len(), 2);

  // First entry: readme.md (file)
  assert_eq!(entries[0]["name"], "readme.md");
  assert_eq!(entries[0]["entry_type"], 2);
  assert_eq!(entries[0]["size"], 24576);
  assert_eq!(entries[0]["content_type"], "text/markdown");
  assert_eq!(entries[0]["sync_status"], "not_synced");
  assert_eq!(entries[0]["has_local"], false);

  // Second entry: images (directory)
  assert_eq!(entries[1]["name"], "images");
  assert_eq!(entries[1]["entry_type"], 3);
}

#[tokio::test]
async fn test_browse_root_with_synced_file() {
  let mock = MockServerState::new()
    .with_directory("/docs/", sample_directory_listing());

  let env = setup_test_env(mock).await;

  // Create a local file so has_local is true
  std::fs::write(env.local_dir.path().join("readme.md"), b"# Hello").expect("write failed");

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/browse/{}", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  let entries = body["entries"].as_array().unwrap();
  assert_eq!(entries[0]["has_local"], true);
}

#[tokio::test]
async fn test_browse_subdirectory() {
  let mock = MockServerState::new()
    .with_directory("/docs/images/", subdirectory_listing());

  let env = setup_test_env(mock).await;
  let client = reqwest::Client::new();

  let response = client
    .get(format!("{}/api/v1/browse/{}/images/", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert_eq!(body["remote_path"], "/docs/images/");

  let entries = body["entries"].as_array().unwrap();
  assert_eq!(entries.len(), 1);
  assert_eq!(entries[0]["name"], "logo.png");
  assert_eq!(entries[0]["entry_type"], 2);
  assert_eq!(entries[0]["size"], 102400);
}

#[tokio::test]
async fn test_browse_nonexistent_relationship() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;
  let client = reqwest::Client::new();

  let response = client
    .get(format!("{}/api/v1/browse/nonexistent-id", env.client_base_url))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_serve_file_from_local() {
  let mock = MockServerState::new()
    .with_file("/docs/readme.md", b"REMOTE CONTENT");

  let env = setup_test_env(mock).await;

  // Write a local file — this should be served instead of remote
  std::fs::write(env.local_dir.path().join("readme.md"), b"LOCAL CONTENT").expect("write failed");

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/{}/readme.md", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body = response.bytes().await.expect("body failed");
  assert_eq!(body.as_ref(), b"LOCAL CONTENT", "should serve local file, not remote");
}

#[tokio::test]
async fn test_serve_file_from_remote_fallback() {
  let mock = MockServerState::new()
    .with_file("/docs/readme.md", b"REMOTE CONTENT");

  let env = setup_test_env(mock).await;
  // No local file exists — should fallback to remote

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/{}/readme.md", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body = response.bytes().await.expect("body failed");
  assert_eq!(body.as_ref(), b"REMOTE CONTENT", "should fall back to remote");
}

#[tokio::test]
async fn test_serve_file_force_remote() {
  let mock = MockServerState::new()
    .with_file("/docs/readme.md", b"REMOTE CONTENT");

  let env = setup_test_env(mock).await;

  // Write a local file — but we force remote, so it should NOT be served
  std::fs::write(env.local_dir.path().join("readme.md"), b"LOCAL CONTENT").expect("write failed");

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/{}/readme.md?source=remote", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body = response.bytes().await.expect("body failed");
  assert_eq!(body.as_ref(), b"REMOTE CONTENT", "should serve from remote when forced");
}

#[tokio::test]
async fn test_serve_file_force_local_not_found() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;
  // No local file exists

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/{}/nonexistent.txt?source=local", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_serve_file_content_type_header() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  std::fs::write(env.local_dir.path().join("styles.css"), b"body {}").expect("write failed");

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/{}/styles.css", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let content_type = response.headers().get("content-type")
    .and_then(|v| v.to_str().ok())
    .unwrap_or("");
  assert_eq!(content_type, "text/css");
}

#[tokio::test]
async fn test_path_traversal_rejected() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  // Use a raw URL with percent-encoded dots to prevent client-side normalization
  // This tests that the server-side safe_local_path check catches traversal
  let client = reqwest::Client::new();

  // Try path traversal via the open-locally endpoint where the path is in the JSON body
  // (not subject to URL normalization)
  let response = client
    .post(format!("{}/api/v1/files/{}/open", env.client_base_url, env.relationship_id))
    .json(&serde_json::json!({ "path": "../../etc/passwd" }))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 403, "path traversal should be rejected with 403");
}

#[tokio::test]
async fn test_upload_proxied_to_remote() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock.clone()).await;

  let client = reqwest::Client::new();
  let response = client
    .put(format!("{}/api/v1/files/{}/new-file.txt", env.client_base_url, env.relationship_id))
    .header("Content-Type", "text/plain")
    .body(b"uploaded content".to_vec())
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert!(body["message"].as_str().unwrap().contains("uploaded"));

  // Verify the mock received the upload
  let uploads = env.mock_state.uploads.lock().await;
  let uploaded = uploads.get("/docs/new-file.txt").expect("upload should arrive at mock");
  assert_eq!(uploaded, b"uploaded content");
}

#[tokio::test]
async fn test_delete_proxied_to_remote() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock.clone()).await;

  let client = reqwest::Client::new();
  let response = client
    .delete(format!("{}/api/v1/files/{}/old-file.txt", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 200);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert!(body["message"].as_str().unwrap().contains("deleted"));

  // Verify the mock received the delete
  let deleted = env.mock_state.deleted.lock().await;
  assert!(deleted.contains(&"/docs/old-file.txt".to_string()));
}

#[tokio::test]
async fn test_open_locally_nonexistent_file() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .post(format!("{}/api/v1/files/{}/open", env.client_base_url, env.relationship_id))
    .json(&serde_json::json!({ "path": "does-not-exist.txt" }))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 404);

  let body: serde_json::Value = response.json().await.expect("parse failed");
  assert!(body["error"].as_str().unwrap().contains("not found locally"));
}

#[tokio::test]
async fn test_upload_nonexistent_relationship() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .put(format!("{}/api/v1/files/nonexistent-rel/somefile.txt", env.client_base_url))
    .body(b"data".to_vec())
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_nonexistent_relationship() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .delete(format!("{}/api/v1/files/nonexistent-rel/somefile.txt", env.client_base_url))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_serve_file_nonexistent_relationship() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/nonexistent-rel/somefile.txt", env.client_base_url))
    .send()
    .await
    .expect("request failed");

  assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_open_locally_path_traversal_rejected() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .post(format!("{}/api/v1/files/{}/open", env.client_base_url, env.relationship_id))
    .json(&serde_json::json!({ "path": "../../etc/passwd" }))
    .send()
    .await
    .expect("request failed");

  // Should be 403 (traversal) or 404 (not found after traversal check) — not 200
  assert!(
    response.status() == 403 || response.status() == 404,
    "expected 403 or 404, got {}",
    response.status(),
  );
}

#[tokio::test]
async fn test_browse_remote_error_returns_502() {
  // No directory registered in mock — the remote returns 404, which our client maps to an error
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/browse/{}", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  // Remote returned 404 for directory listing — our browse should return 502 Bad Gateway
  assert_eq!(response.status(), 502);
}

#[tokio::test]
async fn test_serve_file_remote_and_local_both_missing() {
  let mock = MockServerState::new();
  let env = setup_test_env(mock).await;

  let client = reqwest::Client::new();
  let response = client
    .get(format!("{}/api/v1/files/{}/nonexistent.txt", env.client_base_url, env.relationship_id))
    .send()
    .await
    .expect("request failed");

  // No local file, remote returns 404 — should get 502 (bad gateway from remote failure)
  assert_eq!(response.status(), 502);
}
