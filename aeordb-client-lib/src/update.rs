//! Self-update flow.
//!
//! Polls `https://aeordb.com/api/version?os=...&arch=...` on startup,
//! caches the result in `AppState.update_info`, and on user request
//! downloads the new artifact, verifies its sha256 against the signed
//! envelope, and hands off to a tiny per-platform relauncher script
//! that waits for the current process to exit, swaps the binary, and
//! re-launches.
//!
//! Ported verbatim from xenocept-client's `src/update.rs`. Brand
//! constants renamed (XENOCEPT_* → AEORDB_*, xenocept → aeordb-client,
//! download names + relauncher --show flag dropped — aeordb-client has
//! no --show CLI flag; it uses --start-minimized for hidden launch, and
//! we WANT the window to open on a fresh-after-update launch so the
//! user sees that the update completed). Trust store + signature
//! algorithm + envelope schema are byte-identical, so a manifest signed
//! by aeordb-www verifies under this client and vice versa.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_VERSION_ENDPOINT: &str = "https://aeordb.com/api/version";
const DEFAULT_DOWNLOAD_BASE: &str = "https://aeordb.com/downloads/";

/// Loopback-test override. When set, replaces the version endpoint and
/// the download base URL prefix. Format: a single base URL — both
/// derived endpoints share it. Empty / unset = production.
///
/// Example: `AEORDB_UPDATE_ENDPOINT=http://127.0.0.1:8765` → endpoint
/// becomes `http://127.0.0.1:8765/api/version`, download base becomes
/// `http://127.0.0.1:8765/downloads/`.
fn version_endpoint() -> String {
  match std::env::var("AEORDB_UPDATE_ENDPOINT") {
    Ok(base) if !base.is_empty() => format!("{}/api/version", base.trim_end_matches('/')),
    _ => DEFAULT_VERSION_ENDPOINT.to_string(),
  }
}
fn download_base() -> String {
  match std::env::var("AEORDB_UPDATE_ENDPOINT") {
    Ok(base) if !base.is_empty() => format!("{}/downloads/", base.trim_end_matches('/')),
    _ => DEFAULT_DOWNLOAD_BASE.to_string(),
  }
}

/// Snapshot of the most recent `/api/version` poll. Stored in
/// `AppState.update_info` and served verbatim from
/// `GET /api/v1/update/status`.
///
/// Trust model: the convenience fields (`latest_version`, `download_url`,
/// `size`, `sha256`) are advisory only — derived by aeordb-www from the
/// signed envelope before the response goes out. The ONLY trusted source
/// for an apply is `signed_manifest` + `signature` + `key_id`, which are
/// verified against `security::sig::effective_trust_store()` in
/// `apply_update` before any binary is downloaded or swapped. A response
/// that lacks a valid signature is treated as "no update available"
/// regardless of what `available` says.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateInfo {
  /// True iff the server returned a version strictly greater than the
  /// running binary's `CARGO_PKG_VERSION` AND a valid signature.
  pub available: bool,
  /// Running binary's version (always populated).
  pub current_version: String,
  /// Latest version reported by the server, when the call succeeded.
  pub latest_version: Option<String>,
  pub release_notes_url: Option<String>,
  pub download_url: Option<String>,
  pub size: Option<i64>,
  pub sha256: Option<String>,
  /// Canonical `<os>-<arch>` the request resolved to.
  pub platform: String,
  /// When the last successful poll completed.
  pub last_checked: Option<DateTime<Utc>>,
  /// Set when the most recent poll failed (network down, 5xx, parse
  /// error, signature verification failure). Cleared on the next
  /// successful poll. 404 / 503 from the server are NOT errors —
  /// they're "no update," see `check_once`.
  pub error: Option<String>,
  /// The signed manifest envelope, as a `serde_json::Value` so its
  /// canonical-bytes form survives JCS-re-canonicalization byte-for-
  /// byte. Stored verbatim from the `/api/version` response.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub signed_manifest: Option<serde_json::Value>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub key_id: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub signature: Option<String>,
}

/// Inner shape of `signed_manifest`. Used during verify + apply to pull
/// out the per-platform `file` + `sha256` from the trusted blob (NOT
/// from the convenience fields, which an attacker could spoof on the
/// wire if they controlled aeordb-www).
#[derive(Debug, Deserialize)]
struct SignedManifest {
  #[serde(default)]
  version: Option<String>,
  #[serde(default)]
  platforms: std::collections::BTreeMap<String, SignedPlatformEntry>,
}

#[derive(Debug, Deserialize)]
struct SignedPlatformEntry {
  file: String,
  #[serde(default)]
  size: Option<i64>,
  sha256: String,
}

pub type SharedUpdateInfo = Arc<RwLock<UpdateInfo>>;

pub fn new_state() -> SharedUpdateInfo {
  Arc::new(RwLock::new(UpdateInfo {
    current_version: env!("CARGO_PKG_VERSION").to_string(),
    platform: current_platform_key(),
    ..Default::default()
  }))
}

/// `<os>-<arch>` string the server expects. Falls back to a permissive
/// "unknown-unknown" if we ever build for a target whose consts we don't
/// recognize — the server will respond 404 and the client will treat
/// that as "no update."
pub fn current_platform_key() -> String {
  let os = std::env::consts::OS; // "linux" | "macos" | "windows"
  let arch = std::env::consts::ARCH; // "x86_64" | "aarch64" | ...
  format!("{os}-{arch}")
}

/// One-shot poll. Updates `state` in place. Never panics, never returns
/// an error — all failure modes land as `info.error` on the shared
/// snapshot so the UI can surface them if it wants.
pub async fn check_once(client: &reqwest::Client, state: &SharedUpdateInfo) {
  let info = check_inner(client).await;
  if let Ok(mut guard) = state.write() {
    // Preserve current_version + platform across writes (they're
    // process-static; check_inner doesn't bother re-stamping them).
    let current = guard.current_version.clone();
    let plat = guard.platform.clone();
    *guard = info;
    if guard.current_version.is_empty() {
      guard.current_version = current;
    }
    if guard.platform.is_empty() {
      guard.platform = plat;
    }
  }
}

async fn check_inner(client: &reqwest::Client) -> UpdateInfo {
  let current_version = env!("CARGO_PKG_VERSION").to_string();
  let os = std::env::consts::OS;
  let arch = std::env::consts::ARCH;
  let platform = format!("{os}-{arch}");
  let url = format!("{}?os={os}&arch={arch}", version_endpoint());

  let resp = match client
    .get(&url)
    .timeout(Duration::from_secs(10))
    .send()
    .await
  {
    Ok(r) => r,
    Err(e) => {
      return UpdateInfo {
        current_version,
        platform,
        error: Some(format!("network: {e}")),
        ..Default::default()
      };
    }
  };

  let status = resp.status().as_u16();
  match status {
    200 => {}
    404 | 503 => {
      tracing::debug!("update check: server returned {status} (no update available)");
      return UpdateInfo {
        current_version,
        platform,
        last_checked: Some(Utc::now()),
        ..Default::default()
      };
    }
    other => {
      return UpdateInfo {
        current_version,
        platform,
        error: Some(format!("server returned {other}")),
        ..Default::default()
      };
    }
  }

  #[derive(Deserialize)]
  struct VersionResponse {
    version: String,
    #[serde(default)]
    released_at: Option<DateTime<Utc>>,
    #[serde(default)]
    release_notes_url: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    size: Option<i64>,
    #[serde(default)]
    sha256: Option<String>,
    // Required for any apply path. If absent, we treat the result as
    // "no update" — better to silently skip than to install something
    // we can't authenticate.
    #[serde(default)]
    signed_manifest: Option<serde_json::Value>,
    #[serde(default)]
    key_id: Option<String>,
    #[serde(default)]
    signature: Option<String>,
  }
  let body: VersionResponse = match resp.json().await {
    Ok(b) => b,
    Err(e) => {
      return UpdateInfo {
        current_version,
        platform,
        error: Some(format!("parse: {e}")),
        ..Default::default()
      };
    }
  };

  let _ = body.released_at; // unused — UI shows last_checked instead

  // Verify the signature BEFORE setting `available`. An unsigned or
  // invalid response gets surfaced as `error` (so a future operator
  // looking at /api/v1/update/status sees what's wrong) but with
  // `available: false` — so the UI never offers to install it.
  let sig_check = verify_manifest_signature(
    body.signed_manifest.as_ref(),
    body.key_id.as_deref(),
    body.signature.as_deref(),
    &body.version,
  );
  let (available, error) = match sig_check {
    Ok(()) => (is_newer(&body.version, &current_version), None),
    Err(e) => (false, Some(format!("signature: {e}"))),
  };

  UpdateInfo {
    available,
    current_version,
    latest_version: Some(body.version),
    release_notes_url: body.release_notes_url,
    download_url: body.download_url,
    size: body.size,
    sha256: body.sha256,
    platform: body.platform.unwrap_or(platform),
    last_checked: Some(Utc::now()),
    error,
    signed_manifest: body.signed_manifest,
    key_id: body.key_id,
    signature: body.signature,
  }
}

/// Verify the ed25519 signature over the JCS-canonical form of the
/// signed manifest envelope. Pulls the trusted public key from
/// `security::sig::effective_trust_store()` by `key_id`. Refuses if:
///   - any of `signed_manifest` / `key_id` / `signature` is missing
///   - `key_id` is not in the bundled trust store
///   - the signature does not verify
///   - the envelope's `version` field disagrees with the response's
///     top-level `version` field (server tried to advertise A but
///     ship B)
fn verify_manifest_signature(
  signed_manifest: Option<&serde_json::Value>,
  key_id: Option<&str>,
  signature_b64: Option<&str>,
  announced_version: &str,
) -> Result<()> {
  let signed_manifest = signed_manifest.ok_or_else(|| anyhow!("response lacks signed_manifest"))?;
  let key_id = key_id.ok_or_else(|| anyhow!("response lacks key_id"))?;
  let signature_b64 = signature_b64.ok_or_else(|| anyhow!("response lacks signature"))?;

  // Locate the trusted public key.
  let trust_store = crate::security::sig::effective_trust_store();
  let key_bytes = trust_store
    .iter()
    .find(|(id, _)| id.as_str() == key_id)
    .map(|(_, bytes)| *bytes)
    .ok_or_else(|| anyhow!("unknown key_id: {}", key_id))?;
  let verifying_key = VerifyingKey::from_bytes(&key_bytes)
    .map_err(|e| anyhow!("malformed trusted public key: {}", e))?;

  // JCS-canonicalize the envelope. Both this code path AND the signer
  // on the aeordb-www side use `json_canon::to_string` on the SAME
  // serde_json shape, so the canonical bytes are byte-identical.
  let canonical =
    json_canon::to_string(signed_manifest).map_err(|e| anyhow!("canonicalize envelope: {}", e))?;

  let sig_bytes = base64::engine::general_purpose::STANDARD
    .decode(signature_b64)
    .map_err(|e| anyhow!("decode signature: {}", e))?;
  if sig_bytes.len() != 64 {
    bail!(
      "signature wrong length: expected 64, got {}",
      sig_bytes.len()
    );
  }
  let mut sig_arr = [0u8; 64];
  sig_arr.copy_from_slice(&sig_bytes);
  let signature = Signature::from_bytes(&sig_arr);

  verifying_key
    .verify(canonical.as_bytes(), &signature)
    .map_err(|_| anyhow!("ed25519 verification failed"))?;

  // Cross-check: the envelope's `version` must match the response's
  // top-level `version`. Without this, an attacker who can downgrade
  // an old signed response could pin clients to an outdated build.
  let envelope: SignedManifest = serde_json::from_value(signed_manifest.clone())
    .map_err(|e| anyhow!("parse signed envelope: {}", e))?;
  match envelope.version.as_deref() {
    Some(v) if v == announced_version => Ok(()),
    Some(v) => bail!("envelope version {} != announced {}", v, announced_version),
    None => bail!("envelope lacks version field"),
  }
}

/// Strict semver-ish comparison: parse "MAJOR.MINOR.PATCH" tuples and
/// compare lexicographically. Bare/prerelease/build-metadata versions
/// fall back to string compare — good enough for our linear release
/// cadence; if we ever ship "0.9.6-rc1" we'll revisit.
fn is_newer(remote: &str, current: &str) -> bool {
  let parse = |s: &str| -> Option<(u32, u32, u32)> {
    let mut it = s.split('.').take(3);
    let major = it.next()?.parse::<u32>().ok()?;
    let minor = it.next()?.parse::<u32>().ok()?;
    let patch = it.next()?.parse::<u32>().ok()?;
    Some((major, minor, patch))
  };
  match (parse(remote), parse(current)) {
    (Some(r), Some(c)) => r > c,
    _ => remote > current,
  }
}

// ---------------------------------------------------------------------------
// Apply path
// ---------------------------------------------------------------------------

/// Progress events emitted by `apply_update` as it works through the
/// download → verify → stage pipeline. Streamed to the UI via NDJSON
/// over the `POST /api/v1/update/apply` response body so the About-
/// page progress bar can show real bytes/total + phase transitions
/// instead of a static "Updating…" label.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum ProgressEvent {
  /// Bytes downloaded so far. `total` is 0 when Content-Length is
  /// absent (HTTP/1.1 chunked responses without a known length).
  Downloading {
    bytes: u64,
    total: u64,
  },
  Verifying,
  Staging,
  Complete,
  Error {
    message: String,
  },
}

/// Download → verify sha256 → stage relauncher → return.
///
/// On success, the caller is expected to exit the process shortly after
/// (via std::process::exit). The relauncher script we spawned is polling
/// for our PID to disappear; once it does, it swaps the binary in place
/// and re-launches.
///
/// The function returns `Ok(())` *before* the relaunch happens (we spawn
/// the relauncher detached and return). Errors are surfaced if download
/// or verification fails — those happen *before* we touch the running
/// binary, so they're safe to retry.
pub async fn apply_update(
  info: &UpdateInfo,
  progress: Option<tokio::sync::mpsc::Sender<ProgressEvent>>,
) -> Result<()> {
  // Re-verify the signature at apply time. The `check_once` poll
  // already verified it once, but this is the moment we touch the
  // filesystem — so we re-check rather than rely on the cached
  // `available` flag. Defense-in-depth against an in-process tamper
  // (a plugin or compromised dependency editing AppState.update_info
  // wouldn't pass this second check unless it also produced a valid
  // ed25519 forgery, which it can't without the offline key).
  let announced_version = info
    .latest_version
    .as_deref()
    .ok_or_else(|| anyhow!("no latest_version in update info"))?;
  verify_manifest_signature(
    info.signed_manifest.as_ref(),
    info.key_id.as_deref(),
    info.signature.as_deref(),
    announced_version,
  )
  .context("manifest signature verify failed at apply time")?;

  // Read the trusted file + sha256 OUT OF THE SIGNED ENVELOPE, not
  // from `info.download_url` / `info.sha256`. The convenience fields
  // are advisory; only the signed envelope is binding.
  let envelope: SignedManifest = serde_json::from_value(
    info
      .signed_manifest
      .clone()
      .ok_or_else(|| anyhow!("apply requires signed_manifest"))?,
  )
  .context("parse signed envelope")?;
  let platform_key = current_platform_key();
  let entry = envelope
    .platforms
    .get(&platform_key)
    .ok_or_else(|| anyhow!("signed envelope has no entry for platform {}", platform_key))?;
  let download_url_owned = format!("{}{}", download_base(), entry.file);
  let download_url = download_url_owned.as_str();
  let expected_sha = Some(entry.sha256.clone());
  let _ = info.download_url; // intentionally unused — see comment above

  let current_exe = std::env::current_exe().context("failed to determine current exe path")?;
  let pid = std::process::id();

  // Stage the artifact next to the current exe so the rename is a
  // same-filesystem move (atomic). For macOS we stage the .zip in
  // /tmp instead — the .app bundle replacement happens via the
  // unzip-and-rsync flow in the relauncher script.
  let stage_dir: PathBuf = if cfg!(target_os = "macos") {
    std::env::temp_dir()
  } else {
    current_exe
      .parent()
      .ok_or_else(|| anyhow!("current_exe has no parent"))?
      .to_path_buf()
  };
  let staged_name = if cfg!(target_os = "macos") {
    "aeordb-client-update.app.zip".to_string()
  } else if cfg!(target_os = "windows") {
    "aeordb-client.new.exe".to_string()
  } else {
    "aeordb-client.new".to_string()
  };
  let staged_path = stage_dir.join(&staged_name);

  download_to(download_url, &staged_path, progress.clone())
    .await
    .with_context(|| format!("download {download_url} -> {}", staged_path.display()))?;

  if let Some(tx) = &progress {
    let _ = tx.send(ProgressEvent::Verifying).await;
  }
  if let Some(sha) = &expected_sha {
    verify_sha256(&staged_path, sha)
      .with_context(|| format!("sha256 verify failed for {}", staged_path.display()))?;
  } else {
    tracing::warn!("update: server returned no sha256 — skipping integrity check");
  }

  // Make it executable on POSIX (Windows ignores; .exe is already
  // executable by extension).
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&staged_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&staged_path, perms)?;
  }

  if let Some(tx) = &progress {
    let _ = tx.send(ProgressEvent::Staging).await;
  }
  spawn_relauncher(pid, &current_exe, &staged_path)?;
  if let Some(tx) = &progress {
    let _ = tx.send(ProgressEvent::Complete).await;
  }
  Ok(())
}

async fn download_to(
  url: &str,
  dest: &Path,
  progress: Option<tokio::sync::mpsc::Sender<ProgressEvent>>,
) -> Result<()> {
  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(600))
    .build()?;
  let mut resp = client.get(url).send().await?;
  if !resp.status().is_success() {
    bail!("download returned {}", resp.status());
  }
  let total = resp.content_length().unwrap_or(0);
  if let Some(parent) = dest.parent() {
    std::fs::create_dir_all(parent)?;
  }
  let mut file = std::fs::File::create(dest)?;
  use std::io::Write;
  let mut downloaded: u64 = 0;
  // Throttle progress emit to ~10/s so a 50 MB download streams ~50
  // events, not ~500. Always emit the final byte regardless.
  let mut last_emit = std::time::Instant::now();
  // Send the initial 0/total so the UI knows the total ASAP.
  if let Some(tx) = &progress {
    let _ = tx
      .send(ProgressEvent::Downloading { bytes: 0, total })
      .await;
  }
  while let Some(chunk) = resp.chunk().await? {
    file.write_all(&chunk)?;
    downloaded += chunk.len() as u64;
    if let Some(tx) = &progress {
      if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
        let _ = tx
          .send(ProgressEvent::Downloading {
            bytes: downloaded,
            total,
          })
          .await;
        last_emit = std::time::Instant::now();
      }
    }
  }
  file.flush()?;
  if let Some(tx) = &progress {
    let _ = tx
      .send(ProgressEvent::Downloading {
        bytes: downloaded,
        total,
      })
      .await;
  }
  Ok(())
}

fn verify_sha256(path: &Path, expected_hex: &str) -> Result<()> {
  let mut file = std::fs::File::open(path)?;
  let mut hasher = Sha256::new();
  std::io::copy(&mut file, &mut hasher)?;
  let got = hex::encode(hasher.finalize());
  if !got.eq_ignore_ascii_case(expected_hex) {
    bail!("sha256 mismatch: expected {expected_hex}, got {got}");
  }
  Ok(())
}

#[cfg(target_os = "linux")]
fn spawn_relauncher(pid: u32, current_exe: &Path, staged: &Path) -> Result<()> {
  // Wait for current PID to exit, swap, relaunch. We do NOT pass
  // --start-minimized here: after a manual update the user clicked
  // "Update", so they're expecting visible confirmation that it
  // finished — popping the window (default Tauri behavior) is the
  // right read.
  let script = format!(
    "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
     mv {staged} {current}; \
     chmod +x {current}; \
     {current} &",
    pid = pid,
    staged = shell_quote(&staged.to_string_lossy()),
    current = shell_quote(&current_exe.to_string_lossy()),
  );
  std::process::Command::new("sh")
    .arg("-c")
    .arg(script)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .context("failed to spawn relauncher")?;
  Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_relauncher(pid: u32, current_exe: &Path, staged: &Path) -> Result<()> {
  // Walk up from <app>/Contents/MacOS/aeordb-client to find the .app root.
  let app_root = find_macos_app_root(current_exe).ok_or_else(|| {
    anyhow!(
      "could not locate .app bundle from {}",
      current_exe.display()
    )
  })?;
  // The staged .zip contains a single top-level .app — unzip into a
  // temp dir, then ditto-replace the live app bundle.
  let extract_dir = std::env::temp_dir().join("aeordb-client-update-extract");
  let script = format!(
    "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; \
     rm -rf {extract_dir}; \
     mkdir -p {extract_dir}; \
     /usr/bin/ditto -x -k {staged} {extract_dir}; \
     NEW_APP=$(find {extract_dir} -maxdepth 2 -name '*.app' -type d | head -n1); \
     if [ -z \"$NEW_APP\" ]; then exit 1; fi; \
     /bin/rm -rf {app_root}.old; \
     /bin/mv {app_root} {app_root}.old; \
     /bin/mv \"$NEW_APP\" {app_root}; \
     /bin/rm -rf {app_root}.old; \
     /usr/bin/open {app_root}",
    pid = pid,
    staged = shell_quote(&staged.to_string_lossy()),
    extract_dir = shell_quote(&extract_dir.to_string_lossy()),
    app_root = shell_quote(&app_root.to_string_lossy()),
  );
  std::process::Command::new("sh")
    .arg("-c")
    .arg(script)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .context("failed to spawn relauncher")?;
  Ok(())
}

#[cfg(target_os = "macos")]
fn find_macos_app_root(exe: &Path) -> Option<PathBuf> {
  let mut p = exe.parent()?;
  while p.parent().is_some() {
    if p.extension().and_then(|s| s.to_str()) == Some("app") {
      return Some(p.to_path_buf());
    }
    p = p.parent()?;
  }
  None
}

#[cfg(target_os = "windows")]
fn spawn_relauncher(pid: u32, current_exe: &Path, staged: &Path) -> Result<()> {
  // Powershell relauncher. Waits for current PID, renames old exe out
  // of the way (Windows allows renaming a running .exe), moves staged
  // exe into place, launches it. Cleans up the .old.exe on exit.
  let current_str = current_exe.to_string_lossy().replace('\'', "''");
  let staged_str = staged.to_string_lossy().replace('\'', "''");
  let script = format!(
    "$pid_ = {pid}; \
     while (Get-Process -Id $pid_ -ErrorAction SilentlyContinue) {{ Start-Sleep -Milliseconds 200 }}; \
     $old = '{current}'; $stg = '{staged}'; $bak = $old + '.old'; \
     if (Test-Path $bak) {{ Remove-Item -Force $bak }}; \
     if (Test-Path $old) {{ Move-Item -Force $old $bak }}; \
     Move-Item -Force $stg $old; \
     Start-Process -FilePath $old; \
     Start-Sleep -Seconds 3; \
     if (Test-Path $bak) {{ Remove-Item -Force $bak }}",
    pid = pid,
    current = current_str,
    staged = staged_str,
  );
  // CREATE_NO_WINDOW = 0x08000000 — keep the powershell console hidden.
  use std::os::windows::process::CommandExt;
  std::process::Command::new("powershell")
    .arg("-NoProfile")
    .arg("-WindowStyle")
    .arg("Hidden")
    .arg("-Command")
    .arg(&script)
    .creation_flags(0x08000000)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .context("failed to spawn relauncher")?;
  Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn spawn_relauncher(_pid: u32, _current_exe: &Path, _staged: &Path) -> Result<()> {
  bail!("self-update not supported on this platform");
}

/// Single-quote a string for `sh -c "..."` use — wraps in single quotes
/// and escapes embedded single quotes via the POSIX-portable `'\''` trick.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn shell_quote(s: &str) -> String {
  format!("'{}'", s.replace('\'', "'\\''"))
}

/// Windows startup cleanup: if a `<exe>.old` exists alongside the
/// running binary, remove it. Created by the relauncher script the
/// previous run; the script also tries to clean up on its own, but it
/// gives up after 3s if the file is still locked, so a second pass at
/// startup is a belt-and-suspenders measure.
#[cfg(target_os = "windows")]
pub fn cleanup_after_relaunch() {
  if let Ok(exe) = std::env::current_exe() {
    let bak = {
      let mut s = exe.into_os_string();
      s.push(".old");
      PathBuf::from(s)
    };
    if bak.exists() {
      let _ = std::fs::remove_file(&bak);
    }
  }
}

#[cfg(not(target_os = "windows"))]
pub fn cleanup_after_relaunch() {}

/// Loopback-test hook: if `AEORDB_TEST_PUBLIC_KEY=<key-id>:<hex32>` is
/// set, register that key in the in-process trust store at startup.
///
/// This is the only way to make a locally-generated keypair verify
/// against the bundled trust store on a release-mode binary. Without
/// this hook the only way to test the apply path end-to-end would be
/// to sign against the real `aeor-202605132323` key, which lives in
/// offline cold storage. This env var stays unset in production.
pub fn ingest_test_public_key() {
  let raw = match std::env::var("AEORDB_TEST_PUBLIC_KEY") {
    Ok(s) if !s.is_empty() => s,
    _ => return,
  };
  let (key_id, hex_part) = match raw.split_once(':') {
    Some(parts) => parts,
    None => {
      tracing::warn!(
        "AEORDB_TEST_PUBLIC_KEY: expected <key-id>:<hex32>, got {:?} — ignoring",
        raw
      );
      return;
    }
  };
  let bytes = match hex::decode(hex_part) {
    Ok(b) => b,
    Err(e) => {
      tracing::warn!(
        "AEORDB_TEST_PUBLIC_KEY: hex decode failed: {} — ignoring",
        e
      );
      return;
    }
  };
  if bytes.len() != 32 {
    tracing::warn!(
      "AEORDB_TEST_PUBLIC_KEY: expected 32 bytes, got {} — ignoring",
      bytes.len()
    );
    return;
  }
  let mut arr = [0u8; 32];
  arr.copy_from_slice(&bytes);
  crate::security::sig::register_test_key(key_id, arr);
  tracing::warn!(
    "AEORDB_TEST_PUBLIC_KEY: registered loopback trust key {} — DO NOT USE IN PRODUCTION",
    key_id
  );
}

#[cfg(test)]
mod tests {
  use super::*;
  use base64::Engine;
  use ed25519_dalek::{Signer, SigningKey};

  #[test]
  fn is_newer_compares_semver() {
    assert!(is_newer("0.9.6", "0.9.5"));
    assert!(is_newer("0.10.0", "0.9.99"));
    assert!(is_newer("1.0.0", "0.9.5"));
    assert!(!is_newer("0.9.5", "0.9.5"));
    assert!(!is_newer("0.9.4", "0.9.5"));
  }

  /// Build a signed envelope with the supplied key. Returns
  /// (envelope_json_value, key_id, signature_b64).
  fn build_signed_envelope(
    signing_key: &SigningKey,
    key_id: &str,
    version: &str,
  ) -> (serde_json::Value, String, String) {
    let envelope = serde_json::json!({
      "kind":             "aeordb-update-manifest",
      "manifest_version": 1,
      "version":          version,
      "released_at":      "2026-05-25T05:00:00Z",
      "platforms": {
        "linux-x86_64":   { "file": "aeordb-client-linux-x86_64",       "size": 100, "sha256": "aa" },
        "windows-x86_64": { "file": "aeordb-client-windows-x86_64.exe", "size": 100, "sha256": "bb" },
      },
    });
    let canonical = json_canon::to_string(&envelope).unwrap();
    let signature = signing_key.sign(canonical.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    (envelope, key_id.to_string(), sig_b64)
  }

  #[test]
  fn verify_rejects_missing_fields() {
    let r = verify_manifest_signature(None, Some("k"), Some("s"), "0.9.6");
    assert!(r.is_err(), "missing signed_manifest must fail");

    let env = serde_json::json!({"version": "0.9.6"});
    let r = verify_manifest_signature(Some(&env), None, Some("s"), "0.9.6");
    assert!(r.is_err(), "missing key_id must fail");

    let r = verify_manifest_signature(Some(&env), Some("k"), None, "0.9.6");
    assert!(r.is_err(), "missing signature must fail");
  }

  #[test]
  fn verify_rejects_unknown_key_id() {
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let (envelope, _key_id, sig) = build_signed_envelope(&sk, "aeor-test-not-registered", "0.9.6");
    let r = verify_manifest_signature(
      Some(&envelope),
      Some("aeor-test-not-registered"),
      Some(&sig),
      "0.9.6",
    );
    assert!(r.is_err(), "unknown key_id must fail");
    assert!(r.unwrap_err().to_string().contains("unknown key_id"));
  }

  #[test]
  fn verify_accepts_valid_signature() {
    let sk = SigningKey::from_bytes(&[11u8; 32]);
    let vk = sk.verifying_key().to_bytes();
    crate::security::sig::register_test_key("aeor-test-update-verify-ok", vk);

    let (envelope, key_id, sig) = build_signed_envelope(&sk, "aeor-test-update-verify-ok", "0.9.6");
    let r = verify_manifest_signature(Some(&envelope), Some(&key_id), Some(&sig), "0.9.6");
    assert!(r.is_ok(), "valid signature must verify: {:?}", r.err());
  }

  #[test]
  fn verify_rejects_tampered_envelope() {
    let sk = SigningKey::from_bytes(&[13u8; 32]);
    let vk = sk.verifying_key().to_bytes();
    crate::security::sig::register_test_key("aeor-test-update-tamper", vk);

    let (mut envelope, key_id, sig) =
      build_signed_envelope(&sk, "aeor-test-update-tamper", "0.9.6");
    // Tamper: swap the linux sha256 (an attacker redirecting the binary).
    envelope["platforms"]["linux-x86_64"]["sha256"] = serde_json::json!("ff");

    let r = verify_manifest_signature(Some(&envelope), Some(&key_id), Some(&sig), "0.9.6");
    assert!(r.is_err(), "tampered envelope must fail signature check");
    assert!(
      r.unwrap_err()
        .to_string()
        .contains("ed25519 verification failed")
    );
  }

  #[test]
  fn verify_rejects_version_mismatch() {
    let sk = SigningKey::from_bytes(&[17u8; 32]);
    let vk = sk.verifying_key().to_bytes();
    crate::security::sig::register_test_key("aeor-test-update-vermismatch", vk);

    // Envelope claims version 0.9.6, but the announced version on the
    // outer response is 1.0.0 (a downgrade-substitution attempt).
    let (envelope, key_id, sig) =
      build_signed_envelope(&sk, "aeor-test-update-vermismatch", "0.9.6");
    let r = verify_manifest_signature(Some(&envelope), Some(&key_id), Some(&sig), "1.0.0");
    assert!(r.is_err(), "version mismatch must fail");
    assert!(r.unwrap_err().to_string().contains("envelope version"));
  }
}
