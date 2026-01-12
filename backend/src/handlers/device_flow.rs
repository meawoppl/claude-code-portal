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
use tracing::info;
use uuid::Uuid;

use crate::AppState;

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
    pub user_code: String,
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

fn generate_access_token() -> String {
    format!(
        "ccp_{}",
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(40)
            .map(|c| c as char)
            .collect::<String>()
    )
}

// POST /auth/device/code
pub async fn device_code(
    State(app_state): State<Arc<AppState>>,
) -> Result<Json<DeviceCodeResponse>, StatusCode> {
    let store = app_state
        .device_flow_store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
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
) -> Result<Json<PollResponse>, StatusCode> {
    let store = app_state
        .device_flow_store
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let mut store_lock = store.write().await;

    let state = store_lock
        .get_mut(&req.device_code)
        .ok_or(StatusCode::NOT_FOUND)?;

    // Check expiration
    if std::time::SystemTime::now() > state.expires_at {
        state.status = DeviceFlowStatus::Expired;
    }

    match &state.status {
        DeviceFlowStatus::Pending => Ok(Json(PollResponse::Pending)),
        DeviceFlowStatus::Complete => {
            let user_id = state.user_id.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
            let access_token = state
                .access_token
                .clone()
                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

            // Fetch user email from database
            use crate::schema::users::dsl::*;
            let mut conn = app_state
                .db_pool
                .get()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let user = users
                .find(user_id)
                .first::<crate::models::User>(&mut conn)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
    Query(query): Query<VerifyQuery>,
) -> impl IntoResponse {
    // In a real implementation, this would render an HTML page
    // For now, redirect to OAuth flow with user_code in state
    let user_code = query.user_code;

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

    // Redirect to Google OAuth with user_code in state
    Redirect::temporary(&format!("/api/auth/google?device_user_code={}", user_code)).into_response()
}

// Called after OAuth success to complete device flow
pub async fn complete_device_flow(
    store: &DeviceFlowStore,
    user_code: &str,
    user_id: Uuid,
) -> Result<(), ()> {
    let mut store_lock = store.write().await;

    // Find the device flow by user_code
    if let Some(state) = store_lock
        .values_mut()
        .find(|s| s.user_code == user_code && s.status == DeviceFlowStatus::Pending)
    {
        state.user_id = Some(user_id);
        state.access_token = Some(generate_access_token());
        state.status = DeviceFlowStatus::Complete;
        info!("Device flow completed for user_code: {}", user_code);
        Ok(())
    } else {
        Err(())
    }
}
