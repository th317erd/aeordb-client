use serde::{Deserialize, Serialize};

use crate::connections::{AuthType, RemoteConnection};
use crate::error::{ClientError, Result};

/// Server upload configuration from GET /upload/config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadConfig {
  pub hash_algorithm:    String,
  pub chunk_size:        usize,
  pub chunk_hash_prefix: String,
}

/// Result of a dedup check from POST /upload/check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupCheckResult {
  pub have:   Vec<String>,
  pub needed: Vec<String>,
}

/// A file to commit via POST /upload/commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitFile {
  pub path:         String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub content_type: Option<String>,
  pub chunks:       Vec<String>,
}

/// A chunk of a file, with its hash and data.
#[derive(Debug, Clone)]
pub struct FileChunk {
  pub hash: String,
  pub data: Vec<u8>,
}

/// Upload client that implements the 4-phase upload protocol.
pub struct UploadClient {
  http_client: reqwest::Client,
  base_url:    String,
  auth_header: Option<String>,
  config:      Option<UploadConfig>,
}

impl UploadClient {
  pub fn new(connection: &RemoteConnection) -> Self {
    let auth_header = if connection.auth_type == AuthType::ApiKey {
      connection.api_key.as_ref().map(|key| format!("Bearer {}", key))
    } else {
      None
    };

    Self {
      http_client: reqwest::Client::new(),
      base_url:    connection.url.clone(),
      auth_header,
      config:      None,
    }
  }

  /// Phase 1: Negotiate upload configuration.
  pub async fn get_config(&mut self) -> Result<UploadConfig> {
    if let Some(ref config) = self.config {
      return Ok(config.clone());
    }

    let url      = format!("{}/upload/config", self.base_url);
    let response = self.http_client.get(&url).send().await
      .map_err(|error| ClientError::Server(format!("upload config failed: {}", error)))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("upload config returned HTTP {}", response.status()),
      ));
    }

    let config: UploadConfig = response.json().await
      .map_err(|error| ClientError::Server(format!("parse upload config: {}", error)))?;

    self.config = Some(config.clone());
    Ok(config)
  }

  /// Split a file into chunks and compute hashes.
  pub fn chunk_file(&self, data: &[u8], config: &UploadConfig) -> Vec<FileChunk> {
    let chunk_size = config.chunk_size;
    let prefix     = config.chunk_hash_prefix.as_bytes();
    let mut chunks = Vec::new();

    let mut offset = 0;
    while offset < data.len() {
      let end        = (offset + chunk_size).min(data.len());
      let chunk_data = &data[offset..end];

      // Hash with prefix: blake3("chunk:" + chunk_bytes)
      let mut hasher = blake3::Hasher::new();
      hasher.update(prefix);
      hasher.update(chunk_data);
      let hash = hasher.finalize().to_hex().to_string();

      chunks.push(FileChunk {
        hash,
        data: chunk_data.to_vec(),
      });

      offset = end;
    }

    chunks
  }

  /// Phase 2: Check which chunks the server already has.
  pub async fn check_dedup(&self, hashes: &[String]) -> Result<DedupCheckResult> {
    let url = format!("{}/upload/check", self.base_url);

    let mut request = self.http_client
      .post(&url)
      .json(&serde_json::json!({ "hashes": hashes }));

    if let Some(ref auth) = self.auth_header {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(format!("dedup check failed: {}", error)))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("dedup check returned HTTP {}", response.status()),
      ));
    }

    response.json().await
      .map_err(|error| ClientError::Server(format!("parse dedup result: {}", error)))
  }

  /// Phase 3: Upload a single chunk.
  pub async fn upload_chunk(&self, hash: &str, data: Vec<u8>) -> Result<()> {
    let url = format!("{}/upload/chunks/{}", self.base_url, hash);

    let mut request = self.http_client.put(&url).body(data);

    if let Some(ref auth) = self.auth_header {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(format!("chunk upload failed: {}", error)))?;

    if !response.status().is_success() {
      return Err(ClientError::Server(
        format!("chunk upload returned HTTP {} for {}", response.status(), hash),
      ));
    }

    Ok(())
  }

  /// Phase 4: Commit files from uploaded chunks.
  pub async fn commit(&self, files: Vec<CommitFile>) -> Result<()> {
    let url = format!("{}/upload/commit", self.base_url);

    let mut request = self.http_client
      .post(&url)
      .json(&serde_json::json!({ "files": files }));

    if let Some(ref auth) = self.auth_header {
      request = request.header("Authorization", auth);
    }

    let response = request.send().await
      .map_err(|error| ClientError::Server(format!("commit failed: {}", error)))?;

    let status = response.status();
    if !status.is_success() {
      let body = response.text().await.unwrap_or_default();
      return Err(ClientError::Server(
        format!("commit returned HTTP {}: {}", status, body),
      ));
    }

    Ok(())
  }

  /// High-level: upload a single file using the full 4-phase protocol.
  /// Returns the chunk hashes used (for state tracking).
  pub async fn upload_file_chunked(
    &mut self,
    path: &str,
    data: &[u8],
    content_type: Option<&str>,
  ) -> Result<Vec<String>> {
    // Phase 1: get config
    let config = self.get_config().await?;

    // Chunk the file
    let chunks = self.chunk_file(data, &config);
    let chunk_hashes: Vec<String> = chunks.iter().map(|c| c.hash.clone()).collect();

    if chunks.is_empty() {
      return Ok(chunk_hashes);
    }

    // Phase 2: dedup check
    let dedup = self.check_dedup(&chunk_hashes).await?;

    let needed_set: std::collections::HashSet<&str> = dedup.needed.iter()
      .map(|s| s.as_str())
      .collect();

    // Phase 3: upload only needed chunks
    let mut uploaded_count = 0;
    for chunk in &chunks {
      if needed_set.contains(chunk.hash.as_str()) {
        self.upload_chunk(&chunk.hash, chunk.data.clone()).await?;
        uploaded_count += 1;
      }
    }

    if uploaded_count > 0 || dedup.have.len() > 0 {
      tracing::debug!(
        "upload {}: {} chunks ({} uploaded, {} deduped)",
        path, chunks.len(), uploaded_count, dedup.have.len(),
      );
    }

    // Phase 4: commit
    self.commit(vec![CommitFile {
      path:         path.to_string(),
      content_type: content_type.map(|s| s.to_string()),
      chunks:       chunk_hashes.clone(),
    }]).await?;

    Ok(chunk_hashes)
  }
}
