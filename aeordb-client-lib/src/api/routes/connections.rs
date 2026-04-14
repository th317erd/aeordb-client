use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::connections::{
  ConnectionManager, ConnectionTestResult, CreateConnectionRequest,
  RemoteConnection, UpdateConnectionRequest,
};
use crate::server::AppState;

pub async fn list_connections(
  State(state): State<AppState>,
) -> Result<Json<Vec<RemoteConnection>>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConnectionManager::new(&state.state_store);

  manager.list()
    .map(Json)
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))
}

pub async fn create_connection(
  State(state): State<AppState>,
  Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<RemoteConnection>), (StatusCode, Json<serde_json::Value>)> {
  let manager = ConnectionManager::new(&state.state_store);

  manager.create(request)
    .map(|connection| (StatusCode::CREATED, Json(connection)))
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))
}

pub async fn get_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<RemoteConnection>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConnectionManager::new(&state.state_store);

  match manager.get(&id) {
    Ok(Some(connection)) => Ok(Json(connection)),
    Ok(None) => Err((
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({ "error": format!("connection not found: {}", id) })),
    )),
    Err(error) => Err((
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    )),
  }
}

pub async fn update_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Json(request): Json<UpdateConnectionRequest>,
) -> Result<Json<RemoteConnection>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConnectionManager::new(&state.state_store);

  manager.update(&id, request)
    .map(Json)
    .map_err(|error| {
      let status = if error.to_string().contains("not found") {
        StatusCode::NOT_FOUND
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}

pub async fn delete_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConnectionManager::new(&state.state_store);

  manager.delete(&id)
    .map(|_| StatusCode::NO_CONTENT)
    .map_err(|error| {
      let status = if error.to_string().contains("not found") {
        StatusCode::NOT_FOUND
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}

pub async fn test_connection(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<ConnectionTestResult>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConnectionManager::new(&state.state_store);

  manager.test_connection(&id).await
    .map(Json)
    .map_err(|error| {
      let status = if error.to_string().contains("not found") {
        StatusCode::NOT_FOUND
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}
