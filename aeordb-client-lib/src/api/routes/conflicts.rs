use axum::extract::State;
use axum::response::Json;
use serde::Deserialize;

use aeordb::engine::{
  list_conflicts_typed,
  RequestContext,
};
use aeordb::engine::conflict_store::{resolve_conflict, dismiss_conflict};

use crate::error::ClientError;
use crate::server::AppState;

#[derive(Deserialize)]
pub struct ResolveRequest {
  /// The conflicted file path (e.g., "/docs/readme.md")
  pub path: String,
  /// "winner" or "loser" — which version to keep as the active file
  pub pick: String,
}

#[derive(Deserialize)]
pub struct DismissRequest {
  /// The conflicted file path
  pub path: String,
}

/// GET /api/v1/conflicts — list all conflicts from aeordb's /.conflicts/
pub async fn list_conflicts(
  State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let conflicts = list_conflicts_typed(&state.state_store.engine())
    .map_err(|error| ClientError::Server(error.to_string()))?;

  // Convert to JSON-serializable format
  let json_conflicts: Vec<serde_json::Value> = conflicts.iter()
    .map(|conflict| {
      serde_json::json!({
        "path":          conflict.path,
        "conflict_type": conflict.conflict_type,
        "auto_winner":   conflict.auto_winner,
        "created_at":    conflict.created_at,
        "winner": {
          "hash":         conflict.winner.hash,
          "virtual_time": conflict.winner.virtual_time,
          "node_id":      conflict.winner.node_id,
          "size":         conflict.winner.size,
          "content_type": conflict.winner.content_type,
        },
        "loser": {
          "hash":         conflict.loser.hash,
          "virtual_time": conflict.loser.virtual_time,
          "node_id":      conflict.loser.node_id,
          "size":         conflict.loser.size,
          "content_type": conflict.loser.content_type,
        },
      })
    })
    .collect();

  Ok(Json(serde_json::json!(json_conflicts)))
}

/// POST /api/v1/conflicts/resolve — resolve a conflict by picking winner or loser
pub async fn resolve_conflict_handler(
  State(state): State<AppState>,
  Json(request): Json<ResolveRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  if request.pick != "winner" && request.pick != "loser" {
    return Err(ClientError::BadRequest(
      format!("invalid pick value '{}': must be 'winner' or 'loser'", request.pick),
    ));
  }

  let ctx          = RequestContext::system();
  let conflict_path = &request.path;

  resolve_conflict(
    &state.state_store.engine(),
    &ctx,
    conflict_path,
    &request.pick,
  ).map_err(|error| {
    let msg = error.to_string();
    if msg.contains("not found") || msg.contains("No conflict") {
      ClientError::NotFound(msg)
    } else {
      ClientError::Server(msg)
    }
  })?;

  Ok(Json(serde_json::json!({
    "resolved":  true,
    "path":      request.path,
    "picked":    request.pick,
  })))
}

/// POST /api/v1/conflicts/dismiss — accept the auto-winner
pub async fn dismiss_conflict_handler(
  State(state): State<AppState>,
  Json(request): Json<DismissRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let ctx          = RequestContext::system();
  let conflict_path = &request.path;

  dismiss_conflict(
    &state.state_store.engine(),
    &ctx,
    conflict_path,
  ).map_err(|error| {
    let msg = error.to_string();
    if msg.contains("not found") || msg.contains("No conflict") {
      ClientError::NotFound(msg)
    } else {
      ClientError::Server(msg)
    }
  })?;

  Ok(Json(serde_json::json!({
    "dismissed": true,
    "path":      request.path,
  })))
}

/// POST /api/v1/conflicts/dismiss-all — dismiss all conflicts (accept auto-winners)
pub async fn dismiss_all_conflicts(
  State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let ctx = RequestContext::system();

  let conflicts = list_conflicts_typed(&state.state_store.engine())
    .map_err(|error| ClientError::Server(error.to_string()))?;

  let mut dismissed = 0;
  for conflict in &conflicts {
    if dismiss_conflict(&state.state_store.engine(), &ctx, &conflict.path).is_ok() {
      dismissed += 1;
    }
  }

  Ok(Json(serde_json::json!({
    "dismissed_count": dismissed,
  })))
}
