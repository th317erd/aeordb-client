use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use aeordb_client_lib::server::{ServerConfig, start_server};

#[derive(Parser)]
#[command(name = "aeordb-client")]
#[command(about = "AeorDB Client — sync-first client for AeorDB")]
#[command(version)]
struct Cli {
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
    host: String,

    /// Port to listen on
    #[arg(short, long, default_value_t = 9400)]
    port: u16,
  },

  /// Show status of the running instance
  Status {
    /// Target instance URL
    #[arg(long, default_value = "http://127.0.0.1:9400")]
    host: String,

    /// Output as JSON
    #[arg(long)]
    json: bool,
  },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  tracing_subscriber::fmt()
    .with_env_filter(
      EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info")),
    )
    .init();

  let cli = Cli::parse();

  match cli.command {
    None | Some(Commands::Start { .. }) => {
      let (headless, host, port) = match cli.command {
        Some(Commands::Start { headless, host, port }) => (headless, host, port),
        _ => (false, "127.0.0.1".to_string(), 9400),
      };

      if headless {
        tracing::info!("starting in headless mode");
      }

      let config = ServerConfig { host, port };
      start_server(config).await?;
    }

    Some(Commands::Status { host, json }) => {
      match reqwest::get(format!("{}/api/v1/status", host)).await {
        Ok(response) => {
          if json {
            let body: serde_json::Value = response.json().await?;
            println!("{}", serde_json::to_string_pretty(&body)?);
          } else {
            let body: serde_json::Value = response.json().await?;
            println!(
              "Status:  {}\nVersion: {}\nUptime:  {}s",
              body["status"], body["version"], body["uptime"]
            );
          }
        }
        Err(error) => {
          eprintln!("Could not connect to aeordb-client at {}: {}", host, error);
          std::process::exit(1);
        }
      }
    }
  }

  Ok(())
}
