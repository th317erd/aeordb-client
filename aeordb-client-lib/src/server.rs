use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::api::routes::conflicts;
use crate::api::routes::connections;
use crate::api::routes::status::get_status;
use crate::api::routes::sync;
use crate::api::routes::system;
use crate::error::{ClientError, Result};
use crate::state::StateStore;
use crate::sync::runner::SyncRunner;

#[derive(Clone)]
pub struct AppState {
  pub started_at:      Instant,
  pub state_store:     Arc<StateStore>,
  pub sync_runner:     SyncRunner,
  pub auth_token:      Option<String>,
  pub shutdown_signal: Option<Arc<Notify>>,
}

pub struct ServerConfig {
  pub host:          String,
  pub port:          u16,
  pub database_path: String,
  pub auth_token:    Option<String>,
}

impl Default for ServerConfig {
  fn default() -> Self {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());

    Self {
      host:          "127.0.0.1".to_string(),
      port:          9400,
      database_path: format!("{}/.aeordb-client/state.aeordb", home),
      auth_token:    None,
    }
  }
}

pub fn build_router(state: AppState) -> Router {
  let api_routes = Router::new()
    .route("/status", get(get_status))
    .route("/connections", get(connections::list_connections).post(connections::create_connection))
    .route("/connections/{id}", get(connections::get_connection).patch(connections::update_connection).delete(connections::delete_connection))
    .route("/connections/{id}/test", post(connections::test_connection))
    .route("/sync", get(sync::list_relationships).post(sync::create_relationship))
    .route("/sync/{id}", get(sync::get_relationship).patch(sync::update_relationship).delete(sync::delete_relationship))
    .route("/sync/{id}/enable", post(sync::enable_relationship))
    .route("/sync/{id}/disable", post(sync::disable_relationship))
    .route("/sync/{id}/trigger", post(sync::trigger_sync))
    .route("/sync/{id}/start", post(sync::start_sync))
    .route("/sync/{id}/stop", post(sync::stop_sync))
    .route("/sync/runner/status", get(sync::sync_runner_status))
    .route("/conflicts", get(conflicts::list_conflicts))
    .route("/conflicts/{id}/resolve", post(conflicts::resolve_conflict))
    .route("/conflicts/resolve-all", post(conflicts::resolve_all_conflicts))
    .route("/shutdown", post(system::shutdown));

  Router::new()
    .nest("/api/v1", api_routes)
    .with_state(state)
}

pub fn create_app_state(database_path: &str) -> Result<AppState> {
  let state_store = StateStore::open_or_create(database_path)?;
  let identity    = state_store.get_or_create_identity()?;

  tracing::info!("client identity: {} ({})", identity.id, identity.name);

  let state_store = Arc::new(state_store);
  let sync_runner = SyncRunner::new(state_store.clone());

  Ok(AppState {
    started_at:      Instant::now(),
    state_store,
    sync_runner,
    auth_token:      None,
    shutdown_signal: None,
  })
}

pub fn create_app_state_with_auth(database_path: &str, auth_token: Option<String>) -> Result<AppState> {
  let mut state = create_app_state(database_path)?;
  state.auth_token = auth_token;
  Ok(state)
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
  let state    = create_app_state_with_auth(&config.database_path, config.auth_token)?;
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
  let state    = create_app_state_with_auth(&config.database_path, config.auth_token)?;
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
