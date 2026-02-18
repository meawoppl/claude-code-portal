//! WebSocket connection management for SessionView

use crate::utils;
use futures_util::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use shared::api::ErrorMessage;
use shared::ProxyMessage;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;
use yew::Callback;

use super::types::{PendingPermission, WsSender};
use std::cell::RefCell;
use std::rc::Rc;

/// Messages that can be sent from WebSocket handlers
pub enum WsEvent {
    Connected(WsSender),
    Error(String),
    Output(String),
    HistoryBatch(Vec<String>),
    Permission(PendingPermission),
    BranchChanged(Option<String>, Option<String>),
}

/// Connect to WebSocket and start receiving messages.
/// Returns immediately, spawns async task to handle connection.
pub fn connect_websocket(
    session_id: Uuid,
    replay_after: Option<String>,
    resuming: bool,
    on_event: Callback<WsEvent>,
) {
    spawn_local(async move {
        let ws_endpoint = utils::ws_url("/ws/client");
        match WebSocket::open(&ws_endpoint) {
            Ok(ws) => {
                let (mut sender, mut receiver) = ws.split();

                let register_msg = ProxyMessage::Register {
                    session_id,
                    session_name: session_id.to_string(),
                    auth_token: None,
                    working_directory: String::new(),
                    resuming,
                    git_branch: None,
                    replay_after,
                    client_version: None,
                    replaces_session_id: None,
                    hostname: None,
                    launcher_id: None,
                };

                if let Ok(json) = serde_json::to_string(&register_msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        on_event.emit(WsEvent::Error("Failed to send registration".to_string()));
                        return;
                    }
                }

                let sender = Rc::new(RefCell::new(Some(sender)));
                on_event.emit(WsEvent::Connected(sender));

                while let Some(msg) = receiver.next().await {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                                handle_proxy_message(proxy_msg, &on_event);
                            }
                        }
                        Err(e) => {
                            log::error!("WebSocket error: {:?}", e);
                            on_event.emit(WsEvent::Error(format!("{:?}", e)));
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to connect WebSocket: {:?}", e);
                on_event.emit(WsEvent::Error(format!("{:?}", e)));
            }
        }
    });
}

/// Handle incoming ProxyMessage and emit appropriate events
fn handle_proxy_message(msg: ProxyMessage, on_event: &Callback<WsEvent>) {
    match msg {
        ProxyMessage::ClaudeOutput { content } => {
            on_event.emit(WsEvent::Output(content.to_string()));
        }
        ProxyMessage::HistoryBatch { messages } => {
            let strings: Vec<String> = messages.into_iter().map(|v| v.to_string()).collect();
            on_event.emit(WsEvent::HistoryBatch(strings));
        }
        ProxyMessage::PermissionRequest {
            request_id,
            tool_name,
            input,
            permission_suggestions,
        } => {
            on_event.emit(WsEvent::Permission(PendingPermission {
                request_id,
                tool_name,
                input,
                permission_suggestions,
            }));
        }
        ProxyMessage::Error { message } => {
            let error_msg = ErrorMessage::new(message);
            let error_json = serde_json::to_string(&error_msg).unwrap_or_default();
            on_event.emit(WsEvent::Output(error_json));
        }
        ProxyMessage::SessionUpdate {
            session_id: _,
            git_branch,
            pr_url,
        } => {
            on_event.emit(WsEvent::BranchChanged(git_branch, pr_url));
        }
        _ => {}
    }
}

/// Send a message over WebSocket
pub fn send_message(sender: &WsSender, msg: ProxyMessage) {
    let sender_rc = sender.clone();
    spawn_local(async move {
        if let Ok(json) = serde_json::to_string(&msg) {
            let maybe_sender = sender_rc.borrow_mut().take();
            if let Some(mut sender) = maybe_sender {
                let _ = sender.send(Message::Text(json)).await;
                *sender_rc.borrow_mut() = Some(sender);
            }
        }
    });
}
