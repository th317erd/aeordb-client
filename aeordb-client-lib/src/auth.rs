use std::path::Path;

use axum::http::HeaderMap;

use crate::error::{ClientError, Result};

/// Generate a random auth token.
pub fn generate_token() -> String {
  use rand::Rng;
  let mut rng   = rand::rng();
  let bytes: [u8; 32] = rng.random();
  hex::encode(bytes)
}

/// Load or create the auth token file.
/// Returns the token string.
pub fn load_or_create_token(auth_file_path: &str) -> Result<String> {
  let path = Path::new(auth_file_path);

  if path.exists() {
    let token = std::fs::read_to_string(path)
      .map_err(|error| ClientError::Configuration(
        format!("failed to read auth token from {}: {}", auth_file_path, error),
      ))?;

    let token = token.trim().to_string();
    if token.is_empty() {
      return Err(ClientError::Configuration(
        format!("auth token file is empty: {}", auth_file_path),
      ));
    }

    return Ok(token);
  }

  // Create a new token
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)?;
  }

  let token = generate_token();
  std::fs::write(path, &token)?;

  // Set file permissions to 0600 (owner read/write only) on Unix
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions)?;
  }

  tracing::info!("generated auth token at {}", auth_file_path);
  Ok(token)
}

/// Validate a Bearer token from request headers.
pub fn validate_token(headers: &HeaderMap, expected_token: &str) -> bool {
  let auth_header = headers
    .get("Authorization")
    .and_then(|value| value.to_str().ok())
    .unwrap_or("");

  if let Some(token) = auth_header.strip_prefix("Bearer ") {
    token.trim() == expected_token
  } else {
    false
  }
}
