use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Json,
};
use diesel::prelude::*;
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tracing::{error, info};
use uuid::Uuid;

use crate::{AppState, models::{NewUser, User}, schema};
#[derive(Debug, Serialize)]
pub struct AuthUrlResponse {
    pub auth_url: String,
}

pub async fn login(
    State(app_state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let client = match &app_state.oauth_basic_client {
        Some(c) => c,
        None => return Redirect::temporary("/auth/dev-login").into_response(),
    };

    let mut auth_request = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()));

    // If device_user_code is provided, include it in state
    if let Some(device_user_code) = query.get("device_user_code") {
        auth_request = auth_request
            .add_extra_param("state", device_user_code);
    }

    let (auth_url, _csrf_token) = auth_request.url();

    Redirect::temporary(auth_url.as_str()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AuthCallbackQuery {
    code: String,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    sub: String,
    email: String,
    name: Option<String>,
    picture: Option<String>,
}

pub async fn callback(
    State(app_state): State<Arc<AppState>>,
    Query(query): Query<AuthCallbackQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let client = app_state.oauth_basic_client.as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    // Exchange code for token
    let token: oauth2::StandardTokenResponse<oauth2::EmptyExtraTokenFields, oauth2::basic::BasicTokenType> = client
        .exchange_code(AuthorizationCode::new(query.code))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| {
            error!("Failed to exchange code: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Fetch user info from Google
    let client = reqwest::Client::new();
    let user_info: GoogleUserInfo = client
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(token.access_token().secret())
        .send()
        .await
        .map_err(|e| {
            error!("Failed to fetch user info: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .json()
        .await
        .map_err(|e| {
            error!("Failed to parse user info: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    info!("User authenticated: {}", user_info.email);

    // Save or update user in database
    let mut conn = app_state.db_pool.get().map_err(|e| {
        error!("Failed to get db connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    use crate::schema::users::dsl::*;

    let user = users
        .filter(google_id.eq(&user_info.sub))
        .first::<User>(&mut conn)
        .optional()
        .map_err(|e| {
            error!("Database error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let user = match user {
        Some(user) => user,
        None => {
            let new_user = NewUser {
                google_id: user_info.sub,
                email: user_info.email,
                name: user_info.name,
                avatar_url: user_info.picture,
            };

            diesel::insert_into(users)
                .values(&new_user)
                .get_result::<User>(&mut conn)
                .map_err(|e| {
                    error!("Failed to create user: {}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
        }
    };

    // Check if this is part of a device flow
    if let Some(device_user_code) = query.state {
        // Complete device flow
        if let Some(ref store) = app_state.device_flow_store {
            if let Ok(()) = crate::handlers::device_flow::complete_device_flow(
                store,
                &device_user_code,
                user.id,
            )
            .await
            {
                info!("Device flow completed for user: {}", user.email);
                return Ok(Redirect::temporary("/auth/device/success"));
            }
        }
    }

    // TODO: Set session cookie here
    // For now, just redirect to the frontend with user info
    Ok(Redirect::temporary(&format!(
        "/app/?user_id={}",
        user.id
    )))
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

pub async fn me(
    State(_app_state): State<Arc<AppState>>,
    // TODO: Extract user ID from session
) -> Result<Json<UserResponse>, StatusCode> {
    // Placeholder - would extract user ID from session
    Err(StatusCode::UNAUTHORIZED)
}

// Development mode handlers (bypass OAuth)
pub async fn dev_login(State(app_state): State<Arc<AppState>>) -> Result<impl IntoResponse, StatusCode> {
    use crate::schema::users::dsl::*;

    let mut conn = app_state.db_pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let user = users
        .filter(email.eq("testing@testing.local"))
        .first::<User>(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    info!("Dev mode: auto-logged in as testing@testing.local");

    // Redirect to dashboard (or frontend home)
    Ok(Redirect::temporary("/app/?dev_user=true"))
}

pub async fn dev_me(State(app_state): State<Arc<AppState>>) -> Result<Json<UserResponse>, StatusCode> {
    use crate::schema::users::dsl::*;

    let mut conn = app_state.db_pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let user = users
        .filter(email.eq("testing@testing.local"))
        .first::<User>(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(UserResponse {
        id: user.id,
        email: user.email,
        name: user.name,
        avatar_url: user.avatar_url,
    }))
}
