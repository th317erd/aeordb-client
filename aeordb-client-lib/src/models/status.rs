use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
  pub status:      String,
  pub version:     String,
  pub uptime:      u64,
  pub client_id:   Option<String>,
  pub client_name: Option<String>,
  pub config_dir:  Option<String>,
  pub data_dir:    Option<String>,
}

impl StatusResponse {
  pub fn new(uptime: u64) -> Self {
    Self {
      status:      "running".to_string(),
      version:     env!("CARGO_PKG_VERSION").to_string(),
      uptime,
      client_id:   None,
      client_name: None,
      config_dir:  None,
      data_dir:    None,
    }
  }

  pub fn with_identity(mut self, client_id: String, client_name: String) -> Self {
    self.client_id   = Some(client_id);
    self.client_name = Some(client_name);
    self
  }
}
