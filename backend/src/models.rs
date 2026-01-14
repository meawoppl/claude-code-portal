use chrono::NaiveDateTime;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Queryable, Selectable, Serialize, Deserialize)]
#[diesel(table_name = crate::schema::users)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct User {
    pub id: Uuid,
    pub google_id: String,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::users)]
pub struct NewUser {
    pub google_id: String,
    pub email: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Queryable, Selectable, Serialize, Deserialize, Clone)]
#[diesel(table_name = crate::schema::sessions)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_name: String,
    pub session_key: String,
    pub working_directory: Option<String>,
    pub status: String,
    pub last_activity: NaiveDateTime,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub git_branch: Option<String>,
    pub total_cost_usd: f64,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::sessions)]
pub struct NewSession {
    pub user_id: Uuid,
    pub session_name: String,
    pub session_key: String,
    pub working_directory: Option<String>,
    pub status: String,
    pub git_branch: Option<String>,
}

/// NewSession variant that allows specifying the ID (for when we want to use Claude's session ID)
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::sessions)]
pub struct NewSessionWithId {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_name: String,
    pub session_key: String,
    pub working_directory: Option<String>,
    pub status: String,
    pub git_branch: Option<String>,
}

#[derive(Debug, Queryable, Selectable, Serialize, Deserialize, Clone)]
#[diesel(table_name = crate::schema::messages)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Message {
    pub id: Uuid,
    pub session_id: Uuid,
    pub role: String,
    pub content: String,
    pub created_at: NaiveDateTime,
    pub user_id: Uuid,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::messages)]
pub struct NewMessage {
    pub session_id: Uuid,
    pub role: String,
    pub content: String,
    pub user_id: Uuid,
}

// ============================================================================
// Proxy Auth Token Models
// ============================================================================

#[derive(Debug, Queryable, Selectable, Serialize, Deserialize)]
#[diesel(table_name = crate::schema::proxy_auth_tokens)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ProxyAuthToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub created_at: NaiveDateTime,
    pub last_used_at: Option<NaiveDateTime>,
    pub expires_at: NaiveDateTime,
    pub revoked: bool,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::proxy_auth_tokens)]
pub struct NewProxyAuthToken {
    pub user_id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub expires_at: NaiveDateTime,
}
