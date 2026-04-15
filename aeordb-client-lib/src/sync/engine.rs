use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::remote::RemoteClient;
use crate::state::StateStore;
use crate::sync::relationships::RelationshipManager;

const SYNC_STATE_PATH: &str = "/sync/state/";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
  pub relative_path:     String,
  pub relationship_id:   String,
  pub content_hash:      String,
  pub local_modified_at: Option<i64>,
  pub remote_modified_at: Option<i64>,
  pub sync_status:       SyncStatus,
  pub last_synced_at:    i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
  Synced,
  PendingPull,
  PendingPush,
  Conflict,
  Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPassResult {
  pub relationship_id:  String,
  pub files_downloaded: u64,
  pub files_skipped:    u64,
  pub files_failed:     u64,
  pub total_bytes:      u64,
  pub duration_ms:      u64,
  pub errors:           Vec<String>,
}

/// Execute a one-shot pull sync pass for a given relationship.
pub async fn pull_sync_pass(
  state: &StateStore,
  relationship_id: &str,
) -> Result<SyncPassResult> {
  let start = std::time::Instant::now();

  // Load the relationship
  let relationship_manager = RelationshipManager::new(state);
  let relationship = relationship_manager.get(relationship_id)?
    .ok_or_else(|| ClientError::Configuration(
      format!("sync relationship not found: {}", relationship_id),
    ))?;

  if !relationship.enabled {
    return Err(ClientError::Configuration(
      format!("sync relationship '{}' is disabled", relationship.name),
    ));
  }

  // Load the connection
  let connection_manager = ConnectionManager::new(state);
  let connection = connection_manager.get(&relationship.remote_connection_id)?
    .ok_or_else(|| ClientError::Configuration(
      format!("connection not found: {}", relationship.remote_connection_id),
    ))?;

  let remote_client = RemoteClient::from_connection(&connection);

  let mut result = SyncPassResult {
    relationship_id:  relationship_id.to_string(),
    files_downloaded: 0,
    files_skipped:    0,
    files_failed:     0,
    total_bytes:      0,
    duration_ms:      0,
    errors:           Vec::new(),
  };

  let filter = relationship.filter.as_deref();

  // Compute hierarchy exclusions — paths owned by child relationships
  let all_relationships = relationship_manager.list()?;
  let exclusions = crate::sync::hierarchy::child_exclusions(&relationship, &all_relationships);

  // Recursively walk the remote directory and sync files
  let remote_entries = match remote_client.list_directory(&relationship.remote_path).await {
    Ok(entries) => entries,
    Err(error) => {
      result.errors.push(format!("failed to list remote directory: {}", error));
      result.duration_ms = start.elapsed().as_millis() as u64;
      return Ok(result);
    }
  };

  sync_directory_recursive(
    state,
    &remote_client,
    &relationship.remote_path,
    &relationship.local_path,
    relationship_id,
    filter,
    &exclusions,
    &remote_entries,
    &mut result,
  ).await;

  result.duration_ms = start.elapsed().as_millis() as u64;

  tracing::info!(
    "sync pass for '{}': {} downloaded, {} skipped, {} failed ({} bytes, {}ms)",
    relationship.name,
    result.files_downloaded,
    result.files_skipped,
    result.files_failed,
    result.total_bytes,
    result.duration_ms,
  );

  Ok(result)
}

async fn sync_directory_recursive(
  state: &StateStore,
  remote_client: &RemoteClient,
  remote_dir_path: &str,
  local_dir_path: &str,
  relationship_id: &str,
  filter: Option<&str>,
  exclusions: &[String],
  entries: &[crate::remote::RemoteEntry],
  result: &mut SyncPassResult,
) {
  for entry in entries {
    let remote_file_path = format!("{}{}", remote_dir_path, entry.name);
    let local_file_path  = format!("{}/{}", local_dir_path, entry.name);

    if entry.is_directory() {
      // Recurse into subdirectory
      let sub_remote_path = format!("{}/", remote_file_path);

      // Check hierarchy exclusions — skip if owned by a child relationship
      if crate::sync::hierarchy::is_excluded_by_child(&sub_remote_path, exclusions) {
        tracing::debug!("skipping {} — owned by child relationship", sub_remote_path);
        continue;
      }

      // Ensure local subdirectory exists
      if let Err(error) = std::fs::create_dir_all(&local_file_path) {
        result.errors.push(format!("failed to create dir {}: {}", local_file_path, error));
        result.files_failed += 1;
        continue;
      }

      match remote_client.list_directory(&sub_remote_path).await {
        Ok(sub_entries) => {
          Box::pin(sync_directory_recursive(
            state, remote_client, &sub_remote_path,
            &local_file_path, relationship_id, filter, exclusions, &sub_entries, result,
          )).await;
        }
        Err(error) => {
          result.errors.push(format!("failed to list {}: {}", sub_remote_path, error));
          result.files_failed += 1;
        }
      }
    } else if entry.is_symlink() {
      // Symlink — create a local symlink pointing to the target
      if let Some(ref target) = entry.target {
        let symlink_path = std::path::Path::new(&local_file_path);

        // Remove existing file/symlink at this path if it exists
        if symlink_path.exists() || symlink_path.is_symlink() {
          let _ = std::fs::remove_file(symlink_path);
        }

        #[cfg(unix)]
        {
          if let Err(error) = std::os::unix::fs::symlink(target, symlink_path) {
            result.errors.push(format!("failed to create symlink {}: {}", local_file_path, error));
            result.files_failed += 1;
          } else {
            result.files_downloaded += 1;
            tracing::debug!("synced symlink: {} → {}", remote_file_path, target);
          }
        }

        #[cfg(not(unix))]
        {
          result.errors.push(format!("symlinks not supported on this platform: {}", remote_file_path));
          result.files_failed += 1;
        }
      }
      continue;
    } else if entry.is_file() {
      // Apply filter
      if !crate::sync::filter::matches_filter(&entry.name, filter) {
        result.files_skipped += 1;
        continue;
      }

      // File — check if we need to download it
      let relative_path = entry.name.clone();
      let state_key     = file_state_key(relationship_id, &remote_file_path);

      // Check existing state
      let existing_state: Option<FileState> = state.read_json(&state_key)
        .unwrap_or(None);

      // Compare: if we have synced state with matching updated_at, skip
      if let Some(ref file_state) = existing_state {
        if file_state.sync_status == SyncStatus::Synced {
          if let Some(remote_updated) = file_state.remote_modified_at {
            if remote_updated == entry.updated_at {
              result.files_skipped += 1;
              continue;
            }
          }
        }
      }

      // Download the file
      match remote_client.download_file(&remote_file_path).await {
        Ok((bytes, _metadata)) => {
          // Write to local filesystem
          if let Some(parent) = Path::new(&local_file_path).parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
              result.errors.push(format!("failed to create parent dir for {}: {}", local_file_path, error));
              result.files_failed += 1;
              continue;
            }
          }

          match std::fs::write(&local_file_path, &bytes) {
            Ok(()) => {
              // Compute content hash
              let hash = blake3::hash(&bytes);

              // Update state tracker
              let file_state = FileState {
                relative_path,
                relationship_id: relationship_id.to_string(),
                content_hash:       hash.to_hex().to_string(),
                local_modified_at:  None,
                remote_modified_at: Some(entry.updated_at),
                sync_status:        SyncStatus::Synced,
                last_synced_at:     Utc::now().timestamp_millis(),
              };

              if let Err(error) = state.store_json(&state_key, &file_state) {
                tracing::warn!("failed to update sync state for {}: {}", remote_file_path, error);
              }

              result.total_bytes += bytes.len() as u64;
              result.files_downloaded += 1;

              tracing::debug!("synced: {} ({} bytes)", remote_file_path, bytes.len());
            }
            Err(error) => {
              result.errors.push(format!("failed to write {}: {}", local_file_path, error));
              result.files_failed += 1;
            }
          }
        }
        Err(error) => {
          result.errors.push(format!("failed to download {}: {}", remote_file_path, error));
          result.files_failed += 1;
        }
      }
    }
  }
}

/// Generate the state store key for a file's sync state.
pub fn file_state_key(relationship_id: &str, remote_path: &str) -> String {
  // Use a hash of the remote path to avoid filesystem-unfriendly characters
  let path_hash = blake3::hash(remote_path.as_bytes());
  let short_hash = &path_hash.to_hex()[..16];
  format!("{}{}/{}.json", SYNC_STATE_PATH, relationship_id, short_hash)
}
