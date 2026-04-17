use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aeordb::engine::{DirectoryOps, RequestContext, StorageEngine};

use crate::error::{ClientError, Result};
use crate::sync::filter::matches_filter;

/// Tracks paths that we wrote ourselves, so the filesystem watcher
/// can ignore events caused by our own write-back operations.
#[derive(Clone)]
pub struct WriteSuppressionSet {
  paths: Arc<Mutex<HashSet<PathBuf>>>,
}

impl WriteSuppressionSet {
  pub fn new() -> Self {
    Self {
      paths: Arc::new(Mutex::new(HashSet::new())),
    }
  }

  /// Mark a path as "we're about to write this — suppress watcher events."
  pub fn suppress(&self, path: &Path) {
    self.paths.lock().unwrap().insert(path.to_path_buf());
  }

  /// Check if a path should be suppressed, and remove it from the set.
  /// Returns true if this event should be ignored.
  pub fn should_suppress(&self, path: &Path) -> bool {
    self.paths.lock().unwrap().remove(path)
  }
}

/// Ingest a local filesystem directory into the local aeordb instance.
/// Walks the directory and stores each file via DirectoryOps::store_file().
///
/// This is used for the initial "filesystem → aeordb" sync when a sync
/// relationship is first started.
pub fn ingest_directory(
  engine: &StorageEngine,
  local_path: &str,
  remote_path: &str,
  filter: Option<&str>,
) -> Result<IngestResult> {
  let ops = DirectoryOps::new(engine);
  let ctx = RequestContext::system();
  let local_dir = Path::new(local_path);

  let mut result = IngestResult {
    files_stored: 0,
    files_skipped: 0,
    files_failed: 0,
    symlinks_stored: 0,
    errors: Vec::new(),
  };

  if !local_dir.exists() {
    return Err(ClientError::Configuration(
      format!("local path does not exist: {}", local_path),
    ));
  }

  ingest_recursive(engine, &ops, &ctx, local_dir, local_path, remote_path, filter, &mut result);

  tracing::info!(
    "ingested {}: {} files, {} symlinks, {} skipped, {} failed",
    local_path, result.files_stored, result.symlinks_stored,
    result.files_skipped, result.files_failed,
  );

  Ok(result)
}

fn ingest_recursive(
  engine: &StorageEngine,
  ops: &DirectoryOps<'_>,
  ctx: &RequestContext,
  current_dir: &Path,
  local_base: &str,
  remote_base: &str,
  filter: Option<&str>,
  result: &mut IngestResult,
) {
  let entries = match std::fs::read_dir(current_dir) {
    Ok(entries) => entries,
    Err(error) => {
      result.errors.push(format!("failed to read {:?}: {}", current_dir, error));
      result.files_failed += 1;
      return;
    }
  };

  for entry_result in entries {
    let entry = match entry_result {
      Ok(entry) => entry,
      Err(error) => {
        result.errors.push(format!("failed to read entry: {}", error));
        result.files_failed += 1;
        continue;
      }
    };

    let entry_path = entry.path();

    if entry_path.is_symlink() {
      let filename = entry_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

      if !matches_filter(filename, filter) {
        result.files_skipped += 1;
        continue;
      }

      let relative     = entry_path.strip_prefix(local_base).unwrap_or(&entry_path);
      let remote_file  = format!("{}{}", remote_base, relative.display());

      if let Ok(target) = std::fs::read_link(&entry_path) {
        let target_str = target.to_string_lossy().to_string();
        match ops.store_symlink(ctx, &remote_file, &target_str) {
          Ok(_) => result.symlinks_stored += 1,
          Err(error) => {
            result.errors.push(format!("failed to store symlink {}: {}", remote_file, error));
            result.files_failed += 1;
          }
        }
      }
    } else if entry_path.is_dir() {
      ingest_recursive(engine, ops, ctx, &entry_path, local_base, remote_base, filter, result);
    } else if entry_path.is_file() {
      let filename = entry_path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

      if !matches_filter(filename, filter) {
        result.files_skipped += 1;
        continue;
      }

      let relative     = entry_path.strip_prefix(local_base).unwrap_or(&entry_path);
      let remote_file  = format!("{}{}", remote_base, relative.display());

      match std::fs::read(&entry_path) {
        Ok(bytes) => {
          let content_type = crate::sync::content_type::mime_from_extension(&entry_path);
          match ops.store_file(ctx, &remote_file, &bytes, content_type.as_deref()) {
            Ok(_) => result.files_stored += 1,
            Err(error) => {
              result.errors.push(format!("failed to store {}: {}", remote_file, error));
              result.files_failed += 1;
            }
          }
        }
        Err(error) => {
          result.errors.push(format!("failed to read {:?}: {}", entry_path, error));
          result.files_failed += 1;
        }
      }
    }
  }
}

/// Project files from the local aeordb instance back to the filesystem.
/// Reads files from aeordb at the given remote_path and writes them
/// to the local_path directory.
///
/// Uses the WriteSuppressionSet to prevent the filesystem watcher
/// from re-ingesting files we just wrote.
pub fn project_to_filesystem(
  engine: &StorageEngine,
  local_path: &str,
  remote_path: &str,
  filter: Option<&str>,
  suppression: &WriteSuppressionSet,
) -> Result<ProjectResult> {
  let ops = DirectoryOps::new(engine);
  let local_dir = Path::new(local_path);

  let mut result = ProjectResult {
    files_written: 0,
    files_skipped: 0,
    files_failed:  0,
    symlinks_written: 0,
    errors: Vec::new(),
  };

  // List the remote directory in aeordb
  let entries = ops.list_directory(remote_path)
    .map_err(|error| ClientError::Server(
      format!("failed to list {} in local aeordb: {}", remote_path, error),
    ))?;

  for entry in entries {
    let remote_file_path = format!("{}{}", remote_path, entry.name);
    let local_file_path  = local_dir.join(&entry.name);

    // entry_type: 2 = file, 3 = directory, 8 = symlink
    if entry.entry_type == 3 {
      // Directory — ensure it exists locally and recurse
      if let Err(error) = std::fs::create_dir_all(&local_file_path) {
        result.errors.push(format!("failed to create dir {:?}: {}", local_file_path, error));
        result.files_failed += 1;
        continue;
      }

      let sub_remote = format!("{}/", remote_file_path);
      let sub_local  = local_file_path.to_string_lossy().to_string();

      match project_to_filesystem(engine, &sub_local, &sub_remote, filter, suppression) {
        Ok(sub_result) => {
          result.files_written    += sub_result.files_written;
          result.files_skipped    += sub_result.files_skipped;
          result.files_failed     += sub_result.files_failed;
          result.symlinks_written += sub_result.symlinks_written;
          for error in sub_result.errors {
            result.errors.push(error);
          }
        }
        Err(error) => {
          result.errors.push(format!("failed to project {}: {}", sub_remote, error));
          result.files_failed += 1;
        }
      }
    } else if entry.entry_type == 8 {
      // Symlink
      if !matches_filter(&entry.name, filter) {
        result.files_skipped += 1;
        continue;
      }

      if let Ok(Some(symlink_record)) = ops.get_symlink(&remote_file_path) {
        suppression.suppress(&local_file_path);

        // Remove existing file/symlink
        if local_file_path.exists() || local_file_path.is_symlink() {
          let _ = std::fs::remove_file(&local_file_path);
        }

        #[cfg(unix)]
        {
          let target = symlink_record.target;
          if let Err(error) = std::os::unix::fs::symlink(&target, &local_file_path) {
            result.errors.push(format!("failed to create symlink {:?}: {}", local_file_path, error));
            result.files_failed += 1;
          } else {
            result.symlinks_written += 1;
          }
        }
      }
    } else if entry.entry_type == 2 {
      // File
      if !matches_filter(&entry.name, filter) {
        result.files_skipped += 1;
        continue;
      }

      match ops.read_file(&remote_file_path) {
        Ok(bytes) => {
          // Ensure parent directory exists
          if let Some(parent) = local_file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
          }

          suppression.suppress(&local_file_path);

          match std::fs::write(&local_file_path, &bytes) {
            Ok(()) => result.files_written += 1,
            Err(error) => {
              result.errors.push(format!("failed to write {:?}: {}", local_file_path, error));
              result.files_failed += 1;
            }
          }
        }
        Err(error) => {
          result.errors.push(format!("failed to read {} from aeordb: {}", remote_file_path, error));
          result.files_failed += 1;
        }
      }
    }
  }

  Ok(result)
}

/// Ingest a single file change from the filesystem into local aeordb.
pub fn ingest_single_file(
  engine: &StorageEngine,
  local_path: &Path,
  local_base: &str,
  remote_base: &str,
) -> Result<()> {
  let ops      = DirectoryOps::new(engine);
  let ctx      = RequestContext::system();
  let relative = local_path.strip_prefix(local_base).unwrap_or(local_path);
  let remote   = format!("{}{}", remote_base, relative.display());

  if local_path.is_symlink() {
    let target = std::fs::read_link(local_path)
      .map_err(|error| ClientError::Server(format!("failed to read symlink: {}", error)))?;

    ops.store_symlink(&ctx, &remote, &target.to_string_lossy())
      .map_err(|error| ClientError::Server(format!("failed to store symlink: {}", error)))?;
  } else if local_path.is_file() {
    let bytes        = std::fs::read(local_path)?;
    let content_type = crate::sync::content_type::mime_from_extension(local_path);

    ops.store_file(&ctx, &remote, &bytes, content_type.as_deref())
      .map_err(|error| ClientError::Server(format!("failed to store file: {}", error)))?;
  }

  Ok(())
}

/// Delete a file from local aeordb (when deleted locally).
pub fn delete_from_aeordb(
  engine: &StorageEngine,
  local_path: &Path,
  local_base: &str,
  remote_base: &str,
) -> Result<()> {
  let ops      = DirectoryOps::new(engine);
  let ctx      = RequestContext::system();
  let relative = local_path.strip_prefix(local_base).unwrap_or(local_path);
  let remote   = format!("{}{}", remote_base, relative.display());

  ops.delete_file(&ctx, &remote)
    .map_err(|error| ClientError::Server(format!("failed to delete from aeordb: {}", error)))?;

  Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IngestResult {
  pub files_stored:   u64,
  pub files_skipped:  u64,
  pub files_failed:   u64,
  pub symlinks_stored: u64,
  pub errors:         Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectResult {
  pub files_written:    u64,
  pub files_skipped:    u64,
  pub files_failed:     u64,
  pub symlinks_written: u64,
  pub errors:           Vec<String>,
}
