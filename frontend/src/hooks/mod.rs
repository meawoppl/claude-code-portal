//! Custom Yew hooks for the frontend application.
//!
//! These hooks encapsulate reusable state logic to keep components clean and focused.

mod use_client_websocket;
mod use_keyboard_nav;
mod use_local_storage;
mod use_sessions;

pub use use_client_websocket::use_client_websocket;
pub use use_keyboard_nav::{use_keyboard_nav, KeyboardNavConfig};
pub use use_sessions::use_sessions;

pub use use_local_storage::use_local_storage;
