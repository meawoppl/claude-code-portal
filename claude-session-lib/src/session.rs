//! Core session management

use chrono::Utc;
use claude_codes::io::{ControlResponse, PermissionResult};
use claude_codes::{AsyncClient, ClaudeInput, ClaudeOutput};
use std::path::Path;
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::buffer::OutputBuffer;
use crate::error::SessionError;
use crate::snapshot::{PendingPermission, SessionConfig, SessionSnapshot};

/// Events emitted by a session
#[derive(Debug)]
pub enum SessionEvent {
    /// Claude produced output (excluding permission requests, which have their own event)
    Output(ClaudeOutput),

    /// Claude is requesting permission for a tool
    ///
    /// This is the canonical event for permission requests. Permission requests
    /// are NOT emitted as `Output(ControlRequest(...))` - only this event is used.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
        permission_suggestions: Vec<claude_codes::io::PermissionSuggestion>,
    },

    /// Session not found locally (e.g., when resuming an expired session)
    ///
    /// This is emitted when Claude reports "No conversation found" error,
    /// indicating the session ID doesn't exist locally. The caller should
    /// typically start a fresh session with a new ID.
    SessionNotFound,

    /// Claude process exited
    Exited { code: i32 },

    /// Session encountered an error
    Error(SessionError),
}

/// Response to a permission request
#[derive(Debug, Clone, Default)]
pub struct PermissionResponse {
    /// Whether to allow the tool use
    pub allow: bool,
    /// Optional modified input (for edit suggestions)
    pub input: Option<serde_json::Value>,
    /// Permissions to grant for future similar operations ("remember this decision")
    pub permissions: Vec<claude_codes::Permission>,
    /// Reason for denial (if allow is false)
    pub reason: Option<String>,
}

impl PermissionResponse {
    /// Create an allow response
    pub fn allow() -> Self {
        Self {
            allow: true,
            input: None,
            permissions: vec![],
            reason: None,
        }
    }

    /// Create an allow response with modified input
    pub fn allow_with_input(input: serde_json::Value) -> Self {
        Self {
            allow: true,
            input: Some(input),
            permissions: vec![],
            reason: None,
        }
    }

    /// Create an allow response with permissions to remember
    pub fn allow_and_remember(permissions: Vec<claude_codes::Permission>) -> Self {
        Self {
            allow: true,
            input: None,
            permissions,
            reason: None,
        }
    }

    /// Create an allow response with input and permissions to remember
    pub fn allow_with_input_and_remember(
        input: serde_json::Value,
        permissions: Vec<claude_codes::Permission>,
    ) -> Self {
        Self {
            allow: true,
            input: Some(input),
            permissions,
            reason: None,
        }
    }

    /// Create a deny response
    pub fn deny() -> Self {
        Self::default()
    }

    /// Create a deny response with a reason
    pub fn deny_with_reason(reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            input: None,
            permissions: vec![],
            reason: Some(reason.into()),
        }
    }
}

/// Internal session state
enum SessionState {
    Running,
    WaitingForPermission {
        #[allow(dead_code)]
        request_id: String,
    },
    Exited {
        code: i32,
    },
}

/// Commands sent to the Claude I/O task
enum IoCommand {
    /// Send user input to Claude
    SendInput(ClaudeInput),
    /// Send a permission response to Claude
    SendPermissionResponse(ControlResponse),
}

/// Events received from the Claude I/O task
enum IoEvent {
    Output(Box<ClaudeOutput>),
    Error(SessionError),
    Exited { code: i32 },
}

/// A managed Claude Code session
///
/// Internally spawns a dedicated I/O task that owns the Claude process and handles
/// both reading stdout and writing stdin. This prevents buffer overflow and avoids
/// deadlocks that would occur if we tried to share the client between tasks with a mutex.
pub struct Session {
    id: Uuid,
    config: SessionConfig,
    /// Channel to send commands (input, permission responses) to the I/O task
    command_tx: Option<mpsc::UnboundedSender<IoCommand>>,
    buffer: OutputBuffer,
    state: SessionState,
    pending_permission: Option<PendingPermission>,
    /// Receiver for events from the I/O task
    event_rx: Option<mpsc::UnboundedReceiver<IoEvent>>,
}

impl Session {
    /// Create a new session (spawns Claude process)
    ///
    /// Spawns a dedicated I/O task that owns the Claude process and handles both
    /// reading stdout and writing stdin, preventing deadlocks and buffer overflow.
    pub async fn new(config: SessionConfig) -> Result<Self, SessionError> {
        let buffer = OutputBuffer::new(config.session_id);
        let client = Self::spawn_claude(&config).await?;

        // Spawn the I/O task that owns the client
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            Self::claude_io_task(client, command_rx, event_tx).await;
        });

        Ok(Self {
            id: config.session_id,
            config,
            command_tx: Some(command_tx),
            buffer,
            state: SessionState::Running,
            pending_permission: None,
            event_rx: Some(event_rx),
        })
    }

    /// Background task that owns the Claude process and handles all I/O.
    ///
    /// This task:
    /// - Continuously reads stdout to prevent OS pipe buffer overflow
    /// - Processes commands from the command channel to send input to Claude
    ///
    /// By owning the client exclusively, we avoid deadlocks that would occur
    /// if we tried to share it between tasks with a mutex.
    async fn claude_io_task(
        mut client: AsyncClient,
        mut command_rx: mpsc::UnboundedReceiver<IoCommand>,
        event_tx: mpsc::UnboundedSender<IoEvent>,
    ) {
        // Take stderr so we can read it if Claude exits unexpectedly
        let mut stderr_reader = client.take_stderr();

        loop {
            tokio::select! {
                // Handle incoming commands (input to send to Claude)
                Some(cmd) = command_rx.recv() => {
                    let result = match cmd {
                        IoCommand::SendInput(input) => client.send(&input).await,
                        IoCommand::SendPermissionResponse(response) => {
                            client.send_control_response(response).await
                        }
                    };
                    if let Err(e) = result {
                        let _ = event_tx.send(IoEvent::Error(SessionError::ClaudeError(e)));
                    }
                }

                // Read output from Claude
                result = client.receive() => {
                    match result {
                        Ok(output) => {
                            if event_tx.send(IoEvent::Output(Box::new(output))).is_err() {
                                // Receiver dropped, session ended
                                break;
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("exit") || err_str.contains("terminated") {
                                let _ = event_tx.send(IoEvent::Exited { code: 1 });
                                break;
                            }
                            // Try to read stderr for more context
                            let stderr_output = Self::read_stderr(&mut stderr_reader).await;
                            let enriched_error = if let Some(stderr) = stderr_output {
                                SessionError::CommunicationError(format!(
                                    "{}\nClaude stderr: {}",
                                    e, stderr
                                ))
                            } else {
                                SessionError::ClaudeError(e)
                            };
                            if event_tx.send(IoEvent::Error(enriched_error)).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Read available stderr output from the Claude process
    async fn read_stderr(
        stderr_reader: &mut Option<tokio::io::BufReader<tokio::process::ChildStderr>>,
    ) -> Option<String> {
        use tokio::io::AsyncReadExt;

        let reader = stderr_reader.as_mut()?;
        let mut buf = Vec::with_capacity(4096);

        // Use a short timeout — stderr may have data already buffered
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            reader.read_to_end(&mut buf),
        )
        .await
        {
            Ok(Ok(_)) if !buf.is_empty() => {
                let text = String::from_utf8_lossy(&buf).trim().to_string();
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            _ => None,
        }
    }

    /// Restore a session from a snapshot
    ///
    /// This restores the buffer and pending permission state,
    /// then spawns a new Claude process with --resume.
    pub async fn restore(snapshot: SessionSnapshot) -> Result<Self, SessionError> {
        let buffer = OutputBuffer::from_snapshot(snapshot.id, snapshot.pending_outputs);

        // Always resume when restoring
        let mut config = snapshot.config;
        config.resume = true;

        let (command_tx, event_rx) = if snapshot.was_running {
            let client = Self::spawn_claude(&config).await?;

            // Spawn the I/O task that owns the client
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            let (command_tx, command_rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                Self::claude_io_task(client, command_rx, event_tx).await;
            });

            (Some(command_tx), Some(event_rx))
        } else {
            (None, None)
        };

        let state = if command_tx.is_some() {
            SessionState::Running
        } else {
            SessionState::Exited { code: 0 }
        };

        Ok(Self {
            id: snapshot.id,
            config,
            command_tx,
            buffer,
            state,
            pending_permission: snapshot.pending_permission,
            event_rx,
        })
    }

    /// Serialize current state for persistence
    pub fn snapshot(&self) -> SessionSnapshot {
        let was_running = matches!(
            self.state,
            SessionState::Running | SessionState::WaitingForPermission { .. }
        );
        SessionSnapshot::new(
            self.id,
            self.config.clone(),
            self.buffer.to_snapshot(),
            self.pending_permission.clone(),
            was_running,
        )
    }

    /// Get the session ID
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Get the session config
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Poll for the next event
    ///
    /// Returns `None` if the session has exited and no more events are available.
    /// Use this in a loop with other async operations via `tokio::select!`.
    ///
    /// Events are delivered from a dedicated I/O task that continuously reads
    /// stdout, so this method will not block other select! branches from being
    /// processed, and stdout will not overflow.
    pub async fn next_event(&mut self) -> Option<SessionEvent> {
        // Loop to skip internal messages (ControlResponse)
        loop {
            let event_rx = self.event_rx.as_mut()?;

            match event_rx.recv().await {
                Some(IoEvent::Output(boxed_output)) => {
                    let output = *boxed_output;

                    // Buffer the output
                    let output_value = serde_json::to_value(&output).unwrap_or_default();
                    self.buffer.push(output_value);

                    // Check for "No conversation found" error (session not found locally)
                    if let ClaudeOutput::Result(ref res) = output {
                        if res.is_error
                            && res
                                .errors
                                .iter()
                                .any(|e| e.contains("No conversation found"))
                        {
                            self.state = SessionState::Exited { code: 1 };
                            self.command_tx = None;
                            self.event_rx = None;
                            return Some(SessionEvent::SessionNotFound);
                        }
                    }

                    // Check for permission requests - emit as PermissionRequest, not Output
                    if let ClaudeOutput::ControlRequest(ref req) = output {
                        if let claude_codes::io::ControlRequestPayload::CanUseTool(ref tool_req) =
                            req.request
                        {
                            let request_id = req.request_id.clone();
                            self.pending_permission = Some(PendingPermission {
                                request_id: request_id.clone(),
                                tool_name: tool_req.tool_name.clone(),
                                input: tool_req.input.clone(),
                                requested_at: Utc::now(),
                            });
                            self.state = SessionState::WaitingForPermission {
                                request_id: request_id.clone(),
                            };

                            // Emit PermissionRequest (not Output) for permission requests
                            return Some(SessionEvent::PermissionRequest {
                                request_id,
                                tool_name: tool_req.tool_name.clone(),
                                input: tool_req.input.clone(),
                                permission_suggestions: tool_req.permission_suggestions.clone(),
                            });
                        }
                    }

                    // Skip ControlResponse (acks from Claude, not useful to callers)
                    if matches!(output, ClaudeOutput::ControlResponse(_)) {
                        // Continue loop to get next event
                        continue;
                    }

                    return Some(SessionEvent::Output(output));
                }
                Some(IoEvent::Exited { code }) => {
                    self.state = SessionState::Exited { code };
                    self.command_tx = None;
                    self.event_rx = None;
                    return Some(SessionEvent::Exited { code });
                }
                Some(IoEvent::Error(e)) => {
                    return Some(SessionEvent::Error(e));
                }
                None => {
                    // Channel closed, I/O task ended
                    self.state = SessionState::Exited { code: 0 };
                    self.command_tx = None;
                    self.event_rx = None;
                    return None;
                }
            }
        }
    }

    /// Send user input to Claude
    ///
    /// The content can be a JSON string value for plain text,
    /// or a more complex JSON structure if needed.
    pub async fn send_input(&mut self, content: serde_json::Value) -> Result<(), SessionError> {
        if let SessionState::Exited { code } = self.state {
            return Err(SessionError::AlreadyExited(code));
        }

        if let Some(ref command_tx) = self.command_tx {
            // Extract string content or serialize to string
            let text = match &content {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let input = ClaudeInput::user_message(text, self.id);
            command_tx
                .send(IoCommand::SendInput(input))
                .map_err(|_| SessionError::CommunicationError("I/O task closed".to_string()))?;
        }

        Ok(())
    }

    /// Respond to a permission request
    ///
    /// Supports simple allow/deny as well as "remember this decision" with permissions.
    pub async fn respond_permission(
        &mut self,
        request_id: &str,
        response: PermissionResponse,
    ) -> Result<(), SessionError> {
        // Verify this is the pending request
        match &self.pending_permission {
            Some(perm) if perm.request_id == request_id => {}
            _ => {
                return Err(SessionError::InvalidPermissionResponse(
                    request_id.to_string(),
                ));
            }
        }

        if let Some(ref command_tx) = self.command_tx {
            let input_value = response
                .input
                .unwrap_or(serde_json::Value::Object(Default::default()));

            let ctrl_response = if response.allow {
                if response.permissions.is_empty() {
                    // Simple allow
                    ControlResponse::from_result(request_id, PermissionResult::allow(input_value))
                } else {
                    // Allow with permissions to remember
                    ControlResponse::from_result(
                        request_id,
                        PermissionResult::allow_with_typed_permissions(
                            input_value,
                            response.permissions,
                        ),
                    )
                }
            } else {
                // Deny with optional reason
                let reason = response.reason.unwrap_or_else(|| "User denied".to_string());
                ControlResponse::from_result(request_id, PermissionResult::deny(reason))
            };

            command_tx
                .send(IoCommand::SendPermissionResponse(ctrl_response))
                .map_err(|_| SessionError::CommunicationError("I/O task closed".to_string()))?;
        }

        self.pending_permission = None;
        self.state = SessionState::Running;

        Ok(())
    }

    /// Gracefully stop the session
    pub async fn stop(&mut self) -> Result<(), SessionError> {
        // Dropping the command_tx will cause the I/O task to exit,
        // which in turn will drop the client and terminate the process
        self.command_tx = None;
        self.event_rx = None;
        self.state = SessionState::Exited { code: 0 };
        Ok(())
    }

    /// Check if session is still running
    pub fn is_running(&self) -> bool {
        matches!(
            self.state,
            SessionState::Running | SessionState::WaitingForPermission { .. }
        )
    }

    /// Check if session has a pending permission request
    pub fn has_pending_permission(&self) -> bool {
        self.pending_permission.is_some()
    }

    /// Get the pending permission request, if any
    pub fn pending_permission(&self) -> Option<&PendingPermission> {
        self.pending_permission.as_ref()
    }

    /// Acknowledge outputs up to the given sequence number
    pub fn ack_outputs(&mut self, seq: u64) {
        self.buffer.ack(seq);
    }

    /// Get pending output count
    pub fn pending_output_count(&self) -> usize {
        self.buffer.pending_count()
    }

    /// Log the resolved path and version of the claude binary for diagnostics.
    fn log_claude_info(claude_path: &Path) {
        if let Ok(full_path) = which::which(claude_path) {
            tracing::info!("Claude binary: {}", full_path.display());
        } else {
            tracing::warn!(
                "Could not resolve full path for '{}' — using PATH lookup",
                claude_path.display()
            );
        }

        match std::process::Command::new(claude_path)
            .arg("--version")
            .output()
        {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                tracing::info!("Claude version: {}", version.trim());
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("claude --version failed: {}", stderr.trim());
            }
            Err(e) => {
                tracing::warn!("Failed to run claude --version: {}", e);
            }
        }
    }

    /// Spawn the Claude process
    async fn spawn_claude(config: &SessionConfig) -> Result<AsyncClient, SessionError> {
        let claude_path = config.claude_path.as_deref().unwrap_or(Path::new("claude"));

        Self::log_claude_info(claude_path);

        let mut cmd = Command::new(claude_path);
        cmd.arg("--print")
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--permission-prompt-tool")
            .arg("stdio")
            .arg("--replay-user-messages");

        if config.resume {
            cmd.arg("--resume").arg(config.session_id.to_string());
        } else {
            cmd.arg("--session-id").arg(config.session_id.to_string());
        }

        // Add extra arguments
        for arg in &config.extra_args {
            cmd.arg(arg);
        }

        cmd.current_dir(&config.working_directory);

        // Log the full command
        let args: Vec<_> = std::iter::once(claude_path.to_string_lossy().to_string())
            .chain(
                [
                    "--print",
                    "--verbose",
                    "--output-format",
                    "stream-json",
                    "--input-format",
                    "stream-json",
                    "--permission-prompt-tool",
                    "stdio",
                    "--replay-user-messages",
                ]
                .iter()
                .map(|s| s.to_string()),
            )
            .chain(if config.resume {
                vec!["--resume".to_string(), config.session_id.to_string()]
            } else {
                vec!["--session-id".to_string(), config.session_id.to_string()]
            })
            .chain(config.extra_args.iter().cloned())
            .collect();
        tracing::info!("Spawning Claude: {}", args.join(" "));

        // Configure stdio
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd.spawn().map_err(SessionError::SpawnFailed)?;

        AsyncClient::new(child).map_err(|e| {
            SessionError::CommunicationError(format!("Failed to create AsyncClient: {}", e))
        })
    }
}
