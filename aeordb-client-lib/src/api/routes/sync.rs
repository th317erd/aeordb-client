use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::server::AppState;
use crate::sync::engine::{SyncPassResult, pull_sync_pass};
use crate::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager,
  SyncRelationship, UpdateSyncRelationshipRequest,
};

pub async fn list_relationships(
  State(state): State<AppState>,
) -> Result<Json<Vec<SyncRelationship>>, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

  manager.list()
    .map(Json)
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))
}

pub async fn create_relationship(
  State(state): State<AppState>,
  Json(request): Json<CreateSyncRelationshipRequest>,
) -> Result<(StatusCode, Json<SyncRelationship>), (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

  manager.create(request)
    .map(|relationship| (StatusCode::CREATED, Json(relationship)))
    .map_err(|error| {
      let status = if error.to_string().contains("not found") {
        StatusCode::BAD_REQUEST
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}

pub async fn get_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

  match manager.get(&id) {
    Ok(Some(relationship)) => Ok(Json(relationship)),
    Ok(None) => Err((
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({ "error": format!("sync relationship not found: {}", id) })),
    )),
    Err(error) => Err((
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    )),
  }
}

pub async fn update_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Json(request): Json<UpdateSyncRelationshipRequest>,
) -> Result<Json<SyncRelationship>, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

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

pub async fn delete_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

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

pub async fn enable_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

  manager.enable(&id)
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

pub async fn disable_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.state_store);

  manager.disable(&id)
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

pub async fn trigger_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncPassResult>, (StatusCode, Json<serde_json::Value>)> {
  pull_sync_pass(&state.state_store, &id).await
    .map(Json)
    .map_err(|error| {
      let status = if error.to_string().contains("not found") || error.to_string().contains("disabled") {
        StatusCode::BAD_REQUEST
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}
