use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};
use crate::remote::RemoteClient;
use crate::state::StateStore;
use crate::sync::conflicts::ConflictManager;
use crate::sync::engine::{FileState, file_state_key};
use crate::sync::filter::matches_filter;
use crate::sync::relationships::RelationshipManager;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReconcileResult {
  pub relationship_id:     String,
  pub files_pulled:        u64,
  pub files_pushed:        u64,
  pub conflicts_detected:  u64,
  pub files_unchanged:     u64,
  pub errors:              Vec<String>,
  pub duration_ms:         u64,
}

/// Reconcile a relationship after coming back online.
/// Compares local filesystem state, state tracker, and remote state
/// to determine what needs to be pulled, pushed, or flagged as a conflict.
pub async fn reconcile(
  state: &StateStore,
  relationship_id: &str,
) -> Result<ReconcileResult> {
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

  let remote_client    = RemoteClient::from_connection(&connection);
  let filter           = relationship.filter.as_deref();
  let conflict_manager = ConflictManager::new(state);

  let mut result = ReconcileResult {
    relationship_id:    relationship_id.to_string(),
    files_pulled:       0,
    files_pushed:       0,
    conflicts_detected: 0,
    files_unchanged:    0,
    errors:             Vec::new(),
    duration_ms:        0,
  };

  // 1. Build a map of what's on the remote
  let remote_entries = match remote_client.list_directory(&relationship.remote_path).await {
    Ok(entries) => entries,
    Err(error) => {
      result.errors.push(format!("failed to list remote: {}", error));
      result.duration_ms = start.elapsed().as_millis() as u64;
      return Ok(result);
    }
  };

  let mut remote_files: HashMap<String, (i64, u64)> = HashMap::new(); // name → (updated_at, size)
  for entry in &remote_entries {
    if entry.is_file() {
      if matches_filter(&entry.name, filter) {
        remote_files.insert(entry.name.clone(), (entry.updated_at, entry.total_size));
      }
    }
  }

  // 2. Build a map of what's on the local filesystem
  let local_path = Path::new(&relationship.local_path);
  let mut local_files: HashMap<String, String> = HashMap::new(); // name → blake3 hash

  if local_path.exists() {
    if let Ok(entries) = std::fs::read_dir(local_path) {
      for entry_result in entries {
        if let Ok(entry) = entry_result {
          let entry_path = entry.path();
          if entry_path.is_file() {
            let filename = entry.file_name().to_string_lossy().to_string();
            if matches_filter(&filename, filter) {
              if let Ok(data) = std::fs::read(&entry_path) {
                let hash = blake3::hash(&data).to_hex().to_string();
                local_files.insert(filename, hash);
              }
            }
          }
        }
      }
    }
  }

  // 3. For each remote file, decide: pull, skip, or conflict
  for (name, (remote_updated_at, _remote_size)) in &remote_files {
    let remote_file_path = format!("{}{}", relationship.remote_path, name);
    let state_key        = file_state_key(relationship_id, &remote_file_path);

    let existing_state: Option<FileState> = state.read_json(&state_key).unwrap_or(None);

    match (&existing_state, local_files.get(name)) {
      // Both sides have the file and state tracker exists
      (Some(tracked), Some(local_hash)) => {
        let local_changed  = *local_hash != tracked.content_hash;
        let remote_changed = tracked.remote_modified_at.map_or(true, |t| t != *remote_updated_at);

        if local_changed && remote_changed {
          // Conflict — both changed
          if !conflict_manager.has_conflict_for(relationship_id, &remote_file_path)? {
            conflict_manager.record_conflict(
              &remote_file_path,
              relationship_id,
              local_hash,
              &format!("remote_updated_{}", remote_updated_at),
              &tracked.content_hash,
              None,
              Some(*remote_updated_at),
            )?;
            result.conflicts_detected += 1;
          }
        } else if remote_changed {
          // Only remote changed — pull
          result.files_pulled += 1;
        } else if local_changed {
          // Only local changed — push (if bidirectional)
          result.files_pushed += 1;
        } else {
          result.files_unchanged += 1;
        }
      }
      // Remote has it, local doesn't, no tracker — new remote file, pull
      (None, None) => {
        result.files_pulled += 1;
      }
      // Remote has it, local doesn't, tracker exists — deleted locally
      (Some(_tracked), None) => {
        // Local was deleted — could be intentional
        result.files_pulled += 1; // Re-download for now
      }
      // Remote has it, local has it, no tracker — first sync
      (None, Some(_local_hash)) => {
        result.files_pulled += 1; // Pull remote version and track
      }
    }
  }

  // 4. Check for local-only files (exist locally but not remotely)
  let remote_names: HashSet<&String> = remote_files.keys().collect();
  for (local_name, _local_hash) in &local_files {
    if !remote_names.contains(local_name) {
      result.files_pushed += 1;
    }
  }

  result.duration_ms = start.elapsed().as_millis() as u64;

  tracing::info!(
    "reconcile for '{}': {} to pull, {} to push, {} conflicts, {} unchanged ({}ms)",
    relationship.name,
    result.files_pulled,
    result.files_pushed,
    result.conflicts_detected,
    result.files_unchanged,
    result.duration_ms,
  );

  Ok(result)
}
