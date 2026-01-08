use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use crate::AppState;

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
    let session_manager = app_state.session_manager.clone();
    ws.on_upgrade(|socket| handle_session_socket(socket, session_manager))
}

async fn handle_session_socket(socket: WebSocket, session_manager: SessionManager) {
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

    // Handle incoming messages
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
                            // Generate or use session key
                            let key = session_name.clone();
                            session_key = Some(key.clone());

                            session_manager.register_session(key, tx.clone());
                            info!("Session registered: {}", session_name);
                        }
                        ProxyMessage::ClaudeOutput { content } => {
                            // Broadcast output to all web clients
                            if let Some(ref key) = session_key {
                                session_manager
                                    .broadcast_to_web_clients(key, ProxyMessage::ClaudeOutput { content });
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

    // Cleanup
    if let Some(key) = session_key {
        session_manager.unregister_session(&key);
    }

    send_task.abort();
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
                                if !session_manager
                                    .send_to_session(key, ProxyMessage::ClaudeInput { content })
                                {
                                    warn!("Failed to send to session, session not found");
                                }
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
