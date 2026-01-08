use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    AppState,
    models::{NewMessage, Session, Message},
    schema,
};

#[derive(Debug, Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<Session>,
}

pub async fn list_sessions(
    State(app_state): State<Arc<AppState>>,
    // TODO: Extract user_id from session
) -> Result<Json<SessionListResponse>, StatusCode> {
    let mut conn = app_state.db_pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::sessions::dsl::*;

    // TODO: Filter by user_id from session
    let results = sessions
        .filter(status.eq("active"))
        .order(last_activity.desc())
        .load::<Session>(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SessionListResponse { sessions: results }))
}

#[derive(Debug, Serialize)]
pub struct SessionDetailResponse {
    pub session: Session,
    pub recent_messages: Vec<Message>,
}

pub async fn get_session(
    State(app_state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionDetailResponse>, StatusCode> {
    let mut conn = app_state.db_pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::sessions;
    use crate::schema::messages;

    let session = sessions::table
        .find(session_id)
        .first::<Session>(&mut conn)
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let recent_messages = messages::table
        .filter(messages::session_id.eq(session_id))
        .order(messages::created_at.desc())
        .limit(50)
        .load::<Message>(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SessionDetailResponse {
        session,
        recent_messages,
    }))
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub message_id: Uuid,
}

pub async fn send_message(
    State(app_state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, StatusCode> {
    let mut conn = app_state.db_pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::messages;

    let new_message = NewMessage {
        session_id: session_id,
        role: "user".to_string(),
        content: req.content,
    };

    let message = diesel::insert_into(messages::table)
        .values(&new_message)
        .get_result::<Message>(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SendMessageResponse {
        message_id: message.id,
    }))
}
