use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
  Arc,
  atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::routing::{delete, get, head, patch, post, put};
use chrono::Utc;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use aeordb_client_lib::connections::{AuthType, RemoteConnection};
use aeordb_client_lib::error::Result as ClientResult;
use aeordb_client_lib::jwt_cache::JwtCache;
use aeordb_client_lib::remote::chunk_hash;
use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::metadata::{FileSyncMeta, SyncMetadataStore, SyncStatus};
use aeordb_client_lib::sync::push::{PushResult, PushScanMode, push_sync};
use aeordb_client_lib::sync::relationships::{DeletePropagation, SyncDirection, SyncRelationship};

// --- Mock server state ---

#[derive(Debug, Clone)]
struct MockServerState {
  /// Uploaded files: remote_path -> content bytes
  files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
  /// Uploaded blob chunks: hash -> content bytes
  chunks: Arc<Mutex<HashMap<String, Vec<u8>>>>,
  /// Created symlinks: remote_path -> target
  symlinks: Arc<Mutex<HashMap<String, String>>>,
  /// Artificial delay for symlink creation tests.
  symlink_delay: Arc<Mutex<Duration>>,
  /// Current symlink requests in flight.
  active_symlink_requests: Arc<AtomicUsize>,
  /// Highest symlink request concurrency observed by the mock server.
  max_symlink_requests: Arc<AtomicUsize>,
  /// Deleted paths
  deleted: Arc<Mutex<Vec<String>>>,
  /// Rename operations: (from, to)
  renamed: Arc<Mutex<Vec<(String, String)>>>,
  /// Number of blob commit requests received.
  blob_commit_requests: Arc<AtomicUsize>,
  /// Number of blob check requests received.
  blob_check_requests: Arc<AtomicUsize>,
  /// Largest number of hashes received in a single blob check request.
  max_blob_check_hashes: Arc<AtomicUsize>,
  /// Optional status code to force every blob check request to return.
  fail_blob_check_status: Arc<AtomicUsize>,
  /// Optional status code to force every chunk upload request to return.
  fail_upload_chunk_status: Arc<AtomicUsize>,
  /// Optional status code to force every blob commit request to return.
  fail_blob_commit_status: Arc<AtomicUsize>,
  /// Return 401 after this many successful blob commit requests.
  fail_blob_commit_after_successes: Arc<AtomicUsize>,
  /// Return `/files/query` responses with the current engine `items` envelope.
  search_items_envelope: Arc<AtomicUsize>,
  /// Return a syntactically valid but incompatible `/files/query` response.
  malformed_search_response: Arc<AtomicUsize>,
}

impl MockServerState {
  fn new() -> Self {
    Self {
      files: Arc::new(Mutex::new(HashMap::new())),
      chunks: Arc::new(Mutex::new(HashMap::new())),
      symlinks: Arc::new(Mutex::new(HashMap::new())),
      symlink_delay: Arc::new(Mutex::new(Duration::ZERO)),
      active_symlink_requests: Arc::new(AtomicUsize::new(0)),
      max_symlink_requests: Arc::new(AtomicUsize::new(0)),
      deleted: Arc::new(Mutex::new(Vec::new())),
      renamed: Arc::new(Mutex::new(Vec::new())),
      blob_commit_requests: Arc::new(AtomicUsize::new(0)),
      blob_check_requests: Arc::new(AtomicUsize::new(0)),
      max_blob_check_hashes: Arc::new(AtomicUsize::new(0)),
      fail_blob_check_status: Arc::new(AtomicUsize::new(0)),
      fail_upload_chunk_status: Arc::new(AtomicUsize::new(0)),
      fail_blob_commit_status: Arc::new(AtomicUsize::new(0)),
      fail_blob_commit_after_successes: Arc::new(AtomicUsize::new(usize::MAX)),
      search_items_envelope: Arc::new(AtomicUsize::new(0)),
      malformed_search_response: Arc::new(AtomicUsize::new(0)),
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

  let active = state.active_symlink_requests.fetch_add(1, Ordering::SeqCst) + 1;
  let mut observed = state.max_symlink_requests.load(Ordering::SeqCst);
  while active > observed {
    match state.max_symlink_requests.compare_exchange(
      observed,
      active,
      Ordering::SeqCst,
      Ordering::SeqCst,
    ) {
      Ok(_) => break,
      Err(current) => observed = current,
    }
  }

  let delay = *state.symlink_delay.lock().await;
  if !delay.is_zero() {
    tokio::time::sleep(delay).await;
  }

  state.symlinks.lock().await.insert(remote_path, target);
  state.active_symlink_requests.fetch_sub(1, Ordering::SeqCst);
  StatusCode::OK
}

async fn handle_head_file(
  Path(path): Path<String>,
  State(state): State<MockServerState>,
) -> (StatusCode, HeaderMap) {
  let remote_path = format!("/{}", path);
  let mut headers = HeaderMap::new();

  if let Some(target) = state.symlinks.lock().await.get(&remote_path).cloned() {
    headers.insert("x-aeordb-entry-type", HeaderValue::from_static("symlink"));
    if let Ok(value) = HeaderValue::from_str(&target) {
      headers.insert("x-aeordb-symlink-target", value);
    }
    return (StatusCode::OK, headers);
  }

  if state.files.lock().await.contains_key(&remote_path) {
    headers.insert("x-aeordb-entry-type", HeaderValue::from_static("file"));
    return (StatusCode::OK, headers);
  }

  (StatusCode::NOT_FOUND, headers)
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

#[derive(Debug, Deserialize)]
struct RenameRequest {
  to: String,
}

async fn handle_rename(
  Path(path): Path<String>,
  State(state): State<MockServerState>,
  axum::Json(request): axum::Json<RenameRequest>,
) -> StatusCode {
  let from_path = format!("/{}", path);
  let to_path = request.to;
  let mut files = state.files.lock().await;
  if files.contains_key(&to_path) {
    return StatusCode::CONFLICT;
  }

  let Some(content) = files.remove(&from_path) else {
    return StatusCode::NOT_FOUND;
  };

  files.insert(to_path.clone(), content);
  drop(files);
  state.renamed.lock().await.push((from_path, to_path));
  StatusCode::OK
}

async fn handle_search(
  State(state): State<MockServerState>,
  axum::Json(request): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
  if state.malformed_search_response.load(Ordering::SeqCst) > 0 {
    return axum::Json(serde_json::json!({
      "unexpected": true,
    }));
  }

  let root = request
    .get("path")
    .and_then(|value| value.as_str())
    .unwrap_or("/");
  let Some(where_clause) = request.get("where") else {
    return axum::Json(serde_json::json!({ "results": [], "has_more": false }));
  };
  let limit = request
    .get("limit")
    .and_then(|value| value.as_u64())
    .unwrap_or(1000) as usize;

  let mut expected_path: Option<String> = None;
  let mut expected_hash: Option<String> = None;
  collect_virtual_eq_constraints(where_clause, &mut expected_path, &mut expected_hash);

  let files = state.files.lock().await;
  let mut results = Vec::new();
  for (path, bytes) in files.iter() {
    if !path.starts_with(root) {
      continue;
    }
    if let Some(expected_path) = expected_path.as_deref() {
      if path != expected_path {
        continue;
      }
    }
    let content_hash = blake3::hash(bytes).to_hex().to_string();
    if expected_hash.as_deref() == Some(content_hash.as_str()) {
      results.push(serde_json::json!({ "path": path }));
      if results.len() >= limit {
        break;
      }
    }
  }

  let total_count = results.len();
  if state.search_items_envelope.load(Ordering::SeqCst) > 0 {
    return axum::Json(serde_json::json!({
      "items": results,
      "has_more": false,
      "total": total_count,
    }));
  }

  axum::Json(serde_json::json!({
    "results": results,
    "has_more": false,
    "total_count": total_count,
  }))
}

fn collect_virtual_eq_constraints(
  clause: &serde_json::Value,
  expected_path: &mut Option<String>,
  expected_hash: &mut Option<String>,
) {
  if let Some(children) = clause.get("and").and_then(|value| value.as_array()) {
    for child in children {
      collect_virtual_eq_constraints(child, expected_path, expected_hash);
    }
    return;
  }

  if clause.get("op").and_then(|value| value.as_str()) != Some("eq") {
    return;
  }

  let Some(field) = clause.get("field").and_then(|value| value.as_str()) else {
    return;
  };
  let Some(value) = clause.get("value").and_then(|value| value.as_str()) else {
    return;
  };

  match field {
    "@path" => *expected_path = Some(value.to_string()),
    "@hash" => *expected_hash = Some(value.to_string()),
    _ => {}
  }
}

async fn handle_health() -> StatusCode {
  StatusCode::OK
}

async fn handle_blob_config() -> axum::Json<serde_json::Value> {
  axum::Json(serde_json::json!({
    "hash_algorithm": "blake3",
    "chunk_size": 4,
    "chunk_hash_prefix": "chunk:"
  }))
}

#[derive(Debug, Deserialize)]
struct BlobCheckRequest {
  hashes: Vec<String>,
}

async fn handle_blob_check(
  State(state): State<MockServerState>,
  axum::Json(request): axum::Json<BlobCheckRequest>,
) -> std::result::Result<axum::Json<serde_json::Value>, StatusCode> {
  state.blob_check_requests.fetch_add(1, Ordering::SeqCst);
  let hash_count = request.hashes.len();
  let mut observed = state.max_blob_check_hashes.load(Ordering::SeqCst);
  while hash_count > observed {
    match state.max_blob_check_hashes.compare_exchange(
      observed,
      hash_count,
      Ordering::SeqCst,
      Ordering::SeqCst,
    ) {
      Ok(_) => break,
      Err(current) => observed = current,
    }
  }

  let forced_status = state.fail_blob_check_status.load(Ordering::SeqCst);
  if forced_status != 0 {
    return Err(
      StatusCode::from_u16(forced_status as u16).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
    );
  }

  let chunks = state.chunks.lock().await;
  let mut have = Vec::new();
  let mut needed = Vec::new();

  for hash in request.hashes {
    if chunks.contains_key(&hash) {
      have.push(hash);
    } else {
      needed.push(hash);
    }
  }

  Ok(axum::Json(serde_json::json!({
    "have": have,
    "needed": needed,
  })))
}

async fn handle_upload_chunk(
  Path(hash): Path<String>,
  State(state): State<MockServerState>,
  body: Bytes,
) -> StatusCode {
  let forced_status = state.fail_upload_chunk_status.load(Ordering::SeqCst);
  if forced_status != 0 {
    return StatusCode::from_u16(forced_status as u16).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
  }

  let expected = chunk_hash("chunk:", &body);
  if expected != hash {
    return StatusCode::BAD_REQUEST;
  }

  state.chunks.lock().await.insert(hash, body.to_vec());
  StatusCode::CREATED
}

#[derive(Debug, Deserialize)]
struct BlobCommitRequest {
  files: Vec<BlobCommitFile>,
}

#[derive(Debug, Deserialize)]
struct BlobCommitFile {
  path: String,
  chunks: Vec<String>,
  content_hash: Option<String>,
  size: Option<u64>,
}

async fn handle_blob_commit(
  State(state): State<MockServerState>,
  axum::Json(request): axum::Json<BlobCommitRequest>,
) -> std::result::Result<axum::Json<serde_json::Value>, StatusCode> {
  let request_index = state.blob_commit_requests.fetch_add(1, Ordering::SeqCst);
  let forced_status = state.fail_blob_commit_status.load(Ordering::SeqCst);
  if forced_status != 0 {
    return Err(
      StatusCode::from_u16(forced_status as u16).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
    );
  }

  if request_index
    >= state
      .fail_blob_commit_after_successes
      .load(Ordering::SeqCst)
  {
    return Err(StatusCode::UNAUTHORIZED);
  }

  let chunks = state.chunks.lock().await;
  let mut committed = Vec::new();

  for file in request.files {
    let mut body = Vec::new();
    for hash in file.chunks {
      let Some(chunk) = chunks.get(&hash) else {
        return Err(StatusCode::BAD_REQUEST);
      };
      body.extend_from_slice(chunk);
    }
    let expected_hash = blake3::hash(&body).to_hex().to_string();
    if file.content_hash.as_deref() != Some(expected_hash.as_str()) {
      return Err(StatusCode::BAD_REQUEST);
    }
    if file.size != Some(body.len() as u64) {
      return Err(StatusCode::BAD_REQUEST);
    }
    committed.push((file.path, body));
  }

  drop(chunks);
  let mut files = state.files.lock().await;
  for (path, body) in committed {
    files.insert(path, body);
  }

  Ok(axum::Json(serde_json::json!({ "committed": true })))
}

// --- Test helpers ---

async fn start_mock_server() -> (SocketAddr, MockServerState) {
  let state = MockServerState::new();

  let app = Router::new()
    .route("/system/health", get(handle_health))
    .route("/blobs/config", get(handle_blob_config))
    .route("/blobs/check", post(handle_blob_check))
    .route("/blobs/chunks/{hash}", put(handle_upload_chunk))
    .route("/blobs/commit", post(handle_blob_commit))
    .route("/files/query", post(handle_search))
    .route("/files/search", post(handle_search))
    .route("/files/{*path}", put(handle_upload))
    .route("/files/{*path}", head(handle_head_file))
    .route("/files/{*path}", patch(handle_rename))
    .route("/files/{*path}", delete(handle_delete))
    .route("/links/{*path}", put(handle_create_symlink))
    .with_state(state.clone());

  let listener = TcpListener::bind("127.0.0.1:0")
    .await
    .expect("failed to bind");
  let address = listener.local_addr().expect("failed to get address");

  tokio::spawn(async move {
    axum::serve(listener, app)
      .await
      .expect("mock server failed");
  });

  (address, state)
}

fn create_state_store() -> (StateStore, std::path::PathBuf) {
  let temp_dir = tempfile::tempdir()
    .expect("failed to create temp dir")
    .keep();
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
    id: "test-conn-001".to_string(),
    name: "test-mock".to_string(),
    url: format!("http://{}", address),
    auth_type: AuthType::None,
    api_key: None,
    share_base_url: None,
    created_at: now,
    updated_at: now,
  }
}

fn make_relationship(local_path: &str) -> SyncRelationship {
  let now = Utc::now();

  SyncRelationship {
    id: "test-rel-001".to_string(),
    name: "test-sync".to_string(),
    remote_connection_id: "test-conn-001".to_string(),
    remote_path: "/docs/".to_string(),
    local_path: local_path.to_string(),
    direction: SyncDirection::PushOnly,
    filter: None,
    delete_propagation: DeletePropagation::default(),
    enabled: true,
    created_at: now,
    updated_at: now,
  }
}

fn synced_file_meta(path: &str, content: &[u8]) -> FileSyncMeta {
  FileSyncMeta {
    path: path.to_string(),
    content_hash: blake3::hash(content).to_hex().to_string(),
    size: content.len() as u64,
    modified_at: 1700000000000,
    sync_status: SyncStatus::Synced,
    last_synced_at: 1700000001000,
  }
}

async fn run_push_sync(
  state: &StateStore,
  connection: &RemoteConnection,
  relationship: &SyncRelationship,
) -> ClientResult<PushResult> {
  let all_relationships = vec![relationship.clone()];
  let http_client = reqwest::Client::new();
  let jwt_cache = JwtCache::new();

  push_sync(
    state,
    connection,
    relationship,
    &all_relationships,
    &http_client,
    &jwt_cache,
    PushScanMode::Lite,
    None,
  )
  .await
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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 3, "should push 3 files");
  assert_eq!(result.files_failed, 0, "no failures expected");
  assert!(result.errors.is_empty(), "no errors expected");
  assert!(result.total_bytes > 0, "should have transferred bytes");

  // Verify files arrived at the mock server.
  let files = mock_state.files.lock().await;
  assert_eq!(
    files.get("/docs/report.pdf").map(|b| b.as_slice()),
    Some(b"pdf-content-here".as_slice())
  );
  assert_eq!(
    files.get("/docs/notes.txt").map(|b| b.as_slice()),
    Some(b"some notes".as_slice())
  );
  assert_eq!(
    files.get("/docs/subdir/nested.md").map(|b| b.as_slice()),
    Some(b"# Nested doc".as_slice())
  );
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
  let result_1 = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  assert_eq!(
    result_1.files_pushed, 1,
    "first push should upload the file"
  );

  // Second push without modifying the file: mtime has not changed,
  // so the file should be skipped.
  let result_2 = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(
    result_2.files_pushed, 0,
    "second push should upload nothing"
  );
  assert_eq!(
    result_2.files_skipped, 1,
    "second push should skip the file"
  );
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
  let result_1 = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  assert_eq!(result_1.files_pushed, 1);

  // Modify the file (change content AND mtime).
  // Sleep briefly so the filesystem mtime actually changes.
  tokio::time::sleep(std::time::Duration::from_millis(50)).await;
  std::fs::write(local_path.join("mutable.txt"), b"version 2").expect("write failed");

  // Second push: should detect the modification and re-upload.
  let result_2 = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(result_2.files_pushed, 1, "should re-upload modified file");
  assert_eq!(
    result_2.files_skipped, 0,
    "should not skip the modified file"
  );

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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1, "only the PDF should be pushed");
  assert_eq!(
    result.files_skipped, 2,
    "two files should be skipped by filter"
  );

  let files = mock_state.files.lock().await;
  assert!(
    files.contains_key("/docs/included.pdf"),
    "PDF should be on remote"
  );
  assert!(
    !files.contains_key("/docs/excluded.txt"),
    "TXT should not be on remote"
  );
  assert!(
    !files.contains_key("/docs/also-excluded.rs"),
    "RS should not be on remote"
  );
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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  // Should push 1 regular file + 1 symlink.
  assert_eq!(result.files_pushed, 2, "should push file and symlink");
  assert_eq!(result.files_failed, 0, "no failures expected");

  // Verify the symlink was created on the remote.
  let symlinks = mock_state.symlinks.lock().await;
  assert!(
    symlinks.contains_key("/docs/link.txt"),
    "symlink should exist on remote"
  );

  let symlink_target = &symlinks["/docs/link.txt"];
  assert!(
    symlink_target.contains("target.txt"),
    "symlink target should reference target.txt, got: {}",
    symlink_target,
  );
}

#[tokio::test]
async fn test_push_bootstraps_existing_remote_symlink_metadata() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  std::os::unix::fs::symlink("target.txt", local_path.join("link.txt")).expect("symlink failed");

  mock_state
    .symlinks
    .lock()
    .await
    .insert("/docs/link.txt".to_string(), "target.txt".to_string());

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(
    result.files_pushed, 0,
    "existing remote symlink should not be re-written"
  );
  assert_eq!(
    result.files_skipped, 1,
    "symlink should be recorded as unchanged"
  );
  assert_eq!(result.files_failed, 0, "no failures expected");
  assert_eq!(
    mock_state.max_symlink_requests.load(Ordering::SeqCst),
    0,
    "no PUT /links calls should be needed for an already-current remote symlink"
  );

  let metadata_store = SyncMetadataStore::new(&state);
  let meta = metadata_store
    .get_file_meta(&relationship.id, "/docs/link.txt")
    .expect("metadata lookup failed")
    .expect("symlink metadata should be stored");
  assert_eq!(meta.sync_status, SyncStatus::Synced);
}

#[tokio::test]
async fn test_push_symlinks_are_not_serialized_during_scan() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  *mock_state.symlink_delay.lock().await = Duration::from_millis(75);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  for index in 0..8 {
    std::os::unix::fs::symlink(
      format!("target-{index}.txt"),
      local_path.join(format!("link-{index}.txt")),
    )
    .expect("symlink failed");
  }

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 8, "all symlinks should be pushed");
  assert_eq!(result.files_failed, 0, "no symlink failures expected");
  assert_eq!(
    mock_state.symlinks.lock().await.len(),
    8,
    "mock server should receive every symlink",
  );

  let available_workers = std::thread::available_parallelism()
    .map(|parallelism| parallelism.get())
    .unwrap_or(1);
  if available_workers > 1 {
    assert!(
      mock_state.max_symlink_requests.load(Ordering::SeqCst) > 1,
      "symlink pushes should run concurrently instead of serializing during scan",
    );
  }
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
  let result_1 = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  assert_eq!(result_1.files_pushed, 2);

  // Delete the file from the local filesystem.
  std::fs::remove_file(local_path.join("remove.txt")).expect("remove failed");

  // Second push: should detect the deletion and propagate it.
  let result_2 = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(
    result_2.files_deleted, 1,
    "should delete 1 file from remote"
  );

  let deleted = mock_state.deleted.lock().await;
  assert!(
    deleted.contains(&"/docs/remove.txt".to_string()),
    "remove.txt should be deleted from remote"
  );
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
  run_push_sync(&state, &connection, &relationship)
    .await
    .expect("first push failed");

  std::fs::remove_file(local_path.join("file.txt")).expect("remove failed");

  // Second push: should NOT delete from remote.
  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(
    result.files_deleted, 0,
    "should not delete when propagation disabled"
  );

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

  let result = run_push_sync(&state, &connection, &relationship)
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

  let result = run_push_sync(&state, &connection, &relationship).await;

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

  run_push_sync(&state, &connection, &relationship)
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
async fn test_push_persists_successful_batch_metadata_before_later_abort() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();

  // The mock blob config uses 4-byte chunks. A file over 32 KiB exceeds the
  // 8,192 chunk batch limit, forcing one file per remote commit without
  // making the test expensive.
  std::fs::write(local_path.join("a.bin"), vec![b'a'; 33_000]).expect("write failed");
  std::fs::write(local_path.join("b.bin"), vec![b'b'; 33_000]).expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());
  mock_state
    .fail_blob_commit_after_successes
    .store(1, Ordering::SeqCst);

  let error = match run_push_sync(&state, &connection, &relationship).await {
    Ok(_) => panic!("second commit should abort after the first commit succeeds"),
    Err(error) => error,
  };
  assert!(
    error.to_string().contains("unrecoverable"),
    "401 commit failure should abort the sync, got: {}",
    error,
  );

  let metadata_store = SyncMetadataStore::new(&state);
  let durable_successes = ["/docs/a.bin", "/docs/b.bin"]
    .into_iter()
    .filter(|path| {
      metadata_store
        .get_file_meta(&relationship.id, path)
        .expect("metadata lookup failed")
        .is_some()
    })
    .count();

  assert_eq!(
    durable_successes, 1,
    "metadata for a successful committed batch must be durable before a later batch aborts",
  );
}

#[tokio::test]
async fn test_push_does_not_rename_arbitrary_same_hash_without_migration() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  let content = b"same bytes at a new unrelated path";
  std::fs::write(local_path.join("new-name.txt"), content).expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());
  mock_state
    .files
    .lock()
    .await
    .insert("/docs/old-name.txt".to_string(), content.to_vec());

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta("/docs/old-name.txt", content),
    )
    .expect("failed to seed old metadata");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1);
  assert!(
    mock_state.renamed.lock().await.is_empty(),
    "same content at a new path must upload/commit, not invent a remote move",
  );

  let files = mock_state.files.lock().await;
  assert_eq!(
    files.get("/docs/old-name.txt").map(|b| b.as_slice()),
    Some(content.as_slice()),
    "old remote path should remain untouched without a migration marker",
  );
  assert_eq!(
    files.get("/docs/new-name.txt").map(|b| b.as_slice()),
    Some(content.as_slice()),
    "new path should be committed independently",
  );
}

#[tokio::test]
async fn test_push_migration_renames_recorded_old_path_after_hash_confirmation() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let parent_dir = tempfile::tempdir().expect("failed to create local dir");
  let parent_path = parent_dir.path();
  let old_local_base = parent_path.join("Pictures");
  std::fs::create_dir_all(&old_local_base).expect("mkdir failed");
  let content = b"photo bytes that were already on the remote";
  std::fs::write(old_local_base.join("photo.jpg"), content).expect("write failed");

  let mut relationship = make_relationship(&parent_path.to_string_lossy());
  relationship.remote_path = "/remote/".to_string();

  let old_remote_path = "/remote-pictures/photo.jpg";
  let new_remote_path = "/remote/Pictures/photo.jpg";
  mock_state
    .files
    .lock()
    .await
    .insert(old_remote_path.to_string(), content.to_vec());

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta(old_remote_path, content),
    )
    .expect("failed to seed old metadata");
  metadata_store
    .begin_path_migration(
      &relationship.id,
      "/remote-pictures/",
      "/remote/",
      &old_local_base.to_string_lossy(),
      &parent_path.to_string_lossy(),
    )
    .expect("failed to seed migration");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1);
  assert_eq!(
    mock_state.renamed.lock().await.as_slice(),
    &[(old_remote_path.to_string(), new_remote_path.to_string())],
    "migration should perform one metadata-only remote move",
  );

  let files = mock_state.files.lock().await;
  assert!(
    !files.contains_key(old_remote_path),
    "old remote path should be moved away",
  );
  assert_eq!(
    files.get(new_remote_path).map(|b| b.as_slice()),
    Some(content.as_slice()),
    "new remote path should hold the existing content",
  );
  drop(files);

  assert!(
    metadata_store
      .get_file_meta(&relationship.id, old_remote_path)
      .expect("failed to get old metadata")
      .is_none(),
    "old path metadata should be removed after migration move",
  );
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, new_remote_path)
      .expect("failed to get new metadata")
      .is_some(),
    "new path metadata should be created after migration move",
  );
  assert!(
    metadata_store
      .get_path_migration(&relationship.id)
      .expect("failed to get migration")
      .is_none(),
    "successful push should clear the completed migration marker",
  );
}

#[tokio::test]
async fn test_push_migration_skips_commit_when_target_already_has_hash() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let parent_dir = tempfile::tempdir().expect("failed to create local dir");
  let parent_path = parent_dir.path();
  let old_local_base = parent_path.join("Pictures");
  std::fs::create_dir_all(&old_local_base).expect("mkdir failed");
  let content = b"photo bytes committed by an earlier interrupted migration";
  std::fs::write(old_local_base.join("photo.jpg"), content).expect("write failed");

  let mut relationship = make_relationship(&parent_path.to_string_lossy());
  relationship.remote_path = "/remote/".to_string();

  let old_remote_path = "/remote-pictures/photo.jpg";
  let new_remote_path = "/remote/Pictures/photo.jpg";
  mock_state
    .files
    .lock()
    .await
    .insert(new_remote_path.to_string(), content.to_vec());

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta(old_remote_path, content),
    )
    .expect("failed to seed old metadata");
  metadata_store
    .begin_path_migration(
      &relationship.id,
      "/remote-pictures/",
      "/remote/",
      &old_local_base.to_string_lossy(),
      &parent_path.to_string_lossy(),
    )
    .expect("failed to seed migration");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 0);
  assert_eq!(result.files_skipped, 1);
  assert!(
    mock_state.renamed.lock().await.is_empty(),
    "already-materialized target should not need a remote rename",
  );
  assert_eq!(
    mock_state.blob_commit_requests.load(Ordering::SeqCst),
    0,
    "already-materialized target should not be committed again",
  );
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, old_remote_path)
      .expect("failed to get old metadata")
      .is_none(),
    "old path metadata should be removed after adopting the target",
  );
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, new_remote_path)
      .expect("failed to get new metadata")
      .is_some(),
    "new path metadata should be created after adopting the target",
  );
  assert!(
    metadata_store
      .get_path_migration(&relationship.id)
      .expect("failed to get migration")
      .is_none(),
    "successful adoption should clear the completed migration marker",
  );
}

#[tokio::test]
async fn test_push_migration_accepts_items_query_envelope() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);
  mock_state.search_items_envelope.store(1, Ordering::SeqCst);

  let parent_dir = tempfile::tempdir().expect("failed to create local dir");
  let parent_path = parent_dir.path();
  let old_local_base = parent_path.join("Pictures");
  std::fs::create_dir_all(&old_local_base).expect("mkdir failed");
  let content = b"photo bytes already materialized at the migration target";
  std::fs::write(old_local_base.join("photo.jpg"), content).expect("write failed");

  let mut relationship = make_relationship(&parent_path.to_string_lossy());
  relationship.remote_path = "/remote/".to_string();

  let old_remote_path = "/remote-pictures/photo.jpg";
  let new_remote_path = "/remote/Pictures/photo.jpg";
  mock_state
    .files
    .lock()
    .await
    .insert(new_remote_path.to_string(), content.to_vec());

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta(old_remote_path, content),
    )
    .expect("failed to seed old metadata");
  metadata_store
    .begin_path_migration(
      &relationship.id,
      "/remote-pictures/",
      "/remote/",
      &old_local_base.to_string_lossy(),
      &parent_path.to_string_lossy(),
    )
    .expect("failed to seed migration");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_failed, 0);
  assert_eq!(result.files_pushed, 0);
  assert_eq!(result.files_skipped, 1);
  assert!(
    mock_state.renamed.lock().await.is_empty(),
    "target adoption should not require a remote rename",
  );
  assert_eq!(
    mock_state.blob_commit_requests.load(Ordering::SeqCst),
    0,
    "target adoption should not recommit an already-materialized file",
  );
}

#[tokio::test]
async fn test_push_migration_query_decode_failure_falls_back_to_commit() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);
  mock_state
    .malformed_search_response
    .store(1, Ordering::SeqCst);

  let parent_dir = tempfile::tempdir().expect("failed to create local dir");
  let parent_path = parent_dir.path();
  let old_local_base = parent_path.join("Pictures");
  std::fs::create_dir_all(&old_local_base).expect("mkdir failed");
  let content = b"photo bytes that should commit when migration verification is unavailable";
  std::fs::write(old_local_base.join("photo.jpg"), content).expect("write failed");

  let mut relationship = make_relationship(&parent_path.to_string_lossy());
  relationship.remote_path = "/remote/".to_string();

  let old_remote_path = "/remote-pictures/photo.jpg";
  let new_remote_path = "/remote/Pictures/photo.jpg";
  mock_state
    .files
    .lock()
    .await
    .insert(old_remote_path.to_string(), content.to_vec());

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta(old_remote_path, content),
    )
    .expect("failed to seed old metadata");
  metadata_store
    .begin_path_migration(
      &relationship.id,
      "/remote-pictures/",
      "/remote/",
      &old_local_base.to_string_lossy(),
      &parent_path.to_string_lossy(),
    )
    .expect("failed to seed migration");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_failed, 0);
  assert!(
    result.errors.is_empty(),
    "migration query decode failures should not surface as per-file sync failures",
  );
  assert_eq!(result.files_pushed, 1);
  assert!(
    mock_state.renamed.lock().await.is_empty(),
    "the client must not move a remote file when migration verification cannot confirm it",
  );

  let files = mock_state.files.lock().await;
  assert_eq!(
    files.get(old_remote_path).map(|b| b.as_slice()),
    Some(content.as_slice()),
    "unverified old remote path should be left untouched",
  );
  assert_eq!(
    files.get(new_remote_path).map(|b| b.as_slice()),
    Some(content.as_slice()),
    "normal commit should materialize the file at the new remote path",
  );
}

#[tokio::test]
async fn test_push_migration_does_not_delete_stale_old_metadata() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let parent_dir = tempfile::tempdir().expect("failed to create local dir");
  let parent_path = parent_dir.path();
  let old_local_base = parent_path.join("Pictures");
  std::fs::create_dir_all(&old_local_base).expect("mkdir failed");
  let content = b"remote content whose local file vanished before migration";

  let mut relationship = make_relationship(&parent_path.to_string_lossy());
  relationship.remote_path = "/remote/".to_string();
  relationship.delete_propagation = DeletePropagation {
    local_to_remote: true,
    remote_to_local: false,
  };

  let old_remote_path = "/remote-pictures/missing.jpg";
  mock_state
    .files
    .lock()
    .await
    .insert(old_remote_path.to_string(), content.to_vec());

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta(old_remote_path, content),
    )
    .expect("failed to seed old metadata");
  metadata_store
    .begin_path_migration(
      &relationship.id,
      "/remote-pictures/",
      "/remote/",
      &old_local_base.to_string_lossy(),
      &parent_path.to_string_lossy(),
    )
    .expect("failed to seed migration");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_deleted, 0);
  assert!(
    mock_state.deleted.lock().await.is_empty(),
    "migration cleanup must not propagate local absence as a remote delete",
  );
  assert!(
    mock_state.files.lock().await.contains_key(old_remote_path),
    "remote file should be left alone when local file no longer maps",
  );
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, old_remote_path)
      .expect("failed to get old metadata")
      .is_none(),
    "stale old metadata should be dropped so future scans do not revisit it",
  );
}

#[tokio::test]
async fn test_push_migration_rename_failure_preserves_old_metadata_and_marker() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let parent_dir = tempfile::tempdir().expect("failed to create local dir");
  let parent_path = parent_dir.path();
  let old_local_base = parent_path.join("Pictures");
  std::fs::create_dir_all(&old_local_base).expect("mkdir failed");
  let content = b"photo bytes already on the remote";
  std::fs::write(old_local_base.join("photo.jpg"), content).expect("write failed");

  let mut relationship = make_relationship(&parent_path.to_string_lossy());
  relationship.remote_path = "/remote/".to_string();
  relationship.delete_propagation = DeletePropagation {
    local_to_remote: true,
    remote_to_local: false,
  };

  let old_remote_path = "/remote-pictures/photo.jpg";
  let new_remote_path = "/remote/Pictures/photo.jpg";
  {
    let mut files = mock_state.files.lock().await;
    files.insert(old_remote_path.to_string(), content.to_vec());
    files.insert(
      new_remote_path.to_string(),
      b"existing destination".to_vec(),
    );
  }

  let metadata_store = SyncMetadataStore::new(&state);
  metadata_store
    .set_file_meta(
      &relationship.id,
      &synced_file_meta(old_remote_path, content),
    )
    .expect("failed to seed old metadata");
  metadata_store
    .begin_path_migration(
      &relationship.id,
      "/remote-pictures/",
      "/remote/",
      &old_local_base.to_string_lossy(),
      &parent_path.to_string_lossy(),
    )
    .expect("failed to seed migration");

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync should record migration error, not abort the whole cycle");

  assert_eq!(result.files_failed, 1);
  assert!(
    result.errors[0].contains("failed to migrate remote"),
    "migration rename conflict should be surfaced",
  );
  assert!(
    mock_state.deleted.lock().await.is_empty(),
    "failed migration must not fall through into delete propagation",
  );
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, old_remote_path)
      .expect("failed to get old metadata")
      .is_some(),
    "old metadata must remain so a later retry can attempt the move again",
  );
  assert!(
    metadata_store
      .get_path_migration(&relationship.id)
      .expect("failed to get migration marker")
      .is_some(),
    "migration marker must remain after partial failure",
  );
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
  run_push_sync(&state, &connection, &relationship)
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
  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("second push failed");

  assert_eq!(
    result.files_pushed, 0,
    "should not re-upload (hash matches)"
  );
  assert_eq!(result.files_skipped, 1, "should skip via hash");

  // Verify mock server received nothing new.
  assert!(
    mock_state.files.lock().await.is_empty(),
    "no files should be uploaded"
  );

  // Verify the stored mtime was updated to match the filesystem.
  let updated_meta = metadata_store
    .get_file_meta(&relationship.id, "/docs/hashskip.txt")
    .expect("get failed")
    .expect("should exist");

  assert_ne!(
    updated_meta.modified_at, 1,
    "mtime should be updated from the stale value"
  );
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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 3);

  let files = mock_state.files.lock().await;
  assert_eq!(
    files.get("/docs/a/file_a.txt").map(|b| b.as_slice()),
    Some(b"A".as_slice())
  );
  assert_eq!(
    files.get("/docs/a/b/file_b.txt").map(|b| b.as_slice()),
    Some(b"B".as_slice())
  );
  assert_eq!(
    files.get("/docs/a/b/c/file_c.txt").map(|b| b.as_slice()),
    Some(b"C".as_slice())
  );
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

  let result = run_push_sync(&state, &connection, &relationship)
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
  // Start a server that rejects chunk uploads with 500.
  let failing_state = MockServerState::new();

  async fn handle_upload_chunk_fail(Path(_hash): Path<String>) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
  }

  let app = Router::new()
    .route("/system/health", get(handle_health))
    .route("/blobs/config", get(handle_blob_config))
    .route("/blobs/check", post(handle_blob_check))
    .route("/blobs/chunks/{hash}", put(handle_upload_chunk_fail))
    .route("/blobs/commit", post(handle_blob_commit))
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

  let result = run_push_sync(&state, &connection, &relationship)
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
async fn test_push_transient_blob_check_failure_aborts_without_file_failures() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  mock_state.fail_blob_check_status.store(
    StatusCode::SERVICE_UNAVAILABLE.as_u16() as usize,
    Ordering::SeqCst,
  );

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  std::fs::write(local_path.join("retry-later.txt"), b"remote is starting").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let error = match run_push_sync(&state, &connection, &relationship).await {
    Ok(_) => panic!("transient blob_check failure should abort the sync attempt"),
    Err(error) => error,
  };

  assert!(
    error.is_transient_upstream(),
    "503 should stay classified as transient upstream, got: {}",
    error,
  );
  assert_eq!(
    mock_state.blob_check_requests.load(Ordering::SeqCst),
    1,
    "transient remote failure must not fall back to per-file isolation",
  );

  let metadata_store = SyncMetadataStore::new(&state);
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, "/docs/retry-later.txt")
      .expect("metadata lookup failed")
      .is_none(),
    "transient remote failure should not record the file as synced or failed",
  );
}

#[tokio::test]
async fn test_push_transient_chunk_upload_failure_aborts_without_file_failures() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  mock_state.fail_upload_chunk_status.store(
    StatusCode::SERVICE_UNAVAILABLE.as_u16() as usize,
    Ordering::SeqCst,
  );

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  std::fs::write(
    local_path.join("retry-upload.txt"),
    b"chunk upload should wait",
  )
  .expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let error = match run_push_sync(&state, &connection, &relationship).await {
    Ok(_) => panic!("transient chunk upload failure should abort the sync attempt"),
    Err(error) => error,
  };

  assert!(
    error.is_transient_upstream(),
    "503 should stay classified as transient upstream, got: {}",
    error,
  );
  assert_eq!(
    mock_state.blob_commit_requests.load(Ordering::SeqCst),
    0,
    "transient chunk upload failure must not proceed to commit or per-file fallback",
  );

  let metadata_store = SyncMetadataStore::new(&state);
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, "/docs/retry-upload.txt")
      .expect("metadata lookup failed")
      .is_none(),
    "transient chunk upload failure should not record the file as synced or failed",
  );
}

#[tokio::test]
async fn test_push_transient_blob_commit_failure_aborts_without_file_failures() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  mock_state
    .fail_blob_commit_status
    .store(StatusCode::BAD_GATEWAY.as_u16() as usize, Ordering::SeqCst);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  std::fs::write(local_path.join("retry-commit.txt"), b"commit should wait").expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let error = match run_push_sync(&state, &connection, &relationship).await {
    Ok(_) => panic!("transient blob commit failure should abort the sync attempt"),
    Err(error) => error,
  };

  assert!(
    error.is_transient_upstream(),
    "502 should stay classified as transient upstream, got: {}",
    error,
  );
  assert_eq!(
    mock_state.blob_commit_requests.load(Ordering::SeqCst),
    1,
    "transient commit failure must not fall back to per-file isolation",
  );

  let metadata_store = SyncMetadataStore::new(&state);
  assert!(
    metadata_store
      .get_file_meta(&relationship.id, "/docs/retry-commit.txt")
      .expect("metadata lookup failed")
      .is_none(),
    "transient blob commit failure should not record the file as synced or failed",
  );
}

#[tokio::test]
async fn test_push_splits_large_blob_check_requests() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  let mut content = Vec::new();
  for index in 0_u32..600 {
    content.extend_from_slice(&index.to_le_bytes());
  }
  std::fs::write(local_path.join("many-unique-chunks.bin"), content).expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1);
  assert_eq!(result.files_failed, 0);
  assert!(
    mock_state.blob_check_requests.load(Ordering::SeqCst) >= 2,
    "large hash sets should be split across multiple blob_check calls",
  );
  assert!(
    mock_state.max_blob_check_hashes.load(Ordering::SeqCst) <= 512,
    "blob_check requests should stay below the body-limit guard",
  );
}

#[tokio::test]
async fn test_push_oversized_blob_commit_manifest_uses_direct_file_upload() {
  let (address, mock_state) = start_mock_server().await;
  let (state, _temp_db) = create_state_store();
  let connection = make_connection(&address);

  let local_dir = tempfile::tempdir().expect("failed to create local dir");
  let local_path = local_dir.path();
  let mut content = Vec::new();
  for index in 0_u32..500_000 {
    content.extend_from_slice(&index.to_le_bytes());
  }
  std::fs::write(local_path.join("huge-manifest.bin"), &content).expect("write failed");

  let relationship = make_relationship(&local_path.to_string_lossy());

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 1);
  assert_eq!(result.files_failed, 0);
  assert_eq!(
    mock_state.blob_check_requests.load(Ordering::SeqCst),
    0,
    "oversized commit manifests should bypass blob_check",
  );
  assert_eq!(
    mock_state.blob_commit_requests.load(Ordering::SeqCst),
    0,
    "oversized commit manifests should not call blob_commit and hit the server body limit",
  );
  assert!(
    mock_state.chunks.lock().await.is_empty(),
    "direct file upload should avoid uploading chunks that cannot be committed",
  );
  let files = mock_state.files.lock().await;
  assert_eq!(
    files.get("/docs/huge-manifest.bin"),
    Some(&content),
    "direct file upload should store the file content at the remote path",
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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  // Duration should be at least 0 and recorded.
  assert!(
    result.duration_ms < 30000,
    "push should complete within 30 seconds"
  );
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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(
    result.files_pushed, file_count,
    "all files should be pushed"
  );
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

  let result = run_push_sync(&state, &connection, &relationship)
    .await
    .expect("push_sync failed");

  assert_eq!(result.files_pushed, 2);

  let files = mock_state.files.lock().await;
  assert!(
    files.contains_key("/my-remote-base/root.txt"),
    "root file should use remote base"
  );
  assert!(
    files.contains_key("/my-remote-base/sub/deep.txt"),
    "nested file should preserve hierarchy under remote base"
  );
}
