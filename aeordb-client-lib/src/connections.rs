use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ClientError, Result};
use crate::state::StateStore;

const CONNECTIONS_PATH: &str = "/connections/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
  ApiKey,
  None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConnection {
  pub id:         String,
  pub name:       String,
  pub url:        String,
  pub auth_type:  AuthType,
  pub api_key:    Option<String>,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConnectionRequest {
  pub name:      String,
  pub url:       String,
  pub auth_type: AuthType,
  pub api_key:   Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConnectionRequest {
  pub name:      Option<String>,
  pub url:       Option<String>,
  pub auth_type: Option<AuthType>,
  pub api_key:   Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionTestResult {
  pub success: bool,
  pub message: String,
  pub latency_ms: Option<u64>,
}

/// Manages remote aeordb connections, persisted in the local state store.
pub struct ConnectionManager<'a> {
  state: &'a StateStore,
}

impl<'a> ConnectionManager<'a> {
  pub fn new(state: &'a StateStore) -> Self {
    Self { state }
  }

  pub fn create(&self, request: CreateConnectionRequest) -> Result<RemoteConnection> {
    let now = Utc::now();

    // Normalize URL: strip trailing slash
    let url = request.url.trim_end_matches('/').to_string();

    let connection = RemoteConnection {
      id:         Uuid::new_v4().to_string(),
      name:       request.name,
      url,
      auth_type:  request.auth_type,
      api_key:    request.api_key,
      created_at: now,
      updated_at: now,
    };

    let path = format!("{}{}.json", CONNECTIONS_PATH, connection.id);
    self.state.store_json(&path, &connection)?;

    tracing::info!("created connection '{}' ({})", connection.name, connection.id);
    Ok(connection)
  }

  pub fn list(&self) -> Result<Vec<RemoteConnection>> {
    let entries = self.state.list_directory(CONNECTIONS_PATH)?;
    let mut connections = Vec::new();

    for entry_name in entries {
      if !entry_name.ends_with(".json") || entry_name == ".keep" {
        continue;
      }

      let path = format!("{}{}", CONNECTIONS_PATH, entry_name);
      if let Some(connection) = self.state.read_json::<RemoteConnection>(&path)? {
        connections.push(connection);
      }
    }

    connections.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(connections)
  }

  pub fn get(&self, id: &str) -> Result<Option<RemoteConnection>> {
    let path = format!("{}{}.json", CONNECTIONS_PATH, id);
    self.state.read_json(&path)
  }

  pub fn update(&self, id: &str, request: UpdateConnectionRequest) -> Result<RemoteConnection> {
    let path = format!("{}{}.json", CONNECTIONS_PATH, id);

    let mut connection = self.state.read_json::<RemoteConnection>(&path)?
      .ok_or_else(|| ClientError::Configuration(
        format!("connection not found: {}", id),
      ))?;

    if let Some(name) = request.name {
      connection.name = name;
    }
    if let Some(url) = request.url {
      connection.url = url.trim_end_matches('/').to_string();
    }
    if let Some(auth_type) = request.auth_type {
      connection.auth_type = auth_type;
    }
    if let Some(api_key) = request.api_key {
      connection.api_key = Some(api_key);
    }

    connection.updated_at = Utc::now();
    self.state.store_json(&path, &connection)?;

    tracing::info!("updated connection '{}' ({})", connection.name, connection.id);
    Ok(connection)
  }

  pub fn delete(&self, id: &str) -> Result<()> {
    let path = format!("{}{}.json", CONNECTIONS_PATH, id);

    if !self.state.exists(&path)? {
      return Err(ClientError::Configuration(
        format!("connection not found: {}", id),
      ));
    }

    self.state.delete(&path)?;
    tracing::info!("deleted connection {}", id);
    Ok(())
  }

  /// Test connectivity to a remote aeordb instance.
  pub async fn test_connection(&self, id: &str) -> Result<ConnectionTestResult> {
    let connection = self.get(id)?
      .ok_or_else(|| ClientError::Configuration(
        format!("connection not found: {}", id),
      ))?;

    let health_url = format!("{}/admin/health", connection.url);
    let client     = reqwest::Client::new();

    let start = std::time::Instant::now();
    let mut request_builder = client.get(&health_url);

    if connection.auth_type == AuthType::ApiKey {
      if let Some(ref api_key) = connection.api_key {
        request_builder = request_builder.header("Authorization", format!("Bearer {}", api_key));
      }
    }

    match tokio::time::timeout(
      std::time::Duration::from_secs(10),
      request_builder.send(),
    ).await {
      Ok(Ok(response)) => {
        let latency = start.elapsed().as_millis() as u64;

        if response.status().is_success() {
          Ok(ConnectionTestResult {
            success:    true,
            message:    format!("connected (HTTP {})", response.status().as_u16()),
            latency_ms: Some(latency),
          })
        } else {
          Ok(ConnectionTestResult {
            success:    false,
            message:    format!("server returned HTTP {}", response.status().as_u16()),
            latency_ms: Some(latency),
          })
        }
      }
      Ok(Err(error)) => {
        Ok(ConnectionTestResult {
          success:    false,
          message:    format!("connection failed: {}", error),
          latency_ms: None,
        })
      }
      Err(_) => {
        Ok(ConnectionTestResult {
          success:    false,
          message:    "connection timed out (10s)".to_string(),
          latency_ms: None,
        })
      }
    }
  }
}
