use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;

use crate::models::status::StatusResponse;
use crate::server::AppState;

pub async fn get_status(
  State(state): State<AppState>,
) -> (StatusCode, Json<StatusResponse>) {
  let uptime   = state.started_at.elapsed().as_secs();
  let mut response = StatusResponse::new(uptime);

  if let Ok(identity) = state.state_store.get_or_create_identity() {
    response = response.with_identity(identity.id, identity.name);
  }

  response.config_dir = Some(state.config_dir.to_string_lossy().to_string());
  response.data_dir   = Some(state.data_dir.to_string_lossy().to_string());

  (StatusCode::OK, Json(response))
}
