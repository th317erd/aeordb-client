use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::connections::{
  ConnectionManager, ConnectionTestResult, CreateConnectionRequest,
  RemoteConnection, UpdateConnectionRequest,
};
use crate::error::ClientError;
use crate::server::AppState;

pub async fn list_connections(
  State(state): State<AppState>,
) -> Result<Json<Vec<RemoteConnection>>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.list().map(Json)
}

pub async fn create_connection(
  State(state): State<AppState>,
  Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<RemoteConnection>), ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.create(request)
    .map(|connection| (StatusCode::CREATED, Json(connection)))
}

pub async fn get_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<RemoteConnection>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);

  match manager.get(&id)? {
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
  manager.update(&id, request).map(Json)
}

pub async fn delete_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<StatusCode, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.delete(&id).map(|_| StatusCode::NO_CONTENT)
}

pub async fn test_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<ConnectionTestResult>, ClientError> {
  let manager = ConnectionManager::new(&state.config_store);
  manager.test_connection(&id).await.map(Json)
}
