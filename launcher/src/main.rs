mod config;
mod connection;
mod process_manager;

use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "claude-portal-launcher")]
#[command(about = "Persistent daemon that launches claude-portal sessions as in-process tasks")]
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

    /// Maximum concurrent sessions
    #[arg(long)]
    max_sessions: Option<usize>,

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

    let auth_token = match args.auth_token.or(config.auth_token) {
        Some(token) => Some(token),
        None if args.dev => None,
        None => {
            tracing::info!("No auth token found, starting device flow authentication");
            let result = portal_auth::device_flow_login(&backend_url, None).await?;
            if let Err(e) = config::save_auth_token(&result.access_token) {
                tracing::warn!("Failed to save auth token to config: {}", e);
            }
            Some(result.access_token)
        }
    };
    let max_sessions = args.max_sessions.or(config.max_processes).unwrap_or(5);

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
    tracing::info!("Max sessions: {}", max_sessions);

    let (process_manager, exit_rx) =
        process_manager::ProcessManager::new(backend_url.clone(), max_sessions);

    connection::run_launcher_loop(
        &backend_url,
        launcher_id,
        &launcher_name,
        auth_token.as_deref(),
        process_manager,
        exit_rx,
    )
    .await
}
