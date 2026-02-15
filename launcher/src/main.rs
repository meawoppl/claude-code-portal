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
    backend_url: String,

    /// JWT auth token for the launcher
    #[arg(long, env = "LAUNCHER_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Human-readable name for this launcher (default: hostname)
    #[arg(long)]
    name: Option<String>,

    /// Path to the claude-portal binary
    #[arg(long, default_value = "claude-portal")]
    proxy_path: String,

    /// Maximum concurrent proxy processes
    #[arg(long, default_value = "5")]
    max_processes: usize,

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

    let launcher_name = args.name.unwrap_or_else(|| {
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
    tracing::info!("Backend URL: {}", args.backend_url);
    tracing::info!("Proxy binary: {}", args.proxy_path);
    tracing::info!("Max processes: {}", args.max_processes);

    let (process_manager, log_rx) = process_manager::ProcessManager::new(
        args.proxy_path.into(),
        args.backend_url.clone(),
        args.max_processes,
        args.dev,
    );

    connection::run_launcher_loop(
        &args.backend_url,
        launcher_id,
        &launcher_name,
        args.auth_token.as_deref(),
        process_manager,
        log_rx,
    )
    .await
}
