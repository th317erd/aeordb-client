use serde::Deserialize;

use crate::connections::RemoteConnection;
use crate::error::Result;
use crate::state::StateStore;
use crate::sync::pull::{pull_sync, PullResult};
use crate::sync::push::{push_sync, PushResult};
use crate::sync::relationships::{SyncDirection, SyncRelationship};

/// Combined result of a bidirectional sync operation.
pub struct SyncResult {
  pub push: Option<PushResult>,
  pub pull: Option<PullResult>,
}

/// Orchestrate a sync cycle for a relationship, calling push and/or pull
/// based on the configured direction.
pub async fn sync_relationship(
  state: &StateStore,
  connection: &RemoteConnection,
  relationship: &SyncRelationship,
  http_client: &reqwest::Client,
) -> Result<SyncResult> {
  let direction = &relationship.direction;
  let mut result = SyncResult { push: None, pull: None };

  // Pull first (if direction allows) so we have the latest remote state
  // before pushing local changes.
  if *direction == SyncDirection::PullOnly || *direction == SyncDirection::Bidirectional {
    result.pull = Some(pull_sync(state, connection, relationship, http_client).await?);
  }

  // Push if direction allows.
  if *direction == SyncDirection::PushOnly || *direction == SyncDirection::Bidirectional {
    result.push = Some(push_sync(state, connection, relationship, http_client).await?);
  }

  Ok(result)
}

// ---- Types used by pull.rs ----
// pull.rs imports these from this module. They represent the remote
// server's sync/diff response format.

/// Response from POST /sync/diff on the remote.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncDiffResponse {
  pub root_hash: String,
  pub changes:   RemoteSyncChanges,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncChanges {
  pub files_added:       Vec<RemoteSyncFileEntry>,
  pub files_modified:    Vec<RemoteSyncFileEntry>,
  pub files_deleted:     Vec<RemoteSyncDeletedEntry>,
  pub symlinks_added:    Vec<RemoteSyncSymlinkEntry>,
  pub symlinks_modified: Vec<RemoteSyncSymlinkEntry>,
  pub symlinks_deleted:  Vec<RemoteSyncDeletedEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncFileEntry {
  pub path:         String,
  pub hash:         String,
  pub size:         u64,
  pub content_type: Option<String>,
  pub chunk_hashes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncSymlinkEntry {
  pub path:   String,
  pub hash:   String,
  pub target: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncDeletedEntry {
  pub path: String,
}
