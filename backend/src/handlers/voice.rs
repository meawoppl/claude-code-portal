//! Voice WebSocket Handler
//!
//! Handles audio streaming for voice-to-text functionality.
//! Audio is received as binary PCM16 frames and will be forwarded
//! to Google Speech-to-Text for transcription.

use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use diesel::prelude::*;
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::sync::Arc;
use tower_cookies::Cookies;
use tracing::{error, info, warn};
use uuid::Uuid;

const SESSION_COOKIE_NAME: &str = "cc_session";

/// Extract user_id from signed session cookie
fn extract_user_id_from_cookies(app_state: &AppState, cookies: &Cookies) -> Option<Uuid> {
    // In dev mode, use the test user
    if app_state.dev_mode {
        let mut conn = app_state.db_pool.get().ok()?;
        use crate::schema::users;
        return users::table
            .filter(users::email.eq("testing@testing.local"))
            .select(users::id)
            .first::<Uuid>(&mut conn)
            .ok();
    }

    // Extract from signed cookie
    let cookie = cookies
        .signed(&app_state.cookie_key)
        .get(SESSION_COOKIE_NAME)?;
    cookie.value().parse().ok()
}

/// Check if user has voice feature enabled
fn check_voice_enabled(app_state: &AppState, user_id: Uuid) -> bool {
    let mut conn = match app_state.db_pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };

    use crate::schema::users;
    users::table
        .filter(users::id.eq(user_id))
        .select(users::voice_enabled)
        .first::<bool>(&mut conn)
        .unwrap_or(false)
}

/// Verify that a session belongs to a specific user
fn verify_session_ownership(app_state: &AppState, session_id: Uuid, user_id: Uuid) -> bool {
    let mut conn = match app_state.db_pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };

    use crate::schema::sessions;
    sessions::table
        .filter(sessions::id.eq(session_id))
        .filter(sessions::user_id.eq(user_id))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0)
        > 0
}

/// WebSocket endpoint for voice audio streaming
///
/// Route: /ws/voice/:session_id
///
/// This endpoint accepts binary audio frames (PCM16, 16kHz mono) and
/// streams them to the speech-to-text service. Transcription results
/// are sent back as JSON messages.
pub async fn handle_voice_websocket(
    ws: WebSocketUpgrade,
    Path(session_id): Path<Uuid>,
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Response {
    // Authenticate the user
    let user_id = match extract_user_id_from_cookies(&app_state, &cookies) {
        Some(id) => id,
        None => {
            warn!("Unauthenticated voice WebSocket connection attempt");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    // Check if user has voice feature enabled
    if !check_voice_enabled(&app_state, user_id) {
        warn!(
            "User {} attempted voice WebSocket but voice is not enabled",
            user_id
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    // Verify session ownership
    if !verify_session_ownership(&app_state, session_id, user_id) {
        warn!(
            "User {} attempted voice WebSocket for session {} they don't own",
            user_id, session_id
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    info!(
        "Voice WebSocket upgrade for user {} on session {}",
        user_id, session_id
    );
    ws.on_upgrade(move |socket| handle_voice_socket(socket, user_id, session_id))
}

async fn handle_voice_socket(socket: WebSocket, user_id: Uuid, session_id: Uuid) {
    let (mut sender, mut receiver) = socket.split();

    info!(
        "Voice WebSocket connected for user {} on session {}",
        user_id, session_id
    );

    // Handle incoming messages
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Binary(data)) => {
                // Binary audio data (PCM16, 16kHz mono)
                info!(
                    "Received {} bytes of audio data for session {}",
                    data.len(),
                    session_id
                );

                // TODO: Forward to Google Speech-to-Text streaming API
                // For now, just acknowledge receipt
                // The Speech-to-Text client will be added in a future step
            }
            Ok(Message::Text(text)) => {
                // Handle control messages (JSON)
                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                    match proxy_msg {
                        ProxyMessage::StartVoice {
                            session_id: msg_session_id,
                            language_code,
                        } => {
                            if msg_session_id != session_id {
                                warn!("StartVoice session_id mismatch");
                                continue;
                            }
                            info!(
                                "Starting voice recognition for session {} with language {}",
                                session_id, language_code
                            );
                            // TODO: Initialize Speech-to-Text streaming session
                        }
                        ProxyMessage::StopVoice {
                            session_id: msg_session_id,
                        } => {
                            if msg_session_id != session_id {
                                warn!("StopVoice session_id mismatch");
                                continue;
                            }
                            info!("Stopping voice recognition for session {}", session_id);
                            // TODO: Close Speech-to-Text streaming session
                        }
                        _ => {
                            warn!("Unexpected message type on voice WebSocket");
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Voice WebSocket closed for session {}", session_id);
                break;
            }
            Ok(Message::Ping(data)) => {
                // Respond to ping with pong
                if sender.send(Message::Pong(data)).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                error!("Voice WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    info!(
        "Voice WebSocket disconnected for user {} on session {}",
        user_id, session_id
    );
}
