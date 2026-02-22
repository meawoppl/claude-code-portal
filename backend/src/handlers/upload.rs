use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    Json,
};
use base64::Engine;
use diesel::prelude::*;
use shared::api::FileUploadResponse;
use shared::protocol::SESSION_COOKIE_NAME;
use shared::ServerToProxy;
use std::sync::Arc;
use tower_cookies::Cookies;
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;

/// Maximum upload file size: 10 MB
const MAX_UPLOAD_SIZE: usize = 10 * 1024 * 1024;

/// Handle file upload to a session
pub async fn upload_file(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(session_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<FileUploadResponse>, StatusCode> {
    let current_user_id = extract_user_id(&app_state, &cookies)?;
    verify_session_write_access(&app_state, session_id, current_user_id)?;

    // Extract file from multipart
    let mut filename = String::new();
    let mut content_type = String::new();
    let mut file_data: Vec<u8> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        if field.name() == Some("file") {
            filename = field.file_name().unwrap_or("uploaded_file").to_string();
            content_type = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            file_data = field
                .bytes()
                .await
                .map_err(|_| StatusCode::BAD_REQUEST)?
                .to_vec();
            break;
        }
    }

    if filename.is_empty() || file_data.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    if file_data.len() > MAX_UPLOAD_SIZE {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let safe_filename = sanitize_filename(&filename);
    let file_size = file_data.len() as u64;

    let encoded = base64::engine::general_purpose::STANDARD.encode(&file_data);

    info!(
        "File upload: {} ({} bytes) to session {} by user {}",
        safe_filename, file_size, session_id, current_user_id
    );

    let session_key = session_id.to_string();
    let msg = ServerToProxy::FileUpload {
        filename: safe_filename.clone(),
        data: encoded,
        content_type,
    };

    if !app_state.session_manager.send_to_session(&session_key, msg) {
        warn!("Session {} not connected, file upload queued", session_id);
    }

    Ok(Json(FileUploadResponse {
        success: true,
        filename: safe_filename,
        size: file_size,
    }))
}

fn extract_user_id(app_state: &AppState, cookies: &Cookies) -> Result<Uuid, StatusCode> {
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

    let cookie = cookies
        .signed(&app_state.cookie_key)
        .get(SESSION_COOKIE_NAME)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    cookie.value().parse().map_err(|_| StatusCode::UNAUTHORIZED)
}

fn verify_session_write_access(
    app_state: &AppState,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode> {
    use crate::schema::{session_members, sessions};

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sessions::table
        .inner_join(session_members::table.on(session_members::session_id.eq(sessions::id)))
        .filter(sessions::id.eq(session_id))
        .filter(session_members::user_id.eq(user_id))
        .filter(session_members::role.ne("viewer"))
        .select(sessions::id)
        .first::<Uuid>(&mut conn)
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::FORBIDDEN)?;

    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    let base = name
        .rsplit('/')
        .next()
        .or_else(|| name.rsplit('\\').next())
        .unwrap_or(name);

    let clean: String = base
        .chars()
        .filter(|c| *c != '/' && *c != '\\' && *c != '\0')
        .collect();

    if clean.is_empty() || clean == "." || clean == ".." {
        "uploaded_file".to_string()
    } else {
        clean
    }
}
