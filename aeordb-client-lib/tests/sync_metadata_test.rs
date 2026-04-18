use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::metadata::{
  FileSyncMeta, SyncCheckpoint, SyncMetadataStore, SyncStatus,
};

fn temp_database_path() -> String {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  temp_dir
    .keep()
    .join("test-sync-meta.aeordb")
    .to_string_lossy()
    .to_string()
}

fn make_file_meta(path: &str, hash: &str, status: SyncStatus) -> FileSyncMeta {
  FileSyncMeta {
    path:           path.to_string(),
    content_hash:   hash.to_string(),
    size:           1024,
    modified_at:    1700000000000,
    sync_status:    status,
    last_synced_at: 1700000001000,
  }
}

// --- Happy path tests ---

#[test]
fn test_set_and_get_file_meta() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let meta = make_file_meta("/docs/readme.md", "abc123", SyncStatus::Synced);
  metadata_store
    .set_file_meta("rel-001", &meta)
    .expect("failed to set file meta");

  let retrieved = metadata_store
    .get_file_meta("rel-001", "/docs/readme.md")
    .expect("failed to get file meta");

  assert!(retrieved.is_some());
  let retrieved = retrieved.unwrap();
  assert_eq!(retrieved.path, "/docs/readme.md");
  assert_eq!(retrieved.content_hash, "abc123");
  assert_eq!(retrieved.size, 1024);
  assert_eq!(retrieved.modified_at, 1700000000000);
  assert_eq!(retrieved.sync_status, SyncStatus::Synced);
  assert_eq!(retrieved.last_synced_at, 1700000001000);
}

#[test]
fn test_get_nonexistent_file_meta_returns_none() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let result = metadata_store
    .get_file_meta("rel-001", "/does/not/exist.txt")
    .expect("failed to get file meta");

  assert!(result.is_none());
}

#[test]
fn test_delete_file_meta() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let meta = make_file_meta("/docs/readme.md", "abc123", SyncStatus::Synced);
  metadata_store
    .set_file_meta("rel-001", &meta)
    .expect("failed to set file meta");

  // Verify it exists
  assert!(
    metadata_store
      .get_file_meta("rel-001", "/docs/readme.md")
      .expect("failed to get")
      .is_some()
  );

  // Delete it
  metadata_store
    .delete_file_meta("rel-001", "/docs/readme.md")
    .expect("failed to delete file meta");

  // Verify it's gone
  let result = metadata_store
    .get_file_meta("rel-001", "/docs/readme.md")
    .expect("failed to get after delete");

  assert!(result.is_none());
}

#[test]
fn test_list_file_metas() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let meta_a = make_file_meta("/docs/readme.md", "hash_a", SyncStatus::Synced);
  let meta_b = make_file_meta("/docs/notes.txt", "hash_b", SyncStatus::PendingPush);
  let meta_c = make_file_meta("/src/main.rs", "hash_c", SyncStatus::PendingPull);

  metadata_store.set_file_meta("rel-001", &meta_a).expect("failed to set a");
  metadata_store.set_file_meta("rel-001", &meta_b).expect("failed to set b");
  metadata_store.set_file_meta("rel-001", &meta_c).expect("failed to set c");

  let mut metas = metadata_store
    .list_file_metas("rel-001")
    .expect("failed to list file metas");

  assert_eq!(metas.len(), 3);

  // Sort by path for deterministic assertion
  metas.sort_by(|a, b| a.path.cmp(&b.path));
  assert_eq!(metas[0].path, "/docs/notes.txt");
  assert_eq!(metas[1].path, "/docs/readme.md");
  assert_eq!(metas[2].path, "/src/main.rs");
}

#[test]
fn test_set_and_get_checkpoint() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let checkpoint = SyncCheckpoint {
    relationship_id:  "rel-001".to_string(),
    remote_root_hash: "deadbeef".to_string(),
    last_sync_at:     1700000005000,
  };

  metadata_store
    .set_checkpoint(&checkpoint)
    .expect("failed to set checkpoint");

  let retrieved = metadata_store
    .get_checkpoint("rel-001")
    .expect("failed to get checkpoint");

  assert!(retrieved.is_some());
  let retrieved = retrieved.unwrap();
  assert_eq!(retrieved.relationship_id, "rel-001");
  assert_eq!(retrieved.remote_root_hash, "deadbeef");
  assert_eq!(retrieved.last_sync_at, 1700000005000);
}

#[test]
fn test_checkpoint_nonexistent_returns_none() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let result = metadata_store
    .get_checkpoint("nonexistent-relationship")
    .expect("failed to get checkpoint");

  assert!(result.is_none());
}

#[test]
fn test_update_existing_file_meta() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let original = make_file_meta("/docs/readme.md", "hash_v1", SyncStatus::Synced);
  metadata_store
    .set_file_meta("rel-001", &original)
    .expect("failed to set original");

  // Update with new hash and status
  let updated = FileSyncMeta {
    path:           "/docs/readme.md".to_string(),
    content_hash:   "hash_v2".to_string(),
    size:           2048,
    modified_at:    1700000010000,
    sync_status:    SyncStatus::PendingPush,
    last_synced_at: 1700000011000,
  };

  metadata_store
    .set_file_meta("rel-001", &updated)
    .expect("failed to set updated");

  let retrieved = metadata_store
    .get_file_meta("rel-001", "/docs/readme.md")
    .expect("failed to get after update")
    .expect("should exist after update");

  assert_eq!(retrieved.content_hash, "hash_v2");
  assert_eq!(retrieved.size, 2048);
  assert_eq!(retrieved.modified_at, 1700000010000);
  assert_eq!(retrieved.sync_status, SyncStatus::PendingPush);
  assert_eq!(retrieved.last_synced_at, 1700000011000);
}

// --- Edge cases and failure paths ---

#[test]
fn test_delete_nonexistent_file_meta_is_noop() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  // Deleting something that doesn't exist should not error
  metadata_store
    .delete_file_meta("rel-001", "/does/not/exist.txt")
    .expect("delete of nonexistent file meta should succeed");
}

#[test]
fn test_list_file_metas_empty_relationship() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let metas = metadata_store
    .list_file_metas("rel-empty")
    .expect("failed to list file metas for empty relationship");

  assert!(metas.is_empty());
}

#[test]
fn test_list_file_metas_after_delete() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let meta_a = make_file_meta("/docs/a.txt", "hash_a", SyncStatus::Synced);
  let meta_b = make_file_meta("/docs/b.txt", "hash_b", SyncStatus::Synced);

  metadata_store.set_file_meta("rel-001", &meta_a).expect("failed to set a");
  metadata_store.set_file_meta("rel-001", &meta_b).expect("failed to set b");

  // Delete one
  metadata_store
    .delete_file_meta("rel-001", "/docs/a.txt")
    .expect("failed to delete a");

  let metas = metadata_store
    .list_file_metas("rel-001")
    .expect("failed to list after delete");

  assert_eq!(metas.len(), 1);
  assert_eq!(metas[0].path, "/docs/b.txt");
}

#[test]
fn test_file_metas_isolated_between_relationships() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let meta_rel1 = make_file_meta("/shared/file.txt", "hash_r1", SyncStatus::Synced);
  let meta_rel2 = make_file_meta("/shared/file.txt", "hash_r2", SyncStatus::PendingPush);

  metadata_store.set_file_meta("rel-001", &meta_rel1).expect("failed to set rel-001");
  metadata_store.set_file_meta("rel-002", &meta_rel2).expect("failed to set rel-002");

  // Each relationship should have its own copy
  let retrieved_1 = metadata_store
    .get_file_meta("rel-001", "/shared/file.txt")
    .expect("failed to get rel-001")
    .expect("should exist in rel-001");

  let retrieved_2 = metadata_store
    .get_file_meta("rel-002", "/shared/file.txt")
    .expect("failed to get rel-002")
    .expect("should exist in rel-002");

  assert_eq!(retrieved_1.content_hash, "hash_r1");
  assert_eq!(retrieved_2.content_hash, "hash_r2");

  // Listing should show 1 entry per relationship
  let metas_1 = metadata_store.list_file_metas("rel-001").expect("list rel-001");
  let metas_2 = metadata_store.list_file_metas("rel-002").expect("list rel-002");
  assert_eq!(metas_1.len(), 1);
  assert_eq!(metas_2.len(), 1);
}

#[test]
fn test_checkpoint_update_overwrites() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let checkpoint_v1 = SyncCheckpoint {
    relationship_id:  "rel-001".to_string(),
    remote_root_hash: "hash_v1".to_string(),
    last_sync_at:     1700000000000,
  };

  metadata_store.set_checkpoint(&checkpoint_v1).expect("failed to set v1");

  let checkpoint_v2 = SyncCheckpoint {
    relationship_id:  "rel-001".to_string(),
    remote_root_hash: "hash_v2".to_string(),
    last_sync_at:     1700000010000,
  };

  metadata_store.set_checkpoint(&checkpoint_v2).expect("failed to set v2");

  let retrieved = metadata_store
    .get_checkpoint("rel-001")
    .expect("failed to get checkpoint")
    .expect("should exist");

  assert_eq!(retrieved.remote_root_hash, "hash_v2");
  assert_eq!(retrieved.last_sync_at, 1700000010000);
}

#[test]
fn test_all_sync_status_variants_roundtrip() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  let statuses = [
    SyncStatus::Synced,
    SyncStatus::PendingPush,
    SyncStatus::PendingPull,
    SyncStatus::Error,
  ];

  for (index, status) in statuses.iter().enumerate() {
    let remote_path = format!("/file_{}.txt", index);
    let meta = make_file_meta(&remote_path, "hash", status.clone());
    metadata_store.set_file_meta("rel-status", &meta).expect("failed to set");

    let retrieved = metadata_store
      .get_file_meta("rel-status", &remote_path)
      .expect("failed to get")
      .expect("should exist");

    assert_eq!(retrieved.sync_status, *status);
  }
}

#[test]
fn test_paths_with_special_characters() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create state store");
  let metadata_store = SyncMetadataStore::new(&store);

  // Paths with spaces, unicode, and special characters should be handled
  // because we blake3-hash the path to produce the storage key.
  let special_paths = [
    "/docs/my file with spaces.txt",
    "/docs/unicode-\u{00e9}\u{00e8}\u{00ea}.md",
    "/path/with/many/nested/segments/file.txt",
    "/root.txt",
  ];

  for (index, remote_path) in special_paths.iter().enumerate() {
    let hash = format!("hash_{}", index);
    let meta = make_file_meta(remote_path, &hash, SyncStatus::Synced);
    metadata_store.set_file_meta("rel-special", &meta).expect("failed to set");

    let retrieved = metadata_store
      .get_file_meta("rel-special", remote_path)
      .expect("failed to get")
      .expect("should exist");

    assert_eq!(retrieved.path, *remote_path);
    assert_eq!(retrieved.content_hash, hash);
  }

  let metas = metadata_store
    .list_file_metas("rel-special")
    .expect("failed to list");

  assert_eq!(metas.len(), special_paths.len());
}
