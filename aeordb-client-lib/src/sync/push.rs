use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, TryStreamExt, stream};
use tokio_util::io::ReaderStream;

use super::file_mtime;
use crate::connections::RemoteConnection;
use crate::error::{ClientError, Result};
use crate::remote::{BlobConfig, CommitFile, RemoteClient, chunk_hash};
use crate::state::StateStore;
use crate::sync::content_type::mime_from_extension;
use crate::sync::filter::matches_filter;
use crate::sync::metadata::{FileSyncMeta, SyncMetadataStore, SyncPathMigration, SyncStatus};
use crate::sync::relationships::SyncRelationship;

const PUSH_BATCH_MAX_FILES: usize = 32;
const PUSH_BATCH_MAX_BYTES: usize = 64 * 1024 * 1024;
const PUSH_BATCH_MAX_CHUNKS: usize = 8_192;
const PUSH_PARALLELISM_LIMIT: usize = 4;
const PUSH_CHUNK_UPLOAD_CONCURRENCY: usize = 5;
const PUSH_SCAN_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const PUSH_FILE_READ_BUFFER_BYTES: usize = 1024 * 1024;
const BLOB_COMMIT_REQUEST_BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
const BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES: usize = 30 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushScanMode {
  /// Use filesystem metadata to avoid hashing files that are known unchanged.
  Lite,
  /// Ignore filesystem metadata and hash every file before deciding what to do.
  Full,
}

pub struct PushProgressReporter<'a> {
  relationship_id: &'a str,
  relationship_name: &'a str,
  activity: &'a crate::sync::activity::SyncActivityLog,
  event_tx: &'a tokio::sync::broadcast::Sender<crate::server::ServerEvent>,
}

impl<'a> PushProgressReporter<'a> {
  pub fn new(
    relationship_id: &'a str,
    relationship_name: &'a str,
    activity: &'a crate::sync::activity::SyncActivityLog,
    event_tx: &'a tokio::sync::broadcast::Sender<crate::server::ServerEvent>,
  ) -> Self {
    Self {
      relationship_id,
      relationship_name,
      activity,
      event_tx,
    }
  }

  fn emit(
    &self,
    summary: String,
    files_affected: u64,
    bytes_transferred: u64,
    duration_ms: u64,
    progress_percent: Option<f64>,
  ) {
    self.emit_with_type(
      "progress",
      summary,
      files_affected,
      bytes_transferred,
      duration_ms,
      progress_percent,
    );
  }

  fn emit_with_type(
    &self,
    event_type: &str,
    summary: String,
    files_affected: u64,
    bytes_transferred: u64,
    duration_ms: u64,
    progress_percent: Option<f64>,
  ) {
    let event = crate::sync::activity::SyncEvent {
      id: uuid::Uuid::new_v4().to_string(),
      relationship_id: self.relationship_id.to_string(),
      relationship_name: self.relationship_name.to_string(),
      event_type: event_type.to_string(),
      summary,
      files_affected,
      bytes_transferred,
      duration_ms,
      errors: Vec::new(),
      progress_percent,
      timestamp: chrono::Utc::now().timestamp_millis(),
    };

    if let Err(error) = self.activity.log_event(&event) {
      tracing::warn!(
        "failed to log push progress for '{}': {}",
        self.relationship_name,
        error,
      );
    }

    if let Ok(json) = serde_json::to_string(&event) {
      let _ = self
        .event_tx
        .send(crate::server::ServerEvent::new("sync_activity", json));
    }
  }
}

/// Result of a push sync operation.
pub struct PushResult {
  pub files_pushed: u64,
  pub files_skipped: u64,
  pub files_failed: u64,
  pub files_deleted: u64,
  pub total_bytes: u64,
  pub duration_ms: u64,
  pub errors: Vec<String>,
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
  scan_mode: PushScanMode,
  progress: Option<&PushProgressReporter<'_>>,
) -> Result<PushResult> {
  let start = Instant::now();

  let jwt_slot = jwt_cache.slot_for(&connection.id);
  let remote_client = RemoteClient::from_connection_cached(connection, http_client, jwt_slot);
  let metadata_store = SyncMetadataStore::new(state);

  // Fetch the engine's chunk parameters once per push cycle. Files
  // uploaded in this cycle all chunk to the same size + use the same
  // hash prefix. If the engine ever changes these mid-flight (very
  // unlikely), a subsequent cycle will pick up the new values.
  let blob_config = remote_client.blob_config().await?;

  let local_base = Path::new(&relationship.local_path);
  if !local_base.exists() {
    return Err(ClientError::Configuration(format!(
      "local path does not exist: {}",
      relationship.local_path
    )));
  }

  let mut files_pushed: u64 = 0;
  let mut files_skipped: u64 = 0;
  let mut files_failed: u64 = 0;
  let mut files_deleted: u64 = 0;
  let mut total_bytes: u64 = 0;
  let mut errors: Vec<String> = Vec::new();
  let mut metadata_by_path: HashMap<String, FileSyncMeta> = metadata_store
    .list_file_metas(&relationship.id)?
    .into_iter()
    .map(|meta| (meta.path.clone(), meta))
    .collect();
  let path_migration = metadata_store.get_path_migration(&relationship.id)?;
  if let Some(migration) = path_migration.as_ref() {
    tracing::info!(
      "push {} scan for '{}': applying path migration local {} -> {}, remote {} -> {}",
      scan_mode.label(),
      relationship.name,
      migration.old_local_path,
      migration.new_local_path,
      migration.old_remote_path,
      migration.new_remote_path,
    );
  }

  // Track which remote paths we see on the filesystem, so we can
  // detect deletions (files in metadata but gone from disk).
  let mut seen_remote_paths: HashSet<String> = HashSet::new();

  // Build the list of local directories owned by child relationships so
  // the walker can skip them — otherwise a parent that wraps a child's
  // folder would re-push every file the child is also responsible for.
  let local_exclusions =
    crate::sync::hierarchy::child_local_exclusions(relationship, all_relationships);

  // Walk the local filesystem recursively in a blocking task since
  // std::fs::read_dir is inherently synchronous and recursive.
  let local_base_owned = local_base.to_path_buf();
  let local_exclusions_owned = local_exclusions.clone();
  let discovered_entries = Arc::new(AtomicU64::new(0));
  let discovered_entries_for_walk = Arc::clone(&discovered_entries);
  let mut walker_task = tokio::task::spawn_blocking(move || {
    walkdir(
      &local_base_owned,
      &local_exclusions_owned,
      &discovered_entries_for_walk,
    )
  });
  let mut walk_heartbeat = Box::pin(tokio::time::sleep(PUSH_SCAN_HEARTBEAT_INTERVAL));
  let walker = loop {
    tokio::select! {
      result = &mut walker_task => {
        break result.map_err(|error| {
          ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("walkdir task panicked: {}", error),
          ))
        })??;
      }
      _ = &mut walk_heartbeat, if progress.is_some() => {
        emit_push_scan_heartbeat(
          relationship.name.as_str(),
          progress,
          push_scan_discovery_summary(
            scan_mode.label(),
            discovered_entries.load(Ordering::Relaxed),
            &relationship.local_path,
            start.elapsed().as_millis() as u64,
          ),
          start.elapsed().as_millis() as u64,
          Some(0.0),
        );
        walk_heartbeat
          .as_mut()
          .reset(tokio::time::Instant::now() + PUSH_SCAN_HEARTBEAT_INTERVAL);
      }
    }
  };

  let scan_mode_label = scan_mode.label();
  tracing::info!(
    "push {} scan for '{}': found {} local entries under {}",
    scan_mode_label,
    relationship.name,
    walker.len(),
    relationship.local_path,
  );
  if let Some(progress) = progress {
    progress.emit(
      format!(
        "{} scanning {} local {} in {}",
        scan_mode_label,
        crate::sync::activity::format_count(walker.len() as u64),
        if walker.len() == 1 {
          "entry"
        } else {
          "entries"
        },
        relationship.local_path,
      ),
      0,
      0,
      0,
      Some(0.0),
    );
  }

  let total_entries = walker.len() as u64;
  let mut processed_entries: u64 = 0;
  let mut inspected_entries: u64 = 0;
  let mut queued_for_hashing: u64 = 0;
  let mut last_inspection_heartbeat = Instant::now();
  let mut pending_batch = PushBatch::default();
  let mut pending_metadata_updates: Vec<FileSyncMeta> = Vec::new();
  let mut file_prep_requests: Vec<PushFilePrepRequest> = Vec::new();
  let mut pending_symlinks: Vec<PendingSymlink> = Vec::new();

  for entry_path in walker {
    inspected_entries += 1;
    if should_emit_push_scan_heartbeat(last_inspection_heartbeat, Instant::now()) {
      emit_push_scan_heartbeat(
        relationship.name.as_str(),
        progress,
        push_scan_inspection_summary(
          scan_mode_label,
          inspected_entries,
          total_entries,
          queued_for_hashing,
          start.elapsed().as_millis() as u64,
        ),
        start.elapsed().as_millis() as u64,
        Some(0.0),
      );
      last_inspection_heartbeat = Instant::now();
    }

    macro_rules! complete_entry {
      () => {
        processed_entries += 1;
        if should_emit_push_scan_progress(processed_entries) {
          flush_push_batch(
            &remote_client,
            &blob_config,
            &metadata_store,
            &relationship.id,
            &mut pending_batch,
            &mut pending_metadata_updates,
            progress,
            processed_entries,
            total_entries,
            &mut files_pushed,
            &mut files_failed,
            &mut total_bytes,
            &mut errors,
            &mut metadata_by_path,
          )
          .await?;
          emit_push_scan_progress(
            relationship.name.as_str(),
            progress,
            processed_entries,
            total_entries,
            files_pushed,
            files_skipped,
            files_failed,
            files_deleted,
            total_bytes,
            start.elapsed().as_millis() as u64,
          );
        }
      };
    }

    let entry_metadata = match entry_path.symlink_metadata() {
      Ok(meta) => meta,
      Err(error) => {
        let message = format!("failed to read metadata for {:?}: {}", entry_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        complete_entry!();
        continue;
      }
    };
    let file_type = entry_metadata.file_type();

    // Compute the remote path for this entry.
    let relative = match entry_path.strip_prefix(local_base) {
      Ok(rel) => rel,
      Err(_) => {
        complete_entry!();
        continue;
      }
    };

    let remote_path = compute_remote_path(relative, &relationship.remote_path);
    let migration_old_remote_path = path_migration
      .as_ref()
      .and_then(|migration| migration_old_remote_path_for_entry(&entry_path, migration));

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
          complete_entry!();
          continue;
        }
      };

      let content_hash = symlink_identity_hash(&remote_path, &target);
      let stored_meta = metadata_by_path.get(&remote_path).cloned();
      if can_lite_fast_skip_symlink(scan_mode, stored_meta.as_ref(), &content_hash) {
        files_skipped += 1;
        complete_entry!();
        continue;
      }

      pending_symlinks.push(PendingSymlink {
        remote_path,
        target,
        content_hash,
        modified_at: metadata_mtime_or_now(&entry_metadata),
      });
      continue;
    }

    // Skip directories -- we only care about files and symlinks.
    if !file_type.is_file() {
      complete_entry!();
      continue;
    }

    // Apply glob filter on the filename.
    let filename = match entry_path.file_name().and_then(|n| n.to_str()) {
      Some(name) => name,
      None => {
        complete_entry!();
        continue;
      }
    };

    if !matches_filter(filename, relationship.filter.as_deref()) {
      files_skipped += 1;
      complete_entry!();
      continue;
    }

    seen_remote_paths.insert(remote_path.clone());
    let file_size = entry_metadata.len();

    // Get filesystem mtime.
    let mtime = match file_mtime(&entry_path) {
      Ok(mtime) => mtime,
      Err(error) => {
        let message = format!("failed to get mtime for {:?}: {}", entry_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        complete_entry!();
        continue;
      }
    };

    // Check stored metadata for this file.
    let stored_meta = metadata_by_path.get(&remote_path).cloned();

    // Lite scans trust size + mtime + Synced status as the cheap unchanged
    // predicate. Full scans intentionally skip this and hash every file.
    if can_lite_fast_skip(scan_mode, stored_meta.as_ref(), file_size, mtime) {
      files_skipped += 1;
      complete_entry!();
      continue;
    }

    let content_type = mime_from_extension(&entry_path);
    file_prep_requests.push(PushFilePrepRequest {
      entry_path,
      remote_path,
      modified_at: mtime,
      content_type,
      stored_meta,
      migration_old_remote_path,
    });
    queued_for_hashing += 1;
  }

  let files_to_prepare = file_prep_requests.len() as u64;
  let prep_concurrency = push_worker_count(file_prep_requests.len());
  if prep_concurrency > 1 {
    tracing::info!(
      "push {} scan for '{}': preparing {} files with {} workers",
      scan_mode_label,
      relationship.name,
      file_prep_requests.len(),
      prep_concurrency,
    );
  }

  let mut prepared_files = stream::iter(file_prep_requests.into_iter().map(|request| async move {
    tokio::task::spawn_blocking(move || prepare_push_file(request))
      .await
      .map_err(|error| format!("file prep task panicked: {}", error))
      .and_then(|result| result)
  }))
  .buffer_unordered(prep_concurrency.max(1));

  let mut prep_heartbeat = Box::pin(tokio::time::sleep(PUSH_SCAN_HEARTBEAT_INTERVAL));
  let mut prepared_files_completed: u64 = 0;
  loop {
    let prepared_result = tokio::select! {
      result = prepared_files.next() => {
        match result {
          Some(result) => result,
          None => break,
        }
      }
      _ = &mut prep_heartbeat, if progress.is_some() && files_to_prepare > 0 => {
        emit_push_scan_heartbeat(
          relationship.name.as_str(),
          progress,
          push_scan_preparation_summary(
            scan_mode_label,
            prepared_files_completed,
            files_to_prepare,
            total_entries,
            start.elapsed().as_millis() as u64,
          ),
          start.elapsed().as_millis() as u64,
          progress_percent(processed_entries, total_entries),
        );
        prep_heartbeat
          .as_mut()
          .reset(tokio::time::Instant::now() + PUSH_SCAN_HEARTBEAT_INTERVAL);
        continue;
      }
    };

    macro_rules! complete_prepared_entry {
      () => {
        processed_entries += 1;
        prepared_files_completed += 1;
        if should_emit_push_scan_progress(processed_entries) {
          flush_push_batch(
            &remote_client,
            &blob_config,
            &metadata_store,
            &relationship.id,
            &mut pending_batch,
            &mut pending_metadata_updates,
            progress,
            processed_entries,
            total_entries,
            &mut files_pushed,
            &mut files_failed,
            &mut total_bytes,
            &mut errors,
            &mut metadata_by_path,
          )
          .await?;
          emit_push_scan_progress(
            relationship.name.as_str(),
            progress,
            processed_entries,
            total_entries,
            files_pushed,
            files_skipped,
            files_failed,
            files_deleted,
            total_bytes,
            start.elapsed().as_millis() as u64,
          );
        }
      };
    }

    let prepared = match prepared_result {
      Ok(prepared) => prepared,
      Err(message) => {
        tracing::warn!("{}", message);
        errors.push(message);
        files_failed += 1;
        complete_prepared_entry!();
        continue;
      }
    };

    let candidate = prepared.file;
    let stored_meta = prepared.stored_meta;
    let migration_old_remote_path = prepared.migration_old_remote_path;
    let remote_path = candidate.remote_path.clone();
    let content_hash = candidate.content_hash.clone();
    let file_size = candidate.file_size;
    let mtime = candidate.modified_at;

    // Hash skip: content unchanged, just update mtime in metadata.
    if let Some(ref meta) = stored_meta {
      if meta.content_hash == content_hash {
        if !needs_hash_skip_metadata_update(meta, file_size, mtime) {
          files_skipped += 1;
          complete_prepared_entry!();
          continue;
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        let updated_meta = FileSyncMeta {
          path: remote_path.clone(),
          content_hash: content_hash.clone(),
          size: file_size,
          modified_at: mtime,
          sync_status: SyncStatus::Synced,
          last_synced_at: now_ms,
        };

        metadata_by_path.insert(remote_path.clone(), updated_meta);
        pending_metadata_updates.push(
          metadata_by_path
            .get(&remote_path)
            .expect("updated hash-skip metadata should exist")
            .clone(),
        );
        files_skipped += 1;
        complete_prepared_entry!();
        continue;
      }
    }

    // Path-migration move detection. This intentionally does NOT do generic
    // same-hash rename detection. A remote move is only safe when the sync
    // root changed and the old remote path is derived from the recorded old
    // local base for this exact file.
    if stored_meta.is_none() {
      if let Some(old_path) = migration_old_remote_path.as_deref() {
        if let Some(source_meta) = metadata_by_path.get(old_path).cloned() {
          if old_path != remote_path && source_meta.content_hash == content_hash {
            match remote_client
              .remote_path_has_content_hash(
                path_migration
                  .as_ref()
                  .map(|migration| migration.old_remote_path.as_str())
                  .unwrap_or("/"),
                &old_path,
                &content_hash,
              )
              .await
            {
              Ok(true) => match remote_client.rename_file(&old_path, &remote_path).await {
                Ok(()) => {
                  let now_ms = chrono::Utc::now().timestamp_millis();
                  metadata_store.delete_file_meta(&relationship.id, &old_path)?;
                  let new_meta = FileSyncMeta {
                    path: remote_path.clone(),
                    content_hash: content_hash.clone(),
                    size: file_size,
                    modified_at: mtime,
                    sync_status: SyncStatus::Synced,
                    last_synced_at: now_ms,
                  };
                  metadata_store.set_file_meta(&relationship.id, &new_meta)?;
                  metadata_by_path.remove(old_path);
                  metadata_by_path.insert(remote_path.clone(), new_meta);
                  files_pushed += 1;
                  tracing::info!("migrated remote path: {} -> {}", old_path, remote_path);
                  complete_prepared_entry!();
                  continue;
                }
                Err(error) => {
                  let message = format!(
                    "failed to migrate remote {} to {}: {}",
                    old_path, remote_path, error
                  );
                  tracing::warn!("{}", message);
                  errors.push(message);
                  files_failed += 1;
                  complete_prepared_entry!();
                  continue;
                }
              },
              Ok(false) => {
                tracing::debug!(
                  "migration old path {} no longer matches recorded hash; uploading {} instead",
                  old_path,
                  remote_path,
                );
              }
              Err(error) => {
                tracing::warn!(
                  "failed to verify migration source {} before moving to {}; uploading {} normally instead: {}",
                  old_path,
                  remote_path,
                  remote_path,
                  error,
                );
              }
            }
          }
        }
      }

      if path_migration.is_some() && migration_old_remote_path.is_some() {
        match remote_client
          .remote_path_has_content_hash(
            path_migration
              .as_ref()
              .map(|migration| migration.new_remote_path.as_str())
              .unwrap_or("/"),
            &remote_path,
            &content_hash,
          )
          .await
        {
          Ok(true) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if let Some(old_path) = migration_old_remote_path.as_deref() {
              if metadata_by_path.contains_key(old_path) {
                metadata_store.delete_file_meta(&relationship.id, old_path)?;
                metadata_by_path.remove(old_path);
              }
            }
            let new_meta = FileSyncMeta {
              path: remote_path.clone(),
              content_hash: content_hash.clone(),
              size: file_size,
              modified_at: mtime,
              sync_status: SyncStatus::Synced,
              last_synced_at: now_ms,
            };
            metadata_store.set_file_meta(&relationship.id, &new_meta)?;
            metadata_by_path.insert(remote_path.clone(), new_meta);
            files_skipped += 1;
            tracing::info!(
              "migration target already has expected content: {}",
              remote_path
            );
            complete_prepared_entry!();
            continue;
          }
          Ok(false) => {}
          Err(error) => {
            tracing::warn!(
              "failed to verify migration target {} before skipping commit; uploading normally instead: {}",
              remote_path,
              error,
            );
          }
        }
      }
    }

    if pending_batch.should_flush_before(&candidate, &blob_config) {
      flush_push_batch(
        &remote_client,
        &blob_config,
        &metadata_store,
        &relationship.id,
        &mut pending_batch,
        &mut pending_metadata_updates,
        progress,
        processed_entries,
        total_entries,
        &mut files_pushed,
        &mut files_failed,
        &mut total_bytes,
        &mut errors,
        &mut metadata_by_path,
      )
      .await?;
    }

    pending_batch.push(candidate, &blob_config);

    if pending_batch.should_flush_now() {
      flush_push_batch(
        &remote_client,
        &blob_config,
        &metadata_store,
        &relationship.id,
        &mut pending_batch,
        &mut pending_metadata_updates,
        progress,
        processed_entries,
        total_entries,
        &mut files_pushed,
        &mut files_failed,
        &mut total_bytes,
        &mut errors,
        &mut metadata_by_path,
      )
      .await?;
    }

    complete_prepared_entry!();
  }

  flush_push_batch(
    &remote_client,
    &blob_config,
    &metadata_store,
    &relationship.id,
    &mut pending_batch,
    &mut pending_metadata_updates,
    progress,
    processed_entries,
    total_entries,
    &mut files_pushed,
    &mut files_failed,
    &mut total_bytes,
    &mut errors,
    &mut metadata_by_path,
  )
  .await?;

  flush_pending_metadata_updates(
    &metadata_store,
    &relationship.id,
    &mut pending_metadata_updates,
  )?;

  push_pending_symlinks(
    &remote_client,
    &metadata_store,
    &relationship.id,
    relationship.name.as_str(),
    scan_mode_label,
    scan_mode,
    pending_symlinks,
    &mut metadata_by_path,
    queued_for_hashing,
    progress,
    &mut processed_entries,
    total_entries,
    &mut files_pushed,
    &mut files_skipped,
    &mut files_failed,
    files_deleted,
    total_bytes,
    &mut errors,
    start,
  )
  .await?;

  emit_push_scan_progress(
    relationship.name.as_str(),
    progress,
    processed_entries,
    total_entries,
    files_pushed,
    files_skipped,
    files_failed,
    files_deleted,
    total_bytes,
    start.elapsed().as_millis() as u64,
  );

  if let Some(migration) = path_migration.as_ref().filter(|_| errors.is_empty()) {
    cleanup_unseen_migration_metadata(
      &metadata_store,
      &relationship.id,
      migration,
      &seen_remote_paths,
      &mut metadata_by_path,
    )?;
  }

  // Detect deleted files: entries in metadata that no longer exist on disk.
  if relationship.delete_propagation.local_to_remote {
    // If a child relationship now owns part of the remote tree, files
    // under those prefixes will be missing from our walk by design —
    // they aren't deletions, so suppress the delete_file call.
    let remote_exclusions =
      crate::sync::hierarchy::child_exclusions(relationship, all_relationships);

    for meta in metadata_by_path.values() {
      if seen_remote_paths.contains(&meta.path) {
        continue;
      }
      if path_migration
        .as_ref()
        .is_some_and(|migration| is_under_remote_base(&meta.path, &migration.old_remote_path))
      {
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

  if path_migration.is_some() && errors.is_empty() {
    metadata_store.clear_path_migration(&relationship.id)?;
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

fn flush_pending_metadata_updates(
  metadata_store: &SyncMetadataStore<'_>,
  relationship_id: &str,
  pending_metadata_updates: &mut Vec<FileSyncMeta>,
) -> Result<()> {
  if pending_metadata_updates.is_empty() {
    return Ok(());
  }

  let metas = coalesce_file_metas(std::mem::take(pending_metadata_updates));
  let started_at = Instant::now();
  metadata_store.set_file_metas_batch(relationship_id, &metas)?;
  log_slow_blob_stage(
    "metadata_deferred_batch_write",
    "push completion batch",
    started_at.elapsed(),
    Some(metas.len() as u64),
    Some(metas.len() as u64),
  );

  Ok(())
}

async fn push_pending_symlinks(
  remote_client: &RemoteClient,
  metadata_store: &SyncMetadataStore<'_>,
  relationship_id: &str,
  relationship_name: &str,
  scan_mode_label: &str,
  scan_mode: PushScanMode,
  pending_symlinks: Vec<PendingSymlink>,
  metadata_by_path: &mut HashMap<String, FileSyncMeta>,
  queued_for_hashing: u64,
  progress: Option<&PushProgressReporter<'_>>,
  processed_entries: &mut u64,
  total_entries: u64,
  files_pushed: &mut u64,
  files_skipped: &mut u64,
  files_failed: &mut u64,
  files_deleted: u64,
  total_bytes: u64,
  errors: &mut Vec<String>,
  start: Instant,
) -> Result<()> {
  let symlinks_to_push = pending_symlinks.len() as u64;
  let symlink_concurrency = push_worker_count(pending_symlinks.len());
  if symlink_concurrency > 1 {
    tracing::info!(
      "push {} scan for '{}': syncing {} symlinks with {} workers after regular files",
      scan_mode_label,
      relationship_name,
      pending_symlinks.len(),
      symlink_concurrency,
    );
  }

  let mut symlink_pushes = stream::iter(pending_symlinks.into_iter().map(|symlink| {
    let client = remote_client.clone();
    async move {
      let remote_path = symlink.remote_path.clone();
      let target = symlink.target.clone();

      if scan_mode == PushScanMode::Lite {
        match client.remote_symlink_target(&remote_path).await {
          Ok(Some(remote_target))
            if symlink_identity_hash(&remote_path, &remote_target) == symlink.content_hash =>
          {
            return Ok(PendingSymlinkOutcome::AlreadyPresent(symlink));
          }
          Ok(_) => {}
          Err(error) => {
            tracing::debug!(
              "failed to inspect remote symlink {} before push; falling back to create/update: {}",
              remote_path,
              error,
            );
          }
        }
      }

      client
        .create_symlink(&remote_path, &target)
        .await
        .map(|_| PendingSymlinkOutcome::Pushed(symlink))
        .map_err(|error| (remote_path, error))
    }
  }))
  .buffer_unordered(symlink_concurrency.max(1));

  let mut symlinks_processed = 0_u64;
  let mut symlink_metadata_updates = Vec::new();
  let mut symlink_heartbeat = Box::pin(tokio::time::sleep(PUSH_SCAN_HEARTBEAT_INTERVAL));
  loop {
    let symlink_result = tokio::select! {
      result = symlink_pushes.next() => {
        match result {
          Some(result) => result,
          None => break,
        }
      }
      _ = &mut symlink_heartbeat, if progress.is_some() && symlinks_to_push > 0 => {
        emit_push_scan_heartbeat(
          relationship_name,
          progress,
          push_scan_symlink_summary(
            scan_mode_label,
            symlinks_processed,
            symlinks_to_push,
            queued_for_hashing,
            start.elapsed().as_millis() as u64,
          ),
          start.elapsed().as_millis() as u64,
          progress_percent(*processed_entries, total_entries),
        );
        symlink_heartbeat
          .as_mut()
          .reset(tokio::time::Instant::now() + PUSH_SCAN_HEARTBEAT_INTERVAL);
        continue;
      }
    };

    symlinks_processed += 1;
    *processed_entries += 1;

    match symlink_result {
      Ok(PendingSymlinkOutcome::Pushed(symlink)) => {
        let meta = synced_symlink_meta(&symlink);
        metadata_by_path.insert(symlink.remote_path.clone(), meta.clone());
        symlink_metadata_updates.push(meta);
        *files_pushed += 1;
        tracing::debug!(
          "pushed symlink: {} -> {}",
          symlink.remote_path,
          symlink.target
        );
      }
      Ok(PendingSymlinkOutcome::AlreadyPresent(symlink)) => {
        let meta = synced_symlink_meta(&symlink);
        metadata_by_path.insert(symlink.remote_path.clone(), meta.clone());
        symlink_metadata_updates.push(meta);
        *files_skipped += 1;
        tracing::debug!(
          "remote symlink already current: {} -> {}",
          symlink.remote_path,
          symlink.target
        );
      }
      Err((remote_path, error)) => {
        let message = format!("failed to push symlink {}: {}", remote_path, error);
        tracing::warn!("{}", message);
        errors.push(message);
        *files_failed += 1;
      }
    }

    if should_emit_push_scan_progress(*processed_entries) {
      emit_push_scan_progress(
        relationship_name,
        progress,
        *processed_entries,
        total_entries,
        *files_pushed,
        *files_skipped,
        *files_failed,
        files_deleted,
        total_bytes,
        start.elapsed().as_millis() as u64,
      );
    }
  }

  flush_pending_metadata_updates(
    metadata_store,
    relationship_id,
    &mut symlink_metadata_updates,
  )?;

  Ok(())
}

fn synced_symlink_meta(symlink: &PendingSymlink) -> FileSyncMeta {
  let now_ms = chrono::Utc::now().timestamp_millis();
  FileSyncMeta {
    path: symlink.remote_path.clone(),
    content_hash: symlink.content_hash.clone(),
    size: 0,
    modified_at: symlink.modified_at,
    sync_status: SyncStatus::Synced,
    last_synced_at: now_ms,
  }
}

fn coalesce_file_metas(metas: Vec<FileSyncMeta>) -> Vec<FileSyncMeta> {
  let mut latest_by_path: HashMap<String, FileSyncMeta> = HashMap::new();

  for meta in metas {
    latest_by_path.insert(meta.path.clone(), meta);
  }

  latest_by_path.into_values().collect()
}

fn migration_old_remote_path_for_entry(
  entry_path: &Path,
  migration: &SyncPathMigration,
) -> Option<String> {
  let old_local_base = Path::new(&migration.old_local_path);
  if let Ok(relative) = entry_path.strip_prefix(old_local_base) {
    return Some(compute_remote_path(relative, &migration.old_remote_path));
  }

  let new_local_base = Path::new(&migration.new_local_path);
  let relative_to_new = entry_path.strip_prefix(new_local_base).ok()?;
  let old_leaf = old_local_base.file_name()?;
  let relative_to_old = relative_to_new.strip_prefix(Path::new(old_leaf)).ok()?;
  Some(compute_remote_path(
    relative_to_old,
    &migration.old_remote_path,
  ))
}

fn cleanup_unseen_migration_metadata(
  metadata_store: &SyncMetadataStore<'_>,
  relationship_id: &str,
  migration: &SyncPathMigration,
  seen_remote_paths: &HashSet<String>,
  metadata_by_path: &mut HashMap<String, FileSyncMeta>,
) -> Result<()> {
  let stale_old_paths: Vec<String> = metadata_by_path
    .keys()
    .filter(|path| {
      is_under_remote_base(path, &migration.old_remote_path) && !seen_remote_paths.contains(*path)
    })
    .cloned()
    .collect();

  for old_path in stale_old_paths {
    metadata_store.delete_file_meta(relationship_id, &old_path)?;
    metadata_by_path.remove(&old_path);
    tracing::info!(
      "dropped stale migration metadata for {} without deleting remote file",
      old_path,
    );
  }

  Ok(())
}

fn is_under_remote_base(path: &str, remote_base: &str) -> bool {
  let base = remote_base.trim_end_matches('/');
  path == base || path.starts_with(&format!("{}/", base))
}

/// Recursively walk a directory, returning all file and symlink paths.
/// Skips directories themselves (the caller handles that). Any directory
/// whose path matches one of `exclusions` is not descended into — used to
/// skip child sync-relationships' local territory so a parent sync doesn't
/// re-push files the child is already handling.
fn walkdir(
  root: &Path,
  exclusions: &[std::path::PathBuf],
  discovered_entries: &AtomicU64,
) -> Result<Vec<std::path::PathBuf>> {
  let mut results = Vec::new();
  walk_recursive(root, exclusions, discovered_entries, &mut results)?;
  Ok(results)
}

fn walk_recursive(
  dir: &Path,
  exclusions: &[std::path::PathBuf],
  discovered_entries: &AtomicU64,
  results: &mut Vec<std::path::PathBuf>,
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
      discovered_entries.fetch_add(1, Ordering::Relaxed);
    } else if file_type.is_dir() {
      if crate::sync::hierarchy::is_local_excluded_by_child(&path, exclusions) {
        continue;
      }
      walk_recursive(&path, exclusions, discovered_entries, results)?;
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

fn can_lite_fast_skip(
  scan_mode: PushScanMode,
  stored_meta: Option<&FileSyncMeta>,
  file_size: u64,
  modified_at: i64,
) -> bool {
  if scan_mode != PushScanMode::Lite {
    return false;
  }

  matches!(
    stored_meta,
    Some(meta)
      if meta.sync_status == SyncStatus::Synced
        && meta.size == file_size
        && meta.modified_at == modified_at
  )
}

fn can_lite_fast_skip_symlink(
  scan_mode: PushScanMode,
  stored_meta: Option<&FileSyncMeta>,
  content_hash: &str,
) -> bool {
  if scan_mode != PushScanMode::Lite {
    return false;
  }

  matches!(
    stored_meta,
    Some(meta)
      if meta.sync_status == SyncStatus::Synced
        && meta.content_hash == content_hash
  )
}

fn needs_hash_skip_metadata_update(meta: &FileSyncMeta, file_size: u64, modified_at: i64) -> bool {
  meta.sync_status != SyncStatus::Synced
    || meta.size != file_size
    || meta.modified_at != modified_at
}

fn metadata_mtime_or_now(metadata: &std::fs::Metadata) -> i64 {
  metadata
    .modified()
    .ok()
    .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
    .map(|duration| duration.as_millis() as i64)
    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
}

fn symlink_identity_hash(remote_path: &str, target: &str) -> String {
  let normalized_target = normalize_remote_path_like_engine(target);
  let mut input = Vec::new();
  input.extend_from_slice(b"symlinkid:");
  input.extend_from_slice(remote_path.as_bytes());
  input.push(0);
  input.extend_from_slice(normalized_target.as_bytes());
  blake3::hash(&input).to_hex().to_string()
}

fn normalize_remote_path_like_engine(path: &str) -> String {
  let path = path.replace('\0', "");
  let trimmed = path.trim();
  if trimmed.is_empty() {
    return "/".to_string();
  }

  let mut segments: Vec<&str> = Vec::new();
  for segment in trimmed.split('/').filter(|segment| !segment.is_empty()) {
    match segment {
      "." => {}
      ".." => {
        segments.pop();
      }
      segment => segments.push(segment),
    }
  }

  if segments.is_empty() {
    "/".to_string()
  } else {
    format!("/{}", segments.join("/"))
  }
}

fn is_unrecoverable_push_error(error: &ClientError) -> bool {
  matches!(
    error,
    ClientError::UpstreamRejected {
      status: 401 | 429,
      ..
    }
  ) || is_unrecoverable_push_error_message(&error.to_string())
}

fn is_transient_push_error(error: &ClientError) -> bool {
  error.is_transient_upstream()
}

fn is_unrecoverable_push_error_message(message: &str) -> bool {
  let lower = message.to_ascii_lowercase();

  lower.contains("401 unauthorized")
    || lower.contains("429 too many requests")
    || lower.contains("invalid or expired token")
}

fn emit_push_scan_progress(
  relationship_name: &str,
  progress: Option<&PushProgressReporter<'_>>,
  processed_entries: u64,
  total_entries: u64,
  files_pushed: u64,
  files_skipped: u64,
  files_failed: u64,
  files_deleted: u64,
  total_bytes: u64,
  elapsed_ms: u64,
) {
  if !should_emit_push_scan_progress(processed_entries) && processed_entries != total_entries {
    return;
  }

  tracing::info!(
    "push progress for '{}': processed={}, pushed={}, skipped={}, failed={}, deleted={}, activity_emitted={}",
    relationship_name,
    processed_entries,
    files_pushed,
    files_skipped,
    files_failed,
    files_deleted,
    progress.is_some(),
  );

  if let Some(progress) = progress {
    progress.emit(
      push_scan_progress_summary(
        processed_entries,
        total_entries,
        files_pushed,
        files_skipped,
        files_failed,
        files_deleted,
        total_bytes,
        elapsed_ms,
      ),
      files_pushed + files_deleted,
      total_bytes,
      elapsed_ms,
      progress_percent(processed_entries, total_entries),
    );
  }
}

fn emit_push_scan_heartbeat(
  relationship_name: &str,
  progress: Option<&PushProgressReporter<'_>>,
  summary: String,
  elapsed_ms: u64,
  progress_percent: Option<f64>,
) {
  tracing::info!(
    "push scan heartbeat for '{}': {}, activity_emitted={}",
    relationship_name,
    summary,
    progress.is_some(),
  );

  if let Some(progress) = progress {
    progress.emit_with_type(
      "scan_heartbeat",
      summary,
      0,
      0,
      elapsed_ms,
      progress_percent,
    );
  }
}

fn should_emit_push_scan_progress(processed_entries: u64) -> bool {
  processed_entries > 0 && processed_entries % 100 == 0
}

fn should_emit_push_scan_heartbeat(last_emit: Instant, now: Instant) -> bool {
  now.duration_since(last_emit) >= PUSH_SCAN_HEARTBEAT_INTERVAL
}

fn push_scan_discovery_summary(
  scan_mode_label: &str,
  discovered_entries: u64,
  local_path: &str,
  elapsed_ms: u64,
) -> String {
  format!(
    "{} scan phase 1/3: discovering local entries in {} · found {} so far · {} elapsed",
    scan_mode_label,
    local_path,
    crate::sync::activity::format_count(discovered_entries),
    crate::sync::activity::format_duration(elapsed_ms),
  )
}

fn push_scan_inspection_summary(
  scan_mode_label: &str,
  inspected_entries: u64,
  total_entries: u64,
  queued_for_hashing: u64,
  elapsed_ms: u64,
) -> String {
  format!(
    "{} scan phase 1/3: inspecting filesystem entries · entry scan {} of {} ({}) · {} queued for hashing/checking · {} elapsed",
    scan_mode_label,
    crate::sync::activity::format_count(inspected_entries),
    crate::sync::activity::format_count(total_entries),
    format_progress_percent(inspected_entries, total_entries),
    crate::sync::activity::format_count(queued_for_hashing),
    crate::sync::activity::format_duration(elapsed_ms),
  )
}

fn push_scan_preparation_summary(
  scan_mode_label: &str,
  prepared_files_completed: u64,
  files_to_prepare: u64,
  total_entries: u64,
  elapsed_ms: u64,
) -> String {
  format!(
    "{} scan phase 2/3: processing queued files · queued-file work {} of {} ({}) · from {} inspected entries · hashing, uploading, and committing as needed · {} elapsed",
    scan_mode_label,
    crate::sync::activity::format_count(prepared_files_completed),
    crate::sync::activity::format_count(files_to_prepare),
    format_progress_percent(prepared_files_completed, files_to_prepare),
    crate::sync::activity::format_count(total_entries),
    crate::sync::activity::format_duration(elapsed_ms),
  )
}

fn push_scan_symlink_summary(
  scan_mode_label: &str,
  symlinks_processed: u64,
  symlinks_to_push: u64,
  queued_for_hashing: u64,
  elapsed_ms: u64,
) -> String {
  format!(
    "{} scan phase 3/3: syncing symlinks · symlink work {} of {} ({}) · {} files queued for hashing/checking · {} elapsed",
    scan_mode_label,
    crate::sync::activity::format_count(symlinks_processed),
    crate::sync::activity::format_count(symlinks_to_push),
    format_progress_percent(symlinks_processed, symlinks_to_push),
    crate::sync::activity::format_count(queued_for_hashing),
    crate::sync::activity::format_duration(elapsed_ms),
  )
}

fn format_progress_percent(done: u64, total: u64) -> String {
  if total == 0 {
    return "100%".to_string();
  }

  let percent = (done as f64 / total as f64 * 100.0).clamp(0.0, 100.0);
  if done > 0 && percent < 1.0 {
    "<1%".to_string()
  } else {
    format!("{:.0}%", percent)
  }
}

fn push_scan_progress_summary(
  processed_entries: u64,
  total_entries: u64,
  files_pushed: u64,
  files_skipped: u64,
  files_failed: u64,
  files_deleted: u64,
  total_bytes: u64,
  elapsed_ms: u64,
) -> String {
  let percent = if total_entries == 0 {
    100.0
  } else {
    (processed_entries as f64 / total_entries as f64 * 100.0).clamp(0.0, 100.0)
  };
  let mut parts = vec![format!(
    "Uploading {} of {} entries ({:.0}%)",
    crate::sync::activity::format_count(processed_entries),
    crate::sync::activity::format_count(total_entries),
    percent,
  )];
  if files_pushed > 0 {
    parts.push(format!(
      "{} committed",
      crate::sync::activity::format_count(files_pushed)
    ));
    parts.push(format!(
      "totaling {}",
      crate::sync::activity::format_bytes(total_bytes)
    ));
  }
  if files_skipped > 0 {
    parts.push(format!(
      "{} unchanged",
      crate::sync::activity::format_count(files_skipped)
    ));
  }
  if files_deleted > 0 {
    parts.push(format!(
      "{} deleted",
      crate::sync::activity::format_count(files_deleted)
    ));
  }
  if files_failed > 0 {
    parts.push(format!(
      "{} failed",
      crate::sync::activity::format_count(files_failed)
    ));
  }
  if let Some(eta) = estimate_remaining(processed_entries, total_entries, elapsed_ms) {
    parts.push(format!("~{} remaining", eta));
  }
  parts.join(" · ")
}

fn estimate_remaining(
  processed_entries: u64,
  total_entries: u64,
  elapsed_ms: u64,
) -> Option<String> {
  if processed_entries == 0 || processed_entries >= total_entries || elapsed_ms == 0 {
    return None;
  }
  let remaining_entries = total_entries - processed_entries;
  let estimated_ms =
    (elapsed_ms as f64 / processed_entries as f64 * remaining_entries as f64) as u64;
  Some(crate::sync::activity::format_duration(estimated_ms))
}

impl PushScanMode {
  fn label(self) -> &'static str {
    match self {
      PushScanMode::Lite => "Lite",
      PushScanMode::Full => "Full",
    }
  }
}

#[cfg(test)]
mod tests {
  use crate::sync::metadata::{FileSyncMeta, SyncPathMigration, SyncStatus};

  fn pending_push_file(
    dir: &tempfile::TempDir,
    name: &str,
    remote_path: &str,
    content: &[u8],
    modified_at: i64,
  ) -> super::PendingPushFile {
    let local_path = dir.path().join(name);
    std::fs::write(&local_path, content).expect("failed to write pending test file");

    super::PendingPushFile {
      local_path,
      remote_path: remote_path.to_string(),
      content_hash: blake3::hash(content).to_hex().to_string(),
      file_size: content.len() as u64,
      modified_at,
      content_type: None,
    }
  }

  fn synced_meta(size: u64, modified_at: i64) -> FileSyncMeta {
    FileSyncMeta {
      path: "/docs/a.txt".to_string(),
      content_hash: "hash-a".to_string(),
      size,
      modified_at,
      sync_status: SyncStatus::Synced,
      last_synced_at: 1,
    }
  }

  #[test]
  fn lite_scan_fast_skips_when_size_mtime_and_status_match() {
    let meta = synced_meta(42, 1000);

    assert!(super::can_lite_fast_skip(
      super::PushScanMode::Lite,
      Some(&meta),
      42,
      1000,
    ));
  }

  #[test]
  fn lite_scan_hashes_when_size_changes_even_if_mtime_matches() {
    let meta = synced_meta(42, 1000);

    assert!(!super::can_lite_fast_skip(
      super::PushScanMode::Lite,
      Some(&meta),
      43,
      1000,
    ));
  }

  #[test]
  fn lite_scan_hashes_when_mtime_changes_even_if_size_matches() {
    let meta = synced_meta(42, 1000);

    assert!(!super::can_lite_fast_skip(
      super::PushScanMode::Lite,
      Some(&meta),
      42,
      1001,
    ));
  }

  #[test]
  fn lite_scan_hashes_when_metadata_is_absent_or_not_synced() {
    let mut meta = synced_meta(42, 1000);
    meta.sync_status = SyncStatus::PendingPush;

    assert!(!super::can_lite_fast_skip(
      super::PushScanMode::Lite,
      None,
      42,
      1000,
    ));
    assert!(!super::can_lite_fast_skip(
      super::PushScanMode::Lite,
      Some(&meta),
      42,
      1000,
    ));
  }

  #[test]
  fn full_scan_never_uses_metadata_fast_skip() {
    let meta = synced_meta(42, 1000);

    assert!(!super::can_lite_fast_skip(
      super::PushScanMode::Full,
      Some(&meta),
      42,
      1000,
    ));
  }

  #[test]
  fn symlink_identity_hash_matches_engine_identity_shape() {
    let mut input = Vec::new();
    input.extend_from_slice(b"symlinkid:");
    input.extend_from_slice(b"/docs/link");
    input.push(0);
    input.extend_from_slice(b"/target/file.txt");
    let expected = blake3::hash(&input).to_hex().to_string();

    assert_eq!(
      super::symlink_identity_hash("/docs/link", "../target/file.txt"),
      expected,
    );
  }

  #[test]
  fn lite_scan_fast_skips_synced_symlink_when_identity_hash_matches() {
    let content_hash = super::symlink_identity_hash("/docs/link", "/target/file.txt");
    let meta = FileSyncMeta {
      path: "/docs/link".to_string(),
      content_hash: content_hash.clone(),
      size: 0,
      modified_at: 1000,
      sync_status: SyncStatus::Synced,
      last_synced_at: 1,
    };

    assert!(super::can_lite_fast_skip_symlink(
      super::PushScanMode::Lite,
      Some(&meta),
      &content_hash,
    ));
  }

  #[test]
  fn symlink_fast_skip_requires_lite_synced_and_matching_hash() {
    let content_hash = super::symlink_identity_hash("/docs/link", "/target/file.txt");
    let mut meta = FileSyncMeta {
      path: "/docs/link".to_string(),
      content_hash: content_hash.clone(),
      size: 0,
      modified_at: 1000,
      sync_status: SyncStatus::Synced,
      last_synced_at: 1,
    };

    assert!(!super::can_lite_fast_skip_symlink(
      super::PushScanMode::Full,
      Some(&meta),
      &content_hash,
    ));
    assert!(!super::can_lite_fast_skip_symlink(
      super::PushScanMode::Lite,
      Some(&meta),
      "different-hash",
    ));

    meta.sync_status = SyncStatus::PendingPush;
    assert!(!super::can_lite_fast_skip_symlink(
      super::PushScanMode::Lite,
      Some(&meta),
      &content_hash,
    ));
  }

  #[test]
  fn hash_skip_metadata_update_is_needed_only_when_metadata_changes() {
    let meta = synced_meta(42, 1000);

    assert!(!super::needs_hash_skip_metadata_update(&meta, 42, 1000));
    assert!(super::needs_hash_skip_metadata_update(&meta, 43, 1000));
    assert!(super::needs_hash_skip_metadata_update(&meta, 42, 1001));

    let mut pending = synced_meta(42, 1000);
    pending.sync_status = SyncStatus::PendingPush;
    assert!(super::needs_hash_skip_metadata_update(&pending, 42, 1000));
  }

  #[test]
  fn coalesced_metadata_updates_keep_only_latest_entry_per_path() {
    let first = synced_meta(42, 1000);
    let mut latest_same_path = synced_meta(43, 2000);
    latest_same_path.content_hash = "hash-new".to_string();

    let mut other_path = synced_meta(7, 3000);
    other_path.path = "/docs/b.txt".to_string();
    other_path.content_hash = "hash-b".to_string();

    let coalesced =
      super::coalesce_file_metas(vec![first, other_path.clone(), latest_same_path.clone()]);

    assert_eq!(coalesced.len(), 2);
    assert!(
      coalesced
        .iter()
        .any(|meta| meta.path == latest_same_path.path
          && meta.content_hash == latest_same_path.content_hash
          && meta.modified_at == latest_same_path.modified_at),
      "latest duplicate-path metadata should win",
    );
    assert!(
      coalesced
        .iter()
        .any(|meta| meta.path == other_path.path && meta.content_hash == other_path.content_hash),
      "distinct paths should be preserved",
    );
  }

  #[test]
  fn migration_old_remote_path_maps_expanded_root_child_back_to_old_base() {
    let migration = SyncPathMigration {
      relationship_id: "rel-1".to_string(),
      old_remote_path: "/workspaces/wyatt/Pictures/".to_string(),
      new_remote_path: "/workspaces/wyatt/".to_string(),
      old_local_path: "/home/wyatt/Pictures".to_string(),
      new_local_path: "/media/Data/Remote/Seafile/wyatt-desktop".to_string(),
      created_at: 1,
    };

    let old_path = super::migration_old_remote_path_for_entry(
      std::path::Path::new("/media/Data/Remote/Seafile/wyatt-desktop/Pictures/Me/photo.jpg"),
      &migration,
    );

    assert_eq!(
      old_path.as_deref(),
      Some("/workspaces/wyatt/Pictures/Me/photo.jpg"),
      "expanding the local sync root to a parent should still map child files to their previous remote path",
    );
  }

  #[test]
  fn push_parallelism_limit_stays_in_sane_range() {
    assert!((3..=5).contains(&super::PUSH_PARALLELISM_LIMIT));
    assert_eq!(super::push_worker_count(0), 0);
    assert!(super::push_worker_count(1) <= 1);
    assert!(super::push_worker_count(1_000) <= super::PUSH_PARALLELISM_LIMIT);
  }

  #[test]
  fn scan_progress_emits_only_at_hundred_entry_boundaries() {
    assert!(!super::should_emit_push_scan_progress(0));
    assert!(!super::should_emit_push_scan_progress(99));
    assert!(super::should_emit_push_scan_progress(100));
    assert!(!super::should_emit_push_scan_progress(101));
    assert!(super::should_emit_push_scan_progress(500));
  }

  #[test]
  fn scan_heartbeat_emits_only_after_interval() {
    let start = std::time::Instant::now();

    assert!(!super::should_emit_push_scan_heartbeat(
      start,
      start + super::PUSH_SCAN_HEARTBEAT_INTERVAL - std::time::Duration::from_millis(1),
    ));
    assert!(super::should_emit_push_scan_heartbeat(
      start,
      start + super::PUSH_SCAN_HEARTBEAT_INTERVAL,
    ));
  }

  #[test]
  fn scan_heartbeat_summaries_are_user_visible() {
    assert_eq!(
      super::push_scan_discovery_summary("Full", 12_345, "/media/Data/Pictures", 31_000),
      "Full scan phase 1/3: discovering local entries in /media/Data/Pictures · found 12,345 so far · 31.0s elapsed",
    );
    assert_eq!(
      super::push_scan_inspection_summary("Full", 10_000, 94_249, 9_900, 61_000),
      "Full scan phase 1/3: inspecting filesystem entries · entry scan 10,000 of 94,249 (11%) · 9,900 queued for hashing/checking · 1m 01s elapsed",
    );
    assert_eq!(
      super::push_scan_preparation_summary("Full", 6, 76_834, 94_249, 91_000),
      "Full scan phase 2/3: processing queued files · queued-file work 6 of 76,834 (<1%) · from 94,249 inspected entries · hashing, uploading, and committing as needed · 1m 31s elapsed",
    );
    assert_eq!(
      super::push_scan_symlink_summary("Full", 40, 100, 32_970, 121_000),
      "Full scan phase 3/3: syncing symlinks · symlink work 40 of 100 (40%) · 32,970 files queued for hashing/checking · 2m 01s elapsed",
    );
  }

  #[test]
  fn scan_progress_summary_uses_post_classification_counters() {
    assert_eq!(
      super::push_scan_progress_summary(500, 1_000, 0, 500, 0, 0, 0, 12_000),
      "Uploading 500 of 1,000 entries (50%) · 500 unchanged · ~12.0s remaining",
    );
  }

  #[test]
  fn scan_progress_summary_includes_upload_rate_and_eta() {
    assert_eq!(
      super::push_scan_progress_summary(1_400, 4_953, 1_152, 248, 0, 0, 573_571_072, 360_000,),
      "Uploading 1,400 of 4,953 entries (28%) · 1,152 committed · totaling 547 MB · 248 unchanged · ~15m 13s remaining",
    );
  }

  #[test]
  fn batch_summary_separates_files_from_reused_chunks() {
    let summary = super::push_batch_summary(super::PushBatchUploadStats {
      strategy: super::PushBatchUploadStrategy::ChunkedBlob,
      files: 10,
      total_file_bytes: 13_170_000,
      checked_chunks: 93,
      uploaded_chunks: 0,
      uploaded_bytes: 0,
      check_duration_ms: 12,
      upload_duration_ms: 0,
      commit_duration_ms: 1_200,
      duration_ms: 6_300,
    });

    assert_eq!(
      summary,
      "Checked 93 chunks · committed 10 files (totaling 12.56 MB) in 6.3s · no chunk upload needed · 93 chunks reused (12.56 MB saved)",
    );
  }

  #[test]
  fn batch_summary_reports_only_missing_chunks_as_uploaded() {
    let summary = super::push_batch_summary(super::PushBatchUploadStats {
      strategy: super::PushBatchUploadStrategy::ChunkedBlob,
      files: 3,
      total_file_bytes: 10_485_760,
      checked_chunks: 40,
      uploaded_chunks: 5,
      uploaded_bytes: 1_310_720,
      check_duration_ms: 50,
      upload_duration_ms: 900,
      commit_duration_ms: 500,
      duration_ms: 2_000,
    });

    assert_eq!(
      summary,
      "Checked 40 chunks · committed 3 files (totaling 10.00 MB) in 2.0s · 1.25 MB sent · 640 KB/s · 5 missing chunks uploaded · 35 chunks reused (8.75 MB saved)",
    );
  }

  #[test]
  fn batch_summary_calls_out_slow_commit_phase() {
    let summary = super::push_batch_summary(super::PushBatchUploadStats {
      strategy: super::PushBatchUploadStrategy::ChunkedBlob,
      files: 1,
      total_file_bytes: 1_007_395_334,
      checked_chunks: 3_843,
      uploaded_chunks: 3_843,
      uploaded_bytes: 1_007_395_334,
      check_duration_ms: 120,
      upload_duration_ms: 600_000,
      commit_duration_ms: 66_621,
      duration_ms: 666_741,
    });

    assert!(
      summary.contains("chunk upload took 10m 00s"),
      "summary should expose slow chunk upload phase: {summary}",
    );
    assert!(
      summary.contains("commit took 1m 06s"),
      "summary should expose slow DB materialization phase: {summary}",
    );
  }

  #[test]
  fn batch_summary_reports_direct_file_uploads_without_chunk_reuse_claims() {
    let summary = super::push_batch_summary(super::PushBatchUploadStats {
      strategy: super::PushBatchUploadStrategy::DirectFilePut,
      files: 1,
      total_file_bytes: 4_526_174_208,
      checked_chunks: 0,
      uploaded_chunks: 0,
      uploaded_bytes: 4_526_174_208,
      check_duration_ms: 0,
      upload_duration_ms: 122_000,
      commit_duration_ms: 0,
      duration_ms: 122_000,
    });

    assert_eq!(
      summary,
      "Streamed 1 file (totaling 4.22 GB) in 2m 02s · 4.22 GB sent · 35.4 MB/s · direct file upload used for oversized blob manifest",
    );
  }

  #[test]
  fn blob_commit_manifest_guard_tracks_server_manifest_limit() {
    assert_eq!(
      super::BLOB_COMMIT_REQUEST_BODY_LIMIT_BYTES,
      32 * 1024 * 1024,
      "client guard should track the server's /blobs/check and /blobs/commit manifest limit",
    );
    assert!(
      super::BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES > 1024 * 1024,
      "the old 1 MiB /blobs/commit limit should no longer force direct uploads",
    );
    assert!(
      super::BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES < super::BLOB_COMMIT_REQUEST_BODY_LIMIT_BYTES,
      "keep a margin below the hard manifest limit for estimate drift",
    );
  }

  #[test]
  fn commit_start_summary_distinguishes_upload_from_commit() {
    assert_eq!(
      super::push_batch_commit_start_summary(1, 1_007_395_334, 3_843, 3_843, 1_007_395_334),
      "Committing 1 file from 3,843 chunks checked · totaling 960.73 MB · 3,843 missing chunks uploaded · 961 MB sent",
    );
    assert_eq!(
      super::push_batch_commit_start_summary(10, 13_170_000, 93, 0, 0),
      "Committing 10 files from 93 chunks checked · totaling 12.56 MB · all chunks already present",
    );
  }

  #[test]
  fn auth_failures_abort_push_cycle() {
    assert!(super::is_unrecoverable_push_error_message(
      "failed to upload /workspaces/wyatt/Pictures/a.png: server error: blob_check returned HTTP 401 Unauthorized",
    ));
    assert!(super::is_unrecoverable_push_error_message(
      "server error: token exchange returned HTTP 429 Too Many Requests for http://files.taraani.org",
    ));
    assert!(super::is_unrecoverable_push_error_message(
      "server error: blob_check returned HTTP 401 Unauthorized: {\"error\":\"Invalid or expired token\"}",
    ));
  }

  #[test]
  fn per_path_permission_failure_does_not_abort_push_cycle() {
    assert!(!super::is_unrecoverable_push_error_message(
      "failed to upload /shared/report.pdf: server error: upload_chunk abc returned HTTP 403 Forbidden",
    ));
  }

  #[test]
  fn ordinary_file_errors_do_not_abort_push_cycle() {
    assert!(!super::is_unrecoverable_push_error_message(
      "failed to read \"/tmp/file\": permission denied",
    ));
    assert!(!super::is_unrecoverable_push_error_message(
      "failed to upload /Pictures/a.png: server error: upload_chunk abc returned HTTP 500 Internal Server Error",
    ));
  }

  #[test]
  fn push_batch_flushes_before_configured_limits() {
    let config = super::BlobConfig {
      hash_algorithm: "blake3".to_string(),
      chunk_size: 4,
      chunk_hash_prefix: "chunk:".to_string(),
    };
    let mut batch = super::PushBatch::default();
    let first = super::PendingPushFile {
      local_path: std::path::PathBuf::from("/tmp/a.txt"),
      remote_path: "/a.txt".to_string(),
      content_hash: "hash-a".to_string(),
      file_size: 4,
      modified_at: 1,
      content_type: Some("text/plain".to_string()),
    };
    let large = super::PendingPushFile {
      local_path: std::path::PathBuf::from("/tmp/b.txt"),
      remote_path: "/b.txt".to_string(),
      content_hash: "hash-b".to_string(),
      file_size: super::PUSH_BATCH_MAX_BYTES as u64,
      modified_at: 2,
      content_type: None,
    };

    assert!(!batch.should_flush_before(&first, &config));
    batch.push(first, &config);
    assert!(batch.should_flush_before(&large, &config));
  }

  #[test]
  fn push_batch_flushes_before_observed_commit_timeout_cliff() {
    let config = super::BlobConfig {
      hash_algorithm: "blake3".to_string(),
      chunk_size: 1024,
      chunk_hash_prefix: "chunk:".to_string(),
    };
    let mut batch = super::PushBatch::default();

    for index in 0..super::PUSH_BATCH_MAX_FILES {
      batch.push(
        super::PendingPushFile {
          local_path: std::path::PathBuf::from(format!("/tmp/file-{}.txt", index)),
          remote_path: format!("/file-{}.txt", index),
          content_hash: format!("hash-{}", index),
          file_size: 9,
          modified_at: index as i64,
          content_type: Some("text/plain".to_string()),
        },
        &config,
      );
    }

    let next = super::PendingPushFile {
      local_path: std::path::PathBuf::from("/tmp/next.txt"),
      remote_path: "/next.txt".to_string(),
      content_hash: "hash-next".to_string(),
      file_size: 4,
      modified_at: 999,
      content_type: Some("text/plain".to_string()),
    };

    assert_eq!(
      super::PUSH_BATCH_MAX_FILES,
      32,
      "live Taraani logs show 94-100 file commits hit the 30s timeout and trigger per-file fallback",
    );
    assert!(batch.should_flush_before(&next, &config));
    assert!(batch.should_flush_now());
  }

  #[test]
  fn push_batch_plan_dedupes_shared_chunks_and_commits_many_files() {
    let config = super::BlobConfig {
      hash_algorithm: "blake3".to_string(),
      chunk_size: 4,
      chunk_hash_prefix: "chunk:".to_string(),
    };
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let files = vec![
      pending_push_file(&temp_dir, "a.bin", "/a.bin", b"aaaabbbb", 1),
      pending_push_file(&temp_dir, "b.bin", "/b.bin", b"aaaacccc", 2),
    ];

    let plan = super::plan_push_batch(&files, &config).expect("batch plan should succeed");

    assert_eq!(plan.commit_files.len(), 2);
    assert_eq!(plan.commit_files[0].chunks.len(), 2);
    assert_eq!(plan.commit_files[1].chunks.len(), 2);
    assert_eq!(
      plan.unique_hashes.len(),
      3,
      "shared chunk should be checked once"
    );
    assert_eq!(
      plan.upload_chunks.len(),
      3,
      "shared chunk should be uploadable once"
    );
    let shared_hash = crate::remote::chunk_hash("chunk:", b"aaaa");
    let shared_ref = plan
      .upload_chunks
      .get(&shared_hash)
      .expect("shared chunk ref should exist");
    assert_eq!(shared_ref.offset, 0);
    assert_eq!(shared_ref.len, 4);
  }

  #[test]
  fn pending_chunk_read_rejects_file_mutation_before_upload() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let local_path = temp_dir.path().join("chunk.bin");
    std::fs::write(&local_path, b"aaaa").expect("failed to write test chunk");
    let hash = crate::remote::chunk_hash("chunk:", b"aaaa");
    let chunk = super::PendingChunkRef {
      hash,
      local_path: local_path.clone(),
      offset: 0,
      len: 4,
    };

    std::fs::write(&local_path, b"zzzz").expect("failed to mutate test chunk");

    let error = super::read_pending_chunk(chunk, "chunk:".to_string())
      .expect_err("changed chunk bytes should be rejected");
    assert!(
      error
        .to_string()
        .contains("file changed while reading chunk"),
      "unexpected error: {error}",
    );
  }
}

struct PushFilePrepRequest {
  entry_path: PathBuf,
  remote_path: String,
  modified_at: i64,
  content_type: Option<String>,
  stored_meta: Option<FileSyncMeta>,
  migration_old_remote_path: Option<String>,
}

struct PendingSymlink {
  remote_path: String,
  target: String,
  content_hash: String,
  modified_at: i64,
}

enum PendingSymlinkOutcome {
  Pushed(PendingSymlink),
  AlreadyPresent(PendingSymlink),
}

struct PreparedPushFile {
  file: PendingPushFile,
  stored_meta: Option<FileSyncMeta>,
  migration_old_remote_path: Option<String>,
}

fn prepare_push_file(
  request: PushFilePrepRequest,
) -> std::result::Result<PreparedPushFile, String> {
  let (content_hash, file_size) = hash_file_content(&request.entry_path)?;

  Ok(PreparedPushFile {
    file: PendingPushFile {
      local_path: request.entry_path,
      remote_path: request.remote_path,
      content_hash,
      file_size,
      modified_at: request.modified_at,
      content_type: request.content_type,
    },
    stored_meta: request.stored_meta,
    migration_old_remote_path: request.migration_old_remote_path,
  })
}

fn hash_file_content(path: &Path) -> std::result::Result<(String, u64), String> {
  let mut file =
    std::fs::File::open(path).map_err(|error| format!("failed to open {:?}: {}", path, error))?;
  let mut hasher = blake3::Hasher::new();
  let mut buffer = vec![0_u8; PUSH_FILE_READ_BUFFER_BYTES];
  let mut total_bytes = 0_u64;

  loop {
    let bytes_read = file
      .read(&mut buffer)
      .map_err(|error| format!("failed to read {:?}: {}", path, error))?;
    if bytes_read == 0 {
      break;
    }
    hasher.update(&buffer[..bytes_read]);
    total_bytes = total_bytes.saturating_add(bytes_read as u64);
  }

  Ok((hasher.finalize().to_hex().to_string(), total_bytes))
}

fn push_worker_count(item_count: usize) -> usize {
  if item_count == 0 {
    return 0;
  }

  std::thread::available_parallelism()
    .map(|parallelism| parallelism.get())
    .unwrap_or(PUSH_PARALLELISM_LIMIT)
    .min(PUSH_PARALLELISM_LIMIT)
    .min(item_count)
    .max(1)
}

#[derive(Clone)]
struct PendingPushFile {
  local_path: PathBuf,
  remote_path: String,
  content_hash: String,
  file_size: u64,
  modified_at: i64,
  content_type: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct PushBatchUploadStats {
  strategy: PushBatchUploadStrategy,
  files: u64,
  total_file_bytes: u64,
  checked_chunks: u64,
  uploaded_chunks: u64,
  uploaded_bytes: u64,
  check_duration_ms: u64,
  upload_duration_ms: u64,
  commit_duration_ms: u64,
  duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushBatchUploadStrategy {
  ChunkedBlob,
  DirectFilePut,
}

#[derive(Default)]
struct PushBatch {
  files: Vec<PendingPushFile>,
  bytes: u64,
  chunks: usize,
}

impl PushBatch {
  fn should_flush_before(&self, file: &PendingPushFile, config: &BlobConfig) -> bool {
    if self.files.is_empty() {
      return false;
    }

    let file_chunks = estimated_chunk_count(file.file_size, config.chunk_size);
    self.files.len() + 1 > PUSH_BATCH_MAX_FILES
      || self.bytes.saturating_add(file.file_size) > PUSH_BATCH_MAX_BYTES as u64
      || self.chunks.saturating_add(file_chunks) > PUSH_BATCH_MAX_CHUNKS
  }

  fn should_flush_now(&self) -> bool {
    self.files.len() >= PUSH_BATCH_MAX_FILES
      || self.bytes >= PUSH_BATCH_MAX_BYTES as u64
      || self.chunks >= PUSH_BATCH_MAX_CHUNKS
  }

  fn push(&mut self, file: PendingPushFile, config: &BlobConfig) {
    self.bytes = self.bytes.saturating_add(file.file_size);
    self.chunks = self
      .chunks
      .saturating_add(estimated_chunk_count(file.file_size, config.chunk_size));
    self.files.push(file);
  }
}

fn estimated_chunk_count(byte_len: u64, chunk_size: usize) -> usize {
  if byte_len == 0 {
    0
  } else {
    byte_len
      .div_ceil(chunk_size.max(1) as u64)
      .min(usize::MAX as u64) as usize
  }
}

fn estimated_blob_commit_body_bytes(files: &[PendingPushFile], config: &BlobConfig) -> usize {
  let mut bytes = "{\"files\":[]}".len();
  for file in files {
    bytes = bytes.saturating_add(estimated_blob_commit_file_bytes(file, config));
  }
  bytes
}

fn estimated_blob_commit_file_bytes(file: &PendingPushFile, config: &BlobConfig) -> usize {
  let chunks = estimated_chunk_count(file.file_size, config.chunk_size);
  let chunk_array_bytes = if chunks == 0 {
    2
  } else {
    // 64 hex chars plus JSON quotes plus a comma/closing bracket allowance.
    chunks.saturating_mul(67).saturating_add(2)
  };
  let path_bytes = serde_json::to_string(&file.remote_path)
    .map(|path| path.len())
    .unwrap_or_else(|_| file.remote_path.len().saturating_add(2));
  let hash_bytes = file.content_hash.len().saturating_add(2);
  let content_type_bytes = file
    .content_type
    .as_ref()
    .and_then(|content_type| serde_json::to_string(content_type).ok())
    .map(|content_type| content_type.len())
    .unwrap_or(0);

  // Field names, punctuation, size digits, and a safety margin. This only
  // decides whether to avoid the chunked protocol before building the exact
  // manifest, so overestimation is acceptable.
  160_usize
    .saturating_add(path_bytes)
    .saturating_add(chunk_array_bytes)
    .saturating_add(hash_bytes)
    .saturating_add(file.file_size.to_string().len())
    .saturating_add(content_type_bytes)
}

fn blob_commit_body_bytes(files: &[CommitFile]) -> usize {
  serde_json::to_vec(&serde_json::json!({ "files": files }))
    .map(|payload| payload.len())
    .unwrap_or(usize::MAX)
}

struct PushBatchPlan {
  commit_files: Vec<CommitFile>,
  unique_hashes: Vec<String>,
  upload_chunks: HashMap<String, PendingChunkRef>,
}

#[derive(Clone)]
struct PendingChunkRef {
  hash: String,
  local_path: PathBuf,
  offset: u64,
  len: usize,
}

fn plan_push_batch(
  files: &[PendingPushFile],
  config: &BlobConfig,
) -> std::result::Result<PushBatchPlan, String> {
  let worker_count = push_worker_count(files.len());
  if worker_count <= 1 {
    return Ok(merge_partial_push_batch_plans(vec![plan_push_batch_range(
      0, files, config,
    )?]));
  }

  let files_per_worker = files.len().div_ceil(worker_count);
  let partials = std::thread::scope(|scope| {
    let mut handles = Vec::new();
    for (chunk_index, file_chunk) in files.chunks(files_per_worker).enumerate() {
      let start_index = chunk_index * files_per_worker;
      handles.push(scope.spawn(move || plan_push_batch_range(start_index, file_chunk, config)));
    }

    handles
      .into_iter()
      .map(|handle| {
        handle
          .join()
          .expect("push batch planning worker should not panic")
      })
      .collect::<std::result::Result<Vec<_>, _>>()
  });

  Ok(merge_partial_push_batch_plans(partials?))
}

struct PartialPushBatchPlan {
  indexed_commit_files: Vec<(usize, CommitFile)>,
  unique_hashes: Vec<String>,
  upload_chunks: HashMap<String, PendingChunkRef>,
}

fn plan_push_batch_range(
  start_index: usize,
  files: &[PendingPushFile],
  config: &BlobConfig,
) -> std::result::Result<PartialPushBatchPlan, String> {
  let mut indexed_commit_files = Vec::with_capacity(files.len());
  let mut upload_chunks: HashMap<String, PendingChunkRef> = HashMap::new();
  let mut unique_hashes = Vec::new();

  for (offset, file) in files.iter().enumerate() {
    let planned_file = plan_file_chunks(file, config)?;
    for chunk_ref in planned_file.chunk_refs {
      let hash = chunk_ref.hash.clone();
      if !upload_chunks.contains_key(&hash) {
        unique_hashes.push(hash.clone());
        upload_chunks.insert(hash, chunk_ref);
      }
    }

    indexed_commit_files.push((
      start_index + offset,
      CommitFile {
        path: file.remote_path.clone(),
        chunks: planned_file.chunk_hashes,
        content_hash: Some(file.content_hash.clone()),
        size: Some(file.file_size),
        content_type: file.content_type.clone(),
      },
    ));
  }

  Ok(PartialPushBatchPlan {
    indexed_commit_files,
    unique_hashes,
    upload_chunks,
  })
}

struct PlannedFileChunks {
  chunk_hashes: Vec<String>,
  chunk_refs: Vec<PendingChunkRef>,
}

fn plan_file_chunks(
  file: &PendingPushFile,
  config: &BlobConfig,
) -> std::result::Result<PlannedFileChunks, String> {
  let mut handle = std::fs::File::open(&file.local_path)
    .map_err(|error| format!("failed to open {:?}: {}", file.local_path, error))?;
  let chunk_size = config.chunk_size.max(1);
  let mut buffer = vec![0_u8; chunk_size];
  let mut offset = 0_u64;
  let mut chunk_hashes = Vec::new();
  let mut chunk_refs = Vec::new();
  let mut content_hasher = blake3::Hasher::new();

  loop {
    let bytes_read = handle
      .read(&mut buffer)
      .map_err(|error| format!("failed to read {:?}: {}", file.local_path, error))?;
    if bytes_read == 0 {
      break;
    }

    let chunk = &buffer[..bytes_read];
    let hash = chunk_hash(&config.chunk_hash_prefix, chunk);
    content_hasher.update(chunk);
    chunk_hashes.push(hash.clone());
    chunk_refs.push(PendingChunkRef {
      hash,
      local_path: file.local_path.clone(),
      offset,
      len: bytes_read,
    });
    offset = offset.saturating_add(bytes_read as u64);
  }

  let planned_hash = content_hasher.finalize().to_hex().to_string();
  if offset != file.file_size || planned_hash != file.content_hash {
    return Err(format!(
      "file changed while preparing upload for {:?}: expected size/hash {}:{}, got {}:{}",
      file.local_path, file.file_size, file.content_hash, offset, planned_hash,
    ));
  }

  Ok(PlannedFileChunks {
    chunk_hashes,
    chunk_refs,
  })
}

fn merge_partial_push_batch_plans(partials: Vec<PartialPushBatchPlan>) -> PushBatchPlan {
  let mut indexed_commit_files = Vec::new();
  let mut upload_chunks: HashMap<String, PendingChunkRef> = HashMap::new();
  let mut unique_hashes = Vec::new();

  for mut partial in partials {
    indexed_commit_files.append(&mut partial.indexed_commit_files);
    for hash in partial.unique_hashes {
      if upload_chunks.contains_key(&hash) {
        continue;
      }
      if let Some(chunk_ref) = partial.upload_chunks.remove(&hash) {
        unique_hashes.push(hash.clone());
        upload_chunks.insert(hash, chunk_ref);
      }
    }
  }

  indexed_commit_files.sort_by_key(|(index, _)| *index);
  let commit_files = indexed_commit_files
    .into_iter()
    .map(|(_, commit_file)| commit_file)
    .collect();

  PushBatchPlan {
    commit_files,
    unique_hashes,
    upload_chunks,
  }
}

async fn flush_push_batch(
  client: &RemoteClient,
  config: &BlobConfig,
  metadata_store: &SyncMetadataStore<'_>,
  relationship_id: &str,
  batch: &mut PushBatch,
  pending_metadata_updates: &mut Vec<FileSyncMeta>,
  progress: Option<&PushProgressReporter<'_>>,
  processed_entries: u64,
  total_entries: u64,
  files_pushed: &mut u64,
  files_failed: &mut u64,
  total_bytes: &mut u64,
  errors: &mut Vec<String>,
  metadata_by_path: &mut HashMap<String, FileSyncMeta>,
) -> Result<()> {
  if batch.files.is_empty() {
    return Ok(());
  }

  let mut files = std::mem::take(&mut batch.files);
  batch.bytes = 0;
  batch.chunks = 0;

  let batch_result = push_batch_via_chunks(
    client,
    config,
    &files,
    progress,
    processed_entries,
    total_entries,
  )
  .await;

  match batch_result {
    Ok(stats) => {
      let metas: Vec<FileSyncMeta> = files.iter().map(make_file_meta).collect();

      for meta in metas {
        let path = meta.path.clone();
        metadata_by_path.insert(path.clone(), meta);
        pending_metadata_updates.push(
          metadata_by_path
            .get(&path)
            .expect("queued push metadata should exist")
            .clone(),
        );
      }

      for file in files.drain(..) {
        *files_pushed += 1;
        *total_bytes += file.file_size;
        tracing::debug!(
          "pushed file: {} ({} bytes)",
          file.remote_path,
          file.file_size
        );
      }
      if let Some(progress) = progress {
        progress.emit(
          push_batch_summary(stats),
          stats.files,
          stats.uploaded_bytes,
          stats.duration_ms,
          progress_percent(processed_entries, total_entries),
        );
      }
      flush_pending_metadata_updates(metadata_store, relationship_id, pending_metadata_updates)?;
      Ok(())
    }
    Err(error) => {
      if is_transient_push_error(&error) {
        tracing::warn!(
          "batched push deferred for {} files because the remote is temporarily unavailable: {}",
          files.len(),
          error,
        );
        return Err(error);
      }

      tracing::warn!(
        "batched push failed for {} files; falling back to per-file isolation: {}",
        files.len(),
        error,
      );
      if is_unrecoverable_push_error(&error) {
        return Err(ClientError::Server(format!(
          "push aborted after unrecoverable remote auth/rate-limit error: {}",
          error,
        )));
      }

      let mut successful_files: Vec<PendingPushFile> = Vec::new();
      let mut successful_metas: Vec<FileSyncMeta> = Vec::new();

      for file in files {
        let push_outcome = push_batch_via_chunks(
          client,
          config,
          std::slice::from_ref(&file),
          progress,
          processed_entries,
          total_entries,
        )
        .await;

        match push_outcome {
          Ok(_stats) => {
            successful_metas.push(make_file_meta(&file));
            successful_files.push(file);
          }
          Err(error) => {
            record_push_failure(
              metadata_store,
              relationship_id,
              &file,
              error,
              files_failed,
              errors,
            )?;
          }
        }
      }

      if !successful_metas.is_empty() {
        pending_metadata_updates.extend(successful_metas.iter().cloned());
      }

      for meta in successful_metas {
        metadata_by_path.insert(meta.path.clone(), meta);
      }

      for file in successful_files {
        *files_pushed += 1;
        *total_bytes += file.file_size;
        tracing::debug!(
          "pushed file: {} ({} bytes)",
          file.remote_path,
          file.file_size
        );
      }

      flush_pending_metadata_updates(metadata_store, relationship_id, pending_metadata_updates)?;

      Ok(())
    }
  }
}

async fn push_batch_via_chunks(
  client: &RemoteClient,
  config: &BlobConfig,
  files: &[PendingPushFile],
  progress: Option<&PushProgressReporter<'_>>,
  processed_entries: u64,
  total_entries: u64,
) -> Result<PushBatchUploadStats> {
  if files.is_empty() {
    return Ok(PushBatchUploadStats {
      strategy: PushBatchUploadStrategy::ChunkedBlob,
      files: 0,
      total_file_bytes: 0,
      checked_chunks: 0,
      uploaded_chunks: 0,
      uploaded_bytes: 0,
      check_duration_ms: 0,
      upload_duration_ms: 0,
      commit_duration_ms: 0,
      duration_ms: 0,
    });
  }

  let batch_started = Instant::now();
  let total_file_bytes = files.iter().map(|file| file.file_size).sum();
  let estimated_commit_bytes = estimated_blob_commit_body_bytes(files, config);
  if estimated_commit_bytes > BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES {
    tracing::warn!(
      "push batch using direct file upload because estimated blob commit body is {} bytes, above safe limit {} bytes (server limit {} bytes)",
      estimated_commit_bytes,
      BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES,
      BLOB_COMMIT_REQUEST_BODY_LIMIT_BYTES,
    );
    return push_batch_via_direct_file_put(
      client,
      files,
      progress,
      processed_entries,
      total_entries,
      estimated_commit_bytes,
    )
    .await;
  }

  let files_for_plan = files.to_vec();
  let config_for_plan = config.clone();
  let plan =
    tokio::task::spawn_blocking(move || plan_push_batch(&files_for_plan, &config_for_plan))
      .await
      .map_err(|error| {
        ClientError::Io(std::io::Error::new(
          std::io::ErrorKind::Other,
          format!("push batch planning task panicked: {}", error),
        ))
      })?
      .map_err(|error| ClientError::Io(std::io::Error::new(std::io::ErrorKind::Other, error)))?;
  let commit_body_bytes = blob_commit_body_bytes(&plan.commit_files);
  if commit_body_bytes > BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES {
    tracing::warn!(
      "push batch using direct file upload because blob commit body is {} bytes, above safe limit {} bytes (server limit {} bytes)",
      commit_body_bytes,
      BLOB_COMMIT_SAFE_REQUEST_BODY_BYTES,
      BLOB_COMMIT_REQUEST_BODY_LIMIT_BYTES,
    );
    return push_batch_via_direct_file_put(
      client,
      files,
      progress,
      processed_entries,
      total_entries,
      commit_body_bytes,
    )
    .await;
  }

  let mut check_duration_ms = 0_u64;
  let mut needed_set: HashSet<String> = if plan.unique_hashes.is_empty() {
    HashSet::new()
  } else {
    let started_at = Instant::now();
    let response = client.blob_check(&plan.unique_hashes).await;
    log_slow_blob_stage(
      "blob_check_batch",
      "batch",
      started_at.elapsed(),
      Some(plan.unique_hashes.len() as u64),
      response
        .as_ref()
        .ok()
        .map(|result| result.needed.len() as u64),
    );
    check_duration_ms = started_at.elapsed().as_millis() as u64;
    let response = response?;
    response.needed.into_iter().collect()
  };

  let chunks_to_upload = needed_set.len() as u64;
  let chunks: Vec<PendingChunkRef> = plan
    .upload_chunks
    .into_iter()
    .filter_map(|(hash, chunk_ref)| needed_set.remove(&hash).then_some(chunk_ref))
    .collect();
  let bytes_to_upload: u64 = chunks.iter().map(|chunk| chunk.len as u64).sum();
  let mut chunks_uploaded = 0_u64;
  let mut bytes_uploaded = 0_u64;
  let upload_started = Instant::now();

  let hash_prefix = config.chunk_hash_prefix.clone();
  let mut uploads = stream::iter(chunks.into_iter().map(|chunk_ref| {
    let client = client.clone();
    let hash_prefix = hash_prefix.clone();
    async move {
      let hash = chunk_ref.hash.clone();
      let bytes = tokio::task::spawn_blocking(move || read_pending_chunk(chunk_ref, hash_prefix))
        .await
        .map_err(|error| {
          ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("push chunk read task panicked: {}", error),
          ))
        })??;
      let byte_count = bytes.len() as u64;
      let started_at = Instant::now();
      let result = client.upload_chunk(&hash, bytes).await;
      log_slow_blob_stage(
        "upload_chunk_batch",
        &hash,
        started_at.elapsed(),
        None,
        None,
      );
      result.map(|_| (hash, byte_count))
    }
  }))
  .buffer_unordered(PUSH_CHUNK_UPLOAD_CONCURRENCY);

  while let Some(result) = uploads.next().await {
    let (hash, byte_count) = result?;
    chunks_uploaded += 1;
    bytes_uploaded += byte_count;
    if chunks_uploaded == 1 || chunks_uploaded % 100 == 0 || chunks_uploaded == chunks_to_upload {
      tracing::info!(
        "push batch chunk progress: uploaded {}/{} needed chunks",
        chunks_uploaded,
        chunks_to_upload,
      );
      if let Some(progress) = progress {
        progress.emit(
          format!(
            "Uploading batch chunks: {} of {} · {} sent",
            crate::sync::activity::format_count(chunks_uploaded),
            crate::sync::activity::format_count(chunks_to_upload),
            crate::sync::activity::format_bytes(bytes_uploaded),
          ),
          0,
          bytes_uploaded,
          batch_started.elapsed().as_millis() as u64,
          chunk_progress_percent(
            processed_entries,
            total_entries,
            chunks_uploaded,
            chunks_to_upload,
          ),
        );
      }
    } else {
      tracing::debug!("uploaded batch chunk {}", hash);
    }
  }
  let upload_duration_ms = upload_started.elapsed().as_millis() as u64;

  if let Some(progress) = progress {
    progress.emit(
      push_batch_commit_start_summary(
        files.len() as u64,
        total_file_bytes,
        plan.unique_hashes.len() as u64,
        chunks_uploaded,
        bytes_uploaded,
      ),
      0,
      bytes_uploaded,
      batch_started.elapsed().as_millis() as u64,
      progress_percent(processed_entries, total_entries),
    );
  }
  let started_at = Instant::now();
  let commit_result = client.blob_commit(&plan.commit_files).await;
  let commit_duration_ms = started_at.elapsed().as_millis() as u64;
  log_slow_blob_commit_stage(
    "blob_commit_batch",
    "batch",
    Duration::from_millis(commit_duration_ms),
    files.len() as u64,
    plan.unique_hashes.len() as u64,
    total_file_bytes,
  );
  commit_result?;

  tracing::info!(
    "push batch committed: files={}, checked_chunks={}, uploaded_chunks={}, uploaded_bytes={}, check_ms={}, upload_ms={}, commit_ms={}",
    files.len(),
    plan.unique_hashes.len(),
    chunks_uploaded,
    bytes_uploaded,
    check_duration_ms,
    upload_duration_ms,
    commit_duration_ms,
  );

  Ok(PushBatchUploadStats {
    strategy: PushBatchUploadStrategy::ChunkedBlob,
    files: files.len() as u64,
    total_file_bytes,
    checked_chunks: plan.unique_hashes.len() as u64,
    uploaded_chunks: chunks_uploaded,
    uploaded_bytes: bytes_to_upload,
    check_duration_ms,
    upload_duration_ms,
    commit_duration_ms,
    duration_ms: batch_started.elapsed().as_millis() as u64,
  })
}

async fn push_batch_via_direct_file_put(
  client: &RemoteClient,
  files: &[PendingPushFile],
  progress: Option<&PushProgressReporter<'_>>,
  processed_entries: u64,
  total_entries: u64,
  commit_body_bytes: usize,
) -> Result<PushBatchUploadStats> {
  let batch_started = Instant::now();
  let upload_started = Instant::now();
  let total_file_bytes: u64 = files.iter().map(|file| file.file_size).sum();
  let mut uploaded_bytes = 0_u64;

  for (index, file) in files.iter().enumerate() {
    let file_started = Instant::now();
    let local_file = tokio::fs::File::open(&file.local_path).await?;
    let file_bytes_sent = Arc::new(AtomicU64::new(0));
    let stream_bytes_sent = Arc::clone(&file_bytes_sent);
    let stream = ReaderStream::new(local_file).map_ok(move |bytes| {
      stream_bytes_sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
      bytes
    });
    let body = reqwest::Body::wrap_stream(stream);

    if let Some(progress) = progress {
      progress.emit(
        format!(
          "Directly uploading {} · {} · blob commit manifest would be {}",
          file.remote_path,
          crate::sync::activity::format_bytes_precise(file.file_size),
          crate::sync::activity::format_bytes(commit_body_bytes as u64),
        ),
        0,
        uploaded_bytes,
        batch_started.elapsed().as_millis() as u64,
        direct_upload_progress_percent(
          processed_entries,
          total_entries,
          files.len(),
          index,
          0,
          file.file_size,
        ),
      );
    }

    let upload = client.upload_file(&file.remote_path, body, file.content_type.as_deref());
    tokio::pin!(upload);
    let mut heartbeat = tokio::time::interval_at(
      tokio::time::Instant::now() + PUSH_SCAN_HEARTBEAT_INTERVAL,
      PUSH_SCAN_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
      tokio::select! {
        result = &mut upload => {
          result?;
          break;
        }
        _ = heartbeat.tick() => {
          let sent_for_file = file_bytes_sent.load(Ordering::Relaxed).min(file.file_size);
          let total_sent = uploaded_bytes.saturating_add(sent_for_file);
          tracing::info!(
            "direct file upload progress: {} sent {}/{}",
            file.remote_path,
            sent_for_file,
            file.file_size,
          );
          if let Some(progress) = progress {
            progress.emit(
              format!(
                "Directly uploading {} · {} of {} sent",
                file.remote_path,
                crate::sync::activity::format_bytes(sent_for_file),
                crate::sync::activity::format_bytes_precise(file.file_size),
              ),
              0,
              total_sent,
              batch_started.elapsed().as_millis() as u64,
              direct_upload_progress_percent(
                processed_entries,
                total_entries,
                files.len(),
                index,
                sent_for_file,
                file.file_size,
              ),
            );
          }
        }
      }
    }

    uploaded_bytes = uploaded_bytes.saturating_add(file.file_size);
    tracing::info!(
      "direct file upload completed: {} bytes={}, duration_ms={}",
      file.remote_path,
      file.file_size,
      file_started.elapsed().as_millis(),
    );
  }

  let upload_duration_ms = upload_started.elapsed().as_millis() as u64;
  tracing::info!(
    "push batch streamed via direct file upload: files={}, uploaded_bytes={}, upload_ms={}, commit_body_bytes={}",
    files.len(),
    uploaded_bytes,
    upload_duration_ms,
    commit_body_bytes,
  );

  Ok(PushBatchUploadStats {
    strategy: PushBatchUploadStrategy::DirectFilePut,
    files: files.len() as u64,
    total_file_bytes,
    checked_chunks: 0,
    uploaded_chunks: 0,
    uploaded_bytes,
    check_duration_ms: 0,
    upload_duration_ms,
    commit_duration_ms: 0,
    duration_ms: batch_started.elapsed().as_millis() as u64,
  })
}

fn read_pending_chunk(chunk: PendingChunkRef, hash_prefix: String) -> Result<Vec<u8>> {
  let mut file = std::fs::File::open(&chunk.local_path)?;
  file.seek(SeekFrom::Start(chunk.offset))?;
  let mut bytes = vec![0_u8; chunk.len];
  file.read_exact(&mut bytes)?;

  let actual_hash = chunk_hash(&hash_prefix, &bytes);
  if actual_hash != chunk.hash {
    return Err(ClientError::Io(std::io::Error::new(
      std::io::ErrorKind::Other,
      format!(
        "file changed while reading chunk {:?} at offset {}: expected {}, got {}",
        chunk.local_path, chunk.offset, chunk.hash, actual_hash,
      ),
    )));
  }

  Ok(bytes)
}

fn push_batch_commit_start_summary(
  files: u64,
  total_file_bytes: u64,
  checked_chunks: u64,
  uploaded_chunks: u64,
  uploaded_bytes: u64,
) -> String {
  let mut parts = vec![format!(
    "Committing {} from {} checked",
    crate::sync::activity::pluralize(files, "file"),
    crate::sync::activity::pluralize(checked_chunks, "chunk"),
  )];

  parts.push(format!(
    "totaling {}",
    crate::sync::activity::format_bytes_precise(total_file_bytes),
  ));

  if uploaded_chunks > 0 {
    parts.push(format!(
      "{} uploaded",
      crate::sync::activity::pluralize(uploaded_chunks, "missing chunk"),
    ));
    parts.push(format!(
      "{} sent",
      crate::sync::activity::format_bytes(uploaded_bytes),
    ));
  } else {
    parts.push("all chunks already present".to_string());
  }

  parts.join(" · ")
}

fn push_batch_summary(stats: PushBatchUploadStats) -> String {
  if stats.strategy == PushBatchUploadStrategy::DirectFilePut {
    let mut parts = vec![format!(
      "Streamed {} (totaling {}) in {}",
      crate::sync::activity::pluralize(stats.files, "file"),
      crate::sync::activity::format_bytes_precise(stats.total_file_bytes),
      crate::sync::activity::format_duration(stats.duration_ms),
    )];
    parts.push(format!(
      "{} sent",
      crate::sync::activity::format_bytes_precise(stats.uploaded_bytes),
    ));
    if let Some(rate) = crate::sync::activity::format_rate(stats.uploaded_bytes, stats.duration_ms)
    {
      parts.push(rate);
    }
    parts.push("direct file upload used for oversized blob manifest".to_string());
    return parts.join(" · ");
  }

  let reused_chunks = stats.checked_chunks.saturating_sub(stats.uploaded_chunks);
  let saved_bytes = stats.total_file_bytes.saturating_sub(stats.uploaded_bytes);
  let mut parts = vec![format!(
    "Checked {} · committed {} (totaling {}) in {}",
    crate::sync::activity::pluralize(stats.checked_chunks, "chunk"),
    crate::sync::activity::pluralize(stats.files, "file"),
    crate::sync::activity::format_bytes_precise(stats.total_file_bytes),
    crate::sync::activity::format_duration(stats.duration_ms),
  )];

  if stats.uploaded_chunks > 0 {
    parts.push(format!(
      "{} sent",
      crate::sync::activity::format_bytes(stats.uploaded_bytes),
    ));
    if let Some(rate) = crate::sync::activity::format_rate(stats.uploaded_bytes, stats.duration_ms)
    {
      parts.push(rate);
    }
    parts.push(format!(
      "{} missing chunks uploaded",
      crate::sync::activity::format_count(stats.uploaded_chunks),
    ));
  } else {
    parts.push("no chunk upload needed".to_string());
  }

  if reused_chunks > 0 {
    parts.push(format!(
      "{} chunks reused ({} saved)",
      crate::sync::activity::format_count(reused_chunks),
      crate::sync::activity::format_bytes_precise(saved_bytes),
    ));
  }
  if stats.check_duration_ms >= 2_000 {
    parts.push(format!(
      "check took {}",
      crate::sync::activity::format_duration(stats.check_duration_ms),
    ));
  }
  if stats.uploaded_chunks > 0 && stats.upload_duration_ms >= 2_000 {
    parts.push(format!(
      "chunk upload took {}",
      crate::sync::activity::format_duration(stats.upload_duration_ms),
    ));
  }
  if stats.commit_duration_ms >= 2_000 {
    parts.push(format!(
      "commit took {}",
      crate::sync::activity::format_duration(stats.commit_duration_ms),
    ));
  }

  parts.join(" · ")
}

fn make_file_meta(file: &PendingPushFile) -> FileSyncMeta {
  let now_ms = chrono::Utc::now().timestamp_millis();
  FileSyncMeta {
    path: file.remote_path.clone(),
    content_hash: file.content_hash.clone(),
    size: file.file_size,
    modified_at: file.modified_at,
    sync_status: SyncStatus::Synced,
    last_synced_at: now_ms,
  }
}

fn record_push_failure(
  metadata_store: &SyncMetadataStore<'_>,
  relationship_id: &str,
  file: &PendingPushFile,
  error: ClientError,
  files_failed: &mut u64,
  errors: &mut Vec<String>,
) -> Result<()> {
  if is_transient_push_error(&error) {
    tracing::warn!(
      "push deferred while uploading {}; remote is temporarily unavailable: {}",
      file.remote_path,
      error,
    );
    return Err(error);
  }

  if is_unrecoverable_push_error(&error) {
    return Err(ClientError::Server(format!(
      "push aborted after unrecoverable remote auth/rate-limit error at {}: {}",
      file.remote_path, error,
    )));
  }

  let message = format!("failed to upload {}: {}", file.remote_path, error);
  let is_forbidden = message.contains("403 Forbidden")
    || matches!(error, ClientError::UpstreamRejected { status: 403, .. });
  tracing::warn!("{}", message);
  errors.push(message);
  *files_failed += 1;

  if is_forbidden {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let suppressed_meta = FileSyncMeta {
      path: file.remote_path.clone(),
      content_hash: file.content_hash.clone(),
      size: file.file_size,
      modified_at: file.modified_at,
      sync_status: SyncStatus::Synced,
      last_synced_at: now_ms,
    };
    let _ = metadata_store.set_file_meta(relationship_id, &suppressed_meta);
  }

  Ok(())
}

fn log_slow_blob_stage(
  stage: &str,
  remote_path: &str,
  elapsed: std::time::Duration,
  count: Option<u64>,
  total: Option<u64>,
) {
  if elapsed < std::time::Duration::from_secs(2) {
    return;
  }

  match (count, total) {
    (Some(count), Some(total)) => tracing::info!(
      "slow push {} for {}: count={}, total={}, duration_ms={}",
      stage,
      remote_path,
      count,
      total,
      elapsed.as_millis(),
    ),
    _ => tracing::info!(
      "slow push {} for {}: duration_ms={}",
      stage,
      remote_path,
      elapsed.as_millis(),
    ),
  }
}

fn log_slow_blob_commit_stage(
  stage: &str,
  remote_path: &str,
  elapsed: std::time::Duration,
  files: u64,
  chunks: u64,
  file_bytes: u64,
) {
  if elapsed < std::time::Duration::from_secs(2) {
    return;
  }

  tracing::info!(
    "slow push {} for {}: files={}, chunks={}, file_bytes={}, duration_ms={}",
    stage,
    remote_path,
    files,
    chunks,
    file_bytes,
    elapsed.as_millis(),
  );
}

fn progress_percent(processed_entries: u64, total_entries: u64) -> Option<f64> {
  if total_entries == 0 {
    return Some(100.0);
  }

  Some(((processed_entries as f64 / total_entries as f64) * 100.0).clamp(0.0, 100.0))
}

fn chunk_progress_percent(
  processed_entries: u64,
  total_entries: u64,
  chunks_uploaded: u64,
  chunks_to_upload: u64,
) -> Option<f64> {
  if total_entries == 0 {
    return Some(100.0);
  }

  if chunks_to_upload == 0 {
    return progress_percent(processed_entries, total_entries);
  }

  let completed_before_file = processed_entries.saturating_sub(1) as f64;
  let file_fraction = (chunks_uploaded as f64 / chunks_to_upload as f64).clamp(0.0, 1.0);
  Some((((completed_before_file + file_fraction) / total_entries as f64) * 100.0).clamp(0.0, 100.0))
}

fn direct_upload_progress_percent(
  processed_entries: u64,
  total_entries: u64,
  file_count: usize,
  file_index: usize,
  file_bytes_sent: u64,
  file_size: u64,
) -> Option<f64> {
  if total_entries == 0 {
    return Some(100.0);
  }

  let batch_entries = file_count.max(1) as u64;
  let completed_before_batch = processed_entries.saturating_sub(batch_entries) as f64;
  let completed_in_batch = file_index.min(file_count) as f64;
  let file_fraction = if file_size == 0 {
    1.0
  } else {
    (file_bytes_sent as f64 / file_size as f64).clamp(0.0, 1.0)
  };

  Some(
    (((completed_before_batch + completed_in_batch + file_fraction) / total_entries as f64)
      * 100.0)
      .clamp(0.0, 100.0),
  )
}
