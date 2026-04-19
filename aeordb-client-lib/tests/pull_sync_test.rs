use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use tokio::sync::Mutex;

use aeordb_client_lib::connections::{AuthType, RemoteConnection};
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::metadata::SyncMetadataStore;
use aeordb_client_lib::sync::pull::pull_sync;
use aeordb_client_lib::sync::relationships::{
  DeletePropagation, SyncDirection, SyncRelationship,
};

// ---------------------------------------------------------------------------
// Mock server state and helpers
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockServerState {
  diff_response: Arc<Mutex<serde_json::Value>>,
  file_contents: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
  diff_call_count: Arc<Mutex<u64>>,
}

impl MockServerState {
  fn new(diff_response: serde_json::Value) -> Self {
    Self {
      diff_response: Arc::new(Mutex::new(diff_response)),
      file_contents: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
      diff_call_count: Arc::new(Mutex::new(0)),
    }
  }

  fn with_file(self, path: &str, content: &[u8]) -> Self {
    let mut map = self.file_contents.lock().unwrap();
    map.insert(path.to_string(), content.to_vec());
    drop(map);
    self
  }
}

async fn handle_sync_diff(
  AxumState(state): AxumState<MockServerState>,
) -> impl IntoResponse {
  let mut count = state.diff_call_count.lock().await;
  *count += 1;

  let response = state.diff_response.lock().await;
  (StatusCode::OK, axum::Json(response.clone()))
}

async fn handle_download_file(
  AxumState(state): AxumState<MockServerState>,
  request: axum::extract::Request,
) -> impl IntoResponse {
  let path = request.uri().path().to_string();
  // Strip the "/files" prefix to get the remote path.
  let remote_path = path.strip_prefix("/files").unwrap_or(&path);

  let contents = state.file_contents.lock().unwrap();

  match contents.get(remote_path) {
    Some(data) => {
      axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("x-aeordb-path", remote_path)
        .header("x-aeordb-size", data.len().to_string())
        .header("content-type", "application/octet-stream")
        .body(axum::body::Body::from(data.clone()))
        .unwrap()
    }
    None => {
      axum::response::Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(axum::body::Body::from("not found"))
        .unwrap()
    }
  }
}

async fn handle_health() -> impl IntoResponse {
  (StatusCode::OK, "ok")
}

/// Start a mock aeordb server, returning the base URL (e.g. "http://127.0.0.1:PORT").
async fn start_mock_server(state: MockServerState) -> String {
  let app = Router::new()
    .route("/sync/diff", post(handle_sync_diff))
    .route("/system/health", get(handle_health))
    .fallback(handle_download_file)
    .with_state(state);

  let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .expect("failed to bind mock server");

  let addr = listener.local_addr().expect("failed to get local addr");
  let base_url = format!("http://{}", addr);

  tokio::spawn(async move {
    axum::serve(listener, app).await.expect("mock server failed");
  });

  base_url
}

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

fn temp_database_path() -> (tempfile::TempDir, String) {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  let db_path = temp_dir.path().join("test-pull.aeordb");
  let db_str = db_path.to_string_lossy().to_string();
  (temp_dir, db_str)
}

fn make_relationship(
  local_path: &str,
  remote_path: &str,
  filter: Option<String>,
  delete_propagation: DeletePropagation,
) -> SyncRelationship {
  let now = Utc::now();
  SyncRelationship {
    id:                   "test-rel-001".to_string(),
    name:                 "test-pull".to_string(),
    remote_connection_id: "test-conn-001".to_string(),
    remote_path:          remote_path.to_string(),
    local_path:           local_path.to_string(),
    direction:            SyncDirection::PullOnly,
    filter,
    delete_propagation,
    enabled:              true,
    created_at:           now,
    updated_at:           now,
  }
}

fn make_connection(base_url: &str) -> RemoteConnection {
  let now = Utc::now();
  RemoteConnection {
    id:         "test-conn-001".to_string(),
    name:       "test-remote".to_string(),
    url:        base_url.to_string(),
    auth_type:  AuthType::None,
    api_key:    None,
    created_at: now,
    updated_at: now,
  }
}

fn make_empty_diff(root_hash: &str) -> serde_json::Value {
  serde_json::json!({
    "root_hash": root_hash,
    "changes": {
      "files_added": [],
      "files_modified": [],
      "files_deleted": [],
      "symlinks_added": [],
      "symlinks_modified": [],
      "symlinks_deleted": [],
    },
    "chunk_hashes_needed": [],
  })
}

fn make_diff_with_added_files(root_hash: &str, files: Vec<(&str, &str, u64)>) -> serde_json::Value {
  let files_added: Vec<serde_json::Value> = files.iter()
    .map(|(path, hash, size)| {
      serde_json::json!({
        "path": path,
        "hash": hash,
        "size": size,
        "content_type": "application/octet-stream",
        "chunk_hashes": [],
      })
    })
    .collect();

  serde_json::json!({
    "root_hash": root_hash,
    "changes": {
      "files_added": files_added,
      "files_modified": [],
      "files_deleted": [],
      "symlinks_added": [],
      "symlinks_modified": [],
      "symlinks_deleted": [],
    },
    "chunk_hashes_needed": [],
  })
}

fn make_diff_with_deleted_files(root_hash: &str, paths: Vec<&str>) -> serde_json::Value {
  let files_deleted: Vec<serde_json::Value> = paths.iter()
    .map(|path| serde_json::json!({ "path": path }))
    .collect();

  serde_json::json!({
    "root_hash": root_hash,
    "changes": {
      "files_added": [],
      "files_modified": [],
      "files_deleted": files_deleted,
      "symlinks_added": [],
      "symlinks_modified": [],
      "symlinks_deleted": [],
    },
    "chunk_hashes_needed": [],
  })
}

// ---------------------------------------------------------------------------
// Happy path tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pull_downloads_new_files() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_diff_with_added_files(
    "abc123rootHash",
    vec![
      ("/docs/readme.md", "hash_readme", 13),
      ("/docs/notes.txt", "hash_notes", 11),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/readme.md", b"Hello, world!")
    .with_file("/docs/notes.txt", b"Some notes.");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_pulled, 2);
  assert_eq!(result.files_failed, 0);
  assert_eq!(result.errors.len(), 0);
  assert_eq!(result.total_bytes, 24); // 13 + 11

  // Verify files exist on local filesystem.
  let readme_path = local_dir.path().join("readme.md");
  let notes_path = local_dir.path().join("notes.txt");

  assert!(readme_path.exists(), "readme.md should exist on disk");
  assert!(notes_path.exists(), "notes.txt should exist on disk");

  let readme_content = std::fs::read_to_string(&readme_path).expect("failed to read readme");
  let notes_content = std::fs::read_to_string(&notes_path).expect("failed to read notes");

  assert_eq!(readme_content, "Hello, world!");
  assert_eq!(notes_content, "Some notes.");
}

#[tokio::test]
async fn test_pull_downloads_nested_files() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_diff_with_added_files(
    "nestedHash",
    vec![
      ("/docs/sub/deep/file.txt", "hash_deep", 8),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/sub/deep/file.txt", b"deep one");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_pulled, 1);

  // Verify the nested file was created with parent directories.
  let file_path = local_dir.path().join("sub").join("deep").join("file.txt");
  assert!(file_path.exists(), "nested file should exist on disk");
  assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "deep one");
}

#[tokio::test]
async fn test_pull_saves_checkpoint() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_diff_with_added_files(
    "checkpoint_root_hash_v1",
    vec![
      ("/docs/file.txt", "hash_file", 5),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/file.txt", b"hello");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  // Verify the checkpoint was saved with the remote root hash.
  let metadata_store = SyncMetadataStore::new(&store);
  let checkpoint = metadata_store.get_checkpoint("test-rel-001")
    .expect("failed to get checkpoint")
    .expect("checkpoint should exist after pull");

  assert_eq!(checkpoint.remote_root_hash, "checkpoint_root_hash_v1");
  assert_eq!(checkpoint.relationship_id, "test-rel-001");
  assert!(checkpoint.last_sync_at > 0);
}

#[tokio::test]
async fn test_pull_saves_file_metadata() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_diff_with_added_files(
    "metaHash",
    vec![
      ("/docs/tracked.txt", "hash_tracked", 7),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/tracked.txt", b"tracked");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  // Verify file metadata was stored.
  let metadata_store = SyncMetadataStore::new(&store);
  let file_meta = metadata_store.get_file_meta("test-rel-001", "/docs/tracked.txt")
    .expect("failed to get file meta")
    .expect("file meta should exist after pull");

  assert_eq!(file_meta.path, "/docs/tracked.txt");
  assert_eq!(file_meta.size, 7);
  assert_eq!(file_meta.sync_status, aeordb_client_lib::sync::metadata::SyncStatus::Synced);

  // The content hash should be a valid BLAKE3 hash of "tracked".
  let expected_hash = blake3::hash(b"tracked").to_hex().to_string();
  assert_eq!(file_meta.content_hash, expected_hash);
}

#[tokio::test]
async fn test_pull_respects_filter() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_diff_with_added_files(
    "filterHash",
    vec![
      ("/docs/report.pdf", "hash_pdf", 10),
      ("/docs/readme.md", "hash_md", 6),
      ("/docs/image.png", "hash_png", 8),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/report.pdf", b"pdfbytes!!")
    .with_file("/docs/readme.md", b"readme")
    .with_file("/docs/image.png", b"pngbytes");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);

  // Only pull .pdf files.
  let relationship = make_relationship(
    &local_path,
    "/docs/",
    Some("*.pdf".to_string()),
    DeletePropagation::default(),
  );
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_pulled, 1, "only the .pdf should be pulled");
  assert_eq!(result.files_skipped, 2, "the .md and .png should be skipped");

  // Verify only the PDF exists locally.
  assert!(local_dir.path().join("report.pdf").exists());
  assert!(!local_dir.path().join("readme.md").exists());
  assert!(!local_dir.path().join("image.png").exists());
}

#[tokio::test]
async fn test_pull_handles_deletions() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  // Pre-create a file on the local filesystem that will be "deleted" by the remote.
  let doomed_path = local_dir.path().join("doomed.txt");
  std::fs::write(&doomed_path, b"i will be deleted").expect("failed to write doomed file");
  assert!(doomed_path.exists());

  let diff = make_diff_with_deleted_files("deleteHash", vec!["/docs/doomed.txt"]);

  let state = MockServerState::new(diff);
  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);

  let delete_propagation = DeletePropagation {
    local_to_remote: false,
    remote_to_local: true,
  };
  let relationship = make_relationship(&local_path, "/docs/", None, delete_propagation);
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  // Pre-store metadata for the file so it can be cleaned up.
  let metadata_store = SyncMetadataStore::new(&store);
  let doomed_meta = aeordb_client_lib::sync::metadata::FileSyncMeta {
    path:           "/docs/doomed.txt".to_string(),
    content_hash:   "old_hash".to_string(),
    size:           17,
    modified_at:    1700000000000,
    sync_status:    aeordb_client_lib::sync::metadata::SyncStatus::Synced,
    last_synced_at: 1700000000000,
  };
  metadata_store.set_file_meta("test-rel-001", &doomed_meta).expect("failed to set meta");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_deleted, 1);
  assert_eq!(result.files_failed, 0);

  // Verify the file was removed from disk.
  assert!(!doomed_path.exists(), "doomed.txt should have been deleted");

  // Verify metadata was cleaned up.
  let meta_after = metadata_store.get_file_meta("test-rel-001", "/docs/doomed.txt")
    .expect("failed to get meta");
  assert!(meta_after.is_none(), "metadata should be deleted after remote deletion");
}

#[tokio::test]
async fn test_pull_deletion_skipped_when_propagation_disabled() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  // Create a file that the remote says is deleted.
  let survivor_path = local_dir.path().join("survivor.txt");
  std::fs::write(&survivor_path, b"i should survive").expect("failed to write");
  assert!(survivor_path.exists());

  let diff = make_diff_with_deleted_files("noDeleteHash", vec!["/docs/survivor.txt"]);

  let state = MockServerState::new(diff);
  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);

  // Delete propagation is disabled (default).
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_deleted, 0, "no files should be deleted when propagation is off");

  // Verify the file survived.
  assert!(survivor_path.exists(), "file should still exist when delete propagation is disabled");
}

#[tokio::test]
async fn test_pull_empty_diff_saves_checkpoint() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_empty_diff("emptyDiffHash");

  let state = MockServerState::new(diff);
  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_pulled, 0);
  assert_eq!(result.files_failed, 0);

  // Even with no changes, the checkpoint should be saved.
  let metadata_store = SyncMetadataStore::new(&store);
  let checkpoint = metadata_store.get_checkpoint("test-rel-001")
    .expect("failed to get checkpoint")
    .expect("checkpoint should be saved even for empty diff");

  assert_eq!(checkpoint.remote_root_hash, "emptyDiffHash");
}

// ---------------------------------------------------------------------------
// Failure path tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pull_handles_download_failure() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  // Diff says a file exists, but the mock server does NOT serve it.
  let diff = make_diff_with_added_files(
    "failDownloadHash",
    vec![
      ("/docs/exists.txt", "hash_exists", 6),
      ("/docs/missing.txt", "hash_missing", 7),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/exists.txt", b"exists");
  // Note: /docs/missing.txt is NOT registered, so it will 404.

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync should not fail entirely due to single file error");

  assert_eq!(result.files_pulled, 1, "the existing file should be pulled");
  assert_eq!(result.files_failed, 1, "the missing file should fail");
  assert_eq!(result.errors.len(), 1, "one error should be recorded");
  assert!(result.errors[0].contains("missing.txt"), "error should reference the failing file");
}

#[tokio::test]
async fn test_pull_fails_when_server_unreachable() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  // Point to a port that nothing is listening on.
  let connection = make_connection("http://127.0.0.1:1");
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await;

  assert!(result.is_err(), "pull_sync should fail when server is unreachable");
  let error_message = format!("{}", result.unwrap_err());
  assert!(
    error_message.contains("sync/diff") || error_message.contains("Connection refused"),
    "error should reference the sync/diff request or connection failure, got: {}",
    error_message,
  );
}

#[tokio::test]
async fn test_pull_handles_server_error_on_diff() {
  // Spin up a mock that returns 500 for /sync/diff.
  let app = Router::new()
    .route("/sync/diff", post(|| async {
      (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }))
    .route("/system/health", get(|| async { "ok" }));

  let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .expect("failed to bind");
  let addr = listener.local_addr().expect("failed to get addr");
  let base_url = format!("http://{}", addr);

  tokio::spawn(async move {
    axum::serve(listener, app).await.unwrap();
  });

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await;

  assert!(result.is_err(), "pull_sync should fail when server returns 500");
  let error_message = format!("{}", result.unwrap_err());
  assert!(
    error_message.contains("500"),
    "error should mention the HTTP status code, got: {}",
    error_message,
  );
}

#[tokio::test]
async fn test_pull_handles_malformed_diff_response() {
  // Spin up a mock that returns invalid JSON for /sync/diff.
  let app = Router::new()
    .route("/sync/diff", post(|| async {
      (StatusCode::OK, "this is not json")
    }))
    .route("/system/health", get(|| async { "ok" }));

  let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
    .await
    .expect("failed to bind");
  let addr = listener.local_addr().expect("failed to get addr");
  let base_url = format!("http://{}", addr);

  tokio::spawn(async move {
    axum::serve(listener, app).await.unwrap();
  });

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await;

  assert!(result.is_err(), "pull_sync should fail with malformed response");
  let error_message = format!("{}", result.unwrap_err());
  assert!(
    error_message.contains("parse"),
    "error should mention parsing failure, got: {}",
    error_message,
  );
}

#[tokio::test]
async fn test_pull_creates_local_directory_if_missing() {
  let parent_dir = tempfile::tempdir().expect("failed to create parent dir");
  let local_path = parent_dir.path().join("nonexistent").join("sync");
  let local_path_str = local_path.to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  assert!(!local_path.exists(), "local path should not exist yet");

  let diff = make_diff_with_added_files(
    "mkdirHash",
    vec![("/docs/file.txt", "hash_file", 5)],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/file.txt", b"hello");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path_str, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync should create missing directories");

  assert_eq!(result.files_pulled, 1);
  assert!(local_path.exists(), "local directory should have been created");
  assert!(local_path.join("file.txt").exists());
}

// ---------------------------------------------------------------------------
// Edge case tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pull_exclude_filter() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  let diff = make_diff_with_added_files(
    "excludeHash",
    vec![
      ("/docs/report.pdf", "hash_pdf", 3),
      ("/docs/.DS_Store", "hash_ds", 4),
      ("/docs/cache.tmp", "hash_tmp", 5),
    ],
  );

  let state = MockServerState::new(diff)
    .with_file("/docs/report.pdf", b"pdf")
    .with_file("/docs/.DS_Store", b"junk")
    .with_file("/docs/cache.tmp", b"cache");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);

  // Exclude .DS_Store and .tmp files.
  let relationship = make_relationship(
    &local_path,
    "/docs/",
    Some("!.DS_Store, !*.tmp".to_string()),
    DeletePropagation::default(),
  );
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_pulled, 1);
  assert_eq!(result.files_skipped, 2);

  assert!(local_dir.path().join("report.pdf").exists());
  assert!(!local_dir.path().join(".DS_Store").exists());
  assert!(!local_dir.path().join("cache.tmp").exists());
}

#[tokio::test]
async fn test_pull_deletion_of_nonexistent_local_file() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  // Remote says a file was deleted, but it never existed locally.
  let diff = make_diff_with_deleted_files("ghostDeleteHash", vec!["/docs/ghost.txt"]);

  let state = MockServerState::new(diff);
  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);

  let delete_propagation = DeletePropagation {
    local_to_remote: false,
    remote_to_local: true,
  };
  let relationship = make_relationship(&local_path, "/docs/", None, delete_propagation);
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  // Should not panic or error -- just a no-op deletion.
  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync should handle deletion of nonexistent file gracefully");

  assert_eq!(result.files_deleted, 1);
  assert_eq!(result.files_failed, 0);
}

#[tokio::test]
async fn test_pull_modified_file_overwrites() {
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path().to_string_lossy().to_string();
  let (_db_dir, db_path) = temp_database_path();

  // Pre-create the file with old content.
  let target_path = local_dir.path().join("report.txt");
  std::fs::write(&target_path, b"old content").expect("failed to write old content");

  let diff_json = serde_json::json!({
    "root_hash": "modifiedHash",
    "changes": {
      "files_added": [],
      "files_modified": [{
        "path": "/docs/report.txt",
        "hash": "hash_modified",
        "size": 11,
        "content_type": "text/plain",
        "chunk_hashes": [],
      }],
      "files_deleted": [],
      "symlinks_added": [],
      "symlinks_modified": [],
      "symlinks_deleted": [],
    },
    "chunk_hashes_needed": [],
  });

  let state = MockServerState::new(diff_json)
    .with_file("/docs/report.txt", b"new content");

  let base_url = start_mock_server(state).await;
  let connection = make_connection(&base_url);
  let relationship = make_relationship(&local_path, "/docs/", None, DeletePropagation::default());
  let store = StateStore::open_or_create(&db_path).expect("failed to create state store");

  let result = pull_sync(&store, &connection, &relationship, &reqwest::Client::new()).await
    .expect("pull_sync failed");

  assert_eq!(result.files_pulled, 1);

  let updated_content = std::fs::read_to_string(&target_path).expect("failed to read");
  assert_eq!(updated_content, "new content", "file should have been overwritten with new content");
}
