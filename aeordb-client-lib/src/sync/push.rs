use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use tokio_util::io::ReaderStream;

use super::file_mtime;
use crate::connections::RemoteConnection;
use crate::error::{ClientError, Result};
use crate::remote::RemoteClient;
use crate::state::StateStore;
use crate::sync::content_type::mime_from_extension;
use crate::sync::filter::matches_filter;
use crate::sync::metadata::{FileSyncMeta, SyncMetadataStore, SyncStatus};
use crate::sync::relationships::SyncRelationship;

/// Result of a push sync operation.
pub struct PushResult {
  pub files_pushed:  u64,
  pub files_skipped: u64,
  pub files_failed:  u64,
  pub files_deleted: u64,
  pub total_bytes:   u64,
  pub duration_ms:   u64,
  pub errors:        Vec<String>,
}

/// Push local filesystem changes to a remote aeordb server.
///
/// Scans the local directory recursively, detects changes by comparing
/// filesystem metadata against stored sync metadata, and uploads changed
/// files directly to the remote. No file content is stored locally in
/// aeordb -- only metadata.
pub async fn push_sync(
  state: &StateStore,
  connection: &RemoteConnection,
  relationship: &SyncRelationship,
  http_client: &reqwest::Client,
) -> Result<PushResult> {
  let start = Instant::now();

  let remote_client = RemoteClient::from_connection(connection, http_client);
  let metadata_store = SyncMetadataStore::new(state);

  let local_base = Path::new(&relationship.local_path);
  if !local_base.exists() {
    return Err(ClientError::Configuration(
      format!("local path does not exist: {}", relationship.local_path),
    ));
  }

  let mut files_pushed: u64 = 0;
  let mut files_skipped: u64 = 0;
  let mut files_failed: u64 = 0;
  let mut files_deleted: u64 = 0;
  let mut total_bytes: u64 = 0;
  let mut errors: Vec<String> = Vec::new();

  // Track which remote paths we see on the filesystem, so we can
  // detect deletions (files in metadata but gone from disk).
  let mut seen_remote_paths: HashSet<String> = HashSet::new();

  // Walk the local filesystem recursively in a blocking task since
  // std::fs::read_dir is inherently synchronous and recursive.
  let local_base_owned = local_base.to_path_buf();
  let walker = tokio::task::spawn_blocking(move || walkdir(&local_base_owned))
    .await
    .map_err(|error| ClientError::Io(
      std::io::Error::new(std::io::ErrorKind::Other, format!("walkdir task panicked: {}", error)),
    ))??;

  for entry_path in walker {
    let file_type = match entry_path.symlink_metadata() {
      Ok(meta) => meta.file_type(),
      Err(error) => {
        let message = format!("failed to read metadata for {:?}: {}", entry_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    };

    // Compute the remote path for this entry.
    let relative = match entry_path.strip_prefix(local_base) {
      Ok(rel) => rel,
      Err(_) => {
        continue;
      }
    };

    let remote_path = compute_remote_path(relative, &relationship.remote_path);

    // Handle symlinks.
    if file_type.is_symlink() {
      seen_remote_paths.insert(remote_path.clone());

      let target = match std::fs::read_link(&entry_path) {
        Ok(target) => target.to_string_lossy().to_string(),
        Err(error) => {
          let message = format!("failed to read symlink {:?}: {}", entry_path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          files_failed += 1;
          continue;
        }
      };

      match remote_client.create_symlink(&remote_path, &target).await {
        Ok(()) => {
          files_pushed += 1;
          tracing::debug!("pushed symlink: {} -> {}", remote_path, target);
        }
        Err(error) => {
          let message = format!("failed to push symlink {}: {}", remote_path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          files_failed += 1;
        }
      }

      continue;
    }

    // Skip directories -- we only care about files and symlinks.
    if !file_type.is_file() {
      continue;
    }

    // Apply glob filter on the filename.
    let filename = match entry_path.file_name().and_then(|n| n.to_str()) {
      Some(name) => name,
      None => {
        continue;
      }
    };

    if !matches_filter(filename, relationship.filter.as_deref()) {
      files_skipped += 1;
      continue;
    }

    seen_remote_paths.insert(remote_path.clone());

    // Get filesystem mtime.
    let mtime = match file_mtime(&entry_path) {
      Ok(mtime) => mtime,
      Err(error) => {
        let message = format!("failed to get mtime for {:?}: {}", entry_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    };

    // Check stored metadata for this file.
    let stored_meta = metadata_store.get_file_meta(&relationship.id, &remote_path)?;

    // Fast skip: mtime matches and status is Synced.
    if let Some(ref meta) = stored_meta {
      if meta.modified_at == mtime && meta.sync_status == SyncStatus::Synced {
        files_skipped += 1;
        continue;
      }
    }

    // Read file content for hashing -- we still need the full content to
    // compute the BLAKE3 hash for change detection. Use async read.
    let content = match tokio::fs::read(&entry_path).await {
      Ok(bytes) => bytes,
      Err(error) => {
        let message = format!("failed to read {:?}: {}", entry_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    };

    let content_hash = blake3::hash(&content).to_hex().to_string();
    let file_size = content.len() as u64;

    // Hash skip: content unchanged, just update mtime in metadata.
    if let Some(ref meta) = stored_meta {
      if meta.content_hash == content_hash {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let updated_meta = FileSyncMeta {
          path:           remote_path.clone(),
          content_hash:   content_hash.clone(),
          size:           file_size,
          modified_at:    mtime,
          sync_status:    SyncStatus::Synced,
          last_synced_at: now_ms,
        };

        metadata_store.set_file_meta(&relationship.id, &updated_meta)?;
        files_skipped += 1;
        continue;
      }
    }

    // Move detection: if no metadata exists for this path but another path
    // has the same content hash, this is likely a file that was moved/renamed
    // locally. Use a remote rename instead of re-uploading the content.
    if stored_meta.is_none() {
      let all_metas = metadata_store.list_file_metas(&relationship.id).unwrap_or_default();
      let moved_from = all_metas.iter().find(|m| {
        m.content_hash == content_hash && m.path != remote_path && !seen_remote_paths.contains(&m.path)
      });

      if let Some(source_meta) = moved_from {
        let old_path = source_meta.path.clone();
        match remote_client.rename_file(&old_path, &remote_path).await {
          Ok(()) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            // Remove old metadata
            metadata_store.delete_file_meta(&relationship.id, &old_path)?;
            // Create new metadata at the new path
            let new_meta = FileSyncMeta {
              path:           remote_path.clone(),
              content_hash:   content_hash.clone(),
              size:           file_size,
              modified_at:    mtime,
              sync_status:    SyncStatus::Synced,
              last_synced_at: now_ms,
            };
            metadata_store.set_file_meta(&relationship.id, &new_meta)?;
            files_pushed += 1;
            tracing::info!("moved on remote: {} -> {}", old_path, remote_path);
            continue;
          }
          Err(error) => {
            // Move failed — fall through to upload
            tracing::debug!("remote move failed ({}), will upload instead", error);
          }
        }
      }
    }

    // Upload to remote using a streaming body from the file on disk.
    let content_type = mime_from_extension(&entry_path);

    // Open the file and create a streaming body to avoid holding the full
    // content in memory during the upload (the `content` Vec is dropped
    // after hashing; we re-open for streaming).
    drop(content);

    let upload_body = match tokio::fs::File::open(&entry_path).await {
      Ok(file) => {
        let stream = ReaderStream::new(file);
        reqwest::Body::wrap_stream(stream)
      }
      Err(error) => {
        let message = format!("failed to open {:?} for upload: {}", entry_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        continue;
      }
    };

    match remote_client
      .upload_file(&remote_path, upload_body, content_type.as_deref())
      .await
    {
      Ok(()) => {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let new_meta = FileSyncMeta {
          path:           remote_path.clone(),
          content_hash,
          size:           file_size,
          modified_at:    mtime,
          sync_status:    SyncStatus::Synced,
          last_synced_at: now_ms,
        };

        metadata_store.set_file_meta(&relationship.id, &new_meta)?;
        files_pushed += 1;
        total_bytes += file_size;
        tracing::debug!("pushed file: {} ({} bytes)", remote_path, file_size);
      }
      Err(error) => {
        let message = format!("failed to upload {}: {}", remote_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
      }
    }
  }

  // Detect deleted files: entries in metadata that no longer exist on disk.
  if relationship.delete_propagation.local_to_remote {
    let tracked_files = metadata_store.list_file_metas(&relationship.id)?;

    for meta in tracked_files {
      if seen_remote_paths.contains(&meta.path) {
        continue;
      }

      // File exists in metadata but not on filesystem -- it was deleted.
      match remote_client.delete_file(&meta.path).await {
        Ok(()) => {
          metadata_store.delete_file_meta(&relationship.id, &meta.path)?;
          files_deleted += 1;
          tracing::debug!("deleted remote file: {}", meta.path);
        }
        Err(error) => {
          let message = format!("failed to delete remote {}: {}", meta.path, error);
          tracing::warn!("{}", message);
          errors.push(message);
          files_failed += 1;
        }
      }
    }
  }

  let duration_ms = start.elapsed().as_millis() as u64;

  Ok(PushResult {
    files_pushed,
    files_skipped,
    files_failed,
    files_deleted,
    total_bytes,
    duration_ms,
    errors,
  })
}

/// Recursively walk a directory, returning all file and symlink paths.
/// Skips directories themselves (the caller handles that).
fn walkdir(root: &Path) -> Result<Vec<std::path::PathBuf>> {
  let mut results = Vec::new();
  walk_recursive(root, &mut results)?;
  Ok(results)
}

fn walk_recursive(dir: &Path, results: &mut Vec<std::path::PathBuf>) -> Result<()> {
  let entries = std::fs::read_dir(dir)?;

  for entry in entries {
    let entry = entry?;
    let path = entry.path();
    let file_type = entry.file_type()?;

    if file_type.is_symlink() || file_type.is_file() {
      results.push(path);
    } else if file_type.is_dir() {
      walk_recursive(&path, results)?;
    }
  }

  Ok(())
}

/// Compute the remote path from a relative local path and the remote base.
///
/// Example:
///   relative: "subdir/report.pdf"
///   remote_base: "/docs/"
///   result: "/docs/subdir/report.pdf"
fn compute_remote_path(relative: &Path, remote_base: &str) -> String {
  let relative_str = relative.to_string_lossy();
  let base = remote_base.trim_end_matches('/');

  format!("{}/{}", base, relative_str)
}
