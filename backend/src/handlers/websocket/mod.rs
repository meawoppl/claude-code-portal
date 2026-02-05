mod auth;
mod message_handlers;
mod permissions;
mod proxy_socket;
mod registration;
mod web_client_socket;

use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dashmap::{DashMap, DashSet};
use shared::ProxyMessage;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tower_cookies::Cookies;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::AppState;

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
    pub sessions: Arc<DashMap<SessionId, ClientSender>>,
    pub web_clients: Arc<DashMap<SessionId, Vec<ClientSender>>>,
    pub user_clients: Arc<DashMap<Uuid, Vec<ClientSender>>>,
    pub last_ack_seq: Arc<DashMap<Uuid, u64>>,
    pending_messages: Arc<DashMap<SessionId, VecDeque<PendingMessage>>>,
    pub pending_truncations: Arc<DashSet<Uuid>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            web_clients: Arc::new(DashMap::new()),
            user_clients: Arc::new(DashMap::new()),
            last_ack_seq: Arc::new(DashMap::new()),
            pending_messages: Arc::new(DashMap::new()),
            pending_truncations: Arc::new(DashSet::new()),
        }
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_session(&self, session_key: SessionId, sender: ClientSender) {
        info!("Registering session: {}", session_key);

        let pending_count = self.replay_pending_messages(&session_key, &sender);
        if pending_count > 0 {
            info!(
                "Replayed {} pending messages to reconnected proxy for session: {}",
                pending_count, session_key
            );
        }

        self.sessions.insert(session_key, sender);
    }

    fn replay_pending_messages(&self, session_key: &SessionId, sender: &ClientSender) -> usize {
        let mut replayed = 0;
        let now = Instant::now();

        if let Some(mut pending) = self.pending_messages.get_mut(session_key) {
            while let Some(pending_msg) = pending.pop_front() {
                if now.duration_since(pending_msg.queued_at) < MAX_PENDING_MESSAGE_AGE {
                    if sender.send(pending_msg.msg).is_ok() {
                        replayed += 1;
                    } else {
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

        self.pending_messages.remove(session_key);
        replayed
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
            if sender.send(msg.clone()).is_ok() {
                return true;
            }
        }

        self.queue_pending_message(session_key, msg)
    }

    fn queue_pending_message(&self, session_key: &SessionId, msg: ProxyMessage) -> bool {
        let mut queue = self
            .pending_messages
            .entry(session_key.clone())
            .or_default();

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

    pub fn broadcast_to_all(&self, msg: ProxyMessage) {
        for entry in self.sessions.iter() {
            let _ = entry.value().send(msg.clone());
        }

        for mut entry in self.web_clients.iter_mut() {
            entry
                .value_mut()
                .retain(|sender| sender.send(msg.clone()).is_ok());
        }

        for mut entry in self.user_clients.iter_mut() {
            entry
                .value_mut()
                .retain(|sender| sender.send(msg.clone()).is_ok());
        }
    }

    pub fn queue_truncation(&self, session_id: Uuid) {
        self.pending_truncations.insert(session_id);
    }

    pub fn drain_pending_truncations(&self) -> Vec<Uuid> {
        let ids: Vec<Uuid> = self.pending_truncations.iter().map(|r| *r).collect();
        for id in &ids {
            self.pending_truncations.remove(id);
        }
        ids
    }
}

// Public handler functions

pub async fn handle_session_websocket(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(|socket| proxy_socket::handle_session_socket(socket, app_state))
}

pub async fn handle_web_client_websocket(
    ws: WebSocketUpgrade,
    State(app_state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Response {
    let user_id = match auth::extract_user_id_from_cookies(&app_state, &cookies) {
        Some(id) => id,
        None => {
            warn!("Unauthenticated WebSocket connection attempt to /ws/client");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };

    info!("Authenticated WebSocket upgrade for user: {}", user_id);
    ws.on_upgrade(move |socket| {
        web_client_socket::handle_web_client_socket(socket, app_state, user_id)
    })
}
