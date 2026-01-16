//! Admin dashboard API handlers
//!
//! These endpoints are restricted to users with is_admin=true.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use bigdecimal::ToPrimitive;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_cookies::Cookies;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{models::User, schema, AppState};

const SESSION_COOKIE_NAME: &str = "cc_session";

// ============================================================================
// Usage Helper - Aggregates cost and token data per user
// ============================================================================

/// Aggregated usage data for a user (includes both active and deleted sessions)
#[derive(Debug, Default, Clone)]
pub struct UserUsage {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
}

/// Fetch aggregated usage for a specific user (active sessions + deleted session costs)
pub fn get_user_usage(
    conn: &mut diesel::PgConnection,
    user_id: Uuid,
) -> Result<UserUsage, diesel::result::Error> {
    // Get cost and tokens from active sessions
    let active_cost: f64 = schema::sessions::table
        .filter(schema::sessions::user_id.eq(user_id))
        .select(diesel::dsl::sum(schema::sessions::total_cost_usd))
        .first::<Option<f64>>(conn)?
        .unwrap_or(0.0);

    let active_input: i64 = schema::sessions::table
        .filter(schema::sessions::user_id.eq(user_id))
        .select(diesel::dsl::sum(schema::sessions::input_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);

    let active_output: i64 = schema::sessions::table
        .filter(schema::sessions::user_id.eq(user_id))
        .select(diesel::dsl::sum(schema::sessions::output_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);

    let active_cache_creation: i64 = schema::sessions::table
        .filter(schema::sessions::user_id.eq(user_id))
        .select(diesel::dsl::sum(schema::sessions::cache_creation_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);

    let active_cache_read: i64 = schema::sessions::table
        .filter(schema::sessions::user_id.eq(user_id))
        .select(diesel::dsl::sum(schema::sessions::cache_read_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);

    // Get usage from deleted sessions for this user (single row per user)
    let (deleted_cost, deleted_input, deleted_output, deleted_cache_creation, deleted_cache_read): (
        f64,
        i64,
        i64,
        i64,
        i64,
    ) = schema::deleted_session_costs::table
        .filter(schema::deleted_session_costs::user_id.eq(user_id))
        .select((
            schema::deleted_session_costs::cost_usd,
            schema::deleted_session_costs::input_tokens,
            schema::deleted_session_costs::output_tokens,
            schema::deleted_session_costs::cache_creation_tokens,
            schema::deleted_session_costs::cache_read_tokens,
        ))
        .first(conn)
        .unwrap_or((0.0, 0, 0, 0, 0));

    Ok(UserUsage {
        cost_usd: active_cost + deleted_cost,
        input_tokens: active_input + deleted_input,
        output_tokens: active_output + deleted_output,
        cache_creation_tokens: active_cache_creation + deleted_cache_creation,
        cache_read_tokens: active_cache_read + deleted_cache_read,
    })
}

// ============================================================================
// Admin Guard - extracts and validates admin user from cookies
// ============================================================================

/// Extract the current user from cookies and verify they are an admin.
/// Returns the User if they are an admin, or an appropriate error status code.
pub async fn require_admin(
    app_state: &Arc<AppState>,
    cookies: &Cookies,
) -> Result<User, StatusCode> {
    // Extract user ID from signed session cookie
    let cookie = cookies
        .signed(&app_state.cookie_key)
        .get(SESSION_COOKIE_NAME)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let user_id: Uuid = cookie
        .value()
        .parse()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Fetch user from database
    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let user = schema::users::table
        .find(user_id)
        .first::<User>(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Check if user is disabled
    if user.disabled {
        warn!("Disabled user {} attempted admin access", user.email);
        return Err(StatusCode::FORBIDDEN);
    }

    // Check if user is admin
    if !user.is_admin {
        warn!("Non-admin user {} attempted admin access", user.email);
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(user)
}

// ============================================================================
// Stats Endpoint - System overview statistics
// ============================================================================

#[derive(Debug, Serialize)]
pub struct AdminStats {
    /// Total number of registered users
    pub total_users: i64,
    /// Number of users with is_admin=true
    pub admin_users: i64,
    /// Number of disabled users
    pub disabled_users: i64,
    /// Total number of sessions (all time)
    pub total_sessions: i64,
    /// Number of active sessions
    pub active_sessions: i64,
    /// Number of currently connected proxy clients
    pub connected_proxy_clients: usize,
    /// Number of currently connected web clients
    pub connected_web_clients: usize,
    /// Total API spend across all sessions
    pub total_spend_usd: f64,
    /// Total input tokens across all sessions
    pub total_input_tokens: i64,
    /// Total output tokens across all sessions
    pub total_output_tokens: i64,
    /// Total cache creation tokens across all sessions
    pub total_cache_creation_tokens: i64,
    /// Total cache read tokens across all sessions
    pub total_cache_read_tokens: i64,
}

pub async fn get_stats(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Result<Json<AdminStats>, StatusCode> {
    let admin = require_admin(&app_state, &cookies).await?;
    info!("Admin {} requested system stats", admin.email);

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Count users
    let total_users: i64 = schema::users::table
        .count()
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to count users: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let admin_users: i64 = schema::users::table
        .filter(schema::users::is_admin.eq(true))
        .count()
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to count admin users: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let disabled_users: i64 = schema::users::table
        .filter(schema::users::disabled.eq(true))
        .count()
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to count disabled users: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Count sessions
    let total_sessions: i64 = schema::sessions::table
        .count()
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to count sessions: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let active_sessions: i64 = schema::sessions::table
        .filter(schema::sessions::status.eq("active"))
        .count()
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to count active sessions: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Get cost total from active sessions
    let active_spend: f64 = schema::sessions::table
        .select(diesel::dsl::sum(schema::sessions::total_cost_usd))
        .first::<Option<f64>>(&mut conn)
        .map_err(|e| {
            error!("Failed to sum active session spend: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .unwrap_or(0.0);

    // Get token totals from active sessions (separate queries due to Diesel type constraints)
    let active_input: i64 = schema::sessions::table
        .select(diesel::dsl::sum(schema::sessions::input_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);
    let active_output: i64 = schema::sessions::table
        .select(diesel::dsl::sum(schema::sessions::output_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);
    let active_cache_creation: i64 = schema::sessions::table
        .select(diesel::dsl::sum(schema::sessions::cache_creation_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);
    let active_cache_read: i64 = schema::sessions::table
        .select(diesel::dsl::sum(schema::sessions::cache_read_tokens))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);

    // Get cost total from deleted sessions
    let deleted_spend: f64 = schema::deleted_session_costs::table
        .select(diesel::dsl::sum(schema::deleted_session_costs::cost_usd))
        .first::<Option<f64>>(&mut conn)
        .map_err(|e| {
            error!("Failed to sum deleted session spend: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .unwrap_or(0.0);

    // Get token totals from deleted sessions
    let deleted_input: i64 = schema::deleted_session_costs::table
        .select(diesel::dsl::sum(
            schema::deleted_session_costs::input_tokens,
        ))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);
    let deleted_output: i64 = schema::deleted_session_costs::table
        .select(diesel::dsl::sum(
            schema::deleted_session_costs::output_tokens,
        ))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);
    let deleted_cache_creation: i64 = schema::deleted_session_costs::table
        .select(diesel::dsl::sum(
            schema::deleted_session_costs::cache_creation_tokens,
        ))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);
    let deleted_cache_read: i64 = schema::deleted_session_costs::table
        .select(diesel::dsl::sum(
            schema::deleted_session_costs::cache_read_tokens,
        ))
        .first::<Option<bigdecimal::BigDecimal>>(&mut conn)
        .ok()
        .flatten()
        .and_then(|d| d.to_i64())
        .unwrap_or(0);

    let total_spend_usd = active_spend + deleted_spend;
    let total_input_tokens = active_input + deleted_input;
    let total_output_tokens = active_output + deleted_output;
    let total_cache_creation_tokens = active_cache_creation + deleted_cache_creation;
    let total_cache_read_tokens = active_cache_read + deleted_cache_read;

    // Get connected client counts from session manager
    let connected_proxy_clients = app_state.session_manager.sessions.len();
    let connected_web_clients: usize = app_state
        .session_manager
        .user_clients
        .iter()
        .map(|r| r.value().len())
        .sum();

    Ok(Json(AdminStats {
        total_users,
        admin_users,
        disabled_users,
        total_sessions,
        active_sessions,
        connected_proxy_clients,
        connected_web_clients,
        total_spend_usd,
        total_input_tokens,
        total_output_tokens,
        total_cache_creation_tokens,
        total_cache_read_tokens,
    }))
}

// ============================================================================
// Users Endpoint - List and manage users
// ============================================================================

#[derive(Debug, Serialize)]
pub struct AdminUserInfo {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_admin: bool,
    pub disabled: bool,
    pub voice_enabled: bool,
    pub created_at: String,
    pub session_count: i64,
    pub total_spend_usd: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminUsersResponse {
    pub users: Vec<AdminUserInfo>,
}

pub async fn list_users(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Result<Json<AdminUsersResponse>, StatusCode> {
    let admin = require_admin(&app_state, &cookies).await?;
    info!("Admin {} requested user list", admin.email);

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get all users
    let users: Vec<User> = schema::users::table
        .order(schema::users::created_at.desc())
        .load(&mut conn)
        .map_err(|e| {
            error!("Failed to load users: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Get session counts and usage per user
    let mut user_infos = Vec::with_capacity(users.len());
    for user in users {
        // Get session count
        let session_count: i64 = schema::sessions::table
            .filter(schema::sessions::user_id.eq(user.id))
            .count()
            .get_result(&mut conn)
            .unwrap_or(0);

        // Get aggregated usage via helper
        let usage = get_user_usage(&mut conn, user.id).unwrap_or_default();

        user_infos.push(AdminUserInfo {
            id: user.id,
            email: user.email,
            name: user.name,
            avatar_url: user.avatar_url,
            is_admin: user.is_admin,
            disabled: user.disabled,
            voice_enabled: user.voice_enabled,
            created_at: user.created_at.to_string(),
            session_count,
            total_spend_usd: usage.cost_usd,
            total_input_tokens: usage.input_tokens,
            total_output_tokens: usage.output_tokens,
            total_cache_creation_tokens: usage.cache_creation_tokens,
            total_cache_read_tokens: usage.cache_read_tokens,
        });
    }

    Ok(Json(AdminUsersResponse { users: user_infos }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub is_admin: Option<bool>,
    pub disabled: Option<bool>,
    pub voice_enabled: Option<bool>,
}

pub async fn update_user(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(user_id): Path<Uuid>,
    Json(update): Json<UpdateUserRequest>,
) -> Result<StatusCode, StatusCode> {
    let admin = require_admin(&app_state, &cookies).await?;

    // Prevent admin from demoting themselves
    if user_id == admin.id && update.is_admin == Some(false) {
        warn!(
            "Admin {} attempted to remove their own admin status",
            admin.email
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Prevent admin from disabling themselves
    if user_id == admin.id && update.disabled == Some(true) {
        warn!(
            "Admin {} attempted to disable their own account",
            admin.email
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get target user for logging
    let target_user: User = schema::users::table
        .find(user_id)
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Build update query
    if let Some(is_admin_val) = update.is_admin {
        diesel::update(schema::users::table.find(user_id))
            .set(schema::users::is_admin.eq(is_admin_val))
            .execute(&mut conn)
            .map_err(|e| {
                error!("Failed to update user admin status: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        info!(
            "Admin {} set is_admin={} for user {}",
            admin.email, is_admin_val, target_user.email
        );
    }

    if let Some(disabled_val) = update.disabled {
        diesel::update(schema::users::table.find(user_id))
            .set(schema::users::disabled.eq(disabled_val))
            .execute(&mut conn)
            .map_err(|e| {
                error!("Failed to update user disabled status: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        info!(
            "Admin {} set disabled={} for user {}",
            admin.email, disabled_val, target_user.email
        );
    }

    if let Some(voice_enabled_val) = update.voice_enabled {
        diesel::update(schema::users::table.find(user_id))
            .set(schema::users::voice_enabled.eq(voice_enabled_val))
            .execute(&mut conn)
            .map_err(|e| {
                error!("Failed to update user voice_enabled status: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        info!(
            "Admin {} set voice_enabled={} for user {}",
            admin.email, voice_enabled_val, target_user.email
        );
    }

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Sessions Endpoint - List and manage all sessions
// ============================================================================

#[derive(Debug, Serialize)]
pub struct AdminSessionInfo {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_email: String,
    pub session_name: String,
    pub working_directory: Option<String>,
    pub git_branch: Option<String>,
    pub status: String,
    pub total_cost_usd: f64,
    pub created_at: String,
    pub last_activity: String,
    pub is_connected: bool,
}

#[derive(Debug, Serialize)]
pub struct AdminSessionsResponse {
    pub sessions: Vec<AdminSessionInfo>,
}

pub async fn list_sessions(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Result<Json<AdminSessionsResponse>, StatusCode> {
    let admin = require_admin(&app_state, &cookies).await?;
    info!("Admin {} requested sessions list", admin.email);

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get all sessions with user email
    let results: Vec<(crate::models::Session, String)> = schema::sessions::table
        .inner_join(schema::users::table)
        .select((schema::sessions::all_columns, schema::users::email))
        .order(schema::sessions::last_activity.desc())
        .load(&mut conn)
        .map_err(|e| {
            error!("Failed to load sessions: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let session_infos: Vec<AdminSessionInfo> = results
        .into_iter()
        .map(|(session, user_email)| {
            let is_connected = app_state
                .session_manager
                .sessions
                .contains_key(&session.id.to_string());

            AdminSessionInfo {
                id: session.id,
                user_id: session.user_id,
                user_email,
                session_name: session.session_name,
                working_directory: session.working_directory,
                git_branch: session.git_branch,
                status: session.status,
                total_cost_usd: session.total_cost_usd,
                created_at: session.created_at.to_string(),
                last_activity: session.last_activity.to_string(),
                is_connected,
            }
        })
        .collect();

    Ok(Json(AdminSessionsResponse {
        sessions: session_infos,
    }))
}

pub async fn delete_session(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let admin = require_admin(&app_state, &cookies).await?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get session info for logging and cost tracking
    let session: crate::models::Session = schema::sessions::table
        .find(session_id)
        .first(&mut conn)
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Remove from session manager (disconnect if connected)
    let session_key = session_id.to_string();
    app_state.session_manager.unregister_session(&session_key);

    // Record the cost and tokens from deleted session
    let has_usage =
        session.total_cost_usd > 0.0 || session.input_tokens > 0 || session.output_tokens > 0;
    if has_usage {
        diesel::insert_into(schema::deleted_session_costs::table)
            .values(crate::models::NewDeletedSessionCosts {
                user_id: session.user_id,
                cost_usd: session.total_cost_usd,
                session_count: 1,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
                cache_creation_tokens: session.cache_creation_tokens,
                cache_read_tokens: session.cache_read_tokens,
            })
            .on_conflict(schema::deleted_session_costs::user_id)
            .do_update()
            .set((
                schema::deleted_session_costs::cost_usd
                    .eq(schema::deleted_session_costs::cost_usd + session.total_cost_usd),
                schema::deleted_session_costs::session_count
                    .eq(schema::deleted_session_costs::session_count + 1),
                schema::deleted_session_costs::input_tokens
                    .eq(schema::deleted_session_costs::input_tokens + session.input_tokens),
                schema::deleted_session_costs::output_tokens
                    .eq(schema::deleted_session_costs::output_tokens + session.output_tokens),
                schema::deleted_session_costs::cache_creation_tokens
                    .eq(schema::deleted_session_costs::cache_creation_tokens
                        + session.cache_creation_tokens),
                schema::deleted_session_costs::cache_read_tokens
                    .eq(schema::deleted_session_costs::cache_read_tokens
                        + session.cache_read_tokens),
                schema::deleted_session_costs::updated_at.eq(diesel::dsl::now),
            ))
            .execute(&mut conn)
            .map_err(|e| {
                error!("Failed to record deleted session cost: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // Delete messages first (foreign key constraint)
    diesel::delete(schema::messages::table.filter(schema::messages::session_id.eq(session_id)))
        .execute(&mut conn)
        .map_err(|e| {
            error!("Failed to delete session messages: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Delete the session
    diesel::delete(schema::sessions::table.find(session_id))
        .execute(&mut conn)
        .map_err(|e| {
            error!("Failed to delete session: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    info!(
        "Admin {} deleted session {} ({}) - cost ${:.4} recorded",
        admin.email, session_id, session.session_name, session.total_cost_usd
    );

    Ok(StatusCode::NO_CONTENT)
}
