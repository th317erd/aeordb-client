use serde::{Deserialize, Serialize};

use crate::connections::{AuthType, RemoteConnection};
use crate::error::{ClientError, Result};

/// A remote aeordb directory entry, as returned by GET /engine/{directory_path}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
  pub name:         String,
  pub entry_type:   String,
  pub total_size:   u64,
  pub created_at:   i64,
  pub updated_at:   i64,
  pub content_type: Option<String>,
}

/// Downloaded file metadata from response headers.
#[derive(Debug, Clone)]
pub struct RemoteFileMetadata {
  pub path:         String,
  pub total_size:   u64,
  pub content_type: Option<String>,
  pub created_at:   Option<i64>,
  pub updated_at:   Option<i64>,
}

/// Client for talking to a remote aeordb instance.
pub struct RemoteClient {
  http_client: reqwest::Client,
  base_url:    String,
  api_key:     Option<String>,
}

impl RemoteClient {
  pub fn from_connection(connection: &RemoteConnection) -> Self {
    let api_key = if connection.auth_type == AuthType::ApiKey {
      connection.api_key.clone()
    } else {
      None
    };

    Self {
      http_client: reqwest::Client::new(),
      base_url:    connection.url.clone(),
      api_key,
    }
  }

  fn auth_header(&self) -> Option<String> {
    self.api_key.as_ref().map(|key| format!("Bearer {}", key))
  }

  /// List the contents of a remote directory.
  pub async fn list_directory(&self, remote_path: &str) -> Result<Vec<RemoteEntry>> {
    let url = format!("{}/engine{}", self.base_url, remote_path);

    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to list remote directory {}: {}", remote_path, error),
      ))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("remote returned HTTP {} for {}", response.status(), remote_path),
      ));
    }

    let entries: Vec<RemoteEntry> = response.json().await
      .map_err(|error| ClientError::Server(
        format!("failed to parse directory listing for {}: {}", remote_path, error),
      ))?;

    Ok(entries)
  }

  /// Download a file from the remote. Returns (bytes, metadata).
  pub async fn download_file(&self, remote_path: &str) -> Result<(Vec<u8>, RemoteFileMetadata)> {
    let url = format!("{}/engine{}", self.base_url, remote_path);

    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to download {}: {}", remote_path, error),
      ))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("remote returned HTTP {} for {}", response.status(), remote_path),
      ));
    }

    let headers = response.headers().clone();

    let path = headers.get("x-path")
      .and_then(|value| value.to_str().ok())
      .unwrap_or(remote_path)
      .to_string();

    let total_size = headers.get("x-total-size")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<u64>().ok())
      .unwrap_or(0);

    let content_type = headers.get("content-type")
      .and_then(|value| value.to_str().ok())
      .map(|value| value.to_string());

    let created_at = headers.get("x-created-at")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<i64>().ok());

    let updated_at = headers.get("x-updated-at")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<i64>().ok());

    let metadata = RemoteFileMetadata {
      path,
      total_size,
      content_type,
      created_at,
      updated_at,
    };

    let bytes = response.bytes().await
      .map_err(|error| ClientError::Server(
        format!("failed to read response body for {}: {}", remote_path, error),
      ))?;

    Ok((bytes.to_vec(), metadata))
  }

  /// Check if a remote path exists (HEAD request).
  pub async fn exists(&self, remote_path: &str) -> Result<bool> {
    let url = format!("{}/engine{}", self.base_url, remote_path);

    let mut request = self.http_client.head(&url);
    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to check existence of {}: {}", remote_path, error),
      ))?;

    Ok(response.status().is_success())
  }

  /// Upload a file to the remote aeordb instance.
  /// Uses the simple PUT /engine/{path} endpoint.
  pub async fn upload_file(
    &self,
    remote_path: &str,
    data: Vec<u8>,
    content_type: Option<&str>,
  ) -> Result<()> {
    let url = format!("{}/engine{}", self.base_url, remote_path);

    let mut request = self.http_client.put(&url).body(data);

    if let Some(content_type) = content_type {
      request = request.header("Content-Type", content_type);
    }

    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to upload {}: {}", remote_path, error),
      ))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("remote returned HTTP {} for PUT {}", response.status(), remote_path),
      ));
    }

    Ok(())
  }

  /// Delete a file on the remote aeordb instance.
  pub async fn delete_file(&self, remote_path: &str) -> Result<()> {
    let url = format!("{}/engine{}", self.base_url, remote_path);

    let mut request = self.http_client.delete(&url);
    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to delete remote {}: {}", remote_path, error),
      ))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("remote returned HTTP {} for DELETE {}", response.status(), remote_path),
      ));
    }

    Ok(())
  }
}
