# Proxy Library Extraction Plan

This document outlines the plan to extract the session wrapping logic from `claude-portal`'s proxy into a reusable library crate.

## Goals

1. Create a standalone `claude-proxy-lib` crate that can wrap Claude Code sessions
2. Allow other projects to embed Claude Code session management without the portal
3. Maintain backward compatibility with the existing portal proxy

## Current Architecture

```
proxy/
├── main.rs          (581 lines)  CLI entry point, session init
├── session.rs       (1,212 lines) Core forwarding logic
├── output_buffer.rs (358 lines)  Persistent message buffer
├── config.rs        (276 lines)  Config file management
├── auth.rs          (183 lines)  Device flow OAuth
├── ui.rs            (341 lines)  Terminal UI output
├── update.rs        (323 lines)  GitHub auto-updates
├── commands.rs      (74 lines)   --init and --logout subcommands
└── util.rs          (117 lines)  JWT parsing, init URL handling
```

**Total: ~3,500 lines**

### Dependencies

- `claude-codes` - Provides `AsyncClient` for spawning/communicating with claude binary
- `tokio-tungstenite` - WebSocket transport (currently hardcoded)
- `shared` - `ProxyMessage` protocol types

## Extraction Strategy

### Phase 1: Create Library Crate Structure

Create a new crate `claude-proxy-lib` in the workspace:

```
claude-proxy-lib/
├── Cargo.toml
└── src/
    ├── lib.rs           # Public API
    ├── session.rs       # Session management (extracted from proxy/session.rs)
    ├── buffer.rs        # Output buffer (from proxy/output_buffer.rs)
    ├── config.rs        # Config management (from proxy/config.rs)
    ├── transport.rs     # Transport trait + WebSocket impl
    └── error.rs         # Error types
```

### Phase 2: Define Transport Abstraction

The current proxy is tightly coupled to WebSocket. Abstract this:

```rust
// src/transport.rs

use async_trait::async_trait;
use crate::error::ProxyError;

/// Messages the proxy can send to a backend/handler
#[derive(Debug, Clone)]
pub enum OutboundMessage {
    Register {
        session_id: Uuid,
        session_name: String,
        working_directory: String,
        git_branch: Option<String>,
    },
    Output {
        seq: u64,
        content: String,
    },
    PermissionRequest {
        request_id: Uuid,
        tool_name: String,
        input: serde_json::Value,
    },
    SessionUpdate {
        git_branch: Option<String>,
    },
    Heartbeat,
}

/// Messages the proxy can receive from a backend/handler
#[derive(Debug, Clone)]
pub enum InboundMessage {
    RegisterAck { success: bool },
    Input { content: serde_json::Value },
    PermissionResponse {
        request_id: Uuid,
        allow: bool,
        input: Option<serde_json::Value>,
    },
    OutputAck { seq: u64 },
    Heartbeat,
    Shutdown,
}

/// Transport layer for proxy communication
#[async_trait]
pub trait Transport: Send + Sync {
    /// Send a message to the backend
    async fn send(&self, msg: OutboundMessage) -> Result<(), ProxyError>;

    /// Receive a message from the backend (blocks until available)
    async fn receive(&self) -> Result<Option<InboundMessage>, ProxyError>;

    /// Check if the connection is alive
    fn is_connected(&self) -> bool;

    /// Close the connection gracefully
    async fn close(&self) -> Result<(), ProxyError>;
}
```

### Phase 3: Extract Core Session Logic

```rust
// src/session.rs

use claude_codes::AsyncClient;
use uuid::Uuid;
use std::path::PathBuf;
use crate::{Transport, OutputBuffer, error::ProxyError};

pub struct SessionConfig {
    pub session_id: Uuid,
    pub working_directory: PathBuf,
    pub session_name: String,
    pub resume: bool,
}

pub struct Session<T: Transport> {
    config: SessionConfig,
    client: AsyncClient,
    transport: T,
    buffer: OutputBuffer,
}

impl<T: Transport> Session<T> {
    /// Create a new session
    pub async fn new(
        config: SessionConfig,
        transport: T,
    ) -> Result<Self, ProxyError> {
        let client = Self::spawn_claude(&config).await?;
        let buffer = OutputBuffer::new(&config.session_id)?;

        Ok(Self {
            config,
            client,
            transport,
            buffer,
        })
    }

    /// Resume an existing session
    pub async fn resume(
        config: SessionConfig,
        transport: T,
    ) -> Result<Self, ProxyError> {
        let mut config = config;
        config.resume = true;
        Self::new(config, transport).await
    }

    /// Run the session message loop
    /// Returns when the session ends (claude exits or transport disconnects)
    pub async fn run(&mut self) -> Result<SessionOutcome, ProxyError> {
        self.register().await?;
        self.replay_pending().await?;
        self.message_loop().await
    }

    /// Send user input to Claude
    pub async fn send_input(&self, content: serde_json::Value) -> Result<(), ProxyError> {
        self.client.send(content).await?;
        Ok(())
    }

    /// Respond to a permission request
    pub async fn respond_permission(
        &self,
        request_id: Uuid,
        allow: bool,
        input: Option<serde_json::Value>,
    ) -> Result<(), ProxyError> {
        // Send control response to claude
        let response = ControlResponse { allow, input, .. };
        self.client.send_control(response).await?;
        Ok(())
    }

    // Internal methods
    async fn spawn_claude(config: &SessionConfig) -> Result<AsyncClient, ProxyError>;
    async fn register(&self) -> Result<(), ProxyError>;
    async fn replay_pending(&self) -> Result<(), ProxyError>;
    async fn message_loop(&mut self) -> Result<SessionOutcome, ProxyError>;
}

pub enum SessionOutcome {
    ClaudeExited { exit_code: i32 },
    TransportDisconnected,
    SessionNotFound,
    Error(ProxyError),
}
```

### Phase 4: Extract Output Buffer

```rust
// src/buffer.rs

use uuid::Uuid;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferedMessage {
    pub seq: u64,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct OutputBuffer {
    session_id: Uuid,
    messages: Vec<BufferedMessage>,
    next_seq: u64,
    acked_seq: u64,
    storage_path: PathBuf,
}

impl OutputBuffer {
    /// Create or load buffer for a session
    pub fn new(session_id: &Uuid) -> Result<Self, std::io::Error>;

    /// Add a message to the buffer
    pub fn push(&mut self, content: String) -> u64;

    /// Mark messages up to seq as acknowledged
    pub fn ack(&mut self, seq: u64);

    /// Get all unacknowledged messages
    pub fn pending(&self) -> impl Iterator<Item = &BufferedMessage>;

    /// Persist buffer to disk
    pub fn save(&self) -> Result<(), std::io::Error>;

    /// Load buffer from disk
    pub fn load(session_id: &Uuid) -> Result<Self, std::io::Error>;
}
```

### Phase 5: Provide Default WebSocket Transport

```rust
// src/transport/websocket.rs

use tokio_tungstenite::{connect_async, WebSocketStream};
use crate::{Transport, InboundMessage, OutboundMessage, ProxyError};

pub struct WebSocketTransport {
    url: String,
    auth_token: String,
    stream: Option<WebSocketStream<...>>,
}

impl WebSocketTransport {
    pub async fn connect(url: &str, auth_token: &str) -> Result<Self, ProxyError>;
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn send(&self, msg: OutboundMessage) -> Result<(), ProxyError> { ... }
    async fn receive(&self) -> Result<Option<InboundMessage>, ProxyError> { ... }
    fn is_connected(&self) -> bool { ... }
    async fn close(&self) -> Result<(), ProxyError> { ... }
}
```

### Phase 6: Extract Config Management

```rust
// src/config.rs

use uuid::Uuid;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectorySession {
    pub session_id: Uuid,
    pub session_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAuth {
    pub auth_token: String,
    pub user_email: String,
    pub backend_url: String,
}

pub struct ConfigManager {
    config_dir: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self, std::io::Error>;

    /// Get or create session for a directory
    pub fn get_session(&self, dir: &Path) -> Option<DirectorySession>;
    pub fn set_session(&self, dir: &Path, session: DirectorySession) -> Result<(), std::io::Error>;
    pub fn remove_session(&self, dir: &Path) -> Result<(), std::io::Error>;

    /// Auth token management
    pub fn get_auth(&self, dir: &Path) -> Option<StoredAuth>;
    pub fn set_auth(&self, dir: &Path, auth: StoredAuth) -> Result<(), std::io::Error>;
    pub fn clear_auth(&self, dir: &Path) -> Result<(), std::io::Error>;
}
```

## Public API Design

```rust
// src/lib.rs

pub mod session;
pub mod buffer;
pub mod config;
pub mod transport;
pub mod error;

pub use session::{Session, SessionConfig, SessionOutcome};
pub use buffer::OutputBuffer;
pub use config::ConfigManager;
pub use transport::{Transport, InboundMessage, OutboundMessage};
pub use error::ProxyError;

// Re-export WebSocket transport as default
pub use transport::websocket::WebSocketTransport;

// Convenience function for simple use cases
pub async fn run_session(
    working_dir: impl AsRef<Path>,
    backend_url: &str,
    auth_token: &str,
) -> Result<SessionOutcome, ProxyError> {
    let config_manager = ConfigManager::new()?;
    let session_info = config_manager
        .get_session(working_dir.as_ref())
        .unwrap_or_else(|| create_new_session_info(working_dir.as_ref()));

    let transport = WebSocketTransport::connect(backend_url, auth_token).await?;

    let config = SessionConfig {
        session_id: session_info.session_id,
        working_directory: working_dir.as_ref().to_path_buf(),
        session_name: session_info.session_name,
        resume: config_manager.get_session(working_dir.as_ref()).is_some(),
    };

    let mut session = Session::new(config, transport).await?;
    session.run().await
}
```

## Migration Plan for Existing Proxy

After the library is extracted:

```rust
// proxy/src/main.rs (simplified)

use claude_proxy_lib::{
    Session, SessionConfig, WebSocketTransport, ConfigManager,
};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Auth handling (keep in proxy for CLI-specific UX)
    let auth_token = get_or_request_auth(&args).await?;

    // Use library for session management
    let config_manager = ConfigManager::new()?;
    let session_info = get_or_create_session(&config_manager, &args)?;

    let transport = WebSocketTransport::connect(&args.backend_url, &auth_token).await?;

    let config = SessionConfig {
        session_id: session_info.session_id,
        working_directory: args.working_dir.clone(),
        session_name: session_info.session_name,
        resume: args.resume,
    };

    // UI hooks for terminal output
    let mut session = Session::new(config, transport).await?;

    // Main loop with UI integration
    loop {
        match session.run().await {
            SessionOutcome::ClaudeExited { exit_code } => {
                ui::print_exit(exit_code);
                break;
            }
            SessionOutcome::TransportDisconnected => {
                ui::print_reconnecting();
                // Reconnection logic stays in proxy
            }
            SessionOutcome::SessionNotFound => {
                ui::print_session_expired();
                // Fresh session logic stays in proxy
            }
            SessionOutcome::Error(e) => {
                ui::print_error(&e);
                break;
            }
        }
    }

    Ok(())
}
```

## Files to Keep in Proxy (Portal-Specific)

These remain in the `proxy/` crate as they're CLI/portal-specific:

| File | Reason |
|------|--------|
| `main.rs` | CLI argument parsing, entry point |
| `ui.rs` | Terminal-specific output formatting |
| `update.rs` | GitHub release auto-update (portal-specific) |
| `auth.rs` | Device flow OAuth (could extract, but UI-heavy) |
| `commands.rs` | --init and --logout subcommands |

## Testing Strategy

### Unit Tests (in library)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Mock transport for testing
    struct MockTransport {
        sent: Vec<OutboundMessage>,
        to_receive: VecDeque<InboundMessage>,
    }

    #[tokio::test]
    async fn test_session_registration() {
        let transport = MockTransport::new();
        transport.queue_receive(InboundMessage::RegisterAck { success: true });

        let config = SessionConfig { ... };
        let session = Session::new(config, transport).await.unwrap();

        assert!(transport.sent.contains(&OutboundMessage::Register { ... }));
    }

    #[tokio::test]
    async fn test_output_buffering() {
        let mut buffer = OutputBuffer::new_memory();

        let seq1 = buffer.push("output 1".into());
        let seq2 = buffer.push("output 2".into());

        assert_eq!(buffer.pending().count(), 2);

        buffer.ack(seq1);
        assert_eq!(buffer.pending().count(), 1);
    }
}
```

### Integration Tests (in proxy)

```rust
#[tokio::test]
async fn test_full_session_lifecycle() {
    // Spin up test backend
    let backend = TestBackend::spawn().await;

    // Run session with real claude binary
    let outcome = claude_proxy_lib::run_session(
        "/tmp/test-dir",
        &backend.url(),
        "test-token",
    ).await.unwrap();

    assert!(matches!(outcome, SessionOutcome::ClaudeExited { .. }));
}
```

## Estimated Effort

| Phase | Effort | Description |
|-------|--------|-------------|
| 1 | 2 hours | Create crate structure, Cargo.toml |
| 2 | 4 hours | Define Transport trait and message types |
| 3 | 8 hours | Extract session.rs, refactor to use trait |
| 4 | 2 hours | Extract output_buffer.rs (minimal changes) |
| 5 | 4 hours | Implement WebSocket transport |
| 6 | 2 hours | Extract config.rs (minimal changes) |
| 7 | 4 hours | Update proxy to use library |
| 8 | 4 hours | Write tests |
| **Total** | **~30 hours** | **3-5 days of focused work** |

## Future Extensions

Once extracted, the library enables:

1. **Embedded sessions** - Use Claude Code in other Rust applications
2. **Alternative transports** - HTTP polling, gRPC, Unix sockets
3. **Custom permission handlers** - Programmatic approval without UI
4. **Session multiplexing** - Multiple sessions in one process
5. **Language bindings** - Python/Node via FFI or WASM

## Open Questions

1. **Should auth be in the library?** Device flow is UI-heavy but reusable
2. **Git branch tracking?** Currently subprocess calls `git` - extract or leave?
3. **Error recovery policy?** How much retry logic belongs in library vs consumer?
4. **Versioning strategy?** Semver for library, independent of portal versions?

## References

- [claude-codes crate](https://github.com/meawoppl/rust-claude-codes) - AsyncClient implementation
- [proxy/session.rs](../proxy/src/session.rs) - Current implementation
- [shared/src/lib.rs](../shared/src/lib.rs) - ProxyMessage protocol
