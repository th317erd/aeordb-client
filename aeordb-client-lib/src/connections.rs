use std::sync::Arc;

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
  pub id: String,
  pub name: String,
  pub url: String,
  pub auth_type: AuthType,
  pub api_key: Option<String>,
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
    let raw = self
      .share_base_url
      .as_deref()
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

fn is_same_host_https_upgrade(base_url: &reqwest::Url, target: &reqwest::Url) -> bool {
  if base_url.scheme() != "http"
    || target.scheme() != "https"
    || target.host_str() != base_url.host_str()
  {
    return false;
  }

  match (base_url.port(), target.port()) {
    (None, None) => true,
    (Some(left), Some(right)) if left == right => true,
    (Some(80), None) => true,
    (None, Some(443)) => true,
    (Some(80), Some(443)) => true,
    _ => false,
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConnectionRequest {
  pub name: String,
  pub url: String,
  pub auth_type: AuthType,
  pub api_key: Option<String>,
  pub share_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConnectionRequest {
  pub name: Option<String>,
  pub url: Option<String>,
  pub auth_type: Option<AuthType>,
  #[serde(
    default,
    deserialize_with = "deserialize_nullable_field",
    skip_serializing_if = "Option::is_none"
  )]
  pub api_key: Option<Option<String>>,
  #[serde(
    default,
    deserialize_with = "deserialize_nullable_field",
    skip_serializing_if = "Option::is_none"
  )]
  pub share_base_url: Option<Option<String>>,
}

fn deserialize_nullable_field<'de, D, T>(
  deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
  D: serde::Deserializer<'de>,
  T: Deserialize<'de>,
{
  Option::<T>::deserialize(deserializer).map(Some)
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
      id: Uuid::new_v4().to_string(),
      name: request.name,
      url,
      auth_type: request.auth_type,
      api_key: request.api_key,
      share_base_url: request.share_base_url,
      created_at: now,
      updated_at: now,
    };

    let new_connection = connection.clone();
    self
      .config
      .update(|config| {
        config.connections.push(new_connection);
      })
      .await?;

    tracing::info!(
      "created connection '{}' ({})",
      connection.name,
      connection.id
    );
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
    Ok(
      config
        .connections
        .into_iter()
        .find(|connection| connection.id == id)
        .map(|mut connection| {
          connection.url = normalize_url(&connection.url);
          connection
        }),
    )
  }

  pub async fn update(
    &self,
    id: &str,
    request: UpdateConnectionRequest,
  ) -> Result<RemoteConnection> {
    let mut updated_connection = None;

    self
      .config
      .update(|config| {
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
          if connection.auth_type == AuthType::None {
            connection.api_key = None;
          }
        }
        if let Some(api_key) = request.api_key {
          connection.api_key = api_key.filter(|s| !s.is_empty());
          connection.auth_type = if connection.api_key.is_some() {
            AuthType::ApiKey
          } else {
            AuthType::None
          };
        }
        if let Some(share_base_url) = request.share_base_url {
          connection.share_base_url = share_base_url.filter(|s| !s.is_empty());
        }

        connection.updated_at = Utc::now();
        updated_connection = Some(connection.clone());
      })
      .await?;

    match updated_connection {
      Some(connection) => {
        tracing::info!(
          "updated connection '{}' ({})",
          connection.name,
          connection.id
        );
        Ok(connection)
      }
      None => Err(ClientError::NotFound(format!(
        "connection not found: {}",
        id
      ))),
    }
  }

  pub async fn delete(&self, id: &str) -> Result<()> {
    let mut found = false;
    let mut matching_relationships = 0usize;

    self
      .config
      .update(|config| {
        matching_relationships = config
          .relationships
          .iter()
          .filter(|relationship| relationship.remote_connection_id == id)
          .count();
        if matching_relationships > 0 {
          return;
        }

        let before = config.connections.len();
        config.connections.retain(|connection| connection.id != id);
        found = config.connections.len() < before;
      })
      .await?;

    if matching_relationships > 0 {
      return Err(ClientError::BadRequest(format!(
        "connection cannot be deleted while sync relationships are still configured against it ({} configured); delete those syncs first",
        matching_relationships
      )));
    }

    if !found {
      return Err(ClientError::NotFound(format!(
        "connection not found: {}",
        id
      )));
    }

    tracing::info!("deleted connection {}", id);
    Ok(())
  }

  /// Test connectivity to a remote aeordb instance.
  pub async fn test_connection(&self, id: &str) -> Result<ConnectionTestResult> {
    let connection = self
      .get(id)
      .await?
      .ok_or_else(|| ClientError::NotFound(format!("connection not found: {}", id)))?;

    let health_url = format!("{}/system/health", connection.base_url());
    let client = reqwest::Client::new();

    let start = std::time::Instant::now();
    let mut request_builder = client.get(&health_url);

    if connection.auth_type == AuthType::ApiKey {
      if let Some(ref api_key) = connection.api_key {
        request_builder = request_builder.header("Authorization", format!("Bearer {}", api_key));
      }
    }

    match tokio::time::timeout(std::time::Duration::from_secs(10), request_builder.send()).await {
      Ok(Ok(response)) => {
        let latency = start.elapsed().as_millis() as u64;

        if response.status().is_success() {
          Ok(ConnectionTestResult {
            success: true,
            message: format!("connected (HTTP {})", response.status().as_u16()),
            latency_ms: Some(latency),
          })
        } else {
          Ok(ConnectionTestResult {
            success: false,
            message: format!("server returned HTTP {}", response.status().as_u16()),
            latency_ms: Some(latency),
          })
        }
      }
      Ok(Err(error)) => Ok(ConnectionTestResult {
        success: false,
        message: format!("connection failed: {}", error),
        latency_ms: None,
      }),
      Err(_) => Ok(ConnectionTestResult {
        success: false,
        message: "connection timed out (10s)".to_string(),
        latency_ms: None,
      }),
    }
  }
}

/// Probe every saved connection for a server-side `http→https` upgrade
/// (issued as a 301/308 redirect by a fronting reverse proxy) and rewrite
/// the stored URL when we see one. Runs once at startup as a fire-and-forget
/// background task.
///
/// Why this exists: nginx in front of many engine deployments forces
/// `Location: https://…` on plain-http requests. reqwest's default
/// redirect policy is fine for GETs but downgrades POST→GET on 301/302,
/// so our /auth/token POST gets redirected as a GET and comes back HTTP
/// 405 — every authenticated call then 401s and the UI fails with vague
/// 502s. exchange_token has its own preserves-POST redirect handler so
/// the connection still works while http, but persisting the upgrade
/// here makes the canonical URL match what the engine actually serves,
/// trims one round trip per token mint, and quiets the noise.
///
/// Only applies the upgrade when the redirect target shares the same
/// host:port and only changes the scheme to https — anything more
/// surprising (different host, downgrade, weird scheme) is logged and
/// left alone so we never silently send credentials somewhere new.
pub async fn probe_and_upgrade_connection_urls(
  config_store: Arc<ConfigStore>,
  jwt_cache: crate::jwt_cache::JwtCache,
) {
  let manager = ConnectionManager::new(&config_store);
  let connections = match manager.list().await {
    Ok(c) => c,
    Err(e) => {
      tracing::warn!("URL upgrade probe: failed to list connections: {}", e);
      return;
    }
  };

  // We need a client that does NOT follow redirects so we can inspect
  // the 3xx response ourselves; the default client follows transparently.
  let probe_client = match reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(10))
    .redirect(reqwest::redirect::Policy::none())
    .build()
  {
    Ok(c) => c,
    Err(e) => {
      tracing::warn!("URL upgrade probe: failed to build HTTP client: {}", e);
      return;
    }
  };

  for conn in connections {
    let base = conn.base_url();
    if base.starts_with("https://") {
      continue;
    }

    // /system/health is the same endpoint test_connection hits — cheap,
    // always present, doesn't require auth.
    let probe_url = format!("{}/system/health", base);
    let resp = match probe_client.get(&probe_url).send().await {
      Ok(r) => r,
      Err(e) => {
        tracing::debug!(
          "URL upgrade probe: '{}' probe failed (likely offline): {}",
          conn.name,
          e
        );
        continue;
      }
    };

    let status = resp.status().as_u16();
    if !matches!(status, 301 | 308) {
      continue;
    }

    let Some(location) = resp
      .headers()
      .get(reqwest::header::LOCATION)
      .and_then(|v| v.to_str().ok())
    else {
      continue;
    };
    let Ok(base_url) = reqwest::Url::parse(&probe_url) else {
      continue;
    };
    let Ok(target) = base_url.join(location) else {
      tracing::warn!(
        "URL upgrade probe: '{}' redirect to unparseable Location '{}'",
        conn.name,
        location
      );
      continue;
    };

    // Only act when the only change is the scheme, http → https, on the
    // same host:port. Anything else (different host, downgrade) is
    // suspicious and we leave the connection alone so the user can
    // investigate.
    if !is_same_host_https_upgrade(&base_url, &target) {
      tracing::warn!(
        "URL upgrade probe: '{}' redirected from {} to {} (not a same-host http→https upgrade); \
         leaving connection URL unchanged",
        conn.name,
        probe_url,
        target,
      );
      continue;
    }

    // Build the new base URL: scheme + host + optional explicit port.
    let mut new_base = format!("https://{}", target.host_str().unwrap_or(""));
    if let Some(port) = target.port() {
      new_base.push(':');
      new_base.push_str(&port.to_string());
    }

    tracing::info!(
      "URL upgrade probe: upgrading '{}' from {} to {} (engine returned {})",
      conn.name,
      base,
      new_base,
      status,
    );

    let update = UpdateConnectionRequest {
      name: None,
      url: Some(new_base),
      auth_type: None,
      api_key: None,
      share_base_url: None,
    };
    match manager.update(&conn.id, update).await {
      Ok(_) => {
        // The cached JWT (if any) was minted for the old base URL — it
        // may still be valid (JWTs are checked by signature, not by
        // host) but invalidate as a precaution so the next request
        // mints fresh against the canonical URL.
        jwt_cache.slot_for(&conn.id).lock().unwrap().take();
      }
      Err(e) => {
        tracing::warn!(
          "URL upgrade probe: failed to persist upgrade for '{}': {}",
          conn.name,
          e,
        );
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
    assert_eq!(
      normalize_url("http://localhost:6830"),
      "http://localhost:6830"
    );
  }

  #[test]
  fn normalize_url_strips_trailing_slashes_and_whitespace() {
    assert_eq!(
      normalize_url("  http://example.com/  "),
      "http://example.com"
    );
    assert_eq!(normalize_url("localhost:6830///"), "http://localhost:6830");
  }

  #[test]
  fn normalize_url_handles_empty_input() {
    assert_eq!(normalize_url(""), "");
    assert_eq!(normalize_url("   "), "");
  }

  #[test]
  fn same_host_https_upgrade_allows_default_port_change() {
    let base = reqwest::Url::parse("http://files.taraani.org/system/health").unwrap();
    let target = reqwest::Url::parse("https://files.taraani.org/system/health").unwrap();
    assert!(is_same_host_https_upgrade(&base, &target));
  }

  #[test]
  fn same_host_https_upgrade_allows_explicit_default_ports() {
    let base = reqwest::Url::parse("http://files.taraani.org:80/system/health").unwrap();
    let target = reqwest::Url::parse("https://files.taraani.org:443/system/health").unwrap();
    assert!(is_same_host_https_upgrade(&base, &target));
  }

  #[test]
  fn same_host_https_upgrade_allows_same_explicit_port() {
    let base = reqwest::Url::parse("http://localhost:6830/system/health").unwrap();
    let target = reqwest::Url::parse("https://localhost:6830/system/health").unwrap();
    assert!(is_same_host_https_upgrade(&base, &target));
  }

  #[test]
  fn same_host_https_upgrade_rejects_different_host() {
    let base = reqwest::Url::parse("http://files.taraani.org/system/health").unwrap();
    let target = reqwest::Url::parse("https://evil.example/system/health").unwrap();
    assert!(!is_same_host_https_upgrade(&base, &target));
  }

  #[test]
  fn same_host_https_upgrade_rejects_non_default_port_change() {
    let base = reqwest::Url::parse("http://localhost:6830/system/health").unwrap();
    let target = reqwest::Url::parse("https://localhost:8443/system/health").unwrap();
    assert!(!is_same_host_https_upgrade(&base, &target));
  }

  #[test]
  fn base_url_normalizes_already_stored_value_without_scheme() {
    let connection = RemoteConnection {
      id: "1".to_string(),
      name: "test".to_string(),
      url: "localhost:6830".to_string(),
      auth_type: AuthType::None,
      api_key: None,
      share_base_url: None,
      created_at: Utc::now(),
      updated_at: Utc::now(),
    };
    assert_eq!(connection.base_url(), "http://localhost:6830");
  }
}
