//! Proxy Token Management Handlers
//!
//! CRUD endpoints for managing proxy authentication tokens.
//! These allow users to create, list, and revoke tokens for CLI access.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use diesel::prelude::*;
use shared::{
    CreateProxyTokenRequest, CreateProxyTokenResponse, ProxyInitConfig, ProxyTokenInfo,
    ProxyTokenListResponse,
};
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    jwt::{create_proxy_token, hash_token},
    models::{NewProxyAuthToken, ProxyAuthToken, User},
    schema::proxy_auth_tokens,
    AppState,
};

/// POST /api/proxy-tokens - Create a new proxy token
pub async fn create_token(
    State(app_state): State<Arc<AppState>>,
    user_id: Uuid, // This would come from session/auth middleware
    Json(req): Json<CreateProxyTokenRequest>,
) -> Result<Json<CreateProxyTokenResponse>, StatusCode> {
    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get user email for JWT claims
    use crate::schema::users;
    let user: User = users::table.find(user_id).first(&mut conn).map_err(|e| {
        error!("Failed to find user: {}", e);
        StatusCode::NOT_FOUND
    })?;

    // Generate token ID
    let token_id = Uuid::new_v4();

    // Calculate expiration
    let expires_at = chrono::Utc::now() + chrono::Duration::days(req.expires_in_days as i64);

    // Create JWT
    let jwt_secret = app_state.jwt_secret.as_bytes();
    let token = create_proxy_token(
        jwt_secret,
        token_id,
        user_id,
        &user.email,
        req.expires_in_days,
    )
    .map_err(|e| {
        error!("Failed to create JWT: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Hash token for storage
    let token_hash = hash_token(&token);

    // Store in database
    let new_token = NewProxyAuthToken {
        user_id,
        name: req.name.clone(),
        token_hash,
        expires_at: expires_at.naive_utc(),
    };

    let saved_token: ProxyAuthToken = diesel::insert_into(proxy_auth_tokens::table)
        .values(&new_token)
        .get_result(&mut conn)
        .map_err(|e| {
            error!("Failed to save token: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Build init URL
    let config = ProxyInitConfig {
        token: token.clone(),
        session_name_prefix: None,
    };
    let encoded_config = config.encode().map_err(|e| {
        error!("Failed to encode config: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let init_url = format!("{}/p/{}", app_state.public_url, encoded_config);

    info!("Created proxy token '{}' for user {}", req.name, user.email);

    Ok(Json(CreateProxyTokenResponse {
        id: saved_token.id,
        token,
        init_url,
        expires_at: expires_at.to_rfc3339(),
    }))
}

/// GET /api/proxy-tokens - List all tokens for the current user
pub async fn list_tokens(
    State(app_state): State<Arc<AppState>>,
    user_id: Uuid, // This would come from session/auth middleware
) -> Result<Json<ProxyTokenListResponse>, StatusCode> {
    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let tokens: Vec<ProxyAuthToken> = proxy_auth_tokens::table
        .filter(proxy_auth_tokens::user_id.eq(user_id))
        .order(proxy_auth_tokens::created_at.desc())
        .load(&mut conn)
        .map_err(|e| {
            error!("Failed to list tokens: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let token_infos: Vec<ProxyTokenInfo> = tokens
        .into_iter()
        .map(|t| ProxyTokenInfo {
            id: t.id,
            name: t.name,
            created_at: t.created_at.and_utc().to_rfc3339(),
            last_used_at: t.last_used_at.map(|dt| dt.and_utc().to_rfc3339()),
            expires_at: t.expires_at.and_utc().to_rfc3339(),
            revoked: t.revoked,
        })
        .collect();

    Ok(Json(ProxyTokenListResponse {
        tokens: token_infos,
    }))
}

/// DELETE /api/proxy-tokens/:id - Revoke a token
pub async fn revoke_token(
    State(app_state): State<Arc<AppState>>,
    user_id: Uuid, // This would come from session/auth middleware
    Path(token_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Update token to revoked (only if owned by user)
    let updated = diesel::update(
        proxy_auth_tokens::table
            .filter(proxy_auth_tokens::id.eq(token_id))
            .filter(proxy_auth_tokens::user_id.eq(user_id)),
    )
    .set(proxy_auth_tokens::revoked.eq(true))
    .execute(&mut conn)
    .map_err(|e| {
        error!("Failed to revoke token: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if updated == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    info!("Revoked proxy token {}", token_id);
    Ok(StatusCode::NO_CONTENT)
}

/// Verify a proxy token and return the user_id if valid
/// This is called from the websocket handler
pub fn verify_and_get_user(
    app_state: &AppState,
    conn: &mut diesel::pg::PgConnection,
    token: &str,
) -> Result<(Uuid, String), StatusCode> {
    // First verify JWT signature and expiration
    let claims =
        crate::jwt::verify_proxy_token(app_state.jwt_secret.as_bytes(), token).map_err(|e| {
            error!("JWT verification failed: {}", e);
            StatusCode::UNAUTHORIZED
        })?;

    // Then check database for revocation
    let token_hash = hash_token(token);
    let db_token: ProxyAuthToken = proxy_auth_tokens::table
        .filter(proxy_auth_tokens::token_hash.eq(&token_hash))
        .first(conn)
        .map_err(|_| {
            error!("Token not found in database");
            StatusCode::UNAUTHORIZED
        })?;

    // Check if revoked
    if db_token.revoked {
        error!("Token has been revoked");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Check if expired (belt and suspenders - JWT already checked this)
    let now = chrono::Utc::now().naive_utc();
    if db_token.expires_at < now {
        error!("Token has expired");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Update last_used_at
    let _ = diesel::update(proxy_auth_tokens::table.find(db_token.id))
        .set(proxy_auth_tokens::last_used_at.eq(diesel::dsl::now))
        .execute(conn);

    Ok((claims.sub, claims.email))
}

// ============================================================================
// Wrapper handlers that extract user_id from session
// ============================================================================

use tower_cookies::Cookies;

/// Wrapper for create_token that extracts user from session
pub async fn create_token_handler(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Json(req): Json<CreateProxyTokenRequest>,
) -> Result<Json<CreateProxyTokenResponse>, StatusCode> {
    let user_id = get_user_id_from_session(&app_state, &cookies).await?;
    create_token(State(app_state), user_id, Json(req)).await
}

/// Wrapper for list_tokens that extracts user from session
pub async fn list_tokens_handler(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Result<Json<ProxyTokenListResponse>, StatusCode> {
    let user_id = get_user_id_from_session(&app_state, &cookies).await?;
    list_tokens(State(app_state), user_id).await
}

/// Wrapper for revoke_token that extracts user from session
pub async fn revoke_token_handler(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(token_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let user_id = get_user_id_from_session(&app_state, &cookies).await?;
    revoke_token(State(app_state), user_id, Path(token_id)).await
}

/// Extract user_id from session cookie
async fn get_user_id_from_session(
    app_state: &AppState,
    cookies: &Cookies,
) -> Result<Uuid, StatusCode> {
    // In dev mode, use the test user
    if app_state.dev_mode {
        let mut conn = app_state
            .db_pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        use crate::schema::users;
        let user: User = users::table
            .filter(users::email.eq("testing@testing.local"))
            .first(&mut conn)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        return Ok(user.id);
    }

    // Get signed session cookie
    let session_cookie = cookies
        .signed(&app_state.cookie_key)
        .get("cc_session")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Parse user_id from cookie value
    let user_id: Uuid = session_cookie
        .value()
        .parse()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(user_id)
}
