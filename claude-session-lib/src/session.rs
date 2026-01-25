//! Core session management

use chrono::Utc;
use claude_codes::io::{ControlResponse, PermissionResult};
use claude_codes::{AsyncClient, ClaudeInput, ClaudeOutput};
use std::path::Path;
use tokio::process::Command;
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
}

impl PermissionResponse {
    /// Create an allow response
    pub fn allow() -> Self {
        Self {
            allow: true,
            input: None,
        }
    }

    /// Create an allow response with modified input
    pub fn allow_with_input(input: serde_json::Value) -> Self {
        Self {
            allow: true,
            input: Some(input),
        }
    }

    /// Create a deny response
    pub fn deny() -> Self {
        Self::default()
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

/// A managed Claude Code session
pub struct Session {
    id: Uuid,
    config: SessionConfig,
    client: Option<AsyncClient>,
    buffer: OutputBuffer,
    state: SessionState,
    pending_permission: Option<PendingPermission>,
}

impl Session {
    /// Create a new session (spawns Claude process)
    pub async fn new(config: SessionConfig) -> Result<Self, SessionError> {
        let buffer = OutputBuffer::new(config.session_id);
        let client = Self::spawn_claude(&config).await?;

        Ok(Self {
            id: config.session_id,
            config,
            client: Some(client),
            buffer,
            state: SessionState::Running,
            pending_permission: None,
        })
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

        let client = if snapshot.was_running {
            Some(Self::spawn_claude(&config).await?)
        } else {
            None
        };

        let state = if client.is_some() {
            SessionState::Running
        } else {
            SessionState::Exited { code: 0 }
        };

        Ok(Self {
            id: snapshot.id,
            config,
            client,
            buffer,
            state,
            pending_permission: snapshot.pending_permission,
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
    pub async fn next_event(&mut self) -> Option<SessionEvent> {
        // Loop to skip internal messages (ControlResponse)
        loop {
            // Poll Claude for output
            let client = self.client.as_mut()?;

            match client.receive().await {
                Ok(output) => {
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
                            self.client = None;
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
                Err(e) => {
                    // Check if process exited
                    let err_str = e.to_string();
                    if err_str.contains("exit") || err_str.contains("terminated") {
                        self.state = SessionState::Exited { code: 1 };
                        self.client = None;
                        return Some(SessionEvent::Exited { code: 1 });
                    }
                    return Some(SessionEvent::Error(SessionError::ClaudeError(e)));
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

        if let Some(ref mut client) = self.client {
            // Extract string content or serialize to string
            let text = match &content {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let input = ClaudeInput::user_message(text, self.id);
            client
                .send(&input)
                .await
                .map_err(SessionError::ClaudeError)?;
        }

        Ok(())
    }

    /// Respond to a permission request
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

        if let Some(ref mut client) = self.client {
            let input_value = response.input.unwrap_or(serde_json::Value::Null);
            let ctrl_response = if response.allow {
                ControlResponse::from_result(request_id, PermissionResult::allow(input_value))
            } else {
                ControlResponse::from_result(
                    request_id,
                    PermissionResult::deny("User denied".to_string()),
                )
            };

            client
                .send_control_response(ctrl_response)
                .await
                .map_err(SessionError::ClaudeError)?;
        }

        self.pending_permission = None;
        self.state = SessionState::Running;

        Ok(())
    }

    /// Send a raw control response to Claude
    ///
    /// This is an advanced method for cases where the simple `respond_permission`
    /// doesn't provide enough control (e.g., sending permissions to remember).
    /// The caller is responsible for ensuring the request_id matches a pending request.
    pub async fn send_raw_control_response(
        &mut self,
        request_id: &str,
        ctrl_response: ControlResponse,
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

        if let Some(ref mut client) = self.client {
            client
                .send_control_response(ctrl_response)
                .await
                .map_err(SessionError::ClaudeError)?;
        }

        self.pending_permission = None;
        self.state = SessionState::Running;

        Ok(())
    }

    /// Gracefully stop the session
    pub async fn stop(&mut self) -> Result<(), SessionError> {
        if let Some(client) = self.client.take() {
            drop(client); // This should terminate the process
        }
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

    /// Spawn the Claude process
    async fn spawn_claude(config: &SessionConfig) -> Result<AsyncClient, SessionError> {
        let claude_path = config.claude_path.as_deref().unwrap_or(Path::new("claude"));

        let mut cmd = Command::new(claude_path);
        cmd.arg("--print")
            .arg("--verbose")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--permission-prompt-tool")
            .arg("stdio");

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
