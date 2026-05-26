use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::Json;
use serde_json::Value;

use crate::error::ClientError;
use crate::preferences::UserPreferences;
use crate::server::AppState;

/// GET /api/v1/preferences
///
/// Returns the current per-installation preferences as JSON. Schema is
/// defined by `crate::preferences::UserPreferences`.
pub async fn get_preferences(
  State(state): State<AppState>,
) -> Json<UserPreferences> {
  Json(state.preferences.get().await)
}

/// PATCH /api/v1/preferences
///
/// Applies an RFC 7396 JSON Merge Patch to the on-disk preferences,
/// matching the engine's `PATCH /files/{path}` semantics. Returns the
/// updated full document.
///
/// Content-Type MUST be `application/merge-patch+json` — this is the
/// same discriminator the engine uses and keeps the protocol consistent
/// for renderer code.
pub async fn patch_preferences(
  State(state): State<AppState>,
  headers: HeaderMap,
  body: axum::body::Bytes,
) -> Result<Json<UserPreferences>, ClientError> {
  // Content-Type discipline. Reject anything else explicitly so a
  // future overload (e.g. wholesale-replace via application/json)
  // doesn't silently land in this handler.
  let content_type = headers.get(header::CONTENT_TYPE)
    .and_then(|v| v.to_str().ok())
    .unwrap_or("");
  if !content_type.starts_with("application/merge-patch+json") {
    return Err(ClientError::BadRequest(format!(
      "PATCH /preferences requires Content-Type: application/merge-patch+json (got {:?})",
      content_type,
    )));
  }

  // Empty patch body is a no-op; treat as success.
  if body.is_empty() {
    return Ok(Json(state.preferences.get().await));
  }

  let patch: Value = serde_json::from_slice(&body).map_err(|error| {
    ClientError::BadRequest(format!("preferences patch is not valid JSON: {}", error))
  })?;

  let updated = state.preferences.merge_patch(patch).await?;
  Ok(Json(updated))
}
