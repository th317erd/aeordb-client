use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::error::ClientError;
use crate::server::AppState;
use crate::sync::relationships::{
  CreateSyncRelationshipRequest, RelationshipManager, SyncDirection, SyncRelationship,
  UpdateSyncRelationshipRequest, normalize_remote_path,
};

pub async fn list_relationships(
  State(state): State<AppState>,
) -> Result<Json<Vec<SyncRelationship>>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.list().await.map(Json)
}

pub async fn create_relationship(
  State(state): State<AppState>,
  Json(request): Json<CreateSyncRelationshipRequest>,
) -> Result<(StatusCode, Json<SyncRelationship>), ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  let relationship = manager.create(request).await?;

  // Auto-start the sync runner for the new relationship
  if relationship.enabled {
    if let Err(error) = state.sync_runner.start(&relationship.id).await {
      tracing::warn!(
        "failed to auto-start sync for '{}': {}",
        relationship.name,
        error
      );
    }
  }

  Ok((StatusCode::CREATED, Json(relationship)))
}

pub async fn get_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);

  match manager.get(&id).await? {
    Some(relationship) => Ok(Json(relationship)),
    None => Err(ClientError::NotFound(format!(
      "sync relationship not found: {}",
      id
    ))),
  }
}

pub async fn update_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
  Json(request): Json<UpdateSyncRelationshipRequest>,
) -> Result<Json<SyncRelationship>, ClientError> {
  use crate::sync::metadata::SyncMetadataStore;

  let manager = RelationshipManager::new(&state.config_store);
  let existing = manager
    .get(&id)
    .await?
    .ok_or_else(|| ClientError::NotFound(format!("sync relationship not found: {}", id)))?;

  if let Some(local_path) = request.local_path.as_deref() {
    validate_local_sync_path(local_path)?;
  }

  let projected_local_path = request
    .local_path
    .clone()
    .unwrap_or_else(|| existing.local_path.clone());
  let projected_remote_path = request
    .remote_path
    .as_deref()
    .map(normalize_remote_path)
    .unwrap_or_else(|| existing.remote_path.clone());
  let paths_changed =
    projected_local_path != existing.local_path || projected_remote_path != existing.remote_path;

  let was_running = paths_changed && state.sync_runner.is_running(&id).await;
  if was_running {
    let _ = state.sync_runner.stop(&id).await;
  }

  let updated = match manager.update(&id, request).await {
    Ok(updated) => updated,
    Err(error) => {
      if was_running && existing.enabled {
        if let Err(start_error) = state.sync_runner.start(&id).await {
          tracing::warn!(
            "failed to restart sync '{}' after update failure: {}",
            existing.name,
            start_error,
          );
        }
      }
      return Err(error);
    }
  };

  if paths_changed {
    let metadata_store = SyncMetadataStore::new(&state.state_store);
    match updated.direction {
      SyncDirection::PushOnly | SyncDirection::Bidirectional => {
        metadata_store.begin_path_migration(
          &updated.id,
          &existing.remote_path,
          &updated.remote_path,
          &existing.local_path,
          &updated.local_path,
        )?;
        tracing::info!(
          "sync relationship '{}' root changed; queued path migration",
          updated.name,
        );
      }
      SyncDirection::PullOnly => {
        metadata_store.clear_relationship_state(&updated.id)?;
        tracing::info!(
          "sync relationship '{}' root changed in pull-only mode; cleared sync state",
          updated.name,
        );
      }
    }

    if updated.enabled {
      if let Err(error) = state.sync_runner.start(&id).await {
        tracing::warn!(
          "failed to start sync after path update for '{}': {}",
          updated.name,
          error,
        );
      }
    }
  }

  Ok(Json(updated))
}

pub async fn delete_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<StatusCode, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  manager.delete(&id).await.map(|_| StatusCode::NO_CONTENT)
}

pub async fn enable_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  let relationship = manager.enable(&id).await?;

  // Start the sync runner
  if let Err(error) = state.sync_runner.start(&id).await {
    tracing::warn!(
      "failed to start sync for '{}': {}",
      relationship.name,
      error
    );
  }

  Ok(Json(relationship))
}

pub async fn disable_relationship(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<SyncRelationship>, ClientError> {
  let manager = RelationshipManager::new(&state.config_store);
  let relationship = manager.disable(&id).await?;

  // Stop the sync runner
  let _ = state.sync_runner.stop(&id).await;

  Ok(Json(relationship))
}

pub async fn trigger_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  run_sync(state, id, false).await
}

/// Force-resync: clear the pull checkpoint, then run a Full push scan. This
/// refreshes remote permission/root-hash views without discarding per-file
/// metadata that push can use for content-hash comparisons.
pub async fn force_resync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  run_sync(state, id, true).await
}

async fn run_sync(
  state: AppState,
  id: String,
  force: bool,
) -> Result<Json<serde_json::Value>, ClientError> {
  use crate::connections::ConnectionManager;
  use crate::sync::metadata::SyncMetadataStore;
  use crate::sync::push::PushScanMode;
  use crate::sync::replication::sync_relationship;

  // Load relationship and connection.
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager
    .get(&id)
    .await?
    .ok_or_else(|| ClientError::NotFound(format!("sync relationship not found: {}", id)))?;

  if !relationship.enabled {
    return Err(ClientError::BadRequest(format!(
      "sync relationship '{}' is disabled",
      relationship.name,
    )));
  }

  let execution_guard = state.sync_runner.execution_guard(&id).await;
  let _sync_guard = match execution_guard.try_lock() {
    Ok(guard) => guard,
    Err(_) if !force => {
      return Ok(Json(serde_json::json!({
        "already_running": true,
        "message": format!("sync already in progress for '{}'", relationship.name),
        "push": null,
        "pull": null,
      })));
    }
    Err(_) => {
      return Err(ClientError::BadRequest(format!(
        "sync already in progress for relationship '{}'",
        relationship.name,
      )));
    }
  };

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager
    .get(&relationship.remote_connection_id)
    .await?
    .ok_or_else(|| ClientError::NotFound("connection not found".to_string()))?;

  if force {
    let metadata_store = SyncMetadataStore::new(&state.state_store);
    metadata_store.clear_checkpoint(&id).map_err(|error| {
      ClientError::Server(format!("failed to clear sync checkpoint: {}", error))
    })?;
    tracing::info!(
      "force-resync: cleared checkpoint and will run a Full push scan for '{}'",
      relationship.name
    );
  }

  // Run the sync (push and/or pull based on direction). Pass the
  // shared JWT cache so the trigger call reuses the cached token
  // instead of minting a fresh one on the engine.
  let all_relationships = relationship_manager.list().await.unwrap_or_default();
  let progress = crate::sync::push::PushProgressReporter::new(
    &id,
    &relationship.name,
    state.sync_runner.activity_log(),
    &state.event_tx,
  );
  let result = sync_relationship(
    &state.state_store,
    &connection,
    &relationship,
    &all_relationships,
    &state.http_client,
    &state.jwt_cache,
    if force {
      PushScanMode::Full
    } else {
      PushScanMode::Lite
    },
    Some(&progress),
  )
  .await
  .map_err(|error| ClientError::Server(error.to_string()))?;

  // Log to activity feed (non-fatal).
  let activity = state.sync_runner.activity_log();
  if let Err(error) = activity.log_full_sync(&id, &relationship.name, &result) {
    tracing::warn!("failed to log trigger activity: {}", error);
  }

  // Broadcast event via SSE.
  {
    use crate::sync::activity::SyncEvent;
    use uuid::Uuid;

    let mut files_affected: u64 = 0;
    let mut bytes_transferred: u64 = 0;
    let mut duration_ms: u64 = 0;
    let mut errors: Vec<String> = Vec::new();

    if let Some(ref pull) = result.pull {
      files_affected += pull.files_pulled + pull.files_deleted + pull.symlinks_pulled;
      bytes_transferred += pull.total_bytes;
      duration_ms += pull.duration_ms;
      errors.extend(pull.errors.iter().cloned());
    }
    if let Some(ref push) = result.push {
      files_affected += push.files_pushed + push.files_deleted;
      bytes_transferred += push.total_bytes;
      duration_ms += push.duration_ms;
      errors.extend(push.errors.iter().cloned());
    }

    let summary = crate::sync::activity::summarize_full_sync_result(&result);
    let event = SyncEvent {
      id: Uuid::new_v4().to_string(),
      relationship_id: id.clone(),
      relationship_name: relationship.name.clone(),
      event_type: "full_sync".to_string(),
      summary,
      files_affected,
      bytes_transferred,
      duration_ms,
      errors,
      progress_percent: None,
      timestamp: chrono::Utc::now().timestamp_millis(),
    };

    if let Ok(json) = serde_json::to_string(&event) {
      let _ = state
        .event_tx
        .send(crate::server::ServerEvent::new("sync_activity", json));
    }
  }

  // Build a response summarizing what happened.
  let push_summary = result.push.map(|p| {
    serde_json::json!({
      "files_pushed":  p.files_pushed,
      "files_skipped": p.files_skipped,
      "files_failed":  p.files_failed,
      "files_deleted": p.files_deleted,
      "total_bytes":   p.total_bytes,
      "duration_ms":   p.duration_ms,
      "errors":        p.errors,
    })
  });

  let pull_summary = result.pull.map(|p| {
    serde_json::json!({
      "files_pulled":    p.files_pulled,
      "files_skipped":   p.files_skipped,
      "files_failed":    p.files_failed,
      "files_deleted":   p.files_deleted,
      "symlinks_pulled": p.symlinks_pulled,
      "total_bytes":     p.total_bytes,
      "duration_ms":     p.duration_ms,
      "errors":          p.errors,
    })
  });

  Ok(Json(serde_json::json!({
    "push": push_summary,
    "pull": pull_summary,
  })))
}

fn validate_local_sync_path(local_path: &str) -> Result<(), ClientError> {
  let path = std::path::Path::new(local_path);
  if !path.exists() {
    std::fs::create_dir_all(path).map_err(|error| {
      ClientError::Configuration(format!(
        "cannot create local path '{}': {}",
        local_path, error,
      ))
    })?;
    tracing::info!("created local sync directory: {}", local_path);
  }

  if !path.is_dir() {
    return Err(ClientError::Configuration(format!(
      "local path is not a directory: {}",
      local_path,
    )));
  }

  Ok(())
}

pub async fn start_sync(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  state
    .sync_runner
    .start(&id)
    .await
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
  state
    .sync_runner
    .stop(&id)
    .await
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

pub async fn pause_all_sync(State(state): State<AppState>) -> Json<serde_json::Value> {
  state.sync_runner.stop_all().await;
  Json(serde_json::json!({ "message": "all sync runners paused" }))
}

pub async fn resume_all_sync(State(state): State<AppState>) -> Json<serde_json::Value> {
  state.sync_runner.start_all_enabled().await;
  Json(serde_json::json!({ "message": "all enabled sync runners resumed" }))
}

pub async fn sync_runner_status(
  State(state): State<AppState>,
) -> Json<Vec<crate::sync::runner::SyncRunnerStatus>> {
  Json(state.sync_runner.status(&state.health_map).await)
}

pub async fn get_sync_activity(
  State(state): State<AppState>,
  Path(id): Path<String>,
) -> Result<Json<Vec<crate::sync::activity::SyncEvent>>, ClientError> {
  state
    .sync_runner
    .activity_log()
    .get_events(&id, 50)
    .map(Json)
    .map_err(|error| ClientError::Server(error.to_string()))
}
