mod auth;
mod config;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use config::{ProxyConfig, SessionAuth};
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "claude-proxy")]
#[command(about = "Wrapper for Claude CLI that proxies sessions to web interface")]
struct Args {
    /// Backend server URL
    #[arg(long, default_value = "ws://localhost:3000")]
    backend_url: String,

    /// Session authentication token (skips OAuth if provided)
    #[arg(long)]
    auth_token: Option<String>,

    /// Session name
    #[arg(long, default_value_t = default_session_name())]
    session_name: String,

    /// Force re-authentication even if cached
    #[arg(long)]
    reauth: bool,

    /// Logout (remove cached authentication for this directory)
    #[arg(long)]
    logout: bool,

    /// All remaining arguments to forward to claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    claude_args: Vec<String>,
}

fn default_session_name() -> String {
    format!(
        "{}@{}",
        std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()),
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string())
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load environment variables
    dotenvy::dotenv().ok();

    // Parse arguments
    let args = Args::parse();

    // Get current working directory
    let cwd = std::env::current_dir()
        .context("Failed to get current directory")?
        .to_string_lossy()
        .to_string();

    // Load config
    let mut config = ProxyConfig::load()
        .context("Failed to load config file")?;

    // Handle logout
    if args.logout {
        if let Some(removed) = config.remove_session_auth(&cwd) {
            config.save()?;
            println!("{} Logged out from {}", "✓".bright_green(), removed.user_email.unwrap_or_default());
        } else {
            println!("No cached authentication found for this directory");
        }
        return Ok(());
    }

    info!("Starting Claude CLI proxy wrapper");
    info!("Session name: {}", args.session_name);
    info!("Backend URL: {}", args.backend_url);

    // Get or create auth token
    let auth_token = if let Some(ref token) = args.auth_token {
        // Explicit token provided via CLI
        token.clone()
    } else if !args.reauth {
        // Try to load from config
        if let Some(session_auth) = config.get_session_auth(&cwd) {
            info!("Using cached authentication for {}", session_auth.user_email.as_ref().unwrap_or(&"unknown user".to_string()));
            session_auth.auth_token.clone()
        } else {
            // No cached auth, need to authenticate
            let (token, user_id, user_email) = auth::device_flow_login(&args.backend_url).await?;

            // Save to config
            config.set_session_auth(
                cwd.clone(),
                SessionAuth {
                    user_id,
                    auth_token: token.clone(),
                    user_email: Some(user_email),
                    last_used: chrono::Utc::now().to_rfc3339(),
                },
            );
            config.save()?;

            token
        }
    } else {
        // Force re-authentication
        info!("Re-authenticating (--reauth flag set)");
        let (token, user_id, user_email) = auth::device_flow_login(&args.backend_url).await?;

        // Update config
        config.set_session_auth(
            cwd.clone(),
            SessionAuth {
                user_id,
                auth_token: token.clone(),
                user_email: Some(user_email),
                last_used: chrono::Utc::now().to_rfc3339(),
            },
        );
        config.save()?;

        token
    };

    // Connect to backend WebSocket
    let ws_url = format!("{}/ws/session", args.backend_url);
    info!("Connecting to backend at {}", ws_url);

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .context("Failed to connect to backend")?;

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Register session with backend
    let register_msg = ProxyMessage::Register {
        session_name: args.session_name.clone(),
        auth_token: args.auth_token.clone(),
        working_directory: cwd.clone(),
    };

    ws_write
        .send(Message::Text(serde_json::to_string(&register_msg)?))
        .await
        .context("Failed to register session")?;

    info!("Session registered with backend");

    // Spawn claude CLI process
    let mut claude_cmd = Command::new("claude");
    claude_cmd
        .args(&args.claude_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut claude_process = claude_cmd
        .spawn()
        .context("Failed to spawn claude CLI process")?;

    info!("Claude CLI process spawned");

    // Get handles to stdin/stdout/stderr
    let mut claude_stdin = claude_process
        .stdin
        .take()
        .context("Failed to get claude stdin")?;

    let claude_stdout = claude_process
        .stdout
        .take()
        .context("Failed to get claude stdout")?;

    let claude_stderr = claude_process
        .stderr
        .take()
        .context("Failed to get claude stderr")?;

    let stdout_reader = BufReader::new(claude_stdout);
    let stderr_reader = BufReader::new(claude_stderr);

    // Spawn task to read claude stdout and forward to backend
    let ws_write_clone = std::sync::Arc::new(tokio::sync::Mutex::new(ws_write));
    let ws_write_for_stdout = ws_write_clone.clone();

    tokio::spawn(async move {
        let mut lines = stdout_reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            info!("Claude stdout: {}", line);

            // Try to parse as JSON and forward
            if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&line) {
                let msg = ProxyMessage::ClaudeOutput {
                    content: json_value,
                };

                if let Ok(json) = serde_json::to_string(&msg) {
                    let mut ws = ws_write_for_stdout.lock().await;
                    if let Err(e) = ws.send(Message::Text(json)).await {
                        error!("Failed to send to backend: {}", e);
                        break;
                    }
                }
            }
        }
    });

    // Spawn task to read claude stderr
    tokio::spawn(async move {
        let mut lines = stderr_reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            warn!("Claude stderr: {}", line);
        }
    });

    // Read from WebSocket and forward to claude stdin
    while let Some(msg) = ws_read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                    match proxy_msg {
                        ProxyMessage::ClaudeInput { content } => {
                            info!("Received input from backend");

                            // Forward to claude stdin as JSON line
                            let json_line = format!("{}\n", serde_json::to_string(&content)?);
                            if let Err(e) = claude_stdin.write_all(json_line.as_bytes()).await {
                                error!("Failed to write to claude stdin: {}", e);
                                break;
                            }

                            if let Err(e) = claude_stdin.flush().await {
                                error!("Failed to flush claude stdin: {}", e);
                                break;
                            }
                        }
                        ProxyMessage::Heartbeat => {
                            // Respond to heartbeat
                            let mut ws = ws_write_clone.lock().await;
                            let heartbeat = ProxyMessage::Heartbeat;
                            if let Ok(json) = serde_json::to_string(&heartbeat) {
                                let _ = ws.send(Message::Text(json)).await;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("WebSocket closed by server");
                break;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    // Wait for claude process to exit
    let status = claude_process.wait().await?;
    info!("Claude process exited with status: {}", status);

    Ok(())
}
