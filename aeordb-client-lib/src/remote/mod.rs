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
  #[serde(default)]
  pub effective_permissions:       Option<String>,
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

/// Classify a `reqwest::Error` from `.send()` into a categorized
/// ClientError. Distinguishes "couldn't even reach the engine"
/// (UpstreamUnreachable — connect/timeout/DNS/TLS) from other
/// transport-layer issues that fall through to UpstreamServer.
/// Without this, every transport failure used to map to
/// ClientError::Server, which the UI then rendered as "the server
/// denied access" — wildly misleading for a connection-refused.
fn classify_reqwest_send_error(error: &reqwest::Error, remote_path: &str) -> ClientError {
  // is_connect: TCP/DNS/TLS failed. is_timeout: request timed out
  // before any response. is_request: malformed request (rare; we
  // build the URL ourselves so this would be a bug not a network
  // issue, but treat as protocol).
  if error.is_connect() || error.is_timeout() {
    ClientError::UpstreamUnreachable(
      format!("couldn't reach engine for {}: {}", remote_path, error),
    )
  } else if error.is_request() {
    ClientError::UpstreamProtocol(
      format!("malformed request to engine for {}: {}", remote_path, error),
    )
  } else {
    // Catch-all: body decode errors, etc. Treat as unreachable since
    // the user's likely action is the same (retry / check network).
    ClientError::UpstreamUnreachable(
      format!("transport error talking to engine for {}: {}", remote_path, error),
    )
  }
}

/// Classify a non-success HTTP status from the upstream engine into
/// a categorized ClientError. 4xx → UpstreamRejected (status code
/// preserved; passive callers should NOT distinguish 403 from 404
/// in user-visible UI per the DB team's 2026-05-23 retraction).
/// 5xx → UpstreamServer. Other → UpstreamServer as a safe fallback.
fn classify_upstream_status(status: u16, body: &str, remote_path: &str) -> ClientError {
  let body_excerpt = body.chars().take(200).collect::<String>();
  if (400..500).contains(&status) {
    ClientError::UpstreamRejected {
      status,
      message: format!("engine refused {}: {}", remote_path, body_excerpt),
    }
  } else {
    ClientError::UpstreamServer {
      status,
      message: format!("engine failed for {}: {}", remote_path, body_excerpt),
    }
  }
}

/// Client for talking to a remote aeordb instance.
/// Handles JWT token exchange: exchanges the API key for a JWT on first
/// authenticated request, caches it, and re-exchanges on 401.
pub struct RemoteClient {
  http_client: reqwest::Client,
  base_url:    String,
  api_key:     Option<String>,
  jwt_token:   std::sync::Mutex<Option<String>>,
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
      base_url:    connection.base_url(),
      api_key,
      jwt_token:   std::sync::Mutex::new(None),
    }
  }

  /// Get the auth header, exchanging API key for JWT if needed.
  pub async fn auth_header(&self) -> Option<String> {
    let api_key = self.api_key.as_ref()?;

    // Check for cached JWT
    {
      let cached = self.jwt_token.lock().unwrap();
      if let Some(ref token) = *cached {
        return Some(format!("Bearer {}", token));
      }
    }

    // Exchange API key for JWT
    match self.exchange_token(api_key).await {
      Ok(token) => {
        let header = format!("Bearer {}", token);
        *self.jwt_token.lock().unwrap() = Some(token);
        Some(header)
      }
      Err(error) => {
        tracing::warn!("JWT token exchange failed: {}", error);
        // Fall back to raw API key
        Some(format!("Bearer {}", api_key))
      }
    }
  }

  /// Clear the cached JWT (e.g., on 401) so the next request re-exchanges.
  fn invalidate_token(&self) {
    *self.jwt_token.lock().unwrap() = None;
  }

  /// Exchange an API key for a JWT token via POST /auth/token.
  async fn exchange_token(&self, api_key: &str) -> Result<String> {
    let url = format!("{}/auth/token", self.base_url);
    let response = self.http_client
      .post(&url)
      .json(&serde_json::json!({ "api_key": api_key }))
      .send()
      .await
      .map_err(|e| ClientError::Server(format!("token exchange failed: {}", e)))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("token exchange returned HTTP {}", response.status()),
      ));
    }

    let body: serde_json::Value = response.json().await
      .map_err(|e| ClientError::Server(format!("token exchange response parse failed: {}", e)))?;

    body.get("token")
      .and_then(|t| t.as_str())
      .map(|s| s.to_string())
      .ok_or_else(|| ClientError::Server("token exchange response missing 'token' field".to_string()))
  }

  /// List the contents of a remote directory.
  pub async fn list_directory(&self, remote_path: &str) -> Result<Vec<RemoteEntry>> {
    let url = format!("{}/files{}", self.base_url, remote_path);

    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header().await {
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
    if let Some(ref auth) = self.auth_header().await {
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
    if let Some(ref auth) = self.auth_header().await {
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

    if let Some(ref auth) = self.auth_header().await {
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
    if let Some(ref auth) = self.auth_header().await {
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

    if let Some(ref auth) = self.auth_header().await {
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
  /// Uses PATCH /files/{from_path} with {"to": "..."} body.
  pub async fn rename_file(&self, from_path: &str, to_path: &str) -> Result<()> {
    let clean_from = from_path.trim_start_matches('/');
    let url = format!("{}/files/{}", self.base_url, clean_from);

    let mut request = self.http_client
      .patch(&url)
      .json(&serde_json::json!({ "to": to_path }));

    if let Some(ref auth) = self.auth_header().await {
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
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| classify_reqwest_send_error(&error, remote_path))?;

    let status = response.status();
    if !status.is_success() {
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(status.as_u16(), &body, remote_path));
    }

    let listing: DirectoryListingResponse = response.json().await
      .map_err(|error| ClientError::UpstreamProtocol(
        format!("failed to parse directory listing for {}: {}", remote_path, error),
      ))?;

    Ok(listing)
  }

  /// Get shares for a path. GET /files/shares?path=...
  pub async fn get_shares(&self, path: &str) -> Result<serde_json::Value> {
    let url = format!("{}/files/shares?path={}", self.base_url, path);
    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to get shares: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("get shares returned HTTP {}", response.status())));
    }
    response.json().await
      .map_err(|e| ClientError::Server(format!("failed to parse shares response: {}", e)))
  }

  /// Grant share access. POST /files/share
  pub async fn share(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}/files/share", self.base_url);
    let mut request = self.http_client.post(&url).json(body);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to share: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("share returned HTTP {}", response.status())));
    }
    // Some share endpoints return empty 200, handle gracefully
    let text = response.text().await.unwrap_or_default();
    if text.is_empty() {
      Ok(serde_json::json!({"ok": true}))
    } else {
      serde_json::from_str(&text)
        .map_err(|e| ClientError::Server(format!("failed to parse share response: {}", e)))
    }
  }

  /// Revoke share access. DELETE /files/shares (with JSON body)
  pub async fn unshare(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}/files/shares", self.base_url);
    let mut request = self.http_client.delete(&url).json(body);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to unshare: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("unshare returned HTTP {}", response.status())));
    }
    let text = response.text().await.unwrap_or_default();
    if text.is_empty() {
      Ok(serde_json::json!({"ok": true}))
    } else {
      serde_json::from_str(&text)
        .map_err(|e| ClientError::Server(format!("failed to parse unshare response: {}", e)))
    }
  }

  /// Get users that can receive shares. GET /auth/keys/users
  pub async fn get_shareable_users(&self) -> Result<serde_json::Value> {
    let url = format!("{}/auth/keys/users", self.base_url);
    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to get users: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("get users returned HTTP {}", response.status())));
    }
    response.json().await
      .map_err(|e| ClientError::Server(format!("failed to parse users response: {}", e)))
  }

  /// Get groups that can receive shares. GET /system/groups
  pub async fn get_shareable_groups(&self) -> Result<serde_json::Value> {
    let url = format!("{}/system/groups", self.base_url);
    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to get groups: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("get groups returned HTTP {}", response.status())));
    }
    response.json().await
      .map_err(|e| ClientError::Server(format!("failed to parse groups response: {}", e)))
  }

  /// Create a share link. POST /files/share-link
  /// The caller should set base_url in the body before calling.
  pub async fn create_share_link(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}/files/share-link", self.base_url);
    let mut request = self.http_client.post(&url).json(body);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to create share link: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("create share link returned HTTP {}", response.status())));
    }
    response.json().await
      .map_err(|e| ClientError::Server(format!("failed to parse share link response: {}", e)))
  }

  /// Get active share links. GET /files/share-links?path=...
  pub async fn get_share_links(&self, path: &str) -> Result<serde_json::Value> {
    let url = format!("{}/files/share-links?path={}", self.base_url, path);
    let mut request = self.http_client.get(&url);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to get share links: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("get share links returned HTTP {}", response.status())));
    }
    response.json().await
      .map_err(|e| ClientError::Server(format!("failed to parse share links response: {}", e)))
  }

  /// Revoke a share link. DELETE /files/share-links/{key_id}
  pub async fn revoke_share_link(&self, key_id: &str) -> Result<serde_json::Value> {
    let url = format!("{}/files/share-links/{}", self.base_url, key_id);
    let mut request = self.http_client.delete(&url);
    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }
    let response = request.send().await
      .map_err(|e| ClientError::Server(format!("failed to revoke share link: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!("revoke share link returned HTTP {}", response.status())));
    }
    let text = response.text().await.unwrap_or_default();
    if text.is_empty() {
      Ok(serde_json::json!({"ok": true}))
    } else {
      serde_json::from_str(&text)
        .map_err(|e| ClientError::Server(format!("failed to parse revoke response: {}", e)))
    }
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
