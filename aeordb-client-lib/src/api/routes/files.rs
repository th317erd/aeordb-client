use std::path::{Path, PathBuf};
use std::time::SystemTime;

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Json, Response};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::connections::ConnectionManager;
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
  pub relationship_id: String,
  pub relationship_name: String,
  pub remote_path: String,
  pub local_path: String,
  pub entries: Vec<BrowseEntry>,
  pub total: Option<u64>,
  pub limit: Option<u64>,
  pub offset: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
  pub limit: Option<u64>,
  pub offset: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BrowseEntry {
  pub name: String,
  pub entry_type: u8,
  pub size: u64,
  pub content_type: Option<String>,
  pub created_at: i64,
  pub updated_at: i64,
  pub sync_status: String,
  pub has_local: bool,
  pub effective_permissions: Option<String>,
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

/// Load a relationship and a ready-to-use RemoteClient for it.
///
/// This is the canonical entry point for any handler that needs to talk
/// to the engine on behalf of a relationship. It collapses what used to
/// be three lines (load_relationship_and_connection + new RemoteClient)
/// into one, and ensures every caller picks the same HTTP client + auth
/// path.
///
/// The RemoteClient is built with `from_connection_cached`, sharing its
/// JWT slot with every other concurrent caller for the same connection
/// — so the engine only sees one `POST /auth/token` per connection per
/// JWT lifetime, instead of one per handler invocation.
async fn load_relationship_client(
  state: &AppState,
  relationship_id: &str,
) -> Result<(SyncRelationship, RemoteClient), ClientError> {
  let relationship_manager = RelationshipManager::new(&state.config_store);
  let relationship = relationship_manager
    .get(relationship_id)
    .await?
    .ok_or_else(|| ClientError::NotFound(format!("relationship not found: {}", relationship_id)))?;

  let connection_manager = ConnectionManager::new(&state.config_store);
  let connection = connection_manager
    .get(&relationship.remote_connection_id)
    .await?
    .ok_or_else(|| {
      ClientError::NotFound(format!(
        "connection not found: {}",
        relationship.remote_connection_id
      ))
    })?;

  let jwt_slot = state.jwt_cache.slot_for(&relationship.remote_connection_id);
  let client = RemoteClient::from_connection_cached(&connection, &state.http_client, jwt_slot);
  Ok((relationship, client))
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
    "css" => "text/css",
    "js" => "application/javascript",
    "json" => "application/json",
    "xml" => "application/xml",
    "txt" => "text/plain",
    "md" => "text/markdown",
    "csv" => "text/csv",
    "pdf" => "application/pdf",
    "png" => "image/png",
    "jpg" | "jpeg" => "image/jpeg",
    "gif" => "image/gif",
    "svg" => "image/svg+xml",
    "webp" => "image/webp",
    "mp4" | "m4v" => "video/mp4",
    "webm" => "video/webm",
    "ogv" | "ogg" => "video/ogg",
    "mov" => "video/quicktime",
    "avi" => "video/x-msvideo",
    "mkv" => "video/x-matroska",
    "mp3" => "audio/mpeg",
    "wav" => "audio/wav",
    "flac" => "audio/flac",
    "aac" => "audio/aac",
    "m4a" => "audio/mp4",
    "zip" => "application/zip",
    "gz" | "gzip" => "application/gzip",
    "tar" => "application/x-tar",
    "yaml" | "yml" => "application/yaml",
    "toml" => "application/toml",
    "wasm" => "application/wasm",
    _ => "application/octet-stream",
  }
}

#[derive(Debug, Clone, Copy)]
struct ByteRange {
  start: u64,
  end: u64,
}

impl ByteRange {
  fn len(self) -> u64 {
    self.end.saturating_sub(self.start) + 1
  }
}

fn parse_byte_range(
  range_header: Option<&HeaderValue>,
  file_len: u64,
) -> Result<Option<ByteRange>, ()> {
  let Some(range_header) = range_header else {
    return Ok(None);
  };
  let Ok(range_value) = range_header.to_str() else {
    return Err(());
  };

  let Some(spec) = range_value.trim().strip_prefix("bytes=") else {
    // Unknown range units should be ignored rather than failed.
    return Ok(None);
  };

  if file_len == 0 || spec.contains(',') {
    return Err(());
  }

  let Some((start_raw, end_raw)) = spec.split_once('-') else {
    return Err(());
  };

  let range = if start_raw.is_empty() {
    let suffix_len = end_raw.parse::<u64>().map_err(|_| ())?;
    if suffix_len == 0 {
      return Err(());
    }
    let start = file_len.saturating_sub(suffix_len);
    ByteRange {
      start,
      end: file_len - 1,
    }
  } else {
    let start = start_raw.parse::<u64>().map_err(|_| ())?;
    if start >= file_len {
      return Err(());
    }

    let end = if end_raw.is_empty() {
      file_len - 1
    } else {
      end_raw.parse::<u64>().map_err(|_| ())?.min(file_len - 1)
    };

    if end < start {
      return Err(());
    }

    ByteRange { start, end }
  };

  Ok(Some(range))
}

fn last_modified_millis(modified: Option<SystemTime>) -> Option<String> {
  let modified = modified?;
  modified
    .duration_since(SystemTime::UNIX_EPOCH)
    .ok()
    .map(|duration| duration.as_millis().to_string())
}

async fn serve_local_file(
  local_path: &Path,
  relative_path: &str,
  request_headers: &HeaderMap,
) -> Result<Response, ClientError> {
  let mut file = tokio::fs::File::open(local_path)
    .await
    .map_err(|error| ClientError::Server(error.to_string()))?;
  let metadata = file
    .metadata()
    .await
    .map_err(|error| ClientError::Server(error.to_string()))?;
  let file_len = metadata.len();
  let content_type = guess_content_type(relative_path);
  let modified_ms = last_modified_millis(metadata.modified().ok());

  let range = match parse_byte_range(request_headers.get(header::RANGE), file_len) {
    Ok(range) => range,
    Err(()) => {
      let mut response_builder = Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_RANGE, format!("bytes */{}", file_len))
        .header(header::CONTENT_LENGTH, "0");
      if let Some(modified_ms) = modified_ms {
        response_builder = response_builder.header("x-aeordb-updated", modified_ms);
      }
      return response_builder
        .body(Body::empty())
        .map_err(|error| ClientError::Server(error.to_string()));
    }
  };

  let mut response_builder = Response::builder()
    .header(header::CONTENT_TYPE, content_type)
    .header(header::ACCEPT_RANGES, "bytes")
    .header("x-aeordb-size", file_len.to_string());
  if let Some(modified_ms) = modified_ms {
    response_builder = response_builder.header("x-aeordb-updated", modified_ms);
  }

  if let Some(range) = range {
    file
      .seek(std::io::SeekFrom::Start(range.start))
      .await
      .map_err(|error| ClientError::Server(error.to_string()))?;
    let stream = ReaderStream::new(file.take(range.len()));
    response_builder = response_builder
      .status(StatusCode::PARTIAL_CONTENT)
      .header(
        header::CONTENT_RANGE,
        format!("bytes {}-{}/{}", range.start, range.end, file_len),
      )
      .header(header::CONTENT_LENGTH, range.len().to_string());

    response_builder
      .body(Body::from_stream(stream))
      .map_err(|error| ClientError::Server(error.to_string()))
  } else {
    let stream = ReaderStream::new(file);
    response_builder = response_builder
      .status(StatusCode::OK)
      .header(header::CONTENT_LENGTH, file_len.to_string());

    response_builder
      .body(Body::from_stream(stream))
      .map_err(|error| ClientError::Server(error.to_string()))
  }
}

// ---------------------------------------------------------------------------
// Path translation
//
// The renderer (JS) deals in relationship-relative paths like `/Susan/foo`,
// because that's what the user sees in the breadcrumb. The engine deals in
// absolute paths like `/Pictures/Family/Susan/foo`. Two-way translation
// happens here at the seam: `compute_remote_path` translates renderer →
// engine, `strip_paths_in_json` translates engine → renderer.
//
// Convention for handlers:
//   - INPUT: any request field named `path` is renderer-relative; call
//     `compute_remote_path` (or `compute_remote_paths` for vec) on it
//     before passing to RemoteClient.
//   - OUTPUT: after calling RemoteClient, run the response through
//     `strip_paths_in_json` so engine-absolute paths in the response come
//     back as renderer-relative.
// ---------------------------------------------------------------------------

/// Compute the full remote path for a relative path within a relationship.
fn compute_remote_path(relationship: &SyncRelationship, relative_path: &str) -> String {
  let base = relationship.remote_path.trim_end_matches('/');
  let relative = relative_path.trim_start_matches('/');
  if relative.is_empty() {
    format!("{}/", base)
  } else {
    format!("{}/{}", base, relative)
  }
}

/// Vec convenience — `compute_remote_path` over an array of relative paths.
/// Used by handlers whose request body has a `paths: Vec<String>` field
/// (e.g. copy).
fn compute_remote_paths(relationship: &SyncRelationship, relative_paths: &[String]) -> Vec<String> {
  relative_paths
    .iter()
    .map(|p| compute_remote_path(relationship, p))
    .collect()
}

/// Inverse of `compute_remote_path` — strip a relationship's remote_path
/// prefix off an engine-absolute path so the result is renderer-friendly
/// (relationship-relative). Engine APIs like `/files/deleted` return
/// absolute paths; the JS layer works in relative coordinates, so we
/// translate at the seam. Paths that don't fall under the relationship
/// are passed through unchanged (the engine should never return those,
/// but better to surface the data than swallow it).
fn strip_remote_prefix(relationship: &SyncRelationship, abs_path: &str) -> String {
  let base = relationship.remote_path.trim_end_matches('/');
  if base.is_empty() {
    return abs_path.to_string();
  }
  if let Some(rest) = abs_path.strip_prefix(base) {
    // rest is "" if the path == base, otherwise starts with "/".
    if rest.is_empty() {
      "/".to_string()
    } else {
      rest.to_string()
    }
  } else {
    abs_path.to_string()
  }
}

/// Apply `strip_remote_prefix` to any string-valued `path` key, or to any
/// string element of a `paths` array key, in a JSON value. Walks objects +
/// arrays recursively. Used by the deleted/snapshot/version-history
/// proxies to translate engine-absolute paths back into relationship-
/// relative ones before handing off to the JS file browser.
///
/// Matching keys: `path` (single) and `paths` (array of strings). Any other
/// key, including unrelated arrays, is walked but not rewritten.
fn strip_paths_in_json(relationship: &SyncRelationship, value: &mut serde_json::Value) {
  match value {
    serde_json::Value::Object(map) => {
      for (key, v) in map.iter_mut() {
        match key.as_str() {
          "path" => {
            if let Some(s) = v.as_str() {
              *v = serde_json::Value::String(strip_remote_prefix(relationship, s));
              continue;
            }
          }
          "paths" => {
            if let Some(items) = v.as_array_mut() {
              for item in items.iter_mut() {
                if let Some(s) = item.as_str() {
                  *item = serde_json::Value::String(strip_remote_prefix(relationship, s));
                }
              }
              continue;
            }
          }
          _ => {}
        }
        strip_paths_in_json(relationship, v);
      }
    }
    serde_json::Value::Array(items) => {
      for item in items.iter_mut() {
        strip_paths_in_json(relationship, item);
      }
    }
    _ => {}
  }
}

/// Compute the local subdirectory path string for a relative path within a relationship.
fn compute_local_subpath(relationship: &SyncRelationship, relative_path: &str) -> String {
  let base = relationship.local_path.trim_end_matches('/');
  let relative = relative_path.trim_start_matches('/');
  if relative.is_empty() {
    format!("{}/", base)
  } else {
    format!("{}/{}", base, relative)
  }
}

/// Roll up a directory's sync state from the metadata of all files
/// living somewhere under that directory. The browse handler calls
/// this for every directory entry in a listing.
///
/// Returns one of the same string values used for file entries
/// ("synced" / "pending_push" / "pending_pull" / "error" /
/// "not_synced") so the client's existing per-file dot mapping can
/// render the directory's rollup without any new state handling.
///
/// Priority — the "worst" descendant wins so the user sees the
/// signal that needs the most attention:
///   1. error          → any descendant failed → red dot
///   2. pending_push / pending_pull → any descendant is in flight → yellow
///   3. synced         → at least one descendant has metadata AND all
///                       known descendants are synced → green
///   4. not_synced     → no metadata for any descendant. Fall back to
///                       the directory's own disk presence: a folder
///                       that exists locally with no per-file metadata
///                       reads as "synced enough" rather than uniformly gray;
///                       a folder that doesn't exist locally reads as
///                       genuinely not synced.
///
/// "All descendants" is matched by path prefix on the engine's
/// canonical remote path. Recursive — a broken file three levels deep
/// will bubble up to the top-level folder's dot. That's the right
/// signal for "is anything in this folder broken or in progress?".
fn folder_rollup_status(
  all_metas: &[crate::sync::metadata::FileSyncMeta],
  dir_remote_path: &str,
  dir_has_local: bool,
) -> String {
  // Trailing slash so "/Pictures/Family" doesn't also match a sibling
  // named "/Pictures/FamilyArchive".
  let prefix = format!("{}/", dir_remote_path.trim_end_matches('/'));

  let mut saw_any = false;
  let mut any_error = false;
  let mut any_push = false;
  let mut any_pull = false;
  let mut all_synced = true; // optimistic; flipped when we see a non-synced child

  for meta in all_metas {
    if !meta.path.starts_with(&prefix) {
      continue;
    }
    saw_any = true;
    match meta.sync_status {
      SyncStatus::Synced => {}
      SyncStatus::PendingPush => {
        any_push = true;
        all_synced = false;
      }
      SyncStatus::PendingPull => {
        any_pull = true;
        all_synced = false;
      }
      SyncStatus::Error => {
        any_error = true;
        all_synced = false;
      }
    }
  }

  if any_error {
    return "error".to_string();
  }
  if any_push {
    return "pending_push".to_string();
  }
  if any_pull {
    return "pending_pull".to_string();
  }
  if saw_any && all_synced {
    return "synced".to_string();
  }

  // No descendant metadata at all. Fall back to the directory's disk
  // presence — same semantics as the per-file fallback above.
  if dir_has_local {
    "synced".to_string()
  } else {
    "not_synced".to_string()
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

  let (relationship, remote_client) = load_relationship_client(&state, relationship_id).await?;

  let remote_path = compute_remote_path(&relationship, relative_path);
  let local_subpath = compute_local_subpath(&relationship, relative_path);

  tracing::info!("browsing {} (remote: {})", relationship_id, remote_path);

  // No more `.map_err(|e| BadGateway(e.to_string()))` here — that
  // collapsed connect-refused / 5xx / 4xx / parse errors into one
  // opaque "bad gateway" string and the UI rendered them all as
  // "the server denied access." The RemoteClient now emits
  // categorized errors (UpstreamUnreachable / UpstreamServer /
  // UpstreamProtocol / UpstreamRejected) and the JSON wire format
  // carries a `category` field so the UI can branch correctly.
  let listing = remote_client
    .list_directory_paginated(&remote_path, query.limit, query.offset)
    .await?;

  let metadata_store = SyncMetadataStore::new(&state.state_store);

  // Load all per-file metadata for this relationship ONCE up front so
  // the directory-rollup branch below can answer "what's the state of
  // this folder's descendants?" without re-querying the store per
  // directory in the listing. List size scales with the relationship's
  // file count, not the listing's; for the ~84-file Aeolus folder we
  // tested with this is sub-millisecond, but for a 100k-file
  // relationship this read becomes the dominant cost of a browse
  // call. If that ever bites we'll want a prefix index on the store
  // (currently keyed by blake3(path), which doesn't support prefix
  // scans), or a per-directory rollup cache. Not worth the complexity
  // until a real workload demands it.
  let all_metas = metadata_store
    .list_file_metas(relationship_id)
    .unwrap_or_default();

  let mut entries = Vec::with_capacity(listing.items.len());
  for entry in listing.items {
    let entry_remote_path = format!("{}/{}", remote_path.trim_end_matches('/'), entry.name);
    let is_dir = entry.entry_type == 3;

    // Determine has_local FIRST so we can use it as a fallback when
    // the metadata store has no entry for this file/folder.
    let local_file_path = Path::new(&relationship.local_path)
      .join(relative_path)
      .join(&entry.name);
    let has_local = local_file_path.exists();

    // Determine sync status. The metadata store is the source of truth
    // when present — it carries pending/error states that aren't
    // observable from the disk alone. When metadata is ABSENT, fall
    // back to local presence: if the file exists on disk it was synced
    // at some prior point and there's nothing pending against it; if
    // it doesn't exist it really hasn't been synced.
    //
    // Why this fallback matters: older or repaired state may be missing
    // per-file metadata while the local file still exists. Without the
    // fallback those files render as "not_synced" even when they are already
    // on disk and content-identical to the remote.
    //
    // For DIRECTORIES, the sync_status is a rollup of all descendants'
    // statuses (see folder_rollup_status below) — there's no per-
    // directory metadata in the store, so we have to derive it from
    // the per-file metadata that lives under this directory's path.
    let sync_status = if is_dir {
      folder_rollup_status(&all_metas, &entry_remote_path, has_local)
    } else {
      match metadata_store.get_file_meta(relationship_id, &entry_remote_path) {
        Ok(Some(meta)) => match meta.sync_status {
          SyncStatus::Synced => "synced".to_string(),
          SyncStatus::PendingPush => "pending_push".to_string(),
          SyncStatus::PendingPull => "pending_pull".to_string(),
          SyncStatus::Error => "error".to_string(),
        },
        _ => {
          if has_local {
            "synced".to_string()
          } else {
            "not_synced".to_string()
          }
        }
      }
    };

    entries.push(BrowseEntry {
      name: entry.name,
      entry_type: entry.entry_type,
      size: entry.size,
      content_type: entry.content_type,
      created_at: entry.created_at,
      updated_at: entry.updated_at,
      sync_status,
      has_local,
      effective_permissions: entry.effective_permissions,
    });
  }

  Ok(Json(BrowseResponse {
    relationship_id: relationship_id.clone(),
    relationship_name: relationship.name.clone(),
    remote_path,
    local_path: local_subpath,
    entries,
    total: listing.total,
    limit: listing.limit,
    offset: listing.offset,
  }))
}

/// Path parameters for browse — handles both root and subpath variants.
#[derive(Debug, Deserialize)]
pub struct BrowseParams {
  pub relationship_id: String,
  pub path: Option<String>,
}

// ---------------------------------------------------------------------------
// 2. Serve file
// ---------------------------------------------------------------------------

/// GET /api/v1/files/{relationship_id}/{*path}
pub async fn serve_file(
  State(state): State<AppState>,
  AxumPath((relationship_id, relative_path)): AxumPath<(String, String)>,
  Query(query): Query<ServeQuery>,
  headers: HeaderMap,
) -> Result<Response, ClientError> {
  let (relationship, remote_client) = load_relationship_client(&state, &relationship_id).await?;

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
    return serve_local_file(&local_path, &relative_path, &headers).await;
  }

  // Proxy from remote
  let remote_path = compute_remote_path(&relationship, &relative_path);
  tracing::info!("serving remote file: {}", remote_path);

  let (resp, metadata) = remote_client
    .download_file_with_range(
      &remote_path,
      headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok()),
    )
    .await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;
  let status = resp.status();
  let upstream_headers = resp.headers().clone();

  let content_type = metadata
    .content_type
    .as_deref()
    .unwrap_or_else(|| guess_content_type(&relative_path));

  // Stream the remote response body through to the client.
  let stream = resp.bytes_stream();
  let body = Body::from_stream(stream);

  let response = Response::builder()
    .status(status)
    .header(header::CONTENT_TYPE, content_type)
    .header(header::ACCEPT_RANGES, "bytes")
    .header("x-aeordb-size", metadata.size.to_string())
    .body(body)
    .map_err(|error| ClientError::Server(error.to_string()))?;

  let mut response = response;
  for header_name in [header::CONTENT_LENGTH, header::CONTENT_RANGE] {
    if let Some(header_value) = upstream_headers.get(&header_name) {
      response
        .headers_mut()
        .insert(header_name, header_value.clone());
    }
  }
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
  let (relationship, remote_client) = load_relationship_client(&state, &relationship_id).await?;

  let remote_path = compute_remote_path(&relationship, &relative_path);
  let content_type = headers
    .get(header::CONTENT_TYPE)
    .and_then(|value| value.to_str().ok())
    .map(|s| s.to_string());

  tracing::info!("uploading to remote: {}", remote_path);

  remote_client
    .upload_file(
      &remote_path,
      reqwest::Body::from(body.to_vec()),
      content_type.as_deref(),
    )
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
  let (relationship, remote_client) = load_relationship_client(&state, &relationship_id).await?;

  let remote_path = compute_remote_path(&relationship, &relative_path);

  tracing::info!("deleting from remote: {}", remote_path);

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
    .get(&relationship_id)
    .await?
    .ok_or_else(|| ClientError::NotFound(format!("relationship not found: {}", relationship_id)))?;

  let local_path = safe_local_path(&relationship, &request.path)?;

  if !local_path.exists() {
    return Err(ClientError::NotFound(format!(
      "file not found locally: {}",
      request.path
    )));
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
  pub to: String,
}

/// POST /api/v1/files/{relationship_id}/rename — rename/move a file on the remote.
pub async fn rename_file(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(request): Json<RenameRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (_, remote_client) = load_relationship_client(&state, &relationship_id).await?;

  remote_client
    .rename_file(&request.from, &request.to)
    .await
    .map_err(|error| ClientError::BadGateway(error.to_string()))?;

  tracing::info!("renamed {} to {}", request.from, request.to);

  Ok(Json(serde_json::json!({
    "renamed": true,
    "from":    request.from,
    "to":      request.to,
  })))
}

// ---------------------------------------------------------------------------
// Engine UI proxy handlers
//
// One-liner handlers that wrap relationship_id → connection + absolute path,
// then forward to the engine via RemoteClient. These exist to give the
// desktop file browser the same surface area as the engine's web portal
// (deleted files, snapshots, version history, copy, symlinks) without the
// renderer needing to know about engine URLs or auth.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct DeletedQuery {
  pub path: Option<String>,
}

/// GET /api/v1/browse/{relationship_id}/deleted?path=…
pub async fn list_deleted(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Query(query): Query<DeletedQuery>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (rel, client) = load_relationship_client(&state, &relationship_id).await?;
  let dir_relative = query.path.unwrap_or_else(|| "/".to_string());
  let remote_dir = compute_remote_path(&rel, &dir_relative);
  let mut value = client.fetch_deleted(&remote_dir).await?;
  strip_paths_in_json(&rel, &mut value);
  Ok(Json(value))
}

#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
  pub path: String,
}

/// POST /api/v1/files/{relationship_id}/restore — body: {path}
/// `path` is relationship-relative.
pub async fn restore_deleted(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(req): Json<RestoreRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (rel, client) = load_relationship_client(&state, &relationship_id).await?;
  let remote_path = compute_remote_path(&rel, &req.path);
  client.restore_file(&remote_path).await.map(Json)
}

/// GET /api/v1/versions/{relationship_id}/history/{*path}
pub async fn version_history(
  State(state): State<AppState>,
  AxumPath((relationship_id, file_path)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (rel, client) = load_relationship_client(&state, &relationship_id).await?;
  let remote_path = compute_remote_path(&rel, &file_path);
  let mut value = client.fetch_version_history(&remote_path).await?;
  strip_paths_in_json(&rel, &mut value);
  Ok(Json(value))
}

/// GET /api/v1/snapshots/{relationship_id} — list all snapshots on the
/// engine this relationship connects to. Snapshots are system-wide on
/// the engine; rel_id just picks which connection to authenticate with.
pub async fn list_snapshots(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (_rel, client) = load_relationship_client(&state, &relationship_id).await?;
  client.fetch_snapshots().await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct CreateSnapshotRequest {
  pub name: String,
}

/// POST /api/v1/snapshots/{relationship_id} — body: {name}
pub async fn create_snapshot(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(req): Json<CreateSnapshotRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (_rel, client) = load_relationship_client(&state, &relationship_id).await?;
  client.create_snapshot(&req.name).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct RestoreFromSnapshotRequest {
  pub path: String,
}

/// POST /api/v1/snapshots/{relationship_id}/{snapshot_id}/restore — body: {path}
pub async fn restore_from_snapshot(
  State(state): State<AppState>,
  AxumPath((relationship_id, snapshot_id)): AxumPath<(String, String)>,
  Json(req): Json<RestoreFromSnapshotRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (rel, client) = load_relationship_client(&state, &relationship_id).await?;
  let remote_path = compute_remote_path(&rel, &req.path);
  client
    .restore_from_snapshot(&snapshot_id, &remote_path)
    .await
    .map(Json)
}

#[derive(Debug, Deserialize)]
pub struct CopyRequest {
  pub paths: Vec<String>,
  pub destination: String,
}

/// POST /api/v1/files/{relationship_id}/copy — body: {paths, destination}
/// All paths are relationship-relative.
pub async fn copy_files(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(req): Json<CopyRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (rel, client) = load_relationship_client(&state, &relationship_id).await?;
  let abs_paths = compute_remote_paths(&rel, &req.paths);
  let abs_dest = compute_remote_path(&rel, &req.destination);
  client.copy_files(&abs_paths, &abs_dest).await.map(Json)
}

#[derive(Debug, Deserialize)]
pub struct SymlinkRequest {
  pub path: String,
  pub target: String,
}

/// POST /api/v1/files/{relationship_id}/symlink — body: {path, target}
/// `path` is relationship-relative. `target` is engine-absolute (no
/// translation — symlink targets aren't relationship-bound).
pub async fn create_symlink(
  State(state): State<AppState>,
  AxumPath(relationship_id): AxumPath<String>,
  Json(req): Json<SymlinkRequest>,
) -> Result<Json<serde_json::Value>, ClientError> {
  let (rel, client) = load_relationship_client(&state, &relationship_id).await?;
  let remote_path = compute_remote_path(&rel, &req.path);
  client
    .create_symlink_via_header(&remote_path, &req.target)
    .await?;
  Ok(Json(serde_json::json!({ "ok": true })))
}
