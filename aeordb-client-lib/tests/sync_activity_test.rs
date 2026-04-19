use std::sync::Arc;

use aeordb_client_lib::state::StateStore;
use aeordb_client_lib::sync::activity::{SyncActivityLog, SyncEvent};

fn temp_database_path() -> String {
  let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
  temp_dir
    .keep()
    .join("test-activity.aeordb")
    .to_string_lossy()
    .to_string()
}

fn create_test_event(relationship_id: &str, event_type: &str, timestamp: i64) -> SyncEvent {
  SyncEvent {
    id:                uuid::Uuid::new_v4().to_string(),
    relationship_id:   relationship_id.to_string(),
    relationship_name: "test-relationship".to_string(),
    event_type:        event_type.to_string(),
    summary:           format!("{} event", event_type),
    files_affected:    5,
    bytes_transferred: 1024,
    duration_ms:       100,
    errors:            Vec::new(),
    timestamp,
  }
}

// --- Happy path tests ---

#[test]
fn test_log_and_retrieve_single_event() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let event = create_test_event("rel-1", "push", 1000);
  log.log_event(&event).expect("failed to log event");

  let events = log.get_events("rel-1", 50).expect("failed to get events");
  assert_eq!(events.len(), 1);
  assert_eq!(events[0].id, event.id);
  assert_eq!(events[0].event_type, "push");
  assert_eq!(events[0].files_affected, 5);
  assert_eq!(events[0].bytes_transferred, 1024);
}

#[test]
fn test_events_returned_newest_first() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let event_old  = create_test_event("rel-1", "pull", 1000);
  let event_mid  = create_test_event("rel-1", "push", 2000);
  let event_new  = create_test_event("rel-1", "full_sync", 3000);

  // Insert in random order.
  log.log_event(&event_mid).expect("failed to log");
  log.log_event(&event_old).expect("failed to log");
  log.log_event(&event_new).expect("failed to log");

  let events = log.get_events("rel-1", 50).expect("failed to get events");
  assert_eq!(events.len(), 3);
  assert_eq!(events[0].timestamp, 3000);
  assert_eq!(events[1].timestamp, 2000);
  assert_eq!(events[2].timestamp, 1000);
}

#[test]
fn test_limit_restricts_returned_events() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  for i in 0..10 {
    let event = create_test_event("rel-1", "push", 1000 + i);
    log.log_event(&event).expect("failed to log");
  }

  let events = log.get_events("rel-1", 3).expect("failed to get events");
  assert_eq!(events.len(), 3);

  // Should be the newest 3.
  assert_eq!(events[0].timestamp, 1009);
  assert_eq!(events[1].timestamp, 1008);
  assert_eq!(events[2].timestamp, 1007);
}

#[test]
fn test_events_isolated_by_relationship() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let event_a = create_test_event("rel-a", "push", 1000);
  let event_b = create_test_event("rel-b", "pull", 2000);

  log.log_event(&event_a).expect("failed to log");
  log.log_event(&event_b).expect("failed to log");

  let events_a = log.get_events("rel-a", 50).expect("failed to get events");
  assert_eq!(events_a.len(), 1);
  assert_eq!(events_a[0].relationship_id, "rel-a");

  let events_b = log.get_events("rel-b", 50).expect("failed to get events");
  assert_eq!(events_b.len(), 1);
  assert_eq!(events_b[0].relationship_id, "rel-b");
}

#[test]
fn test_event_with_errors_roundtrips() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let mut event = create_test_event("rel-1", "error", 5000);
  event.errors = vec!["connection timeout".to_string(), "retry failed".to_string()];

  log.log_event(&event).expect("failed to log");

  let events = log.get_events("rel-1", 50).expect("failed to get events");
  assert_eq!(events.len(), 1);
  assert_eq!(events[0].errors.len(), 2);
  assert_eq!(events[0].errors[0], "connection timeout");
  assert_eq!(events[0].errors[1], "retry failed");
}

#[test]
fn test_all_event_types_stored() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  for (i, event_type) in ["pull", "push", "full_sync", "error"].iter().enumerate() {
    let event = create_test_event("rel-1", event_type, 1000 + i as i64);
    log.log_event(&event).expect("failed to log");
  }

  let events = log.get_events("rel-1", 50).expect("failed to get events");
  assert_eq!(events.len(), 4);

  let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
  assert!(types.contains(&"pull"));
  assert!(types.contains(&"push"));
  assert!(types.contains(&"full_sync"));
  assert!(types.contains(&"error"));
}

// --- Edge cases and empty/nonexistent paths ---

#[test]
fn test_get_events_empty_relationship_returns_empty() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let events = log.get_events("nonexistent-relationship", 50).expect("failed to get events");
  assert!(events.is_empty());
}

#[test]
fn test_limit_zero_returns_empty() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let event = create_test_event("rel-1", "push", 1000);
  log.log_event(&event).expect("failed to log");

  let events = log.get_events("rel-1", 0).expect("failed to get events");
  assert!(events.is_empty());
}

#[test]
fn test_limit_exceeds_available_events() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  let event = create_test_event("rel-1", "push", 1000);
  log.log_event(&event).expect("failed to log");

  let events = log.get_events("rel-1", 1000).expect("failed to get events");
  assert_eq!(events.len(), 1);
}

// --- Helper method tests ---

#[test]
fn test_log_error_creates_error_event() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  log.log_error("rel-1", "test-rel", "connection refused")
    .expect("failed to log error");

  let events = log.get_events("rel-1", 50).expect("failed to get events");
  assert_eq!(events.len(), 1);
  assert_eq!(events[0].event_type, "error");
  assert_eq!(events[0].summary, "connection refused");
  assert_eq!(events[0].errors, vec!["connection refused"]);
  assert_eq!(events[0].files_affected, 0);
  assert_eq!(events[0].bytes_transferred, 0);
}

#[test]
fn test_directory_structure_includes_activity() {
  let path  = temp_database_path();
  let store = StateStore::open_or_create(&path).expect("failed to create");

  assert!(store.exists("/sync/activity/").expect("exists check failed"));
}

// --- Serialization roundtrip ---

#[test]
fn test_sync_event_serialization_roundtrip() {
  let event = create_test_event("rel-1", "push", 9999);
  let json  = serde_json::to_string(&event).expect("failed to serialize");
  let deserialized: SyncEvent = serde_json::from_str(&json).expect("failed to deserialize");

  assert_eq!(deserialized.id, event.id);
  assert_eq!(deserialized.relationship_id, event.relationship_id);
  assert_eq!(deserialized.relationship_name, event.relationship_name);
  assert_eq!(deserialized.event_type, event.event_type);
  assert_eq!(deserialized.summary, event.summary);
  assert_eq!(deserialized.files_affected, event.files_affected);
  assert_eq!(deserialized.bytes_transferred, event.bytes_transferred);
  assert_eq!(deserialized.duration_ms, event.duration_ms);
  assert_eq!(deserialized.timestamp, event.timestamp);
}

// --- Multiple events at same timestamp ---

#[test]
fn test_multiple_events_same_timestamp_no_collision() {
  let path  = temp_database_path();
  let store = Arc::new(StateStore::open_or_create(&path).expect("failed to create"));
  let log   = SyncActivityLog::new(store);

  // Two different events at the same timestamp should not collide
  // because the filename includes a short UUID.
  let event_1 = create_test_event("rel-1", "push", 5000);
  let event_2 = create_test_event("rel-1", "pull", 5000);

  log.log_event(&event_1).expect("failed to log event 1");
  log.log_event(&event_2).expect("failed to log event 2");

  let events = log.get_events("rel-1", 50).expect("failed to get events");
  assert_eq!(events.len(), 2);
}
