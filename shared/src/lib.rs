use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Proxy token types in separate module
pub mod proxy_tokens;
pub use proxy_tokens::*;

// Protocol constants shared between backend and proxy
pub mod protocol;

// API client types and trait
pub mod api;
pub use api::{ApiClientConfig, ApiError, CcProxyApi};

// Re-export claude-codes types for frontend message parsing
pub use claude_codes::io::{
    ContentBlock, ImageBlock, ImageSource, PermissionSuggestion, TextBlock, ThinkingBlock,
    ToolResultBlock, ToolResultContent, ToolUseBlock,
};

/// Message types for the WebSocket proxy protocol
/// These are used to communicate between:
/// - proxy <-> backend (session connection)
/// - frontend <-> backend (web client connection)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyMessage {
    /// Register a new session or connect to an existing one
    Register {
        /// The Claude Code session ID (UUID) - used as primary key
        session_id: Uuid,
        /// Human-readable session name for display
        session_name: String,
        /// JWT auth token for user authentication
        auth_token: Option<String>,
        /// Working directory where the session was started
        working_directory: String,
        /// Whether this is resuming an existing session
        #[serde(default)]
        resuming: bool,
        /// Current git branch (if in a git repo)
        #[serde(default)]
        git_branch: Option<String>,
        /// Only replay messages created after this timestamp (ISO 8601 format)
        /// If None, replay all history. Used by web clients to avoid duplicate messages.
        #[serde(default)]
        replay_after: Option<String>,
        /// Client version (e.g., "1.0.0") - helps track client versions in use
        #[serde(default)]
        client_version: Option<String>,
        /// If this session replaces a previous one (e.g. after SessionNotFound),
        /// the old session ID so the backend can mark it as replaced.
        #[serde(default)]
        replaces_session_id: Option<Uuid>,
    },

    /// Output from Claude Code to be displayed
    ClaudeOutput { content: serde_json::Value },

    /// Input to Claude Code from user
    ClaudeInput {
        content: serde_json::Value,
        /// Optional send mode (normal, wiggum)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        send_mode: Option<SendMode>,
    },

    /// Heartbeat to keep connection alive
    Heartbeat,

    /// Error message
    Error { message: String },

    /// Session status update
    SessionStatus { status: SessionStatus },

    /// Permission request from Claude (tool wants to execute)
    PermissionRequest {
        /// Unique ID for this permission request (to correlate responses)
        request_id: String,
        /// Name of the tool requesting permission
        tool_name: String,
        /// Tool input parameters
        input: serde_json::Value,
        /// Suggested permissions to grant (for "allow & remember" feature)
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permission_suggestions: Vec<PermissionSuggestion>,
    },

    /// Permission response from user
    PermissionResponse {
        /// The request_id this responds to
        request_id: String,
        /// Whether to allow the tool
        allow: bool,
        /// The original tool input (required when allow=true)
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        /// Permissions to grant for future similar operations
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permissions: Vec<PermissionSuggestion>,
        /// Optional reason for denial
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Backend acknowledgment of session registration
    RegisterAck {
        /// Whether registration succeeded
        success: bool,
        /// The session ID that was registered
        session_id: Uuid,
        /// Error message if registration failed
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Update session metadata (e.g., git branch changed)
    SessionUpdate {
        /// The session ID to update
        session_id: Uuid,
        /// Updated git branch (if changed)
        #[serde(skip_serializing_if = "Option::is_none")]
        git_branch: Option<String>,
    },

    /// User spend update (sent to web clients periodically)
    UserSpendUpdate {
        /// Total spend across all sessions for this user
        total_spend_usd: f64,
        /// Per-session spend breakdown
        session_costs: Vec<SessionCost>,
    },

    /// Batch of historical messages sent during replay (backend -> web client).
    /// Sent as a single message to avoid per-message rendering overhead.
    HistoryBatch { messages: Vec<serde_json::Value> },

    /// Sequenced output from Claude Code (proxy -> backend)
    /// Messages are held in proxy buffer until acknowledged
    SequencedOutput {
        /// Monotonic sequence number for this output
        seq: u64,
        /// The actual output content
        content: serde_json::Value,
    },

    /// Acknowledge receipt of output messages (backend -> proxy)
    /// All messages with seq <= ack_seq are confirmed stored
    OutputAck {
        /// The session this acknowledgment is for
        session_id: Uuid,
        /// All messages with sequence <= this are confirmed received
        ack_seq: u64,
    },

    /// Sequenced input from frontend (frontend -> backend -> proxy)
    /// Backend stores these until proxy acknowledges receipt
    SequencedInput {
        /// The session this input is for
        session_id: Uuid,
        /// Monotonic sequence number for this input
        seq: i64,
        /// The actual input content
        content: serde_json::Value,
        /// Send mode (normal or wiggum loop)
        #[serde(skip_serializing_if = "Option::is_none")]
        send_mode: Option<SendMode>,
    },

    /// Acknowledge receipt of input messages (proxy -> backend)
    /// Backend removes pending inputs with seq <= ack_seq
    InputAck {
        /// The session this acknowledgment is for
        session_id: Uuid,
        /// All inputs with sequence <= this are confirmed received
        ack_seq: i64,
    },

    // =========================================================================
    // Voice Input Messages (frontend <-> backend)
    // =========================================================================
    /// Start voice recording session (frontend -> backend)
    StartVoice {
        /// The session to associate voice input with
        session_id: Uuid,
        /// Language code for speech recognition (default: "en-US")
        #[serde(default = "default_language_code")]
        language_code: String,
    },

    /// Stop voice recording (frontend -> backend)
    StopVoice {
        /// The session to stop voice input for
        session_id: Uuid,
    },

    /// Transcription result from speech-to-text (backend -> frontend)
    Transcription {
        /// The session this transcription is for
        session_id: Uuid,
        /// The transcribed text
        transcript: String,
        /// Whether this is a final result (vs interim/partial)
        is_final: bool,
        /// Confidence score (0.0 to 1.0)
        confidence: f32,
    },

    /// Voice error (backend -> frontend)
    VoiceError {
        /// The session this error is for
        session_id: Uuid,
        /// Error message
        message: String,
    },

    /// Voice session ended (backend -> frontend)
    /// Sent when speech recognition detects end of speech (single_utterance mode)
    VoiceEnded {
        /// The session that ended
        session_id: Uuid,
    },

    /// Server is shutting down (backend -> all clients)
    /// Sent to all connected WebSocket clients before graceful shutdown
    ServerShutdown {
        /// Human-readable reason for shutdown (e.g., "Server restarting for update")
        reason: String,
        /// Suggested delay before reconnecting (milliseconds)
        reconnect_delay_ms: u64,
    },

    // =========================================================================
    // Launcher Messages (launcher <-> backend)
    // =========================================================================
    /// Register a launcher daemon with the backend
    LauncherRegister {
        launcher_id: Uuid,
        launcher_name: String,
        auth_token: Option<String>,
        hostname: String,
        #[serde(default)]
        version: Option<String>,
    },

    /// Backend acknowledges launcher registration
    LauncherRegisterAck {
        success: bool,
        launcher_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Request to launch a new proxy instance (backend -> launcher)
    LaunchSession {
        request_id: Uuid,
        user_id: Uuid,
        auth_token: String,
        working_directory: String,
        #[serde(default)]
        session_name: Option<String>,
        #[serde(default)]
        claude_args: Vec<String>,
    },

    /// Response from launcher about a launch request (launcher -> backend)
    LaunchSessionResult {
        request_id: Uuid,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Request to stop a running session (backend -> launcher)
    StopSession { session_id: Uuid },

    /// Launcher heartbeat with summary of running processes
    LauncherHeartbeat {
        launcher_id: Uuid,
        running_sessions: Vec<Uuid>,
        uptime_secs: u64,
    },
}

fn default_language_code() -> String {
    "en-US".to_string()
}

/// Cost information for a single session
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionCost {
    pub session_id: Uuid,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Active,
    Inactive,
    Disconnected,
}

impl SessionStatus {
    pub fn as_str(&self) -> &str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Inactive => "inactive",
            SessionStatus::Disconnected => "disconnected",
        }
    }
}

/// Send mode for user input
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SendMode {
    /// Normal single message send
    #[default]
    Normal,
    /// Wiggum mode - iterative autonomous loop until completion
    /// Proxy will re-send the prompt after each result until Claude responds with "DONE"
    Wiggum,
}

/// Info about a connected launcher daemon
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LauncherInfo {
    pub launcher_id: Uuid,
    pub launcher_name: String,
    pub hostname: String,
    pub connected: bool,
    pub running_sessions: u32,
}

/// API types for HTTP endpoints
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_name: String,
    pub session_key: String,
    pub working_directory: String,
    pub status: SessionStatus,
    pub last_activity: String,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    /// The current user's role in this session (owner, editor, viewer)
    pub my_role: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    /// Whether voice input is enabled for this user (admin-controlled)
    #[serde(default)]
    pub voice_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    Assistant,
    User,
    Result,
    Error,
    Portal,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::System => "system",
            Self::Assistant => "assistant",
            Self::User => "user",
            Self::Result => "result",
            Self::Error => "error",
            Self::Portal => "portal",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

impl MessageRole {
    pub fn from_type_str(s: &str) -> Self {
        match s {
            "system" => Self::System,
            "assistant" => Self::Assistant,
            "user" => Self::User,
            "result" => Self::Result,
            "error" => Self::Error,
            "portal" => Self::Portal,
            _ => Self::Unknown,
        }
    }
}

/// A portal-originated message that can carry text or images.
/// Serializes with `"type": "portal"` for the frontend's `ClaudeMessage` enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortalMessage {
    /// Always "portal" — used as the serde tag for ClaudeMessage dispatch
    #[serde(rename = "type")]
    pub message_type: String,
    pub content: Vec<PortalContent>,
}

impl PortalMessage {
    pub fn text(text: String) -> Self {
        Self {
            message_type: "portal".to_string(),
            content: vec![PortalContent::Text { text }],
        }
    }

    pub fn image(media_type: String, data: String) -> Self {
        Self {
            message_type: "portal".to_string(),
            content: vec![PortalContent::Image { media_type, data }],
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PortalContent {
    Text { text: String },
    Image { media_type: String, data: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: Uuid,
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
}

// ============================================================================
// Device Flow Types (shared between backend and proxy)
// ============================================================================

/// Request to poll for device flow completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePollRequest {
    pub device_code: String,
}

/// Response from device flow polling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum DevicePollResponse {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "complete")]
    Complete {
        access_token: String,
        user_id: String,
        user_email: String,
    },
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "denied")]
    Denied,
}

// ============================================================================
// App Configuration (served to frontend)
// ============================================================================

/// Application configuration returned by /api/config endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Custom title for the app (displayed in top bar)
    /// Defaults to "Claude Code Sessions" if not configured
    pub app_title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequenced_output_roundtrip() {
        let msg = ProxyMessage::SequencedOutput {
            seq: 42,
            content: serde_json::json!({"type": "assistant", "text": "hello"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProxyMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ProxyMessage::SequencedOutput { seq, content } => {
                assert_eq!(seq, 42);
                assert_eq!(content["type"], "assistant");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn output_ack_roundtrip() {
        let session_id = Uuid::new_v4();
        let msg = ProxyMessage::OutputAck {
            session_id,
            ack_seq: 99,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProxyMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ProxyMessage::OutputAck {
                session_id: sid,
                ack_seq,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(ack_seq, 99);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn sequenced_input_roundtrip() {
        let session_id = Uuid::new_v4();
        let msg = ProxyMessage::SequencedInput {
            session_id,
            seq: 5,
            content: serde_json::json!({"type": "human", "message": "test"}),
            send_mode: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProxyMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ProxyMessage::SequencedInput {
                session_id: sid,
                seq,
                content,
                send_mode,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(seq, 5);
                assert_eq!(content["type"], "human");
                assert!(send_mode.is_none());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn input_ack_roundtrip() {
        let session_id = Uuid::new_v4();
        let msg = ProxyMessage::InputAck {
            session_id,
            ack_seq: 10,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProxyMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ProxyMessage::InputAck {
                session_id: sid,
                ack_seq,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(ack_seq, 10);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn permission_request_roundtrip() {
        let msg = ProxyMessage::PermissionRequest {
            request_id: "req-123".to_string(),
            tool_name: "Bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
            permission_suggestions: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProxyMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ProxyMessage::PermissionRequest {
                request_id,
                tool_name,
                ..
            } => {
                assert_eq!(request_id, "req-123");
                assert_eq!(tool_name, "Bash");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn claude_input_with_send_mode() {
        let msg = ProxyMessage::ClaudeInput {
            content: serde_json::json!({"text": "hello"}),
            send_mode: Some(SendMode::Wiggum),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ProxyMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ProxyMessage::ClaudeInput { send_mode, .. } => {
                assert_eq!(send_mode, Some(SendMode::Wiggum));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn claude_input_default_send_mode() {
        // When send_mode is absent, it should default to None
        let json = r#"{"type":"ClaudeInput","content":{"text":"hi"}}"#;
        let parsed: ProxyMessage = serde_json::from_str(json).unwrap();

        match parsed {
            ProxyMessage::ClaudeInput { send_mode, .. } => {
                assert_eq!(send_mode, None);
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn register_with_defaults() {
        // Fields with #[serde(default)] should work when absent
        let json = r#"{
            "type": "Register",
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "session_name": "test",
            "auth_token": null,
            "working_directory": "/tmp"
        }"#;
        let parsed: ProxyMessage = serde_json::from_str(json).unwrap();

        match parsed {
            ProxyMessage::Register {
                resuming,
                git_branch,
                replay_after,
                client_version,
                replaces_session_id,
                ..
            } => {
                assert!(!resuming);
                assert!(git_branch.is_none());
                assert!(replay_after.is_none());
                assert!(client_version.is_none());
                assert!(replaces_session_id.is_none());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn session_status_serialization() {
        assert_eq!(SessionStatus::Active.as_str(), "active");
        assert_eq!(SessionStatus::Inactive.as_str(), "inactive");
        assert_eq!(SessionStatus::Disconnected.as_str(), "disconnected");

        let json = serde_json::to_string(&SessionStatus::Active).unwrap();
        assert_eq!(json, "\"active\"");
    }
}
