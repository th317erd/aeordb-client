use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::state::StateStore;
use crate::sync::pull::PullResult;
use crate::sync::push::PushResult;
use crate::sync::replication::SyncResult;

/// A single recorded sync event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEvent {
  pub id:                String,
  pub relationship_id:   String,
  pub relationship_name: String,
  pub event_type:        String,
  pub summary:           String,
  pub files_affected:    u64,
  pub bytes_transferred: u64,
  pub duration_ms:       u64,
  pub errors:            Vec<String>,
  pub timestamp:         i64,
}

/// Persists sync activity events in the state database.
#[derive(Clone)]
pub struct SyncActivityLog {
  state: Arc<StateStore>,
}

impl SyncActivityLog {
  pub fn new(state: Arc<StateStore>) -> Self {
    Self { state }
  }

  /// Store a sync event at `/sync/activity/{relationship_id}/{timestamp}-{short_id}.json`.
  pub fn log_event(&self, event: &SyncEvent) -> Result<()> {
    let short_id = &event.id[..8];
    let path = format!(
      "/sync/activity/{}/{}-{}.json",
      event.relationship_id, event.timestamp, short_id,
    );

    self.state.store_json(&path, event)?;
    Ok(())
  }

  /// List events for a relationship, newest first, limited to `limit`.
  pub fn get_events(&self, relationship_id: &str, limit: usize) -> Result<Vec<SyncEvent>> {
    let directory = format!("/sync/activity/{}/", relationship_id);

    if !self.state.exists(&directory)? {
      return Ok(Vec::new());
    }

    let mut entries = self.state.list_directory(&directory)?;

    // Filter out placeholder files.
    entries.retain(|name| name.ends_with(".json") && name != ".keep");

    // Sort descending by name (timestamp prefix ensures chronological order).
    entries.sort();
    entries.reverse();

    // Limit.
    entries.truncate(limit);

    let mut events = Vec::with_capacity(entries.len());
    for entry in &entries {
      let path = format!("{}{}", directory, entry);
      if let Some(event) = self.state.read_json::<SyncEvent>(&path)? {
        events.push(event);
      }
    }

    Ok(events)
  }

  /// Create and log a `SyncEvent` from a `PullResult`.
  pub fn log_pull(
    &self,
    relationship_id: &str,
    relationship_name: &str,
    result: &PullResult,
  ) -> Result<()> {
    let files_affected = result.files_pulled + result.files_deleted + result.symlinks_pulled;
    let summary = format!(
      "pulled={}, deleted={}, skipped={}, failed={}, symlinks={}",
      result.files_pulled, result.files_deleted,
      result.files_skipped, result.files_failed,
      result.symlinks_pulled,
    );

    let event = SyncEvent {
      id:                Uuid::new_v4().to_string(),
      relationship_id:   relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type:        "pull".to_string(),
      summary,
      files_affected,
      bytes_transferred: result.total_bytes,
      duration_ms:       result.duration_ms,
      errors:            result.errors.clone(),
      timestamp:         chrono::Utc::now().timestamp_millis(),
    };

    self.log_event(&event)
  }

  /// Create and log a `SyncEvent` from a `PushResult`.
  pub fn log_push(
    &self,
    relationship_id: &str,
    relationship_name: &str,
    result: &PushResult,
  ) -> Result<()> {
    let files_affected = result.files_pushed + result.files_deleted;
    let summary = format!(
      "pushed={}, deleted={}, skipped={}, failed={}",
      result.files_pushed, result.files_deleted,
      result.files_skipped, result.files_failed,
    );

    let event = SyncEvent {
      id:                Uuid::new_v4().to_string(),
      relationship_id:   relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type:        "push".to_string(),
      summary,
      files_affected,
      bytes_transferred: result.total_bytes,
      duration_ms:       result.duration_ms,
      errors:            result.errors.clone(),
      timestamp:         chrono::Utc::now().timestamp_millis(),
    };

    self.log_event(&event)
  }

  /// Create and log a `SyncEvent` from a combined `SyncResult`.
  pub fn log_full_sync(
    &self,
    relationship_id: &str,
    relationship_name: &str,
    result: &SyncResult,
  ) -> Result<()> {
    let mut files_affected:    u64 = 0;
    let mut bytes_transferred: u64 = 0;
    let mut duration_ms:       u64 = 0;
    let mut errors: Vec<String>    = Vec::new();
    let mut parts: Vec<String>     = Vec::new();

    if let Some(ref pull) = result.pull {
      files_affected    += pull.files_pulled + pull.files_deleted + pull.symlinks_pulled;
      bytes_transferred += pull.total_bytes;
      duration_ms       += pull.duration_ms;
      errors.extend(pull.errors.iter().cloned());
      parts.push(format!(
        "pull(pulled={}, deleted={}, failed={})",
        pull.files_pulled, pull.files_deleted, pull.files_failed,
      ));
    }

    if let Some(ref push) = result.push {
      files_affected    += push.files_pushed + push.files_deleted;
      bytes_transferred += push.total_bytes;
      duration_ms       += push.duration_ms;
      errors.extend(push.errors.iter().cloned());
      parts.push(format!(
        "push(pushed={}, deleted={}, failed={})",
        push.files_pushed, push.files_deleted, push.files_failed,
      ));
    }

    let summary = if parts.is_empty() {
      "no-op".to_string()
    } else {
      parts.join(", ")
    };

    let event = SyncEvent {
      id:                Uuid::new_v4().to_string(),
      relationship_id:   relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type:        "full_sync".to_string(),
      summary,
      files_affected,
      bytes_transferred,
      duration_ms,
      errors,
      timestamp:         chrono::Utc::now().timestamp_millis(),
    };

    self.log_event(&event)
  }

  /// Log an error event.
  pub fn log_error(
    &self,
    relationship_id: &str,
    relationship_name: &str,
    error_message: &str,
  ) -> Result<()> {
    let event = SyncEvent {
      id:                Uuid::new_v4().to_string(),
      relationship_id:   relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type:        "error".to_string(),
      summary:           error_message.to_string(),
      files_affected:    0,
      bytes_transferred: 0,
      duration_ms:       0,
      errors:            vec![error_message.to_string()],
      timestamp:         chrono::Utc::now().timestamp_millis(),
    };

    self.log_event(&event)
  }
}
