mod config;
mod connection;
mod process_manager;

use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "claude-portal-launcher")]
#[command(about = "Persistent daemon that launches claude-portal instances on demand")]
struct Args {
    /// Backend WebSocket URL (e.g., ws://localhost:3000)
    #[arg(long)]
    backend_url: Option<String>,

    /// JWT auth token for the launcher
    #[arg(long, env = "LAUNCHER_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Human-readable name for this launcher (default: hostname)
    #[arg(long)]
    name: Option<String>,

    /// Path to the claude-portal binary
    #[arg(long)]
    proxy_path: Option<String>,

    /// Maximum concurrent proxy processes
    #[arg(long)]
    max_processes: Option<usize>,

    /// Development mode (no auth required)
    #[arg(long)]
    dev: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = config::load_config();

    // CLI args override config file values
    let backend_url = args
        .backend_url
        .or(config.backend_url)
        .ok_or_else(|| anyhow::anyhow!("--backend-url is required (or set in config file)"))?;

    let auth_token = args.auth_token.or(config.auth_token);
    let proxy_path = args
        .proxy_path
        .or(config.proxy_path)
        .unwrap_or_else(|| "claude-portal".to_string());
    let max_processes = args.max_processes.or(config.max_processes).unwrap_or(5);

    let launcher_name = args.name.or(config.name).unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    });

    let launcher_id = Uuid::new_v4();

    tracing::info!(
        "Starting launcher '{}' (id: {})",
        launcher_name,
        launcher_id
    );
    tracing::info!("Backend URL: {}", backend_url);
    tracing::info!("Proxy binary: {}", proxy_path);
    tracing::info!("Max processes: {}", max_processes);

    let (process_manager, log_rx) = process_manager::ProcessManager::new(
        proxy_path.into(),
        backend_url.clone(),
        max_processes,
        args.dev,
    );

    connection::run_launcher_loop(
        &backend_url,
        launcher_id,
        &launcher_name,
        auth_token.as_deref(),
        process_manager,
        log_rx,
    )
    .await
}
