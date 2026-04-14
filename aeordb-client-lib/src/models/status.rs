use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
  pub status:  String,
  pub version: String,
  pub uptime:  u64,
}

impl StatusResponse {
  pub fn new(uptime: u64) -> Self {
    Self {
      status:  "running".to_string(),
      version: env!("CARGO_PKG_VERSION").to_string(),
      uptime,
    }
  }
}
