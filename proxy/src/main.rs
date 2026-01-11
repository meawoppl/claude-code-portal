mod auth;
mod commands;
mod config;
mod session;
mod ui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use claude_codes::AsyncClient;
use config::{ProxyConfig, SessionAuth};
use session::SessionConfig;
use tokio::io::AsyncBufReadExt;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(name = "claude-proxy")]
#[command(about = "Wrapper for Claude CLI that proxies sessions to web interface")]
struct Args {
    /// Initialize with a token URL from the web interface
    /// Format: https://server.com/p/{base64_config} or just the JWT token
    #[arg(long)]
    init: Option<String>,

    /// Backend server URL (overrides URL from --init if provided)
    #[arg(long)]
    backend_url: Option<String>,

    /// Session authentication token (skips OAuth if provided)
    #[arg(long)]
    auth_token: Option<String>,

    /// Session name (auto-generated if not provided)
    #[arg(long)]
    session_name: Option<String>,

    /// Force starting a new session instead of resuming
    #[arg(long)]
    new_session: bool,

    /// Force re-authentication even if cached
    #[arg(long)]
    reauth: bool,

    /// Logout (remove cached authentication for this directory)
    #[arg(long)]
    logout: bool,

    /// Development mode - skip authentication entirely
    #[arg(long)]
    dev: bool,

    /// All remaining arguments to forward to claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    claude_args: Vec<String>,
}

fn default_session_name() -> String {
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("{}-{}", hostname, timestamp)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    dotenvy::dotenv().ok();

    let args = Args::parse();
    let cwd = std::env::current_dir()
        .context("Failed to get current directory")?
        .to_string_lossy()
        .to_string();

    let mut config = ProxyConfig::load().context("Failed to load config file")?;

    // Handle subcommands that exit early
    if args.logout {
        return commands::handle_logout(&mut config, &cwd);
    }

    if let Some(ref init_value) = args.init {
        return commands::handle_init(&mut config, &cwd, init_value, args.backend_url.as_deref());
    }

    // Resolve session (new or resume)
    let (session_id, session_name, resuming) = resolve_session(&args, &cwd)?;

    // Resolve backend URL: CLI arg > config (required, no default)
    let backend_url = args.backend_url.clone()
        .or_else(|| config.get_backend_url(&cwd).map(|s| s.to_string()))
        .ok_or_else(|| anyhow::anyhow!(
            "No backend URL configured. Run with --init <URL> first, or specify --backend-url explicitly."
        ))?;

    // Print startup info
    ui::print_startup_banner();
    ui::print_session_info(&session_name, &session_id.to_string(), &backend_url, resuming);

    // Resolve auth token
    let auth_token = resolve_auth_token(&args, &mut config, &cwd, &backend_url).await?;

    // Build session config
    let session_config = SessionConfig {
        backend_url,
        session_id,
        session_name,
        auth_token,
        working_directory: cwd,
        resuming,
    };

    // Start Claude and run session
    run_proxy_session(session_config).await
}

/// Resolve which session to use (new or resume existing)
fn resolve_session(args: &Args, cwd: &str) -> Result<(Uuid, String, bool)> {
    let (mut config, lock) = ProxyConfig::load_locked()
        .context("Failed to load config with lock")?;

    let existing_session = config.get_directory_session(cwd).cloned();
    let had_existing = existing_session.is_some();

    if args.new_session || existing_session.is_none() {
        // Start a new session
        let session_id = Uuid::new_v4();
        let session_name = args.session_name.clone().unwrap_or_else(default_session_name);

        let dir_session = ProxyConfig::create_directory_session(session_id, session_name.clone());
        config.set_directory_session(cwd.to_string(), dir_session);
        config.save_with_lock(&lock)?;

        if args.new_session && had_existing {
            warn!("Starting new session (--new-session flag) - previous session will not be resumed");
            ui::print_new_session_forced();
        } else if !had_existing {
            info!("No existing session for directory {}, creating new session {}", cwd, session_id);
            ui::print_no_previous_session();
        }

        info!("New session ID: {}", session_id);
        Ok((session_id, session_name, false))
    } else {
        // Resume existing session
        let existing = existing_session.unwrap();
        let session_name = args.session_name.clone()
            .unwrap_or_else(|| existing.session_name.clone());

        config.touch_directory_session(cwd);
        config.save_with_lock(&lock)?;

        info!(
            "Resuming session {} (created: {}, last used: {})",
            existing.session_id, existing.created_at, existing.last_used
        );
        ui::print_resuming_session(&existing.session_id.to_string(), &existing.created_at);

        Ok((existing.session_id, session_name, true))
    }
}

/// Resolve the authentication token
async fn resolve_auth_token(
    args: &Args,
    config: &mut ProxyConfig,
    cwd: &str,
    backend_url: &str,
) -> Result<Option<String>> {
    if args.dev {
        ui::print_dev_mode();
        return Ok(None);
    }

    if let Some(ref token) = args.auth_token {
        return Ok(Some(token.clone()));
    }

    if !args.reauth {
        if let Some(session_auth) = config.get_session_auth(cwd) {
            ui::print_user(
                session_auth.user_email.as_deref().unwrap_or("unknown user")
            );
            return Ok(Some(session_auth.auth_token.clone()));
        }
    }

    // Need to authenticate
    info!("Authenticating via device flow");
    let (token, user_id, user_email) = auth::device_flow_login(backend_url).await?;

    config.set_session_auth(
        cwd.to_string(),
        SessionAuth {
            user_id,
            auth_token: token.clone(),
            user_email: Some(user_email),
            last_used: chrono::Utc::now().to_rfc3339(),
            backend_url: None,
            session_prefix: None,
        },
    );
    config.atomic_save()?;

    Ok(Some(token))
}

/// Start Claude and run the proxy session
async fn run_proxy_session(config: SessionConfig) -> Result<()> {
    ui::print_status("Starting Claude CLI...");

    let mut claude_client = create_claude_client(&config).await?;

    ui::print_started();

    // Log stderr in background
    if let Some(mut stderr) = claude_client.take_stderr() {
        tokio::spawn(async move {
            let mut line = String::new();
            while let Ok(n) = stderr.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                warn!("Claude stderr: {}", line.trim());
                line.clear();
            }
        });
    }

    // Create input channel (shared across reconnections)
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Run the connection loop
    let result = session::run_connection_loop(&config, &mut claude_client, input_tx, &mut input_rx).await;

    info!("Proxy shutting down");
    let _ = claude_client.shutdown().await;

    result
}

/// Create the Claude async client
async fn create_claude_client(config: &SessionConfig) -> Result<AsyncClient> {
    let base_args = [
        "--print",
        "--verbose",
        "--output-format", "stream-json",
        "--input-format", "stream-json",
        "--replay-user-messages",
        "--permission-prompt-tool", "stdio",
    ];

    let child = if config.resuming {
        info!("Using --resume {} to resume Claude session", config.session_id);

        tokio::process::Command::new("claude")
            .args(base_args)
            .args(["--resume", &config.session_id.to_string()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn Claude process for resume")?
    } else {
        info!("Starting fresh Claude session with ID {}", config.session_id);

        tokio::process::Command::new("claude")
            .args(base_args)
            .args(["--session-id", &config.session_id.to_string()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn Claude process")?
    };

    AsyncClient::new(child)
        .map_err(|e| anyhow::anyhow!("Failed to create AsyncClient: {}", e))
}
