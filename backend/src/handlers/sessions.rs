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

use crate::{models::Message, models::Session, AppState};

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

    use crate::schema::{session_members, sessions};

    // Get all sessions the user is a member of (owner, editor, or viewer)
    let results = sessions::table
        .inner_join(session_members::table.on(session_members::session_id.eq(sessions::id)))
        .filter(session_members::user_id.eq(current_user_id))
        .select(Session::as_select())
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
    cookies: Cookies,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionDetailResponse>, StatusCode> {
    // Require authentication
    let current_user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::{messages, session_members, sessions};

    // Only return session if user is a member (owner, editor, or viewer)
    let session = sessions::table
        .inner_join(session_members::table.on(session_members::session_id.eq(sessions::id)))
        .filter(sessions::id.eq(session_id))
        .filter(session_members::user_id.eq(current_user_id))
        .select(Session::as_select())
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

    use crate::schema::{deleted_session_costs, session_members, sessions};

    // Only owners can delete sessions - verify user is an owner
    let session = sessions::table
        .inner_join(session_members::table.on(session_members::session_id.eq(sessions::id)))
        .filter(sessions::id.eq(session_id))
        .filter(session_members::user_id.eq(current_user_id))
        .filter(session_members::role.eq("owner"))
        .select(Session::as_select())
        .first::<Session>(&mut conn)
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Record the cost and tokens from deleted session
    let has_usage =
        session.total_cost_usd > 0.0 || session.input_tokens > 0 || session.output_tokens > 0;
    if has_usage {
        diesel::insert_into(deleted_session_costs::table)
            .values(crate::models::NewDeletedSessionCosts {
                user_id: current_user_id,
                cost_usd: session.total_cost_usd,
                session_count: 1,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
                cache_creation_tokens: session.cache_creation_tokens,
                cache_read_tokens: session.cache_read_tokens,
            })
            .on_conflict(deleted_session_costs::user_id)
            .do_update()
            .set((
                deleted_session_costs::cost_usd
                    .eq(deleted_session_costs::cost_usd + session.total_cost_usd),
                deleted_session_costs::session_count.eq(deleted_session_costs::session_count + 1),
                deleted_session_costs::input_tokens
                    .eq(deleted_session_costs::input_tokens + session.input_tokens),
                deleted_session_costs::output_tokens
                    .eq(deleted_session_costs::output_tokens + session.output_tokens),
                deleted_session_costs::cache_creation_tokens
                    .eq(deleted_session_costs::cache_creation_tokens
                        + session.cache_creation_tokens),
                deleted_session_costs::cache_read_tokens
                    .eq(deleted_session_costs::cache_read_tokens + session.cache_read_tokens),
                deleted_session_costs::updated_at.eq(diesel::dsl::now),
            ))
            .execute(&mut conn)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    // Delete the session
    diesel::delete(sessions::table.filter(sessions::id.eq(session_id)))
        .execute(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::NO_CONTENT)
}
