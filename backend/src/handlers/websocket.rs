use crate::{
    models::{NewSessionMember, NewSessionWithId},
    AppState,
};
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
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tower_cookies::Cookies;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const SESSION_COOKIE_NAME: &str = "cc_session";

/// Maximum number of messages to queue per session when proxy is disconnected
const MAX_PENDING_MESSAGES_PER_SESSION: usize = 100;

/// Maximum age of pending messages before they're dropped (5 minutes)
const MAX_PENDING_MESSAGE_AGE: Duration = Duration::from_secs(300);

/// A message queued for a disconnected proxy
#[derive(Clone)]
struct PendingMessage {
    msg: ProxyMessage,
    queued_at: Instant,
}

pub type SessionId = String;
pub type ClientSender = mpsc::UnboundedSender<ProxyMessage>;

#[derive(Clone)]
pub struct SessionManager {
    // Map of session_key -> sender to that session's WebSocket
    pub sessions: Arc<DashMap<SessionId, ClientSender>>,
    // Map of session_key -> list of web client senders
    pub web_clients: Arc<DashMap<SessionId, Vec<ClientSender>>>,
    // Map of user_id -> list of web client senders (for user-level broadcasts)
    pub user_clients: Arc<DashMap<Uuid, Vec<ClientSender>>>,
    // Map of session_id -> last acknowledged sequence number (for deduplication)
    pub last_ack_seq: Arc<DashMap<Uuid, u64>>,
    // Map of session_key -> pending messages for disconnected proxies
    pending_messages: Arc<DashMap<SessionId, VecDeque<PendingMessage>>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            web_clients: Arc::new(DashMap::new()),
            user_clients: Arc::new(DashMap::new()),
            last_ack_seq: Arc::new(DashMap::new()),
            pending_messages: Arc::new(DashMap::new()),
        }
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_session(&self, session_key: SessionId, sender: ClientSender) {
        info!("Registering session: {}", session_key);

        // Replay any pending messages before registering the new sender
        let pending_count = self.replay_pending_messages(&session_key, &sender);
        if pending_count > 0 {
            info!(
                "Replayed {} pending messages to reconnected proxy for session: {}",
                pending_count, session_key
            );
        }

        self.sessions.insert(session_key, sender);
    }

    /// Replay pending messages to a newly connected proxy
    /// Returns the number of messages replayed
    fn replay_pending_messages(&self, session_key: &SessionId, sender: &ClientSender) -> usize {
        let mut replayed = 0;
        let now = Instant::now();

        if let Some(mut pending) = self.pending_messages.get_mut(session_key) {
            // Filter out expired messages and send valid ones
            while let Some(pending_msg) = pending.pop_front() {
                if now.duration_since(pending_msg.queued_at) < MAX_PENDING_MESSAGE_AGE {
                    if sender.send(pending_msg.msg).is_ok() {
                        replayed += 1;
                    } else {
                        // Sender failed, stop replaying
                        warn!("Failed to replay pending message, sender closed");
                        break;
                    }
                } else {
                    debug!(
                        "Dropping expired pending message (age: {:?})",
                        now.duration_since(pending_msg.queued_at)
                    );
                }
            }
        }

        // Clean up the pending queue for this session
        self.pending_messages.remove(session_key);

        replayed
    }

    pub fn unregister_session(&self, session_key: &SessionId) {
        info!("Unregistering session: {}", session_key);
        self.sessions.remove(session_key);
        // Note: We keep pending_messages around so messages can still be queued
        // and will be delivered when the proxy reconnects
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

    /// Send a message to a session's proxy.
    /// If the proxy is disconnected, the message is queued for delivery when it reconnects.
    /// Returns true if the message was sent or queued successfully.
    pub fn send_to_session(&self, session_key: &SessionId, msg: ProxyMessage) -> bool {
        if let Some(sender) = self.sessions.get(session_key) {
            if sender.send(msg.clone()).is_ok() {
                return true;
            }
        }

        // Proxy not connected or send failed - queue the message
        self.queue_pending_message(session_key, msg)
    }

    /// Queue a message for a disconnected proxy
    fn queue_pending_message(&self, session_key: &SessionId, msg: ProxyMessage) -> bool {
        let mut queue = self
            .pending_messages
            .entry(session_key.clone())
            .or_default();

        // Enforce size limit - drop oldest messages if queue is full
        while queue.len() >= MAX_PENDING_MESSAGES_PER_SESSION {
            if let Some(dropped) = queue.pop_front() {
                warn!(
                    "Pending message queue full for session {}, dropping oldest message (age: {:?})",
                    session_key,
                    Instant::now().duration_since(dropped.queued_at)
                );
            }
        }

        queue.push_back(PendingMessage {
            msg,
            queued_at: Instant::now(),
        });

        info!(
            "Queued message for disconnected proxy, session: {}, queue size: {}",
            session_key,
            queue.len()
        );

        true
    }

    /// Get the number of pending messages for a session (for monitoring/debugging)
    #[allow(dead_code)]
    pub fn pending_message_count(&self, session_key: &SessionId) -> usize {
        self.pending_messages
            .get(session_key)
            .map(|q| q.len())
            .unwrap_or(0)
    }

    pub fn add_user_client(&self, user_id: Uuid, sender: ClientSender) {
        info!("Adding web client for user: {}", user_id);
        self.user_clients.entry(user_id).or_default().push(sender);
    }

    pub fn broadcast_to_user(&self, user_id: &Uuid, msg: ProxyMessage) {
        if let Some(mut clients) = self.user_clients.get_mut(user_id) {
            clients.retain(|sender| sender.send(msg.clone()).is_ok());
        }
    }

    pub fn get_all_user_ids(&self) -> Vec<Uuid> {
        self.user_clients.iter().map(|r| *r.key()).collect()
    }
}

/// Handle Claude output (both legacy ClaudeOutput and new SequencedOutput)
fn handle_claude_output(
    session_manager: &SessionManager,
    session_key: &Option<SessionId>,
    db_session_id: Option<Uuid>,
    db_pool: &crate::db::DbPool,
    tx: &ClientSender,
    content: serde_json::Value,
    seq: Option<u64>,
) {
    // Broadcast output to all web clients (always, even for replays)
    if let Some(ref key) = session_key {
        session_manager.broadcast_to_web_clients(
            key,
            ProxyMessage::ClaudeOutput {
                content: content.clone(),
            },
        );
    }

    // Check for deduplication if this is a sequenced message
    if let (Some(session_id), Some(seq_num)) = (db_session_id, seq) {
        let last_ack = session_manager
            .last_ack_seq
            .get(&session_id)
            .map(|v| *v)
            .unwrap_or(0);

        if seq_num <= last_ack {
            // This is a replay of an already-stored message, skip DB storage
            info!(
                "Skipping duplicate message seq={} (last_ack={})",
                seq_num, last_ack
            );
            // Still send ACK to confirm we have it
            let _ = tx.send(ProxyMessage::OutputAck {
                session_id,
                ack_seq: seq_num,
            });
            return;
        }
    }

    // Store message and update last_activity in DB
    if let (Some(session_id), Ok(mut conn)) = (db_session_id, db_pool.get()) {
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

            // Extract and store cost and token usage from result messages
            if role == "result" {
                let cost = content.get("total_cost_usd").and_then(|c| c.as_f64());
                // Token counts are nested under "usage" in the result message
                let usage = content.get("usage");
                let input_tokens = usage
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(|t| t.as_i64());
                let output_tokens = usage
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|t| t.as_i64());
                let cache_creation = usage
                    .and_then(|u| u.get("cache_creation_input_tokens"))
                    .and_then(|t| t.as_i64());
                let cache_read = usage
                    .and_then(|u| u.get("cache_read_input_tokens"))
                    .and_then(|t| t.as_i64());

                // Update cost if present
                if let Some(cost_val) = cost {
                    if let Err(e) = diesel::update(sessions::table.find(session_id))
                        .set(sessions::total_cost_usd.eq(cost_val))
                        .execute(&mut conn)
                    {
                        error!("Failed to update session cost: {}", e);
                    }
                }

                // Update token counts if present
                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_creation.is_some()
                    || cache_read.is_some()
                {
                    if let Err(e) = diesel::update(sessions::table.find(session_id))
                        .set((
                            sessions::input_tokens.eq(input_tokens.unwrap_or(0)),
                            sessions::output_tokens.eq(output_tokens.unwrap_or(0)),
                            sessions::cache_creation_tokens.eq(cache_creation.unwrap_or(0)),
                            sessions::cache_read_tokens.eq(cache_read.unwrap_or(0)),
                        ))
                        .execute(&mut conn)
                    {
                        error!("Failed to update session tokens: {}", e);
                    }
                }
            }

            // Truncate to keep only last 100 messages
            let _ = super::messages::truncate_session_messages_internal(&mut conn, session_id);
        }

        // Update last_activity
        let _ = diesel::update(sessions::table.find(session_id))
            .set(sessions::last_activity.eq(diesel::dsl::now))
            .execute(&mut conn);

        // Update last_ack tracker and send acknowledgment for sequenced messages
        if let Some(seq_num) = seq {
            session_manager
                .last_ack_seq
                .entry(session_id)
                .and_modify(|v| {
                    if seq_num > *v {
                        *v = seq_num;
                    }
                })
                .or_insert(seq_num);

            // Send acknowledgment back to proxy
            let _ = tx.send(ProxyMessage::OutputAck {
                session_id,
                ack_seq: seq_num,
            });
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
                            git_branch,
                            replay_after: _, // Not used for proxy connections
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
                                            sessions::git_branch.eq(&git_branch),
                                        ))
                                        .execute(&mut conn)
                                    {
                                        Ok(_) => {
                                            db_session_id = Some(existing_session.id);
                                            registration_success = true;
                                            info!(
                                                "Session reactivated in DB: {} ({}) branch: {:?}",
                                                session_name, claude_session_id, git_branch
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
                                            git_branch: git_branch.clone(),
                                        };

                                        match diesel::insert_into(sessions::table)
                                            .values(&new_session)
                                            .get_result::<crate::models::Session>(&mut conn)
                                        {
                                            Ok(session) => {
                                                // Create session_members entry for the owner
                                                use crate::schema::session_members;
                                                let new_member = NewSessionMember {
                                                    session_id: session.id,
                                                    user_id,
                                                    role: "owner".to_string(),
                                                };
                                                if let Err(e) =
                                                    diesel::insert_into(session_members::table)
                                                        .values(&new_member)
                                                        .execute(&mut conn)
                                                {
                                                    error!(
                                                        "Failed to create session_member: {}",
                                                        e
                                                    );
                                                }

                                                db_session_id = Some(session.id);
                                                registration_success = true;
                                                info!(
                                                    "Session created in DB: {} ({}) branch: {:?}",
                                                    session_name, claude_session_id, git_branch
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
                                            git_branch: git_branch.clone(),
                                        };

                                        match diesel::insert_into(sessions::table)
                                            .values(&new_session)
                                            .get_result::<crate::models::Session>(&mut conn)
                                        {
                                            Ok(session) => {
                                                // Create session_members entry for the owner
                                                use crate::schema::session_members;
                                                let new_member = NewSessionMember {
                                                    session_id: session.id,
                                                    user_id,
                                                    role: "owner".to_string(),
                                                };
                                                if let Err(e) =
                                                    diesel::insert_into(session_members::table)
                                                        .values(&new_member)
                                                        .execute(&mut conn)
                                                {
                                                    error!(
                                                        "Failed to create session_member: {}",
                                                        e
                                                    );
                                                }

                                                db_session_id = Some(session.id);
                                                registration_success = true;
                                                info!(
                                                    "Session persisted to DB: {} ({}) branch: {:?}",
                                                    session_name, claude_session_id, git_branch
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
                            // Legacy: Handle unsequenced output (for backwards compatibility)
                            handle_claude_output(
                                &session_manager,
                                &session_key,
                                db_session_id,
                                &db_pool,
                                &tx,
                                content,
                                None, // No sequence number
                            );
                        }
                        ProxyMessage::SequencedOutput { seq, content } => {
                            // New: Handle sequenced output with acknowledgment
                            handle_claude_output(
                                &session_manager,
                                &session_key,
                                db_session_id,
                                &db_pool,
                                &tx,
                                content,
                                Some(seq),
                            );
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
                            // Store permission request in database for replay on reconnect
                            if let (Some(session_id), Ok(mut conn)) = (db_session_id, db_pool.get())
                            {
                                use crate::schema::pending_permission_requests;

                                let new_request = crate::models::NewPendingPermissionRequest {
                                    session_id,
                                    request_id: request_id.clone(),
                                    tool_name: tool_name.clone(),
                                    input: input.clone(),
                                    permission_suggestions: if permission_suggestions.is_empty() {
                                        None
                                    } else {
                                        Some(
                                            serde_json::to_value(&permission_suggestions)
                                                .unwrap_or_default(),
                                        )
                                    },
                                };

                                // Use upsert to replace any existing pending request for this session
                                if let Err(e) =
                                    diesel::insert_into(pending_permission_requests::table)
                                        .values(&new_request)
                                        .on_conflict(pending_permission_requests::session_id)
                                        .do_update()
                                        .set((
                                            pending_permission_requests::request_id.eq(&request_id),
                                            pending_permission_requests::tool_name.eq(&tool_name),
                                            pending_permission_requests::input.eq(&input),
                                            pending_permission_requests::permission_suggestions.eq(
                                                if permission_suggestions.is_empty() {
                                                    None
                                                } else {
                                                    Some(
                                                        serde_json::to_value(
                                                            &permission_suggestions,
                                                        )
                                                        .unwrap_or_default(),
                                                    )
                                                },
                                            ),
                                            pending_permission_requests::created_at
                                                .eq(diesel::dsl::now),
                                        ))
                                        .execute(&mut conn)
                                {
                                    error!("Failed to store pending permission request: {}", e);
                                }
                            }

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
                        ProxyMessage::SessionUpdate {
                            session_id: update_session_id,
                            git_branch,
                        } => {
                            // Update session metadata in DB
                            if let (Some(current_session_id), Ok(mut conn)) =
                                (db_session_id, db_pool.get())
                            {
                                // Verify the session_id matches to prevent spoofing
                                if current_session_id == update_session_id {
                                    use crate::schema::sessions;
                                    if let Err(e) =
                                        diesel::update(sessions::table.find(current_session_id))
                                            .set(sessions::git_branch.eq(&git_branch))
                                            .execute(&mut conn)
                                    {
                                        error!("Failed to update git_branch: {}", e);
                                    } else {
                                        info!(
                                            "Updated git_branch for session {}: {:?}",
                                            current_session_id, git_branch
                                        );

                                        // Broadcast to web clients so they update immediately
                                        if let Some(ref key) = session_key {
                                            session_manager.broadcast_to_web_clients(
                                                key,
                                                ProxyMessage::SessionUpdate {
                                                    session_id: current_session_id,
                                                    git_branch: git_branch.clone(),
                                                },
                                            );
                                        }
                                    }
                                } else {
                                    warn!(
                                        "SessionUpdate session_id mismatch: {} != {}",
                                        update_session_id, current_session_id
                                    );
                                }
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

/// Verify that a user has access to a session (is a member with any role)
fn verify_session_access(
    app_state: &AppState,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<crate::models::Session, ()> {
    let mut conn = app_state.db_pool.get().map_err(|_| ())?;
    use crate::schema::{session_members, sessions};
    sessions::table
        .inner_join(session_members::table.on(session_members::session_id.eq(sessions::id)))
        .filter(sessions::id.eq(session_id))
        .filter(session_members::user_id.eq(user_id))
        .select(crate::models::Session::as_select())
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

    // Register this client for user-level broadcasts (like spend updates)
    session_manager.add_user_client(user_id, tx.clone());

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
                            git_branch: _,
                            replay_after,
                        } => {
                            // Verify the user has access to this session before allowing connection
                            match verify_session_access(&app_state, session_id, user_id) {
                                Ok(_session) => {
                                    // User has access to this session, allow connection
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
                                    // If replay_after is set, only send messages after that timestamp
                                    if let Ok(mut conn) = db_pool.get() {
                                        use crate::schema::messages;

                                        // Parse replay_after timestamp if provided
                                        let replay_after_time =
                                            replay_after.as_ref().and_then(|ts| {
                                                chrono::NaiveDateTime::parse_from_str(
                                                    ts,
                                                    "%Y-%m-%dT%H:%M:%S%.f",
                                                )
                                                .or_else(|_| {
                                                    chrono::NaiveDateTime::parse_from_str(
                                                        ts,
                                                        "%Y-%m-%dT%H:%M:%S",
                                                    )
                                                })
                                                .ok()
                                            });

                                        let history: Vec<crate::models::Message> =
                                            if let Some(after) = replay_after_time {
                                                messages::table
                                                    .filter(messages::session_id.eq(session_id))
                                                    .filter(messages::created_at.gt(after))
                                                    .order(messages::created_at.asc())
                                                    .load(&mut conn)
                                                    .unwrap_or_default()
                                            } else {
                                                messages::table
                                                    .filter(messages::session_id.eq(session_id))
                                                    .order(messages::created_at.asc())
                                                    .load(&mut conn)
                                                    .unwrap_or_default()
                                            };

                                        info!(
                                            "Sending {} historical messages to web client (replay_after: {:?})",
                                            history.len(), replay_after
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

                                        // Replay pending permission request if one exists
                                        use crate::schema::pending_permission_requests;
                                        if let Ok(pending) = pending_permission_requests::table
                                            .filter(
                                                pending_permission_requests::session_id
                                                    .eq(session_id),
                                            )
                                            .first::<crate::models::PendingPermissionRequest>(
                                                &mut conn,
                                            )
                                        {
                                            info!(
                                                "Replaying pending permission request for session {}: {} ({})",
                                                session_id, pending.tool_name, pending.request_id
                                            );

                                            // Convert stored permission_suggestions back to Vec
                                            let suggestions: Vec<serde_json::Value> = pending
                                                .permission_suggestions
                                                .and_then(|v| serde_json::from_value(v).ok())
                                                .unwrap_or_default();

                                            let _ = tx.send(ProxyMessage::PermissionRequest {
                                                request_id: pending.request_id,
                                                tool_name: pending.tool_name,
                                                input: pending.input,
                                                permission_suggestions: suggestions,
                                            });
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
                                if let Some(session_id) = verified_session_id {
                                    info!("Web client sending PermissionResponse: {} -> {} (permissions: {}, reason: {:?})",
                                          request_id, if allow { "allow" } else { "deny" }, permissions.len(), reason);

                                    // Clear pending permission request from database
                                    if let Ok(mut conn) = db_pool.get() {
                                        use crate::schema::pending_permission_requests;
                                        if let Err(e) = diesel::delete(
                                            pending_permission_requests::table.filter(
                                                pending_permission_requests::session_id
                                                    .eq(session_id),
                                            ),
                                        )
                                        .execute(&mut conn)
                                        {
                                            error!(
                                                "Failed to clear pending permission request: {}",
                                                e
                                            );
                                        }
                                    }

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
