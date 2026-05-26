//! Process-wide JWT cache keyed by connection_id.
//!
//! Without this, every API handler that built a `RemoteClient` got a
//! fresh empty cache and hit `POST /auth/token` on the engine on every
//! request — argon2-verifying the API key + (until the engine team
//! shipped an opt-out) minting a fresh refresh-token row per call.
//! The dashboard's 15s `/system/stats` poll alone produced 5,760 token
//! exchanges per day per connection.
//!
//! Now: one shared slot per connection, lives in `AppState`. The first
//! request mints; subsequent requests reuse. On 401 the slot is cleared
//! and the next request re-mints (see `remote/mod.rs::invalidate_token`).
//!
//! The slot type is `Arc<std::sync::Mutex<Option<String>>>` deliberately
//! — sync Mutex (not tokio) because the critical section is just a
//! string clone/replace, and `RemoteClient::auth_header` is already
//! using sync Mutex internally so swapping in this shared slot is a
//! zero-friction drop-in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// Per-connection JWT slot. `None` = not yet exchanged (or recently
/// invalidated). `Some(token)` = the raw JWT, ready to wrap in
/// `"Bearer …"`.
pub type JwtSlot = Arc<Mutex<Option<String>>>;

/// Shared map of `connection_id -> JwtSlot`. Cloning is cheap (Arc bump);
/// the inner RwLock protects insertions. Slots themselves use a separate
/// Mutex so two requests against the same connection don't fight over
/// the outer RwLock once their slot exists.
#[derive(Clone, Default)]
pub struct JwtCache {
  slots: Arc<RwLock<HashMap<String, JwtSlot>>>,
}

impl JwtCache {
  pub fn new() -> Self {
    Self::default()
  }

  /// Return the existing slot for `connection_id`, or create one if
  /// missing. Subsequent calls with the same id share the same slot
  /// (cloning the Arc), so a token minted by one request is visible to
  /// the next.
  pub fn slot_for(&self, connection_id: &str) -> JwtSlot {
    // Fast path: read lock, slot already exists.
    if let Some(slot) = self.slots.read().expect("jwt cache poisoned").get(connection_id) {
      return slot.clone();
    }
    // Slow path: upgrade to write lock, double-check (another caller may
    // have inserted while we were upgrading), insert if still missing.
    let mut guard = self.slots.write().expect("jwt cache poisoned");
    guard.entry(connection_id.to_string())
      .or_insert_with(|| Arc::new(Mutex::new(None)))
      .clone()
  }

  /// Drop a slot entirely (e.g. when the connection is deleted from
  /// config). Pending tokens for that connection are discarded.
  #[allow(dead_code)]
  pub fn drop_slot(&self, connection_id: &str) {
    let mut guard = self.slots.write().expect("jwt cache poisoned");
    guard.remove(connection_id);
  }
}
