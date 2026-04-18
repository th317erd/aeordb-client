use std::path::Path;
use std::time::Instant;

use crate::connections::{AuthType, RemoteConnection};
use crate::error::{ClientError, Result};
use crate::remote::RemoteClient;
use crate::state::StateStore;
use crate::sync::filter::matches_filter;
use crate::sync::metadata::{
  FileSyncMeta, SyncCheckpoint, SyncMetadataStore, SyncStatus,
};
use crate::sync::relationships::SyncRelationship;
use crate::sync::replication::{
  RemoteSyncDiffResponse, RemoteSyncFileEntry, RemoteSyncSymlinkEntry,
};

/// Result of a pull sync operation.
#[derive(Debug)]
pub struct PullResult {
  pub files_pulled:   u64,
  pub files_skipped:  u64,
  pub files_failed:   u64,
  pub files_deleted:  u64,
  pub symlinks_pulled: u64,
  pub total_bytes:    u64,
  pub duration_ms:    u64,
  pub errors:         Vec<String>,
}

/// Pull remote changes from an aeordb server to the local filesystem.
///
/// Asks the remote for changes since the last known root hash, downloads
/// changed files directly to disk, and stores only metadata in the local
/// aeordb state store. No file content is stored locally in aeordb.
pub async fn pull_sync(
  state: &StateStore,
  connection: &RemoteConnection,
  relationship: &SyncRelationship,
) -> Result<PullResult> {
  let start = Instant::now();

  let remote_client = RemoteClient::from_connection(connection);
  let metadata_store = SyncMetadataStore::new(state);

  let local_base = Path::new(&relationship.local_path);
  if !local_base.exists() {
    std::fs::create_dir_all(local_base)?;
  }

  let mut files_pulled: u64 = 0;
  let mut files_skipped: u64 = 0;
  let mut files_failed: u64 = 0;
  let mut files_deleted: u64 = 0;
  let mut symlinks_pulled: u64 = 0;
  let mut total_bytes: u64 = 0;
  let mut errors: Vec<String> = Vec::new();

  // Load the last sync checkpoint to get incremental diffs.
  let checkpoint = metadata_store.get_checkpoint(&relationship.id)?;
  let since_root_hash = checkpoint.as_ref().map(|c| c.remote_root_hash.clone());

  // Fetch the diff from the remote.
  let diff = fetch_remote_diff(connection, since_root_hash.as_deref()).await?;
  let new_root_hash = diff.root_hash.clone();

  // Process added and modified files.
  let files_to_download: Vec<&RemoteSyncFileEntry> = diff.changes.files_added.iter()
    .chain(diff.changes.files_modified.iter())
    .collect();

  for file_entry in files_to_download {
    // Apply glob filter on the filename portion.
    let filename = Path::new(&file_entry.path)
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or("");

    if !matches_filter(filename, relationship.filter.as_deref()) {
      files_skipped += 1;
      continue;
    }

    // Compute the local file path from the remote path.
    let local_file_path = compute_local_path(
      &file_entry.path,
      &relationship.remote_path,
      local_base,
    );

    // Create parent directories if needed.
    if let Some(parent) = local_file_path.parent() {
      if let Err(error) = std::fs::create_dir_all(parent) {
        let message = format!(
          "failed to create parent directory for {:?}: {}",
          local_file_path, error,
        );
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    }

    // Download the file content from the remote.
    match remote_client.download_file(&file_entry.path).await {
      Ok((content, _metadata)) => {
        let file_size = content.len() as u64;

        // Write to the local filesystem.
        if let Err(error) = std::fs::write(&local_file_path, &content) {
          let message = format!("failed to write {:?}: {}", local_file_path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          files_failed += 1;
          continue;
        }

        // Compute BLAKE3 hash of the downloaded content.
        let content_hash = blake3::hash(&content).to_hex().to_string();
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Get mtime from the written file.
        let mtime = file_mtime(&local_file_path).unwrap_or(now_ms);

        // Store metadata (no file content in local aeordb).
        let file_meta = FileSyncMeta {
          path:           file_entry.path.clone(),
          content_hash,
          size:           file_size,
          modified_at:    mtime,
          sync_status:    SyncStatus::Synced,
          last_synced_at: now_ms,
        };

        if let Err(error) = metadata_store.set_file_meta(&relationship.id, &file_meta) {
          let message = format!("failed to store metadata for {}: {}", file_entry.path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          // File was written successfully, so we still count it.
        }

        files_pulled += 1;
        total_bytes += file_size;
        tracing::debug!("pulled file: {} ({} bytes)", file_entry.path, file_size);
      }
      Err(error) => {
        let message = format!("failed to download {}: {}", file_entry.path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
      }
    }
  }

  // Process deleted files.
  if relationship.delete_propagation.remote_to_local {
    for deleted_entry in &diff.changes.files_deleted {
      let local_file_path = compute_local_path(
        &deleted_entry.path,
        &relationship.remote_path,
        local_base,
      );

      if local_file_path.exists() {
        if let Err(error) = std::fs::remove_file(&local_file_path) {
          let message = format!("failed to delete {:?}: {}", local_file_path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          files_failed += 1;
          continue;
        }
      }

      // Remove the metadata entry regardless of whether the local file existed.
      if let Err(error) = metadata_store.delete_file_meta(&relationship.id, &deleted_entry.path) {
        let message = format!("failed to delete metadata for {}: {}", deleted_entry.path, error);
        tracing::warn!("{}", message);
        errors.push(message);
      }

      files_deleted += 1;
      tracing::debug!("deleted local file: {}", deleted_entry.path);
    }
  }

  // Process added and modified symlinks.
  let symlinks_to_create: Vec<&RemoteSyncSymlinkEntry> = diff.changes.symlinks_added.iter()
    .chain(diff.changes.symlinks_modified.iter())
    .collect();

  for symlink_entry in symlinks_to_create {
    let local_symlink_path = compute_local_path(
      &symlink_entry.path,
      &relationship.remote_path,
      local_base,
    );

    // Create parent directories if needed.
    if let Some(parent) = local_symlink_path.parent() {
      if let Err(error) = std::fs::create_dir_all(parent) {
        let message = format!(
          "failed to create parent directory for symlink {:?}: {}",
          local_symlink_path, error,
        );
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    }

    // Remove existing file/symlink before creating new one.
    if local_symlink_path.exists() || local_symlink_path.is_symlink() {
      let _ = std::fs::remove_file(&local_symlink_path);
    }

    #[cfg(unix)]
    {
      if let Err(error) = std::os::unix::fs::symlink(&symlink_entry.target, &local_symlink_path) {
        let message = format!("failed to create symlink {:?}: {}", local_symlink_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    }

    #[cfg(not(unix))]
    {
      let message = format!("symlinks not supported on this platform: {}", symlink_entry.path);
      tracing::warn!("{}", message);
      errors.push(message);
      files_failed += 1;
      continue;
    }

    // Store symlink metadata.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let symlink_meta = FileSyncMeta {
      path:           symlink_entry.path.clone(),
      content_hash:   symlink_entry.hash.clone(),
      size:           0,
      modified_at:    now_ms,
      sync_status:    SyncStatus::Synced,
      last_synced_at: now_ms,
    };

    if let Err(error) = metadata_store.set_file_meta(&relationship.id, &symlink_meta) {
      let message = format!("failed to store symlink metadata for {}: {}", symlink_entry.path, error);
      tracing::warn!("{}", message);
      errors.push(message);
    }

    symlinks_pulled += 1;
    tracing::debug!("pulled symlink: {} -> {}", symlink_entry.path, symlink_entry.target);
  }

  // Process deleted symlinks.
  if relationship.delete_propagation.remote_to_local {
    for deleted_entry in &diff.changes.symlinks_deleted {
      let local_symlink_path = compute_local_path(
        &deleted_entry.path,
        &relationship.remote_path,
        local_base,
      );

      if local_symlink_path.exists() || local_symlink_path.is_symlink() {
        if let Err(error) = std::fs::remove_file(&local_symlink_path) {
          let message = format!("failed to delete symlink {:?}: {}", local_symlink_path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          files_failed += 1;
          continue;
        }
      }

      if let Err(error) = metadata_store.delete_file_meta(&relationship.id, &deleted_entry.path) {
        let message = format!("failed to delete symlink metadata for {}: {}", deleted_entry.path, error);
        tracing::warn!("{}", message);
        errors.push(message);
      }

      files_deleted += 1;
      tracing::debug!("deleted local symlink: {}", deleted_entry.path);
    }
  }

  // Save the new checkpoint with the remote's root hash.
  let now_ms = chrono::Utc::now().timestamp_millis();
  let new_checkpoint = SyncCheckpoint {
    relationship_id:  relationship.id.clone(),
    remote_root_hash: new_root_hash,
    last_sync_at:     now_ms,
  };

  metadata_store.set_checkpoint(&new_checkpoint)?;

  let duration_ms = start.elapsed().as_millis() as u64;

  tracing::info!(
    "pull sync complete for '{}': {} pulled, {} skipped, {} failed, {} deleted, {} symlinks ({}ms)",
    relationship.name, files_pulled, files_skipped, files_failed,
    files_deleted, symlinks_pulled, duration_ms,
  );

  Ok(PullResult {
    files_pulled,
    files_skipped,
    files_failed,
    files_deleted,
    symlinks_pulled,
    total_bytes,
    duration_ms,
    errors,
  })
}

/// Call POST /sync/diff on the remote aeordb server.
///
/// This is a standalone implementation for the pull module, separate from
/// replication.rs's version. The pull module works with hex-encoded root
/// hashes (from SyncCheckpoint) rather than raw byte slices.
async fn fetch_remote_diff(
  connection: &RemoteConnection,
  since_root_hash: Option<&str>,
) -> Result<RemoteSyncDiffResponse> {
  let url = format!("{}/sync/diff", connection.url);
  let client = reqwest::Client::new();

  let body = serde_json::json!({
    "since_root_hash": since_root_hash,
  });

  let mut request = client.post(&url).json(&body);

  if connection.auth_type == AuthType::ApiKey {
    if let Some(ref api_key) = connection.api_key {
      request = request.header("Authorization", format!("Bearer {}", api_key));
    }
  }

  let response = request.send().await
    .map_err(|error| ClientError::Server(
      format!("sync/diff request failed: {}", error),
    ))?;

  if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(ClientError::Server(
      format!("sync/diff returned HTTP {}: {}", status, body),
    ));
  }

  response.json().await
    .map_err(|error| ClientError::Server(
      format!("failed to parse sync/diff response: {}", error),
    ))
}

/// Compute the local filesystem path from a remote path.
///
/// Strips the remote base prefix and joins the remainder onto the local base.
///
/// Example:
///   remote_path:  "/docs/subdir/report.pdf"
///   remote_base:  "/docs/"
///   local_base:   "/home/user/sync"
///   result:       "/home/user/sync/subdir/report.pdf"
fn compute_local_path(
  remote_path: &str,
  remote_base: &str,
  local_base: &Path,
) -> std::path::PathBuf {
  let base = remote_base.trim_end_matches('/');

  // Strip the remote base prefix to get the relative portion.
  let relative = if remote_path.starts_with(base) {
    &remote_path[base.len()..]
  } else {
    remote_path
  };

  // Strip leading slash from the relative path.
  let relative = relative.trim_start_matches('/');

  local_base.join(relative)
}

/// Get the file modification time as milliseconds since the Unix epoch.
fn file_mtime(path: &Path) -> Result<i64> {
  let metadata = path.metadata()?;
  let modified = metadata.modified()?;
  let duration = modified
    .duration_since(std::time::UNIX_EPOCH)
    .map_err(|error| ClientError::Io(
      std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("system time error: {}", error),
      ),
    ))?;

  Ok(duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_compute_local_path_basic() {
    let result = compute_local_path(
      "/docs/readme.md",
      "/docs/",
      Path::new("/home/user/sync"),
    );
    assert_eq!(result, Path::new("/home/user/sync/readme.md"));
  }

  #[test]
  fn test_compute_local_path_nested() {
    let result = compute_local_path(
      "/docs/subdir/report.pdf",
      "/docs/",
      Path::new("/home/user/sync"),
    );
    assert_eq!(result, Path::new("/home/user/sync/subdir/report.pdf"));
  }

  #[test]
  fn test_compute_local_path_root_base() {
    let result = compute_local_path(
      "/file.txt",
      "/",
      Path::new("/tmp/sync"),
    );
    assert_eq!(result, Path::new("/tmp/sync/file.txt"));
  }

  #[test]
  fn test_compute_local_path_no_trailing_slash() {
    let result = compute_local_path(
      "/docs/readme.md",
      "/docs",
      Path::new("/home/user/sync"),
    );
    assert_eq!(result, Path::new("/home/user/sync/readme.md"));
  }

  #[test]
  fn test_compute_local_path_deeply_nested() {
    let result = compute_local_path(
      "/data/a/b/c/file.txt",
      "/data/",
      Path::new("/mnt/sync"),
    );
    assert_eq!(result, Path::new("/mnt/sync/a/b/c/file.txt"));
  }
}
