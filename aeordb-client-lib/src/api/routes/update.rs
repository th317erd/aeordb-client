//! Self-update API surface.
//!
//! - `GET /api/v1/update/status` — latest cached snapshot of the
//!   `/api/version` poll. Always returns 200 with whatever's in
//!   `AppState.update_info`; fields are sparse if the startup poll
//!   hasn't completed yet.
//! - `POST /api/v1/update/check` — force a fresh poll right now. Used
//!   by a "Check for updates" button; the startup poll covers the
//!   common path.
//! - `POST /api/v1/update/apply` — streamed apply with NDJSON progress.
//!   On success the process exits ~500ms after the last event so the
//!   relauncher script can swap the binary and re-launch.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Json, Response};

use crate::server::AppState;
use crate::update;

pub async fn update_status(State(state): State<AppState>) -> Json<update::UpdateInfo> {
  let info = state.update_info.read().map(|g| g.clone()).unwrap_or_default();
  Json(info)
}

pub async fn update_check(
  State(state): State<AppState>,
) -> Result<Json<update::UpdateInfo>, (StatusCode, String)> {
  let client = reqwest::Client::new();
  update::check_once(&client, &state.update_info).await;
  let info = state.update_info.read().map(|g| g.clone()).unwrap_or_default();
  Ok(Json(info))
}

/// NDJSON-streamed apply. Each line is a JSON `ProgressEvent`. After
/// the stream closes the process exits 500ms later — the relauncher
/// (already PID-polling) takes over from there.
pub async fn update_apply(
  State(state): State<AppState>,
) -> Result<Response, (StatusCode, String)> {
  use axum::body::Body;

  let info = state.update_info.read().map(|g| g.clone()).unwrap_or_default();
  if !info.available {
    return Err((StatusCode::CONFLICT, "no update available".to_string()));
  }

  let (tx, mut rx) = tokio::sync::mpsc::channel::<update::ProgressEvent>(32);
  let info_for_task = info.clone();
  tokio::spawn(async move {
    let result = update::apply_update(&info_for_task, Some(tx.clone())).await;
    if let Err(e) = result {
      let _ = tx.send(update::ProgressEvent::Error { message: e.to_string() }).await;
    }
    // Drop the sender so the stream terminates, then give the last
    // line ~500ms to flush before we exit and the connection drops.
    drop(tx);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    tracing::info!("update applied — exiting to let relauncher swap in new binary");
    std::process::exit(0);
  });

  let stream = async_stream::stream! {
    while let Some(event) = rx.recv().await {
      let line = serde_json::to_string(&event).unwrap_or_default();
      yield Ok::<String, std::convert::Infallible>(format!("{line}\n"));
    }
  };
  let body = Body::from_stream(stream);

  let response = axum::response::Response::builder()
    .status(StatusCode::OK)
    .header("Content-Type", "application/x-ndjson")
    .header("Cache-Control", "no-cache")
    // Disable proxy buffering (nginx-style) so the NDJSON lines arrive
    // at the renderer in real time. The dev daemon doesn't sit behind
    // a proxy, but the production build might, and a stalled progress
    // bar reads as a hung update.
    .header("X-Accel-Buffering", "no")
    .body(body)
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("response build: {e}")))?;
  Ok(response)
}
