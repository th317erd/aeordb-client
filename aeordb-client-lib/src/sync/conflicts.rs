use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ClientError, Result};
use crate::state::StateStore;

const CONFLICTS_PATH: &str = "/sync/conflicts/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
  KeepLocal,
  KeepRemote,
  KeepBoth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRecord {
  pub id:                String,
  pub file_path:         String,
  pub relationship_id:   String,
  pub local_hash:        String,
  pub remote_hash:       String,
  pub last_synced_hash:  String,
  pub local_modified_at: Option<i64>,
  pub remote_modified_at: Option<i64>,
  pub detected_at:       DateTime<Utc>,
}

/// Manages sync conflicts, persisted in the local state store.
pub struct ConflictManager<'a> {
  state: &'a StateStore,
}

impl<'a> ConflictManager<'a> {
  pub fn new(state: &'a StateStore) -> Self {
    Self { state }
  }

  /// Record a new conflict.
  pub fn create(&self, record: ConflictRecord) -> Result<ConflictRecord> {
    let path = format!("{}{}.json", CONFLICTS_PATH, record.id);
    self.state.store_json(&path, &record)?;

    tracing::warn!(
      "conflict detected: {} (relationship {})",
      record.file_path, record.relationship_id,
    );

    Ok(record)
  }

  /// Create a new ConflictRecord with a generated ID and current timestamp.
  pub fn record_conflict(
    &self,
    file_path: &str,
    relationship_id: &str,
    local_hash: &str,
    remote_hash: &str,
    last_synced_hash: &str,
    local_modified_at: Option<i64>,
    remote_modified_at: Option<i64>,
  ) -> Result<ConflictRecord> {
    let record = ConflictRecord {
      id:                 Uuid::new_v4().to_string(),
      file_path:          file_path.to_string(),
      relationship_id:    relationship_id.to_string(),
      local_hash:         local_hash.to_string(),
      remote_hash:        remote_hash.to_string(),
      last_synced_hash:   last_synced_hash.to_string(),
      local_modified_at,
      remote_modified_at,
      detected_at:        Utc::now(),
    };

    self.create(record)
  }

  /// List all pending conflicts.
  pub fn list(&self) -> Result<Vec<ConflictRecord>> {
    let entries = self.state.list_directory(CONFLICTS_PATH)?;
    let mut conflicts = Vec::new();

    for entry_name in entries {
      if !entry_name.ends_with(".json") || entry_name == ".keep" {
        continue;
      }

      let path = format!("{}{}", CONFLICTS_PATH, entry_name);
      if let Some(conflict) = self.state.read_json::<ConflictRecord>(&path)? {
        conflicts.push(conflict);
      }
    }

    conflicts.sort_by(|a, b| a.detected_at.cmp(&b.detected_at));
    Ok(conflicts)
  }

  /// List conflicts for a specific sync relationship.
  pub fn list_for_relationship(&self, relationship_id: &str) -> Result<Vec<ConflictRecord>> {
    let all = self.list()?;
    Ok(all.into_iter()
      .filter(|c| c.relationship_id == relationship_id)
      .collect())
  }

  /// Get a specific conflict by ID.
  pub fn get(&self, id: &str) -> Result<Option<ConflictRecord>> {
    let path = format!("{}{}.json", CONFLICTS_PATH, id);
    self.state.read_json(&path)
  }

  /// Resolve a conflict by removing it from the queue.
  /// The actual file resolution (overwrite local, overwrite remote, or rename)
  /// is handled by the caller.
  pub fn resolve(&self, id: &str) -> Result<()> {
    let path = format!("{}{}.json", CONFLICTS_PATH, id);

    if !self.state.exists(&path)? {
      return Err(ClientError::Configuration(
        format!("conflict not found: {}", id),
      ));
    }

    self.state.delete(&path)?;
    tracing::info!("resolved conflict {}", id);
    Ok(())
  }

  /// Resolve all pending conflicts.
  pub fn resolve_all(&self) -> Result<u64> {
    let conflicts = self.list()?;
    let count     = conflicts.len() as u64;

    for conflict in conflicts {
      self.resolve(&conflict.id)?;
    }

    Ok(count)
  }

  /// Check if a conflict already exists for a given file in a relationship.
  pub fn has_conflict_for(&self, relationship_id: &str, file_path: &str) -> Result<bool> {
    let conflicts = self.list_for_relationship(relationship_id)?;
    Ok(conflicts.iter().any(|c| c.file_path == file_path))
  }
}
