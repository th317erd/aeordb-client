use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::connections::{
  ConnectionManager, ConnectionTestResult, CreateConnectionRequest,
  RemoteConnection, UpdateConnectionRequest,
};
use crate::error::ClientError;
use crate::remote::{RemoteClient, ENTRY_TYPE_DIRECTORY};
use crate::server::AppState;

pub async fn list_connections(
  State(state): State<AppState>,
) -> Result<Json<Vec<RemoteConnection>>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.list().await.map(Json)
}

/// GET /api/v1/health/connections
///
/// One-shot snapshot of every connection's most recent health-check
/// result. The background pinger (crate::health) populates this every
/// 10s; consumers that want live updates should subscribe to the
/// `connection_health` SSE event in addition to fetching this.
///
/// Routed under `/health/...` (not `/connections/health`) to avoid
/// colliding with the `/connections/{id}` dynamic matcher.
///
/// Returns an empty array before the first ping completes (~10s after
/// boot), or for connections created after the most recent tick.
pub async fn list_health(
  State(state): State<AppState>,
) -> Json<Vec<crate::health::HealthSnapshot>> {
  let map = state.health_map.lock().await;
  let snapshots: Vec<_> = map.values().cloned().collect();
  Json(snapshots)
}

pub async fn create_connection(
  State(state): State<AppState>,
  Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<RemoteConnection>), ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.create(request).await
    .map(|connection| (StatusCode::CREATED, Json(connection)))
}

pub async fn get_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<RemoteConnection>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);

  match manager.get(&id).await? {
    Some(connection) => Ok(Json(connection)),
    None => Err(ClientError::NotFound(format!("connection not found: {}", id))),
  }
}

pub async fn update_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Json(request): Json<UpdateConnectionRequest>,
) -> Result<Json<RemoteConnection>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.update(&id, request).await.map(Json)
}

pub async fn delete_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<StatusCode, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.delete(&id).await.map(|_| StatusCode::NO_CONTENT)
}

pub async fn test_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<ConnectionTestResult>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.test_connection(&id).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
  pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PortalUrlQuery {
  pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PortalUrlResponse {
  pub url: String,
}

/// GET /api/v1/connections/{id}/portal-url?path=/some/dir
///
/// Mints a short-lived JWT from the connection's API key and returns a
/// pre-authenticated portal URL pointing at the given path on the engine's
/// web UI. The renderer opens this URL via `open_external_url` to surface
/// the file/folder in the user's browser already logged in.
///
/// 404 if the connection is missing or has no API key.
pub async fn portal_url(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<PortalUrlQuery>,
) -> Result<Json<PortalUrlResponse>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  let connection = manager.get(&id).await?
    .ok_or_else(|| ClientError::NotFound(format!("connection not found: {}", id)))?;

  let path = query.path.unwrap_or_else(|| "/".to_string());
  let normalized = if path.starts_with('/') { path } else { format!("/{}", path) };

  let client = RemoteClient::from_connection(&connection, &state.http_client);
  let url = client.portal_url(&normalized).await?;
  Ok(Json(PortalUrlResponse { url }))
}

#[derive(Debug, Serialize)]
pub struct BrowseEntry {
  pub name:      String,
  pub full_path: String,
}

#[derive(Debug, Serialize)]
pub struct BrowseResponse {
  pub path:    String,
  pub entries: Vec<BrowseEntry>,
}

/// GET /api/v1/connections/{id}/browse?path=/some/dir
///
/// Lists subdirectories on the remote so the JS folder picker doesn't have
/// to make cross-origin requests (which the aeordb engine doesn't allow
/// CORS preflight for).
pub async fn browse_remote(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Query(query): Query<BrowseQuery>,
) -> Result<Json<BrowseResponse>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  let connection = manager.get(&id).await?
    .ok_or_else(|| ClientError::NotFound(format!("connection not found: {}", id)))?;

  let raw_path = query.path.unwrap_or_else(|| "/".to_string());
  let normalized = if raw_path.starts_with('/') { raw_path.clone() } else { format!("/{}", raw_path) };
  let trimmed = normalized.trim_end_matches('/');
  let path_for_request = if trimmed.is_empty() { "/".to_string() } else { format!("{}/", trimmed) };
  let is_root = trimmed.is_empty();

  let client = RemoteClient::from_connection(&connection, &state.http_client);

  let items = client.list_directory(&path_for_request).await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  let entries = items.into_iter()
    .filter(|entry| entry.entry_type == ENTRY_TYPE_DIRECTORY)
    .map(|entry| {
      let full = format!("{}/{}", trimmed, entry.name);
      BrowseEntry { name: entry.name, full_path: full }
    })
    .collect();

  Ok(Json(BrowseResponse {
    path: if is_root { "/".to_string() } else { trimmed.to_string() },
    entries,
  }))
}

/// GET /api/v1/connections/{id}/proxy/{*path}
///
/// Proxies a GET request from the client UI to the remote aeordb,
/// attaching the connection's JWT. Needed because the engine doesn't
/// accept CORS preflight from the local Tauri origin, so cross-origin
/// fetches from the dashboard preview fail before the request leaves.
/// Used by aeor-remote-dashboard to fetch /system/stats and other
/// engine endpoints without hitting CORS.
pub async fn proxy_remote(
  State(state): State<AppState>,
  Path((id, remote_path)): Path<(String, String)>,
  axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Result<axum::response::Response, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  let connection = manager.get(&id).await?
    .ok_or_else(|| ClientError::NotFound(format!("connection not found: {}", id)))?;

  let client = RemoteClient::from_connection(&connection, &state.http_client);
  let base = connection.base_url();
  let query_suffix = match query {
    Some(q) if !q.is_empty() => format!("?{}", q),
    _ => String::new(),
  };
  let url = format!("{}/{}{}", base, remote_path, query_suffix);

  let mut request = state.http_client.get(&url);
  if let Some(ref auth) = client.auth_header().await {
    request = request.header("Authorization", auth);
  }

  let upstream = request.send().await
    .map_err(|error| ClientError::BadGateway(format!("proxy fetch failed: {}", error)))?;

  let status = upstream.status();
  let content_type = upstream.headers()
    .get(axum::http::header::CONTENT_TYPE)
    .and_then(|v| v.to_str().ok())
    .unwrap_or("application/octet-stream")
    .to_string();
  let body = upstream.bytes().await
    .map_err(|error| ClientError::BadGateway(format!("proxy read failed: {}", error)))?;

  let mut response = axum::response::Response::builder()
    .status(status)
    .header(axum::http::header::CONTENT_TYPE, content_type)
    .body(axum::body::Body::from(body))
    .map_err(|error| ClientError::Server(format!("proxy response build failed: {}", error)))?;

  // Mirror upstream status code into our error mapping for non-2xx so the
  // dashboard's "Failed to load stats" message reflects the real cause.
  if !status.is_success() {
    *response.status_mut() = status;
  }
  Ok(response)
}
