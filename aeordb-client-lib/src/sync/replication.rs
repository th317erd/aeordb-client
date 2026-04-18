use std::sync::Arc;

use serde::{Deserialize, Serialize};

use aeordb::engine::{
  StorageEngine,
  compute_sync_diff, get_needed_chunks, apply_sync_chunks,
  ChunkData,
};

use crate::connections::{AuthType, RemoteConnection};
use crate::error::{ClientError, Result};

/// Response from POST /sync/diff on the remote.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncDiffResponse {
  pub root_hash:           String,
  pub changes:             RemoteSyncChanges,
  pub chunk_hashes_needed: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncChanges {
  pub files_added:      Vec<RemoteSyncFileEntry>,
  pub files_modified:   Vec<RemoteSyncFileEntry>,
  pub files_deleted:    Vec<RemoteSyncDeletedEntry>,
  pub symlinks_added:   Vec<RemoteSyncSymlinkEntry>,
  pub symlinks_modified: Vec<RemoteSyncSymlinkEntry>,
  pub symlinks_deleted: Vec<RemoteSyncDeletedEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncFileEntry {
  pub path:         String,
  pub hash:         String,
  pub size:         u64,
  pub content_type: Option<String>,
  pub chunk_hashes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncSymlinkEntry {
  pub path:   String,
  pub hash:   String,
  pub target: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSyncDeletedEntry {
  pub path: String,
}

/// Response from POST /sync/chunks on the remote.
#[derive(Debug, Deserialize)]
pub struct RemoteSyncChunksResponse {
  pub chunks: Vec<RemoteChunkData>,
}

#[derive(Debug, Deserialize)]
pub struct RemoteChunkData {
  pub hash: String,
  pub data: String, // base64-encoded
}

/// Result of a replication cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationResult {
  pub pulled_chunks:    usize,
  pub pushed_chunks:    usize,
  pub files_changed:    usize,
  pub conflicts:        usize,
  pub local_root_hash:  String,
  pub remote_root_hash: String,
  pub duration_ms:      u64,
  pub errors:           Vec<String>,
}

/// Execute a full bidirectional replication cycle between the local embedded
/// aeordb and a remote aeordb server.
///
/// 1. Compute local diff (what we have that remote might not)
/// 2. Ask remote for its diff (what it has that we might not)
/// 3. Pull remote chunks → apply locally
/// 4. Push local chunks → send to remote
pub async fn replicate(
  engine: &Arc<StorageEngine>,
  connection: &RemoteConnection,
  last_remote_root_hash: Option<&[u8]>,
) -> Result<ReplicationResult> {
  let start = std::time::Instant::now();
  let mut errors = Vec::new();

  // --- Step 1: Compute our local diff ---
  let local_diff = compute_sync_diff(engine, last_remote_root_hash, None, false)
    .map_err(|error| ClientError::Server(
      format!("failed to compute local sync diff: {}", error),
    ))?;

  let local_root_hash = hex::encode(&local_diff.root_hash);

  // --- Step 2: Ask remote for its diff ---
  let remote_diff = fetch_remote_diff(
    connection,
    last_remote_root_hash,
  ).await?;

  let remote_root_hash = remote_diff.root_hash.clone();

  // --- Step 3: Pull — fetch chunks from remote that we need ---
  let pulled_chunks = if !remote_diff.chunk_hashes_needed.is_empty() {
    // The remote diff told us what chunks compose their changed files.
    // We need to collect ALL chunk hashes from the remote's changed files
    // and figure out which ones we don't have locally.
    let mut all_remote_chunk_hashes: Vec<String> = Vec::new();

    for file in &remote_diff.changes.files_added {
      for hash in &file.chunk_hashes {
        all_remote_chunk_hashes.push(hash.clone());
      }
    }
    for file in &remote_diff.changes.files_modified {
      for hash in &file.chunk_hashes {
        all_remote_chunk_hashes.push(hash.clone());
      }
    }

    // Deduplicate
    all_remote_chunk_hashes.sort();
    all_remote_chunk_hashes.dedup();

    if !all_remote_chunk_hashes.is_empty() {
      match fetch_remote_chunks(connection, &all_remote_chunk_hashes).await {
        Ok(chunks) => {
          let count = chunks.len();

          // Convert to library ChunkData format
          let chunk_data: Vec<ChunkData> = chunks.into_iter()
            .filter_map(|chunk| {
              let hash = hex::decode(&chunk.hash).ok()?;
              let data = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &chunk.data,
              ).ok()?;
              Some(ChunkData { hash, data })
            })
            .collect();

          // Apply to local engine
          if let Err(error) = apply_sync_chunks(engine, &chunk_data) {
            errors.push(format!("failed to apply remote chunks: {}", error));
          }

          count
        }
        Err(error) => {
          errors.push(format!("failed to fetch remote chunks: {}", error));
          0
        }
      }
    } else {
      0
    }
  } else {
    0
  };

  // --- Step 4: Push — send our chunks to remote ---
  let pushed_chunks = if !local_diff.chunk_hashes_needed.is_empty() {
    match get_needed_chunks(engine, &local_diff.chunk_hashes_needed) {
      Ok(chunks) => {
        let count = chunks.len();
        if !chunks.is_empty() {
          if let Err(error) = push_chunks_to_remote(connection, &chunks).await {
            errors.push(format!("failed to push chunks to remote: {}", error));
          }
        }
        count
      }
      Err(error) => {
        errors.push(format!("failed to get local chunks: {}", error));
        0
      }
    }
  } else {
    0
  };

  // Count total file changes
  let files_changed = remote_diff.changes.files_added.len()
    + remote_diff.changes.files_modified.len()
    + remote_diff.changes.files_deleted.len()
    + remote_diff.changes.symlinks_added.len()
    + remote_diff.changes.symlinks_modified.len()
    + remote_diff.changes.symlinks_deleted.len();

  // Check for conflicts
  let conflicts = aeordb::engine::list_conflicts_typed(engine)
    .map(|c| c.len())
    .unwrap_or(0);

  let duration_ms = start.elapsed().as_millis() as u64;

  tracing::info!(
    "replication: pulled {} chunks, pushed {} chunks, {} file changes, {} conflicts ({}ms)",
    pulled_chunks, pushed_chunks, files_changed, conflicts, duration_ms,
  );

  Ok(ReplicationResult {
    pulled_chunks,
    pushed_chunks,
    files_changed,
    conflicts,
    local_root_hash,
    remote_root_hash,
    duration_ms,
    errors,
  })
}

/// Call POST /sync/diff on the remote aeordb server.
async fn fetch_remote_diff(
  connection: &RemoteConnection,
  since_root_hash: Option<&[u8]>,
) -> Result<RemoteSyncDiffResponse> {
  let url    = format!("{}/sync/diff", connection.url);
  let client = reqwest::Client::new();

  let body = serde_json::json!({
    "since_root_hash": since_root_hash.map(hex::encode),
  });

  let mut request = client.post(&url).json(&body);

  if connection.auth_type == AuthType::ApiKey {
    if let Some(ref api_key) = connection.api_key {
      request = request.header("Authorization", format!("Bearer {}", api_key));
    }
  }

  let response = request.send().await
    .map_err(|error| ClientError::Server(format!("sync/diff request failed: {}", error)))?;

  if !response.status().is_success() {
    let status = response.status();
    let body   = response.text().await.unwrap_or_default();
    return Err(ClientError::Server(
      format!("sync/diff returned HTTP {}: {}", status, body),
    ));
  }

  response.json().await
    .map_err(|error| ClientError::Server(format!("failed to parse sync/diff response: {}", error)))
}

/// Call POST /sync/chunks on the remote to fetch chunks we need.
async fn fetch_remote_chunks(
  connection: &RemoteConnection,
  chunk_hashes: &[String],
) -> Result<Vec<RemoteChunkData>> {
  let url    = format!("{}/sync/chunks", connection.url);
  let client = reqwest::Client::new();

  let body = serde_json::json!({ "hashes": chunk_hashes });

  let mut request = client.post(&url).json(&body);

  if connection.auth_type == AuthType::ApiKey {
    if let Some(ref api_key) = connection.api_key {
      request = request.header("Authorization", format!("Bearer {}", api_key));
    }
  }

  let response = request.send().await
    .map_err(|error| ClientError::Server(format!("sync/chunks request failed: {}", error)))?;

  if !response.status().is_success() {
    let status = response.status();
    let body   = response.text().await.unwrap_or_default();
    return Err(ClientError::Server(
      format!("sync/chunks returned HTTP {}: {}", status, body),
    ));
  }

  let result: RemoteSyncChunksResponse = response.json().await
    .map_err(|error| ClientError::Server(format!("failed to parse sync/chunks response: {}", error)))?;

  Ok(result.chunks)
}

/// Push local chunks to the remote via POST /sync/chunks.
async fn push_chunks_to_remote(
  connection: &RemoteConnection,
  chunks: &[ChunkData],
) -> Result<()> {
  let url    = format!("{}/sync/chunks", connection.url);
  let client = reqwest::Client::new();

  // Encode chunks as base64 for transport
  let encoded_chunks: Vec<serde_json::Value> = chunks.iter()
    .map(|chunk| {
      serde_json::json!({
        "hash": hex::encode(&chunk.hash),
        "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &chunk.data),
      })
    })
    .collect();

  let body = serde_json::json!({ "chunks": encoded_chunks });

  let mut request = client.post(&url).json(&body);

  if connection.auth_type == AuthType::ApiKey {
    if let Some(ref api_key) = connection.api_key {
      request = request.header("Authorization", format!("Bearer {}", api_key));
    }
  }

  let response = request.send().await
    .map_err(|error| ClientError::Server(format!("push chunks failed: {}", error)))?;

  if !response.status().is_success() {
    let status = response.status();
    let body   = response.text().await.unwrap_or_default();
    return Err(ClientError::Server(
      format!("push chunks returned HTTP {}: {}", status, body),
    ));
  }

  Ok(())
}
