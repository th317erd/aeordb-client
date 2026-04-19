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

  #[error("io error: {0}")]
  Io(#[from] std::io::Error),

  #[error("serialization error: {0}")]
  Serialization(#[from] serde_json::Error),
}

impl IntoResponse for ClientError {
  fn into_response(self) -> Response {
    let status = match &self {
      ClientError::NotFound(_)      => StatusCode::NOT_FOUND,
      ClientError::BadRequest(_)    => StatusCode::BAD_REQUEST,
      ClientError::Forbidden(_)     => StatusCode::FORBIDDEN,
      ClientError::BadGateway(_)    => StatusCode::BAD_GATEWAY,
      ClientError::Configuration(_) => StatusCode::BAD_REQUEST,
      ClientError::Server(_)        => StatusCode::INTERNAL_SERVER_ERROR,
      ClientError::Io(_)            => StatusCode::INTERNAL_SERVER_ERROR,
      ClientError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };

    let body = serde_json::json!({ "error": self.to_string() });
    (status, Json(body)).into_response()
  }
}

pub type Result<T> = std::result::Result<T, ClientError>;
