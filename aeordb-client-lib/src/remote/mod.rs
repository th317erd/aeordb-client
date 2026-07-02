use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use std::time::Duration;

use crate::connections::{AuthType, RemoteConnection};
use crate::error::{ClientError, Result};
use crate::jwt_cache::JwtSlot;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const BLOB_COMMIT_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const FILE_UPLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const FILE_DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const BLOB_CHECK_MAX_HASHES_PER_REQUEST: usize = 512;

static DEFAULT_NO_REDIRECT_HTTP_CLIENT: LazyLock<reqwest::Client> =
  LazyLock::new(|| build_no_redirect_http_client(DEFAULT_REQUEST_TIMEOUT));
static SEARCH_NO_REDIRECT_HTTP_CLIENT: LazyLock<reqwest::Client> =
  LazyLock::new(|| build_no_redirect_http_client(SEARCH_REQUEST_TIMEOUT));
static BLOB_COMMIT_NO_REDIRECT_HTTP_CLIENT: LazyLock<reqwest::Client> =
  LazyLock::new(|| build_no_redirect_http_client(BLOB_COMMIT_REQUEST_TIMEOUT));
static FILE_DOWNLOAD_NO_REDIRECT_HTTP_CLIENT: LazyLock<reqwest::Client> =
  LazyLock::new(|| build_no_redirect_http_client(FILE_DOWNLOAD_REQUEST_TIMEOUT));

fn build_no_redirect_http_client(timeout: Duration) -> reqwest::Client {
  reqwest::Client::builder()
    .timeout(timeout)
    .redirect(reqwest::redirect::Policy::none())
    .build()
    .expect("manual-redirect HTTP client should build")
}

fn no_redirect_http_client(timeout: Duration) -> &'static reqwest::Client {
  if timeout == SEARCH_REQUEST_TIMEOUT {
    &SEARCH_NO_REDIRECT_HTTP_CLIENT
  } else if timeout == BLOB_COMMIT_REQUEST_TIMEOUT {
    &BLOB_COMMIT_NO_REDIRECT_HTTP_CLIENT
  } else if timeout == FILE_DOWNLOAD_REQUEST_TIMEOUT {
    &FILE_DOWNLOAD_NO_REDIRECT_HTTP_CLIENT
  } else {
    &DEFAULT_NO_REDIRECT_HTTP_CLIENT
  }
}

/// Minimal percent-encoder for URL query values. Mirrors the engine-side
/// helper in share_link_routes.rs so the portal URL we mint here is
/// indistinguishable from one minted by the engine's share-link flow.
fn simple_url_encode(input: &str) -> String {
  let mut out = String::with_capacity(input.len() * 2);
  for byte in input.bytes() {
    match byte {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
        out.push(byte as char)
      }
      _ => out.push_str(&format!("%{:02X}", byte)),
    }
  }
  out
}

/// Entry type constants from aeordb.
pub const ENTRY_TYPE_FILE: u8 = 2;
pub const ENTRY_TYPE_DIRECTORY: u8 = 3;
pub const ENTRY_TYPE_SYMLINK: u8 = 8;

/// A remote aeordb directory entry, as returned by GET /files/{directory_path}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
  pub name: String,
  pub entry_type: u8,
  #[serde(default)]
  pub size: u64,
  #[serde(default)]
  pub created_at: i64,
  #[serde(default)]
  pub updated_at: i64,
  #[serde(default)]
  pub content_type: Option<String>,
  #[serde(default)]
  pub path: Option<String>,
  #[serde(default)]
  pub hash: Option<String>,
  #[serde(default)]
  pub target: Option<String>,
  #[serde(default)]
  pub effective_permissions: Option<String>,
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
  pub path: String,
  pub size: u64,
  pub content_type: Option<String>,
  pub created_at: Option<i64>,
  pub updated_at: Option<i64>,
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
    ClientError::UpstreamUnreachable(format!(
      "couldn't reach engine for {}: {}",
      remote_path, error
    ))
  } else if error.is_request() {
    ClientError::UpstreamProtocol(format!(
      "malformed request to engine for {}: {}",
      remote_path, error
    ))
  } else {
    // Catch-all: body decode errors, etc. Treat as unreachable since
    // the user's likely action is the same (retry / check network).
    ClientError::UpstreamUnreachable(format!(
      "transport error talking to engine for {}: {}",
      remote_path, error
    ))
  }
}

/// Classify a non-success HTTP status from the upstream engine into
/// a categorized ClientError. 4xx → UpstreamRejected (status code
/// preserved; passive callers should NOT distinguish 403 from 404
/// in user-visible UI per the DB team's 2026-05-23 retraction).
/// 5xx → UpstreamServer. Other → UpstreamServer as a safe fallback.
fn classify_upstream_status(status: u16, body: &str, remote_path: &str) -> ClientError {
  let body_excerpt = upstream_body_message(body);
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

fn upstream_body_message(body: &str) -> String {
  serde_json::from_str::<serde_json::Value>(body)
    .ok()
    .and_then(|value| {
      value
        .get("error")
        .and_then(|error| error.as_str())
        .map(ToOwned::to_owned)
    })
    .unwrap_or_else(|| body.chars().take(200).collect::<String>())
}

fn response_excerpt(body: &str) -> String {
  body.chars().take(500).collect::<String>()
}

async fn json_value_response(
  response: reqwest::Response,
  context: &str,
) -> Result<serde_json::Value> {
  let status = response.status();
  let text = response.text().await.unwrap_or_default();

  if !status.is_success() {
    return Err(classify_upstream_status(status.as_u16(), &text, context));
  }

  if text.trim().is_empty() {
    return Ok(serde_json::json!({"ok": true}));
  }

  serde_json::from_str(&text).map_err(|error| {
    ClientError::UpstreamProtocol(format!(
      "failed to parse {} response: {}; body: {}",
      context,
      error,
      response_excerpt(&text)
    ))
  })
}

/// Client for talking to a remote aeordb instance.
/// Handles JWT token exchange: exchanges the API key for a JWT on first
/// authenticated request, caches it, and re-exchanges on 401.
#[derive(Clone)]
pub struct RemoteClient {
  http_client: reqwest::Client,
  base_url: String,
  api_key: Option<String>,
  /// Shared JWT slot. Multiple RemoteClient instances for the same
  /// connection share the same slot (via `crate::jwt_cache::JwtCache`)
  /// so a token minted by one request handler is visible to the next.
  /// Without this, every API handler creating a fresh RemoteClient hit
  /// `POST /auth/token` on the engine — see jwt_cache.rs for the why.
  jwt_slot: JwtSlot,
}

impl RemoteClient {
  /// Standalone constructor — gives the client its own private JWT
  /// slot. Use this only when there's no shared cache available (e.g.
  /// one-shot CLI commands, tests). Production code paths should call
  /// `from_connection_cached` so the JWT survives across requests.
  pub fn from_connection(connection: &RemoteConnection, http_client: &reqwest::Client) -> Self {
    Self::from_connection_cached(
      connection,
      http_client,
      std::sync::Arc::new(std::sync::Mutex::new(None)),
    )
  }

  /// Shared-cache constructor. `jwt_slot` should come from
  /// `AppState.jwt_cache.slot_for(&connection.id)` so all concurrent
  /// requests for this connection share the same token.
  pub fn from_connection_cached(
    connection: &RemoteConnection,
    http_client: &reqwest::Client,
    jwt_slot: JwtSlot,
  ) -> Self {
    let api_key = if connection.auth_type == AuthType::ApiKey {
      connection.api_key.clone()
    } else {
      None
    };

    Self {
      http_client: http_client.clone(),
      base_url: connection.base_url(),
      api_key,
      jwt_slot,
    }
  }

  /// Get the auth header, exchanging API key for JWT if needed.
  pub async fn auth_header(&self) -> Option<String> {
    let api_key = self.api_key.as_ref()?;

    // Check for cached JWT (in the shared slot).
    {
      let cached = self.jwt_slot.lock().unwrap();
      if let Some(ref token) = *cached {
        return Some(format!("Bearer {}", token));
      }
    }

    // Exchange API key for JWT and stash in the shared slot so the
    // next request on this connection — possibly from a different
    // handler — reuses it.
    match self.exchange_token(api_key).await {
      Ok(token) => {
        let header = format!("Bearer {}", token);
        *self.jwt_slot.lock().unwrap() = Some(token);
        Some(header)
      }
      Err(error) => {
        tracing::warn!("JWT token exchange failed: {}", error);
        // Fall back to raw API key
        Some(format!("Bearer {}", api_key))
      }
    }
  }

  /// Clear the cached JWT (e.g. on 401) so the next request re-exchanges.
  /// Pub so handlers can clear the shared slot when they see a 401 from
  /// the engine — the cache is then re-populated on the very next call.
  pub fn invalidate_token(&self) {
    *self.jwt_slot.lock().unwrap() = None;
  }

  /// Send a request with the cached JWT; on 401, invalidate the slot
  /// and retry exactly once with a freshly-minted token. Surfaces the
  /// second response regardless of status (a 401 after re-mint means
  /// the underlying api_key is genuinely bad).
  ///
  /// `build` is called once per attempt to construct a fresh
  /// RequestBuilder — RequestBuilder isn't cloneable, so we can't
  /// retry by re-using the same builder. Don't put streaming bodies
  /// (`reqwest::Body::wrap_stream`) through here: the second call
  /// would re-read the stream, which is one-shot. The upload path
  /// has its own non-retrying flow for exactly that reason.
  ///
  /// Closure must be `Fn` so it can be invoked twice — capture inputs
  /// by reference, not by move.
  pub async fn authed_send<F>(&self, build: F) -> reqwest::Result<reqwest::Response>
  where
    F: Fn() -> reqwest::RequestBuilder,
  {
    self
      .authed_send_with_timeout(build, DEFAULT_REQUEST_TIMEOUT)
      .await
  }

  async fn authed_send_with_timeout<F>(
    &self,
    build: F,
    timeout: Duration,
  ) -> reqwest::Result<reqwest::Response>
  where
    F: Fn() -> reqwest::RequestBuilder,
  {
    let attempt = || async {
      let mut req = build();
      if let Some(auth) = self.auth_header().await {
        req = req.header("Authorization", auth);
      }
      self
        .send_following_auth_redirects_with_timeout(req, timeout)
        .await
    };
    let response = attempt().await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
      tracing::debug!("RemoteClient: 401 from engine, invalidating JWT and retrying once");
      self.invalidate_token();
      return attempt().await;
    }
    Ok(response)
  }

  async fn send_following_auth_redirects(
    &self,
    request: reqwest::RequestBuilder,
  ) -> reqwest::Result<reqwest::Response> {
    self
      .send_following_auth_redirects_with_timeout(request, DEFAULT_REQUEST_TIMEOUT)
      .await
  }

  async fn send_following_auth_redirects_with_timeout(
    &self,
    request: reqwest::RequestBuilder,
    timeout: Duration,
  ) -> reqwest::Result<reqwest::Response> {
    let client = no_redirect_http_client(timeout);
    let mut request = request.build()?;

    for _ in 0..5 {
      let retry_request = request.try_clone();
      let response = client.execute(request).await?;
      let status = response.status();
      let is_redirect = matches!(status.as_u16(), 301 | 302 | 307 | 308);
      if !is_redirect {
        return Ok(response);
      }

      let Some(location) = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
      else {
        return Ok(response);
      };
      let Ok(next_url) = response.url().join(location) else {
        return Ok(response);
      };
      let Some(mut next_request) = retry_request else {
        return Ok(response);
      };
      *next_request.url_mut() = next_url;
      request = next_request;
    }

    client.execute(request).await
  }

  /// Build a pre-authenticated portal URL pointing at a file/directory on
  /// the engine's web UI. Used by the desktop client's "Open Remotely" flow
  /// — the user clicks, the renderer asks for this URL, and it opens in the
  /// system browser already logged in via a short-lived JWT.
  ///
  /// Returns `Err(NotFound)` if the connection has no API key configured
  /// (we can't mint a token without one).
  pub async fn portal_url(&self, path: &str) -> Result<String> {
    let api_key = self.api_key.as_ref().ok_or_else(|| {
      ClientError::NotFound("connection has no API key; cannot mint a portal token".to_string())
    })?;
    let token = self.exchange_token(api_key).await?;
    let base = self.base_url.trim_end_matches('/');
    Ok(format!(
      "{}/?token={}&page=files&path={}",
      base,
      token,
      simple_url_encode(path),
    ))
  }

  /// Exchange an API key for a JWT token via POST /auth/token.
  ///
  /// `include_refresh: false` is the daemon default — we never need a
  /// refresh-token row stored on the engine because we always have the
  /// raw API key in memory and can re-mint a JWT at any time. Without
  /// this flag, the engine creates a persistent
  /// /.aeordb-system/refresh-tokens/... record on every token mint,
  /// which (combined with our 15s dashboard poll) was leaking
  /// thousands of orphaned token rows per day. Only the interactive
  /// browser-login flow on the portal needs `include_refresh: true`.
  async fn exchange_token(&self, api_key: &str) -> Result<String> {
    let initial_url = format!("{}/auth/token", self.base_url);
    let body = serde_json::json!({
      "api_key":         api_key,
      "include_refresh": false,
    });

    let auth_client = reqwest::Client::builder()
      .timeout(Duration::from_secs(30))
      .redirect(reqwest::redirect::Policy::none())
      .build()
      .map_err(|e| {
        ClientError::Server(format!(
          "failed to create no-redirect auth HTTP client: {}",
          e,
        ))
      })?;

    // Manually walk 301/302/307/308 redirects while preserving POST.
    // reqwest's default policy downgrades POST→GET on 301/302 (the
    // browser convention), which is the wrong answer for our auth
    // endpoint: nginx in front of many engine deployments returns
    // `301 Moved Permanently` to upgrade http→https, and the GET that
    // reqwest then issues hits no /auth/token GET handler and comes
    // back HTTP 405. Manual handling re-issues the same POST against
    // the redirected URL so the upgrade is transparent.
    let mut redirect_chain: Vec<String> = vec![initial_url.clone()];
    let mut current_url = initial_url.clone();
    let response = loop {
      let response = auth_client
        .post(&current_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
          ClientError::Server(format!(
            "token exchange POST to {} failed: {}",
            current_url, e,
          ))
        })?;

      let status = response.status();
      let is_redirect = matches!(status.as_u16(), 301 | 302 | 307 | 308);
      if !is_redirect {
        break response;
      }

      let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

      let Some(loc) = location else {
        return Err(ClientError::Server(format!(
          "token exchange returned HTTP {} with no Location header (chain: {})",
          status,
          redirect_chain.join(" → "),
        )));
      };

      // Resolve the Location against the current URL (handles both
      // absolute and relative redirects).
      let next_url = match reqwest::Url::parse(&current_url).and_then(|base| base.join(&loc)) {
        Ok(u) => u.to_string(),
        Err(e) => {
          return Err(ClientError::Server(format!(
            "token exchange got HTTP {} with unparseable Location '{}': {}",
            status, loc, e,
          )));
        }
      };

      if redirect_chain.len() >= 5 {
        return Err(ClientError::Server(format!(
          "token exchange exceeded 5 redirects (chain: {} → {})",
          redirect_chain.join(" → "),
          next_url,
        )));
      }
      redirect_chain.push(next_url.clone());
      current_url = next_url;
    };

    if !response.status().is_success() {
      let chain_summary = if redirect_chain.len() > 1 {
        format!(" (after redirect: {})", redirect_chain.join(" → "))
      } else {
        String::new()
      };
      return Err(ClientError::Server(format!(
        "token exchange returned HTTP {} for {}{} \
         — check that the connection URL is correct and that the engine is reachable",
        response.status(),
        self.base_url,
        chain_summary,
      )));
    }

    let body: serde_json::Value = response
      .json()
      .await
      .map_err(|e| ClientError::Server(format!("token exchange response parse failed: {}", e)))?;

    body
      .get("token")
      .and_then(|t| t.as_str())
      .map(|s| s.to_string())
      .ok_or_else(|| {
        ClientError::Server("token exchange response missing 'token' field".to_string())
      })
  }

  /// List the contents of a remote directory.
  pub async fn list_directory(&self, remote_path: &str) -> Result<Vec<RemoteEntry>> {
    let url = format!("{}/files{}", self.base_url, remote_path);

    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|error| {
        ClientError::Server(format!(
          "failed to list remote directory {}: {}",
          remote_path, error
        ))
      })?;

    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "remote returned HTTP {} for {}",
        response.status(),
        remote_path
      )));
    }

    /// Wrapper for the collection response format: `{items: [...]}`.
    #[derive(Deserialize)]
    struct ItemsWrapper {
      items: Vec<RemoteEntry>,
    }

    let wrapper: ItemsWrapper = response.json().await.map_err(|error| {
      ClientError::Server(format!(
        "failed to parse directory listing for {}: {}",
        remote_path, error
      ))
    })?;

    Ok(wrapper.items)
  }

  /// Download a file from the remote as a streaming response.
  ///
  /// Returns the response and parsed metadata from headers. The caller is
  /// responsible for streaming the response body to disk (or wherever) in
  /// chunks, avoiding buffering the entire file in memory.
  pub async fn download_file(
    &self,
    remote_path: &str,
  ) -> Result<(reqwest::Response, RemoteFileMetadata)> {
    self.download_file_with_range(remote_path, None).await
  }

  /// Download a file from the remote as a streaming response, optionally
  /// forwarding a browser byte range request. Older engines ignore Range and
  /// return 200; newer/range-aware engines can return 206 and Content-Range.
  pub async fn download_file_with_range(
    &self,
    remote_path: &str,
    range: Option<&str>,
  ) -> Result<(reqwest::Response, RemoteFileMetadata)> {
    let url = format!("{}/files{}", self.base_url, remote_path);

    let response = self
      .authed_send_with_timeout(
        || {
          let request = self.http_client.get(&url);
          if let Some(range) = range {
            request
              .header(reqwest::header::RANGE, range)
              .timeout(FILE_DOWNLOAD_REQUEST_TIMEOUT)
          } else {
            request.timeout(FILE_DOWNLOAD_REQUEST_TIMEOUT)
          }
        },
        FILE_DOWNLOAD_REQUEST_TIMEOUT,
      )
      .await
      .map_err(|error| {
        ClientError::Server(format!("failed to download {}: {}", remote_path, error))
      })?;

    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "remote returned HTTP {} for {}",
        response.status(),
        remote_path
      )));
    }

    let headers = response.headers().clone();

    let path = headers
      .get("x-aeordb-path")
      .and_then(|value| value.to_str().ok())
      .unwrap_or(remote_path)
      .to_string();

    let size = headers
      .get("x-aeordb-size")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<u64>().ok())
      .unwrap_or(0);

    let content_type = headers
      .get("content-type")
      .and_then(|value| value.to_str().ok())
      .map(|value| value.to_string());

    let created_at = headers
      .get("x-aeordb-created-at")
      .and_then(|value| value.to_str().ok())
      .and_then(|value| value.parse::<i64>().ok());

    let updated_at = headers
      .get("x-aeordb-updated-at")
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

  /// Open the remote engine's file-change SSE stream for a path prefix.
  pub async fn file_event_stream(&self, path_prefix: &str) -> Result<reqwest::Response> {
    let url = format!(
      "{}/system/events?events=entries_created,entries_deleted&path_prefix={}",
      self.base_url,
      simple_url_encode(path_prefix),
    );

    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|error| classify_reqwest_send_error(&error, "/system/events"))?;

    if !response.status().is_success() {
      let status = response.status().as_u16();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(status, &body, "/system/events"));
    }

    Ok(response)
  }

  /// Check if a remote path exists (HEAD request).
  pub async fn exists(&self, remote_path: &str) -> Result<bool> {
    let url = format!("{}/files{}", self.base_url, remote_path);

    let response = self
      .authed_send(|| self.http_client.head(&url))
      .await
      .map_err(|error| {
        ClientError::Server(format!(
          "failed to check existence of {}: {}",
          remote_path, error
        ))
      })?;

    Ok(response.status().is_success())
  }

  /// Verify that a specific remote path currently has the expected full-file
  /// content hash. Uses the database virtual metadata indexes instead of
  /// downloading the file.
  pub async fn remote_path_has_content_hash(
    &self,
    query_root: &str,
    remote_path: &str,
    content_hash: &str,
  ) -> Result<bool> {
    let url = format!("{}/files/query", self.base_url);
    let body = serde_json::json!({
      "path": query_root,
      "where": {
        "and": [
          {
            "field": "@path",
            "op": "eq",
            "value": remote_path,
          },
          {
            "field": "@hash",
            "op": "eq",
            "value": content_hash,
          }
        ]
      },
      "limit": 1,
      "select": ["@path"],
    });

    let response = self
      .authed_send(|| self.http_client.post(&url).json(&body))
      .await
      .map_err(|error| classify_reqwest_send_error(&error, remote_path))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        remote_path,
      ));
    }

    let body = response.text().await.map_err(|error| {
      ClientError::UpstreamUnreachable(format!(
        "failed to read hash query response for {}: {}",
        remote_path, error
      ))
    })?;

    let query: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
      ClientError::UpstreamProtocol(format!(
        "failed to parse hash query response for {}: {}; body: {}",
        remote_path,
        error,
        response_excerpt(&body),
      ))
    })?;

    let rows = query
      .get("results")
      .or_else(|| query.get("items"))
      .and_then(|value| value.as_array())
      .ok_or_else(|| {
        ClientError::UpstreamProtocol(format!(
          "hash query response for {} missing results/items array; body: {}",
          remote_path,
          response_excerpt(&body),
        ))
      })?;

    Ok(rows.iter().any(|result| {
      result
        .get("path")
        .or_else(|| result.get("@path"))
        .and_then(|value| value.as_str())
        == Some(remote_path)
    }))
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

    let mut request = self
      .http_client
      .put(&url)
      .timeout(FILE_UPLOAD_REQUEST_TIMEOUT)
      .body(body);

    if let Some(content_type) = content_type {
      request = request.header("Content-Type", content_type);
    }

    if let Some(ref auth) = self.auth_header().await {
      request = request.header("Authorization", auth);
    }

    let response = request
      .send()
      .await
      .map_err(|error| classify_reqwest_send_error(&error, remote_path))?;

    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        remote_path,
      ));
    }

    Ok(())
  }

  /// Delete a file on the remote aeordb instance.
  pub async fn delete_file(&self, remote_path: &str) -> Result<()> {
    let url = format!("{}/files{}", self.base_url, remote_path);

    let response = self
      .authed_send(|| self.http_client.delete(&url))
      .await
      .map_err(|error| {
        ClientError::Server(format!(
          "failed to delete remote {}: {}",
          remote_path, error
        ))
      })?;

    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "remote returned HTTP {} for DELETE {}",
        response.status(),
        remote_path
      )));
    }

    Ok(())
  }

  /// Create a symlink on the remote aeordb instance.
  /// Uses PUT /links/{path} with {"target": "..."} body.
  pub async fn create_symlink(&self, remote_path: &str, target: &str) -> Result<()> {
    let url = format!("{}/links{}", self.base_url, remote_path);
    let body = serde_json::json!({ "target": target });

    let response = self
      .authed_send(|| self.http_client.put(&url).json(&body))
      .await
      .map_err(|error| {
        ClientError::Server(format!(
          "failed to create symlink {}: {}",
          remote_path, error
        ))
      })?;

    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "remote returned HTTP {} for symlink {}",
        response.status(),
        remote_path
      )));
    }

    Ok(())
  }

  /// Return the remote symlink target for `remote_path`, without following the
  /// symlink. Returns `Ok(None)` when the path is absent or is not a symlink.
  pub async fn remote_symlink_target(&self, remote_path: &str) -> Result<Option<String>> {
    let url = format!("{}/files{}?nofollow=true", self.base_url, remote_path);

    let response = self
      .authed_send(|| self.http_client.head(&url))
      .await
      .map_err(|error| {
        ClientError::Server(format!(
          "failed to inspect remote symlink {}: {}",
          remote_path, error
        ))
      })?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
      return Ok(None);
    }

    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "remote returned HTTP {} while inspecting symlink {}",
        response.status(),
        remote_path
      )));
    }

    let headers = response.headers();
    let target = headers
      .get("x-aeordb-symlink-target")
      .or_else(|| headers.get("x-aeordb-link-target"))
      .and_then(|value| value.to_str().ok())
      .map(str::to_string);
    if target.is_some() {
      return Ok(target);
    }

    let entry_type = headers
      .get("x-aeordb-entry-type")
      .or_else(|| headers.get("x-aeordb-type"))
      .and_then(|value| value.to_str().ok())
      .unwrap_or_default();
    if entry_type.eq_ignore_ascii_case("symlink") || entry_type == "8" {
      return Ok(None);
    }

    Ok(None)
  }

  /// Rename/move a file or directory on the remote.
  /// Uses PATCH /files/{from_path} with {"to": "..."} body.
  pub async fn rename_file(&self, from_path: &str, to_path: &str) -> Result<()> {
    let clean_from = from_path.trim_start_matches('/');
    let url = format!("{}/files/{}", self.base_url, clean_from);
    let body = serde_json::json!({ "to": to_path });

    let response = self
      .authed_send(|| self.http_client.patch(&url).json(&body))
      .await
      .map_err(|error| {
        ClientError::Server(format!(
          "failed to rename {} to {}: {}",
          from_path, to_path, error
        ))
      })?;

    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "remote returned HTTP {} for rename {} to {}",
        response.status(),
        from_path,
        to_path
      )));
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

    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|error| classify_reqwest_send_error(&error, remote_path))?;

    let status = response.status();
    if !status.is_success() {
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        remote_path,
      ));
    }

    let listing: DirectoryListingResponse = response.json().await.map_err(|error| {
      ClientError::UpstreamProtocol(format!(
        "failed to parse directory listing for {}: {}",
        remote_path, error
      ))
    })?;

    Ok(listing)
  }

  /// Search files under a remote subtree using the engine's global search
  /// endpoint, scoped with the `path` field so the desktop client only sees
  /// results inside the selected sync relationship.
  pub async fn search_files(
    &self,
    remote_root: &str,
    query: &str,
    limit: Option<u64>,
    offset: Option<u64>,
  ) -> Result<serde_json::Value> {
    let url = format!("{}/files/search", self.base_url);
    let body = serde_json::json!({
      "path": remote_root,
      "query": query,
      "limit": limit.unwrap_or(100).min(1000),
      "offset": offset.unwrap_or(0),
    });

    let response = self
      .authed_send_with_timeout(
        || {
          self
            .http_client
            .post(&url)
            .json(&body)
            .timeout(SEARCH_REQUEST_TIMEOUT)
        },
        SEARCH_REQUEST_TIMEOUT,
      )
      .await
      .map_err(|error| classify_reqwest_send_error(&error, "/files/search"))?;

    json_value_response(response, "/files/search").await
  }

  /// Get shares for a path. GET /files/shares?path=...
  pub async fn get_shares(&self, path: &str) -> Result<serde_json::Value> {
    let url = format!(
      "{}/files/shares?path={}",
      self.base_url,
      simple_url_encode(path)
    );
    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/files/shares"))?;
    json_value_response(response, "/files/shares").await
  }

  /// Grant share access. POST /files/share
  pub async fn share(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}/files/share", self.base_url);
    let response = self
      .authed_send(|| self.http_client.post(&url).json(body))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/files/share"))?;
    json_value_response(response, "/files/share").await
  }

  /// Revoke share access. DELETE /files/shares (with JSON body)
  pub async fn unshare(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}/files/shares", self.base_url);
    let response = self
      .authed_send(|| self.http_client.delete(&url).json(body))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/files/shares"))?;
    json_value_response(response, "/files/shares").await
  }

  /// Get users that can receive shares. GET /auth/keys/users
  pub async fn get_shareable_users(&self) -> Result<serde_json::Value> {
    let url = format!("{}/auth/keys/users", self.base_url);
    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/auth/keys/users"))?;
    json_value_response(response, "/auth/keys/users").await
  }

  /// Get groups that can receive shares. GET /system/groups
  pub async fn get_shareable_groups(&self) -> Result<serde_json::Value> {
    let url = format!("{}/system/groups", self.base_url);
    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/system/groups"))?;
    json_value_response(response, "/system/groups").await
  }

  /// Create a share link. POST /files/share-link
  /// The caller should set base_url in the body before calling.
  pub async fn create_share_link(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}/files/share-link", self.base_url);
    let response = self
      .authed_send(|| self.http_client.post(&url).json(body))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/files/share-link"))?;
    json_value_response(response, "/files/share-link").await
  }

  /// Get active share links. GET /files/share-links?path=...
  pub async fn get_share_links(&self, path: &str) -> Result<serde_json::Value> {
    let url = format!(
      "{}/files/share-links?path={}",
      self.base_url,
      simple_url_encode(path)
    );
    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/files/share-links"))?;
    json_value_response(response, "/files/share-links").await
  }

  /// Revoke a share link. DELETE /files/share-links/{key_id}
  pub async fn revoke_share_link(&self, key_id: &str) -> Result<serde_json::Value> {
    let url = format!("{}/files/share-links/{}", self.base_url, key_id);
    let response = self
      .authed_send(|| self.http_client.delete(&url))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/files/share-links/{key_id}"))?;
    json_value_response(response, "/files/share-links/{key_id}").await
  }

  // ---------------------------------------------------------------------------
  // Engine UI proxy methods
  //
  // The desktop client's file browser needs feature parity with the engine's
  // web portal: deleted-file listings, snapshots, version history, paste-as-
  // copy/move, symlinks. The portal's JS hits the engine endpoints directly;
  // the client routes through client-lib so the renderer doesn't have to
  // know about base URLs or auth tokens. These methods do the engine call;
  // the matching api/routes/files.rs handlers map relationship_id →
  // connection + absolute path before calling them.
  // ---------------------------------------------------------------------------

  /// GET /files/deleted?path={dir_path} — list deleted entries in a dir.
  pub async fn fetch_deleted(&self, dir_path: &str) -> Result<serde_json::Value> {
    let url = format!(
      "{}/files/deleted?path={}",
      self.base_url,
      simple_url_encode(dir_path)
    );
    self.engine_get_json(&url, "fetch_deleted").await
  }

  /// POST /files/restore {path} — undelete a file.
  pub async fn restore_file(&self, file_path: &str) -> Result<serde_json::Value> {
    let url = format!("{}/files/restore", self.base_url);
    self
      .engine_post_json(
        &url,
        &serde_json::json!({ "path": file_path }),
        "restore_file",
      )
      .await
  }

  /// GET /versions/history/{file_path} — list versions of a file.
  pub async fn fetch_version_history(&self, file_path: &str) -> Result<serde_json::Value> {
    let clean = file_path.trim_start_matches('/');
    let url = format!("{}/versions/history/{}", self.base_url, clean);
    self.engine_get_json(&url, "fetch_version_history").await
  }

  /// GET /versions/snapshots — list all snapshots on this engine. Not
  /// relationship-scoped (snapshots are system-wide); the rel_id in the
  /// client route is just for picking the connection.
  pub async fn fetch_snapshots(&self) -> Result<serde_json::Value> {
    let url = format!("{}/versions/snapshots", self.base_url);
    self.engine_get_json(&url, "fetch_snapshots").await
  }

  /// POST /versions/snapshots {name} — create a named snapshot.
  pub async fn create_snapshot(&self, name: &str) -> Result<serde_json::Value> {
    let url = format!("{}/versions/snapshots", self.base_url);
    self
      .engine_post_json(
        &url,
        &serde_json::json!({ "name": name }),
        "create_snapshot",
      )
      .await
  }

  /// POST /versions/snapshots/{snap_id}/restore {path} — restore one file
  /// from a snapshot.
  pub async fn restore_from_snapshot(
    &self,
    snapshot_id: &str,
    file_path: &str,
  ) -> Result<serde_json::Value> {
    let url = format!(
      "{}/versions/snapshots/{}/restore",
      self.base_url, snapshot_id
    );
    self
      .engine_post_json(
        &url,
        &serde_json::json!({ "path": file_path }),
        "restore_from_snapshot",
      )
      .await
  }

  /// POST /files/copy {paths, destination} — copy files to a new dir.
  pub async fn copy_files(&self, paths: &[String], destination: &str) -> Result<serde_json::Value> {
    let url = format!("{}/files/copy", self.base_url);
    self
      .engine_post_json(
        &url,
        &serde_json::json!({ "paths": paths, "destination": destination }),
        "copy_files",
      )
      .await
  }

  /// PUT /files{path} with X-Aeor-Symlink: {target} header. Matches the
  /// engine's current header-based symlink convention (the older
  /// PUT /links/{path} body is still served and used by the sync runner —
  /// see `create_symlink` above — but new UI flows use this one).
  pub async fn create_symlink_via_header(&self, file_path: &str, target: &str) -> Result<()> {
    let clean = file_path.trim_start_matches('/');
    let url = format!("{}/files/{}", self.base_url, clean);
    let response = self
      .authed_send(|| {
        self
          .http_client
          .put(&url)
          .header("Content-Type", "application/json")
          .header("X-Aeor-Symlink", target)
      })
      .await
      .map_err(|e| ClientError::Server(format!("create_symlink_via_header send: {}", e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "create_symlink_via_header returned HTTP {} for {}",
        response.status(),
        file_path,
      )));
    }
    Ok(())
  }

  // ---------------------------------------------------------------------------
  // Blob / chunk API
  //
  // The engine stores file content as content-addressable chunks: each
  // chunk is keyed by `blake3("chunk:" + bytes)`. A file is a list of
  // ordered chunk hashes + metadata. Uploading a file means:
  //
  //   1. blob_config()   — discover the engine's chunk size (256 KB today).
  //   2. Split local content into chunks, compute each chunk's hash.
  //   3. blob_check()    — ask the engine which of those hashes it ALREADY
  //                        has. The "needed" list is the subset to upload.
  //   4. upload_chunk()  — PUT each needed chunk's bytes. Idempotent;
  //                        existing chunks return 200 instead of 201.
  //   5. blob_commit()   — POST the file path + ordered chunk hashes;
  //                        engine atomically materializes the file from
  //                        chunks already in its store.
  //
  // Downloading is the same idea inverted: GET the file's chunk hash list
  // (already available in the /sync/diff response), figure out which we
  // already have locally (dedup against existing local chunks), then
  // /sync/chunks-fetch the rest and reassemble.
  //
  // The win over the old full-file PUT path: a 1 GB file with a 4 KB edit
  // re-uploads ~256 KB (one chunk) instead of 1 GB. Same gain on pull.
  // Cross-file dedup also falls out of this: two files sharing a chunk
  // only store it once on the engine.

  /// GET /blobs/config — engine parameters (chunk size, hash algo).
  pub async fn blob_config(&self) -> Result<BlobConfig> {
    let url = format!("{}/blobs/config", self.base_url);
    let response = self
      .authed_send(|| self.http_client.get(&url))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/blobs/config"))?;
    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        "/blobs/config",
      ));
    }
    response
      .json::<BlobConfig>()
      .await
      .map_err(|e| ClientError::UpstreamProtocol(format!("blob_config parse: {}", e)))
  }

  /// POST /blobs/check — for a list of chunk hashes, return which the
  /// engine already has + which it needs us to upload.
  pub async fn blob_check(&self, hashes: &[String]) -> Result<BlobCheckResponse> {
    if hashes.is_empty() {
      return Ok(BlobCheckResponse {
        have: Vec::new(),
        needed: Vec::new(),
      });
    }

    let mut combined = BlobCheckResponse {
      have: Vec::new(),
      needed: Vec::new(),
    };

    for chunk in hashes.chunks(BLOB_CHECK_MAX_HASHES_PER_REQUEST) {
      let response = self.blob_check_once(chunk).await?;
      combined.have.extend(response.have);
      combined.needed.extend(response.needed);
    }

    Ok(combined)
  }

  async fn blob_check_once(&self, hashes: &[String]) -> Result<BlobCheckResponse> {
    let url = format!("{}/blobs/check", self.base_url);
    let body = serde_json::json!({ "hashes": hashes });
    let response = self
      .authed_send(|| self.http_client.post(&url).json(&body))
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/blobs/check"))?;
    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        "/blobs/check",
      ));
    }
    response
      .json::<BlobCheckResponse>()
      .await
      .map_err(|e| ClientError::UpstreamProtocol(format!("blob_check parse: {}", e)))
  }

  /// PUT /blobs/chunks/{hash} — upload a single chunk's raw bytes. The
  /// engine hash-verifies `blake3("chunk:" + bytes)` against the URL and
  /// rejects on mismatch (defense against silent corruption mid-transit).
  /// 200 = chunk already existed; 201 = newly stored.
  ///
  /// Uses the same redirect-preserving send path as JSON API calls so
  /// an http→https engine redirect does not strip Authorization from
  /// chunk PUTs. On 401 we re-mint the JWT manually and retry once.
  pub async fn upload_chunk(&self, hash_hex: &str, bytes: Vec<u8>) -> Result<()> {
    let url = format!("{}/blobs/chunks/{}", self.base_url, hash_hex);
    let send = |body: Vec<u8>| async {
      let mut req = self.http_client.put(&url).body(body);
      if let Some(ref auth) = self.auth_header().await {
        req = req.header("Authorization", auth);
      }
      self.send_following_auth_redirects(req).await
    };
    let mut response = send(bytes.clone())
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/blobs/chunks"))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
      self.invalidate_token();
      response = send(bytes)
        .await
        .map_err(|e| classify_reqwest_send_error(&e, "/blobs/chunks retry"))?;
    }
    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        &format!("/blobs/chunks/{}", hash_hex),
      ));
    }
    Ok(())
  }

  /// POST /blobs/commit — materialize one or more files from already-
  /// uploaded chunks. The engine validates that every referenced chunk
  /// hash exists in its store, then atomically writes the file metadata.
  pub async fn blob_commit(&self, files: &[CommitFile]) -> Result<serde_json::Value> {
    let url = format!("{}/blobs/commit", self.base_url);
    let body = serde_json::json!({ "files": files });
    let response = self
      .authed_send_with_timeout(
        || {
          self
            .http_client
            .post(&url)
            .timeout(BLOB_COMMIT_REQUEST_TIMEOUT)
            .json(&body)
        },
        BLOB_COMMIT_REQUEST_TIMEOUT,
      )
      .await
      .map_err(|e| classify_reqwest_send_error(&e, "/blobs/commit"))?;
    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(classify_upstream_status(
        status.as_u16(),
        &body,
        "/blobs/commit",
      ));
    }
    response
      .json()
      .await
      .map_err(|e| ClientError::UpstreamProtocol(format!("blob_commit parse: {}", e)))
  }

  /// POST /sync/chunks — fetch raw chunk bytes for the given hashes.
  /// Engine returns `{ chunks: [{ hash, data: base64, size }, ...] }`.
  /// We decode and return owned `(hash, bytes)` pairs.
  ///
  /// Engine limits: at most 10,000 hashes per request, at most 512 MB
  /// of response payload. Callers must batch if either bound would be
  /// exceeded — we DON'T batch internally here so the caller can see
  /// the boundary and decide how to chunk (pun intended).
  pub async fn sync_chunks(&self, hashes: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
    let url = format!("{}/sync/chunks", self.base_url);
    let body = serde_json::json!({ "hashes": hashes });
    let response = self
      .authed_send(|| self.http_client.post(&url).json(&body))
      .await
      .map_err(|e| ClientError::Server(format!("sync_chunks send: {}", e)))?;
    if !response.status().is_success() {
      let status = response.status();
      let body = response.text().await.unwrap_or_default();
      return Err(ClientError::Server(format!(
        "sync_chunks returned HTTP {}: {}",
        status, body,
      )));
    }
    #[derive(Deserialize)]
    struct ChunksResponse {
      chunks: Vec<ChunkEntry>,
    }
    #[derive(Deserialize)]
    struct ChunkEntry {
      hash: String,
      data: String,
    }
    let parsed: ChunksResponse = response
      .json()
      .await
      .map_err(|e| ClientError::Server(format!("sync_chunks parse: {}", e)))?;

    let mut out = Vec::with_capacity(parsed.chunks.len());
    for entry in parsed.chunks {
      let bytes = base64::engine::general_purpose::STANDARD
        .decode(&entry.data)
        .map_err(|e| {
          ClientError::Server(format!("sync_chunks base64 decode {}: {}", entry.hash, e))
        })?;
      out.push((entry.hash, bytes));
    }
    Ok(out)
  }

  // --- engine call helpers ---
  //
  // Thin wrappers around `authed_send` (which handles auth + 401 retry)
  // that handle the JSON serialization + status-check + body parse
  // boilerplate for the engine UI proxy methods.

  async fn engine_get_json(&self, url: &str, op: &str) -> Result<serde_json::Value> {
    let response = self
      .authed_send(|| self.http_client.get(url))
      .await
      .map_err(|e| ClientError::Server(format!("{} send: {}", op, e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "{} returned HTTP {}",
        op,
        response.status(),
      )));
    }
    response
      .json()
      .await
      .map_err(|e| ClientError::Server(format!("{} parse: {}", op, e)))
  }

  async fn engine_post_json(
    &self,
    url: &str,
    body: &serde_json::Value,
    op: &str,
  ) -> Result<serde_json::Value> {
    let response = self
      .authed_send(|| self.http_client.post(url).json(body))
      .await
      .map_err(|e| ClientError::Server(format!("{} send: {}", op, e)))?;
    if !response.status().is_success() {
      return Err(ClientError::Server(format!(
        "{} returned HTTP {}",
        op,
        response.status(),
      )));
    }
    // Empty body is OK — return {ok: true}.
    let text = response.text().await.unwrap_or_default();
    if text.is_empty() {
      Ok(serde_json::json!({"ok": true}))
    } else {
      serde_json::from_str(&text).map_err(|e| ClientError::Server(format!("{} parse: {}", op, e)))
    }
  }
}

/// GET /blobs/config response — engine parameters for the chunk API.
/// `chunk_size` is the maximum bytes per chunk; PUT /blobs/chunks/{hash}
/// rejects anything bigger. `chunk_hash_prefix` is what gets prepended
/// to chunk bytes before hashing (currently "chunk:") so the same byte
/// sequence stored as a chunk vs as a file metadata blob hashes
/// differently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobConfig {
  pub hash_algorithm: String,
  pub chunk_size: usize,
  pub chunk_hash_prefix: String,
}

/// POST /blobs/check response. `have` is the subset of the requested
/// hashes the engine already has stored; `needed` is the subset the
/// engine wants the client to upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobCheckResponse {
  pub have: Vec<String>,
  pub needed: Vec<String>,
}

/// One file in a POST /blobs/commit batch. `chunks` are the hex-encoded
/// chunk hashes in order; the engine concatenates them in that order to
/// materialize the file's content. Trusted sync clients can include
/// `content_hash` + `size` so newer engines can validate from chunk
/// metadata without rereading every chunk body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitFile {
  pub path: String,
  pub chunks: Vec<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub content_hash: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub size: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub content_type: Option<String>,
}

/// Compute the engine-canonical chunk hash for a byte slice:
/// `blake3("chunk:" + bytes)`, hex-encoded. The "chunk:" prefix
/// distinguishes chunk-blob hashes from file-metadata hashes (same
/// algorithm, different namespace).
pub fn chunk_hash(prefix: &str, bytes: &[u8]) -> String {
  let mut hasher = blake3::Hasher::new();
  hasher.update(prefix.as_bytes());
  hasher.update(bytes);
  hasher.finalize().to_hex().to_string()
}

/// Paginated directory listing response from GET /files/{path}?limit=N&offset=M.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryListingResponse {
  pub items: Vec<RemoteEntry>,
  #[serde(default)]
  pub total: Option<u64>,
  #[serde(default)]
  pub limit: Option<u64>,
  #[serde(default)]
  pub offset: Option<u64>,
}

#[cfg(test)]
mod tests {
  use super::*;

  use axum::body::Bytes;
  use axum::extract::{Path, State};
  use axum::http::{HeaderMap, StatusCode, header};
  use axum::response::IntoResponse;
  use axum::routing::{get, post, put};
  use axum::{Json, Router};
  use chrono::Utc;
  use tokio::net::TcpListener;

  #[test]
  fn search_timeout_is_longer_than_default_request_timeout() {
    assert!(
      SEARCH_REQUEST_TIMEOUT > DEFAULT_REQUEST_TIMEOUT,
      "large indexed searches must not share the short interactive request budget",
    );
  }

  #[test]
  fn blob_commit_timeout_is_longer_than_default_request_timeout() {
    assert!(
      BLOB_COMMIT_REQUEST_TIMEOUT > DEFAULT_REQUEST_TIMEOUT,
      "blob commit can legitimately materialize many files and must not share the short interactive request budget",
    );
  }

  #[test]
  fn manual_redirect_http_clients_are_reused() {
    let default_a: *const reqwest::Client = no_redirect_http_client(DEFAULT_REQUEST_TIMEOUT);
    let default_b: *const reqwest::Client = no_redirect_http_client(DEFAULT_REQUEST_TIMEOUT);
    let search_a: *const reqwest::Client = no_redirect_http_client(SEARCH_REQUEST_TIMEOUT);
    let search_b: *const reqwest::Client = no_redirect_http_client(SEARCH_REQUEST_TIMEOUT);
    let commit_a: *const reqwest::Client = no_redirect_http_client(BLOB_COMMIT_REQUEST_TIMEOUT);
    let commit_b: *const reqwest::Client = no_redirect_http_client(BLOB_COMMIT_REQUEST_TIMEOUT);

    assert_eq!(
      default_a, default_b,
      "default manual-redirect requests must reuse the same HTTP client and connection pool",
    );
    assert_eq!(
      search_a, search_b,
      "search manual-redirect requests must reuse the same HTTP client and connection pool",
    );
    assert_eq!(
      commit_a, commit_b,
      "blob commit manual-redirect requests must reuse the same HTTP client and connection pool",
    );
    assert_ne!(
      default_a, search_a,
      "search keeps a separate longer-timeout pool",
    );
    assert_ne!(
      default_a, commit_a,
      "blob commit keeps a separate longer-timeout pool",
    );
  }

  #[test]
  fn commit_file_serializes_trusted_sync_fast_path_fields() {
    let payload = serde_json::to_value(CommitFile {
      path: "/docs/report.pdf".to_string(),
      chunks: vec!["chunk-a".to_string(), "chunk-b".to_string()],
      content_hash: Some("whole-file-hash".to_string()),
      size: Some(1234),
      content_type: Some("application/pdf".to_string()),
    })
    .expect("commit file should serialize");

    assert_eq!(payload["path"], "/docs/report.pdf");
    assert_eq!(payload["chunks"][0], "chunk-a");
    assert_eq!(payload["chunks"][1], "chunk-b");
    assert_eq!(payload["content_hash"], "whole-file-hash");
    assert_eq!(payload["size"], 1234);
    assert_eq!(payload["content_type"], "application/pdf");
  }

  #[tokio::test]
  async fn blob_config_classifies_transport_failure_as_upstream_unreachable() {
    let reqwest_client = reqwest::Client::new();
    let client = RemoteClient::from_connection(
      &test_connection("http://127.0.0.1:1".to_string()),
      &reqwest_client,
    );

    let error = client
      .blob_config()
      .await
      .expect_err("unreachable blob config endpoint should fail");

    assert!(
      matches!(error, ClientError::UpstreamUnreachable(_)),
      "expected upstream unreachable, got: {}",
      error,
    );
    assert!(
      error.to_string().contains("/blobs/config"),
      "error should identify the blob config request, got: {}",
      error,
    );
  }

  async fn start_test_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
      .await
      .expect("failed to bind test server");
    let address = listener
      .local_addr()
      .expect("failed to read test server address");

    tokio::spawn(async move {
      axum::serve(listener, app)
        .await
        .expect("test server failed");
    });

    format!("http://{}", address)
  }

  fn test_connection(url: String) -> RemoteConnection {
    RemoteConnection {
      id: "test-connection".to_string(),
      name: "Test Connection".to_string(),
      url,
      auth_type: AuthType::ApiKey,
      api_key: Some("raw-api-key".to_string()),
      share_base_url: None,
      created_at: Utc::now(),
      updated_at: Utc::now(),
    }
  }

  async fn auth_token(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    if body.get("api_key").and_then(|v| v.as_str()) != Some("raw-api-key") {
      return (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "bad api key" })),
      );
    }
    if body.get("include_refresh").and_then(|v| v.as_bool()) != Some(false) {
      return (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": "include_refresh must be false" })),
      );
    }

    (
      StatusCode::OK,
      Json(serde_json::json!({ "token": "jwt-from-post-target" })),
    )
  }

  async fn redirect_auth_token(State(target): State<String>) -> impl IntoResponse {
    (
      StatusCode::MOVED_PERMANENTLY,
      [(header::LOCATION, target)],
      "",
    )
  }

  async fn redirect_without_location() -> impl IntoResponse {
    (StatusCode::MOVED_PERMANENTLY, "")
  }

  async fn redirect_files_root(State(target): State<String>) -> impl IntoResponse {
    (
      StatusCode::MOVED_PERMANENTLY,
      [(header::LOCATION, target)],
      "",
    )
  }

  async fn protected_files_root(headers: HeaderMap) -> impl IntoResponse {
    if headers
      .get(header::AUTHORIZATION)
      .and_then(|v| v.to_str().ok())
      != Some("Bearer jwt-from-post-target")
    {
      return (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "missing auth" })),
      );
    }

    (
      StatusCode::OK,
      Json(serde_json::json!({
        "items": [
          {
            "name": "Documents",
            "entry_type": ENTRY_TYPE_DIRECTORY,
            "size": 0,
            "created_at": 0,
            "updated_at": 0,
            "content_type": null,
            "path": "/Documents/",
            "hash": null
          }
        ]
      })),
    )
  }

  async fn redirect_chunk_upload(
    State(target_base): State<String>,
    Path(hash): Path<String>,
  ) -> impl IntoResponse {
    (
      StatusCode::MOVED_PERMANENTLY,
      [(
        header::LOCATION,
        format!("{}/blobs/chunks/{}", target_base, hash),
      )],
      "",
    )
  }

  async fn protected_chunk_upload(headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    if headers
      .get(header::AUTHORIZATION)
      .and_then(|v| v.to_str().ok())
      != Some("Bearer jwt-from-post-target")
    {
      return (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "missing auth" })),
      );
    }
    if body.as_ref() != b"chunk bytes" {
      return (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": "wrong body" })),
      );
    }

    (StatusCode::CREATED, Json(serde_json::json!({ "ok": true })))
  }

  #[tokio::test]
  async fn exchange_token_preserves_post_across_301_redirect() {
    let target_base = start_test_server(Router::new().route("/auth/token", post(auth_token))).await;
    let redirect_base = start_test_server(
      Router::new()
        .route("/auth/token", post(redirect_auth_token))
        .with_state(format!("{}/auth/token", target_base)),
    )
    .await;

    let reqwest_client = reqwest::Client::new();
    let client = RemoteClient::from_connection(&test_connection(redirect_base), &reqwest_client);

    let token = client
      .exchange_token("raw-api-key")
      .await
      .expect("token exchange should follow 301 by re-issuing POST");

    assert_eq!(token, "jwt-from-post-target");
  }

  #[tokio::test]
  async fn exchange_token_reports_redirect_without_location() {
    let redirect_base =
      start_test_server(Router::new().route("/auth/token", post(redirect_without_location))).await;

    let reqwest_client = reqwest::Client::new();
    let client = RemoteClient::from_connection(&test_connection(redirect_base), &reqwest_client);

    let error = client
      .exchange_token("raw-api-key")
      .await
      .expect_err("redirect without Location should fail");

    assert!(
      error.to_string().contains("no Location header"),
      "unexpected error: {}",
      error,
    );
  }

  #[tokio::test]
  async fn list_directory_preserves_authorization_across_redirect() {
    let target_base =
      start_test_server(Router::new().route("/files/", get(protected_files_root))).await;
    let redirect_base = start_test_server(
      Router::new()
        .route("/auth/token", post(auth_token))
        .route("/files/", get(redirect_files_root))
        .with_state(format!("{}/files/", target_base)),
    )
    .await;

    let reqwest_client = reqwest::Client::new();
    let client = RemoteClient::from_connection(&test_connection(redirect_base), &reqwest_client);

    let entries = client
      .list_directory("/")
      .await
      .expect("directory listing should preserve auth through redirect");

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "Documents");
    assert!(entries[0].is_directory());
  }

  #[tokio::test]
  async fn upload_chunk_preserves_authorization_across_redirect() {
    let target_base =
      start_test_server(Router::new().route("/blobs/chunks/{hash}", put(protected_chunk_upload)))
        .await;
    let redirect_base = start_test_server(
      Router::new()
        .route("/auth/token", post(auth_token))
        .route("/blobs/chunks/{hash}", put(redirect_chunk_upload))
        .with_state(target_base),
    )
    .await;

    let reqwest_client = reqwest::Client::new();
    let client = RemoteClient::from_connection(&test_connection(redirect_base), &reqwest_client);

    client
      .upload_chunk("abc123", b"chunk bytes".to_vec())
      .await
      .expect("chunk upload should preserve auth through redirect");
  }
}
