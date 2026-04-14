use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use crate::server::AppState;
use crate::sync::conflicts::{ConflictManager, ConflictRecord, ConflictResolution};

#[derive(Deserialize)]
pub struct ConflictQuery {
  pub sync_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ResolveRequest {
  pub resolution: ConflictResolution,
}

pub async fn list_conflicts(
  State(state): State<AppState>,
  Query(query): Query<ConflictQuery>,
) -> Result<Json<Vec<ConflictRecord>>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConflictManager::new(&state.state_store);

  let conflicts = match query.sync_id {
    Some(ref sync_id) => manager.list_for_relationship(sync_id),
    None => manager.list(),
  };

  conflicts
    .map(Json)
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))
}

pub async fn resolve_conflict(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Json(request): Json<ResolveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConflictManager::new(&state.state_store);

  // Get the conflict record before resolving
  let conflict = manager.get(&id)
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))?
    .ok_or_else(|| (
      StatusCode::NOT_FOUND,
      Json(serde_json::json!({ "error": format!("conflict not found: {}", id) })),
    ))?;

  // TODO: Implement the actual file resolution logic based on request.resolution
  // For now, just remove the conflict from the queue
  let _ = request.resolution; // Will be used when we implement actual resolution

  manager.resolve(&id)
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))?;

  Ok(Json(serde_json::json!({
    "resolved": true,
    "conflict_id": id,
    "file_path": conflict.file_path,
  })))
}

pub async fn resolve_all_conflicts(
  State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
  let manager = ConflictManager::new(&state.state_store);

  let count = manager.resolve_all()
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))?;

  Ok(Json(serde_json::json!({
    "resolved_count": count,
  })))
}
