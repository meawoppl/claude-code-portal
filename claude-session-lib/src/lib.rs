//! Claude Session Library
//!
//! A library for managing Claude Code sessions, designed for use in
//! persistence services that need to manage multiple sessions, handle
//! restarts, and maintain state across service restarts.
//!
//! # Overview
//!
//! The library provides:
//! - `Session` - A managed Claude Code session with event-based API
//! - `SessionSnapshot` - Serializable session state for persistence
//! - `OutputBuffer` - Buffer for replay on session restore
//!
//! # Example
//!
//! ```ignore
//! use claude_session_lib::{Session, SessionConfig, SessionEvent, PermissionResponse};
//! use uuid::Uuid;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = SessionConfig {
//!         session_id: Uuid::new_v4(),
//!         working_directory: std::env::current_dir()?,
//!         session_name: "my-session".to_string(),
//!         resume: false,
//!         claude_path: None,
//!     };
//!
//!     let mut session = Session::new(config).await?;
//!
//!     // Send initial input
//!     session.send_input(serde_json::json!("Hello!")).await?;
//!
//!     // Process events
//!     while let Some(event) = session.next_event().await {
//!         match event {
//!             SessionEvent::Output(output) => {
//!                 println!("Claude: {:?}", output);
//!             }
//!             SessionEvent::PermissionRequest { request_id, tool_name, .. } => {
//!                 // Auto-approve for this example
//!                 session.respond_permission(&request_id, PermissionResponse::allow()).await?;
//!             }
//!             SessionEvent::Exited { code } => {
//!                 println!("Session exited with code {}", code);
//!                 break;
//!             }
//!             SessionEvent::Error(e) => {
//!                 eprintln!("Error: {}", e);
//!                 break;
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

pub mod buffer;
pub mod error;
pub mod session;
pub mod snapshot;

// Re-export main types at crate root
pub use buffer::{BufferedOutput, OutputBuffer};
pub use error::SessionError;
pub use session::{PermissionResponse, Session, SessionEvent};
pub use snapshot::{PendingPermission, SessionConfig, SessionSnapshot};

// Re-export claude_codes types that appear in our public API
pub use claude_codes::ClaudeOutput;
