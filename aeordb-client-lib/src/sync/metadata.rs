use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use aeordb::engine::MergeDepth;

use crate::error::Result;
use crate::state::StateStore;

const SYNC_META_PATH: &str = "/sync/meta/";
const SYNC_MIGRATIONS_PATH: &str = "/sync/migrations/";
const SYNC_FILES_PATH: &str = "/sync/files/";
const SYNC_FILES_INDEX_PATH: &str = "/sync/files-index/";
const SYNC_FILES_V2_PATH: &str = "/sync/files-v2/";
const FILE_META_BUCKET_HEX_CHARS: usize = 3;

/// Per-file sync metadata. Tracks the state of a single file
/// in a sync relationship WITHOUT storing the actual file content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSyncMeta {
  pub path: String,         // remote path (e.g., "/docs/readme.md")
  pub content_hash: String, // blake3 hash of file content
  pub size: u64,
  pub modified_at: i64, // local filesystem mtime (ms since epoch)
  pub sync_status: SyncStatus,
  pub last_synced_at: i64, // when this file was last synced (ms since epoch)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
  Synced,
  PendingPush,
  PendingPull,
  Error,
}

/// Per-relationship sync checkpoint. Tracks the last known
/// remote state so we can ask for incremental diffs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncCheckpoint {
  pub relationship_id: String,
  pub remote_root_hash: String, // hex-encoded root hash from last sync
  pub last_sync_at: i64,        // timestamp of last sync (ms since epoch)
}

/// Pending path migration for a relationship whose sync root changed.
///
/// This marker is intentionally separate from file metadata: migration
/// planning needs the old per-file metadata to calculate safe remote moves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPathMigration {
  pub relationship_id: String,
  pub old_remote_path: String,
  pub new_remote_path: String,
  pub old_local_path: String,
  pub new_local_path: String,
  pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileMetaBucket {
  #[serde(rename = "$v")]
  version: u8,
  #[serde(default)]
  items: HashMap<String, FileSyncMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileMetaV2Manifest {
  #[serde(rename = "$v")]
  version: u8,
  bucket_prefix_hex_chars: usize,
}

/// Manages sync metadata in the local aeordb state store.
pub struct SyncMetadataStore<'a> {
  state: &'a StateStore,
}

impl<'a> SyncMetadataStore<'a> {
  pub fn new(state: &'a StateStore) -> Self {
    Self { state }
  }

  /// Retrieve per-file sync metadata for a given relationship and remote path.
  /// Returns `None` if no metadata exists for that file.
  pub fn get_file_meta(
    &self,
    relationship_id: &str,
    remote_path: &str,
  ) -> Result<Option<FileSyncMeta>> {
    let path_hash = file_meta_path_hash(remote_path);
    if self.has_v2_manifest(relationship_id)? {
      let bucket_key = self.file_meta_bucket_key(relationship_id, &path_hash);
      if let Some(bucket) = self.state.read_json::<FileMetaBucket>(&bucket_key)? {
        if let Some(meta) = bucket.items.get(&path_hash) {
          return Ok(Some(meta.clone()));
        }
      }
      return Ok(None);
    }

    let legacy_index = self.load_legacy_file_meta_index(relationship_id)?;
    if let Some(meta) = legacy_index.get(remote_path) {
      return Ok(Some(meta.clone()));
    }

    let key = self.file_meta_key(relationship_id, remote_path);
    self.state.read_json::<FileSyncMeta>(&key)
  }

  /// Store per-file sync metadata for a given relationship.
  pub fn set_file_meta(&self, relationship_id: &str, meta: &FileSyncMeta) -> Result<()> {
    self.set_file_metas_batch(relationship_id, std::slice::from_ref(meta))
  }

  /// Store multiple per-file metadata entries for a relationship with one
  /// state-database write. This is important for large push batches: writing
  /// thousands of individual tiny JSON files through DirectoryOps is far too
  /// expensive.
  pub fn set_file_metas_batch(&self, relationship_id: &str, metas: &[FileSyncMeta]) -> Result<()> {
    if metas.is_empty() {
      return Ok(());
    }

    self.ensure_v2_storage(relationship_id)?;

    let mut bucket_items: HashMap<String, serde_json::Map<String, serde_json::Value>> =
      HashMap::new();
    for meta in metas {
      let path_hash = file_meta_path_hash(&meta.path);
      let bucket_key = self.file_meta_bucket_key(relationship_id, &path_hash);
      bucket_items
        .entry(bucket_key)
        .or_default()
        .insert(path_hash, serde_json::to_value(meta)?);
    }

    let patches = bucket_items
      .into_iter()
      .map(|(bucket_key, items)| {
        (
          bucket_key,
          serde_json::json!({
            "$v": 1,
            "items": items,
          }),
          MergeDepth::ReplaceBeyond(2),
        )
      })
      .collect();

    self.state.merge_json_files_batch(patches)
  }

  /// Delete per-file sync metadata for a given relationship and remote path.
  pub fn delete_file_meta(&self, relationship_id: &str, remote_path: &str) -> Result<()> {
    if self.has_v2_manifest(relationship_id)? {
      let path_hash = file_meta_path_hash(remote_path);
      self.state.merge_json_file(
        &self.file_meta_bucket_key(relationship_id, &path_hash),
        serde_json::json!({
          "items": {
            path_hash: null,
          },
        }),
        MergeDepth::ReplaceBeyond(2),
      )?;
      return Ok(());
    }

    let mut index = self.load_legacy_file_meta_index(relationship_id)?;
    if index.remove(remote_path).is_some() {
      self.store_legacy_file_meta_index(relationship_id, &index)?;
    }

    let key = self.file_meta_key(relationship_id, remote_path);
    if self.state.exists(&key)? {
      self.state.delete(&key)?;
    }

    Ok(())
  }

  /// List all tracked file metadata entries for a relationship.
  pub fn list_file_metas(&self, relationship_id: &str) -> Result<Vec<FileSyncMeta>> {
    if self.has_v2_manifest(relationship_id)? {
      return self.list_file_metas_v2(relationship_id);
    }

    let index = self.load_legacy_file_meta_index(relationship_id)?;
    if !index.is_empty() {
      return Ok(index.into_values().collect());
    }

    self.list_file_metas_legacy(relationship_id)
  }

  fn ensure_v2_storage(&self, relationship_id: &str) -> Result<()> {
    if self.has_v2_manifest(relationship_id)? {
      return Ok(());
    }

    let index = self.load_legacy_file_meta_index(relationship_id)?;
    let mut files: Vec<(String, serde_json::Value)> = Vec::new();

    if !index.is_empty() {
      let mut buckets: HashMap<String, HashMap<String, FileSyncMeta>> = HashMap::new();
      for meta in index.into_values() {
        let path_hash = file_meta_path_hash(&meta.path);
        let bucket = file_meta_bucket_from_hash(&path_hash);
        buckets.entry(bucket).or_default().insert(path_hash, meta);
      }

      for (bucket, items) in buckets {
        files.push((
          self.file_meta_bucket_key_for_bucket(relationship_id, &bucket),
          serde_json::to_value(FileMetaBucket { version: 1, items })?,
        ));
      }
    }

    files.push((
      self.file_meta_v2_manifest_key(relationship_id),
      serde_json::to_value(FileMetaV2Manifest {
        version: 1,
        bucket_prefix_hex_chars: FILE_META_BUCKET_HEX_CHARS,
      })?,
    ));

    self.state.store_json_values_batch(files)
  }

  fn list_file_metas_v2(&self, relationship_id: &str) -> Result<Vec<FileSyncMeta>> {
    let directory = self.file_meta_v2_relationship_dir(relationship_id);
    if !self.state.exists(&directory)? {
      return Ok(Vec::new());
    }

    let mut results = Vec::new();
    for entry in self.state.list_directory(&directory)? {
      if entry == ".keep" || entry == "manifest.json" || !entry.ends_with(".json") {
        continue;
      }

      let path = format!("{}{}", directory, entry);
      if let Some(bucket) = self.state.read_json::<FileMetaBucket>(&path)? {
        results.extend(bucket.items.into_values());
      }
    }

    Ok(results)
  }

  fn load_legacy_file_meta_index(
    &self,
    relationship_id: &str,
  ) -> Result<HashMap<String, FileSyncMeta>> {
    let key = self.file_meta_index_key(relationship_id);
    if let Some(index) = self
      .state
      .read_json::<HashMap<String, FileSyncMeta>>(&key)?
    {
      return Ok(index);
    }

    let legacy = self.list_file_metas_legacy(relationship_id)?;
    Ok(
      legacy
        .into_iter()
        .map(|meta| (meta.path.clone(), meta))
        .collect(),
    )
  }

  fn store_legacy_file_meta_index(
    &self,
    relationship_id: &str,
    index: &HashMap<String, FileSyncMeta>,
  ) -> Result<()> {
    let key = self.file_meta_index_key(relationship_id);
    self.state.store_json(&key, index)
  }

  fn has_v2_manifest(&self, relationship_id: &str) -> Result<bool> {
    self
      .state
      .exists(&self.file_meta_v2_manifest_key(relationship_id))
  }

  fn file_meta_v2_relationship_dir(&self, relationship_id: &str) -> String {
    format!("{}{}/", SYNC_FILES_V2_PATH, relationship_id)
  }

  fn file_meta_v2_manifest_key(&self, relationship_id: &str) -> String {
    format!(
      "{}manifest.json",
      self.file_meta_v2_relationship_dir(relationship_id)
    )
  }

  fn file_meta_bucket_key(&self, relationship_id: &str, path_hash: &str) -> String {
    self.file_meta_bucket_key_for_bucket(relationship_id, &file_meta_bucket_from_hash(path_hash))
  }

  fn file_meta_bucket_key_for_bucket(&self, relationship_id: &str, bucket: &str) -> String {
    format!(
      "{}{}.json",
      self.file_meta_v2_relationship_dir(relationship_id),
      bucket,
    )
  }

  fn list_file_metas_legacy(&self, relationship_id: &str) -> Result<Vec<FileSyncMeta>> {
    let directory = format!("{}{}/", SYNC_FILES_PATH, relationship_id);

    if !self.state.exists(&directory)? {
      return Ok(Vec::new());
    }

    let entries = self.state.list_directory(&directory)?;
    let mut results = Vec::new();

    for entry in entries {
      if entry == ".keep" {
        continue;
      }

      let path = format!("{}{}", directory, entry);

      if let Some(meta) = self.state.read_json::<FileSyncMeta>(&path)? {
        results.push(meta);
      }
    }

    Ok(results)
  }

  /// Retrieve the sync checkpoint for a relationship.
  /// Returns `None` if no checkpoint exists yet.
  pub fn get_checkpoint(&self, relationship_id: &str) -> Result<Option<SyncCheckpoint>> {
    let key = format!("{}{}.json", SYNC_META_PATH, relationship_id);
    self.state.read_json::<SyncCheckpoint>(&key)
  }

  /// Store or update the sync checkpoint for a relationship.
  pub fn set_checkpoint(&self, checkpoint: &SyncCheckpoint) -> Result<()> {
    let key = format!("{}{}.json", SYNC_META_PATH, checkpoint.relationship_id);
    self.state.store_json(&key, checkpoint)
  }

  /// Clear only the remote diff checkpoint for a relationship. Per-file
  /// metadata is intentionally preserved so a forced scan can still compare
  /// hashes and avoid treating unchanged local files as new work.
  pub fn clear_checkpoint(&self, relationship_id: &str) -> Result<()> {
    let checkpoint_key = format!("{}{}.json", SYNC_META_PATH, relationship_id);
    if self.state.exists(&checkpoint_key)? {
      self.state.delete(&checkpoint_key)?;
    }

    Ok(())
  }

  /// Record a pending base-path migration and clear the pull checkpoint.
  /// Per-file metadata is preserved so push can map old remote paths to
  /// deterministic new paths before the next full scan proceeds.
  pub fn begin_path_migration(
    &self,
    relationship_id: &str,
    old_remote_path: &str,
    new_remote_path: &str,
    old_local_path: &str,
    new_local_path: &str,
  ) -> Result<()> {
    self.clear_checkpoint(relationship_id)?;

    let migration = SyncPathMigration {
      relationship_id: relationship_id.to_string(),
      old_remote_path: old_remote_path.to_string(),
      new_remote_path: new_remote_path.to_string(),
      old_local_path: old_local_path.to_string(),
      new_local_path: new_local_path.to_string(),
      created_at: chrono::Utc::now().timestamp_millis(),
    };

    self
      .state
      .store_json(&self.path_migration_key(relationship_id), &migration)
  }

  /// Retrieve a pending path migration marker for a relationship.
  pub fn get_path_migration(&self, relationship_id: &str) -> Result<Option<SyncPathMigration>> {
    self
      .state
      .read_json::<SyncPathMigration>(&self.path_migration_key(relationship_id))
  }

  /// Clear a completed path migration marker without touching file metadata.
  pub fn clear_path_migration(&self, relationship_id: &str) -> Result<()> {
    let key = self.path_migration_key(relationship_id);
    if self.state.exists(&key)? {
      self.state.delete(&key)?;
    }

    Ok(())
  }

  /// Clear all sync state for a relationship so the next pull starts from
  /// scratch: deletes the per-file metadata directory AND the checkpoint.
  /// This is intentionally stronger than Force Sync, which only clears the
  /// checkpoint and preserves per-file metadata for content-hash comparisons.
  pub fn clear_relationship_state(&self, relationship_id: &str) -> Result<()> {
    self.clear_checkpoint(relationship_id)?;
    self.clear_path_migration(relationship_id)?;

    let files_dir = format!("{}{}/", SYNC_FILES_PATH, relationship_id);
    if self.state.exists(&files_dir)? {
      let entries = self.state.list_directory(&files_dir)?;
      for entry in entries {
        if entry == ".keep" {
          continue;
        }
        let path = format!("{}{}", files_dir, entry);
        self.state.delete(&path)?;
      }
    }

    let index_key = self.file_meta_index_key(relationship_id);
    if self.state.exists(&index_key)? {
      self.state.delete(&index_key)?;
    }

    let files_v2_dir = self.file_meta_v2_relationship_dir(relationship_id);
    if self.state.exists(&files_v2_dir)? {
      let entries = self.state.list_directory(&files_v2_dir)?;
      for entry in entries {
        if entry == ".keep" {
          continue;
        }
        let path = format!("{}{}", files_v2_dir, entry);
        self.state.delete(&path)?;
      }
    }

    Ok(())
  }

  fn file_meta_index_key(&self, relationship_id: &str) -> String {
    format!("{}{}.json", SYNC_FILES_INDEX_PATH, relationship_id)
  }

  fn path_migration_key(&self, relationship_id: &str) -> String {
    format!("{}{}.json", SYNC_MIGRATIONS_PATH, relationship_id)
  }

  /// Compute the storage key for a file's sync metadata.
  /// Uses a blake3 hash of the remote path to avoid filesystem-unfriendly
  /// characters in the key.
  fn file_meta_key(&self, relationship_id: &str, remote_path: &str) -> String {
    let path_hash = blake3::hash(remote_path.as_bytes());
    format!(
      "{}{}/{}.json",
      SYNC_FILES_PATH,
      relationship_id,
      path_hash.to_hex(),
    )
  }
}

fn file_meta_path_hash(remote_path: &str) -> String {
  blake3::hash(remote_path.as_bytes()).to_hex().to_string()
}

fn file_meta_bucket_from_hash(path_hash: &str) -> String {
  path_hash.chars().take(FILE_META_BUCKET_HEX_CHARS).collect()
}
