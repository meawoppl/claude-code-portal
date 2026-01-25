//! Error types for claude-session-lib

/// Errors that can occur during session management
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Failed to spawn Claude process: {0}")]
    SpawnFailed(#[source] std::io::Error),

    #[error("Claude process communication error: {0}")]
    CommunicationError(String),

    #[error("Session not found locally (expired)")]
    SessionNotFound,

    #[error("Invalid permission response: no pending request with id {0}")]
    InvalidPermissionResponse(String),

    #[error("Session already exited with code {0}")]
    AlreadyExited(i32),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Claude client error: {0}")]
    ClaudeError(#[from] claude_codes::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = SessionError::SessionNotFound;
        assert_eq!(format!("{}", err), "Session not found locally (expired)");

        let err = SessionError::AlreadyExited(42);
        assert_eq!(format!("{}", err), "Session already exited with code 42");

        let err = SessionError::InvalidPermissionResponse("req-123".to_string());
        assert_eq!(
            format!("{}", err),
            "Invalid permission response: no pending request with id req-123"
        );

        let err = SessionError::CommunicationError("connection lost".to_string());
        assert_eq!(
            format!("{}", err),
            "Claude process communication error: connection lost"
        );
    }

    #[test]
    fn test_error_debug() {
        let err = SessionError::SessionNotFound;
        // Debug representation should include the variant name
        let debug = format!("{:?}", err);
        assert!(debug.contains("SessionNotFound"));
    }
}
