use std::net::SocketAddr;
use std::time::Instant;

use axum::Router;
use axum::routing::get;
use tokio::net::TcpListener;

use crate::api::routes::status::get_status;
use crate::error::{ClientError, Result};

#[derive(Debug, Clone)]
pub struct AppState {
  pub started_at: Instant,
}

pub struct ServerConfig {
  pub host: String,
  pub port: u16,
}

impl Default for ServerConfig {
  fn default() -> Self {
    Self {
      host: "127.0.0.1".to_string(),
      port: 9400,
    }
  }
}

pub fn build_router(state: AppState) -> Router {
  let api_routes = Router::new()
    .route("/status", get(get_status));

  Router::new()
    .nest("/api/v1", api_routes)
    .with_state(state)
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
  let state = AppState {
    started_at: Instant::now(),
  };

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
  let state = AppState {
    started_at: Instant::now(),
  };

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
