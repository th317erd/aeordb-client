//! Per-installation user preferences.
//!
//! Lives in `{config_dir}/preferences.yaml` next to `config.yaml`. Two
//! files because they have different semantics:
//!
//! - `config.yaml` — operational configuration the user sets explicitly
//!   (connections, sync interval, auto-start). Edited via the Settings
//!   page or by hand.
//! - `preferences.yaml` — UI state the app remembers on the user's
//!   behalf (open tabs, view mode, default open-locally-vs-remotely).
//!   Edited automatically as the user clicks around; can also be
//!   hand-edited.
//!
//! Both files belong to **this install** — they intentionally don't
//! sync to the engine. If a future feature needs roaming-across-devices
//! state, that lives in a separate engine-backed store.
//!
//! The PATCH wire protocol is RFC 7396 JSON Merge Patch, matching the
//! engine's `/files` merge-patch endpoint, so renderer code that learns
//! merge-patch once works against both surfaces.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::{ClientError, Result};

// ---------------------------------------------------------------------------
// Typed schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserPreferences {
  #[serde(default)]
  pub file_browser: FileBrowserPrefs,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileBrowserPrefs {
  /// "local" or "remote" — drives the Open Locally/Open Remotely
  /// split-button default. Unrecognized values are ignored by the UI.
  #[serde(default)]
  pub open_default: Option<String>,

  #[serde(default)]
  pub show_hidden: bool,

  /// Open tabs in the order the user arranged them. Migrated wholesale
  /// from the previous `aeordb-file-browser` localStorage blob.
  #[serde(default)]
  pub tabs: Vec<FileBrowserTab>,

  /// JS-side tab IDs are strings like "tab-1" (assembled from
  /// `tab_counter` in aeor-file-browser-base.js::_openTab). Keep this
  /// as String so a numeric schema doesn't silently 400 the save.
  #[serde(default)]
  pub active_tab_id: Option<String>,

  #[serde(default)]
  pub tab_counter: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBrowserTab {
  pub id:                String,
  #[serde(default)]
  pub name:              Option<String>,
  #[serde(default)]
  pub path:              Option<String>,
  #[serde(default)]
  pub view_mode:         Option<String>,
  #[serde(default)]
  pub page_size:         Option<i64>,
  #[serde(default)]
  pub preview_height:    Option<i64>,
  #[serde(default)]
  pub relationship_id:   Option<String>,
  #[serde(default)]
  pub relationship_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct PreferencesStore {
  path:        PathBuf,
  preferences: RwLock<UserPreferences>,
}

impl PreferencesStore {
  /// Load preferences from `{config_dir}/preferences.yaml`. If the file
  /// doesn't exist, returns a default-empty store and writes the empty
  /// doc on the first save. If the file exists but is malformed, that's
  /// an error — we don't silently nuke the user's preferences.
  pub fn load(config_dir: &Path) -> Result<Self> {
    let path = config_dir.join("preferences.yaml");

    let preferences = if path.exists() {
      let contents = std::fs::read_to_string(&path).map_err(|error| {
        ClientError::Configuration(
          format!("failed to read preferences at {:?}: {}", path, error),
        )
      })?;

      // Empty file → defaults. serde_yaml chokes on an empty string.
      if contents.trim().is_empty() {
        UserPreferences::default()
      } else {
        serde_yaml::from_str(&contents).map_err(|error| {
          ClientError::Configuration(
            format!("failed to parse preferences at {:?}: {}", path, error),
          )
        })?
      }
    } else {
      UserPreferences::default()
    };

    Ok(Self {
      path,
      preferences: RwLock::new(preferences),
    })
  }

  pub async fn get(&self) -> UserPreferences {
    self.preferences.read().await.clone()
  }

  /// Apply an RFC 7396 JSON Merge Patch to the stored preferences and
  /// persist the result. The patch must round-trip through our typed
  /// schema — an unknown key or wrong type returns 400-equivalent and
  /// leaves the on-disk state untouched.
  pub async fn merge_patch(&self, patch: Value) -> Result<UserPreferences> {
    let mut prefs = self.preferences.write().await;

    // 1. Serialize current state to JSON so we can merge against it.
    let mut current = serde_json::to_value(&*prefs).map_err(|error| {
      ClientError::Server(format!("failed to serialize preferences: {}", error))
    })?;

    // 2. Apply RFC 7396 merge in-place.
    apply_merge_patch(&mut current, patch);

    // 3. Round-trip through the typed schema to validate. This catches
    //    unknown keys (if serde_with #[serde(deny_unknown_fields)] is
    //    set — we don't, so typos are silently dropped) and wrong types
    //    (a string where a bool was expected). The current schema is
    //    permissive by design; if we want strict, add deny_unknown_fields.
    let merged: UserPreferences = serde_json::from_value(current).map_err(|error| {
      ClientError::BadRequest(format!("preferences patch produced invalid state: {}", error))
    })?;

    // 4. Persist + commit in-memory.
    self.save_inner(&merged)?;
    *prefs = merged.clone();
    Ok(merged)
  }

  fn save_inner(&self, prefs: &UserPreferences) -> Result<()> {
    if let Some(parent) = self.path.parent() {
      std::fs::create_dir_all(parent).map_err(|error| {
        ClientError::Configuration(
          format!("failed to create preferences directory {:?}: {}", parent, error),
        )
      })?;
    }

    let yaml = serde_yaml::to_string(prefs).map_err(|error| {
      ClientError::Configuration(format!("failed to serialize preferences: {}", error))
    })?;

    std::fs::write(&self.path, yaml).map_err(|error| {
      ClientError::Configuration(
        format!("failed to write preferences to {:?}: {}", self.path, error),
      )
    })?;

    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
  }
}

// ---------------------------------------------------------------------------
// RFC 7396 Merge Patch
// ---------------------------------------------------------------------------

/// Apply a JSON Merge Patch per RFC 7396.
///
/// - Patch is an object: each key recursively merges into target, with
///   `null` deleting and non-objects replacing.
/// - Patch is anything else: replaces target wholesale.
///
/// No depth bounding here — preferences are small and shallow, and the
/// schema is fixed, so unbounded recursion is fine.
fn apply_merge_patch(target: &mut Value, patch: Value) {
  match patch {
    Value::Object(patch_map) => {
      if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
      }
      let target_map = target.as_object_mut().expect("just ensured target is object");
      for (key, value) in patch_map {
        if value.is_null() {
          target_map.remove(&key);
        } else if value.is_object() {
          // Recurse into the target's value at this key.
          let entry = target_map.entry(key).or_insert(Value::Null);
          apply_merge_patch(entry, value);
        } else {
          target_map.insert(key, value);
        }
      }
    }
    other => {
      *target = other;
    }
  }
}
