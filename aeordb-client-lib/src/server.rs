use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use axum::Router;
use axum::routing::{delete, get, post};
use tokio::net::TcpListener;
use tokio::sync::{Notify, broadcast};

use crate::api::routes::conflicts;
use crate::api::routes::connections;
use crate::api::routes::events;
use crate::api::routes::files;
use crate::api::routes::preferences as preferences_routes;
use crate::api::routes::settings;
use crate::api::routes::shares;
use crate::api::routes::status::get_status;
use crate::api::routes::sync;
use crate::api::routes::system;
use crate::config::{ConfigStore, default_config_path, default_data_path};
use crate::error::{ClientError, Result};
use crate::health::{HealthMap, new_health_map, start_health_pinger};
use crate::preferences::PreferencesStore;
use crate::state::StateStore;
use crate::sync::runner::SyncRunner;

/// A single SSE event broadcast to all `/api/v1/events` subscribers. The
/// `event_name` becomes the SSE `event:` field so JS listeners can route
/// on it (e.g. `evtSource.addEventListener('connection_health', …)`). The
/// `data` is opaque to the broadcast channel — typically JSON, but the
/// SSE handler passes it through verbatim as the `data:` field.
#[derive(Clone, Debug)]
pub struct ServerEvent {
  pub event_name: String,
  pub data:       String,
}

impl ServerEvent {
  pub fn new(event_name: impl Into<String>, data: impl Into<String>) -> Self {
    Self { event_name: event_name.into(), data: data.into() }
  }
}

#[derive(Clone)]
pub struct AppState {
  pub started_at:      Instant,
  pub state_store:     Arc<StateStore>,
  pub config_store:    Arc<ConfigStore>,
  pub sync_runner:     SyncRunner,
  pub http_client:     reqwest::Client,
  pub shutdown_signal: Option<Arc<Notify>>,
  pub event_tx:        broadcast::Sender<ServerEvent>,
  pub config_dir:      PathBuf,
  pub data_dir:        PathBuf,
  pub health_map:      HealthMap,
  pub preferences:     Arc<PreferencesStore>,

  /// Desired auto-start state. Written by `PATCH /api/v1/settings`
  /// (auto_start_system field); read by main.rs's autostart listener
  /// thread which owns the tauri-plugin-autostart handle.
  pub autostart_enabled: Arc<AtomicBool>,
  /// Notified after `autostart_enabled` changes. The listener thread
  /// wakes, reads the latest desired state, and applies it via the
  /// plugin.
  pub autostart_signal:  Arc<Notify>,
}

pub struct ServerConfig {
  pub host:        String,
  pub port:        u16,
  pub config_path: PathBuf,
  pub data_path:   PathBuf,
}

impl Default for ServerConfig {
  fn default() -> Self {
    Self {
      host:        "127.0.0.1".to_string(),
      port:        9400,
      config_path: default_config_path(),
      data_path:   default_data_path(),
    }
  }
}

pub fn build_router(state: AppState) -> Router {
  let api_routes = Router::new()
    .route("/status", get(get_status))
    .route("/connections", get(connections::list_connections).post(connections::create_connection))
    .route("/connections/{id}", get(connections::get_connection).patch(connections::update_connection).delete(connections::delete_connection))
    .route("/connections/{id}/test", post(connections::test_connection))
    .route("/connections/{id}/browse", get(connections::browse_remote))
    .route("/connections/{id}/portal-url", get(connections::portal_url))
    .route("/connections/{id}/proxy/{*path}", get(connections::proxy_remote))
    .route("/health/connections", get(connections::list_health))
    .route("/sync", get(sync::list_relationships).post(sync::create_relationship))
    .route("/sync/{id}", get(sync::get_relationship).patch(sync::update_relationship).delete(sync::delete_relationship))
    .route("/sync/{id}/enable", post(sync::enable_relationship))
    .route("/sync/{id}/disable", post(sync::disable_relationship))
    .route("/sync/{id}/activity", get(sync::get_sync_activity))
    .route("/sync/{id}/trigger", post(sync::trigger_sync))
    .route("/sync/{id}/force-resync", post(sync::force_resync))
    .route("/sync/{id}/start", post(sync::start_sync))
    .route("/sync/{id}/stop", post(sync::stop_sync))
    .route("/sync/runner/status", get(sync::sync_runner_status))
    .route("/sync/pause-all", post(sync::pause_all_sync))
    .route("/sync/resume-all", post(sync::resume_all_sync))
    .route("/conflicts", get(conflicts::list_conflicts))
    .route("/conflicts/resolve", post(conflicts::resolve_conflict_handler))
    .route("/conflicts/dismiss", post(conflicts::dismiss_conflict_handler))
    .route("/conflicts/dismiss-all", post(conflicts::dismiss_all_conflicts))
    .route("/browse/{relationship_id}", get(files::browse))
    .route("/browse/{relationship_id}/{*path}", get(files::browse))
    .route("/browse/{relationship_id}/deleted", get(files::list_deleted))
    .route("/files/{relationship_id}/{*path}", get(files::serve_file).put(files::upload_file).delete(files::delete_file))
    .route("/files/{relationship_id}/open", post(files::open_locally))
    .route("/files/{relationship_id}/rename", post(files::rename_file))
    .route("/files/{relationship_id}/restore", post(files::restore_deleted))
    .route("/files/{relationship_id}/copy", post(files::copy_files))
    .route("/files/{relationship_id}/symlink", post(files::create_symlink))
    .route("/versions/{relationship_id}/history/{*path}", get(files::version_history))
    .route("/snapshots/{relationship_id}", get(files::list_snapshots).post(files::create_snapshot))
    .route("/snapshots/{relationship_id}/{snapshot_id}/restore", post(files::restore_from_snapshot))
    .route("/shares/{relationship_id}", get(shares::get_shares).post(shares::share).delete(shares::unshare))
    .route("/shares/{relationship_id}/users", get(shares::get_shareable_users))
    .route("/shares/{relationship_id}/groups", get(shares::get_shareable_groups))
    .route("/shares/{relationship_id}/link", post(shares::create_share_link))
    .route("/shares/{relationship_id}/links", get(shares::get_share_links))
    .route("/shares/{relationship_id}/links/{key_id}", delete(shares::revoke_share_link))
    .route("/settings", get(settings::get_settings).patch(settings::update_settings))
    .route("/preferences", get(preferences_routes::get_preferences).patch(preferences_routes::patch_preferences))
    .route("/open-folder", post(system::open_folder))
    .route("/pick-directory", post(system::pick_directory))
    .route("/shutdown", post(system::shutdown))
    .route("/events", get(events::event_stream));

  Router::new()
    .nest("/api/v1", api_routes)
    .with_state(state)
}

pub fn create_app_state(config: &ServerConfig) -> Result<AppState> {
  let data_path_str = config.data_path.to_string_lossy().to_string();
  let state_store   = StateStore::open_or_create(&data_path_str)?;
  let identity      = state_store.get_or_create_identity()?;

  tracing::info!("client identity: {} ({})", identity.id, identity.name);

  let config_store = ConfigStore::load(&config.config_path)?;

  let http_client = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(30))
    .build()
    .map_err(|error| ClientError::Server(format!("failed to create HTTP client: {}", error)))?;

  let (event_tx, _) = broadcast::channel(256);

  let state_store  = Arc::new(state_store);
  let config_store = Arc::new(config_store);
  let sync_runner  = SyncRunner::new(state_store.clone(), config_store.clone(), http_client.clone(), event_tx.clone());

  let config_dir = config.config_path.parent()
    .unwrap_or_else(|| std::path::Path::new("."))
    .to_path_buf();
  let data_dir = config.data_path.parent()
    .unwrap_or_else(|| std::path::Path::new("."))
    .to_path_buf();

  let health_map = new_health_map();
  let preferences = Arc::new(PreferencesStore::load(&config_dir)?);

  // Autostart desired-state lives in AppState so the settings PATCH
  // handler can publish updates. Bool starts false; main.rs seeds it
  // from the persisted config once a runtime is available, then ticks
  // the signal to force the listener to reconcile against the OS.
  let autostart_enabled = Arc::new(AtomicBool::new(false));
  let autostart_signal  = Arc::new(Notify::new());

  Ok(AppState {
    started_at:      Instant::now(),
    state_store,
    config_store,
    sync_runner,
    http_client,
    shutdown_signal: None,
    event_tx,
    config_dir,
    data_dir,
    health_map,
    preferences,
    autostart_enabled,
    autostart_signal,
  })
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
  let state    = create_app_state(&config)?;
  let _health_handle = start_health_pinger(
    state.config_store.clone(),
    state.event_tx.clone(),
    state.health_map.clone(),
  );
  let router   = build_router(state);
  let address  = format!("{}:{}", config.host, config.port);
  let listener = TcpListener::bind(&address).await.map_err(|error| {
    ClientError::Server(format!("failed to bind to {}: {}", address, error))
  })?;

  tracing::info!("aeordb-client listening on {}", address);

  axum::serve(listener, router).await.map_err(|error| {
    ClientError::Server(format!("server error: {}", error))
  })?;

  Ok(())
}

/// Start the server and return the bound address. Useful for tests
/// where we need to bind to port 0 and discover the assigned port.
pub async fn start_server_with_handle(
  config: ServerConfig,
) -> Result<(SocketAddr, tokio::task::JoinHandle<Result<()>>)> {
  let state    = create_app_state(&config)?;
  let _health_handle = start_health_pinger(
    state.config_store.clone(),
    state.event_tx.clone(),
    state.health_map.clone(),
  );
  let router   = build_router(state);
  let address  = format!("{}:{}", config.host, config.port);
  let listener = TcpListener::bind(&address).await.map_err(|error| {
    ClientError::Server(format!("failed to bind to {}: {}", address, error))
  })?;

  let bound_address = listener.local_addr().map_err(|error| {
    ClientError::Server(format!("failed to get local address: {}", error))
  })?;

  tracing::info!("aeordb-client listening on {}", bound_address);

  let handle = tokio::spawn(async move {
    axum::serve(listener, router).await.map_err(|error| {
      ClientError::Server(format!("server error: {}", error))
    })?;

    Ok(())
  });

  Ok((bound_address, handle))
}
