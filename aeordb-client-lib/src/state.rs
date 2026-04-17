use std::path::Path;
use std::sync::Arc;

use aeordb::engine::{DirectoryOps, RequestContext, StorageEngine};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ClientError, Result};

/// The local state store backed by an embedded aeordb instance.
/// Stores client identity, connection configs, sync relationships,
/// per-file sync state, conflicts, and settings.
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
  pub fn open_or_create(database_path: &str) -> Result<Self> {
    let path = Path::new(database_path);

    let engine = if path.exists() {
      StorageEngine::open(database_path)
        .map_err(|error| ClientError::Configuration(
          format!("failed to open state database at {}: {}", database_path, error),
        ))?
    } else {
      // Ensure parent directory exists
      if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
      }

      StorageEngine::create(database_path)
        .map_err(|error| ClientError::Configuration(
          format!("failed to create state database at {}: {}", database_path, error),
        ))?
    };

    let engine = Arc::new(engine);
    let ops    = DirectoryOps::new(&engine);
    let ctx    = RequestContext::system();

    ops
      .ensure_root_directory(&ctx)
      .map_err(|error| ClientError::Configuration(
        format!("failed to initialize state database root: {}", error),
      ))?;

    let store = Self { engine };
    store.ensure_directory_structure()?;

    Ok(store)
  }

  /// Create a DirectoryOps handle for this store.
  fn ops(&self) -> DirectoryOps<'_> {
    DirectoryOps::new(&self.engine)
  }

  /// Ensure the required directory structure exists in the local state db.
  /// Note: connections and relationships are now stored in the YAML config
  /// file, not in the state database.
  fn ensure_directory_structure(&self) -> Result<()> {
    let directories = [
      "/client/",
      "/sync/",
      "/sync/state/",
      "/sync/conflicts/",
      "/settings/",
    ];

    for directory_path in &directories {
      if !self.exists(directory_path)? {
        // Store a placeholder file to create the directory implicitly.
        // aeordb creates parent directories when you store a file.
        let placeholder_path = format!("{}/.keep", directory_path);
        self.store_json(&placeholder_path, &serde_json::json!({}))?;
      }
    }

    Ok(())
  }

  /// Get or create the client identity.
  pub fn get_or_create_identity(&self) -> Result<ClientIdentity> {
    let identity_path = "/client/identity.json";

    match self.read_json::<ClientIdentity>(identity_path)? {
      Some(identity) => Ok(identity),
      None => {
        let identity = ClientIdentity {
          id:   Uuid::new_v4().to_string(),
          name: hostname(),
        };

        self.store_json(identity_path, &identity)?;
        tracing::info!("generated client identity: {} ({})", identity.id, identity.name);

        Ok(identity)
      }
    }
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
  /// Returns None if the file does not exist.
  pub fn read_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<Option<T>> {
    if !self.exists(path)? {
      return Ok(None);
    }

    let data = self.ops()
      .read_file(path)
      .map_err(|error| ClientError::Server(
        format!("failed to read {} from state database: {}", path, error),
      ))?;

    let value = serde_json::from_slice(&data)?;
    Ok(Some(value))
  }

  /// Delete a file from the local database.
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
  pub fn exists(&self, path: &str) -> Result<bool> {
    self.ops()
      .exists(path)
      .map_err(|error| ClientError::Server(
        format!("failed to check existence of {} in state database: {}", path, error),
      ))
  }

  /// List entries in a directory.
  pub fn list_directory(&self, path: &str) -> Result<Vec<String>> {
    let children = self.ops()
      .list_directory(path)
      .map_err(|error| ClientError::Server(
        format!("failed to list {} in state database: {}", path, error),
      ))?;

    Ok(children.into_iter().map(|child| child.name).collect())
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
