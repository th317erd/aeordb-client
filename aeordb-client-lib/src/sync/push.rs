use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use super::file_mtime;
use crate::connections::RemoteConnection;
use crate::error::{ClientError, Result};
use crate::remote::{BlobConfig, CommitFile, RemoteClient, chunk_hash};
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
  all_relationships: &[SyncRelationship],
  http_client: &reqwest::Client,
  jwt_cache: &crate::jwt_cache::JwtCache,
) -> Result<PushResult> {
  let start = Instant::now();

  let jwt_slot = jwt_cache.slot_for(&connection.id);
  let remote_client = RemoteClient::from_connection_cached(connection, http_client, jwt_slot);
  let metadata_store = SyncMetadataStore::new(state);

  // Fetch the engine's chunk parameters once per push cycle. Files
  // uploaded in this cycle all chunk to the same size + use the same
  // hash prefix. If the engine ever changes these mid-flight (very
  // unlikely), a subsequent cycle will pick up the new values.
  let blob_config = remote_client.blob_config().await
    .map_err(|e| ClientError::Server(format!("blob_config failed: {}", e)))?;

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

  // Build the list of local directories owned by child relationships so
  // the walker can skip them — otherwise a parent that wraps a child's
  // folder would re-push every file the child is also responsible for.
  let local_exclusions = crate::sync::hierarchy::child_local_exclusions(relationship, all_relationships);

  // Walk the local filesystem recursively in a blocking task since
  // std::fs::read_dir is inherently synchronous and recursive.
  let local_base_owned = local_base.to_path_buf();
  let local_exclusions_owned = local_exclusions.clone();
  let walker = tokio::task::spawn_blocking(move || walkdir(&local_base_owned, &local_exclusions_owned))
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

    // Chunk-based upload via the engine's content-addressable blob API.
    // We've already got the full file in `content` (buffered for the
    // blake3 hash above); slice it into engine-sized chunks, ask the
    // engine which it's missing, upload only those, then commit the
    // file by listing the ordered chunk hashes.
    //
    // The previous code path used a full-file PUT, which re-uploaded
    // the entire body on every change regardless of how small the
    // diff was — a 1 GB file with a 4 KB edit was 1 GB on the wire.
    // The chunk path makes that the same 4 KB edit roughly one chunk
    // (~256 KB today) of network traffic; cross-file dedup falls out
    // naturally too (a chunk that's already in the engine's store
    // skips re-upload regardless of which file referenced it first).
    let content_type = mime_from_extension(&entry_path);

    let push_outcome = push_file_via_chunks(
      &remote_client,
      &blob_config,
      &remote_path,
      &content,
      content_type.as_deref(),
    ).await;
    // Drop after the chunk path is done with it — could be large.
    drop(content);

    match push_outcome {
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
        let is_forbidden = message.contains("403 Forbidden");
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;

        // 403 means the user lacks write permission on this remote path
        // (typical when the share grants read-only access). Retrying every
        // sync cycle spams the activity log. Record metadata with the
        // current local hash/mtime so the fast/hash skip paths catch it
        // next cycle — we'll only re-attempt if the local file actually
        // changes (in which case the user presumably means to push).
        if is_forbidden {
          let now_ms = chrono::Utc::now().timestamp_millis();
          let suppressed_meta = FileSyncMeta {
            path:           remote_path.clone(),
            content_hash:   content_hash.clone(),
            size:           file_size,
            modified_at:    mtime,
            sync_status:    SyncStatus::Synced,
            last_synced_at: now_ms,
          };
          let _ = metadata_store.set_file_meta(&relationship.id, &suppressed_meta);
        }
      }
    }
  }

  // Detect deleted files: entries in metadata that no longer exist on disk.
  if relationship.delete_propagation.local_to_remote {
    let tracked_files = metadata_store.list_file_metas(&relationship.id)?;
    // If a child relationship now owns part of the remote tree, files
    // under those prefixes will be missing from our walk by design —
    // they aren't deletions, so suppress the delete_file call.
    let remote_exclusions = crate::sync::hierarchy::child_exclusions(relationship, all_relationships);

    for meta in tracked_files {
      if seen_remote_paths.contains(&meta.path) {
        continue;
      }
      if crate::sync::hierarchy::is_excluded_by_child(&meta.path, &remote_exclusions) {
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
/// Skips directories themselves (the caller handles that). Any directory
/// whose path matches one of `exclusions` is not descended into — used to
/// skip child sync-relationships' local territory so a parent sync doesn't
/// re-push files the child is already handling.
fn walkdir(root: &Path, exclusions: &[std::path::PathBuf]) -> Result<Vec<std::path::PathBuf>> {
  let mut results = Vec::new();
  walk_recursive(root, exclusions, &mut results)?;
  Ok(results)
}

fn walk_recursive(
  dir:        &Path,
  exclusions: &[std::path::PathBuf],
  results:    &mut Vec<std::path::PathBuf>,
) -> Result<()> {
  let entries = std::fs::read_dir(dir)?;

  for entry in entries {
    let entry = entry?;
    let path = entry.path();
    let file_type = entry.file_type()?;

    if file_type.is_symlink() || file_type.is_file() {
      // A file at the exclusion root itself shouldn't happen (exclusions
      // are directories), but check anyway so we never sync a file that
      // belongs to a child relationship.
      if crate::sync::hierarchy::is_local_excluded_by_child(&path, exclusions) {
        continue;
      }
      results.push(path);
    } else if file_type.is_dir() {
      if crate::sync::hierarchy::is_local_excluded_by_child(&path, exclusions) {
        continue;
      }
      walk_recursive(&path, exclusions, results)?;
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

/// Upload one file's content via the engine's chunk API.
///
/// Pipeline:
///   1. Split `bytes` into `config.chunk_size`-byte chunks.
///   2. Hash each chunk: `blake3(config.chunk_hash_prefix + chunk_bytes)`.
///   3. POST /blobs/check with the deduped hash list → get "needed".
///   4. PUT /blobs/chunks/{hash} for each needed chunk (dedup so a
///      file that references the same chunk twice only uploads it once).
///   5. POST /blobs/commit with the file path + ordered chunk hashes.
///
/// Empty files (zero bytes) work too: chunks list is empty, the engine
/// commits a zero-byte file from the empty list. The /blobs/check call
/// for an empty hash list is also a valid no-op.
async fn push_file_via_chunks(
  client:       &RemoteClient,
  config:       &BlobConfig,
  remote_path:  &str,
  bytes:        &[u8],
  content_type: Option<&str>,
) -> Result<()> {
  // Pre-compute (hash, byte-range) for each chunk so the upload phase
  // can borrow chunk slices without re-hashing.
  let mut hashes_in_order: Vec<String> = Vec::new();
  let mut chunk_slices:    Vec<(String, &[u8])> = Vec::new();
  for chunk in bytes.chunks(config.chunk_size) {
    let h = chunk_hash(&config.chunk_hash_prefix, chunk);
    hashes_in_order.push(h.clone());
    chunk_slices.push((h, chunk));
  }

  // De-dup the check request — a file that references the same chunk
  // multiple times only needs to ask about it once.
  let mut seen = HashSet::new();
  let unique_hashes: Vec<String> = hashes_in_order.iter()
    .filter(|h| seen.insert((*h).clone()))
    .cloned()
    .collect();

  // /blobs/check on an empty list is allowed; engine returns empty
  // have/needed. Skip the round-trip in that case.
  let mut needed_set: HashSet<String> = if unique_hashes.is_empty() {
    HashSet::new()
  } else {
    let response = client.blob_check(&unique_hashes).await?;
    response.needed.into_iter().collect()
  };

  // Upload each needed chunk exactly once. needed_set.remove returns
  // true the first time we see a hash; subsequent occurrences in
  // chunk_slices are skipped automatically.
  for (hash, chunk_bytes) in &chunk_slices {
    if needed_set.remove(hash) {
      client.upload_chunk(hash, chunk_bytes.to_vec()).await?;
    }
  }

  // Commit. The engine validates that every referenced chunk exists in
  // its store; if we forgot to upload one, this fails with InvalidInput.
  let commit_file = CommitFile {
    path:         remote_path.to_string(),
    chunks:       hashes_in_order,
    content_type: content_type.map(|s| s.to_string()),
  };
  client.blob_commit(&[commit_file]).await?;
  Ok(())
}
