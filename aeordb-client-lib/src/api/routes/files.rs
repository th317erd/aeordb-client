use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};

use crate::connections::{ConnectionManager, RemoteConnection};
use crate::error::ClientError;
use crate::remote::RemoteClient;
use crate::server::AppState;
use crate::sync::metadata::{SyncMetadataStore, SyncStatus};
use crate::sync::relationships::{RelationshipManager, SyncRelationship};

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct BrowseResponse {
  pub relationship_id:   String,
  pub relationship_name: String,
  pub remote_path:       String,
  pub local_path:        String,
  pub entries:           Vec<BrowseEntry>,
  pub total:             Option<u64>,
  pub limit:             Option<u64>,
  pub offset:            Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
  pub limit:  Option<u64>,
  pub offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BrowseEntry {
  pub name:         String,
  pub entry_type:   u8,
  pub size:         u64,
  pub content_type: Option<String>,
  pub created_at:   i64,
  pub updated_at:   i64,
  pub sync_status:  String,
  pub has_local:    bool,
}

#[derive(Debug, Deserialize)]
pub struct ServeQuery {
  pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenLocallyRequest {
  pub path: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load a relationship and its associated connection from the config store.
async fn load_relationship_and_connection(
  state: &AppState,
  relationship_id: &str,
) -> Result<(SyncRelationship, RemoteConnection), ClientError> {
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager
    .get(relationship_id).await?
    .ok_or_else(|| ClientError::NotFound(format!("relationship not found: {}", relationship_id)))?;

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager
    .get(&relationship.remote_connection_id).await?
    .ok_or_else(|| ClientError::NotFound(
      format!("connection not found: {}", relationship.remote_connection_id),
    ))?;

  Ok((relationship, connection))
}

/// Compute a safe local path from a relationship base and a relative path.
/// Returns 403-equivalent error if the result escapes the relationship's local directory.
fn safe_local_path(
  relationship: &SyncRelationship,
  relative_path: &str,
) -> Result<PathBuf, ClientError> {
  let local_base = Path::new(&relationship.local_path);

  // Per-segment validation: reject any segment that is ".." or empty
  let cleaned: Vec<&str> = relative_path
    .split('/')
    .filter(|segment| !segment.is_empty())
    .collect::<Vec<_>>();

  for segment in &cleaned {
    if *segment == ".." {
      return Err(ClientError::Forbidden("path traversal denied".to_string()));
    }
  }

  let cleaned_relative: PathBuf = cleaned.iter().collect();
  let requested = local_base.join(&cleaned_relative);

  // If the local base dir exists, canonicalize for a definitive check.
  if let Ok(canonical_base) = local_base.canonicalize() {
    if let Ok(canonical) = requested.canonicalize() {
      if !canonical.starts_with(&canonical_base) {
        return Err(ClientError::Forbidden("path traversal denied".to_string()));
      }
      return Ok(canonical);
    }
  }

  // Fallback: segments already validated above, so join is safe.
  Ok(requested)
}

/// Guess a Content-Type from a file extension.
fn guess_content_type(path: &str) -> &'static str {
  let extension = path.rsplit('.').next().unwrap_or("");
  match extension.to_ascii_lowercase().as_str() {
    "html" | "htm" => "text/html",
    "css"          => "text/css",
    "js"           => "application/javascript",
    "json"         => "application/json",
    "xml"          => "application/xml",
    "txt"          => "text/plain",
    "md"           => "text/markdown",
    "csv"          => "text/csv",
    "pdf"          => "application/pdf",
    "png"          => "image/png",
    "jpg" | "jpeg" => "image/jpeg",
    "gif"          => "image/gif",
    "svg"          => "image/svg+xml",
    "webp"         => "image/webp",
    "zip"          => "application/zip",
    "gz" | "gzip"  => "application/gzip",
    "tar"          => "application/x-tar",
    "yaml" | "yml" => "application/yaml",
    "toml"         => "application/toml",
    "wasm"         => "application/wasm",
    _              => "application/octet-stream",
  }
}

/// Compute the full remote path for a relative path within a relationship.
fn compute_remote_path(relationship: &SyncRelationship, relative_path: &str) -> String {
  let base     = relationship.remote_path.trim_end_matches('/');
  let relative = relative_path.trim_start_matches('/');
  if relative.is_empty() {
    format!("{}/", base)
  } else {
    format!("{}/{}", base, relative)
  }
}

/// Compute the local subdirectory path string for a relative path within a relationship.
fn compute_local_subpath(relationship: &SyncRelationship, relative_path: &str) -> String {
  let base     = relationship.local_path.trim_end_matches('/');
  let relative = relative_path.trim_start_matches('/');
  if relative.is_empty() {
    format!("{}/", base)
  } else {
    format!("{}/{}", base, relative)
  }
}

// ---------------------------------------------------------------------------
// 1. Browse
// ---------------------------------------------------------------------------

/// GET /api/v1/browse/{relationship_id} (root)
/// GET /api/v1/browse/{relationship_id}/{*path} (subdirectory)
pub async fn browse(
  State(state): State<AppState>,
  AxumPath(params): AxumPath<BrowseParams>,
  Query(query): Query<BrowseQuery>,
) -> Result<Json<BrowseResponse>, ClientError> {
  let relationship_id = &params.relationship_id;
  let relative_path = params.path.as_deref().unwrap_or("");

  let (relationship, connection) = load_relationship_and_connection(&state, relationship_id).await?;

  let remote_path = compute_remote_path(&relationship, relative_path);
  let local_subpath = compute_local_subpath(&relationship, relative_path);

  tracing::info!("browsing {} (remote: {})", relationship_id, remote_path);

  let remote_client = RemoteClient::from_connection(&connection, &state.http_client);
  let listing = remote_client
    .list_directory_paginated(&remote_path, query.limit, query.offset)
    .await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  let metadata_store = SyncMetadataStore::new(&state.state_store);

  let mut entries = Vec::with_capacity(listing.items.len());
  for entry in listing.items {
    let entry_remote_path = format!("{}/{}", remote_path.trim_end_matches('/'), entry.name);

    // Determine sync status
    let sync_status = match metadata_store.get_file_meta(relationship_id, &entry_remote_path) {
      Ok(Some(meta)) => match meta.sync_status {
        SyncStatus::Synced      => "synced".to_string(),
        SyncStatus::PendingPush => "pending_push".to_string(),
        SyncStatus::PendingPull => "pending_pull".to_string(),
        SyncStatus::Error       => "error".to_string(),
      },
      _ => "not_synced".to_string(),
    };

    // Determine has_local
    let local_file_path = Path::new(&relationship.local_path)
      .join(relative_path)
      .join(&entry.name);
    let has_local = local_file_path.exists();

    entries.push(BrowseEntry {
      name:         entry.name,
      entry_type:   entry.entry_type,
      size:         entry.size,
      content_type: entry.content_type,
      created_at:   entry.created_at,
      updated_at:   entry.updated_at,
      sync_status,
      has_local,
    });
  }

  Ok(Json(BrowseResponse {
    relationship_id:   relationship_id.clone(),
    relationship_name: relationship.name.clone(),
    remote_path,
    local_path:        local_subpath,
    entries,
    total:             listing.total,
    limit:             listing.limit,
    offset:            listing.offset,
  }))
}

/// Path parameters for browse — handles both root and subpath variants.
#[derive(Debug, Deserialize)]
pub struct BrowseParams {
  pub relationship_id: String,
  pub path:            Option<String>,
}

// ---------------------------------------------------------------------------
// 2. Serve file
// ---------------------------------------------------------------------------

/// GET /api/v1/files/{relationship_id}/{*path}
pub async fn serve_file(
  State(state): State<AppState>,
  AxumPath((relationship_id, relative_path)): AxumPath<(String, String)>,
  Query(query): Query<ServeQuery>,
) -> Result<Response, ClientError> {
  let (relationship, connection) = load_relationship_and_connection(&state, &relationship_id).await?;

  let force_remote = query.source.as_deref() == Some("remote");
  let force_local = query.source.as_deref() == Some("local");

  // Compute safe local path
  let local_path = safe_local_path(&relationship, &relative_path)?;
  let local_exists = local_path.exists();

  // Force local — 404 if not on disk
  if force_local && !local_exists {
    return Err(ClientError::NotFound("file not found locally".to_string()));
  }

  // Serve from local if we can (and not forced to remote)
  if !force_remote && local_exists {
    tracing::info!("serving local file: {}", local_path.display());
    let bytes = tokio::fs::read(&local_path)
      .await
      .map_err(|error| ClientError::Server(error.to_string()))?;

    let content_type = guess_content_type(&relative_path);

    let response = Response::builder()
      .status(StatusCode::OK)
      .header(header::CONTENT_TYPE, content_type)
      .body(Body::from(bytes))
      .map_err(|error| ClientError::Server(error.to_string()))?;

    return Ok(response);
  }

  // Proxy from remote
  let remote_path = compute_remote_path(&relationship, &relative_path);
  tracing::info!("serving remote file: {}", remote_path);

  let remote_client = RemoteClient::from_connection(&connection, &state.http_client);
  let (resp, metadata) = remote_client
    .download_file(&remote_path)
    .await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  let content_type = metadata
    .content_type
    .as_deref()
    .unwrap_or_else(|| guess_content_type(&relative_path));

  // Stream the remote response body through to the client.
  let stream = resp.bytes_stream();
  let body = Body::from_stream(stream);

  let response = Response::builder()
    .status(StatusCode::OK)
    .header(header::CONTENT_TYPE, content_type)
    .body(body)
    .map_err(|error| ClientError::Server(error.to_string()))?;

  Ok(response)
}

// ---------------------------------------------------------------------------
// 3. Upload
// ---------------------------------------------------------------------------

/// PUT /api/v1/files/{relationship_id}/{*path}
pub async fn upload_file(
  State(state): State<AppState>,
  AxumPath((relationship_id, relative_path)): AxumPath<(String, String)>,
  headers: HeaderMap,
  body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (relationship, connection) = load_relationship_and_connection(&state, &relationship_id).await?;

  let remote_path = compute_remote_path(&relationship, &relative_path);
  let content_type = headers
    .get(header::CONTENT_TYPE)
    .and_then(|value| value.to_str().ok())
    .map(|s| s.to_string());

  tracing::info!("uploading to remote: {}", remote_path);

  let remote_client = RemoteClient::from_connection(&connection, &state.http_client);
  remote_client
    .upload_file(&remote_path, reqwest::Body::from(body.to_vec()), content_type.as_deref())
    .await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  Ok(Json(serde_json::json!({
    "message": format!("uploaded {}", remote_path),
  })))
}

// ---------------------------------------------------------------------------
// 4. Delete
// ---------------------------------------------------------------------------

/// DELETE /api/v1/files/{relationship_id}/{*path}
pub async fn delete_file(
  State(state): State<AppState>,
  AxumPath((relationship_id, relative_path)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (relationship, connection) = load_relationship_and_connection(&state, &relationship_id).await?;

  let remote_path = compute_remote_path(&relationship, &relative_path);

  tracing::info!("deleting from remote: {}", remote_path);

  let remote_client = RemoteClient::from_connection(&connection, &state.http_client);
  remote_client
    .delete_file(&remote_path)
    .await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  Ok(Json(serde_json::json!({
    "message": format!("deleted {}", remote_path),
  })))
}

// ---------------------------------------------------------------------------
// 5. Open locally
// ---------------------------------------------------------------------------

/// POST /api/v1/files/{relationship_id}/open
pub async fn open_locally(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(request): Json<OpenLocallyRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager
    .get(&relationship_id).await?
    .ok_or_else(|| ClientError::NotFound(format!("relationship not found: {}", relationship_id)))?;

  let local_path = safe_local_path(&relationship, &request.path)?;

  if !local_path.exists() {
    return Err(ClientError::NotFound(format!("file not found locally: {}", request.path)));
  }

  open::that(&local_path)
    .map_err(|error| ClientError::Server(format!("failed to open: {}", error)))?;

  tracing::info!("opened locally: {}", local_path.display());

  Ok(Json(serde_json::json!({
    "message": format!("opened {}", local_path.display()),
  })))
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
  pub from: String,
  pub to:   String,
}

/// POST /api/v1/files/{relationship_id}/rename — rename/move a file on the remote.
pub async fn rename_file(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(request): Json<RenameRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (_, connection) = load_relationship_and_connection(&state, &relationship_id).await?;

  let remote_client = RemoteClient::from_connection(&connection, &state.http_client);

  remote_client.rename_file(&request.from, &request.to).await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  tracing::info!("renamed {} to {}", request.from, request.to);

  Ok(Json(serde_json::json!({
    "renamed": true,
    "from":    request.from,
    "to":      request.to,
  })))
}
