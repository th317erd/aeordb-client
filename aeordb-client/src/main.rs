use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use aeordb_client_lib::config::{default_config_path, default_data_path};
use aeordb_client_lib::server::{ServerConfig, build_router, create_app_state};

mod cli;
mod static_files;

#[derive(Parser)]
#[command(name = "aeordb-client")]
#[command(about = "AeorDB Client -- sync-first client for AeorDB")]
#[command(version)]
struct Cli {
  /// Target instance URL (for subcommands that talk to a running instance)
  #[arg(long, global = true, default_value = "http://127.0.0.1:9400")]
  host: String,

  /// Output as JSON
  #[arg(long, global = true)]
  json: bool,

  #[command(subcommand)]
  command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
  /// Start the client (default if no subcommand given)
  Start {
    /// Run in headless mode (no UI, no systray)
    #[arg(long)]
    headless: bool,

    /// Host address to bind to
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    /// Port to listen on
    #[arg(short, long, default_value_t = 9400)]
    port: u16,

    /// Path to config YAML file
    #[arg(long, env = "AEORDB_CLIENT_CONFIG")]
    config: Option<PathBuf>,

    /// Path to local state database
    #[arg(long, env = "AEORDB_CLIENT_DB")]
    database: Option<PathBuf>,
  },

  /// Show status of the running instance
  Status,

  /// Stop the running instance
  Stop,

  /// Manage remote connections
  Connections {
    #[command(subcommand)]
    action: ConnectionAction,
  },

  /// Manage sync relationships
  Sync {
    #[command(subcommand)]
    action: SyncAction,
  },
}

#[derive(Subcommand)]
enum ConnectionAction {
  /// List all connections
  List,
  /// Add a new connection
  Add {
    /// Connection name
    #[arg(long)]
    name: String,
    /// Remote aeordb URL
    #[arg(long)]
    url: String,
    /// API key (optional)
    #[arg(long)]
    api_key: Option<String>,
  },
  /// Remove a connection
  Remove {
    /// Connection ID
    id: String,
  },
  /// Test connectivity
  Test {
    /// Connection ID
    id: String,
  },
}

#[derive(Subcommand)]
enum SyncAction {
  /// List all sync relationships
  List,
  /// Add a new sync relationship
  Add {
    /// Relationship name
    #[arg(long)]
    name: String,
    /// Connection ID
    #[arg(long)]
    connection: String,
    /// Remote directory path
    #[arg(long)]
    remote_path: String,
    /// Local directory path
    #[arg(long)]
    local_path: String,
    /// Sync direction: bidirectional, pull-only, push-only
    #[arg(long, default_value = "pull-only")]
    direction: String,
    /// File filter (glob pattern)
    #[arg(long)]
    filter: Option<String>,
  },
  /// Remove a sync relationship
  Remove {
    /// Relationship ID
    id: String,
  },
  /// Show sync status
  Status {
    /// Relationship ID (optional, shows all if omitted)
    id: Option<String>,
  },
  /// Trigger a full sync pass
  Trigger {
    /// Relationship ID
    id: String,
  },
  /// Pause sync (one or all)
  Pause {
    /// Relationship ID (optional, pauses all if omitted)
    id: Option<String>,
  },
  /// Resume sync (one or all)
  Resume {
    /// Relationship ID (optional, resumes all if omitted)
    id: Option<String>,
  },
}

fn main() -> anyhow::Result<()> {
  let cli = Cli::parse();

  match cli.command {
    None | Some(Commands::Start { .. }) => {
      // Server mode -- initialize logging
      tracing_subscriber::fmt()
        .with_env_filter(
          EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

      let (headless, bind, port, config_path, data_path) = match cli.command {
        Some(Commands::Start { headless, bind, port, config, database }) => {
          (
            headless,
            bind,
            port,
            config.unwrap_or_else(default_config_path),
            database.unwrap_or_else(default_data_path),
          )
        }
        _ => (
          false,
          "127.0.0.1".to_string(),
          9400,
          default_config_path(),
          default_data_path(),
        ),
      };

      if headless {
        tracing::info!("starting in headless mode");
      }

      tracing::info!("config: {}", config_path.display());
      tracing::info!("data:   {}", data_path.display());

      // Singleton check: if an instance is already running on this port, don't start another
      let check_url = format!("http://{}:{}/api/v1/status", bind, port);
      if let Ok(response) = reqwest::blocking::get(&check_url) {
        if response.status().is_success() {
          eprintln!("aeordb-client is already running on {}:{}", bind, port);
          eprintln!("Use 'aeordb-client status' to check it, or 'aeordb-client stop' to stop it.");
          std::process::exit(1);
        }
      }

      let server_config = ServerConfig {
        host:        bind.clone(),
        port,
        config_path,
        data_path,
      };

      let mut state = create_app_state(&server_config)
        .map_err(|error| anyhow::anyhow!("failed to initialize: {}", error))?;

      // Wire up the API-triggered shutdown signal
      let api_shutdown = std::sync::Arc::new(tokio::sync::Notify::new());
      state.shutdown_signal = Some(api_shutdown.clone());

      let api_router    = build_router(state);
      let static_router = static_files::static_routes();
      let app           = api_router.merge(static_router);

      // Create the tokio runtime manually -- Tauri must own the main thread
      let runtime = tokio::runtime::Runtime::new()?;

      // Start HTTP server on the runtime, signal readiness via channel
      let (ready_tx, ready_rx) = std::sync::mpsc::channel();
      let address = format!("{}:{}", bind, port);

      runtime.spawn(async move {
        let listener = match tokio::net::TcpListener::bind(&address).await {
          Ok(l) => l,
          Err(error) => {
            tracing::error!("failed to bind to {}: {}", address, error);
            let _ = ready_tx.send(Err(anyhow::anyhow!("failed to bind to {}: {}", address, error)));
            return;
          }
        };

        let bound_addr = listener.local_addr().expect("listener has local address");
        tracing::info!("aeordb-client listening on {}", bound_addr);
        tracing::info!("UI available at http://{}", bound_addr);

        let _ = ready_tx.send(Ok(bound_addr));

        // Shutdown on either OS signal or API request
        let shutdown_future = async move {
          tokio::select! {
            _ = shutdown_signal() => {}
            _ = api_shutdown.notified() => { tracing::info!("shutdown requested via API"); }
          }
        };

        if let Err(error) = axum::serve(listener, app)
          .with_graceful_shutdown(shutdown_future)
          .await
        {
          tracing::error!("server error: {}", error);
        }

        tracing::info!("aeordb-client shut down gracefully");
      });

      // Wait for the server to be ready (or fail to bind)
      let bound_addr = ready_rx.recv()??;

      if headless {
        // Block the main thread until shutdown signal
        runtime.block_on(async {
          shutdown_signal().await;
        });
      } else {
        // Run Tauri on the main thread -- webview loads from our HTTP server
        let url = format!("http://{}", bound_addr);

        tauri::Builder::default()
          .plugin(tauri_plugin_shell::init())
          .setup(move |app| {
            use tauri::Manager;
            use tauri::menu::{MenuBuilder, MenuItemBuilder};
            use tauri::tray::TrayIconBuilder;
            use tauri::image::Image;

            // --- Create the main window ---
            let parsed_url: tauri::Url = url.parse().expect("valid localhost URL");
            let window = tauri::WebviewWindowBuilder::new(
              app,
              "main",
              tauri::WebviewUrl::External(parsed_url),
            )
            .title("AeorDB Client")
            .inner_size(1200.0, 850.0)
            .min_inner_size(900.0, 650.0)
            .build()?;

            // --- Close-to-tray: hide window on close instead of quitting ---
            let window_for_close = window.clone();
            window.on_window_event(move |event| {
              if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window_for_close.hide();
              }
            });

            // --- Systray ---
            let icon_bytes = include_bytes!("../icons/icon.png");
            let icon = Image::from_bytes(icon_bytes)
              .unwrap_or_else(|_| Image::new(&[255, 255, 255, 255], 1, 1));

            let window_for_open    = window.clone();
            let app_handle_for_quit = app.handle().clone();

            let open_item  = MenuItemBuilder::with_id("open", "Open AeorDB Client").build(app)?;
            let pause_item = MenuItemBuilder::with_id("pause", "Pause All Sync").build(app)?;
            let quit_item  = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

            let tray_menu = MenuBuilder::new(app)
              .item(&open_item)
              .separator()
              .item(&pause_item)
              .separator()
              .item(&quit_item)
              .build()?;

            TrayIconBuilder::new()
              .icon(icon)
              .tooltip("AeorDB Client")
              .menu(&tray_menu)
              .on_menu_event(move |app, event| {
                match event.id().as_ref() {
                  "open" => {
                    let _ = window_for_open.show();
                    let _ = window_for_open.set_focus();
                  }
                  "pause" => {
                    // TODO: toggle pause/resume via API
                    tracing::info!("pause/resume sync requested from tray");
                  }
                  "quit" => {
                    tracing::info!("quit requested from tray");
                    app_handle_for_quit.exit(0);
                  }
                  _ => {}
                }
              })
              .on_tray_icon_event(move |tray, event| {
                if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                  if let Some(webview_window) = tray.app_handle().get_webview_window("main") {
                    let _ = webview_window.show();
                    let _ = webview_window.set_focus();
                  }
                }
              })
              .build(app)?;

            Ok(())
          })
          .run(tauri::generate_context!())
          .expect("error while running tauri application");
      }
    }

    Some(Commands::Status) => {
      let runtime = tokio::runtime::Runtime::new()?;
      runtime.block_on(cli::status::run(&cli.host, cli.json))?;
    }

    Some(Commands::Stop) => {
      let runtime = tokio::runtime::Runtime::new()?;
      runtime.block_on(async {
        match cli::api_post(&cli.host, "/api/v1/shutdown", &serde_json::json!({})).await {
          Ok(_) => println!("Shutdown initiated."),
          Err(error) => {
            eprintln!("Failed to stop instance: {}", error);
            std::process::exit(1);
          }
        }
      });
    }

    Some(Commands::Connections { action }) => {
      let runtime = tokio::runtime::Runtime::new()?;
      runtime.block_on(async {
        match action {
          ConnectionAction::List => {
            cli::connections::list(&cli.host, cli.json).await?;
          }
          ConnectionAction::Add { name, url, api_key } => {
            cli::connections::add(&cli.host, cli.json, &name, &url, api_key.as_deref()).await?;
          }
          ConnectionAction::Remove { id } => {
            cli::connections::remove(&cli.host, &id).await?;
          }
          ConnectionAction::Test { id } => {
            cli::connections::test(&cli.host, cli.json, &id).await?;
          }
        }
        Ok::<(), anyhow::Error>(())
      })?;
    }

    Some(Commands::Sync { action }) => {
      let runtime = tokio::runtime::Runtime::new()?;
      runtime.block_on(async {
        match action {
          SyncAction::List => {
            cli::sync::list(&cli.host, cli.json).await?;
          }
          SyncAction::Add { name, connection, remote_path, local_path, direction, filter } => {
            cli::sync::add(&cli.host, cli.json, &name, &connection, &remote_path, &local_path, &direction, filter.as_deref()).await?;
          }
          SyncAction::Remove { id } => {
            cli::sync::remove(&cli.host, &id).await?;
          }
          SyncAction::Status { id } => {
            cli::sync::status(&cli.host, cli.json, id.as_deref()).await?;
          }
          SyncAction::Trigger { id } => {
            cli::sync::trigger(&cli.host, cli.json, &id).await?;
          }
          SyncAction::Pause { id } => {
            cli::sync::pause(&cli.host, id.as_deref()).await?;
          }
          SyncAction::Resume { id } => {
            cli::sync::resume(&cli.host, id.as_deref()).await?;
          }
        }
        Ok::<(), anyhow::Error>(())
      })?;
    }
  }

  Ok(())
}

async fn shutdown_signal() {
  let ctrl_c = async {
    tokio::signal::ctrl_c()
      .await
      .expect("failed to install CTRL+C handler");
  };

  #[cfg(unix)]
  let terminate = async {
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
      .expect("failed to install SIGTERM handler")
      .recv()
      .await;
  };

  #[cfg(not(unix))]
  let terminate = std::future::pending::<()>();

  tokio::select! {
    _ = ctrl_c => { tracing::info!("received CTRL+C, shutting down..."); }
    _ = terminate => { tracing::info!("received SIGTERM, shutting down..."); }
  }
}
