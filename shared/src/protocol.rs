/// Session cookie name used for web client authentication.
/// Shared between all backend handlers that read or write the session cookie.
pub const SESSION_COOKIE_NAME: &str = "cc_session";

/// Maximum number of messages to queue per session when the proxy is disconnected.
pub const MAX_PENDING_MESSAGES_PER_SESSION: usize = 100;

/// Maximum age (in seconds) of pending messages before they are dropped.
pub const MAX_PENDING_MESSAGE_AGE_SECS: u64 = 300;

/// Device authorization code lifetime in seconds (5 minutes).
pub const DEVICE_CODE_EXPIRES_SECS: u64 = 300;
