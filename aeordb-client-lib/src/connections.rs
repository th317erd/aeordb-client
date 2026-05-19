use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::ConfigStore;
use crate::error::{ClientError, Result};

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
  #[serde(default)]
  pub share_base_url: Option<String>,
  pub created_at: DateTime<Utc>,
  pub updated_at: DateTime<Utc>,
}

impl RemoteConnection {
  /// Normalized base URL for HTTP requests — guarantees a scheme and no
  /// trailing slash, even if the stored value predates input normalization.
  pub fn base_url(&self) -> String {
    normalize_url(&self.url)
  }

  /// The base URL to use when generating share links.
  /// Falls back to the connection URL if no explicit share domain is set.
  pub fn effective_share_url(&self) -> String {
    let raw = self.share_base_url.as_deref()
      .filter(|s| !s.is_empty())
      .unwrap_or(&self.url);
    normalize_url(raw)
  }
}

/// Normalize a user-supplied base URL: trim whitespace and trailing slashes,
/// and prepend `http://` if no scheme is present. Bare `host:port` strings
/// like `localhost:6830` would otherwise fail reqwest's URL parser.
fn normalize_url(input: &str) -> String {
  let trimmed = input.trim().trim_end_matches('/');
  if trimmed.is_empty() {
    return String::new();
  }
  if trimmed.contains("://") {
    trimmed.to_string()
  } else {
    format!("http://{}", trimmed)
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConnectionRequest {
  pub name:      String,
  pub url:       String,
  pub auth_type: AuthType,
  pub api_key:   Option<String>,
  pub share_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConnectionRequest {
  pub name:      Option<String>,
  pub url:       Option<String>,
  pub auth_type: Option<AuthType>,
  pub api_key:   Option<String>,
  pub share_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionTestResult {
  pub success: bool,
  pub message: String,
  pub latency_ms: Option<u64>,
}

/// Manages remote aeordb connections, persisted in the YAML config file.
pub struct ConnectionManager<'a> {
  config: &'a ConfigStore,
}

impl<'a> ConnectionManager<'a> {
  pub fn new(config: &'a ConfigStore) -> Self {
    Self { config }
  }

  pub async fn create(&self, request: CreateConnectionRequest) -> Result<RemoteConnection> {
    let now = Utc::now();

    let url = normalize_url(&request.url);

    let connection = RemoteConnection {
      id:         Uuid::new_v4().to_string(),
      name:       request.name,
      url,
      auth_type:  request.auth_type,
      api_key:    request.api_key,
      share_base_url: request.share_base_url,
      created_at: now,
      updated_at: now,
    };

    let new_connection = connection.clone();
    self.config.update(|config| {
      config.connections.push(new_connection);
    }).await?;

    tracing::info!("created connection '{}' ({})", connection.name, connection.id);
    Ok(connection)
  }

  pub async fn list(&self) -> Result<Vec<RemoteConnection>> {
    let config = self.config.get().await?;
    let mut connections = config.connections;
    for connection in &mut connections {
      connection.url = normalize_url(&connection.url);
    }
    connections.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(connections)
  }

  pub async fn get(&self, id: &str) -> Result<Option<RemoteConnection>> {
    let config = self.config.get().await?;
    Ok(config.connections.into_iter()
      .find(|connection| connection.id == id)
      .map(|mut connection| {
        connection.url = normalize_url(&connection.url);
        connection
      }))
  }

  pub async fn update(&self, id: &str, request: UpdateConnectionRequest) -> Result<RemoteConnection> {
    let mut updated_connection = None;

    self.config.update(|config| {
      let Some(connection) = config.connections.iter_mut().find(|c| c.id == id) else {
        return;
      };

      if let Some(name) = request.name {
        connection.name = name;
      }
      if let Some(url) = request.url {
        connection.url = normalize_url(&url);
      }
      if let Some(auth_type) = request.auth_type {
        connection.auth_type = auth_type;
      }
      if let Some(api_key) = request.api_key {
        connection.api_key = Some(api_key);
      }
      if let Some(share_base_url) = request.share_base_url {
        connection.share_base_url = Some(share_base_url).filter(|s| !s.is_empty());
      }

      connection.updated_at = Utc::now();
      updated_connection = Some(connection.clone());
    }).await?;

    match updated_connection {
      Some(connection) => {
        tracing::info!("updated connection '{}' ({})", connection.name, connection.id);
        Ok(connection)
      }
      None => Err(ClientError::NotFound(
        format!("connection not found: {}", id),
      )),
    }
  }

  pub async fn delete(&self, id: &str) -> Result<()> {
    let mut found = false;

    self.config.update(|config| {
      let before = config.connections.len();
      config.connections.retain(|connection| connection.id != id);
      found = config.connections.len() < before;
    }).await?;

    if !found {
      return Err(ClientError::NotFound(
        format!("connection not found: {}", id),
      ));
    }

    tracing::info!("deleted connection {}", id);
    Ok(())
  }

  /// Test connectivity to a remote aeordb instance.
  pub async fn test_connection(&self, id: &str) -> Result<ConnectionTestResult> {
    let connection = self.get(id).await?
      .ok_or_else(|| ClientError::NotFound(
        format!("connection not found: {}", id),
      ))?;

    let health_url = format!("{}/system/health", connection.base_url());
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn normalize_url_adds_http_scheme_when_missing() {
    assert_eq!(normalize_url("localhost:6830"), "http://localhost:6830");
    assert_eq!(normalize_url("127.0.0.1:6830"), "http://127.0.0.1:6830");
    assert_eq!(normalize_url("example.com"), "http://example.com");
  }

  #[test]
  fn normalize_url_preserves_explicit_scheme() {
    assert_eq!(normalize_url("https://example.com"), "https://example.com");
    assert_eq!(normalize_url("http://localhost:6830"), "http://localhost:6830");
  }

  #[test]
  fn normalize_url_strips_trailing_slashes_and_whitespace() {
    assert_eq!(normalize_url("  http://example.com/  "), "http://example.com");
    assert_eq!(normalize_url("localhost:6830///"), "http://localhost:6830");
  }

  #[test]
  fn normalize_url_handles_empty_input() {
    assert_eq!(normalize_url(""), "");
    assert_eq!(normalize_url("   "), "");
  }

  #[test]
  fn base_url_normalizes_already_stored_value_without_scheme() {
    let connection = RemoteConnection {
      id:             "1".to_string(),
      name:           "test".to_string(),
      url:            "localhost:6830".to_string(),
      auth_type:      AuthType::None,
      api_key:        None,
      share_base_url: None,
      created_at:     Utc::now(),
      updated_at:     Utc::now(),
    };
    assert_eq!(connection.base_url(), "http://localhost:6830");
  }
}

