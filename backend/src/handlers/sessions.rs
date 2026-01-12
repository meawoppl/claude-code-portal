use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use serde::Serialize;
use std::sync::Arc;
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::{
    models::{Message, Session},
    AppState,
};

const SESSION_COOKIE_NAME: &str = "cc_session";

#[derive(Debug, Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<Session>,
}

pub async fn list_sessions(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Result<Json<SessionListResponse>, StatusCode> {
    // Extract user_id from session cookie
    let current_user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::sessions;

    // Filter sessions by user_id - return all sessions for UI to group by status
    let results = sessions::table
        .filter(sessions::user_id.eq(current_user_id))
        .order(sessions::last_activity.desc())
        .load::<Session>(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SessionListResponse { sessions: results }))
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

#[derive(Debug, Serialize)]
pub struct SessionDetailResponse {
    pub session: Session,
    pub recent_messages: Vec<Message>,
}

pub async fn get_session(
    State(app_state): State<Arc<AppState>>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionDetailResponse>, StatusCode> {
    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::messages;
    use crate::schema::sessions;

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

pub async fn delete_session(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let current_user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::sessions;

    // Only delete if session belongs to the user
    let deleted = diesel::delete(
        sessions::table
            .filter(sessions::id.eq(session_id))
            .filter(sessions::user_id.eq(current_user_id)),
    )
    .execute(&mut conn)
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if deleted > 0 {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
