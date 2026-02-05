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
use shared::protocol::{MAX_PENDING_MESSAGES_PER_SESSION, MAX_PENDING_MESSAGE_AGE_SECS};

/// Maximum age of pending messages before they're dropped
const MAX_PENDING_MESSAGE_AGE: Duration = Duration::from_secs(MAX_PENDING_MESSAGE_AGE_SECS);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_heartbeat() -> ProxyMessage {
        ProxyMessage::Heartbeat
    }

    fn make_output(n: u32) -> ProxyMessage {
        ProxyMessage::ClaudeOutput {
            content: serde_json::json!({"n": n}),
        }
    }

    #[test]
    fn session_register_and_send() {
        let mgr = SessionManager::new();
        let (tx, mut rx) = mpsc::unbounded_channel();

        mgr.register_session("s1".into(), tx);

        // Sending to a registered session should succeed
        assert!(mgr.send_to_session(&"s1".into(), make_heartbeat()));

        // Message should be receivable
        let msg = rx.try_recv().unwrap();
        assert!(matches!(msg, ProxyMessage::Heartbeat));
    }

    #[test]
    fn send_to_unregistered_queues_pending() {
        let mgr = SessionManager::new();

        // No session registered, message should be queued
        assert!(mgr.send_to_session(&"s1".into(), make_output(1)));
        assert!(mgr.send_to_session(&"s1".into(), make_output(2)));

        // Now register - pending messages should replay
        let (tx, mut rx) = mpsc::unbounded_channel();
        mgr.register_session("s1".into(), tx);

        let msg1 = rx.try_recv().unwrap();
        let msg2 = rx.try_recv().unwrap();
        assert!(matches!(msg1, ProxyMessage::ClaudeOutput { .. }));
        assert!(matches!(msg2, ProxyMessage::ClaudeOutput { .. }));

        // Queue should be drained
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn pending_queue_overflow_drops_oldest() {
        let mgr = SessionManager::new();

        // Fill the queue beyond MAX_PENDING_MESSAGES_PER_SESSION
        for i in 0..(MAX_PENDING_MESSAGES_PER_SESSION + 10) as u32 {
            mgr.send_to_session(&"s1".into(), make_output(i));
        }

        // Register and collect all replayed messages
        let (tx, mut rx) = mpsc::unbounded_channel();
        mgr.register_session("s1".into(), tx);

        let mut received = vec![];
        while let Ok(msg) = rx.try_recv() {
            received.push(msg);
        }

        assert_eq!(received.len(), MAX_PENDING_MESSAGES_PER_SESSION);

        // First received should be the 11th message (first 10 were dropped)
        if let ProxyMessage::ClaudeOutput { content } = &received[0] {
            assert_eq!(content["n"], 10);
        } else {
            panic!("Expected ClaudeOutput");
        }
    }

    #[test]
    fn unregister_removes_session() {
        let mgr = SessionManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();

        mgr.register_session("s1".into(), tx);
        assert!(mgr.sessions.contains_key("s1"));

        mgr.unregister_session(&"s1".into());
        assert!(!mgr.sessions.contains_key("s1"));
    }

    #[test]
    fn broadcast_to_web_clients() {
        let mgr = SessionManager::new();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        mgr.add_web_client("s1".into(), tx1);
        mgr.add_web_client("s1".into(), tx2);

        mgr.broadcast_to_web_clients(&"s1".into(), make_heartbeat());

        assert!(matches!(rx1.try_recv().unwrap(), ProxyMessage::Heartbeat));
        assert!(matches!(rx2.try_recv().unwrap(), ProxyMessage::Heartbeat));
    }

    #[test]
    fn broadcast_removes_closed_clients() {
        let mgr = SessionManager::new();
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();

        mgr.add_web_client("s1".into(), tx1);
        mgr.add_web_client("s1".into(), tx2);

        // Drop rx1 to simulate a disconnected client
        drop(rx1);

        mgr.broadcast_to_web_clients(&"s1".into(), make_heartbeat());

        // tx2's client should still receive
        assert!(matches!(rx2.try_recv().unwrap(), ProxyMessage::Heartbeat));

        // The dead client (tx1) should have been removed
        let clients = mgr.web_clients.get("s1").unwrap();
        assert_eq!(clients.len(), 1);
    }

    #[test]
    fn broadcast_to_user() {
        let mgr = SessionManager::new();
        let user_id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::unbounded_channel();

        mgr.add_user_client(user_id, tx);
        mgr.broadcast_to_user(&user_id, make_heartbeat());

        assert!(matches!(rx.try_recv().unwrap(), ProxyMessage::Heartbeat));
    }

    #[test]
    fn broadcast_to_all() {
        let mgr = SessionManager::new();
        let (session_tx, mut session_rx) = mpsc::unbounded_channel();
        let (web_tx, mut web_rx) = mpsc::unbounded_channel();
        let (user_tx, mut user_rx) = mpsc::unbounded_channel();

        mgr.register_session("s1".into(), session_tx);
        mgr.add_web_client("s1".into(), web_tx);
        mgr.add_user_client(Uuid::new_v4(), user_tx);

        mgr.broadcast_to_all(make_heartbeat());

        assert!(matches!(
            session_rx.try_recv().unwrap(),
            ProxyMessage::Heartbeat
        ));
        assert!(matches!(
            web_rx.try_recv().unwrap(),
            ProxyMessage::Heartbeat
        ));
        assert!(matches!(
            user_rx.try_recv().unwrap(),
            ProxyMessage::Heartbeat
        ));
    }

    #[test]
    fn truncation_queue_and_drain() {
        let mgr = SessionManager::new();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        mgr.queue_truncation(id1);
        mgr.queue_truncation(id2);
        mgr.queue_truncation(id1); // duplicate should be idempotent

        let drained = mgr.drain_pending_truncations();
        assert_eq!(drained.len(), 2);
        assert!(drained.contains(&id1));
        assert!(drained.contains(&id2));

        // After draining, should be empty
        let drained2 = mgr.drain_pending_truncations();
        assert!(drained2.is_empty());
    }

    #[test]
    fn last_ack_seq_tracking() {
        let mgr = SessionManager::new();
        let session_id = Uuid::new_v4();

        // Initially no ack tracked
        assert!(mgr.last_ack_seq.get(&session_id).is_none());

        // Insert and verify
        mgr.last_ack_seq.insert(session_id, 5);
        assert_eq!(*mgr.last_ack_seq.get(&session_id).unwrap(), 5);

        // Update with higher value
        mgr.last_ack_seq.entry(session_id).and_modify(|v| {
            if 10 > *v {
                *v = 10;
            }
        });
        assert_eq!(*mgr.last_ack_seq.get(&session_id).unwrap(), 10);

        // Should not regress with lower value
        mgr.last_ack_seq.entry(session_id).and_modify(|v| {
            if 3 > *v {
                *v = 3;
            }
        });
        assert_eq!(*mgr.last_ack_seq.get(&session_id).unwrap(), 10);
    }

    #[test]
    fn send_to_disconnected_session_queues_and_replays() {
        let mgr = SessionManager::new();
        let (tx, _rx) = mpsc::unbounded_channel();

        // Register then unregister to simulate disconnect
        mgr.register_session("s1".into(), tx);
        mgr.unregister_session(&"s1".into());

        // Send while disconnected
        mgr.send_to_session(&"s1".into(), make_output(1));
        mgr.send_to_session(&"s1".into(), make_output(2));

        // Reconnect
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        mgr.register_session("s1".into(), tx2);

        // Should receive the queued messages
        let msg1 = rx2.try_recv().unwrap();
        let msg2 = rx2.try_recv().unwrap();
        assert!(matches!(msg1, ProxyMessage::ClaudeOutput { .. }));
        assert!(matches!(msg2, ProxyMessage::ClaudeOutput { .. }));
    }

    #[test]
    fn get_all_user_ids() {
        let mgr = SessionManager::new();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        mgr.add_user_client(id1, tx1);
        mgr.add_user_client(id2, tx2);

        let ids = mgr.get_all_user_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }
}
