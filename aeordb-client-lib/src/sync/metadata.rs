use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::state::StateStore;

const SYNC_META_PATH: &str = "/sync/meta/";
const SYNC_FILES_PATH: &str = "/sync/files/";

/// Per-file sync metadata. Tracks the state of a single file
/// in a sync relationship WITHOUT storing the actual file content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSyncMeta {
  pub path:           String,     // remote path (e.g., "/docs/readme.md")
  pub content_hash:   String,     // blake3 hash of file content
  pub size:           u64,
  pub modified_at:    i64,        // local filesystem mtime (ms since epoch)
  pub sync_status:    SyncStatus,
  pub last_synced_at: i64,        // when this file was last synced (ms since epoch)
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
  pub relationship_id:  String,
  pub remote_root_hash: String, // hex-encoded root hash from last sync
  pub last_sync_at:     i64,    // timestamp of last sync (ms since epoch)
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
    let key = self.file_meta_key(relationship_id, remote_path);
    self.state.read_json::<FileSyncMeta>(&key)
  }

  /// Store per-file sync metadata for a given relationship.
  pub fn set_file_meta(
    &self,
    relationship_id: &str,
    meta: &FileSyncMeta,
  ) -> Result<()> {
    let key = self.file_meta_key(relationship_id, &meta.path);
    self.state.store_json(&key, meta)
  }

  /// Delete per-file sync metadata for a given relationship and remote path.
  pub fn delete_file_meta(
    &self,
    relationship_id: &str,
    remote_path: &str,
  ) -> Result<()> {
    let key = self.file_meta_key(relationship_id, remote_path);

    if !self.state.exists(&key)? {
      return Ok(());
    }

    self.state.delete(&key)
  }

  /// List all tracked file metadata entries for a relationship.
  pub fn list_file_metas(
    &self,
    relationship_id: &str,
  ) -> Result<Vec<FileSyncMeta>> {
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
  pub fn get_checkpoint(
    &self,
    relationship_id: &str,
  ) -> Result<Option<SyncCheckpoint>> {
    let key = format!("{}{}.json", SYNC_META_PATH, relationship_id);
    self.state.read_json::<SyncCheckpoint>(&key)
  }

  /// Store or update the sync checkpoint for a relationship.
  pub fn set_checkpoint(&self, checkpoint: &SyncCheckpoint) -> Result<()> {
    let key = format!("{}{}.json", SYNC_META_PATH, checkpoint.relationship_id);
    self.state.store_json(&key, checkpoint)
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
