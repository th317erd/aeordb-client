use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::file_mtime_async;
use crate::connections::RemoteConnection;
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
  all_relationships: &[SyncRelationship],
  http_client: &reqwest::Client,
  jwt_cache: &crate::jwt_cache::JwtCache,
) -> Result<PullResult> {
  let start = Instant::now();

  let jwt_slot = jwt_cache.slot_for(&connection.id);
  let remote_client = RemoteClient::from_connection_cached(connection, http_client, jwt_slot);
  let metadata_store = SyncMetadataStore::new(state);

  // Fetch the engine's chunk parameters once per pull cycle.
  let blob_config = remote_client.blob_config().await
    .map_err(|e| ClientError::Server(format!("blob_config failed: {}", e)))?;

  let local_base = Path::new(&relationship.local_path);
  if !local_base.exists() {
    tokio::fs::create_dir_all(local_base).await?;
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

  // Fetch the diff from the remote. Pass the cache so this call
  // reuses the same JWT slot the rest of pull_sync uses, instead of
  // minting its own token via the (now-removed) inline /auth/token
  // dance that was creating a fresh refresh-token row on every cycle.
  let diff = fetch_remote_diff(connection, since_root_hash.as_deref(), http_client, jwt_cache).await?;
  let new_root_hash = diff.root_hash.clone();

  // Process added and modified files.
  //
  // Two-phase chunk-based pull:
  //   Phase 1 — plan: per file, hash any existing local file by chunks
  //             and compute which remote chunks we don't already have
  //             locally. Accumulate a deduped set across all files.
  //   Phase 2 — fetch: pull all unique needed chunks in batches via
  //             /sync/chunks.
  //   Phase 3 — assemble: write each file by stitching local-reuse
  //             chunks and fetched chunks in the order the engine gave
  //             us, then atomic-rename into place.
  //
  // The "local-reuse" path is the bandwidth win — if the user touched
  // only one chunk in a 100MB file, we re-use the other ~399 chunks
  // straight from the existing on-disk copy and pull only the dirty
  // ~256KB from the engine.
  let files_to_download: Vec<&RemoteSyncFileEntry> = diff.changes.files_added.iter()
    .chain(diff.changes.files_modified.iter())
    .collect();

  // Per-file plan computed during phase 1.
  struct FilePlan<'a> {
    entry:        &'a RemoteSyncFileEntry,
    local_path:   PathBuf,
    // hash -> (offset_in_local_file, chunk_len) for chunks already on disk.
    local_chunks: HashMap<String, (u64, usize)>,
  }

  let mut plans: Vec<FilePlan> = Vec::with_capacity(files_to_download.len());
  let mut unique_needed: HashMap<String, ()> = HashMap::new();

  // Remote-path prefixes owned by child relationships — files under
  // these prefixes are this sync's responsibility to skip, since a
  // more-specific relationship is already handling them. Without this
  // a parent that covers /docs/ and a child that covers /docs/secrets/
  // would both pull the same files into different local directories.
  let remote_exclusions = crate::sync::hierarchy::child_exclusions(relationship, all_relationships);

  for file_entry in files_to_download {
    if crate::sync::hierarchy::is_excluded_by_child(&file_entry.path, &remote_exclusions) {
      files_skipped += 1;
      continue;
    }

    let filename = Path::new(&file_entry.path)
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or("");

    if !matches_filter(filename, relationship.filter.as_deref()) {
      files_skipped += 1;
      continue;
    }

    let local_file_path = compute_local_path(
      &file_entry.path,
      &relationship.remote_path,
      local_base,
    );

    if let Some(parent) = local_file_path.parent() {
      if let Err(error) = tokio::fs::create_dir_all(parent).await {
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

    // Hash the existing local file by chunks so we can avoid re-fetching
    // any chunk whose hash matches one the engine already lists.
    let local_chunks = match hash_local_file_chunks(
      &local_file_path,
      blob_config.chunk_size,
      &blob_config.chunk_hash_prefix,
    ).await {
      Ok(map) => map,
      Err(error) => {
        // Hashing failure isn't fatal — fall back to "treat all chunks
        // as needed" so we still pull the file end-to-end.
        tracing::warn!(
          "failed to hash existing local file {:?} (will refetch in full): {}",
          local_file_path, error,
        );
        HashMap::new()
      }
    };

    for hash in &file_entry.chunk_hashes {
      if !local_chunks.contains_key(hash) {
        unique_needed.insert(hash.clone(), ());
      }
    }

    plans.push(FilePlan { entry: file_entry, local_path: local_file_path, local_chunks });
  }

  // Phase 2: batch-fetch unique needed chunks.
  //
  // Engine caps /sync/chunks at 10,000 hashes per request and 512MB
  // total response bytes. With 256KB chunks the worst-case batch is
  // 512MB / 256KB ≈ 2,000 chunks, so we cap at 2,000 to stay safely
  // under both limits. Smaller chunks in the batch waste a little
  // request overhead but never trip the response-size guard.
  const SYNC_CHUNKS_BATCH: usize = 2000;
  let mut fetched: HashMap<String, Vec<u8>> = HashMap::with_capacity(unique_needed.len());
  let needed_hashes: Vec<String> = unique_needed.into_keys().collect();
  let mut fetch_error: Option<String> = None;

  for batch in needed_hashes.chunks(SYNC_CHUNKS_BATCH) {
    match remote_client.sync_chunks(batch).await {
      Ok(pairs) => {
        for (h, bytes) in pairs {
          fetched.insert(h, bytes);
        }
      }
      Err(error) => {
        fetch_error = Some(format!("sync_chunks failed: {}", error));
        break;
      }
    }
  }

  // Phase 3: assemble each file.
  for plan in plans {
    if let Some(ref err) = fetch_error {
      let message = format!("skipping {} ({})", plan.entry.path, err);
      tracing::warn!("{}", message);
      errors.push(message);
      files_failed += 1;
      continue;
    }

    match assemble_file(&plan.entry.chunk_hashes, &plan.local_path, &plan.local_chunks, &fetched).await {
      Ok((file_size, content_hash)) => {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mtime = file_mtime_async(&plan.local_path).await.unwrap_or(now_ms);

        let file_meta = FileSyncMeta {
          path:           plan.entry.path.clone(),
          content_hash,
          size:           file_size,
          modified_at:    mtime,
          sync_status:    SyncStatus::Synced,
          last_synced_at: now_ms,
        };

        if let Err(error) = metadata_store.set_file_meta(&relationship.id, &file_meta) {
          let message = format!("failed to store metadata for {}: {}", plan.entry.path, error);
          tracing::warn!("{}", message);
          errors.push(message);
        }

        files_pulled += 1;
        total_bytes += file_size;
        tracing::debug!("pulled file: {} ({} bytes, chunked)", plan.entry.path, file_size);
      }
      Err(error) => {
        let message = format!("failed to assemble {}: {}", plan.entry.path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
      }
    }
  }

  // Process deleted files.
  if relationship.delete_propagation.remote_to_local {
    for deleted_entry in &diff.changes.files_deleted {
      if crate::sync::hierarchy::is_excluded_by_child(&deleted_entry.path, &remote_exclusions) {
        continue;
      }

      let local_file_path = compute_local_path(
        &deleted_entry.path,
        &relationship.remote_path,
        local_base,
      );

      if local_file_path.exists() {
        if let Err(error) = tokio::fs::remove_file(&local_file_path).await {
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
    if crate::sync::hierarchy::is_excluded_by_child(&symlink_entry.path, &remote_exclusions) {
      continue;
    }

    let local_symlink_path = compute_local_path(
      &symlink_entry.path,
      &relationship.remote_path,
      local_base,
    );

    // Create parent directories if needed.
    if let Some(parent) = local_symlink_path.parent() {
      if let Err(error) = tokio::fs::create_dir_all(parent).await {
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
    // Use symlink_metadata to detect symlinks (metadata follows symlinks).
    let exists = tokio::fs::symlink_metadata(&local_symlink_path).await.is_ok();
    if exists {
      let _ = tokio::fs::remove_file(&local_symlink_path).await;
    }

    #[cfg(unix)]
    {
      if let Err(error) = tokio::fs::symlink(&symlink_entry.target, &local_symlink_path).await {
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
      if crate::sync::hierarchy::is_excluded_by_child(&deleted_entry.path, &remote_exclusions) {
        continue;
      }

      let local_symlink_path = compute_local_path(
        &deleted_entry.path,
        &relationship.remote_path,
        local_base,
      );

      let exists = tokio::fs::symlink_metadata(&local_symlink_path).await.is_ok();
      if exists {
        if let Err(error) = tokio::fs::remove_file(&local_symlink_path).await {
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
  http_client: &reqwest::Client,
  jwt_cache: &crate::jwt_cache::JwtCache,
) -> Result<RemoteSyncDiffResponse> {
  let base = connection.base_url();
  let url = format!("{}/sync/diff", base);

  let body = serde_json::json!({
    "since_root_hash": since_root_hash,
  });

  // Authenticate via the shared JWT cache instead of doing an inline
  // /auth/token POST on every diff. Previously this function did its
  // own token exchange per call which (a) duplicated the logic in
  // RemoteClient::auth_header and (b) bypassed the cache entirely,
  // minting a fresh JWT + refresh-token-row on every 60s pull cycle.
  // authed_send handles auth + 401-retry uniformly with every other
  // engine call.
  let jwt_slot = jwt_cache.slot_for(&connection.id);
  let remote_client = RemoteClient::from_connection_cached(connection, http_client, jwt_slot);

  let response = remote_client.authed_send(|| http_client.post(&url).json(&body)).await
    .map_err(|error| ClientError::Server(format!("sync/diff request failed: {}", error)))?;

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

/// Hash an existing local file by chunks using the same scheme the
/// engine uses, so we can identify which remote chunks are already on
/// disk and don't need to be re-downloaded.
///
/// Returns a map of chunk hash → (offset, length) so the assembler can
/// seek back into the same local file and copy those chunks straight
/// into the new file rather than fetching them over the network.
///
/// If the file doesn't exist or is empty, returns an empty map (the
/// new-file case — all chunks will be marked needed).
async fn hash_local_file_chunks(
  path: &Path,
  chunk_size: usize,
  hash_prefix: &str,
) -> Result<HashMap<String, (u64, usize)>> {
  let mut map: HashMap<String, (u64, usize)> = HashMap::new();

  let mut file = match tokio::fs::File::open(path).await {
    Ok(f) => f,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
    Err(e) => return Err(ClientError::Io(e)),
  };

  let mut buf = vec![0u8; chunk_size];
  let mut offset: u64 = 0;
  loop {
    // read_exact-on-truncated-eof would error; instead read in a loop
    // until we've either filled the buffer or hit EOF, so the final
    // (possibly partial) chunk is handled correctly.
    let mut filled = 0;
    while filled < chunk_size {
      let n = file.read(&mut buf[filled..]).await.map_err(ClientError::Io)?;
      if n == 0 { break; }
      filled += n;
    }
    if filled == 0 { break; }

    let h = crate::remote::chunk_hash(hash_prefix, &buf[..filled]);
    map.insert(h, (offset, filled));
    offset += filled as u64;

    if filled < chunk_size { break; }
  }

  Ok(map)
}

/// Assemble a file from a list of remote chunk hashes, reading each
/// chunk either from the existing local file (when its hash matches one
/// the engine listed for the same path) or from the network-fetched map.
///
/// Writes to `<path>.tmp` and atomic-renames into place so a partial
/// write can't leave a torn file on disk. Returns `(size, content_hash)`
/// where content_hash is the blake3 of the assembled bytes (NOT
/// chunk-prefixed) — this matches the engine's whole-file hash and is
/// what we store in FileSyncMeta.
async fn assemble_file(
  chunk_hashes:    &[String],
  local_path:      &Path,
  local_chunks:    &HashMap<String, (u64, usize)>,
  fetched:         &HashMap<String, Vec<u8>>,
) -> Result<(u64, String)> {
  // Hold the existing local file open across the whole assembly so the
  // local-reuse reads can still find the bytes we hashed earlier. The
  // handle is dropped before the rename below so the rename can succeed
  // on Windows (Linux/macOS would tolerate an open handle but Windows
  // would refuse the rename).
  let mut local_reader = if local_chunks.is_empty() {
    None
  } else {
    match tokio::fs::File::open(local_path).await {
      Ok(f) => Some(f),
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
      Err(e) => return Err(ClientError::Io(e)),
    }
  };

  let tmp_path: PathBuf = {
    let mut p = local_path.as_os_str().to_owned();
    p.push(".aeordb-pull.tmp");
    PathBuf::from(p)
  };

  let mut tmp_file = tokio::fs::File::create(&tmp_path).await.map_err(ClientError::Io)?;
  let mut hasher = blake3::Hasher::new();
  let mut total_size: u64 = 0;

  // Cleanup helper: on any error below, remove the tmp file so we don't
  // leak partial artifacts on disk.
  let cleanup_tmp = |path: &Path| {
    let path = path.to_owned();
    tokio::spawn(async move { let _ = tokio::fs::remove_file(&path).await; });
  };

  for hash in chunk_hashes {
    let bytes: Vec<u8> = if let Some(&(offset, len)) = local_chunks.get(hash) {
      let reader = local_reader.as_mut().ok_or_else(|| ClientError::Server(
        "internal: local_chunks non-empty but local file not open".to_string()
      ))?;
      reader.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| {
        cleanup_tmp(&tmp_path);
        ClientError::Io(e)
      })?;
      let mut buf = vec![0u8; len];
      if let Err(e) = reader.read_exact(&mut buf).await {
        cleanup_tmp(&tmp_path);
        return Err(ClientError::Io(e));
      }
      buf
    } else if let Some(bytes) = fetched.get(hash) {
      bytes.clone()
    } else {
      cleanup_tmp(&tmp_path);
      return Err(ClientError::Server(format!(
        "chunk {} missing from both local and fetched sets", hash
      )));
    };

    hasher.update(&bytes);
    total_size += bytes.len() as u64;
    if let Err(e) = tmp_file.write_all(&bytes).await {
      cleanup_tmp(&tmp_path);
      return Err(ClientError::Io(e));
    }
  }

  if let Err(e) = tmp_file.flush().await {
    cleanup_tmp(&tmp_path);
    return Err(ClientError::Io(e));
  }
  drop(tmp_file);
  // Release the local read handle before the rename so Windows will
  // allow the overwrite.
  drop(local_reader);

  if let Err(e) = tokio::fs::rename(&tmp_path, local_path).await {
    cleanup_tmp(&tmp_path);
    return Err(ClientError::Io(e));
  }

  let content_hash = hasher.finalize().to_hex().to_string();
  Ok((total_size, content_hash))
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
