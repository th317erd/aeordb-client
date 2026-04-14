use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use aeordb_client_lib::server::{ServerConfig, start_server};

mod cli;

#[derive(Parser)]
#[command(name = "aeordb-client")]
#[command(about = "AeorDB Client — sync-first client for AeorDB")]
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

    /// Path to local state database
    #[arg(long, env = "AEORDB_CLIENT_DB")]
    database: Option<String>,
  },

  /// Show status of the running instance
  Status,

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

fn default_database_path() -> String {
  let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
  format!("{}/.aeordb-client/state.aeordb", home)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let cli = Cli::parse();

  match cli.command {
    None | Some(Commands::Start { .. }) => {
      // Server mode — initialize logging
      tracing_subscriber::fmt()
        .with_env_filter(
          EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

      let (headless, bind, port, database) = match cli.command {
        Some(Commands::Start { headless, bind, port, database }) => {
          (headless, bind, port, database)
        }
        _ => (false, "127.0.0.1".to_string(), 9400, None),
      };

      if headless {
        tracing::info!("starting in headless mode");
      }

      let database_path = database.unwrap_or_else(default_database_path);

      let config = ServerConfig {
        host: bind,
        port,
        database_path,
      };

      start_server(config).await?;
    }

    Some(Commands::Status) => {
      cli::status::run(&cli.host, cli.json).await?;
    }

    Some(Commands::Connections { action }) => {
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
    }

    Some(Commands::Sync { action }) => {
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
    }
  }

  Ok(())
}
