use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Message types for the WebSocket proxy protocol
/// These are used to communicate between:
/// - proxy <-> backend (session connection)
/// - frontend <-> backend (web client connection)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyMessage {
    /// Register a new session or connect to an existing one
    Register {
        session_name: String,
        auth_token: Option<String>,
        working_directory: String,
    },

    /// Output from Claude Code to be displayed
    ClaudeOutput {
        content: serde_json::Value,
    },

    /// Input to Claude Code from user
    ClaudeInput {
        content: serde_json::Value,
    },

    /// Heartbeat to keep connection alive
    Heartbeat,

    /// Error message
    Error {
        message: String,
    },

    /// Session status update
    SessionStatus {
        status: SessionStatus,
    },
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
    pub session_name: String,
    pub working_directory: Option<String>,
    pub status: SessionStatus,
    pub last_activity: String,
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
