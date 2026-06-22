use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::state::StateStore;
use crate::sync::pull::PullResult;
use crate::sync::push::PushResult;
use crate::sync::replication::SyncResult;

const LATEST_EVENTS_LIMIT: usize = 100;

/// A single recorded sync event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEvent {
  pub id: String,
  pub relationship_id: String,
  pub relationship_name: String,
  pub event_type: String,
  pub summary: String,
  pub files_affected: u64,
  pub bytes_transferred: u64,
  pub duration_ms: u64,
  pub errors: Vec<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub progress_percent: Option<f64>,
  pub timestamp: i64,
}

/// Persists sync activity events in the state database.
#[derive(Clone)]
pub struct SyncActivityLog {
  state: Arc<StateStore>,
  latest_progress: Arc<Mutex<HashMap<String, SyncEvent>>>,
}

impl SyncActivityLog {
  pub fn new(state: Arc<StateStore>) -> Self {
    Self {
      state,
      latest_progress: Arc::new(Mutex::new(HashMap::new())),
    }
  }

  /// Store a sync event at `/sync/activity/{relationship_id}/{timestamp}-{short_id}.json`.
  pub fn log_event(&self, event: &SyncEvent) -> Result<()> {
    let short_id = event.id.get(..8).unwrap_or(&event.id);
    let path = format!(
      "/sync/activity/{}/{}-{}.json",
      event.relationship_id, event.timestamp, short_id,
    );

    if event.event_type == "progress" {
      self
        .latest_progress()
        .insert(event.relationship_id.clone(), event.clone());
      return Ok(());
    }

    self.state.store_json(&path, event)?;
    self.update_latest_events_index(event)?;
    self.latest_progress().remove(&event.relationship_id);
    Ok(())
  }

  /// List events for a relationship, newest first, limited to `limit`.
  pub fn get_events(&self, relationship_id: &str, limit: usize) -> Result<Vec<SyncEvent>> {
    if limit == 0 {
      return Ok(Vec::new());
    }

    let index_path = format!("/sync/activity/{}/latest-events.json", relationship_id);
    let latest_progress = self.latest_progress().get(relationship_id).cloned();

    if let Some(mut events) = self.state.read_json::<Vec<SyncEvent>>(&index_path)? {
      events.retain(is_durable_activity_event);
      if let Some(event) = latest_progress {
        events.push(event);
      }
      sort_dedupe_truncate(&mut events, limit);
      return Ok(events);
    }

    let directory = format!("/sync/activity/{}/", relationship_id);

    if !self.state.exists(&directory)? {
      let mut events = latest_progress.into_iter().collect::<Vec<_>>();
      sort_dedupe_truncate(&mut events, limit);
      return Ok(events);
    }

    let mut timestamped_entries = Vec::new();
    for name in self.state.list_directory(&directory)? {
      if !name.ends_with(".json") || name == ".keep" || name == "latest-events.json" {
        continue;
      }
      if name == "latest-progress.json" {
        continue;
      } else {
        timestamped_entries.push(name);
      }
    }

    timestamped_entries.sort_by(|a, b| b.cmp(a));

    let mut events = Vec::with_capacity(limit.saturating_add(1));
    let mut seen_ids = HashSet::new();

    if let Some(event) = latest_progress {
      seen_ids.insert(event.id.clone());
      events.push(event);
    }

    // Filenames begin with millisecond timestamps, so descending filename
    // order gives us newest-first candidates without opening the whole
    // activity directory. Read a small cushion beyond the requested limit so
    // an in-memory progress event can still sort into the correct spot.
    let candidate_limit = limit.saturating_mul(2).saturating_add(10);
    for entry in timestamped_entries.iter().take(candidate_limit) {
      let path = format!("{}{}", directory, entry);
      if let Some(event) = self.state.read_json::<SyncEvent>(&path)? {
        if is_durable_activity_event(&event) && seen_ids.insert(event.id.clone()) {
          events.push(event);
        }
      }
    }

    sort_dedupe_truncate(&mut events, limit);
    if let Err(error) = self.state.store_json(&index_path, &events) {
      tracing::debug!("failed to backfill latest activity index: {}", error);
    }

    Ok(events)
  }

  fn update_latest_events_index(&self, event: &SyncEvent) -> Result<()> {
    let index_path = format!(
      "/sync/activity/{}/latest-events.json",
      event.relationship_id
    );
    let mut events = self
      .state
      .read_json::<Vec<SyncEvent>>(&index_path)?
      .unwrap_or_default();
    events.push(event.clone());
    events.retain(is_durable_activity_event);
    sort_dedupe_truncate(&mut events, LATEST_EVENTS_LIMIT);
    self.state.store_json(&index_path, &events)
  }

  fn latest_progress(&self) -> MutexGuard<'_, HashMap<String, SyncEvent>> {
    self
      .latest_progress
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  /// Create and log a `SyncEvent` from a `PullResult`.
  pub fn log_pull(
    &self,
    relationship_id: &str,
    relationship_name: &str,
    result: &PullResult,
  ) -> Result<()> {
    let files_affected = result.files_pulled + result.files_deleted + result.symlinks_pulled;
    let summary = summarize_pull_result(result);

    let event = SyncEvent {
      id: Uuid::new_v4().to_string(),
      relationship_id: relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type: "pull".to_string(),
      summary,
      files_affected,
      bytes_transferred: result.total_bytes,
      duration_ms: result.duration_ms,
      errors: result.errors.clone(),
      progress_percent: None,
      timestamp: chrono::Utc::now().timestamp_millis(),
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
    let summary = summarize_push_result(result);

    let event = SyncEvent {
      id: Uuid::new_v4().to_string(),
      relationship_id: relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type: "push".to_string(),
      summary,
      files_affected,
      bytes_transferred: result.total_bytes,
      duration_ms: result.duration_ms,
      errors: result.errors.clone(),
      progress_percent: None,
      timestamp: chrono::Utc::now().timestamp_millis(),
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
    let mut files_affected: u64 = 0;
    let mut bytes_transferred: u64 = 0;
    let mut duration_ms: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    if let Some(ref pull) = result.pull {
      files_affected += pull.files_pulled + pull.files_deleted + pull.symlinks_pulled;
      bytes_transferred += pull.total_bytes;
      duration_ms += pull.duration_ms;
      errors.extend(pull.errors.iter().cloned());
    }

    if let Some(ref push) = result.push {
      files_affected += push.files_pushed + push.files_deleted;
      bytes_transferred += push.total_bytes;
      duration_ms += push.duration_ms;
      errors.extend(push.errors.iter().cloned());
    }

    let summary = summarize_full_sync_result(result);

    let event = SyncEvent {
      id: Uuid::new_v4().to_string(),
      relationship_id: relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type: "full_sync".to_string(),
      summary,
      files_affected,
      bytes_transferred,
      duration_ms,
      errors,
      progress_percent: None,
      timestamp: chrono::Utc::now().timestamp_millis(),
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
      id: Uuid::new_v4().to_string(),
      relationship_id: relationship_id.to_string(),
      relationship_name: relationship_name.to_string(),
      event_type: "error".to_string(),
      summary: error_message.to_string(),
      files_affected: 0,
      bytes_transferred: 0,
      duration_ms: 0,
      errors: vec![error_message.to_string()],
      progress_percent: None,
      timestamp: chrono::Utc::now().timestamp_millis(),
    };

    self.log_event(&event)
  }
}

fn sort_dedupe_truncate(events: &mut Vec<SyncEvent>, limit: usize) {
  events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp).then_with(|| b.id.cmp(&a.id)));

  let mut seen_ids = HashSet::new();
  events.retain(|event| seen_ids.insert(event.id.clone()));
  events.truncate(limit);
}

fn is_durable_activity_event(event: &SyncEvent) -> bool {
  event.event_type != "progress"
}

pub(crate) fn summarize_push_result(result: &PushResult) -> String {
  let changed = result.files_pushed + result.files_deleted;
  if changed == 0 && result.files_failed == 0 {
    return format!(
      "No upload needed · {} already current · {}",
      pluralize(result.files_skipped, "file"),
      format_duration(result.duration_ms),
    );
  }

  let mut parts = Vec::new();
  if result.files_pushed > 0 {
    if result.total_bytes > 0 {
      parts.push(format!(
        "Uploaded {} ({})",
        pluralize(result.files_pushed, "file"),
        format_bytes(result.total_bytes),
      ));
    } else {
      parts.push(format!(
        "Committed {} (0 B sent)",
        pluralize(result.files_pushed, "remote update"),
      ));
    }
  }
  if result.files_deleted > 0 {
    parts.push(format!(
      "Deleted {}",
      pluralize(result.files_deleted, "remote file")
    ));
  }
  if result.files_skipped > 0 {
    parts.push(format!("{} unchanged", format_count(result.files_skipped)));
  }
  if result.files_failed > 0 {
    parts.push(format!("{} failed", format_count(result.files_failed)));
  }
  parts.push(format_duration(result.duration_ms));
  if let Some(rate) = format_rate(result.total_bytes, result.duration_ms) {
    parts.push(rate);
  }
  parts.join(" · ")
}

pub(crate) fn summarize_pull_result(result: &PullResult) -> String {
  let changed = result.files_pulled + result.files_deleted + result.symlinks_pulled;
  if changed == 0 && result.files_failed == 0 {
    return format!(
      "No receive needed · {} already current · {}",
      pluralize(result.files_skipped, "file"),
      format_duration(result.duration_ms),
    );
  }

  let mut parts = Vec::new();
  if result.files_pulled > 0 {
    parts.push(format!(
      "Received {} ({})",
      pluralize(result.files_pulled, "file"),
      format_bytes(result.total_bytes),
    ));
  }
  if result.files_deleted > 0 {
    parts.push(format!(
      "Deleted {}",
      pluralize(result.files_deleted, "local file")
    ));
  }
  if result.symlinks_pulled > 0 {
    parts.push(format!(
      "Updated {}",
      pluralize(result.symlinks_pulled, "symlink")
    ));
  }
  if result.files_skipped > 0 {
    parts.push(format!("{} unchanged", format_count(result.files_skipped)));
  }
  if result.files_failed > 0 {
    parts.push(format!("{} failed", format_count(result.files_failed)));
  }
  parts.push(format_duration(result.duration_ms));
  if let Some(rate) = format_rate(result.total_bytes, result.duration_ms) {
    parts.push(rate);
  }
  parts.join(" · ")
}

pub(crate) fn summarize_full_sync_result(result: &SyncResult) -> String {
  let mut parts = Vec::new();
  let mut duration_ms = 0;
  if let Some(ref pull) = result.pull {
    duration_ms += pull.duration_ms;
    parts.push(summarize_pull_result(pull));
  }
  if let Some(ref push) = result.push {
    duration_ms += push.duration_ms;
    parts.push(summarize_push_result(push));
  }
  let completion = format!("Sync completed in {}", format_duration_minutes(duration_ms));
  if parts.is_empty() {
    format!("{} · No sync work needed", completion)
  } else {
    format!("{} · {}", completion, parts.join(" / "))
  }
}

pub(crate) fn pluralize(count: u64, singular: &str) -> String {
  if count == 1 {
    format!("{} {}", format_count(count), singular)
  } else {
    format!("{} {}s", format_count(count), singular)
  }
}

pub(crate) fn format_count(count: u64) -> String {
  let raw = count.to_string();
  let mut out = String::with_capacity(raw.len() + raw.len() / 3);
  for (idx, ch) in raw.chars().rev().enumerate() {
    if idx > 0 && idx % 3 == 0 {
      out.push(',');
    }
    out.push(ch);
  }
  out.chars().rev().collect()
}

pub(crate) fn format_bytes(bytes: u64) -> String {
  format_bytes_with_precision(bytes, false)
}

pub(crate) fn format_bytes_precise(bytes: u64) -> String {
  format_bytes_with_precision(bytes, true)
}

fn format_bytes_with_precision(bytes: u64, precise: bool) -> String {
  const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
  let mut value = bytes as f64;
  let mut unit = 0;
  while value >= 1024.0 && unit < UNITS.len() - 1 {
    value /= 1024.0;
    unit += 1;
  }
  if unit == 0 {
    format!("{} {}", bytes, UNITS[unit])
  } else if precise {
    format!("{:.2} {}", value, UNITS[unit])
  } else if value >= 100.0 {
    format!("{:.0} {}", value, UNITS[unit])
  } else if value >= 10.0 {
    format!("{:.1} {}", value, UNITS[unit])
  } else {
    format!("{:.2} {}", value, UNITS[unit])
  }
}

pub(crate) fn format_duration(duration_ms: u64) -> String {
  if duration_ms < 1_000 {
    return format!("{}ms", duration_ms);
  }
  if duration_ms < 60_000 {
    return format!("{:.1}s", duration_ms as f64 / 1_000.0);
  }

  let total_seconds = duration_ms / 1_000;
  let minutes = total_seconds / 60;
  let seconds = total_seconds % 60;
  if minutes < 60 {
    return format!("{}m {:02}s", minutes, seconds);
  }

  let hours = minutes / 60;
  let minutes = minutes % 60;
  format!("{}h {:02}m", hours, minutes)
}

pub(crate) fn format_duration_minutes(duration_ms: u64) -> String {
  format!("{:.2} minutes", duration_ms as f64 / 60_000.0)
}

pub(crate) fn format_rate(bytes: u64, duration_ms: u64) -> Option<String> {
  if bytes == 0 || duration_ms == 0 {
    return None;
  }
  let bytes_per_second = (bytes as f64 / duration_ms as f64 * 1_000.0) as u64;
  Some(format!("{}/s", format_bytes(bytes_per_second)))
}

#[cfg(test)]
mod tests {
  use super::{SyncActivityLog, SyncEvent};
  use crate::state::StateStore;
  use crate::sync::pull::PullResult;
  use crate::sync::push::PushResult;
  use crate::sync::replication::SyncResult;
  use std::sync::Arc;

  fn event(id: &str, event_type: &str, summary: &str, timestamp: i64) -> SyncEvent {
    SyncEvent {
      id: id.to_string(),
      relationship_id: "rel-1".to_string(),
      relationship_name: "Test Sync".to_string(),
      event_type: event_type.to_string(),
      summary: summary.to_string(),
      files_affected: 0,
      bytes_transferred: 0,
      duration_ms: 0,
      errors: Vec::new(),
      progress_percent: if event_type == "progress" {
        Some(10.0)
      } else {
        None
      },
      timestamp,
    }
  }

  #[test]
  fn progress_events_keep_only_latest_progress_pointer() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("state.aeordb");
    let state =
      StateStore::open_or_create(db_path.to_str().expect("utf8 path")).expect("state store");
    let log = SyncActivityLog::new(Arc::new(state));

    log
      .log_event(&event("old-progress", "progress", "old progress", 1000))
      .expect("old progress");
    log
      .log_event(&event("new-progress", "progress", "new progress", 3000))
      .expect("new progress");

    let events = log.get_events("rel-1", 10).expect("events");
    let summaries: Vec<&str> = events.iter().map(|event| event.summary.as_str()).collect();

    assert_eq!(summaries, vec!["new progress"]);
    assert!(
      !log
        .state
        .exists("/sync/activity/rel-1/latest-progress.json")
        .expect("exists check"),
      "ephemeral progress must not be persisted to the state database",
    );
  }

  #[test]
  fn scan_heartbeat_events_are_durable_activity() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("state.aeordb");
    let state =
      StateStore::open_or_create(db_path.to_str().expect("utf8 path")).expect("state store");
    let log = SyncActivityLog::new(Arc::new(state));

    log
      .log_event(&event(
        "scan-heartbeat",
        "scan_heartbeat",
        "Full scan inspecting local entries",
        2000,
      ))
      .expect("scan heartbeat");

    let events = log.get_events("rel-1", 10).expect("events");
    let summaries: Vec<&str> = events.iter().map(|event| event.summary.as_str()).collect();

    assert_eq!(summaries, vec!["Full scan inspecting local entries"]);
    assert!(
      log
        .state
        .exists("/sync/activity/rel-1/2000-scan-hea.json")
        .expect("exists check"),
      "scan heartbeats should be stored as visible activity history",
    );
  }

  #[test]
  fn get_events_merges_latest_progress_with_latest_events_index() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("state.aeordb");
    let state =
      StateStore::open_or_create(db_path.to_str().expect("utf8 path")).expect("state store");
    let log = SyncActivityLog::new(Arc::new(state));

    log
      .log_event(&event("push-1", "push", "pushed one file", 1000))
      .expect("push event");
    log
      .log_event(&event("new-progress", "progress", "new progress", 3000))
      .expect("new progress");

    // If get_events scanned the directory, this manually inserted event would
    // show up. With the index present, reads stay bounded to latest-events.
    let stray = event("stray", "push", "stray historical file", 4000);
    log
      .state
      .store_json("/sync/activity/rel-1/4000-stray.json", &stray)
      .expect("stray event");

    let events = log.get_events("rel-1", 10).expect("events");
    let summaries: Vec<&str> = events.iter().map(|event| event.summary.as_str()).collect();

    assert_eq!(summaries, vec!["new progress", "pushed one file"]);
  }

  #[test]
  fn get_events_ignores_legacy_progress_entries_in_latest_events_index() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("state.aeordb");
    let state =
      StateStore::open_or_create(db_path.to_str().expect("utf8 path")).expect("state store");
    let log = SyncActivityLog::new(Arc::new(state));

    let legacy_progress = event("legacy-progress", "progress", "stale progress", 3000);
    let completion = event("completion", "full_sync", "completed", 2000);
    log
      .state
      .store_json(
        "/sync/activity/rel-1/latest-events.json",
        &vec![legacy_progress, completion],
      )
      .expect("legacy index");

    let events = log.get_events("rel-1", 10).expect("events");
    let summaries: Vec<&str> = events.iter().map(|event| event.summary.as_str()).collect();

    assert_eq!(summaries, vec!["completed"]);
  }

  #[test]
  fn get_events_ignores_legacy_timestamped_progress_entries_during_backfill() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("state.aeordb");
    let state =
      StateStore::open_or_create(db_path.to_str().expect("utf8 path")).expect("state store");
    let log = SyncActivityLog::new(Arc::new(state));

    let legacy_progress = event("legacy-progress", "progress", "stale progress", 3000);
    let completion = event("completion", "full_sync", "completed", 2000);
    log
      .state
      .store_json("/sync/activity/rel-1/3000-legacy.json", &legacy_progress)
      .expect("legacy progress file");
    log
      .state
      .store_json("/sync/activity/rel-1/2000-completion.json", &completion)
      .expect("completion file");

    let events = log.get_events("rel-1", 10).expect("events");
    let summaries: Vec<&str> = events.iter().map(|event| event.summary.as_str()).collect();

    assert_eq!(summaries, vec!["completed"]);
  }

  #[test]
  fn get_events_ignores_legacy_persisted_latest_progress_file() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let db_path = temp_dir.path().join("state.aeordb");
    let state =
      StateStore::open_or_create(db_path.to_str().expect("utf8 path")).expect("state store");
    let log = SyncActivityLog::new(Arc::new(state));

    let legacy_progress = event("legacy-progress", "progress", "stale progress", 3000);
    let completion = event("completion", "full_sync", "completed", 2000);
    log
      .state
      .store_json(
        "/sync/activity/rel-1/latest-progress.json",
        &legacy_progress,
      )
      .expect("legacy latest progress file");
    log
      .state
      .store_json("/sync/activity/rel-1/2000-completion.json", &completion)
      .expect("completion file");

    let events = log.get_events("rel-1", 10).expect("events");
    let summaries: Vec<&str> = events.iter().map(|event| event.summary.as_str()).collect();

    assert_eq!(summaries, vec!["completed"]);
  }

  #[test]
  fn formats_counts_bytes_duration_and_rate_for_humans() {
    assert_eq!(super::format_count(4_953), "4,953");
    assert_eq!(super::format_bytes(573_571_072), "547 MB");
    assert_eq!(super::format_bytes_precise(13_170_000), "12.56 MB");
    assert_eq!(super::format_duration(12_100), "12.1s");
    assert_eq!(super::format_duration(184_000), "3m 04s");
    assert_eq!(
      super::format_rate(573_571_072, 360_000),
      Some("1.52 MB/s".to_string()),
    );
    assert_eq!(super::format_duration_minutes(2_284_500), "38.08 minutes");
  }

  #[test]
  fn push_summary_reports_outcome_not_raw_counters() {
    let result = PushResult {
      files_pushed: 1_152,
      files_skipped: 248,
      files_failed: 0,
      files_deleted: 0,
      total_bytes: 573_571_072,
      duration_ms: 360_000,
      errors: Vec::new(),
    };

    assert_eq!(
      super::summarize_push_result(&result),
      "Uploaded 1,152 files (547 MB) · 248 unchanged · 6m 00s · 1.52 MB/s",
    );
  }

  #[test]
  fn no_upload_summary_still_explains_what_happened() {
    let result = PushResult {
      files_pushed: 0,
      files_skipped: 4_953,
      files_failed: 0,
      files_deleted: 0,
      total_bytes: 0,
      duration_ms: 8_200,
      errors: Vec::new(),
    };

    assert_eq!(
      super::summarize_push_result(&result),
      "No upload needed · 4,953 files already current · 8.2s",
    );
  }

  #[test]
  fn zero_byte_push_summary_uses_commit_language() {
    let result = PushResult {
      files_pushed: 1,
      files_skipped: 4_969,
      files_failed: 0,
      files_deleted: 0,
      total_bytes: 0,
      duration_ms: 2_292_000,
      errors: Vec::new(),
    };

    assert_eq!(
      super::summarize_push_result(&result),
      "Committed 1 remote update (0 B sent) · 4,969 unchanged · 38m 12s",
    );
  }

  #[test]
  fn pull_summary_uses_receive_language() {
    let result = PullResult {
      files_pulled: 42,
      files_skipped: 8,
      files_failed: 0,
      files_deleted: 0,
      symlinks_pulled: 0,
      total_bytes: 26_340_000,
      duration_ms: 12_100,
      errors: Vec::new(),
    };

    assert_eq!(
      super::summarize_pull_result(&result),
      "Received 42 files (25.1 MB) · 8 unchanged · 12.1s · 2.08 MB/s",
    );
  }

  #[test]
  fn full_sync_summary_reports_completion_minutes() {
    let result = SyncResult {
      pull: Some(PullResult {
        files_pulled: 0,
        files_skipped: 2,
        files_failed: 0,
        files_deleted: 0,
        symlinks_pulled: 0,
        total_bytes: 0,
        duration_ms: 1_500,
        errors: Vec::new(),
      }),
      push: Some(PushResult {
        files_pushed: 10,
        files_skipped: 4_960,
        files_failed: 0,
        files_deleted: 0,
        total_bytes: 13_170_000,
        duration_ms: 221_000,
        errors: Vec::new(),
      }),
    };

    assert_eq!(
      super::summarize_full_sync_result(&result),
      "Sync completed in 3.71 minutes · No receive needed · 2 files already current · 1.5s / Uploaded 10 files (12.6 MB) · 4,960 unchanged · 3m 41s · 58.2 KB/s",
    );
  }
}
