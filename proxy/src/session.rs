//! Session management and WebSocket connection handling.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use claude_codes::io::{ContentBlock, ControlRequestPayload};
use claude_codes::{AsyncClient, ClaudeInput, ClaudeOutput};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::output_buffer::PendingOutputBuffer;
use crate::ui;

/// Type alias for the WebSocket stream
type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Type alias for the shared WebSocket write half
type SharedWsWrite = Arc<tokio::sync::Mutex<SplitSink<WsStream, Message>>>;

/// Type alias for the WebSocket read half
type WsRead = SplitStream<WsStream>;

/// WebSocket connection wrapper that owns both read and write halves.
/// Provides convenient methods for sending/receiving messages.
pub struct WebSocketConnection {
    write: SplitSink<WsStream, Message>,
    read: SplitStream<WsStream>,
}

impl WebSocketConnection {
    /// Create a new connection from a WebSocket stream
    pub fn new(stream: WsStream) -> Self {
        let (write, read) = stream.split();
        Self { write, read }
    }

    /// Send a ProxyMessage
    pub async fn send(&mut self, msg: &ProxyMessage) -> Result<(), String> {
        let json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        self.write
            .send(Message::Text(json))
            .await
            .map_err(|e| e.to_string())
    }

    /// Receive the next message
    pub async fn recv(&mut self) -> Option<Result<Message, tokio_tungstenite::tungstenite::Error>> {
        self.read.next().await
    }

    /// Split into write and read halves for concurrent use
    pub fn split(self) -> (SplitSink<WsStream, Message>, SplitStream<WsStream>) {
        (self.write, self.read)
    }
}

/// Configuration for a proxy session
#[derive(Clone)]
pub struct SessionConfig {
    pub backend_url: String,
    pub session_id: Uuid,
    pub session_name: String,
    pub auth_token: Option<String>,
    pub working_directory: String,
    pub resuming: bool,
    pub git_branch: Option<String>,
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

/// State that persists across WebSocket reconnections for a session.
/// This includes the input channel, output buffer, and session config.
pub struct SessionState<'a> {
    /// Session configuration
    pub config: &'a SessionConfig,
    /// Claude client for communication
    pub claude_client: &'a mut AsyncClient,
    /// Sender for input messages (cloned per connection)
    pub input_tx: mpsc::UnboundedSender<String>,
    /// Receiver for input messages (persists across connections)
    pub input_rx: &'a mut mpsc::UnboundedReceiver<String>,
    /// Output buffer with persistence
    pub output_buffer: Arc<Mutex<PendingOutputBuffer>>,
    /// Backoff state for reconnection
    pub backoff: Backoff,
    /// Whether this is the first connection attempt
    pub first_connection: bool,
}

impl<'a> SessionState<'a> {
    /// Create a new session state
    pub fn new(
        config: &'a SessionConfig,
        claude_client: &'a mut AsyncClient,
        input_tx: mpsc::UnboundedSender<String>,
        input_rx: &'a mut mpsc::UnboundedReceiver<String>,
    ) -> Result<Self> {
        let output_buffer = match PendingOutputBuffer::new(config.session_id) {
            Ok(buf) => buf,
            Err(e) => {
                warn!(
                    "Failed to create output buffer, continuing without persistence: {}",
                    e
                );
                PendingOutputBuffer::new(config.session_id)?
            }
        };
        let output_buffer = Arc::new(Mutex::new(output_buffer));

        Ok(Self {
            config,
            claude_client,
            input_tx,
            input_rx,
            output_buffer,
            backoff: Backoff::new(),
            first_connection: true,
        })
    }

    /// Log pending messages from previous session
    pub async fn log_pending_messages(&self) {
        let buf = self.output_buffer.lock().await;
        let pending = buf.pending_count();
        if pending > 0 {
            info!(
                "Loaded {} pending messages from previous session, will replay on connect",
                pending
            );
        }
    }

    /// Persist the output buffer
    pub async fn persist_buffer(&self) {
        if let Err(e) = self.output_buffer.lock().await.persist() {
            warn!("Failed to persist output buffer: {}", e);
        }
    }

    /// Get pending message count
    pub async fn pending_count(&self) -> usize {
        self.output_buffer.lock().await.pending_count()
    }
}

/// Run the WebSocket connection loop with auto-reconnect
pub async fn run_connection_loop(
    config: &SessionConfig,
    claude_client: &mut AsyncClient,
    input_tx: mpsc::UnboundedSender<String>,
    input_rx: &mut mpsc::UnboundedReceiver<String>,
) -> Result<()> {
    let mut session = SessionState::new(config, claude_client, input_tx, input_rx)?;
    session.log_pending_messages().await;

    loop {
        if session.first_connection {
            ui::print_ready_banner();
        }

        let result = run_single_connection(&mut session).await;
        session.first_connection = false;

        match result {
            ConnectionResult::ClaudeExited => {
                info!("Claude process exited, shutting down");
                session.persist_buffer().await;
                return Ok(());
            }
            ConnectionResult::Disconnected(duration) => {
                session.backoff.reset_if_stable(duration);
                session.persist_buffer().await;

                let pending = session.pending_count().await;
                ui::print_disconnected_with_pending(session.backoff.current_secs(), pending);
                info!(
                    "WebSocket disconnected, {} pending messages, reconnecting in {}s",
                    pending,
                    session.backoff.current_secs()
                );

                tokio::time::sleep(session.backoff.sleep_duration()).await;
                session.backoff.advance();
            }
        }
    }
}

/// Run a single WebSocket connection until it disconnects or Claude exits
async fn run_single_connection(session: &mut SessionState<'_>) -> ConnectionResult {
    // Connect to WebSocket
    let mut conn =
        match connect_to_backend(&session.config.backend_url, session.first_connection).await {
            Ok(conn) => conn,
            Err(duration) => return ConnectionResult::Disconnected(duration),
        };

    // Re-detect git branch on reconnect (it may have changed)
    let current_branch = get_git_branch(&session.config.working_directory);
    let config_with_branch = SessionConfig {
        git_branch: current_branch,
        ..session.config.clone()
    };

    // Register with backend and wait for acknowledgment
    if let Err(duration) = register_session(&mut conn, &config_with_branch).await {
        return ConnectionResult::Disconnected(duration);
    }

    // Replay pending messages after successful registration
    {
        let buf = session.output_buffer.lock().await;
        let pending_count = buf.pending_count();
        if pending_count > 0 {
            info!(
                "Replaying {} pending messages after reconnect",
                pending_count
            );
            for pending in buf.get_pending() {
                let msg = ProxyMessage::SequencedOutput {
                    seq: pending.seq,
                    content: pending.content.clone(),
                };
                if let Err(e) = conn.send(&msg).await {
                    error!(
                        "Failed to replay pending message seq={}: {}",
                        pending.seq, e
                    );
                    return ConnectionResult::Disconnected(Duration::ZERO);
                }
            }
            info!("Finished replaying pending messages");
        }
    }

    if !session.first_connection {
        ui::print_connection_restored();
    }

    // Run the message loop - split connection for concurrent read/write
    run_message_loop(session, &config_with_branch, conn).await
}

/// Connect to the backend WebSocket
async fn connect_to_backend(
    backend_url: &str,
    first_connection: bool,
) -> Result<WebSocketConnection, Duration> {
    let ws_url = format!("{}/ws/session", backend_url);

    if first_connection {
        ui::print_status("Connecting to backend...");
    } else {
        ui::print_status("Reconnecting to backend...");
    }

    match connect_async(&ws_url).await {
        Ok((stream, _)) => {
            ui::print_connected();
            Ok(WebSocketConnection::new(stream))
        }
        Err(e) => {
            ui::print_failed();
            error!("Failed to connect to backend: {}", e);
            Err(Duration::ZERO)
        }
    }
}

/// Register session with the backend and wait for acknowledgment
async fn register_session(
    conn: &mut WebSocketConnection,
    config: &SessionConfig,
) -> Result<(), Duration> {
    ui::print_status("Registering session...");

    let register_msg = ProxyMessage::Register {
        session_id: config.session_id,
        session_name: config.session_name.clone(),
        auth_token: config.auth_token.clone(),
        working_directory: config.working_directory.clone(),
        resuming: config.resuming,
        git_branch: config.git_branch.clone(),
    };

    if let Err(e) = conn.send(&register_msg).await {
        ui::print_failed();
        error!("Failed to send registration message: {}", e);
        return Err(Duration::ZERO);
    }

    // Wait for RegisterAck with timeout
    let ack_timeout = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = conn.recv().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(ProxyMessage::RegisterAck {
                        success,
                        session_id: _,
                        error,
                    }) = serde_json::from_str::<ProxyMessage>(&text)
                    {
                        return Some((success, error));
                    }
                }
                Ok(Message::Close(_)) => return None,
                Err(_) => return None,
                _ => continue,
            }
        }
        None
    })
    .await;

    match ack_timeout {
        Ok(Some((true, _))) => {
            ui::print_registered();
            Ok(())
        }
        Ok(Some((false, error))) => {
            let err_msg = error.as_deref().unwrap_or("Unknown error");
            ui::print_registration_failed(err_msg);
            if err_msg.contains("Authentication") || err_msg.contains("authenticate") {
                ui::print_reauth_hint();
            }
            error!("Registration failed: {}", err_msg);
            Err(Duration::ZERO)
        }
        Ok(None) => {
            ui::print_failed();
            error!("Connection closed during registration");
            Err(Duration::ZERO)
        }
        Err(_) => {
            // Timeout - assume success for backwards compatibility with older backends
            ui::print_registered();
            info!(
                "No RegisterAck received (timeout), assuming success for backwards compatibility"
            );
            Ok(())
        }
    }
}

/// Permission response data
#[derive(Debug)]
pub struct PermissionResponseData {
    pub request_id: String,
    pub allow: bool,
    pub input: Option<serde_json::Value>,
    pub permissions: Vec<serde_json::Value>,
    pub reason: Option<String>,
}

/// State for the main message loop, reducing parameter count
/// Contains channels and state that are specific to a single connection attempt.
/// Note: input_rx is passed separately as it persists across reconnections.
pub struct ConnectionState {
    /// Receiver for permission responses from frontend
    pub perm_rx: mpsc::UnboundedReceiver<PermissionResponseData>,
    /// Receiver for output acknowledgments from backend
    pub ack_rx: mpsc::UnboundedReceiver<u64>,
    /// Sender for Claude outputs to the output forwarder
    pub output_tx: mpsc::UnboundedSender<ClaudeOutput>,
    /// Receiver to detect WebSocket disconnection
    pub disconnect_rx: tokio::sync::oneshot::Receiver<()>,
    /// Session ID
    pub session_id: Uuid,
    /// When the connection was established
    pub connection_start: Instant,
    /// Buffer for pending outputs
    pub output_buffer: Arc<Mutex<PendingOutputBuffer>>,
}

/// Run the main message forwarding loop
async fn run_message_loop(
    session: &mut SessionState<'_>,
    config: &SessionConfig,
    conn: WebSocketConnection,
) -> ConnectionResult {
    let connection_start = Instant::now();
    let session_id = config.session_id;

    // Split connection for concurrent read/write
    let (ws_write, ws_read) = conn.split();

    // Channel for Claude outputs
    let (output_tx, output_rx) = mpsc::unbounded_channel::<ClaudeOutput>();

    // Channel for permission responses from frontend
    let (perm_tx, perm_rx) = mpsc::unbounded_channel::<PermissionResponseData>();

    // Channel for output acknowledgments from backend
    let (ack_tx, ack_rx) = mpsc::unbounded_channel::<u64>();

    // Wrap ws_write for sharing
    let ws_write = std::sync::Arc::new(tokio::sync::Mutex::new(ws_write));

    // Channel to signal WebSocket disconnection
    let (disconnect_tx, disconnect_rx) = tokio::sync::oneshot::channel::<()>();

    // Shared state for tracking git branch updates
    let current_branch = Arc::new(Mutex::new(config.git_branch.clone()));

    // Spawn output forwarder task with buffer
    let output_task = spawn_output_forwarder(
        output_rx,
        ws_write.clone(),
        session_id,
        config.working_directory.clone(),
        current_branch,
        session.output_buffer.clone(),
    );

    // Spawn WebSocket reader task
    let reader_task = spawn_ws_reader(
        ws_read,
        session.input_tx.clone(),
        perm_tx,
        ack_tx,
        ws_write.clone(),
        disconnect_tx,
    );

    // Create connection state (per-connection channels and timing)
    let mut conn_state = ConnectionState {
        perm_rx,
        ack_rx,
        output_tx,
        disconnect_rx,
        session_id,
        connection_start,
        output_buffer: session.output_buffer.clone(),
    };

    // Main loop
    let result = run_main_loop(session.claude_client, session.input_rx, &mut conn_state).await;

    // Clean up
    output_task.abort();
    reader_task.abort();

    result
}

/// Get the current git branch name, if in a git repository
fn get_git_branch(cwd: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty() && s != "HEAD")
            } else {
                None
            }
        })
}

/// Check if a tool use is a Bash command containing "git"
fn is_git_bash_command(output: &ClaudeOutput) -> bool {
    if let ClaudeOutput::User(user) = output {
        for block in &user.message.content {
            if let ContentBlock::ToolResult(tr) = block {
                // Check if this is a result from a Bash tool
                // We check tool_use_id pattern or content for git indicators
                // The safest way is to track pending tool calls, but for simplicity
                // we check if the result content mentions git commands
                if let Some(ref content) = tr.content {
                    let content_str = format!("{:?}", content);
                    // Check if this looks like output from a git command
                    if content_str.contains("git ")
                        || content_str.contains("branch")
                        || content_str.contains("checkout")
                        || content_str.contains("merge")
                        || content_str.contains("rebase")
                        || content_str.contains("commit")
                    {
                        return true;
                    }
                }
            }
        }
    }
    // Also check if an assistant message contains a Bash tool_use with git
    if let ClaudeOutput::Assistant(asst) = output {
        for block in &asst.message.content {
            if let ContentBlock::ToolUse(tu) = block {
                if tu.name == "Bash" {
                    if let Some(cmd) = tu.input.get("command").and_then(|v| v.as_str()) {
                        if cmd.contains("git ") {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// Check and send git branch update if changed
async fn check_and_send_branch_update(
    ws_write: &SharedWsWrite,
    session_id: Uuid,
    working_directory: &str,
    current_branch: &Arc<Mutex<Option<String>>>,
) {
    let new_branch = get_git_branch(working_directory);
    let mut branch_guard = current_branch.lock().await;

    if *branch_guard != new_branch {
        info!(
            "Git branch changed: {:?} -> {:?}",
            *branch_guard, new_branch
        );
        *branch_guard = new_branch.clone();

        // Send SessionUpdate to backend
        let update_msg = ProxyMessage::SessionUpdate {
            session_id,
            git_branch: new_branch,
        };

        if let Ok(json) = serde_json::to_string(&update_msg) {
            let mut ws = ws_write.lock().await;
            if let Err(e) = ws.send(Message::Text(json)).await {
                error!("Failed to send branch update: {}", e);
            }
        }
    }
}

/// Spawn the output forwarder task
fn spawn_output_forwarder(
    mut output_rx: mpsc::UnboundedReceiver<ClaudeOutput>,
    ws_write: SharedWsWrite,
    session_id: Uuid,
    working_directory: String,
    current_branch: Arc<Mutex<Option<String>>>,
    output_buffer: Arc<Mutex<PendingOutputBuffer>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut message_count: u64 = 0;
        let mut pending_git_check = false;

        while let Some(output) = output_rx.recv().await {
            message_count += 1;

            // Log detailed info about the message
            log_claude_output(&output);

            // Check if this is a git-related bash command
            if is_git_bash_command(&output) {
                pending_git_check = true;
            }

            // Handle ControlRequest specially - these are NOT buffered (time-sensitive)
            if let ClaudeOutput::ControlRequest(req) = &output {
                if let ControlRequestPayload::CanUseTool(tool_req) = &req.request {
                    let msg = ProxyMessage::PermissionRequest {
                        request_id: req.request_id.clone(),
                        tool_name: tool_req.tool_name.clone(),
                        input: tool_req.input.clone(),
                        permission_suggestions: tool_req.permission_suggestions.clone(),
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let mut ws = ws_write.lock().await;
                        if let Err(e) = ws.send(Message::Text(json)).await {
                            error!("Failed to send permission request to backend: {}", e);
                            break;
                        }
                    }
                    continue;
                }
            }

            // Skip control responses (they're acks from Claude, not for backend)
            if matches!(&output, ClaudeOutput::ControlResponse(_)) {
                continue;
            }

            // For all other outputs, serialize and buffer with sequence number
            let content = serde_json::to_value(&output)
                .unwrap_or(serde_json::Value::String(format!("{:?}", output)));

            // Add to buffer and get sequence number
            let seq = {
                let mut buf = output_buffer.lock().await;
                buf.push(content.clone())
            };

            // Send as sequenced output
            let msg = ProxyMessage::SequencedOutput { seq, content };

            if let Ok(json) = serde_json::to_string(&msg) {
                let mut ws = ws_write.lock().await;
                if let Err(e) = ws.send(Message::Text(json)).await {
                    error!("Failed to send to backend: {}", e);
                    break;
                }
            }

            // Check for branch update after git commands or every 100 messages
            let should_check_branch = pending_git_check || message_count.is_multiple_of(100);
            if should_check_branch {
                pending_git_check = false;
                check_and_send_branch_update(
                    &ws_write,
                    session_id,
                    &working_directory,
                    &current_branch,
                )
                .await;
            }
        }
        info!("Output forwarder ended");
    })
}

/// Log detailed information about Claude output
fn log_claude_output(output: &ClaudeOutput) {
    match output {
        ClaudeOutput::System(sys) => {
            info!("← [system] subtype={}", sys.subtype);
            if sys.subtype == "init" {
                if let Some(model) = sys.data.get("model").and_then(|v| v.as_str()) {
                    info!("  model: {}", model);
                }
                if let Some(cwd) = sys.data.get("cwd").and_then(|v| v.as_str()) {
                    info!("  cwd: {}", truncate(cwd, 60));
                }
                if let Some(tools) = sys.data.get("tools").and_then(|v| v.as_array()) {
                    info!("  tools: {} available", tools.len());
                }
            }
        }
        ClaudeOutput::Assistant(asst) => {
            let msg = &asst.message;
            let stop = msg.stop_reason.as_deref().unwrap_or("none");

            // Count content blocks by type
            let mut text_count = 0;
            let mut tool_count = 0;
            let mut thinking_count = 0;

            for block in &msg.content {
                match block {
                    ContentBlock::Text(t) => {
                        text_count += 1;
                        let preview = truncate(&t.text, 80);
                        info!("← [assistant] text: {}", preview);
                    }
                    ContentBlock::ToolUse(tu) => {
                        tool_count += 1;
                        let input_preview = format_tool_input(&tu.name, &tu.input);
                        info!("← [assistant] tool_use: {} {}", tu.name, input_preview);
                    }
                    ContentBlock::Thinking(th) => {
                        thinking_count += 1;
                        let preview = truncate(&th.thinking, 60);
                        debug!("← [assistant] thinking: {}", preview);
                    }
                    ContentBlock::ToolResult(tr) => {
                        let status = if tr.is_error.unwrap_or(false) {
                            "error"
                        } else {
                            "ok"
                        };
                        info!("← [assistant] tool_result: {} ({})", tr.tool_use_id, status);
                    }
                    ContentBlock::Image(_) => {
                        info!("← [assistant] image block");
                    }
                }
            }

            if text_count + tool_count + thinking_count > 1 {
                info!(
                    "  stop_reason={}, blocks: {} text, {} tools, {} thinking",
                    stop, text_count, tool_count, thinking_count
                );
            } else if tool_count > 0 || stop != "none" {
                info!("  stop_reason={}", stop);
            }
        }
        ClaudeOutput::User(user) => {
            for block in &user.message.content {
                match block {
                    ContentBlock::Text(t) => {
                        info!("← [user] text: {}", truncate(&t.text, 80));
                    }
                    ContentBlock::ToolResult(tr) => {
                        let status = if tr.is_error.unwrap_or(false) {
                            "ERROR"
                        } else {
                            "ok"
                        };
                        let content_preview = tr
                            .content
                            .as_ref()
                            .map(|c| {
                                let s = format!("{:?}", c);
                                if s.len() > 60 {
                                    format!("{}...", truncate(&s, 60))
                                } else {
                                    s
                                }
                            })
                            .unwrap_or_default();
                        info!("← [user] tool_result [{}]: {}", status, content_preview);
                    }
                    _ => {
                        info!("← [user] other block");
                    }
                }
            }
        }
        ClaudeOutput::Result(res) => {
            let status = if res.is_error { "ERROR" } else { "success" };
            let duration = format_duration(res.duration_ms);
            let api_duration = format_duration(res.duration_api_ms);
            info!(
                "← [result] {} | {} total | {} API | {} turns",
                status, duration, api_duration, res.num_turns
            );
            if res.total_cost_usd > 0.0 {
                info!("  cost: ${:.4}", res.total_cost_usd);
            }
        }
        ClaudeOutput::ControlRequest(req) => {
            info!("← [control_request] id={}", req.request_id);
            match &req.request {
                ControlRequestPayload::CanUseTool(tool_req) => {
                    let input_preview = format_tool_input(&tool_req.tool_name, &tool_req.input);
                    info!("  tool: {} {}", tool_req.tool_name, input_preview);
                }
                ControlRequestPayload::HookCallback(_) => {
                    info!("  hook callback");
                }
                ControlRequestPayload::McpMessage(_) => {
                    info!("  MCP message");
                }
                ControlRequestPayload::Initialize(_) => {
                    info!("  initialize");
                }
            }
        }
        ClaudeOutput::ControlResponse(resp) => {
            debug!("← [control_response] {:?}", resp);
        }
    }
}

/// Format tool input for logging
fn format_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| format!("$ {}", truncate(s, 70)))
            .unwrap_or_default(),
        "Read" | "Edit" | "Write" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 70).to_string())
            .unwrap_or_default(),
        "Glob" | "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("?");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            format!("'{}' in {}", truncate(pattern, 40), truncate(path, 30))
        }
        "Task" => input
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60).to_string())
            .unwrap_or_default(),
        "WebFetch" | "WebSearch" => input
            .get("url")
            .or_else(|| input.get("query"))
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60).to_string())
            .unwrap_or_default(),
        _ => {
            // Generic: show first string field
            if let Some(obj) = input.as_object() {
                obj.iter()
                    .find_map(|(k, v)| v.as_str().map(|s| format!("{}={}", k, truncate(s, 50))))
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
    }
}

/// Truncate a string to max length, adding "..." if truncated
fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        // Find a safe UTF-8 boundary
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Format duration in ms to human readable
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60000;
        let secs = (ms % 60000) / 1000;
        format!("{}m{}s", mins, secs)
    }
}

/// Spawn the WebSocket reader task
fn spawn_ws_reader(
    mut ws_read: WsRead,
    input_tx: mpsc::UnboundedSender<String>,
    perm_tx: mpsc::UnboundedSender<PermissionResponseData>,
    ack_tx: mpsc::UnboundedSender<u64>,
    ws_write: SharedWsWrite,
    disconnect_tx: tokio::sync::oneshot::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if !handle_ws_text_message(&text, &input_tx, &perm_tx, &ack_tx, &ws_write).await
                    {
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
async fn handle_ws_text_message(
    text: &str,
    input_tx: &mpsc::UnboundedSender<String>,
    perm_tx: &mpsc::UnboundedSender<PermissionResponseData>,
    ack_tx: &mpsc::UnboundedSender<u64>,
    ws_write: &SharedWsWrite,
) -> bool {
    debug!("ws recv: {}", truncate(text, 200));

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
            info!("→ [input] {}", truncate(&text, 80));
            if input_tx.send(text).is_err() {
                error!("Failed to send input to channel");
                return false;
            }
        }
        ProxyMessage::PermissionResponse {
            request_id,
            allow,
            input,
            permissions,
            reason,
        } => {
            info!(
                "→ [perm_response] {} allow={} permissions={} reason={:?}",
                request_id,
                allow,
                permissions.len(),
                reason
            );
            if perm_tx
                .send(PermissionResponseData {
                    request_id,
                    allow,
                    input,
                    permissions,
                    reason,
                })
                .is_err()
            {
                error!("Failed to send permission response to channel");
                return false;
            }
        }
        ProxyMessage::OutputAck {
            session_id: _,
            ack_seq,
        } => {
            debug!("→ [output_ack] seq={}", ack_seq);
            if ack_tx.send(ack_seq).is_err() {
                error!("Failed to send output ack to channel");
                return false;
            }
        }
        ProxyMessage::Heartbeat => {
            debug!("heartbeat");
            let mut ws = ws_write.lock().await;
            if let Ok(json) = serde_json::to_string(&ProxyMessage::Heartbeat) {
                let _ = ws.send(Message::Text(json)).await;
            }
        }
        _ => {
            debug!("ws msg: {:?}", proxy_msg);
        }
    }

    true
}

/// Run the main select loop
async fn run_main_loop(
    claude_client: &mut AsyncClient,
    input_rx: &mut mpsc::UnboundedReceiver<String>,
    state: &mut ConnectionState,
) -> ConnectionResult {
    use claude_codes::io::{ControlResponse, PermissionResult};

    loop {
        tokio::select! {
            _ = &mut state.disconnect_rx => {
                info!("WebSocket disconnected");
                return ConnectionResult::Disconnected(state.connection_start.elapsed());
            }

            Some(text) = input_rx.recv() => {
                debug!("sending to claude process: {}", truncate(&text, 100));
                let input = ClaudeInput::user_message(&text, state.session_id);

                if let Err(e) = claude_client.send(&input).await {
                    error!("Failed to send to Claude: {}", e);
                    return ConnectionResult::ClaudeExited;
                }
            }

            Some(perm_response) = state.perm_rx.recv() => {
                info!("sending permission response to claude: {:?}", perm_response);

                // Create the control response
                let ctrl_response = if perm_response.allow {
                    // Use the original input from the permission response
                    let input = perm_response.input.unwrap_or(serde_json::Value::Object(Default::default()));

                    if perm_response.permissions.is_empty() {
                        // Simple allow without remembering
                        ControlResponse::from_result(
                            &perm_response.request_id,
                            PermissionResult::allow(input)
                        )
                    } else {
                        // Allow with permissions for future similar operations
                        ControlResponse::from_result(
                            &perm_response.request_id,
                            PermissionResult::allow_with_permissions(input, perm_response.permissions)
                        )
                    }
                } else {
                    ControlResponse::from_result(
                        &perm_response.request_id,
                        PermissionResult::deny(perm_response.reason.unwrap_or_else(|| "User denied".to_string()))
                    )
                };

                if let Err(e) = claude_client.send_control_response(ctrl_response).await {
                    error!("Failed to send permission response to Claude: {}", e);
                    return ConnectionResult::ClaudeExited;
                }
            }

            Some(ack_seq) = state.ack_rx.recv() => {
                // Acknowledge receipt of messages from backend
                let mut buf = state.output_buffer.lock().await;
                buf.acknowledge(ack_seq);
                // Persist periodically (on every ack for now, could be batched)
                if let Err(e) = buf.persist() {
                    warn!("Failed to persist buffer after ack: {}", e);
                }
            }

            result = claude_client.receive() => {
                match handle_claude_output(result, &state.output_tx, state.connection_start) {
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
                info!("--- ready for input ---");
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
