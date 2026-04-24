use std::path::PathBuf;
use std::fs::File;
use fs2::FileExt;

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

      // Singleton: acquire an exclusive file lock to prevent multiple instances.
      // The lock is held for the lifetime of the process — released on exit.
      let lock_path = data_path.parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("aeordb-client.lock");
      if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
      }
      let lock_file = File::create(&lock_path)
        .map_err(|e| anyhow::anyhow!("failed to create lock file: {}", e))?;
      if lock_file.try_lock_exclusive().is_err() {
        // Another instance is running — ask it to shut down and take over.
        eprintln!("aeordb-client is already running — requesting shutdown for takeover...");

        // Try graceful shutdown via API (may fail if the instance is unresponsive)
        let shutdown_url = format!("http://{}:{}/api/v1/shutdown", bind, port);
        let api_responded = reqwest::blocking::Client::builder()
          .timeout(std::time::Duration::from_secs(3))
          .build()
          .ok()
          .and_then(|client| {
            client.post(&shutdown_url)
              .header("Content-Type", "application/json")
              .body("{}")
              .send()
              .ok()
          })
          .is_some();

        if !api_responded {
          eprintln!("API unresponsive — finding and killing the old process...");
        }

        // Wait for the lock to be released (up to 5 seconds for graceful shutdown)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut acquired = false;
        while std::time::Instant::now() < deadline {
          if lock_file.try_lock_exclusive().is_ok() {
            acquired = true;
            break;
          }
          std::thread::sleep(std::time::Duration::from_millis(200));
        }

        if !acquired {
          // Graceful shutdown failed — forcibly kill the process holding the lock.
          // On Linux/macOS, we can find the PID from the lock file.
          eprintln!("graceful shutdown failed — force-killing old instance...");

          #[cfg(unix)]
          {
            use std::process::Command;
            // Use fuser to find who holds the lock
            if let Ok(output) = Command::new("fuser").arg(&lock_path).output() {
              let pids = String::from_utf8_lossy(&output.stdout);
              for pid_str in pids.split_whitespace() {
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                  unsafe { libc::kill(pid, libc::SIGTERM); }
                  eprintln!("sent SIGTERM to PID {}", pid);
                }
              }
            }
          }

          // Wait a bit more for SIGTERM to take effect
          let kill_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
          while std::time::Instant::now() < kill_deadline {
            if lock_file.try_lock_exclusive().is_ok() {
              acquired = true;
              break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
          }
        }

        if acquired {
          eprintln!("takeover complete — starting new instance.");
        } else {
          eprintln!("error: could not acquire lock. Kill the old instance manually.");
          std::process::exit(1);
        }
      }
      // Keep lock_file alive for the process lifetime — released on exit.
      let _lock_guard = lock_file;

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

      let sync_runner   = state.sync_runner.clone();
      let sync_runner_shutdown = sync_runner.clone();
      let sync_runner_post_tauri = sync_runner.clone();
      let api_router    = build_router(state);
      let static_router = static_files::static_routes();
      let app           = api_router.merge(static_router);

      // Create the tokio runtime manually -- Tauri must own the main thread
      let runtime = tokio::runtime::Runtime::new()?;

      // Start continuous sync for all enabled relationships
      runtime.spawn(async move {
        sync_runner.start_all_enabled().await;
      });

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
        // Block the main thread until shutdown signal, then stop all sync runners
        runtime.block_on(async {
          shutdown_signal().await;
          tracing::info!("stopping all sync runners...");
          sync_runner_shutdown.stop_all().await;
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

            let window_for_open      = window.clone();
            let app_handle_for_quit  = app.handle().clone();
            let sync_runner_for_quit = sync_runner_shutdown.clone();
            let api_base             = format!("http://{}", bound_addr);
            let paused              = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

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

            let paused_clone = paused.clone();

            TrayIconBuilder::new()
              .icon(icon)
              .tooltip("AeorDB Client")
              .menu(&tray_menu)
              .on_menu_event(move |_app, event| {
                match event.id().as_ref() {
                  "open" => {
                    let _ = window_for_open.show();
                    let _ = window_for_open.set_focus();
                  }
                  "pause" => {
                    let is_paused = paused_clone.load(std::sync::atomic::Ordering::Relaxed);
                    let endpoint = if is_paused {
                      format!("{}/api/v1/sync/resume-all", api_base)
                    } else {
                      format!("{}/api/v1/sync/pause-all", api_base)
                    };

                    match reqwest::blocking::Client::new().post(&endpoint).send() {
                      Ok(_) => {
                        let new_paused = !is_paused;
                        paused_clone.store(new_paused, std::sync::atomic::Ordering::Relaxed);

                        let new_text = if new_paused { "Resume All Sync" } else { "Pause All Sync" };
                        let _ = pause_item.set_text(new_text);

                        tracing::info!("sync {}", if new_paused { "paused" } else { "resumed" });
                      }
                      Err(error) => {
                        tracing::error!("failed to toggle sync: {}", error);
                      }
                    }
                  }
                  "quit" => {
                    tracing::info!("quit requested from tray — shutting down gracefully");
                    let runner = sync_runner_for_quit.clone();
                    let handle = app_handle_for_quit.clone();
                    std::thread::spawn(move || {
                      let rt = tokio::runtime::Runtime::new().unwrap();
                      rt.block_on(async { runner.stop_all().await });
                      handle.exit(0);
                    });
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

        // Tauri exited — stop all sync runners before runtime drops
        runtime.block_on(async {
          tracing::info!("stopping all sync runners...");
          sync_runner_post_tauri.stop_all().await;
        });
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
