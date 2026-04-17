use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::ConfigStore;
use crate::connections::ConnectionManager;
use crate::error::{ClientError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
  Bidirectional,
  PullOnly,
  PushOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePropagation {
  pub local_to_remote: bool,
  pub remote_to_local: bool,
}

impl Default for DeletePropagation {
  fn default() -> Self {
    Self {
      local_to_remote: false,
      remote_to_local: false,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRelationship {
  pub id:                   String,
  pub name:                 String,
  pub remote_connection_id: String,
  pub remote_path:          String,
  pub local_path:           String,
  pub direction:            SyncDirection,
  pub filter:               Option<String>,
  pub delete_propagation:   DeletePropagation,
  pub enabled:              bool,
  pub created_at:           DateTime<Utc>,
  pub updated_at:           DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSyncRelationshipRequest {
  pub name:                 String,
  pub remote_connection_id: String,
  pub remote_path:          String,
  pub local_path:           String,
  pub direction:            SyncDirection,
  pub filter:               Option<String>,
  pub delete_propagation:   Option<DeletePropagation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSyncRelationshipRequest {
  pub name:               Option<String>,
  pub direction:          Option<SyncDirection>,
  pub filter:             Option<String>,
  pub delete_propagation: Option<DeletePropagation>,
  pub enabled:            Option<bool>,
}

/// Manages sync relationships, persisted in the YAML config file.
pub struct RelationshipManager<'a> {
  config: &'a ConfigStore,
}

impl<'a> RelationshipManager<'a> {
  pub fn new(config: &'a ConfigStore) -> Self {
    Self { config }
  }

  pub fn create(&self, request: CreateSyncRelationshipRequest) -> Result<SyncRelationship> {
    // Validate that the referenced connection exists
    let connection_manager = ConnectionManager::new(self.config);
    if connection_manager.get(&request.remote_connection_id)?.is_none() {
      return Err(ClientError::Configuration(
        format!("connection not found: {}", request.remote_connection_id),
      ));
    }

    // Validate local path exists or can be created
    let local_path = std::path::Path::new(&request.local_path);
    if !local_path.exists() {
      std::fs::create_dir_all(local_path).map_err(|error| {
        ClientError::Configuration(
          format!("cannot create local path '{}': {}", request.local_path, error),
        )
      })?;
      tracing::info!("created local sync directory: {}", request.local_path);
    }

    if !local_path.is_dir() {
      return Err(ClientError::Configuration(
        format!("local path is not a directory: {}", request.local_path),
      ));
    }

    // Normalize remote path: ensure leading slash, ensure trailing slash
    let remote_path = normalize_remote_path(&request.remote_path);

    let now = Utc::now();
    let relationship = SyncRelationship {
      id:                   Uuid::new_v4().to_string(),
      name:                 request.name,
      remote_connection_id: request.remote_connection_id,
      remote_path,
      local_path:           request.local_path,
      direction:            request.direction,
      filter:               request.filter,
      delete_propagation:   request.delete_propagation.unwrap_or_default(),
      enabled:              true,
      created_at:           now,
      updated_at:           now,
    };

    let new_relationship = relationship.clone();
    self.config.update(|config| {
      config.relationships.push(new_relationship);
    })?;

    tracing::info!(
      "created sync relationship '{}' ({}) -- {} {} <-> {}",
      relationship.name,
      relationship.id,
      relationship.remote_connection_id,
      relationship.remote_path,
      relationship.local_path,
    );

    Ok(relationship)
  }

  pub fn list(&self) -> Result<Vec<SyncRelationship>> {
    let config = self.config.get()?;
    let mut relationships = config.relationships;
    relationships.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(relationships)
  }

  pub fn get(&self, id: &str) -> Result<Option<SyncRelationship>> {
    let config = self.config.get()?;
    Ok(config.relationships.into_iter().find(|relationship| relationship.id == id))
  }

  pub fn update(&self, id: &str, request: UpdateSyncRelationshipRequest) -> Result<SyncRelationship> {
    let mut updated_relationship = None;

    self.config.update(|config| {
      let Some(relationship) = config.relationships.iter_mut().find(|r| r.id == id) else {
        return;
      };

      if let Some(name) = request.name {
        relationship.name = name;
      }
      if let Some(direction) = request.direction {
        relationship.direction = direction;
      }
      if let Some(filter) = request.filter {
        relationship.filter = Some(filter);
      }
      if let Some(delete_propagation) = request.delete_propagation {
        relationship.delete_propagation = delete_propagation;
      }
      if let Some(enabled) = request.enabled {
        relationship.enabled = enabled;
      }

      relationship.updated_at = Utc::now();
      updated_relationship = Some(relationship.clone());
    })?;

    match updated_relationship {
      Some(relationship) => {
        tracing::info!("updated sync relationship '{}' ({})", relationship.name, relationship.id);
        Ok(relationship)
      }
      None => Err(ClientError::Configuration(
        format!("sync relationship not found: {}", id),
      )),
    }
  }

  pub fn delete(&self, id: &str) -> Result<()> {
    let mut found = false;

    self.config.update(|config| {
      let before = config.relationships.len();
      config.relationships.retain(|relationship| relationship.id != id);
      found = config.relationships.len() < before;
    })?;

    if !found {
      return Err(ClientError::Configuration(
        format!("sync relationship not found: {}", id),
      ));
    }

    tracing::info!("deleted sync relationship {}", id);
    Ok(())
  }

  pub fn enable(&self, id: &str) -> Result<SyncRelationship> {
    self.update(id, UpdateSyncRelationshipRequest {
      name: None, direction: None, filter: None,
      delete_propagation: None, enabled: Some(true),
    })
  }

  pub fn disable(&self, id: &str) -> Result<SyncRelationship> {
    self.update(id, UpdateSyncRelationshipRequest {
      name: None, direction: None, filter: None,
      delete_propagation: None, enabled: Some(false),
    })
  }
}

fn normalize_remote_path(path: &str) -> String {
  let mut normalized = path.to_string();

  if !normalized.starts_with('/') {
    normalized = format!("/{}", normalized);
  }

  if !normalized.ends_with('/') {
    normalized = format!("{}/", normalized);
  }

  normalized
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_normalize_remote_path() {
    assert_eq!(normalize_remote_path("docs"), "/docs/");
    assert_eq!(normalize_remote_path("/docs"), "/docs/");
    assert_eq!(normalize_remote_path("/docs/"), "/docs/");
    assert_eq!(normalize_remote_path("docs/"), "/docs/");
    assert_eq!(normalize_remote_path("/"), "/");
  }
}
