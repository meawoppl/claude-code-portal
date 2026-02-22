use serde::{Deserialize, Serialize};
use uuid::Uuid;
pub use ws_bridge::WsEndpoint;

use crate::{
    AgentType, DirectoryEntry, PermissionSuggestion, SendMode, SessionCost, SessionStatus,
};

// =============================================================================
// Session endpoint: proxy <-> backend (/ws/session)
// =============================================================================

pub struct SessionEndpoint;

impl WsEndpoint for SessionEndpoint {
    const PATH: &'static str = "/ws/session";
    type ServerMsg = ServerToProxy;
    type ClientMsg = ProxyToServer;
}

/// Messages the proxy sends to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyToServer {
    /// Register a new session or connect to an existing one
    Register {
        session_id: Uuid,
        session_name: String,
        auth_token: Option<String>,
        working_directory: String,
        #[serde(default)]
        resuming: bool,
        #[serde(default)]
        git_branch: Option<String>,
        #[serde(default)]
        replay_after: Option<String>,
        #[serde(default)]
        client_version: Option<String>,
        #[serde(default)]
        replaces_session_id: Option<Uuid>,
        #[serde(default)]
        hostname: Option<String>,
        #[serde(default)]
        launcher_id: Option<Uuid>,
        #[serde(default)]
        agent_type: AgentType,
    },

    /// Raw output from Claude Code (unsequenced fallback)
    ClaudeOutput { content: serde_json::Value },

    /// Sequenced output from Claude Code
    SequencedOutput {
        seq: u64,
        content: serde_json::Value,
    },

    /// Keepalive heartbeat
    Heartbeat,

    /// Permission request from Claude (tool wants to execute)
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permission_suggestions: Vec<PermissionSuggestion>,
    },

    /// Update session metadata (e.g., git branch changed)
    SessionUpdate {
        session_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        git_branch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        pr_url: Option<String>,
    },

    /// Acknowledge receipt of input messages
    InputAck { session_id: Uuid, ack_seq: i64 },

    /// Session status update
    SessionStatus { status: SessionStatus },
}

/// Messages the backend sends to the proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerToProxy {
    /// Acknowledge session registration
    RegisterAck {
        success: bool,
        session_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Keepalive heartbeat
    Heartbeat,

    /// User input (unsequenced fallback)
    ClaudeInput {
        content: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        send_mode: Option<SendMode>,
    },

    /// Sequenced user input
    SequencedInput {
        session_id: Uuid,
        seq: i64,
        content: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        send_mode: Option<SendMode>,
    },

    /// User's permission decision
    PermissionResponse {
        request_id: String,
        allow: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permissions: Vec<PermissionSuggestion>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Acknowledge receipt of output messages
    OutputAck { session_id: Uuid, ack_seq: u64 },

    /// File uploaded by user, to be written to working directory
    FileUpload {
        filename: String,
        /// File content as base64-encoded string
        data: String,
        content_type: String,
    },

    /// Server is shutting down
    ServerShutdown {
        reason: String,
        reconnect_delay_ms: u64,
    },
}

// =============================================================================
// Client endpoint: frontend <-> backend (/ws/client)
// =============================================================================

pub struct ClientEndpoint;

impl WsEndpoint for ClientEndpoint {
    const PATH: &'static str = "/ws/client";
    type ServerMsg = ServerToClient;
    type ClientMsg = ClientToServer;
}

/// Messages the frontend sends to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientToServer {
    /// Register to receive updates for a session
    Register {
        session_id: Uuid,
        session_name: String,
        auth_token: Option<String>,
        working_directory: String,
        #[serde(default)]
        resuming: bool,
        #[serde(default)]
        git_branch: Option<String>,
        #[serde(default)]
        replay_after: Option<String>,
        #[serde(default)]
        client_version: Option<String>,
        #[serde(default)]
        replaces_session_id: Option<Uuid>,
        #[serde(default)]
        hostname: Option<String>,
        #[serde(default)]
        launcher_id: Option<Uuid>,
        #[serde(default)]
        agent_type: AgentType,
    },

    /// User sends input to Claude
    ClaudeInput {
        content: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        send_mode: Option<SendMode>,
    },

    /// User's permission decision
    PermissionResponse {
        request_id: String,
        allow: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permissions: Vec<PermissionSuggestion>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Start a chunked file upload
    FileUploadStart {
        upload_id: String,
        filename: String,
        content_type: String,
        total_chunks: u32,
    },

    /// A single chunk of a file upload
    FileUploadChunk {
        upload_id: String,
        chunk_index: u32,
        /// Base64-encoded chunk data (~1KB decoded per chunk)
        data: String,
    },
}

/// Messages the backend sends to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerToClient {
    /// Output from Claude Code
    ClaudeOutput { content: serde_json::Value },

    /// Batch of historical messages for replay
    HistoryBatch { messages: Vec<serde_json::Value> },

    /// Permission request from Claude (tool wants to execute)
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permission_suggestions: Vec<PermissionSuggestion>,
    },

    /// Error message
    Error { message: String },

    /// Session metadata changed
    SessionUpdate {
        session_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        git_branch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        pr_url: Option<String>,
    },

    /// User spend data update
    UserSpendUpdate {
        total_spend_usd: f64,
        session_costs: Vec<SessionCost>,
    },

    /// Server is shutting down
    ServerShutdown {
        reason: String,
        reconnect_delay_ms: u64,
    },

    /// Session status changed
    SessionStatus { status: SessionStatus },

    /// Launch session result (forwarded from launcher)
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

    /// A proxy process exited (forwarded from launcher)
    SessionExited {
        session_id: Uuid,
        exit_code: Option<i32>,
    },
}

// =============================================================================
// Launcher endpoint: launcher <-> backend (/ws/launcher)
// =============================================================================

pub struct LauncherEndpoint;

impl WsEndpoint for LauncherEndpoint {
    const PATH: &'static str = "/ws/launcher";
    type ServerMsg = ServerToLauncher;
    type ClientMsg = LauncherToServer;
}

/// Messages the launcher sends to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LauncherToServer {
    /// Register a launcher daemon
    LauncherRegister {
        launcher_id: Uuid,
        launcher_name: String,
        auth_token: Option<String>,
        hostname: String,
        #[serde(default)]
        version: Option<String>,
    },

    /// Result of a launch request
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

    /// Launcher heartbeat with running process summary
    LauncherHeartbeat {
        launcher_id: Uuid,
        running_sessions: Vec<Uuid>,
        uptime_secs: u64,
    },

    /// Log output from a proxy process
    ProxyLog {
        session_id: Uuid,
        level: String,
        message: String,
        timestamp: String,
    },

    /// A proxy process exited
    SessionExited {
        session_id: Uuid,
        exit_code: Option<i32>,
    },

    /// Directory listing response
    ListDirectoriesResult {
        request_id: Uuid,
        #[serde(default)]
        entries: Vec<DirectoryEntry>,
        error: Option<String>,
        #[serde(default)]
        resolved_path: Option<String>,
    },
}

/// Messages the backend sends to the launcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerToLauncher {
    /// Acknowledge launcher registration
    LauncherRegisterAck {
        success: bool,
        launcher_id: Uuid,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Request to launch a new proxy instance
    LaunchSession {
        request_id: Uuid,
        user_id: Uuid,
        auth_token: String,
        working_directory: String,
        #[serde(default)]
        session_name: Option<String>,
        #[serde(default)]
        claude_args: Vec<String>,
        #[serde(default)]
        agent_type: AgentType,
    },

    /// Request to stop a running session
    StopSession { session_id: Uuid },

    /// Request directory listing
    ListDirectories { request_id: Uuid, path: String },

    /// Server is shutting down
    ServerShutdown {
        reason: String,
        reconnect_delay_ms: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_endpoint_path() {
        assert_eq!(SessionEndpoint::PATH, "/ws/session");
    }

    #[test]
    fn client_endpoint_path() {
        assert_eq!(ClientEndpoint::PATH, "/ws/client");
    }

    #[test]
    fn launcher_endpoint_path() {
        assert_eq!(LauncherEndpoint::PATH, "/ws/launcher");
    }

    #[test]
    fn proxy_to_server_register_roundtrip() {
        let msg = ProxyToServer::Register {
            session_id: Uuid::nil(),
            session_name: "test".into(),
            auth_token: None,
            working_directory: "/tmp".into(),
            resuming: false,
            git_branch: None,
            replay_after: None,
            client_version: None,
            replaces_session_id: None,
            hostname: None,
            launcher_id: None,
            agent_type: AgentType::Claude,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"Register""#));
        let parsed: ProxyToServer = serde_json::from_str(&json).unwrap();
        match parsed {
            ProxyToServer::Register { session_name, .. } => {
                assert_eq!(session_name, "test");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn server_to_proxy_sequenced_input_roundtrip() {
        let msg = ServerToProxy::SequencedInput {
            session_id: Uuid::nil(),
            seq: 5,
            content: serde_json::json!({"text": "hello"}),
            send_mode: Some(SendMode::Wiggum),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"SequencedInput""#));
        let parsed: ServerToProxy = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerToProxy::SequencedInput { seq, send_mode, .. } => {
                assert_eq!(seq, 5);
                assert_eq!(send_mode, Some(SendMode::Wiggum));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn client_to_server_claude_input_roundtrip() {
        let msg = ClientToServer::ClaudeInput {
            content: serde_json::json!({"text": "hi"}),
            send_mode: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"ClaudeInput""#));
        let parsed: ClientToServer = serde_json::from_str(&json).unwrap();
        match parsed {
            ClientToServer::ClaudeInput { send_mode, .. } => {
                assert!(send_mode.is_none());
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn server_to_client_output_roundtrip() {
        let msg = ServerToClient::ClaudeOutput {
            content: serde_json::json!({"type": "assistant", "text": "hello"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ServerToClient = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerToClient::ClaudeOutput { content } => {
                assert_eq!(content["text"], "hello");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn launcher_to_server_register_roundtrip() {
        let msg = LauncherToServer::LauncherRegister {
            launcher_id: Uuid::nil(),
            launcher_name: "test-launcher".into(),
            auth_token: Some("tok".into()),
            hostname: "host1".into(),
            version: Some("1.0".into()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"LauncherRegister""#));
        let parsed: LauncherToServer = serde_json::from_str(&json).unwrap();
        match parsed {
            LauncherToServer::LauncherRegister { launcher_name, .. } => {
                assert_eq!(launcher_name, "test-launcher");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn server_to_launcher_launch_roundtrip() {
        let msg = ServerToLauncher::LaunchSession {
            request_id: Uuid::nil(),
            user_id: Uuid::nil(),
            auth_token: "token".into(),
            working_directory: "/home".into(),
            session_name: Some("my-session".into()),
            claude_args: vec!["--verbose".into()],
            agent_type: AgentType::Claude,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"LaunchSession""#));
        let parsed: ServerToLauncher = serde_json::from_str(&json).unwrap();
        match parsed {
            ServerToLauncher::LaunchSession {
                working_directory,
                claude_args,
                ..
            } => {
                assert_eq!(working_directory, "/home");
                assert_eq!(claude_args, vec!["--verbose"]);
            }
            _ => panic!("Wrong variant"),
        }
    }

    /// Verify wire-format compatibility of per-endpoint types.
    #[test]
    fn wire_compat_register() {
        // Register JSON format
        let json = r#"{
            "type": "Register",
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "session_name": "test",
            "auth_token": null,
            "working_directory": "/tmp"
        }"#;
        // Must parse as both ProxyToServer and ClientToServer
        let _: ProxyToServer = serde_json::from_str(json).unwrap();
        let _: ClientToServer = serde_json::from_str(json).unwrap();
    }

    #[test]
    fn wire_compat_server_shutdown() {
        let json = r#"{"type":"ServerShutdown","reason":"update","reconnect_delay_ms":5000}"#;
        // Must parse in all three server->X enums
        let _: ServerToProxy = serde_json::from_str(json).unwrap();
        let _: ServerToClient = serde_json::from_str(json).unwrap();
        let _: ServerToLauncher = serde_json::from_str(json).unwrap();
    }
}
