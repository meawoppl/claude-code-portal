//! Session management and WebSocket connection handling.

use std::time::{Duration, Instant};

use anyhow::Result;
use claude_codes::{AsyncClient, ClaudeInput, ClaudeOutput};
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info};
use uuid::Uuid;

use crate::ui;

/// Configuration for a proxy session
#[derive(Clone)]
pub struct SessionConfig {
    pub backend_url: String,
    pub session_id: Uuid,
    pub session_name: String,
    pub auth_token: Option<String>,
    pub working_directory: String,
    pub resuming: bool,
}

/// Exponential backoff helper
pub struct Backoff {
    current: u64,
    initial: u64,
    max: u64,
    multiplier: u64,
    stable_threshold: u64,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            current: 1,
            initial: 1,
            max: 30,
            multiplier: 2,
            stable_threshold: 30,
        }
    }

    /// Get the current backoff duration
    pub fn current_secs(&self) -> u64 {
        self.current
    }

    /// Advance to the next backoff interval
    pub fn advance(&mut self) {
        self.current = (self.current * self.multiplier).min(self.max);
    }

    /// Reset backoff if connection was stable
    pub fn reset_if_stable(&mut self, connection_duration: Duration) {
        if connection_duration.as_secs() >= self.stable_threshold {
            info!(
                "Connection was stable for {}s, resetting backoff",
                connection_duration.as_secs()
            );
            self.current = self.initial;
        }
    }

    /// Get a sleep duration
    pub fn sleep_duration(&self) -> Duration {
        Duration::from_secs(self.current)
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Result from a single WebSocket connection attempt
pub enum ConnectionResult {
    /// Claude process exited normally
    ClaudeExited,
    /// WebSocket disconnected, includes how long the connection was up
    Disconnected(Duration),
}

/// Run the WebSocket connection loop with auto-reconnect
pub async fn run_connection_loop(
    config: &SessionConfig,
    claude_client: &mut AsyncClient,
    input_tx: mpsc::UnboundedSender<String>,
    input_rx: &mut mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    let mut backoff = Backoff::new();
    let mut first_connection = true;

    loop {
        if first_connection {
            ui::print_ready_banner();
        }

        let result = run_single_connection(
            config,
            claude_client,
            input_tx.clone(),
            input_rx,
            first_connection,
        )
        .await;

        first_connection = false;

        match result {
            ConnectionResult::ClaudeExited => {
                info!("Claude process exited, shutting down");
                return Ok(());
            }
            ConnectionResult::Disconnected(duration) => {
                backoff.reset_if_stable(duration);

                ui::print_disconnected(backoff.current_secs());
                info!("WebSocket disconnected, reconnecting in {}s", backoff.current_secs());

                tokio::time::sleep(backoff.sleep_duration()).await;
                backoff.advance();
            }
        }
    }
}

/// Run a single WebSocket connection until it disconnects or Claude exits
async fn run_single_connection(
    config: &SessionConfig,
    claude_client: &mut AsyncClient,
    input_tx: mpsc::UnboundedSender<String>,
    input_rx: &mut mpsc::UnboundedReceiver<String>,
    first_connection: bool,
) -> ConnectionResult {
    // Connect to WebSocket
    let ws_stream = match connect_to_backend(&config.backend_url, first_connection).await {
        Ok(stream) => stream,
        Err(duration) => return ConnectionResult::Disconnected(duration),
    };

    let (mut ws_write, ws_read) = ws_stream.split();

    // Register with backend
    if let Err(duration) = register_session(&mut ws_write, config).await {
        return ConnectionResult::Disconnected(duration);
    }

    if !first_connection {
        ui::print_connection_restored();
    }

    // Run the message loop
    run_message_loop(config, claude_client, input_tx, input_rx, ws_write, ws_read).await
}

/// Connect to the backend WebSocket
async fn connect_to_backend(
    backend_url: &str,
    first_connection: bool,
) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Duration> {
    let ws_url = format!("{}/ws/session", backend_url);

    if first_connection {
        ui::print_status("Connecting to backend...");
    } else {
        ui::print_status("Reconnecting to backend...");
    }

    match connect_async(&ws_url).await {
        Ok((stream, _)) => {
            ui::print_connected();
            Ok(stream)
        }
        Err(e) => {
            ui::print_failed();
            error!("Failed to connect to backend: {}", e);
            Err(Duration::ZERO)
        }
    }
}

/// Register session with the backend
async fn register_session<S>(ws_write: &mut S, config: &SessionConfig) -> Result<(), Duration>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    ui::print_status("Registering session...");

    let register_msg = ProxyMessage::Register {
        session_id: config.session_id,
        session_name: config.session_name.clone(),
        auth_token: config.auth_token.clone(),
        working_directory: config.working_directory.clone(),
        resuming: config.resuming,
    };

    let json = serde_json::to_string(&register_msg).unwrap_or_default();

    if let Err(e) = ws_write.send(Message::Text(json)).await {
        ui::print_failed();
        error!("Failed to register session: {}", e);
        return Err(Duration::ZERO);
    }

    ui::print_registered();
    Ok(())
}

/// Run the main message forwarding loop
async fn run_message_loop<S, R>(
    config: &SessionConfig,
    claude_client: &mut AsyncClient,
    input_tx: mpsc::UnboundedSender<String>,
    input_rx: &mut mpsc::UnboundedReceiver<String>,
    ws_write: S,
    ws_read: R,
) -> ConnectionResult
where
    S: SinkExt<Message> + Unpin + Send + 'static,
    S::Error: std::fmt::Display,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
{
    let connection_start = Instant::now();
    let session_id = config.session_id;

    // Channel for Claude outputs
    let (output_tx, output_rx) = mpsc::unbounded_channel::<ClaudeOutput>();

    // Wrap ws_write for sharing
    let ws_write = std::sync::Arc::new(tokio::sync::Mutex::new(ws_write));

    // Channel to signal WebSocket disconnection
    let (disconnect_tx, mut disconnect_rx) = tokio::sync::oneshot::channel::<()>();

    // Spawn output forwarder task
    let output_task = spawn_output_forwarder(output_rx, ws_write.clone());

    // Spawn WebSocket reader task
    let reader_task = spawn_ws_reader(ws_read, input_tx, ws_write.clone(), disconnect_tx);

    // Main loop
    let result = run_main_loop(
        claude_client,
        input_rx,
        &output_tx,
        &mut disconnect_rx,
        session_id,
        connection_start,
    )
    .await;

    // Clean up
    output_task.abort();
    reader_task.abort();

    result
}

/// Spawn the output forwarder task
fn spawn_output_forwarder<S>(
    mut output_rx: mpsc::UnboundedReceiver<ClaudeOutput>,
    ws_write: std::sync::Arc<tokio::sync::Mutex<S>>,
) -> tokio::task::JoinHandle<()>
where
    S: SinkExt<Message> + Unpin + Send + 'static,
    S::Error: std::fmt::Display,
{
    tokio::spawn(async move {
        while let Some(output) = output_rx.recv().await {
            info!("Claude output [{}]", output.message_type());

            let content = serde_json::to_value(&output)
                .unwrap_or(serde_json::Value::String(format!("{:?}", output)));
            let msg = ProxyMessage::ClaudeOutput { content };

            if let Ok(json) = serde_json::to_string(&msg) {
                let mut ws = ws_write.lock().await;
                if let Err(e) = ws.send(Message::Text(json)).await {
                    error!("Failed to send to backend: {}", e);
                    break;
                }
            }
        }
        info!("Output forwarder ended");
    })
}

/// Spawn the WebSocket reader task
fn spawn_ws_reader<S, R>(
    mut ws_read: R,
    input_tx: mpsc::UnboundedSender<String>,
    ws_write: std::sync::Arc<tokio::sync::Mutex<S>>,
    disconnect_tx: tokio::sync::oneshot::Sender<()>,
) -> tokio::task::JoinHandle<()>
where
    S: SinkExt<Message> + Unpin + Send + 'static,
    S::Error: std::fmt::Display,
    R: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if !handle_ws_text_message(&text, &input_tx, &ws_write).await {
                        break;
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
        let _ = disconnect_tx.send(());
    })
}

/// Handle a text message from the WebSocket
async fn handle_ws_text_message<S>(
    text: &str,
    input_tx: &mpsc::UnboundedSender<String>,
    ws_write: &std::sync::Arc<tokio::sync::Mutex<S>>,
) -> bool
where
    S: SinkExt<Message> + Unpin + Send,
    S::Error: std::fmt::Display,
{
    info!(
        "Received WebSocket message: {}",
        &text[..std::cmp::min(text.len(), 200)]
    );

    let proxy_msg = match serde_json::from_str::<ProxyMessage>(text) {
        Ok(msg) => msg,
        Err(_) => return true, // Continue on parse error
    };

    match proxy_msg {
        ProxyMessage::ClaudeInput { content } => {
            let text = match &content {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if input_tx.send(text).is_err() {
                error!("Failed to send input to channel");
                return false;
            }
        }
        ProxyMessage::Heartbeat => {
            let mut ws = ws_write.lock().await;
            if let Ok(json) = serde_json::to_string(&ProxyMessage::Heartbeat) {
                let _ = ws.send(Message::Text(json)).await;
            }
        }
        _ => {}
    }

    true
}

/// Run the main select loop
async fn run_main_loop(
    claude_client: &mut AsyncClient,
    input_rx: &mut mpsc::UnboundedReceiver<String>,
    output_tx: &mpsc::UnboundedSender<ClaudeOutput>,
    disconnect_rx: &mut tokio::sync::oneshot::Receiver<()>,
    session_id: Uuid,
    connection_start: Instant,
) -> ConnectionResult {
    loop {
        tokio::select! {
            _ = &mut *disconnect_rx => {
                info!("WebSocket disconnected");
                return ConnectionResult::Disconnected(connection_start.elapsed());
            }

            Some(text) = input_rx.recv() => {
                info!("Sending to Claude: {}", text);
                let input = ClaudeInput::user_message(&text, session_id);

                if let Err(e) = claude_client.send(&input).await {
                    error!("Failed to send to Claude: {}", e);
                    return ConnectionResult::ClaudeExited;
                }
            }

            result = claude_client.receive() => {
                match handle_claude_output(result, output_tx, connection_start) {
                    Some(result) => return result,
                    None => continue,
                }
            }
        }
    }
}

/// Handle output from Claude, returning Some(result) if we should exit
fn handle_claude_output(
    result: Result<ClaudeOutput, claude_codes::Error>,
    output_tx: &mpsc::UnboundedSender<ClaudeOutput>,
    connection_start: Instant,
) -> Option<ConnectionResult> {
    match result {
        Ok(output) => {
            let is_result = matches!(&output, ClaudeOutput::Result(_));
            if output_tx.send(output).is_err() {
                error!("Failed to forward Claude output");
                return Some(ConnectionResult::Disconnected(connection_start.elapsed()));
            }
            if is_result {
                info!("Received Result message, ready for next query");
            }
            None
        }
        Err(claude_codes::Error::ConnectionClosed) => {
            info!("Claude connection closed");
            Some(ConnectionResult::ClaudeExited)
        }
        Err(e) => {
            error!("Error receiving from Claude: {}", e);
            if matches!(e, claude_codes::Error::Io(_)) {
                Some(ConnectionResult::ClaudeExited)
            } else {
                None // Continue on parse errors
            }
        }
    }
}
