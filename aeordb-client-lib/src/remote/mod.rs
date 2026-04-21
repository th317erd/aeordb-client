use serde::{Deserialize, Serialize};

use crate::connections::{AuthType, RemoteConnection};
use crate::error::{ClientError, Result};

/// Entry type constants from aeordb.
pub const ENTRY_TYPE_FILE:      u8 = 2;
pub const ENTRY_TYPE_DIRECTORY: u8 = 3;
pub const ENTRY_TYPE_SYMLINK:   u8 = 8;

/// A remote aeordb directory entry, as returned by GET /files/{directory_path}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
  pub name:                        String,
  pub entry_type:                  u8,
  #[serde(default)]
  pub size:                        u64,
  #[serde(default)]
  pub created_at:                  i64,
  #[serde(default)]
  pub updated_at:                  i64,
  #[serde(default)]
  pub content_type:                Option<String>,
  #[serde(default)]
  pub path:                        Option<String>,
  #[serde(default)]
  pub hash:                        Option<String>,
  #[serde(default)]
  pub target:                      Option<String>,
}

impl RemoteEntry {
  pub fn is_file(&self) -> bool {
    self.entry_type == ENTRY_TYPE_FILE
  }

  pub fn is_directory(&self) -> bool {
    self.entry_type == ENTRY_TYPE_DIRECTORY
  }

  pub fn is_symlink(&self) -> bool {
    self.entry_type == ENTRY_TYPE_SYMLINK
  }
}

/// Downloaded file metadata from response headers.
#[derive(Debug, Clone)]
pub struct RemoteFileMetadata {
  pub path:         String,
  pub size:         u64,
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
  pub fn from_connection(connection: &RemoteConnection, http_client: &reqwest::Client) -> Self {
    let api_key = if connection.auth_type == AuthType::ApiKey {
      connection.api_key.clone()
    } else {
      None
    };

    Self {
      http_client: http_client.clone(),
      base_url:    connection.url.clone(),
      api_key,
    }
  }

  fn auth_header(&self) -> Option<String> {
    self.api_key.as_ref().map(|key| format!("Bearer {}", key))
  }

  /// List the contents of a remote directory.
  pub async fn list_directory(&self, remote_path: &str) -> Result<Vec<RemoteEntry>> {
    let url = format!("{}/files{}", self.base_url, remote_path);

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

    /// Wrapper for the collection response format: `{items: [...]}`.
    #[derive(Deserialize)]
    struct ItemsWrapper {
      items: Vec<RemoteEntry>,
    }

    let wrapper: ItemsWrapper = response.json().await
      .map_err(|error| ClientError::Server(
        format!("failed to parse directory listing for {}: {}", remote_path, error),
      ))?;

    Ok(wrapper.items)
  }

  /// Download a file from the remote as a streaming response.
  ///
  /// Returns the response and parsed metadata from headers. The caller is
  /// responsible for streaming the response body to disk (or wherever) in
  /// chunks, avoiding buffering the entire file in memory.
  pub async fn download_file(&self, remote_path: &str) -> Result<(reqwest::Response, RemoteFileMetadata)> {
    let url = format!("{}/files{}", self.base_url, remote_path);

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

    let path = headers.get("x-aeordb-path")
      .and_then(|value| value.to_str().ok())
      .unwrap_or(remote_path)
      .to_string();

    let size = headers.get("x-aeordb-size")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<u64>().ok())
      .unwrap_or(0);

    let content_type = headers.get("content-type")
      .and_then(|value| value.to_str().ok())
      .map(|value| value.to_string());

    let created_at = headers.get("x-aeordb-created-at")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<i64>().ok());

    let updated_at = headers.get("x-aeordb-updated-at")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<i64>().ok());

    let metadata = RemoteFileMetadata {
      path,
      size,
      content_type,
      created_at,
      updated_at,
    };

    Ok((response, metadata))
  }

  /// Check if a remote path exists (HEAD request).
  pub async fn exists(&self, remote_path: &str) -> Result<bool> {
    let url = format!("{}/files{}", self.base_url, remote_path);

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
  ///
  /// Accepts a `reqwest::Body` so the caller can provide either an in-memory
  /// buffer or a streaming body from a file on disk (via `Body::wrap_stream`).
  pub async fn upload_file(
    &self,
    remote_path: &str,
    body: reqwest::Body,
    content_type: Option<&str>,
  ) -> Result<()> {
    let url = format!("{}/files{}", self.base_url, remote_path);

    let mut request = self.http_client.put(&url).body(body);

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
    let url = format!("{}/files{}", self.base_url, remote_path);

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

  /// Create a symlink on the remote aeordb instance.
  /// Uses PUT /links/{path} with {"target": "..."} body.
  pub async fn create_symlink(&self, remote_path: &str, target: &str) -> Result<()> {
    let url = format!("{}/links{}", self.base_url, remote_path);

    let mut request = self.http_client
      .put(&url)
      .json(&serde_json::json!({ "target": target }));

    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to create symlink {}: {}", remote_path, error),
      ))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("remote returned HTTP {} for symlink {}", response.status(), remote_path),
      ));
    }

    Ok(())
  }

  /// Rename/move a file or directory on the remote.
  /// Uses PATCH /files/ with {"from": "...", "to": "..."} body.
  pub async fn rename_file(&self, from_path: &str, to_path: &str) -> Result<()> {
    let url = format!("{}/files/", self.base_url);

    let mut request = self.http_client
      .patch(&url)
      .json(&serde_json::json!({ "from": from_path, "to": to_path }));

    if let Some(ref auth) = self.auth_header() {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(
        format!("failed to rename {} to {}: {}", from_path, to_path, error),
      ))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("remote returned HTTP {} for rename {} to {}", response.status(), from_path, to_path),
      ));
    }

    Ok(())
  }

  /// List directory with pagination. Returns entries plus pagination metadata.
  pub async fn list_directory_paginated(
    &self,
    remote_path: &str,
    limit: Option<u64>,
    offset: Option<u64>,
  ) -> Result<DirectoryListingResponse> {
    let mut url = format!("{}/files{}", self.base_url, remote_path);

    let mut params = Vec::new();
    if let Some(limit) = limit {
      params.push(format!("limit={}", limit));
    }
    if let Some(offset) = offset {
      params.push(format!("offset={}", offset));
    }
    if !params.is_empty() {
      url = format!("{}?{}", url, params.join("&"));
    }

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

    let listing: DirectoryListingResponse = response.json().await
      .map_err(|error| ClientError::Server(
        format!("failed to parse directory listing for {}: {}", remote_path, error),
      ))?;

    Ok(listing)
  }
}

/// Paginated directory listing response from GET /files/{path}?limit=N&offset=M.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryListingResponse {
  pub items:  Vec<RemoteEntry>,
  #[serde(default)]
  pub total:  Option<u64>,
  #[serde(default)]
  pub limit:  Option<u64>,
  #[serde(default)]
  pub offset: Option<u64>,
}
