//! Trust store for ed25519 signature verification.
//!
//! Holds the bundled Aeor public key(s) used by `crate::update` to
//! authenticate self-update manifests. Ported from xenocept-client's
//! `src/security/sig.rs`, trimmed to the trust-store concerns only —
//! the plugin-manifest (v2) verification code lives in xenocept because
//! aeordb-client has no plugin system.
//!
//! Key rotation discipline is the same as xenocept's:
//!   - Each bundled key has a filename `aeor-<YYYYMMDDHHMM>-public-key.bin`
//!     matching the moment its keypair was generated.
//!   - The first tuple element (key-id) is the protocol-level value that
//!     every signed manifest's `key_id` field MUST match.
//!   - During a rotation window both old and new keys live in the slice;
//!     once a new release of the client is universal, the old entry is
//!     dropped in the next client release.
//!
//! Tests register ephemeral keys via `register_test_key` so the real
//! verify path can authenticate test-only signatures without poisoning
//! a shared env var across parallel threads.

use std::sync::{Mutex, OnceLock};

/// Bundled production trust store. Each tuple is (key-id, 32-byte
/// ed25519 verifying-key bytes).
///
/// ROTATED 2026-05-13 (in xenocept): `aeor-202605122015` is COMPROMISED.
/// Its private bytes were committed to git (mislabeled as the public
/// key) before history was scrubbed. It was removed from the trust
/// store; any manifest still signed under that key-id will fail with
/// UnknownKeyId at verify time — the intended outcome, since those
/// signatures no longer represent genuine authorization.
pub static AEOR_PUBLIC_KEYS: &[(&str, &[u8])] = &[(
  "aeor-202605132323",
  include_bytes!("aeor-202605132323-public-key.bin"),
)];

/// In-process test-only key registry. Tests register ephemeral
/// verifying keys here so the real verify path can authenticate
/// signatures produced by test-side signing without touching env vars.
///
/// In production this OnceLock stays uninitialized; `effective_trust_store`
/// short-circuits the Mutex acquire on the common path.
static TEST_TRUST_STORE: OnceLock<Mutex<Vec<(String, [u8; 32])>>> = OnceLock::new();

/// Register an ephemeral test verifying key under the given key_id.
/// Idempotent: subsequent calls with the same id overwrite (so a test
/// can "rotate" its key without leaking earlier entries).
pub fn register_test_key(key_id: &str, verifying_key: [u8; 32]) {
  let mu = TEST_TRUST_STORE.get_or_init(|| Mutex::new(Vec::new()));
  let mut guard = mu.lock().expect("test trust store mutex poisoned");
  if let Some(slot) = guard.iter_mut().find(|(id, _)| id == key_id) {
    slot.1 = verifying_key;
  } else {
    guard.push((key_id.to_string(), verifying_key));
  }
}

/// Compiled-in keys plus anything registered in `TEST_TRUST_STORE`.
/// Returns owned tuples so callers don't worry about lifetimes across
/// the merge of static + dynamic.
pub fn effective_trust_store() -> Vec<(String, [u8; 32])> {
  let mut out: Vec<(String, [u8; 32])> = AEOR_PUBLIC_KEYS
    .iter()
    .filter_map(|(id, bytes)| {
      if bytes.len() != 32 {
        return None;
      }
      let mut arr = [0u8; 32];
      arr.copy_from_slice(bytes);
      Some((id.to_string(), arr))
    })
    .collect();

  if let Some(mu) = TEST_TRUST_STORE.get() {
    if let Ok(guard) = mu.lock() {
      out.extend(guard.iter().cloned());
    }
  }

  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn trust_store_contains_current_aeor_key() {
    assert_eq!(AEOR_PUBLIC_KEYS.len(), 1, "expected exactly one Aeor key");
    let (id, bytes) = AEOR_PUBLIC_KEYS[0];
    assert_eq!(id, "aeor-202605132323");
    assert_eq!(bytes.len(), 32, "ed25519 verifying-key is 32 raw bytes");
  }

  #[test]
  fn rotated_old_key_no_longer_trusted() {
    assert!(
      !AEOR_PUBLIC_KEYS
        .iter()
        .any(|(id, _)| *id == "aeor-202605122015"),
      "aeor-202605122015 is COMPROMISED and must not be re-added to the trust store",
    );
  }

  #[test]
  fn register_test_key_appears_in_effective_store() {
    let id = "test-sig-trust-store-1";
    let key = [42u8; 32];
    register_test_key(id, key);
    let store = effective_trust_store();
    assert!(store.iter().any(|(k, v)| k == id && v == &key));
  }
}
