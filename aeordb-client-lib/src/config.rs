use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::connections::RemoteConnection;
use crate::error::{ClientError, Result};
use crate::sync::relationships::SyncRelationship;

/// Client-level settings (sync interval, auto-start, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSettings {
  #[serde(default = "default_sync_interval")]
  pub sync_interval_seconds: u64,
  #[serde(default = "default_auto_start_sync")]
  pub auto_start_sync: bool,
  #[serde(default)]
  pub client_name: Option<String>,
}

fn default_sync_interval() -> u64 { 60 }
fn default_auto_start_sync() -> bool { true }

impl Default for ClientSettings {
  fn default() -> Self {
    Self {
      sync_interval_seconds: 60,
      auto_start_sync: true,
      client_name: None,
    }
  }
}

/// Human-editable configuration stored as YAML.
/// Contains connections and sync relationships.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
  #[serde(default)]
  pub connections:   Vec<RemoteConnection>,
  #[serde(default)]
  pub relationships: Vec<SyncRelationship>,
  #[serde(default)]
  pub settings:      ClientSettings,
}

impl Default for ClientConfig {
  fn default() -> Self {
    Self {
      connections:   Vec::new(),
      relationships: Vec::new(),
      settings:      ClientSettings::default(),
    }
  }
}

/// Thread-safe config store that reads/writes YAML.
pub struct ConfigStore {
  config_path: PathBuf,
  config:      RwLock<ClientConfig>,
}

impl ConfigStore {
  /// Load config from the given YAML file path.
  /// Creates a default config if the file does not exist.
  pub fn load(config_path: &Path) -> Result<Self> {
    if let Some(parent) = config_path.parent() {
      std::fs::create_dir_all(parent).map_err(|error| {
        ClientError::Configuration(
          format!("failed to create config directory {:?}: {}", parent, error),
        )
      })?;
    }

    let config = if config_path.exists() {
      let contents = std::fs::read_to_string(config_path).map_err(|error| {
        ClientError::Configuration(
          format!("failed to read config at {:?}: {}", config_path, error),
        )
      })?;

      serde_yaml::from_str(&contents).map_err(|error| {
        ClientError::Configuration(
          format!("failed to parse config at {:?}: {}", config_path, error),
        )
      })?
    } else {
      let default_config = ClientConfig::default();
      let store = Self {
        config_path: config_path.to_path_buf(),
        config:      RwLock::new(default_config.clone()),
      };
      store.save_inner(&default_config)?;
      default_config
    };

    Ok(Self {
      config_path: config_path.to_path_buf(),
      config:      RwLock::new(config),
    })
  }

  /// Save the current config to disk.
  pub fn save(&self) -> Result<()> {
    let config = self.config.read().map_err(|error| {
      ClientError::Configuration(format!("config lock poisoned: {}", error))
    })?;
    self.save_inner(&config)
  }

  fn save_inner(&self, config: &ClientConfig) -> Result<()> {
    let yaml = serde_yaml::to_string(config).map_err(|error| {
      ClientError::Configuration(
        format!("failed to serialize config: {}", error),
      )
    })?;

    std::fs::write(&self.config_path, yaml).map_err(|error| {
      ClientError::Configuration(
        format!("failed to write config to {:?}: {}", self.config_path, error),
      )
    })?;

    Ok(())
  }

  /// Get the config file path.
  pub fn config_path(&self) -> &Path {
    &self.config_path
  }

  /// Get a snapshot of the current config.
  pub fn get(&self) -> Result<ClientConfig> {
    let config = self.config.read().map_err(|error| {
      ClientError::Configuration(format!("config lock poisoned: {}", error))
    })?;
    Ok(config.clone())
  }

  /// Update the config with a closure and save to disk.
  pub fn update<F>(&self, updater: F) -> Result<()>
  where
    F: FnOnce(&mut ClientConfig),
  {
    let mut config = self.config.write().map_err(|error| {
      ClientError::Configuration(format!("config lock poisoned: {}", error))
    })?;
    updater(&mut config);
    self.save_inner(&config)
  }
}

/// Return the default XDG config directory for aeordb-client.
pub fn default_config_dir() -> PathBuf {
  dirs::config_dir()
    .unwrap_or_else(|| PathBuf::from("."))
    .join("aeordb-client")
}

/// Return the default config file path.
pub fn default_config_path() -> PathBuf {
  default_config_dir().join("config.yaml")
}

/// Return the default XDG data directory for aeordb-client.
pub fn default_data_dir() -> PathBuf {
  dirs::data_dir()
    .unwrap_or_else(|| PathBuf::from("."))
    .join("aeordb-client")
}

/// Return the default state database path.
pub fn default_data_path() -> PathBuf {
  default_data_dir().join("state.aeordb")
}
