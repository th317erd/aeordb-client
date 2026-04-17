use aeordb::engine::{
  StorageEngine, DirectoryOps, RequestContext,
  compute_sync_diff, get_needed_chunks, apply_sync_chunks,
  list_conflicts_typed, file_history,
};

fn create_test_engine() -> (StorageEngine, std::path::PathBuf) {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir").keep();
  let db_path  = temp_dir.join("test.aeordb");
  let db_str   = db_path.to_string_lossy().to_string();

  let engine = StorageEngine::create(&db_str).expect("failed to create engine");
  let ops    = DirectoryOps::new(&engine);
  let ctx    = RequestContext::system();
  ops.ensure_root_directory(&ctx).expect("failed to init root");

  (engine, temp_dir)
}

#[test]
fn test_compute_sync_diff_on_empty_db() {
  let (engine, _temp) = create_test_engine();

  let diff = compute_sync_diff(&engine, None, None, false)
    .expect("compute_sync_diff failed");

  // Empty database — no files, no changes
  assert!(diff.files_added.is_empty());
  assert!(diff.files_modified.is_empty());
  assert!(diff.files_deleted.is_empty());
}

#[test]
fn test_compute_sync_diff_detects_new_file() {
  let (engine, _temp) = create_test_engine();
  let ops = DirectoryOps::new(&engine);
  let ctx = RequestContext::system();

  // Store a file
  ops.store_file(&ctx, "/docs/hello.txt", b"hello world", Some("text/plain"))
    .expect("store failed");

  // Diff from scratch — should see the file as added
  let diff = compute_sync_diff(&engine, None, None, false)
    .expect("diff failed");

  assert!(!diff.files_added.is_empty(), "should detect the new file");

  // Check that we can find our file in the added list
  let found = diff.files_added.iter().any(|f| f.path == "/docs/hello.txt");
  assert!(found, "hello.txt should be in files_added");
}

#[test]
fn test_list_conflicts_typed_empty() {
  let (engine, _temp) = create_test_engine();

  let conflicts = list_conflicts_typed(&engine)
    .expect("list_conflicts_typed failed");

  assert!(conflicts.is_empty(), "new database should have no conflicts");
}

#[test]
fn test_file_history_empty() {
  let (engine, _temp) = create_test_engine();

  let history = file_history(&engine, "/nonexistent.txt")
    .expect("file_history failed");

  assert!(history.is_empty(), "nonexistent file should have no history");
}

#[test]
fn test_get_needed_chunks_empty() {
  let (engine, _temp) = create_test_engine();

  // Request chunks that don't exist — should return empty
  let fake_hash = vec![0u8; 32];
  let chunks = get_needed_chunks(&engine, &[fake_hash])
    .expect("get_needed_chunks failed");

  assert!(chunks.is_empty(), "nonexistent chunks should return empty");
}

#[test]
fn test_apply_sync_chunks_empty() {
  let (engine, _temp) = create_test_engine();

  let count = apply_sync_chunks(&engine, &[])
    .expect("apply_sync_chunks failed");

  assert_eq!(count, 0, "no chunks applied");
}
