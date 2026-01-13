use crate::models::{Message, NewMessage};
use crate::schema::messages;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_cookies::Cookies;
use tracing::{error, info};
use uuid::Uuid;

const SESSION_COOKIE_NAME: &str = "cc_session";

/// Maximum number of messages to keep per session
pub const MAX_MESSAGES_PER_SESSION: i64 = 100;

/// Request body for creating a new message
#[derive(Debug, Deserialize)]
pub struct CreateMessageRequest {
    pub role: String,
    pub content: String,
}

/// Response for message operations
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message: Message,
}

/// Response for listing messages
#[derive(Debug, Serialize)]
pub struct MessagesListResponse {
    pub messages: Vec<Message>,
    pub total: i64,
}

/// Extract user_id from signed session cookie
fn extract_user_id(app_state: &AppState, cookies: &Cookies) -> Result<Uuid, StatusCode> {
    // In dev mode, allow unauthenticated access with test user
    if app_state.dev_mode {
        let mut conn = app_state
            .db_pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        use crate::schema::users;
        return users::table
            .filter(users::email.eq("testing@testing.local"))
            .select(users::id)
            .first::<Uuid>(&mut conn)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Extract from signed cookie
    let cookie = cookies
        .signed(&app_state.cookie_key)
        .get(SESSION_COOKIE_NAME)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    cookie.value().parse().map_err(|_| StatusCode::UNAUTHORIZED)
}

/// Verify that a session belongs to the authenticated user
fn verify_session_ownership(
    conn: &mut diesel::pg::PgConnection,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<crate::models::Session, StatusCode> {
    use crate::schema::sessions;
    sessions::table
        .filter(sessions::id.eq(session_id))
        .filter(sessions::user_id.eq(user_id))
        .first::<crate::models::Session>(conn)
        .map_err(|_| StatusCode::NOT_FOUND)
}

/// Create a new message for a session
pub async fn create_message(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(session_id): Path<Uuid>,
    Json(req): Json<CreateMessageRequest>,
) -> Result<Json<MessageResponse>, StatusCode> {
    // Require authentication
    let current_user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify the session belongs to the authenticated user
    let session = verify_session_ownership(&mut conn, session_id, current_user_id)?;

    let new_message = NewMessage {
        session_id,
        role: req.role,
        content: req.content,
        user_id: session.user_id,
    };

    let message: Message = diesel::insert_into(messages::table)
        .values(&new_message)
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to create message: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Auto-truncate after insert to maintain the limit
    let _ = truncate_session_messages_internal(&mut conn, session_id);

    Ok(Json(MessageResponse { message }))
}

/// List messages for a session
pub async fn list_messages(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(session_id): Path<Uuid>,
) -> Result<Json<MessagesListResponse>, StatusCode> {
    // Require authentication
    let current_user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Verify the session belongs to the authenticated user
    let _session = verify_session_ownership(&mut conn, session_id, current_user_id)?;

    let message_list: Vec<Message> = messages::table
        .filter(messages::session_id.eq(session_id))
        .order(messages::created_at.asc())
        .load(&mut conn)
        .map_err(|e| {
            error!("Failed to list messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let total = message_list.len() as i64;

    Ok(Json(MessagesListResponse {
        messages: message_list,
        total,
    }))
}

/// Internal function to truncate session messages
/// Keeps only the last MAX_MESSAGES_PER_SESSION messages for a session
/// Returns the number of deleted messages
pub fn truncate_session_messages_internal(
    conn: &mut diesel::pg::PgConnection,
    session_id: Uuid,
) -> Result<usize, StatusCode> {
    // Count total messages for this session
    let total_count: i64 = messages::table
        .filter(messages::session_id.eq(session_id))
        .count()
        .get_result(conn)
        .map_err(|e| {
            error!("Failed to count messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if total_count <= MAX_MESSAGES_PER_SESSION {
        return Ok(0);
    }

    let to_delete = total_count - MAX_MESSAGES_PER_SESSION;

    // Get the IDs of the oldest messages to delete
    // We order by created_at ASC and take the first `to_delete` messages
    let ids_to_delete: Vec<Uuid> = messages::table
        .filter(messages::session_id.eq(session_id))
        .order(messages::created_at.asc())
        .limit(to_delete)
        .select(messages::id)
        .load(conn)
        .map_err(|e| {
            error!("Failed to get messages to delete: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if ids_to_delete.is_empty() {
        return Ok(0);
    }

    // Delete the messages
    let deleted = diesel::delete(messages::table.filter(messages::id.eq_any(&ids_to_delete)))
        .execute(conn)
        .map_err(|e| {
            error!("Failed to delete old messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    info!(
        "Truncated session {}: deleted {} old messages, keeping last {}",
        session_id, deleted, MAX_MESSAGES_PER_SESSION
    );

    Ok(deleted)
}
