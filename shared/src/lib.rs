use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Proxy token types in separate module
pub mod proxy_tokens;
pub use proxy_tokens::*;

// API client types and trait
pub mod api;
pub use api::{ApiClientConfig, ApiError, CcProxyApi};

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
    },

    /// Output from Claude Code to be displayed
    ClaudeOutput { content: serde_json::Value },

    /// Input to Claude Code from user
    ClaudeInput { content: serde_json::Value },

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
        permission_suggestions: Vec<serde_json::Value>,
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
        permissions: Vec<serde_json::Value>,
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

/// API types for HTTP endpoints
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_name: String,
    pub session_key: String,
    pub working_directory: Option<String>,
    pub status: SessionStatus,
    pub last_activity: String,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: Uuid,
    pub role: String,
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
