use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    Json,
};
use diesel::prelude::*;
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_cookies::Cookies;
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    jwt::{create_proxy_token, hash_token},
    models::NewProxyAuthToken,
    schema::proxy_auth_tokens,
    AppState,
};

const SESSION_COOKIE_NAME: &str = "cc_session";

/// Error response for device flow endpoints
#[derive(Debug, Serialize)]
pub struct DeviceFlowError {
    pub error: String,
    pub message: String,
}

/// Device flow API error that returns JSON
pub struct DeviceFlowApiError {
    status: StatusCode,
    error: String,
    message: String,
}

impl DeviceFlowApiError {
    fn service_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error: "service_unavailable".to_string(),
            message: "Device flow authentication is not available. Server may be in dev mode or OAuth is not configured.".to_string(),
        }
    }

    fn not_found(msg: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            error: "not_found".to_string(),
            message: msg.to_string(),
        }
    }

    fn internal_error(msg: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: "internal_error".to_string(),
            message: msg.to_string(),
        }
    }
}

impl IntoResponse for DeviceFlowApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(DeviceFlowError {
                error: self.error,
                message: self.message,
            }),
        )
            .into_response()
    }
}

// In-memory store for device flow state
// In production, use Redis or database
pub type DeviceFlowStore = Arc<RwLock<HashMap<String, DeviceFlowState>>>;

#[derive(Debug, Clone)]
pub struct DeviceFlowState {
    pub device_code: String,
    pub user_code: String,
    pub user_id: Option<Uuid>,
    pub access_token: Option<String>,
    pub expires_at: std::time::SystemTime,
    pub status: DeviceFlowStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeviceFlowStatus {
    Pending,
    Complete,
    Expired,
    Denied,
}

#[derive(Debug, Serialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct PollRequest {
    pub device_code: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub enum PollResponse {
    #[serde(rename = "pending")]
    Pending,

    #[serde(rename = "complete")]
    Complete {
        access_token: String,
        user_id: String,
        user_email: String,
    },

    #[serde(rename = "expired")]
    Expired,

    #[serde(rename = "denied")]
    Denied,
}

#[derive(Debug, Deserialize)]
pub struct VerifyQuery {
    pub user_code: Option<String>,
}

fn generate_user_code() -> String {
    let chars: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(6)
        .map(|c| c as char)
        .collect::<String>()
        .to_uppercase();

    // Format as XXX-XXX for readability
    format!("{}-{}", &chars[0..3], &chars[3..6])
}

fn generate_device_code() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(|c| c as char)
        .collect()
}

// POST /auth/device/code
pub async fn device_code(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<DeviceCodeResponse>, DeviceFlowApiError> {
    let store = app_state
        .device_flow_store
        .as_ref()
        .ok_or_else(DeviceFlowApiError::service_unavailable)?;
    let device_code = generate_device_code();
    let user_code = generate_user_code();

    let expires_in = 300; // 5 minutes
    let expires_at = std::time::SystemTime::now() + std::time::Duration::from_secs(expires_in);

    let state = DeviceFlowState {
        device_code: device_code.clone(),
        user_code: user_code.clone(),
        user_id: None,
        access_token: None,
        expires_at,
        status: DeviceFlowStatus::Pending,
    };

    let mut store_lock = store.write().await;
    store_lock.insert(device_code.clone(), state);

    let verification_uri = format!("{}/api/auth/device", app_state.public_url);

    Ok(Json(DeviceCodeResponse {
        device_code,
        user_code,
        verification_uri,
        expires_in,
        interval: 5,
    }))
}

// POST /auth/device/poll
pub async fn device_poll(
    State(app_state): State<Arc<AppState>>,
    Json(req): Json<PollRequest>,
) -> Result<Json<PollResponse>, DeviceFlowApiError> {
    let store = app_state
        .device_flow_store
        .as_ref()
        .ok_or_else(DeviceFlowApiError::service_unavailable)?;
    let mut store_lock = store.write().await;

    let state = store_lock
        .get_mut(&req.device_code)
        .ok_or_else(|| DeviceFlowApiError::not_found("Device code not found or expired"))?;

    // Check expiration
    if std::time::SystemTime::now() > state.expires_at {
        state.status = DeviceFlowStatus::Expired;
    }

    match &state.status {
        DeviceFlowStatus::Pending => Ok(Json(PollResponse::Pending)),
        DeviceFlowStatus::Complete => {
            let user_id = state
                .user_id
                .ok_or_else(|| DeviceFlowApiError::internal_error("Missing user ID"))?;
            let access_token = state
                .access_token
                .clone()
                .ok_or_else(|| DeviceFlowApiError::internal_error("Missing access token"))?;

            // Fetch user email from database
            use crate::schema::users::dsl::*;
            let mut conn = app_state
                .db_pool
                .get()
                .map_err(|_| DeviceFlowApiError::internal_error("Database connection failed"))?;

            let user = users
                .find(user_id)
                .first::<crate::models::User>(&mut conn)
                .map_err(|_| DeviceFlowApiError::internal_error("User not found"))?;

            Ok(Json(PollResponse::Complete {
                access_token,
                user_id: user_id.to_string(),
                user_email: user.email,
            }))
        }
        DeviceFlowStatus::Expired => Ok(Json(PollResponse::Expired)),
        DeviceFlowStatus::Denied => Ok(Json(PollResponse::Denied)),
    }
}

// GET /auth/device - Show verification page
pub async fn device_verify_page(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Query(query): Query<VerifyQuery>,
) -> impl IntoResponse {
    // If no user_code provided, show a form to enter it
    let user_code = match query.user_code {
        Some(code) => code,
        None => {
            return axum::response::Html(DEVICE_CODE_FORM_HTML.to_string()).into_response();
        }
    };

    // Check if user code exists
    let store = match &app_state.device_flow_store {
        Some(s) => s,
        None => return Redirect::temporary("/").into_response(),
    };
    let store_lock = store.read().await;
    let valid = store_lock
        .values()
        .any(|state| state.user_code == user_code && state.status == DeviceFlowStatus::Pending);

    drop(store_lock);

    if !valid {
        return Redirect::temporary("/api/auth/device/error?message=Invalid+or+expired+code")
            .into_response();
    }

    // Check if user is already logged in via session cookie
    if let Some(cookie) = cookies
        .signed(&app_state.cookie_key)
        .get(SESSION_COOKIE_NAME)
    {
        if let Ok(user_id) = cookie.value().parse::<Uuid>() {
            // User is already logged in - complete device flow directly
            if let Ok(()) = complete_device_flow(&app_state, store, &user_code, user_id).await {
                info!(
                    "Device flow completed using existing session for user: {}",
                    user_id
                );
                return Redirect::temporary("/api/auth/device/success").into_response();
            }
        }
    }

    // User not logged in - redirect to Google OAuth with user_code in state
    Redirect::temporary(&format!("/api/auth/google?device_user_code={}", user_code)).into_response()
}

const DEVICE_CODE_FORM_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Device Authentication - Claude Code Portal</title>
    <style>
        :root {
            --bg-dark: #1a1b26;
            --bg-darker: #16161e;
            --text-primary: #c0caf5;
            --text-secondary: #7f849c;
            --accent: #7aa2f7;
            --accent-hover: #9eb3ff;
            --border: #292e42;
            --success: #9ece6a;
            --error: #f7768e;
        }
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        body {
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            background: var(--bg-dark);
            color: var(--text-primary);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }
        .container {
            background: var(--bg-darker);
            border: 1px solid var(--border);
            border-radius: 12px;
            padding: 2rem;
            max-width: 400px;
            width: 90%;
            text-align: center;
        }
        h1 {
            font-size: 1.5rem;
            margin-bottom: 0.5rem;
            color: var(--accent);
        }
        p {
            color: var(--text-secondary);
            margin-bottom: 1.5rem;
            font-size: 0.9rem;
        }
        .code-input {
            width: 100%;
            padding: 1rem;
            font-size: 1.5rem;
            text-align: center;
            background: var(--bg-dark);
            border: 2px solid var(--border);
            border-radius: 8px;
            color: var(--text-primary);
            font-family: 'Courier New', monospace;
            letter-spacing: 0.25rem;
            text-transform: uppercase;
            margin-bottom: 1rem;
        }
        .code-input:focus {
            outline: none;
            border-color: var(--accent);
        }
        .code-input::placeholder {
            color: var(--text-secondary);
            letter-spacing: normal;
            text-transform: none;
        }
        button {
            width: 100%;
            padding: 0.75rem 1.5rem;
            font-size: 1rem;
            background: var(--accent);
            color: var(--bg-dark);
            border: none;
            border-radius: 8px;
            cursor: pointer;
            font-weight: 600;
            transition: background 0.2s;
        }
        button:hover {
            background: var(--accent-hover);
        }
        .hint {
            margin-top: 1rem;
            font-size: 0.8rem;
            color: var(--text-secondary);
        }
    </style>
</head>
<body>
    <div class="container">
        <h1>Device Authentication</h1>
        <p>Enter the code displayed in your terminal to authenticate this device.</p>
        <form action="/api/auth/device" method="get">
            <input
                type="text"
                name="user_code"
                class="code-input"
                placeholder="XXX-XXX"
                pattern="[A-Za-z0-9]{3}-?[A-Za-z0-9]{3}"
                maxlength="7"
                required
                autofocus
            >
            <button type="submit">Continue</button>
        </form>
        <p class="hint">The code is shown in your terminal after running <code>claude-portal</code></p>
    </div>
</body>
</html>
"#;

// Called after OAuth success to complete device flow
// Creates a proper JWT token and stores it in the database
pub async fn complete_device_flow(
    app_state: &AppState,
    store: &DeviceFlowStore,
    user_code: &str,
    user_id: Uuid,
) -> Result<(), ()> {
    // First, get user email from database (needed for JWT claims)
    let mut conn = app_state.db_pool.get().map_err(|e| {
        error!("Failed to get database connection: {}", e);
    })?;

    use crate::schema::users;
    let user: crate::models::User = users::table.find(user_id).first(&mut conn).map_err(|e| {
        error!("Failed to find user: {}", e);
    })?;

    // Generate token ID and create JWT
    let token_id = Uuid::new_v4();
    let expires_in_days: u32 = 30; // Device flow tokens valid for 30 days
    let jwt_secret = app_state.jwt_secret.as_bytes();

    let token = create_proxy_token(jwt_secret, token_id, user_id, &user.email, expires_in_days)
        .map_err(|e| {
            error!("Failed to create JWT: {}", e);
        })?;

    // Store token hash in database
    let token_hash = hash_token(&token);
    let expires_at = chrono::Utc::now() + chrono::Duration::days(expires_in_days as i64);

    let new_token = NewProxyAuthToken {
        user_id,
        name: format!(
            "Device auth {}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        ),
        token_hash,
        expires_at: expires_at.naive_utc(),
    };

    diesel::insert_into(proxy_auth_tokens::table)
        .values(&new_token)
        .execute(&mut conn)
        .map_err(|e| {
            error!("Failed to save token to database: {}", e);
        })?;

    // Now update the in-memory store with the JWT token
    let mut store_lock = store.write().await;

    // Find the device flow by user_code
    if let Some(state) = store_lock
        .values_mut()
        .find(|s| s.user_code == user_code && s.status == DeviceFlowStatus::Pending)
    {
        state.user_id = Some(user_id);
        state.access_token = Some(token);
        state.status = DeviceFlowStatus::Complete;
        info!(
            "Device flow completed for user_code: {}, user: {}",
            user_code, user.email
        );
        Ok(())
    } else {
        error!("Device flow state not found for user_code: {}", user_code);
        Err(())
    }
}
