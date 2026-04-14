use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;

use crate::models::status::StatusResponse;
use crate::server::AppState;

pub async fn get_status(
  State(state): State<AppState>,
) -> (StatusCode, Json<StatusResponse>) {
  let uptime = state.started_at.elapsed().as_secs();
  let response = StatusResponse::new(uptime);

  (StatusCode::OK, Json(response))
}
