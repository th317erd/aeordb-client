use std::path::Path;

use chrono::Utc;

use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::remote::RemoteClient;
use crate::state::StateStore;
use crate::sync::engine::{FileState, SyncStatus};
use crate::sync::relationships::RelationshipManager;

/// Execute a one-shot push sync pass for a given relationship.
/// Scans the local directory and uploads files that are new or changed
/// compared to the state tracker.
pub async fn push_sync_pass(
  state: &StateStore,
  relationship_id: &str,
) -> Result<PushSyncResult> {
  let start = std::time::Instant::now();

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

  let connection_manager = ConnectionManager::new(state);
  let connection = connection_manager.get(&relationship.remote_connection_id)?
    .ok_or_else(|| ClientError::Configuration(
      format!("connection not found: {}", relationship.remote_connection_id),
    ))?;

  let remote_client = RemoteClient::from_connection(&connection);

  let mut result = PushSyncResult {
    relationship_id:  relationship_id.to_string(),
    files_uploaded:   0,
    files_skipped:    0,
    files_failed:     0,
    total_bytes:      0,
    duration_ms:      0,
    errors:           Vec::new(),
  };

  // Walk the local directory
  let local_path = Path::new(&relationship.local_path);
  if !local_path.exists() {
    result.errors.push(format!("local path does not exist: {}", relationship.local_path));
    result.duration_ms = start.elapsed().as_millis() as u64;
    return Ok(result);
  }

  let filter = relationship.filter.as_deref();

  push_directory_recursive(
    state,
    &remote_client,
    local_path,
    &relationship.local_path,
    &relationship.remote_path,
    relationship_id,
    filter,
    &mut result,
  ).await;

  result.duration_ms = start.elapsed().as_millis() as u64;

  tracing::info!(
    "push sync for '{}': {} uploaded, {} skipped, {} failed ({} bytes, {}ms)",
    relationship.name,
    result.files_uploaded,
    result.files_skipped,
    result.files_failed,
    result.total_bytes,
    result.duration_ms,
  );

  Ok(result)
}

async fn push_directory_recursive(
  state: &StateStore,
  remote_client: &RemoteClient,
  current_dir: &Path,
  local_base: &str,
  remote_base: &str,
  relationship_id: &str,
  filter: Option<&str>,
  result: &mut PushSyncResult,
) {
  let entries = match std::fs::read_dir(current_dir) {
    Ok(entries) => entries,
    Err(error) => {
      result.errors.push(format!("failed to read directory {:?}: {}", current_dir, error));
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

    if entry_path.is_dir() {
      Box::pin(push_directory_recursive(
        state, remote_client, &entry_path,
        local_base, remote_base, relationship_id, filter, result,
      )).await;
    } else if entry_path.is_file() {
      // Apply filter
      let filename = entry_path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
      if !crate::sync::filter::matches_filter(filename, filter) {
        result.files_skipped += 1;
        continue;
      }
      // Compute the remote path
      let relative = entry_path.strip_prefix(local_base)
        .unwrap_or(&entry_path);
      let remote_file_path = format!("{}{}", remote_base, relative.display());

      // Read the file
      let bytes = match std::fs::read(&entry_path) {
        Ok(bytes) => bytes,
        Err(error) => {
          result.errors.push(format!("failed to read {:?}: {}", entry_path, error));
          result.files_failed += 1;
          continue;
        }
      };

      // Check hash against state tracker
      let content_hash = blake3::hash(&bytes).to_hex().to_string();
      let state_key    = super::engine::file_state_key(relationship_id, &remote_file_path);

      let existing_state: Option<FileState> = state.read_json(&state_key)
        .unwrap_or(None);

      if let Some(ref file_state) = existing_state {
        if file_state.content_hash == content_hash && file_state.sync_status == SyncStatus::Synced {
          result.files_skipped += 1;
          continue;
        }
      }

      // Detect content type from extension
      let content_type = mime_from_extension(&entry_path);

      // Upload
      match remote_client.upload_file(&remote_file_path, bytes.clone(), content_type.as_deref()).await {
        Ok(()) => {
          // Update state tracker
          let file_state = FileState {
            relative_path:      relative.display().to_string(),
            relationship_id:    relationship_id.to_string(),
            content_hash,
            local_modified_at:  entry_path.metadata().ok()
              .and_then(|m| m.modified().ok())
              .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64),
            remote_modified_at: None,
            sync_status:        SyncStatus::Synced,
            last_synced_at:     Utc::now().timestamp_millis(),
          };

          if let Err(error) = state.store_json(&state_key, &file_state) {
            tracing::warn!("failed to update sync state for {}: {}", remote_file_path, error);
          }

          result.total_bytes += bytes.len() as u64;
          result.files_uploaded += 1;

          tracing::debug!("pushed: {} ({} bytes)", remote_file_path, bytes.len());
        }
        Err(error) => {
          result.errors.push(format!("failed to upload {}: {}", remote_file_path, error));
          result.files_failed += 1;
        }
      }
    }
  }
}

fn mime_from_extension(path: &Path) -> Option<String> {
  let extension = path.extension()?.to_str()?;

  let mime = match extension.to_lowercase().as_str() {
    "json"             => "application/json",
    "txt"              => "text/plain",
    "md" | "markdown"  => "text/markdown",
    "html" | "htm"     => "text/html",
    "css"              => "text/css",
    "js" | "mjs"       => "application/javascript",
    "xml"              => "application/xml",
    "csv"              => "text/csv",
    "pdf"              => "application/pdf",
    "png"              => "image/png",
    "jpg" | "jpeg"     => "image/jpeg",
    "gif"              => "image/gif",
    "svg"              => "image/svg+xml",
    "webp"             => "image/webp",
    "zip"              => "application/zip",
    "tar"              => "application/x-tar",
    "gz"               => "application/gzip",
    "yaml" | "yml"     => "application/yaml",
    "toml"             => "application/toml",
    "rs"               => "text/x-rust",
    "py"               => "text/x-python",
    _                  => "application/octet-stream",
  };

  Some(mime.to_string())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PushSyncResult {
  pub relationship_id: String,
  pub files_uploaded:  u64,
  pub files_skipped:   u64,
  pub files_failed:    u64,
  pub total_bytes:     u64,
  pub duration_ms:     u64,
  pub errors:          Vec<String>,
}
