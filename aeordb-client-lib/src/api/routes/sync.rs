use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::error::ClientError;
use crate::server::AppState;
use crate::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager,
  SyncRelationship, UpdateSyncRelationshipRequest,
};

pub async fn list_relationships(
  State(state): State<AppState>,
) -> Result<Json<Vec<SyncRelationship>>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.list().map(Json)
}

pub async fn create_relationship(
  State(state): State<AppState>,
  Json(request): Json<CreateSyncRelationshipRequest>,
) -> Result<(StatusCode, Json<SyncRelationship>), ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.create(request)
    .map(|relationship| (StatusCode::CREATED, Json(relationship)))
}

pub async fn get_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);

  match manager.get(&id)? {
    Some(relationship) => Ok(Json(relationship)),
    None => Err(ClientError::NotFound(format!("sync relationship not found: {}", id))),
  }
}

pub async fn update_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Json(request): Json<UpdateSyncRelationshipRequest>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.update(&id, request).map(Json)
}

pub async fn delete_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<StatusCode, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.delete(&id).map(|_| StatusCode::NO_CONTENT)
}

pub async fn enable_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.enable(&id).map(Json)
}

pub async fn disable_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.disable(&id).map(Json)
}

pub async fn trigger_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  use crate::connections::ConnectionManager;
  use crate::sync::replication::sync_relationship;

  // Load relationship and connection.
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager.get(&id)?
    .ok_or_else(|| ClientError::NotFound(format!("sync relationship not found: {}", id)))?;

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager.get(&relationship.remote_connection_id)?
    .ok_or_else(|| ClientError::NotFound("connection not found".to_string()))?;

  // Run the sync (push and/or pull based on direction).
  let result = sync_relationship(&state.state_store, &connection, &relationship, &state.http_client)
    .await
    .map_err(|error| ClientError::Server(error.to_string()))?;

  // Log to activity feed (non-fatal).
  let activity = state.sync_runner.activity_log();
  if let Err(error) = activity.log_full_sync(&id, &relationship.name, &result) {
    tracing::warn!("failed to log trigger activity: {}", error);
  }

  // Build a response summarizing what happened.
  let push_summary = result.push.map(|p| serde_json::json!({
    "files_pushed":  p.files_pushed,
    "files_skipped": p.files_skipped,
    "files_failed":  p.files_failed,
    "files_deleted": p.files_deleted,
    "total_bytes":   p.total_bytes,
    "duration_ms":   p.duration_ms,
    "errors":        p.errors,
  }));

  let pull_summary = result.pull.map(|p| serde_json::json!({
    "files_pulled":    p.files_pulled,
    "files_skipped":   p.files_skipped,
    "files_failed":    p.files_failed,
    "files_deleted":   p.files_deleted,
    "symlinks_pulled": p.symlinks_pulled,
    "total_bytes":     p.total_bytes,
    "duration_ms":     p.duration_ms,
    "errors":          p.errors,
  }));

  Ok(Json(serde_json::json!({
    "push": push_summary,
    "pull": pull_summary,
  })))
}

pub async fn start_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  state.sync_runner.start(&id).await
    .map(|_| Json(serde_json::json!({ "message": format!("sync started for {}", id) })))
    .map_err(|error| {
      let msg = error.to_string();
      if msg.contains("already running") {
        ClientError::BadRequest(msg)
      } else if msg.contains("not found") || msg.contains("disabled") {
        ClientError::BadRequest(msg)
      } else {
        ClientError::Server(msg)
      }
    })
}

pub async fn stop_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  state.sync_runner.stop(&id).await
    .map(|_| Json(serde_json::json!({ "message": format!("sync stopped for {}", id) })))
    .map_err(|error| {
      let msg = error.to_string();
      if msg.contains("not running") {
        ClientError::BadRequest(msg)
      } else {
        ClientError::Server(msg)
      }
    })
}

pub async fn pause_all_sync(
  State(state): State<AppState>,
) -> Json<serde_json::Value> {
  state.sync_runner.stop_all().await;
  Json(serde_json::json!({ "message": "all sync runners paused" }))
}

pub async fn resume_all_sync(
  State(state): State<AppState>,
) -> Json<serde_json::Value> {
  state.sync_runner.start_all_enabled().await;
  Json(serde_json::json!({ "message": "all enabled sync runners resumed" }))
}

pub async fn sync_runner_status(
  State(state): State<AppState>,
) -> Json<Vec<crate::sync::runner::SyncRunnerStatus>> {
  Json(state.sync_runner.status().await)
}

pub async fn get_sync_activity(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<Vec<crate::sync::activity::SyncEvent>>, ClientError> {
  state.sync_runner.activity_log()
    .get_events(&id, 50)
    .map(Json)
    .map_err(|error| ClientError::Server(error.to_string()))
}
