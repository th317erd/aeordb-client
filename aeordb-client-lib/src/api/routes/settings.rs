use axum::extract::State;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::autostart;
use crate::error::ClientError;
use crate::server::AppState;

#[derive(Serialize)]
pub struct SettingsResponse {
  pub sync_interval_seconds: u64,
  pub auto_start_sync:       bool,
  pub auto_start_system:     bool,
  pub client_name:           Option<String>,
  pub config_dir:            String,
  pub data_dir:              String,
}

/// Partial update request. All fields are optional.
#[derive(Deserialize)]
pub struct UpdateSettingsRequest {
  pub sync_interval_seconds: Option<u64>,
  pub auto_start_sync:       Option<bool>,
  pub auto_start_system:     Option<bool>,
  pub client_name:           Option<String>,
}

pub async fn get_settings(
  State(state): State<AppState>,
) -> Result<Json<SettingsResponse>, ClientError> {
  let config = state.config_store.get().await?;

  Ok(Json(SettingsResponse {
    sync_interval_seconds: config.settings.sync_interval_seconds,
    auto_start_sync:       config.settings.auto_start_sync,
    auto_start_system:     config.settings.auto_start_system,
    client_name:           config.settings.client_name,
    config_dir:            state.config_dir.to_string_lossy().to_string(),
    data_dir:              state.data_dir.to_string_lossy().to_string(),
  }))
}

pub async fn update_settings(
  State(state): State<AppState>,
  Json(request): Json<UpdateSettingsRequest>,
) -> Result<Json<SettingsResponse>, ClientError> {
  // Validate sync_interval_seconds if provided.
  if let Some(interval) = request.sync_interval_seconds {
    if interval < 10 || interval > 3600 {
      return Err(ClientError::BadRequest(
        "sync_interval_seconds must be between 10 and 3600".to_string(),
      ));
    }
  }

  state.config_store.update(|config| {
    if let Some(interval) = request.sync_interval_seconds {
      config.settings.sync_interval_seconds = interval;
    }
    if let Some(auto_start) = request.auto_start_sync {
      config.settings.auto_start_sync = auto_start;
    }
    if let Some(ref client_name) = request.client_name {
      if client_name.is_empty() {
        config.settings.client_name = None;
      } else {
        config.settings.client_name = Some(client_name.clone());
      }
    }
    if let Some(auto_start_system) = request.auto_start_system {
      config.settings.auto_start_system = auto_start_system;
    }
  }).await?;

  // Install/remove system autostart if the setting changed.
  if let Some(enabled) = request.auto_start_system {
    if let Err(error) = autostart::set_autostart(enabled) {
      tracing::warn!("failed to update system autostart: {}", error);
    }
  }

  // Re-read to return the updated state.
  let config = state.config_store.get().await?;

  Ok(Json(SettingsResponse {
    sync_interval_seconds: config.settings.sync_interval_seconds,
    auto_start_sync:       config.settings.auto_start_sync,
    auto_start_system:     config.settings.auto_start_system,
    client_name:           config.settings.client_name,
    config_dir:            state.config_dir.to_string_lossy().to_string(),
    data_dir:              state.data_dir.to_string_lossy().to_string(),
  }))
}
