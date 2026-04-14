use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
  #[error("server error: {0}")]
  Server(String),

  #[error("configuration error: {0}")]
  Configuration(String),

  #[error("io error: {0}")]
  Io(#[from] std::io::Error),

  #[error("serialization error: {0}")]
  Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ClientError>;
