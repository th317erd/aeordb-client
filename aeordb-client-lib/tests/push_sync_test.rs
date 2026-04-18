use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::Router;
use chrono::Utc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use aeordb_client_lib::connections::{AuthType, RemoteConnection};
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::metadata::{SyncMetadataStore, SyncStatus};
use aeordb_client_lib::sync::push::push_sync;
use aeordb_client_lib::sync::relationships::{
  DeletePropagation, SyncDirection, SyncRelationship,
};

// --- Mock server state ---

#[derive(Debug, Clone)]
struct MockServerState {
  /// Uploaded files: remote_path -> content bytes
  files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
  /// Created symlinks: remote_path -> target
  symlinks: Arc<Mutex<HashMap<String, String>>>,
  /// Deleted paths
  deleted: Arc<Mutex<Vec<String>>>,
}

impl MockServerState {
  fn new() -> Self {
    Self {
      files:    Arc::new(Mutex::new(HashMap::new())),
      symlinks: Arc::new(Mutex::new(HashMap::new())),
      deleted:  Arc::new(Mutex::new(Vec::new())),
    }
  }
}

// --- Mock server handlers ---

async fn handle_upload(
  Path(path): Path<String>,
  State(state): State<MockServerState>,
  body: Bytes,
) -> StatusCode {
  let remote_path = format!("/{}", path);
  state.files.lock().await.insert(remote_path, body.to_vec());
  StatusCode::OK
}

async fn handle_create_symlink(
  Path(path): Path<String>,
  State(state): State<MockServerState>,
  body: Bytes,
) -> StatusCode {
  let remote_path = format!("/{}", path);
  let parsed: serde_json::Value = match serde_json::from_slice(&body) {
    Ok(val) => val,
    Err(_) => return StatusCode::BAD_REQUEST,
  };

  let target = match parsed.get("target").and_then(|t| t.as_str()) {
    Some(t) => t.to_string(),
    None => return StatusCode::BAD_REQUEST,
  };

  state.symlinks.lock().await.insert(remote_path, target);
  StatusCode::OK
}

async fn handle_delete(
  Path(path): Path<String>,
  State(state): State<MockServerState>,
) -> StatusCode {
  let remote_path = format!("/{}", path);
  state.files.lock().await.remove(&remote_path);
  state.deleted.lock().await.push(remote_path);
  StatusCode::OK
}

async fn handle_health() -> StatusCode {
  StatusCode::OK
}

// --- Test helpers ---

async fn start_mock_server() -> (SocketAddr, MockServerState) {
  let state = MockServerState::new();

  let app = Router::new()
    .route("/system/health", get(handle_health))
    .route("/files/{*path}", put(handle_upload))
    .route("/files/{*path}", delete(handle_delete))
    .route("/links/{*path}", put(handle_create_symlink))
    .with_state(state.clone());

  let listener = TcpListener::bind("127.0.0.1:0").await.expect("failed to bind");
  let address = listener.local_addr().expect("failed to get address");

  tokio::spawn(async move {
    axum::serve(listener, app).await.expect("mock server failed");
  });

  (address, state)
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

fn make_connection(address: &SocketAddr) -> RemoteConnection {
  let now = Utc::now();

  RemoteConnection {
    id:         "test-conn-001".to_string(),
    name:       "test-mock".to_string(),
    url:        format!("http://{}", address),
    auth_type:  AuthType::None,
    api_key:    None,
    created_at: now,
    updated_at: now,
  }
}

fn make_relationship(local_path: &str) -> SyncRelationship {
  let now = Utc::now();

  SyncRelationship {
    id:                   "test-rel-001".to_string(),
    name:                 "test-sync".to_string(),
    remote_connection_id: "test-conn-001".to_string(),
    remote_path:          "/docs/".to_string(),
    local_path:           local_path.to_string(),
    direction:            SyncDirection::PushOnly,
    filter:               None,
    delete_propagation:   DeletePropagation::default(),
    enabled:              true,
    created_at:           now,
    updated_at:           now,
  }
}

// --- Tests ---

#[tokio::test]
async fn test_push_uploads_new_files() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  // Create a temp directory with local files.
  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("report.pdf"), b"pdf-content-here").expect("write failed");
  std::fs::write(local_path.join("notes.txt"), b"some notes").expect("write failed");
  std::fs::create_dir_all(local_path.join("subdir")).expect("mkdir failed");
  std::fs::write(local_path.join("subdir/nested.md"), b"# Nested doc").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 3, "should push 3 files");
  assert_eq!(result.files_failed, 0, "no failures expected");
  assert!(result.errors.is_empty(), "no errors expected");
  assert!(result.total_bytes > 0, "should have transferred bytes");

  // Verify files arrived at the mock server.
  let files = mock_state.files.lock().await;
  assert_eq!(files.get("/docs/report.pdf").map(|b| b.as_slice()), Some(b"pdf-content-here".as_slice()));
  assert_eq!(files.get("/docs/notes.txt").map(|b| b.as_slice()), Some(b"some notes".as_slice()));
  assert_eq!(files.get("/docs/subdir/nested.md").map(|b| b.as_slice()), Some(b"# Nested doc".as_slice()));
}

#[tokio::test]
async fn test_push_skips_unchanged_files() {
  let (address, _mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("stable.txt"), b"unchanged content").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  // First push: should upload the file.
  let result_1 = push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  assert_eq!(result_1.files_pushed, 1, "first push should upload the file");

  // Second push without modifying the file: mtime has not changed,
  // so the file should be skipped.
  let result_2 = push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(result_2.files_pushed, 0, "second push should upload nothing");
  assert_eq!(result_2.files_skipped, 1, "second push should skip the file");
}

#[tokio::test]
async fn test_push_detects_modified_files() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("mutable.txt"), b"version 1").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  // First push.
  let result_1 = push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  assert_eq!(result_1.files_pushed, 1);

  // Modify the file (change content AND mtime).
  // Sleep briefly so the filesystem mtime actually changes.
  tokio::time::sleep(std::time::Duration::from_millis(50)).await;
  std::fs::write(local_path.join("mutable.txt"), b"version 2").expect("write failed");

  // Second push: should detect the modification and re-upload.
  let result_2 = push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(result_2.files_pushed, 1, "should re-upload modified file");
  assert_eq!(result_2.files_skipped, 0, "should not skip the modified file");

  // Verify the remote has the updated content.
  let files = mock_state.files.lock().await;
  assert_eq!(
    files.get("/docs/mutable.txt").map(|b| b.as_slice()),
    Some(b"version 2".as_slice()),
  );
}

#[tokio::test]
async fn test_push_respects_filter() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("included.pdf"), b"pdf-bytes").expect("write failed");
  std::fs::write(local_path.join("excluded.txt"), b"text-bytes").expect("write failed");
  std::fs::write(local_path.join("also-excluded.rs"), b"fn main(){}").expect("write failed");

  let mut relationship = make_relationship(&local_path.to_string_lossy());
  relationship.filter = Some("*.pdf".to_string());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1, "only the PDF should be pushed");
  assert_eq!(result.files_skipped, 2, "two files should be skipped by filter");

  let files = mock_state.files.lock().await;
  assert!(files.contains_key("/docs/included.pdf"), "PDF should be on remote");
  assert!(!files.contains_key("/docs/excluded.txt"), "TXT should not be on remote");
  assert!(!files.contains_key("/docs/also-excluded.rs"), "RS should not be on remote");
}

#[tokio::test]
async fn test_push_handles_symlinks() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  // Create a real file and a symlink pointing to it.
  let target_file = local_path.join("target.txt");
  std::fs::write(&target_file, b"target content").expect("write failed");

  let symlink_path = local_path.join("link.txt");
  std::os::unix::fs::symlink(&target_file, &symlink_path).expect("symlink failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  // Should push 1 regular file + 1 symlink.
  assert_eq!(result.files_pushed, 2, "should push file and symlink");
  assert_eq!(result.files_failed, 0, "no failures expected");

  // Verify the symlink was created on the remote.
  let symlinks = mock_state.symlinks.lock().await;
  assert!(symlinks.contains_key("/docs/link.txt"), "symlink should exist on remote");

  let symlink_target = &symlinks["/docs/link.txt"];
  assert!(
    symlink_target.contains("target.txt"),
    "symlink target should reference target.txt, got: {}",
    symlink_target,
  );
}

#[tokio::test]
async fn test_push_deletes_removed_files() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("keep.txt"), b"keep this").expect("write failed");
  std::fs::write(local_path.join("remove.txt"), b"delete me").expect("write failed");

  let mut relationship = make_relationship(&local_path.to_string_lossy());
  relationship.delete_propagation = DeletePropagation {
    local_to_remote: true,
    remote_to_local: false,
  };

  // First push: both files uploaded.
  let result_1 = push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  assert_eq!(result_1.files_pushed, 2);

  // Delete the file from the local filesystem.
  std::fs::remove_file(local_path.join("remove.txt")).expect("remove failed");

  // Second push: should detect the deletion and propagate it.
  let result_2 = push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(result_2.files_deleted, 1, "should delete 1 file from remote");

  let deleted = mock_state.deleted.lock().await;
  assert!(deleted.contains(&"/docs/remove.txt".to_string()), "remove.txt should be deleted from remote");
}

#[tokio::test]
async fn test_push_does_not_delete_when_propagation_disabled() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("file.txt"), b"content").expect("write failed");

  let mut relationship = make_relationship(&local_path.to_string_lossy());
  relationship.delete_propagation = DeletePropagation {
    local_to_remote: false,
    remote_to_local: false,
  };

  // Push, then delete local file.
  push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  std::fs::remove_file(local_path.join("file.txt")).expect("remove failed");

  // Second push: should NOT delete from remote.
  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(result.files_deleted, 0, "should not delete when propagation disabled");

  let deleted = mock_state.deleted.lock().await;
  assert!(deleted.is_empty(), "nothing should have been deleted");
}

#[tokio::test]
async fn test_push_empty_directory() {
  let (address, _mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 0);
  assert_eq!(result.files_skipped, 0);
  assert_eq!(result.files_failed, 0);
  assert_eq!(result.files_deleted, 0);
  assert_eq!(result.total_bytes, 0);
  assert!(result.errors.is_empty());
}

#[tokio::test]
async fn test_push_nonexistent_local_path_errors() {
  let (address, _mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let relationship = make_relationship("/nonexistent/path/that/does/not/exist");

  let result = push_sync(&state, &connection, &relationship).await;

  assert!(result.is_err(), "should fail for nonexistent local path");
}

#[tokio::test]
async fn test_push_metadata_stored_correctly() {
  let (address, _mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  let content = b"metadata test content";
  std::fs::write(local_path.join("meta-test.txt"), content).expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  // Verify metadata was stored correctly.
  let metadata_store = SyncMetadataStore::new(&state);
  let meta = metadata_store
    .get_file_meta(&relationship.id, "/docs/meta-test.txt")
    .expect("get_file_meta failed")
    .expect("metadata should exist");

  assert_eq!(meta.path, "/docs/meta-test.txt");
  assert_eq!(meta.size, content.len() as u64);
  assert_eq!(meta.sync_status, SyncStatus::Synced);
  assert!(!meta.content_hash.is_empty(), "content_hash should be set");
  assert!(meta.modified_at > 0, "modified_at should be positive");
  assert!(meta.last_synced_at > 0, "last_synced_at should be positive");

  // Verify the hash is a valid blake3 hash of the content.
  let expected_hash = blake3::hash(content).to_hex().to_string();
  assert_eq!(meta.content_hash, expected_hash);
}

#[tokio::test]
async fn test_push_hash_skip_updates_mtime() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  let content = b"same content both times";
  std::fs::write(local_path.join("hashskip.txt"), content).expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  // First push: uploads the file.
  push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  // Manually tamper with the stored mtime so it differs from the filesystem,
  // but keep the same hash. This simulates a "touched" file (mtime changed
  // but content identical).
  let metadata_store = SyncMetadataStore::new(&state);
  let mut meta = metadata_store
    .get_file_meta(&relationship.id, "/docs/hashskip.txt")
    .expect("get failed")
    .expect("should exist");

  meta.modified_at = 1; // Force mtime mismatch.
  metadata_store
    .set_file_meta(&relationship.id, &meta)
    .expect("set failed");

  // Clear the mock server file store to verify nothing is re-uploaded.
  mock_state.files.lock().await.clear();

  // Second push: mtime differs -> reads file -> hashes -> same hash -> skip.
  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(result.files_pushed, 0, "should not re-upload (hash matches)");
  assert_eq!(result.files_skipped, 1, "should skip via hash");

  // Verify mock server received nothing new.
  assert!(mock_state.files.lock().await.is_empty(), "no files should be uploaded");

  // Verify the stored mtime was updated to match the filesystem.
  let updated_meta = metadata_store
    .get_file_meta(&relationship.id, "/docs/hashskip.txt")
    .expect("get failed")
    .expect("should exist");

  assert_ne!(updated_meta.modified_at, 1, "mtime should be updated from the stale value");
}

#[tokio::test]
async fn test_push_nested_directories() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  // Create a deeply nested directory structure.
  std::fs::create_dir_all(local_path.join("a/b/c")).expect("mkdir failed");
  std::fs::write(local_path.join("a/file_a.txt"), b"A").expect("write failed");
  std::fs::write(local_path.join("a/b/file_b.txt"), b"B").expect("write failed");
  std::fs::write(local_path.join("a/b/c/file_c.txt"), b"C").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 3);

  let files = mock_state.files.lock().await;
  assert_eq!(files.get("/docs/a/file_a.txt").map(|b| b.as_slice()), Some(b"A".as_slice()));
  assert_eq!(files.get("/docs/a/b/file_b.txt").map(|b| b.as_slice()), Some(b"B".as_slice()));
  assert_eq!(files.get("/docs/a/b/c/file_c.txt").map(|b| b.as_slice()), Some(b"C".as_slice()));
}

#[tokio::test]
async fn test_push_exclude_filter() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("good.txt"), b"good").expect("write failed");
  std::fs::write(local_path.join("bad.tmp"), b"bad").expect("write failed");
  std::fs::write(local_path.join(".DS_Store"), b"junk").expect("write failed");

  let mut relationship = make_relationship(&local_path.to_string_lossy());
  relationship.filter = Some("!*.tmp, !.DS_Store".to_string());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1, "only good.txt should be pushed");

  let files = mock_state.files.lock().await;
  assert!(files.contains_key("/docs/good.txt"));
  assert!(!files.contains_key("/docs/bad.tmp"));
  assert!(!files.contains_key("/docs/.DS_Store"));
}

#[tokio::test]
async fn test_push_upload_failure_records_error() {
  // Start a server that rejects all PUT requests with 500.
  let failing_state = MockServerState::new();

  async fn handle_upload_fail(
    Path(_path): Path<String>,
  ) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
  }

  let app = Router::new()
    .route("/system/health", get(handle_health))
    .route("/files/{*path}", put(handle_upload_fail))
    .with_state(failing_state);

  let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
  let address = listener.local_addr().expect("addr failed");

  tokio::spawn(async move {
    axum::serve(listener, app).await.expect("server failed");
  });

  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("will-fail.txt"), b"data").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync should still return Ok with errors recorded");

  assert_eq!(result.files_pushed, 0, "no files should succeed");
  assert_eq!(result.files_failed, 1, "one file should fail");
  assert_eq!(result.errors.len(), 1, "one error should be recorded");
  assert!(
    result.errors[0].contains("will-fail.txt"),
    "error message should mention the file",
  );
}

#[tokio::test]
async fn test_push_duration_is_recorded() {
  let (address, _mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("timed.txt"), b"timing test").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  // Duration should be at least 0 and recorded.
  assert!(result.duration_ms < 30000, "push should complete within 30 seconds");
}

#[tokio::test]
async fn test_push_large_number_of_files() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  let file_count = 50;
  for index in 0..file_count {
    let filename = format!("file_{:03}.txt", index);
    let content = format!("content of file {}", index);
    std::fs::write(local_path.join(&filename), content.as_bytes()).expect("write failed");
  }

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, file_count, "all files should be pushed");
  assert_eq!(result.files_failed, 0);

  let files = mock_state.files.lock().await;
  assert_eq!(files.len(), file_count as usize);
}

#[tokio::test]
async fn test_push_remote_path_computation() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  std::fs::write(local_path.join("root.txt"), b"root").expect("write failed");
  std::fs::create_dir_all(local_path.join("sub")).expect("mkdir failed");
  std::fs::write(local_path.join("sub/deep.txt"), b"deep").expect("write failed");

  let mut relationship = make_relationship(&local_path.to_string_lossy());
  relationship.remote_path = "/my-remote-base/".to_string();

  let result = push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 2);

  let files = mock_state.files.lock().await;
  assert!(files.contains_key("/my-remote-base/root.txt"), "root file should use remote base");
  assert!(files.contains_key("/my-remote-base/sub/deep.txt"), "nested file should preserve hierarchy under remote base");
}
