#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

use fs2::FileExt;
use std::fs::File;
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

    /// Launch with the main window hidden, tray icon visible. Used by
    /// the autostart plugin so logging in doesn't pop a window on top
    /// of whatever you were doing. The user opens the window via the
    /// tray icon. No-op when --headless is also set.
    #[arg(long)]
    start_minimized: bool,

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
          EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

      // Self-update bootstrap. Both calls are no-ops in the common case:
      //   - cleanup_after_relaunch is Windows-only; deletes <exe>.old if
      //     the previous run's relauncher left one behind.
      //   - ingest_test_public_key only fires when AEORDB_TEST_PUBLIC_KEY
      //     is set (loopback tests against a local update server).
      //     Stays unset in production.
      aeordb_client_lib::update::cleanup_after_relaunch();
      aeordb_client_lib::update::ingest_test_public_key();

      let (headless, start_minimized, bind, port, config_path, data_path) = match cli.command {
        Some(Commands::Start {
          headless,
          start_minimized,
          bind,
          port,
          config,
          database,
        }) => (
          headless,
          start_minimized,
          bind,
          port,
          config.unwrap_or_else(default_config_path),
          database.unwrap_or_else(default_data_path),
        ),
        _ => (
          false,
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
      let lock_path = data_path
        .parent()
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
            client
              .post(&shutdown_url)
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
                  unsafe {
                    libc::kill(pid, libc::SIGTERM);
                  }
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
        host: bind.clone(),
        port,
        config_path,
        data_path,
      };

      let mut state = create_app_state(&server_config)
        .map_err(|error| anyhow::anyhow!("failed to initialize: {}", error))?;

      // Wire up the API-triggered shutdown signal
      let api_shutdown = std::sync::Arc::new(tokio::sync::Notify::new());
      state.shutdown_signal = Some(api_shutdown.clone());
      let api_shutdown_for_tauri = api_shutdown.clone();

      let sync_runner = state.sync_runner.clone();
      let sync_runner_shutdown = sync_runner.clone();
      let sync_runner_post_tauri = sync_runner.clone();
      let state_store_shutdown = state.state_store.clone();
      let state_store_maintenance = state_store_shutdown.clone();
      let state_store_post_tauri = state_store_shutdown.clone();
      let state_store_tauri_exit = state_store_shutdown.clone();
      let url_upgrade_config_store = state.config_store.clone();
      let url_upgrade_jwt_cache = state.jwt_cache.clone();
      let health_config_store = state.config_store.clone();
      let health_event_tx = state.event_tx.clone();
      let health_map_handle = state.health_map.clone();
      // Self-update startup poll handle — populated on the runtime
      // below so the About page has a snapshot to render immediately.
      let update_info_for_poll = state.update_info.clone();
      // Autostart plumbing — the listener thread (spawned after Tauri
      // is set up) owns the plugin handle and reconciles the desired
      // state against the OS.
      let autostart_enabled = state.autostart_enabled.clone();
      let autostart_signal = state.autostart_signal.clone();
      let autostart_config_store = state.config_store.clone();
      let api_router = build_router(state);
      let static_router = static_files::static_routes();
      let app = api_router.merge(static_router);

      // Create the tokio runtime manually -- Tauri must own the main thread
      let runtime = tokio::runtime::Runtime::new()?;
      let runtime_handle = runtime.handle().clone();

      // Normalize saved engine URLs before starting sync. The library
      // HTTP-server entrypoint does this in start_server(); the desktop
      // path builds AppState directly, so it must run the same probe here
      // or http→https reverse-proxy redirects keep every sync on port 80.
      runtime.spawn(async move {
        aeordb_client_lib::connections::probe_and_upgrade_connection_urls(
          url_upgrade_config_store,
          url_upgrade_jwt_cache,
        )
        .await;
        sync_runner.start_all_enabled_if_configured().await;
      });

      // Start the connection health pinger so UI can react when an
      // engine that was unreachable at boot comes online (auto-refresh
      // file-browser tabs stuck on "Cannot reach the server"). The
      // function does its own tokio::spawn internally; we just need to
      // be inside the runtime when we call it. The returned JoinHandle
      // is dropped — the detached task runs for the process lifetime.
      runtime.spawn(async move {
        let _ = aeordb_client_lib::health::start_health_pinger(
          health_config_store,
          health_event_tx,
          health_map_handle,
        );
      });

      // Fire-and-forget self-update check on startup. Tolerates network
      // down / 503 / 404; the result lands in `update_info` and is
      // served by GET /api/v1/update/status. The About page polls that
      // endpoint on mount, so a slow first poll just shows
      // "You're up to date" until the response lands a moment later.
      runtime.spawn(async move {
        let client = reqwest::Client::new();
        aeordb_client_lib::update::check_once(&client, &update_info_for_poll).await;
      });

      // Start HTTP server on the runtime, signal readiness via channel
      let (ready_tx, ready_rx) = std::sync::mpsc::channel();
      let address = format!("{}:{}", bind, port);

      runtime.spawn(async move {
        state_store_maintenance.start_maintenance_tasks();
      });

      runtime.spawn(async move {
        let listener = match tokio::net::TcpListener::bind(&address).await {
          Ok(l) => l,
          Err(error) => {
            tracing::error!("failed to bind to {}: {}", address, error);
            let _ = ready_tx.send(Err(anyhow::anyhow!(
              "failed to bind to {}: {}",
              address,
              error
            )));
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
        let api_shutdown_for_headless = api_shutdown_for_tauri.clone();
        runtime.block_on(async {
          tokio::select! {
            _ = shutdown_signal() => {}
            _ = api_shutdown_for_headless.notified() => {
              tracing::info!("API shutdown received — exiting headless mode");
            }
          }
          tracing::info!("stopping all sync runners...");
          sync_runner_shutdown.stop_all().await;
          tracing::info!("shutting down local state database...");
          if let Err(error) = state_store_shutdown.shutdown() {
            tracing::error!("state database shutdown failed: {}", error);
          }
        });
      } else {
        // Run Tauri on the main thread -- webview loads from our HTTP server
        let url = format!("http://{}", bound_addr);

        tauri::Builder::default()
          .plugin(tauri_plugin_shell::init())
          // tauri-plugin-autostart writes the platform-appropriate
          // autostart entry (XDG .desktop on Linux, registry Run key on
          // Windows, launchd on macOS). args=["start","--start-minimized"]
          // means: when the OS launches us at login, run with the window
          // hidden and the tray icon visible — clicking the tray opens
          // the window. No --headless: we want the tray icon so the user
          // can tell the daemon is alive.
          .plugin(
            tauri_plugin_autostart::Builder::new()
              .app_name("aeordb-client")
              .args(vec!["start", "--start-minimized"])
              .build(),
          )
          .invoke_handler(tauri::generate_handler![open_external_url])
          .setup(move |app| {
            use tauri::Manager;
            use tauri::image::Image;
            use tauri::menu::{MenuBuilder, MenuItemBuilder};
            use tauri::tray::TrayIconBuilder;

            // Bridge OS signals (SIGTERM/SIGINT) and the /api/v1/shutdown
            // request into Tauri's event loop. Without this, the HTTP
            // server shuts down but Tauri's main thread keeps the
            // process alive — dev-watch then can't restart cleanly.
            let exit_handle = app.handle().clone();
            let api_shutdown_for_exit = api_shutdown_for_tauri.clone();
            let sync_runner_for_exit = sync_runner_shutdown.clone();
            let state_store_for_exit = state_store_tauri_exit.clone();
            runtime_handle.spawn(async move {
              tokio::select! {
                _ = shutdown_signal() => {
                  tracing::info!("OS signal received — exiting Tauri");
                }
                _ = api_shutdown_for_exit.notified() => {
                  tracing::info!("API shutdown received — exiting Tauri");
                }
              }
              tracing::info!("stopping all sync runners...");
              sync_runner_for_exit.stop_all().await;
              tracing::info!("shutting down local state database...");
              if let Err(error) = state_store_for_exit.shutdown() {
                tracing::error!("state database shutdown failed: {}", error);
              }
              exit_handle.exit(0);
            });

            // --- Create the main window ---
            // start_minimized → build the window hidden. The tray icon
            // is still added below, so the user sees that the app
            // launched and can click it to open the window. Used by
            // the autostart path so logging in doesn't punch a window
            // through whatever the user was looking at.
            let parsed_url: tauri::Url = url.parse().expect("valid localhost URL");
            let window_builder = tauri::WebviewWindowBuilder::new(
              app,
              "main",
              tauri::WebviewUrl::External(parsed_url),
            )
            .title("AeorDB Client")
            .inner_size(1200.0, 850.0)
            .min_inner_size(900.0, 650.0)
            .visible(!start_minimized);

            #[cfg(not(debug_assertions))]
            let window_builder = window_builder.initialization_script(
              "window.addEventListener('contextmenu', event => event.preventDefault(), { capture: true });",
            );

            let window = window_builder.build()?;

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

            let window_for_open = window.clone();
            let app_handle_for_quit = app.handle().clone();
            let sync_runner_for_quit = sync_runner_shutdown.clone();
            let state_store_for_quit = state_store_tauri_exit.clone();
            let api_base = format!("http://{}", bound_addr);
            let paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

            let open_item = MenuItemBuilder::with_id("open", "Open AeorDB Client").build(app)?;
            let pause_item = MenuItemBuilder::with_id("pause", "Pause All Sync").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

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
              .on_menu_event(move |_app, event| match event.id().as_ref() {
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

                      let new_text = if new_paused {
                        "Resume All Sync"
                      } else {
                        "Pause All Sync"
                      };
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
                  let state_store = state_store_for_quit.clone();
                  let handle = app_handle_for_quit.clone();
                  std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                      runner.stop_all().await;
                      tracing::info!("shutting down local state database...");
                      if let Err(error) = state_store.shutdown() {
                        tracing::error!("state database shutdown failed: {}", error);
                      }
                    });
                    handle.exit(0);
                  });
                }
                _ => {}
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

            // --- Autostart listener + boot-time reconciliation ---
            // Owns the plugin handle. On startup, loads the persisted
            // setting and reconciles against the OS (covers the case
            // where the user toggled autostart on, then manually
            // deleted the .desktop file). After that, blocks on the
            // signal Notify; the settings PATCH handler ticks it
            // whenever the user toggles the checkbox.
            use tauri_plugin_autostart::ManagerExt;
            let as_handle = app.handle().clone();
            let as_enabled = autostart_enabled.clone();
            let as_signal = autostart_signal.clone();
            let as_config = autostart_config_store.clone();
            std::thread::spawn(move || {
              let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build autostart runtime");
              rt.block_on(async move {
                // Seed the desired-state atomic from the persisted
                // config now that we have a runtime. AppState's bool
                // started at false because create_app_state is sync.
                if let Ok(config) = as_config.get().await {
                  as_enabled.store(
                    config.settings.auto_start_system,
                    std::sync::atomic::Ordering::SeqCst,
                  );
                }

                // Initial reconciliation. We always call enable() when
                // the setting is on, even if is_enabled() already says
                // true — this overwrites any stale hand-rolled .desktop
                // file from the pre-plugin era (which used --headless
                // and never properly started the tray/UI). The plugin's
                // enable() is idempotent so re-running it on a
                // correctly-installed entry is a no-op rewrite.
                let desired = as_enabled.load(std::sync::atomic::Ordering::SeqCst);
                let mgr = as_handle.autolaunch();
                let result = if desired { mgr.enable() } else { mgr.disable() };
                match result {
                  Ok(_) => tracing::info!("autostart reconciled at startup: enabled={}", desired),
                  Err(error) => {
                    tracing::warn!("failed to reconcile autostart at startup: {}", error)
                  }
                }

                // Listen for further changes.
                loop {
                  as_signal.notified().await;
                  let desired = as_enabled.load(std::sync::atomic::Ordering::SeqCst);
                  let mgr = as_handle.autolaunch();
                  let result = if desired { mgr.enable() } else { mgr.disable() };
                  match result {
                    Ok(_) => tracing::info!("autostart applied: enabled={}", desired),
                    Err(error) => tracing::warn!("failed to apply autostart toggle: {}", error),
                  }
                }
              });
            });

            Ok(())
          })
          .run(tauri::generate_context!())
          .expect("error while running tauri application");

        // Tauri exited — stop all sync runners before runtime drops
        runtime.block_on(async {
          tracing::info!("stopping all sync runners...");
          sync_runner_post_tauri.stop_all().await;
          tracing::info!("shutting down local state database...");
          if let Err(error) = state_store_post_tauri.shutdown() {
            tracing::error!("state database shutdown failed: {}", error);
          }
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
          SyncAction::Add {
            name,
            connection,
            remote_path,
            local_path,
            direction,
            filter,
          } => {
            cli::sync::add(
              &cli.host,
              cli.json,
              &name,
              &connection,
              &remote_path,
              &local_path,
              &direction,
              filter.as_deref(),
            )
            .await?;
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

/// Tauri command — shell-open an http(s) URL in the user's default browser.
/// Required because Tauri's webview can't navigate to external URLs on its
/// own (no browser-tab context to spawn into), so footer links etc. would
/// no-op in the desktop app. Plain-browser previews fall through to the
/// anchor's default navigation; the JS only routes through here when
/// `window.__TAURI_INTERNALS__` is present.
///
/// The http/https allowlist is deliberate: we don't need a general-purpose
/// URL launcher and the narrow gate forecloses any future code path from
/// shell-opening unexpected URIs (file://, javascript:, etc).
#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
  // Scope: http(s) URLs + mailto: links. Anything else (file://,
  // javascript:, ssh://, custom schemes) is rejected — we don't need a
  // general-purpose URL launcher and the narrow allowlist forecloses
  // any future code path from shell-opening unexpected URIs. mailto:
  // is allowed because the About-page email link routes through here
  // and the WebView itself has no mail handler — without this branch
  // the click would render "URL can't be shown" and blow away the app.
  let scheme_ok =
    url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:");
  if !scheme_ok {
    return Err(format!(
      "only http(s) and mailto: URLs are allowed; got: {}",
      url
    ));
  }
  let result = if cfg!(target_os = "linux") {
    std::process::Command::new("xdg-open").arg(&url).spawn()
  } else if cfg!(target_os = "macos") {
    std::process::Command::new("open").arg(&url).spawn()
  } else if cfg!(target_os = "windows") {
    // `start` is a cmd builtin — must go via cmd.exe.
    std::process::Command::new("cmd")
      .args(["/C", "start", "", &url])
      .spawn()
  } else {
    return Err("unsupported platform".to_string());
  };
  result
    .map(|_| ())
    .map_err(|error| format!("spawn failed: {}", error))
}
