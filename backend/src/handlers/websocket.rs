use crate::{db::DbPool, models::NewSession, AppState};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use dashmap::DashMap;
use diesel::prelude::*;
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

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
            .or_insert_with(Vec::new)
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
                            session_name,
                            auth_token,
                            working_directory,
                        } => {
                            // Generate or use session key
                            let key = session_name.clone();
                            session_key = Some(key.clone());

                            // Register in memory
                            session_manager.register_session(key.clone(), tx.clone());

                            // Persist to database
                            if let Ok(mut conn) = db_pool.get() {
                                use crate::schema::sessions;

                                // Try to find existing session with this key
                                let existing: Option<crate::models::Session> = sessions::table
                                    .filter(sessions::session_key.eq(&key))
                                    .first(&mut conn)
                                    .optional()
                                    .unwrap_or(None);

                                if let Some(existing_session) = existing {
                                    // Update existing session to active
                                    let _ =
                                        diesel::update(sessions::table.find(existing_session.id))
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
                                            .execute(&mut conn);
                                    db_session_id = Some(existing_session.id);
                                    info!("Session reactivated in DB: {}", session_name);
                                } else {
                                    // Create new session - validate JWT token to get user_id
                                    let user_id =
                                        get_user_id_from_token(&app_state, auth_token.as_deref());

                                    if let Some(user_id) = user_id {
                                        let new_session = NewSession {
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
                                                info!("Session persisted to DB: {}", session_name);
                                            }
                                            Err(e) => {
                                                error!("Failed to persist session: {}", e);
                                            }
                                        }
                                    } else {
                                        warn!("No valid user_id for session, not persisting to DB");
                                    }
                                }
                            }

                            info!("Session registered: {}", session_name);
                        }
                        ProxyMessage::ClaudeOutput { content } => {
                            // Broadcast output to all web clients
                            if let Some(ref key) = session_key {
                                session_manager.broadcast_to_web_clients(
                                    key,
                                    ProxyMessage::ClaudeOutput { content },
                                );
                            }

                            // Update last_activity in DB
                            if let (Some(session_id), Ok(mut conn)) = (db_session_id, db_pool.get())
                            {
                                use crate::schema::sessions;
                                let _ = diesel::update(sessions::table.find(session_id))
                                    .set(sessions::last_activity.eq(diesel::dsl::now))
                                    .execute(&mut conn);
                            }
                        }
                        ProxyMessage::Heartbeat => {
                            // Respond to heartbeat
                            let _ = tx.send(ProxyMessage::Heartbeat);
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

pub async fn handle_web_client_websocket(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
) -> Response {
    let session_manager = app_state.session_manager.clone();
    ws.on_upgrade(|socket| handle_web_client_socket(socket, session_manager))
}

async fn handle_web_client_socket(socket: WebSocket, session_manager: SessionManager) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ProxyMessage>();

    let mut session_key: Option<SessionId> = None;

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
                            session_name,
                            auth_token: _,
                            working_directory: _,
                        } => {
                            // Web client connecting to a session
                            let key = session_name.clone();
                            session_key = Some(key.clone());

                            session_manager.add_web_client(key, tx.clone());
                            info!("Web client connected to session: {}", session_name);
                        }
                        ProxyMessage::ClaudeInput { content } => {
                            // Forward input to the actual session
                            if let Some(ref key) = session_key {
                                info!("Web client sending ClaudeInput to session: {}", key);
                                info!("Available sessions: {:?}", session_manager.sessions.iter().map(|r| r.key().clone()).collect::<Vec<_>>());
                                if !session_manager
                                    .send_to_session(key, ProxyMessage::ClaudeInput { content })
                                {
                                    warn!("Failed to send to session '{}', session not found in SessionManager", key);
                                }
                            } else {
                                warn!("Web client tried to send ClaudeInput but no session_key set (not registered?)");
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
