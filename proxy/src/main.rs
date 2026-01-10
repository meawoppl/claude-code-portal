mod auth;
mod config;

use anyhow::{Context, Result};
use clap::Parser;
use claude_codes::{AsyncClient, ClaudeCliBuilder, ClaudeInput, ClaudeOutput};
use colored::Colorize;
use config::{ProxyConfig, SessionAuth};
use futures_util::{SinkExt, StreamExt};
use shared::{ProxyInitConfig, ProxyMessage};
use tokio::io::AsyncBufReadExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "claude-proxy")]
#[command(about = "Wrapper for Claude CLI that proxies sessions to web interface")]
struct Args {
    /// Initialize with a token URL from the web interface
    /// Format: https://server.com/p/{base64_config} or just the JWT token
    #[arg(long)]
    init: Option<String>,

    /// Backend server URL (auto-detected from --init URL if provided)
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
    // Initialize tracing with info level by default
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

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
    let mut config = ProxyConfig::load().context("Failed to load config file")?;

    // Handle logout
    if args.logout {
        if let Some(removed) = config.remove_session_auth(&cwd) {
            config.save()?;
            println!(
                "{} Logged out from {}",
                "✓".bright_green(),
                removed.user_email.unwrap_or_default()
            );
        } else {
            println!("No cached authentication found for this directory");
        }
        return Ok(());
    }

    // Handle --init: parse init URL or token and save to config
    if let Some(init_value) = &args.init {
        let (backend_url, token, session_prefix) = parse_init_value(init_value)?;

        // Extract user info from JWT (basic parsing without verification)
        let user_email = extract_email_from_jwt(&token);

        println!(
            "{} Initializing proxy with token for {}",
            "→".bright_blue(),
            user_email.as_deref().unwrap_or("unknown user")
        );

        // Save to config
        config.set_session_auth(
            cwd.clone(),
            SessionAuth {
                user_id: String::new(), // Will be populated from JWT claims
                auth_token: token.clone(),
                user_email: user_email.clone(),
                last_used: chrono::Utc::now().to_rfc3339(),
                backend_url: backend_url.clone(),
                session_prefix: session_prefix.clone(),
            },
        );

        // Also save the backend URL
        if let Some(url) = &backend_url {
            config.set_backend_url(&cwd, url);
        }

        // Save session name prefix if provided
        if let Some(prefix) = session_prefix {
            config.set_session_prefix(&cwd, &prefix);
        }

        config.save()?;

        println!(
            "{} Configuration saved for {}",
            "✓".bright_green(),
            user_email.unwrap_or_else(|| "this directory".to_string())
        );
        println!("  Backend: {}", backend_url.as_deref().unwrap_or(&args.backend_url));
        println!();
        println!("You can now run {} without arguments.", "claude-proxy".bright_cyan());

        return Ok(());
    }

    println!();
    println!("{}", "╭──────────────────────────────────────╮".bright_blue());
    println!("{}", "│       Claude Code Proxy Starting     │".bright_blue());
    println!("{}", "╰──────────────────────────────────────╯".bright_blue());
    println!();
    println!("  {} {}", "Session:".dimmed(), args.session_name.bright_white());
    println!("  {} {}", "Backend:".dimmed(), args.backend_url.bright_white());
    println!();

    // Get or create auth token
    let auth_token: Option<String> = if args.dev {
        // Dev mode - skip authentication entirely
        println!("  {} {}", "Mode:".dimmed(), "development (no auth)".bright_yellow());
        println!();
        None
    } else if let Some(ref token) = args.auth_token {
        // Explicit token provided via CLI
        Some(token.clone())
    } else if !args.reauth {
        // Try to load from config
        if let Some(session_auth) = config.get_session_auth(&cwd) {
            println!(
                "  {} {}",
                "User:".dimmed(),
                session_auth
                    .user_email
                    .as_ref()
                    .unwrap_or(&"unknown user".to_string())
                    .bright_white()
            );
            println!();
            Some(session_auth.auth_token.clone())
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
                    backend_url: None,
                    session_prefix: None,
                },
            );
            config.save()?;

            Some(token)
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
                backend_url: None,
                session_prefix: None,
            },
        );
        config.save()?;

        Some(token)
    };

    // Connect to backend WebSocket
    let ws_url = format!("{}/ws/session", args.backend_url);
    print!("  {} Connecting to backend... ", "→".bright_blue());
    std::io::Write::flush(&mut std::io::stdout())?;

    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .context("Failed to connect to backend")?;

    println!("{}", "connected".bright_green());

    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Register session with backend
    print!("  {} Registering session... ", "→".bright_blue());
    std::io::Write::flush(&mut std::io::stdout())?;

    let register_msg = ProxyMessage::Register {
        session_name: args.session_name.clone(),
        auth_token,
        working_directory: cwd.clone(),
    };

    ws_write
        .send(Message::Text(serde_json::to_string(&register_msg)?))
        .await
        .context("Failed to register session")?;

    println!("{}", "registered".bright_green());

    // Create Claude client with JSON streaming mode
    print!("  {} Starting Claude CLI... ", "→".bright_blue());
    std::io::Write::flush(&mut std::io::stdout())?;

    // Generate a unique session ID for Claude
    let claude_session_id = uuid::Uuid::new_v4();

    let builder = ClaudeCliBuilder::new().session_id(claude_session_id);
    let mut claude_client = AsyncClient::from_builder(builder)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start Claude client: {}", e))?;

    println!("{}", "started".bright_green());
    println!();
    println!("{}", "╭──────────────────────────────────────╮".bright_green());
    println!("{}", "│         ✓ Proxy Ready                │".bright_green());
    println!("{}", "╰──────────────────────────────────────╯".bright_green());
    println!();
    println!("  Session is now visible in the web interface.");
    println!("  Press {} to stop.", "Ctrl+C".bright_yellow());
    println!();

    // Take stderr for logging
    let stderr_reader = claude_client.take_stderr();
    if let Some(mut stderr) = stderr_reader {
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

    // Create channels for coordinating between tasks
    let (output_tx, mut output_rx) = tokio::sync::mpsc::unbounded_channel::<ClaudeOutput>();
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Wrap ws_write for sharing between tasks
    let ws_write = std::sync::Arc::new(tokio::sync::Mutex::new(ws_write));
    let ws_write_for_output = ws_write.clone();

    // Task: Forward Claude outputs to backend
    tokio::spawn(async move {
        while let Some(output) = output_rx.recv().await {
            info!("Claude output [{}]", output.message_type());

            // Serialize the ClaudeOutput to JSON for the backend
            let content = serde_json::to_value(&output)
                .unwrap_or(serde_json::Value::String(format!("{:?}", output)));
            let msg = ProxyMessage::ClaudeOutput { content };

            if let Ok(json) = serde_json::to_string(&msg) {
                let mut ws = ws_write_for_output.lock().await;
                if let Err(e) = ws.send(Message::Text(json)).await {
                    error!("Failed to send to backend: {}", e);
                    break;
                }
            }
        }
        info!("Output forwarder ended");
    });

    // Task: Read WebSocket messages and forward to input channel
    let input_tx_clone = input_tx.clone();
    let ws_write_for_heartbeat = ws_write.clone();
    tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    info!("Received WebSocket message: {}", &text[..std::cmp::min(text.len(), 200)]);
                    if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                        match proxy_msg {
                            ProxyMessage::ClaudeInput { content } => {
                                let text = match &content {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                if input_tx_clone.send(text).is_err() {
                                    error!("Failed to send input to channel");
                                    break;
                                }
                            }
                            ProxyMessage::Heartbeat => {
                                let mut ws = ws_write_for_heartbeat.lock().await;
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
        info!("WebSocket reader ended");
    });

    // Main loop: Send inputs and receive outputs using AsyncClient
    loop {
        tokio::select! {
            // Check for incoming user input from WebSocket
            Some(text) = input_rx.recv() => {
                info!("Sending to Claude: {}", text);
                let input = ClaudeInput::user_message(&text, claude_session_id);

                if let Err(e) = claude_client.send(&input).await {
                    error!("Failed to send to Claude: {}", e);
                    break;
                }
            }

            // Try to receive output from Claude
            result = claude_client.receive() => {
                match result {
                    Ok(output) => {
                        let is_result = matches!(&output, ClaudeOutput::Result(_));
                        if output_tx.send(output).is_err() {
                            error!("Failed to forward Claude output");
                            break;
                        }
                        // Don't break on Result - Claude session continues
                        if is_result {
                            info!("Received Result message, ready for next query");
                        }
                    }
                    Err(claude_codes::Error::ConnectionClosed) => {
                        info!("Claude connection closed");
                        break;
                    }
                    Err(e) => {
                        error!("Error receiving from Claude: {}", e);
                        // Continue on parse errors, break on connection errors
                        if matches!(e, claude_codes::Error::Io(_)) {
                            break;
                        }
                    }
                }
            }
        }
    }

    info!("Proxy shutting down");
    let _ = claude_client.shutdown().await;

    Ok(())
}

/// Parse an init value which can be:
/// - A full URL: https://server.com/p/{base64_config}
/// - Just the base64 config part
/// - A raw JWT token
///
/// Returns (backend_url, token, session_prefix)
fn parse_init_value(value: &str) -> Result<(Option<String>, String, Option<String>)> {
    // Check if it's a URL
    if value.starts_with("http://") || value.starts_with("https://") {
        // Parse URL to extract backend and config
        let url = url::Url::parse(value).context("Invalid init URL")?;

        // Extract backend URL (scheme + host)
        let backend_url = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or("localhost"),
            url.port().map(|p| format!(":{}", p)).unwrap_or_default()
        );

        // WebSocket URL
        let ws_scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        let ws_url = format!(
            "{}://{}{}",
            ws_scheme,
            url.host_str().unwrap_or("localhost"),
            url.port().map(|p| format!(":{}", p)).unwrap_or_default()
        );

        // Extract config from path (expected: /p/{config})
        let path = url.path();
        if let Some(config_part) = path.strip_prefix("/p/") {
            let config = ProxyInitConfig::decode(config_part)
                .map_err(|e| anyhow::anyhow!("Failed to decode init config from URL: {}", e))?;

            return Ok((Some(ws_url), config.token, config.session_name_prefix));
        }

        // If not /p/ path, maybe the whole path is the config?
        anyhow::bail!("Invalid init URL format. Expected: https://server.com/p/{{config}}");
    }

    // Check if it looks like a JWT (three base64 parts separated by dots)
    if value.contains('.') && value.split('.').count() == 3 {
        // It's a raw JWT token
        return Ok((None, value.to_string(), None));
    }

    // Try to decode as ProxyInitConfig
    match ProxyInitConfig::decode(value) {
        Ok(config) => Ok((None, config.token, config.session_name_prefix)),
        Err(_) => {
            // Maybe it's just a token?
            Ok((None, value.to_string(), None))
        }
    }
}

/// Extract email from JWT without verification (for display purposes only)
fn extract_email_from_jwt(token: &str) -> Option<String> {
    // JWT format: header.payload.signature
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    // Decode payload (with padding fix for base64)
    let payload = parts[1];
    let padded = match payload.len() % 4 {
        2 => format!("{}==", payload),
        3 => format!("{}=", payload),
        _ => payload.to_string(),
    };

    // Use URL-safe base64 decoding
    let decoded = base64_url_decode(&padded).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

    json.get("email").and_then(|e| e.as_str()).map(String::from)
}

/// Simple base64url decoder
fn base64_url_decode(input: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let chars: Vec<u8> = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| {
            ALPHABET
                .iter()
                .position(|&x| x == c as u8)
                .map(|p| p as u8)
                .ok_or_else(|| anyhow::anyhow!("Invalid base64url character: {}", c))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut result = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let b0 = chars[i];
        let b1 = if i + 1 < chars.len() { chars[i + 1] } else { 0 };
        let b2 = if i + 2 < chars.len() { chars[i + 2] } else { 0 };
        let b3 = if i + 3 < chars.len() { chars[i + 3] } else { 0 };

        result.push((b0 << 2) | (b1 >> 4));

        if i + 2 < chars.len() {
            result.push((b1 << 4) | (b2 >> 2));
        }

        if i + 3 < chars.len() {
            result.push((b2 << 6) | b3);
        }

        i += 4;
    }

    Ok(result)
}
