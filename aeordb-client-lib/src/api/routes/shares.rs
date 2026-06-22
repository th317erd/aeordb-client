use axum::extract::{Path as AxumPath, Query, State};
use axum::response::Json;
use serde::Deserialize;

use crate::connections::ConnectionManager;
use crate::error::ClientError;
use crate::remote::RemoteClient;
use crate::server::AppState;
use crate::sync::relationships::RelationshipManager;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn load_connection_and_client(
  state: &AppState,
  relationship_id: &str,
) -> Result<(RemoteClient, crate::connections::RemoteConnection), ClientError> {
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager
    .get(relationship_id)
    .await?
    .ok_or_else(|| ClientError::NotFound(format!("relationship not found: {}", relationship_id)))?;

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager
    .get(&relationship.remote_connection_id)
    .await?
    .ok_or_else(|| {
      ClientError::NotFound(format!(
        "connection not found: {}",
        relationship.remote_connection_id
      ))
    })?;

  let client = RemoteClient::from_connection(&connection, &state.http_client);
  Ok((client, connection))
}

// ---------------------------------------------------------------------------
// Query types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PathQuery {
  pub path: Option<String>,
}

// ---------------------------------------------------------------------------
// 1. GET /api/v1/shares/{relationship_id}?path=...
// ---------------------------------------------------------------------------

pub async fn get_shares(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Query(query): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;
  let path = query.path.as_deref().unwrap_or("/");

  let result = client.get_shares(path).await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 2. POST /api/v1/shares/{relationship_id}
// ---------------------------------------------------------------------------

pub async fn share(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;

  let result = client.share(&body).await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 3. DELETE /api/v1/shares/{relationship_id}
// ---------------------------------------------------------------------------

pub async fn unshare(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;

  let result = client.unshare(&body).await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 4. GET /api/v1/shares/{relationship_id}/users
// ---------------------------------------------------------------------------

pub async fn get_shareable_users(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;

  let result = client.get_shareable_users().await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 5. GET /api/v1/shares/{relationship_id}/groups
// ---------------------------------------------------------------------------

pub async fn get_shareable_groups(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;

  let result = client.get_shareable_groups().await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 6. POST /api/v1/shares/{relationship_id}/link
// ---------------------------------------------------------------------------

pub async fn create_share_link(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(mut body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, connection) = load_connection_and_client(&state, &relationship_id).await?;

  // Inject the share base URL from the connection config.
  // The client JS doesn't know the remote server's URL, so the proxy fills it in.
  let share_url = connection.effective_share_url().to_string();
  if let Some(obj) = body.as_object_mut() {
    obj.insert("base_url".to_string(), serde_json::Value::String(share_url));
  }

  let result = client.create_share_link(&body).await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 7. GET /api/v1/shares/{relationship_id}/links?path=...
// ---------------------------------------------------------------------------

pub async fn get_share_links(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Query(query): Query<PathQuery>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;
  let path = query.path.as_deref().unwrap_or("/");

  let result = client.get_share_links(path).await?;

  Ok(Json(result))
}

// ---------------------------------------------------------------------------
// 8. DELETE /api/v1/shares/{relationship_id}/links/{key_id}
// ---------------------------------------------------------------------------

pub async fn revoke_share_link(
  State(state): State<AppState>,
  AxumPath((relationship_id, key_id)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (client, _) = load_connection_and_client(&state, &relationship_id).await?;

  let result = client.revoke_share_link(&key_id).await?;

  Ok(Json(result))
}
