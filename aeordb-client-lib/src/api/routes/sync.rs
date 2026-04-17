use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::server::AppState;
use crate::sync::replication::replicate;
use crate::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager,
  SyncRelationship, UpdateSyncRelationshipRequest,
};

pub async fn list_relationships(
  State(state): State<AppState>,
) -> Result<Json<Vec<SyncRelationship>>, (StatusCode, Json<serde_json::Value>)> {
  let manager = RelationshipManager::new(&state.config_store);

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
  let manager = RelationshipManager::new(&state.config_store);

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
  let manager = RelationshipManager::new(&state.config_store);

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
  let manager = RelationshipManager::new(&state.config_store);

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
  let manager = RelationshipManager::new(&state.config_store);

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
  let manager = RelationshipManager::new(&state.config_store);

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
  let manager = RelationshipManager::new(&state.config_store);

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
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
  use crate::sync::relationships::RelationshipManager;
  use crate::sync::filesystem_bridge::{
    WriteSuppressionSet, ingest_directory, project_to_filesystem,
  };
  use crate::connections::ConnectionManager;

  // Load relationship and connection
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager.get(&id)
    .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": error.to_string() }))))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("sync relationship not found: {}", id) }))))?;

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager.get(&relationship.remote_connection_id)
    .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": error.to_string() }))))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "connection not found" }))))?;

  let direction  = &relationship.direction;
  let suppression = WriteSuppressionSet::new();

  // Step 1: Ingest local filesystem → local aeordb (for push-capable directions)
  let ingest_result = if *direction == crate::sync::relationships::SyncDirection::PushOnly
    || *direction == crate::sync::relationships::SyncDirection::Bidirectional
  {
    ingest_directory(
      state.state_store.engine(),
      &relationship.local_path,
      &relationship.remote_path,
      relationship.filter.as_deref(),
    ).ok()
  } else {
    None
  };

  // Step 2: Replicate local aeordb ↔ remote aeordb
  let replication_result = replicate(
    state.state_store.engine(),
    &connection,
    None,
  ).await.map_err(|error| (
    StatusCode::INTERNAL_SERVER_ERROR,
    Json(serde_json::json!({ "error": error.to_string() })),
  ))?;

  // Step 3: Project changes to filesystem (for pull-capable directions)
  let project_result = if *direction == crate::sync::relationships::SyncDirection::PullOnly
    || *direction == crate::sync::relationships::SyncDirection::Bidirectional
  {
    project_to_filesystem(
      state.state_store.engine(),
      &relationship.local_path,
      &relationship.remote_path,
      relationship.filter.as_deref(),
      &suppression,
    ).ok()
  } else {
    None
  };

  Ok(Json(serde_json::json!({
    "replication": replication_result,
    "ingest":      ingest_result,
    "project":     project_result,
  })))
}

pub async fn start_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
  state.sync_runner.start(&id).await
    .map(|_| Json(serde_json::json!({ "message": format!("sync started for {}", id) })))
    .map_err(|error| {
      let status = if error.to_string().contains("already running") {
        StatusCode::CONFLICT
      } else if error.to_string().contains("not found") || error.to_string().contains("disabled") {
        StatusCode::BAD_REQUEST
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}

pub async fn stop_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
  state.sync_runner.stop(&id).await
    .map(|_| Json(serde_json::json!({ "message": format!("sync stopped for {}", id) })))
    .map_err(|error| {
      let status = if error.to_string().contains("not running") {
        StatusCode::BAD_REQUEST
      } else {
        StatusCode::INTERNAL_SERVER_ERROR
      };
      (status, Json(serde_json::json!({ "error": error.to_string() })))
    })
}

pub async fn sync_runner_status(
  State(state): State<AppState>,
) -> Json<Vec<crate::sync::runner::SyncRunnerStatus>> {
  Json(state.sync_runner.status().await)
}
