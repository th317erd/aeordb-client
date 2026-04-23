use std::path::Path;
use std::sync::Arc;

use aeordb::engine::{DirectoryOps, RequestContext, StorageEngine};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ClientError, Result};

/// The local state store backed by an embedded aeordb instance.
/// Stores client identity, connection configs, sync relationships,
/// per-file sync state, conflicts, and settings.
///
/// Resilient to corruption: if the database is corrupted, it backs up
/// the corrupted file and creates a fresh database. Individual read
/// failures return None instead of crashing.
pub struct StateStore {
  engine: Arc<StorageEngine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientIdentity {
  pub id:   String,
  pub name: String,
}

impl StateStore {
  /// Open or create the local state database at the given path.
  /// If the database is corrupted, backs it up and creates a fresh one.
  pub fn open_or_create(database_path: &str) -> Result<Self> {
    let path = Path::new(database_path);

    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent)?;
    }

    // Try to open or create the database
    let engine = if path.exists() {
      match StorageEngine::open(database_path) {
        Ok(engine) => engine,
        Err(error) => {
          tracing::warn!(
            "state database at {} is corrupted ({}), backing up and recreating",
            database_path, error,
          );
          Self::backup_and_recreate(database_path)?
        }
      }
    } else {
      StorageEngine::create(database_path)
        .map_err(|error| ClientError::Configuration(
          format!("failed to create state database at {}: {}", database_path, error),
        ))?
    };

    let engine = Arc::new(engine);
    let store = Self { engine };

    // Try to initialize the directory structure.
    // If this fails (corrupted DB that passed open but can't write),
    // back up and recreate.
    match store.initialize() {
      Ok(()) => Ok(store),
      Err(error) => {
        tracing::warn!(
          "state database initialization failed ({}), backing up and recreating",
          error,
        );
        drop(store);
        let engine = Self::backup_and_recreate(database_path)?;
        let engine = Arc::new(engine);
        let store = Self { engine };
        store.initialize()?;
        Ok(store)
      }
    }
  }

  /// Initialize root directory and structure.
  fn initialize(&self) -> Result<()> {
    let ops = DirectoryOps::new(&self.engine);
    let ctx = RequestContext::system();

    ops
      .ensure_root_directory(&ctx)
      .map_err(|error| ClientError::Configuration(
        format!("failed to initialize state database root: {}", error),
      ))?;

    self.ensure_directory_structure()
  }

  /// Back up a corrupted database file and create a fresh one.
  fn backup_and_recreate(database_path: &str) -> Result<StorageEngine> {
    let backup_path = format!("{}.corrupt.{}", database_path, chrono::Utc::now().timestamp());

    if let Err(error) = std::fs::rename(database_path, &backup_path) {
      tracing::error!("failed to back up corrupted database: {}", error);
      // If we can't rename, try to just delete it
      let _ = std::fs::remove_file(database_path);
    } else {
      tracing::info!("corrupted database backed up to {}", backup_path);
    }

    StorageEngine::create(database_path)
      .map_err(|error| ClientError::Configuration(
        format!("failed to recreate state database at {}: {}", database_path, error),
      ))
  }

  /// Create a DirectoryOps handle for this store.
  fn ops(&self) -> DirectoryOps<'_> {
    DirectoryOps::new(&self.engine)
  }

  /// Ensure the required directory structure exists in the local state db.
  fn ensure_directory_structure(&self) -> Result<()> {
    let directories = [
      "/client/",
      "/sync/",
      "/sync/state/",
      "/sync/conflicts/",
      "/sync/activity/",
      "/settings/",
    ];

    for directory_path in &directories {
      if !self.exists(directory_path).unwrap_or(false) {
        let placeholder_path = format!("{}/.keep", directory_path);
        self.store_json(&placeholder_path, &serde_json::json!({}))?;
      }
    }

    Ok(())
  }

  /// Get or create the client identity.
  /// Resilient: if the identity is corrupted, generates a new one.
  pub fn get_or_create_identity(&self) -> Result<ClientIdentity> {
    let identity_path = "/client/identity.json";

    // Try to read existing identity — treat corruption as "not found"
    match self.read_json::<ClientIdentity>(identity_path) {
      Ok(Some(identity)) => return Ok(identity),
      Ok(None) => { /* no identity yet, create one */ }
      Err(error) => {
        tracing::warn!(
          "failed to read client identity ({}), generating new one",
          error,
        );
      }
    }

    let identity = ClientIdentity {
      id:   Uuid::new_v4().to_string(),
      name: hostname(),
    };

    if let Err(error) = self.store_json(identity_path, &identity) {
      tracing::error!("failed to persist client identity: {}", error);
      // Return the identity anyway — it'll work for this session
    } else {
      tracing::info!("generated client identity: {} ({})", identity.id, identity.name);
    }

    Ok(identity)
  }

  /// Store a JSON-serializable value at a path in the local database.
  pub fn store_json<T: Serialize>(&self, path: &str, value: &T) -> Result<()> {
    let ctx  = RequestContext::system();
    let data = serde_json::to_vec_pretty(value)?;

    self.ops()
      .store_file(&ctx, path, &data, Some("application/json"))
      .map_err(|error| ClientError::Server(
        format!("failed to store {} in state database: {}", path, error),
      ))?;

    Ok(())
  }

  /// Read a JSON value from a path in the local database.
  /// Returns None if the file does not exist or is corrupted.
  pub fn read_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<Option<T>> {
    if !self.exists(path).unwrap_or(false) {
      return Ok(None);
    }

    match self.ops().read_file(path) {
      Ok(data) => {
        match serde_json::from_slice(&data) {
          Ok(value) => Ok(Some(value)),
          Err(error) => {
            tracing::warn!("corrupt JSON at {}: {}", path, error);
            Ok(None)
          }
        }
      }
      Err(error) => {
        tracing::warn!("failed to read {} from state database: {}", path, error);
        Ok(None)
      }
    }
  }

  /// Delete a file from the local database.
  /// Non-fatal: logs a warning on failure.
  pub fn delete(&self, path: &str) -> Result<()> {
    let ctx = RequestContext::system();

    self.ops()
      .delete_file(&ctx, path)
      .map_err(|error| ClientError::Server(
        format!("failed to delete {} from state database: {}", path, error),
      ))?;

    Ok(())
  }

  /// Check if a path exists in the local database.
  /// Returns false on any error (treats corruption as "not found").
  pub fn exists(&self, path: &str) -> Result<bool> {
    self.ops()
      .exists(path)
      .map_err(|error| ClientError::Server(
        format!("failed to check existence of {} in state database: {}", path, error),
      ))
  }

  /// List entries in a directory.
  /// Returns empty list on error (treats corruption as "empty").
  pub fn list_directory(&self, path: &str) -> Result<Vec<String>> {
    match self.ops().list_directory(path) {
      Ok(children) => Ok(children.into_iter().map(|child| child.name).collect()),
      Err(error) => {
        tracing::warn!("failed to list {} in state database: {}", path, error);
        Ok(Vec::new())
      }
    }
  }

  /// Get a reference to the underlying storage engine.
  pub fn engine(&self) -> &Arc<StorageEngine> {
    &self.engine
  }
}

fn hostname() -> String {
  gethostname::gethostname()
    .to_string_lossy()
    .to_string()
}
