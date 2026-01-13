use crate::{models::NewSessionWithId, AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use diesel::prelude::*;
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_cookies::Cookies;
use tracing::{error, info, warn};
use uuid::Uuid;

const SESSION_COOKIE_NAME: &str = "cc_session";

pub type SessionId = String;
pub type ClientSender = mpsc::UnboundedSender<ProxyMessage>;

#[derive(Clone)]
pub struct SessionManager {
    // Map of session_key -> sender to that session's WebSocket
    pub sessions: Arc<DashMap<SessionId, ClientSender>>,
    // Map of session_key -> list of web client senders
    pub web_clients: Arc<DashMap<SessionId, Vec<ClientSender>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            web_clients: Arc::new(DashMap::new()),
        }
    }

    pub fn register_session(&self, session_key: SessionId, sender: ClientSender) {
        info!("Registering session: {}", session_key);
        self.sessions.insert(session_key, sender);
    }

    pub fn unregister_session(&self, session_key: &SessionId) {
        info!("Unregistering session: {}", session_key);
        self.sessions.remove(session_key);
    }

    pub fn add_web_client(&self, session_key: SessionId, sender: ClientSender) {
        info!("Adding web client for session: {}", session_key);
        self.web_clients
            .entry(session_key)
            .or_default()
            .push(sender);
    }

    pub fn broadcast_to_web_clients(&self, session_key: &SessionId, msg: ProxyMessage) {
        if let Some(mut clients) = self.web_clients.get_mut(session_key) {
            clients.retain(|sender| sender.send(msg.clone()).is_ok());
        }
    }

    pub fn send_to_session(&self, session_key: &SessionId, msg: ProxyMessage) -> bool {
        if let Some(sender) = self.sessions.get(session_key) {
            sender.send(msg).is_ok()
        } else {
            false
        }
    }
}

pub async fn handle_session_websocket(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(|socket| handle_session_socket(socket, app_state))
}

async fn handle_session_socket(socket: WebSocket, app_state: Arc<AppState>) {
    let session_manager = app_state.session_manager.clone();
    let db_pool = app_state.db_pool.clone();
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ProxyMessage>();

    let mut session_key: Option<SessionId> = None;
    let mut db_session_id: Option<Uuid> = None;

    // Spawn task to send messages to the WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle incoming messages
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                    match proxy_msg {
                        ProxyMessage::Register {
                            session_id: claude_session_id,
                            session_name,
                            auth_token,
                            working_directory,
                            resuming,
                        } => {
                            // Use session_id as the key for in-memory tracking
                            let key = claude_session_id.to_string();
                            session_key = Some(key.clone());

                            // Register in memory
                            session_manager.register_session(key.clone(), tx.clone());

                            // Track registration result for RegisterAck
                            let mut registration_success = false;
                            let mut registration_error: Option<String> = None;

                            // Persist to database
                            if let Ok(mut conn) = db_pool.get() {
                                use crate::schema::sessions;

                                // Look up by the Claude session ID (which is now our primary key)
                                let existing: Option<crate::models::Session> = sessions::table
                                    .find(claude_session_id)
                                    .first(&mut conn)
                                    .optional()
                                    .unwrap_or(None);

                                if let Some(existing_session) = existing {
                                    // Update existing session to active
                                    match diesel::update(sessions::table.find(existing_session.id))
                                        .set((
                                            sessions::status.eq("active"),
                                            sessions::last_activity.eq(diesel::dsl::now),
                                            sessions::working_directory.eq(
                                                if working_directory.is_empty() {
                                                    None
                                                } else {
                                                    Some(&working_directory)
                                                },
                                            ),
                                        ))
                                        .execute(&mut conn)
                                    {
                                        Ok(_) => {
                                            db_session_id = Some(existing_session.id);
                                            registration_success = true;
                                            info!(
                                                "Session reactivated in DB: {} ({})",
                                                session_name, claude_session_id
                                            );
                                        }
                                        Err(e) => {
                                            error!("Failed to reactivate session: {}", e);
                                            registration_error =
                                                Some("Failed to reactivate session".to_string());
                                        }
                                    }
                                } else if resuming {
                                    // Trying to resume but session doesn't exist in DB
                                    // This can happen if the session was deleted or is on a different backend
                                    warn!("Resuming session {} but not found in DB, creating new entry", claude_session_id);

                                    let user_id =
                                        get_user_id_from_token(&app_state, auth_token.as_deref());
                                    if let Some(user_id) = user_id {
                                        let new_session = NewSessionWithId {
                                            id: claude_session_id,
                                            user_id,
                                            session_name: session_name.clone(),
                                            session_key: key.clone(),
                                            working_directory: if working_directory.is_empty() {
                                                None
                                            } else {
                                                Some(working_directory.clone())
                                            },
                                            status: "active".to_string(),
                                        };

                                        match diesel::insert_into(sessions::table)
                                            .values(&new_session)
                                            .get_result::<crate::models::Session>(&mut conn)
                                        {
                                            Ok(session) => {
                                                db_session_id = Some(session.id);
                                                registration_success = true;
                                                info!(
                                                    "Session created in DB: {} ({})",
                                                    session_name, claude_session_id
                                                );
                                            }
                                            Err(e) => {
                                                error!("Failed to persist session: {}", e);
                                                registration_error =
                                                    Some("Failed to persist session".to_string());
                                            }
                                        }
                                    } else {
                                        warn!("No valid user_id for session, not persisting to DB");
                                        registration_error = Some(
                                            "Authentication failed - please re-authenticate"
                                                .to_string(),
                                        );
                                    }
                                } else {
                                    // Create new session with the provided session_id as primary key
                                    let user_id =
                                        get_user_id_from_token(&app_state, auth_token.as_deref());

                                    if let Some(user_id) = user_id {
                                        let new_session = NewSessionWithId {
                                            id: claude_session_id,
                                            user_id,
                                            session_name: session_name.clone(),
                                            session_key: key.clone(),
                                            working_directory: if working_directory.is_empty() {
                                                None
                                            } else {
                                                Some(working_directory.clone())
                                            },
                                            status: "active".to_string(),
                                        };

                                        match diesel::insert_into(sessions::table)
                                            .values(&new_session)
                                            .get_result::<crate::models::Session>(&mut conn)
                                        {
                                            Ok(session) => {
                                                db_session_id = Some(session.id);
                                                registration_success = true;
                                                info!(
                                                    "Session persisted to DB: {} ({})",
                                                    session_name, claude_session_id
                                                );
                                            }
                                            Err(e) => {
                                                error!("Failed to persist session: {}", e);
                                                registration_error =
                                                    Some("Failed to persist session".to_string());
                                            }
                                        }
                                    } else {
                                        warn!("No valid user_id for session, not persisting to DB");
                                        registration_error = Some(
                                            "Authentication failed - please re-authenticate"
                                                .to_string(),
                                        );
                                    }
                                }
                            } else {
                                error!("Failed to get database connection");
                                registration_error = Some("Database connection failed".to_string());
                            }

                            // Send RegisterAck to proxy
                            let ack = ProxyMessage::RegisterAck {
                                success: registration_success,
                                session_id: claude_session_id,
                                error: registration_error,
                            };
                            let _ = tx.send(ack);

                            info!(
                                "Session registered: {} ({}) - success: {}",
                                session_name, claude_session_id, registration_success
                            );
                        }
                        ProxyMessage::ClaudeOutput { content } => {
                            // Broadcast output to all web clients
                            if let Some(ref key) = session_key {
                                session_manager.broadcast_to_web_clients(
                                    key,
                                    ProxyMessage::ClaudeOutput {
                                        content: content.clone(),
                                    },
                                );
                            }

                            // Store message and update last_activity in DB
                            if let (Some(session_id), Ok(mut conn)) = (db_session_id, db_pool.get())
                            {
                                use crate::schema::{messages, sessions};

                                // Get user_id from session
                                if let Ok(session) = sessions::table
                                    .find(session_id)
                                    .first::<crate::models::Session>(&mut conn)
                                {
                                    // Determine role from content type
                                    let role = content
                                        .get("type")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("assistant");

                                    let new_message = crate::models::NewMessage {
                                        session_id,
                                        role: role.to_string(),
                                        content: content.to_string(),
                                        user_id: session.user_id,
                                    };

                                    if let Err(e) = diesel::insert_into(messages::table)
                                        .values(&new_message)
                                        .execute(&mut conn)
                                    {
                                        error!("Failed to store message: {}", e);
                                    }

                                    // Truncate to keep only last 100 messages
                                    let _ = super::messages::truncate_session_messages_internal(
                                        &mut conn, session_id,
                                    );
                                }

                                // Update last_activity
                                let _ = diesel::update(sessions::table.find(session_id))
                                    .set(sessions::last_activity.eq(diesel::dsl::now))
                                    .execute(&mut conn);
                            }
                        }
                        ProxyMessage::Heartbeat => {
                            // Respond to heartbeat
                            let _ = tx.send(ProxyMessage::Heartbeat);
                        }
                        ProxyMessage::PermissionRequest {
                            request_id,
                            tool_name,
                            input,
                            permission_suggestions,
                        } => {
                            // Forward permission request to all web clients
                            if let Some(ref key) = session_key {
                                info!("Permission request from proxy for tool: {} (request_id: {}, suggestions: {})", tool_name, request_id, permission_suggestions.len());
                                session_manager.broadcast_to_web_clients(
                                    key,
                                    ProxyMessage::PermissionRequest {
                                        request_id,
                                        tool_name,
                                        input,
                                        permission_suggestions,
                                    },
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("WebSocket closed");
                break;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    // Cleanup - mark session as disconnected in DB
    if let Some(session_id) = db_session_id {
        if let Ok(mut conn) = db_pool.get() {
            use crate::schema::sessions;
            let _ = diesel::update(sessions::table.find(session_id))
                .set(sessions::status.eq("disconnected"))
                .execute(&mut conn);
        }
    }

    if let Some(key) = session_key {
        session_manager.unregister_session(&key);
    }

    send_task.abort();
}

/// Get user_id from auth token using JWT verification
fn get_user_id_from_token(app_state: &AppState, auth_token: Option<&str>) -> Option<Uuid> {
    let mut conn = app_state.db_pool.get().ok()?;
    use crate::schema::users;

    // Try to verify JWT token if provided
    if let Some(token) = auth_token {
        match super::proxy_tokens::verify_and_get_user(app_state, &mut conn, token) {
            Ok((user_id, email)) => {
                info!("JWT token verified for user: {}", email);
                return Some(user_id);
            }
            Err(e) => {
                warn!("JWT verification failed: {:?}, falling back to dev mode", e);
            }
        }
    }

    // Dev mode fallback: use the test user
    if app_state.dev_mode {
        users::table
            .filter(users::email.eq("testing@testing.local"))
            .select(users::id)
            .first::<Uuid>(&mut conn)
            .ok()
    } else {
        // In production, require valid token
        None
    }
}

/// Extract user_id from signed session cookie for web client authentication
fn extract_user_id_from_cookies(app_state: &AppState, cookies: &Cookies) -> Option<Uuid> {
    // In dev mode, use the test user
    if app_state.dev_mode {
        let mut conn = app_state.db_pool.get().ok()?;
        use crate::schema::users;
        return users::table
            .filter(users::email.eq("testing@testing.local"))
            .select(users::id)
            .first::<Uuid>(&mut conn)
            .ok();
    }

    // Extract from signed cookie
    let cookie = cookies
        .signed(&app_state.cookie_key)
        .get(SESSION_COOKIE_NAME)?;
    cookie.value().parse().ok()
}

/// Verify that a session belongs to a specific user
fn verify_session_ownership(
    app_state: &AppState,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<crate::models::Session, ()> {
    let mut conn = app_state.db_pool.get().map_err(|_| ())?;
    use crate::schema::sessions;
    sessions::table
        .filter(sessions::id.eq(session_id))
        .filter(sessions::user_id.eq(user_id))
        .first::<crate::models::Session>(&mut conn)
        .map_err(|_| ())
}

pub async fn handle_web_client_websocket(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Response {
    // Authenticate the user before upgrading the WebSocket
    let user_id = match extract_user_id_from_cookies(&app_state, &cookies) {
        Some(id) => id,
        None => {
            warn!("Unauthenticated WebSocket connection attempt to /ws/client");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    info!("Authenticated WebSocket upgrade for user: {}", user_id);
    ws.on_upgrade(move |socket| handle_web_client_socket(socket, app_state, user_id))
}

async fn handle_web_client_socket(socket: WebSocket, app_state: Arc<AppState>, user_id: Uuid) {
    let session_manager = app_state.session_manager.clone();
    let db_pool = app_state.db_pool.clone();
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ProxyMessage>();

    let mut session_key: Option<SessionId> = None;
    let mut verified_session_id: Option<Uuid> = None;

    // Spawn task to send messages to the WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle incoming messages from web client
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                    match proxy_msg {
                        ProxyMessage::Register {
                            session_id,
                            session_name,
                            auth_token: _,
                            working_directory: _,
                            resuming: _,
                        } => {
                            // Verify the user owns this session before allowing connection
                            match verify_session_ownership(&app_state, session_id, user_id) {
                                Ok(_session) => {
                                    // User owns this session, allow connection
                                    let key = session_id.to_string();
                                    session_key = Some(key.clone());
                                    verified_session_id = Some(session_id);

                                    // Register this web client to receive new messages
                                    session_manager.add_web_client(key, tx.clone());
                                    info!(
                                        "Web client connected to session: {} ({}) for user {}",
                                        session_name, session_id, user_id
                                    );

                                    // Send existing messages from DB as history
                                    if let Ok(mut conn) = db_pool.get() {
                                        use crate::schema::messages;

                                        let history: Vec<crate::models::Message> = messages::table
                                            .filter(messages::session_id.eq(session_id))
                                            .order(messages::created_at.asc())
                                            .load(&mut conn)
                                            .unwrap_or_default();

                                        info!(
                                            "Sending {} historical messages to web client",
                                            history.len()
                                        );

                                        for msg in history {
                                            // Convert stored message to ClaudeOutput format
                                            // The content is stored as JSON string, parse it back
                                            let content =
                                                match serde_json::from_str::<serde_json::Value>(
                                                    &msg.content,
                                                ) {
                                                    Ok(json) => json,
                                                    Err(_) => {
                                                        // If not valid JSON, wrap as text
                                                        serde_json::json!({
                                                            "type": msg.role,
                                                            "content": msg.content
                                                        })
                                                    }
                                                };

                                            let _ = tx.send(ProxyMessage::ClaudeOutput { content });
                                        }
                                    }
                                }
                                Err(_) => {
                                    // User doesn't own this session - reject
                                    warn!(
                                        "User {} attempted to access session {} they don't own",
                                        user_id, session_id
                                    );
                                    let _ = tx.send(ProxyMessage::Error {
                                        message: "Access denied: you don't own this session"
                                            .to_string(),
                                    });
                                    break;
                                }
                            }
                        }
                        ProxyMessage::ClaudeInput { content } => {
                            // Only allow if session ownership was verified
                            if let Some(ref key) = session_key {
                                if verified_session_id.is_some() {
                                    info!("Web client sending ClaudeInput to session: {}", key);
                                    if !session_manager
                                        .send_to_session(key, ProxyMessage::ClaudeInput { content })
                                    {
                                        warn!("Failed to send to session '{}', session not found in SessionManager", key);
                                    }
                                } else {
                                    warn!(
                                        "Attempted ClaudeInput without verified session ownership"
                                    );
                                }
                            } else {
                                warn!("Web client tried to send ClaudeInput but no session_key set (not registered?)");
                            }
                        }
                        ProxyMessage::PermissionResponse {
                            request_id,
                            allow,
                            input,
                            permissions,
                            reason,
                        } => {
                            // Only allow if session ownership was verified
                            if let Some(ref key) = session_key {
                                if verified_session_id.is_some() {
                                    info!("Web client sending PermissionResponse: {} -> {} (permissions: {}, reason: {:?})",
                                          request_id, if allow { "allow" } else { "deny" }, permissions.len(), reason);
                                    if !session_manager.send_to_session(
                                        key,
                                        ProxyMessage::PermissionResponse {
                                            request_id,
                                            allow,
                                            input,
                                            permissions,
                                            reason,
                                        },
                                    ) {
                                        warn!("Failed to send PermissionResponse to session '{}', session not connected", key);
                                    }
                                } else {
                                    warn!("Attempted PermissionResponse without verified session ownership");
                                }
                            } else {
                                warn!("Web client tried to send PermissionResponse but no session_key set");
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("Web client WebSocket closed");
                break;
            }
            Err(e) => {
                error!("Web client WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    send_task.abort();
}
