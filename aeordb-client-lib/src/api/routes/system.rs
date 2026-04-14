use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;

use crate::server::AppState;

/// POST /api/v1/shutdown — initiate graceful shutdown.
pub async fn shutdown(
  State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
  tracing::info!("shutdown requested via API");

  if let Some(ref shutdown_signal) = state.shutdown_signal {
    shutdown_signal.notify_one();
  }

  (StatusCode::OK, Json(serde_json::json!({
    "message": "shutdown initiated",
  })))
}
