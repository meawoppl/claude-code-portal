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
use tracing::error;
use uuid::Uuid;

use shared::protocol::SESSION_COOKIE_NAME;

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

/// Verify that a user has access to a session (is a member with any role)
fn verify_session_access(
    conn: &mut diesel::pg::PgConnection,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<crate::models::Session, StatusCode> {
    use crate::schema::{session_members, sessions};
    sessions::table
        .inner_join(session_members::table.on(session_members::session_id.eq(sessions::id)))
        .filter(sessions::id.eq(session_id))
        .filter(session_members::user_id.eq(user_id))
        .select(crate::models::Session::as_select())
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

    // Verify the user has access to the session
    let session = verify_session_access(&mut conn, session_id, current_user_id)?;

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

    // Queue session for truncation (batched for efficiency)
    app_state.session_manager.queue_truncation(session_id);

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

    // Verify the user has access to the session
    let _session = verify_session_access(&mut conn, session_id, current_user_id)?;

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
