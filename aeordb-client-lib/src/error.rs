use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
  #[error("server error: {0}")]
  Server(String),

  #[error("configuration error: {0}")]
  Configuration(String),

  #[error("not found: {0}")]
  NotFound(String),

  #[error("bad request: {0}")]
  BadRequest(String),

  #[error("forbidden: {0}")]
  Forbidden(String),

  #[error("bad gateway: {0}")]
  BadGateway(String),

  /// Couldn't reach the upstream engine at all — connection refused,
  /// DNS failure, TLS handshake error, timeout, etc. Distinct from
  /// BadGateway (which historically conflated this with "engine
  /// responded but unhappily") because the user's actionable fix is
  /// different (start the engine / check the network), so the wire
  /// category matters for the UI to render a useful message.
  #[error("upstream unreachable: {0}")]
  UpstreamUnreachable(String),

  /// Upstream engine returned a 5xx. The engine is reachable, it's
  /// just unhappy. Status code is preserved so callers can
  /// distinguish 500 / 502 / 503 / etc.
  #[error("upstream server error (HTTP {status}): {message}")]
  UpstreamServer { status: u16, message: String },

  /// Upstream returned a response we couldn't parse — JSON shape
  /// mismatch, malformed body, etc. Usually means engine/client
  /// versions have drifted.
  #[error("upstream protocol error: {0}")]
  UpstreamProtocol(String),

  /// Upstream returned a 4xx — 403, 404, etc. Status code is
  /// preserved but **passive** callers (browse / list, where the user
  /// didn't initiate the operation that got rejected) SHOULD NOT use
  /// the distinction between 403 and 404 to drive user-visible
  /// behavior. See the DB team's 2026-05-23 retraction note
  /// (`aeordb-client/bot-docs/bug-reports/2026-05-22-pull-sync-
  /// silently-treats-permission-failures-as-success.md`). Passive
  /// renderers should treat all 4xx the same (e.g. "empty folder")
  /// to avoid leaking denial vs. nonexistent. User-initiated ops
  /// (delete, rename, upload) ARE free to surface the rejection.
  #[error("upstream rejected (HTTP {status}): {message}")]
  UpstreamRejected { status: u16, message: String },

  #[error("io error: {0}")]
  Io(#[from] std::io::Error),

  #[error("serialization error: {0}")]
  Serialization(#[from] serde_json::Error),
}

impl ClientError {
  pub fn is_transient_upstream(&self) -> bool {
    match self {
      ClientError::UpstreamUnreachable(_) => true,
      ClientError::UpstreamServer { status, .. } => matches!(*status, 502 | 503 | 504),
      // Older remote helpers still surface some upstream failures as generic
      // server strings. Keep this fallback until every remote path is
      // structured.
      ClientError::Server(message) => {
        let lower = message.to_ascii_lowercase();
        lower.contains("http 502")
          || lower.contains("http 503")
          || lower.contains("http 504")
          || lower.contains("bad gateway")
          || lower.contains("service unavailable")
          || lower.contains("gateway timeout")
          || lower.contains("connection refused")
          || lower.contains("connection reset")
          || lower.contains("timed out")
          || lower.contains("timeout")
      }
      _ => false,
    }
  }
}

/// Short category tag emitted in the JSON error body. UI branches on
/// this rather than parsing the human-readable message — `category`
/// is the stable contract; `error` is human prose that may change.
fn category_for(err: &ClientError) -> &'static str {
  match err {
    ClientError::NotFound(_) => "not_found",
    ClientError::BadRequest(_) => "bad_request",
    ClientError::Forbidden(_) => "forbidden",
    ClientError::BadGateway(_) => "bad_gateway",
    ClientError::Configuration(_) => "configuration",
    ClientError::Server(_) => "server",
    ClientError::Io(_) => "io",
    ClientError::Serialization(_) => "serialization",
    ClientError::UpstreamUnreachable(_) => "upstream_unreachable",
    ClientError::UpstreamServer { .. } => "upstream_server",
    ClientError::UpstreamProtocol(_) => "upstream_protocol",
    ClientError::UpstreamRejected { .. } => "upstream_rejected",
  }
}

impl IntoResponse for ClientError {
  fn into_response(self) -> Response {
    let status = match &self {
      ClientError::NotFound(_) => StatusCode::NOT_FOUND,
      ClientError::BadRequest(_) => StatusCode::BAD_REQUEST,
      ClientError::Forbidden(_) => StatusCode::FORBIDDEN,
      ClientError::BadGateway(_) => StatusCode::BAD_GATEWAY,
      ClientError::Configuration(_) => StatusCode::BAD_REQUEST,
      ClientError::Server(_) => StatusCode::INTERNAL_SERVER_ERROR,
      ClientError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
      ClientError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
      ClientError::UpstreamUnreachable(_) => StatusCode::BAD_GATEWAY,
      ClientError::UpstreamServer { .. } => StatusCode::BAD_GATEWAY,
      ClientError::UpstreamProtocol(_) => StatusCode::BAD_GATEWAY,
      // Mirror the upstream status when it was a 4xx so the proxy
      // doesn't lie about what the engine said. Falls back to 502
      // if the upstream status doesn't map to a known StatusCode.
      ClientError::UpstreamRejected { status, .. } => {
        StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
      }
    };

    let category = category_for(&self);
    let body = serde_json::json!({
      "error":    self.to_string(),
      "category": category,
    });
    (status, Json(body)).into_response()
  }
}

pub type Result<T> = std::result::Result<T, ClientError>;
