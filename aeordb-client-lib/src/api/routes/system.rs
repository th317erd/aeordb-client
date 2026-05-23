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
///
/// Guards (must mirror the absent-Tauri-command checks since this is now
/// the single entry point for "open locally" affordances across the UI):
///   - Path must be absolute. Relative paths would resolve against the
///     binary's CWD, which is opaque to the WebView caller.
///   - Path must exist AND be a directory. Passing a file path through
///     `open::that` would launch the OS default handler for that file
///     (text editor for /etc/passwd, browser for .html, etc.), which is
///     a surprising and easily-misused side effect for a button labeled
///     "Open Locally."
pub async fn open_folder(
  Json(request): Json<OpenFolderRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
  let path = std::path::Path::new(&request.path);

  if !path.is_absolute() {
    return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
      "error": format!("path must be absolute; got: {}", request.path),
    })));
  }

  let metadata = match std::fs::metadata(path) {
    Ok(m) => m,
    Err(error) => {
      return (StatusCode::NOT_FOUND, Json(serde_json::json!({
        "error": format!("cannot access path '{}': {}", request.path, error),
      })));
    }
  };

  if !metadata.is_dir() {
    return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
      "error": format!("path is not a directory: {}", request.path),
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

/// POST /api/v1/pick-directory — open a native directory picker dialog.
/// Returns the selected directory path, or null if cancelled.
pub async fn pick_directory() -> (StatusCode, Json<serde_json::Value>) {
  let result = tokio::task::spawn_blocking(|| {
    rfd::FileDialog::new()
      .set_title("Select Directory")
      .pick_folder()
  }).await;

  match result {
    Ok(Some(path)) => {
      let path_str = path.to_string_lossy().to_string();
      tracing::info!("directory picked: {}", path_str);
      (StatusCode::OK, Json(serde_json::json!({
        "path": path_str,
      })))
    }
    Ok(None) => {
      (StatusCode::OK, Json(serde_json::json!({
        "path": null,
      })))
    }
    Err(error) => {
      tracing::error!("directory picker failed: {}", error);
      (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
        "error": format!("dialog failed: {}", error),
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
    // notify_waiters wakes ALL pending .notified() futures so both the
    // HTTP server (graceful axum::serve shutdown) and the Tauri-exit
    // bridge in main.rs receive the signal. notify_one would wake only
    // one — leaving the other half of the process alive.
    shutdown_signal.notify_waiters();
  }

  (StatusCode::OK, Json(serde_json::json!({
    "message": "shutdown initiated",
  })))
}
