use axum::{extract::State, http::StatusCode, Json};
use diesel::prelude::*;
use shared::api::SoundSettingsResponse;
use shared::protocol::SESSION_COOKIE_NAME;
use std::sync::Arc;
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::AppState;

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

pub async fn get_sound_settings(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Result<Json<SoundSettingsResponse>, StatusCode> {
    let user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::users;
    let sound_config: Option<serde_json::Value> = users::table
        .find(user_id)
        .select(users::sound_config)
        .first(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SoundSettingsResponse { sound_config }))
}

pub async fn save_sound_settings(
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
    Json(config): Json<serde_json::Value>,
) -> Result<StatusCode, StatusCode> {
    let user_id = extract_user_id(&app_state, &cookies)?;

    let mut conn = app_state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    use crate::schema::users;
    diesel::update(users::table.find(user_id))
        .set(users::sound_config.eq(Some(config)))
        .execute(&mut conn)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}
