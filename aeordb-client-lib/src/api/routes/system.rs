use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use crate::server::AppState;

#[derive(Deserialize)]
pub struct OpenFolderRequest {
  pub path: String,
}

/// POST /api/v1/open-folder — open a directory in the native file explorer.
pub async fn open_folder(
  Json(request): Json<OpenFolderRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
  let path = std::path::Path::new(&request.path);

  if !path.exists() {
    return (StatusCode::NOT_FOUND, Json(serde_json::json!({
      "error": format!("path does not exist: {}", request.path),
    })));
  }

  match open::that(&request.path) {
    Ok(()) => {
      tracing::info!("opened folder: {}", request.path);
      (StatusCode::OK, Json(serde_json::json!({
        "message": format!("opened {}", request.path),
      })))
    }
    Err(error) => {
      tracing::error!("failed to open folder {}: {}", request.path, error);
      (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
        "error": format!("failed to open folder: {}", error),
      })))
    }
  }
}

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
