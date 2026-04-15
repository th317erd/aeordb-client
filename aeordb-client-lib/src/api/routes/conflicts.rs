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

  // Resolve the conflict by performing the requested action
  let resolution_action = match request.resolution {
    ConflictResolution::KeepLocal => {
      // Push the local version to remote
      // Load relationship to find paths
      let rel_manager = crate::sync::relationships::RelationshipManager::new(&state.state_store);
      if let Ok(Some(relationship)) = rel_manager.get(&conflict.relationship_id) {
        let conn_manager = crate::connections::ConnectionManager::new(&state.state_store);
        if let Ok(Some(connection)) = conn_manager.get(&relationship.remote_connection_id) {
          let mut upload_client = crate::remote::upload::UploadClient::new(&connection);

          // Determine local file path from the conflict
          let relative = conflict.file_path.strip_prefix(&relationship.remote_path).unwrap_or(&conflict.file_path);
          let local_path = format!("{}/{}", relationship.local_path, relative);

          if let Ok(bytes) = std::fs::read(&local_path) {
            let content_type = crate::sync::push::mime_from_extension(std::path::Path::new(&local_path));
            let _ = upload_client.upload_file_chunked(&conflict.file_path, &bytes, content_type.as_deref()).await;
          }
        }
      }
      "kept_local"
    }
    ConflictResolution::KeepRemote => {
      // Pull the remote version to local
      let rel_manager = crate::sync::relationships::RelationshipManager::new(&state.state_store);
      if let Ok(Some(relationship)) = rel_manager.get(&conflict.relationship_id) {
        let conn_manager = crate::connections::ConnectionManager::new(&state.state_store);
        if let Ok(Some(connection)) = conn_manager.get(&relationship.remote_connection_id) {
          let remote_client = crate::remote::RemoteClient::from_connection(&connection);

          let relative = conflict.file_path.strip_prefix(&relationship.remote_path).unwrap_or(&conflict.file_path);
          let local_path = format!("{}/{}", relationship.local_path, relative);

          if let Ok((bytes, _metadata)) = remote_client.download_file(&conflict.file_path).await {
            let _ = std::fs::write(&local_path, &bytes);
          }
        }
      }
      "kept_remote"
    }
    ConflictResolution::KeepBoth => {
      // Rename the local version with a ".conflict" suffix, then pull remote
      let rel_manager = crate::sync::relationships::RelationshipManager::new(&state.state_store);
      if let Ok(Some(relationship)) = rel_manager.get(&conflict.relationship_id) {
        let conn_manager = crate::connections::ConnectionManager::new(&state.state_store);
        if let Ok(Some(connection)) = conn_manager.get(&relationship.remote_connection_id) {
          let remote_client = crate::remote::RemoteClient::from_connection(&connection);

          let relative = conflict.file_path.strip_prefix(&relationship.remote_path).unwrap_or(&conflict.file_path);
          let local_path = format!("{}/{}", relationship.local_path, relative);

          // Rename local to .local-conflict
          let conflict_path = format!("{}.local-conflict", local_path);
          let _ = std::fs::rename(&local_path, &conflict_path);

          // Pull remote version
          if let Ok((bytes, _metadata)) = remote_client.download_file(&conflict.file_path).await {
            let _ = std::fs::write(&local_path, &bytes);
          }
        }
      }
      "kept_both"
    }
  };

  manager.resolve(&id)
    .map_err(|error| (
      StatusCode::INTERNAL_SERVER_ERROR,
      Json(serde_json::json!({ "error": error.to_string() })),
    ))?;

  Ok(Json(serde_json::json!({
    "resolved": true,
    "conflict_id": id,
    "file_path": conflict.file_path,
    "action": resolution_action,
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
