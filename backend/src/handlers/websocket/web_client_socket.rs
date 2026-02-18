use super::permissions::{handle_permission_response, replay_pending_permission};
use super::{ClientSender, SessionId, SessionManager};
use crate::models::NewPendingInput;
use crate::AppState;
use axum::extract::ws::{Message, WebSocket};
use diesel::prelude::*;
use futures_util::{SinkExt, StreamExt};
use shared::api::RawMessageFallback;
use shared::{ProxyMessage, SendMode};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

pub async fn handle_web_client_socket(socket: WebSocket, app_state: Arc<AppState>, user_id: Uuid) {
    let session_manager = app_state.session_manager.clone();
    let db_pool = app_state.db_pool.clone();
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ProxyMessage>();

    let mut session_key: Option<SessionId> = None;
    let mut verified_session_id: Option<Uuid> = None;

    session_manager.add_user_client(user_id, tx.clone());

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(proxy_msg) = serde_json::from_str::<ProxyMessage>(&text) {
                    let should_break = handle_web_client_message(
                        proxy_msg,
                        &app_state,
                        &session_manager,
                        &db_pool,
                        &tx,
                        user_id,
                        &mut session_key,
                        &mut verified_session_id,
                    );
                    if should_break {
                        break;
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

/// Returns true if the connection should be closed
#[allow(clippy::too_many_arguments)]
fn handle_web_client_message(
    proxy_msg: ProxyMessage,
    app_state: &AppState,
    session_manager: &SessionManager,
    db_pool: &crate::db::DbPool,
    tx: &ClientSender,
    user_id: Uuid,
    session_key: &mut Option<SessionId>,
    verified_session_id: &mut Option<Uuid>,
) -> bool {
    match proxy_msg {
        ProxyMessage::Register {
            session_id,
            session_name,
            replay_after,
            ..
        } => handle_web_register(
            app_state,
            session_manager,
            db_pool,
            tx,
            user_id,
            session_id,
            &session_name,
            replay_after,
            session_key,
            verified_session_id,
        ),
        ProxyMessage::ClaudeInput { content, send_mode } => {
            handle_web_input(
                session_manager,
                db_pool,
                session_key,
                *verified_session_id,
                content,
                send_mode,
            );
            false
        }
        ProxyMessage::PermissionResponse {
            request_id,
            allow,
            input,
            permissions,
            reason,
        } => {
            if let (Some(ref key), Some(session_id)) = (session_key, *verified_session_id) {
                handle_permission_response(
                    session_manager,
                    key,
                    session_id,
                    db_pool,
                    request_id,
                    allow,
                    input,
                    permissions,
                    reason,
                );
            } else {
                warn!("Web client tried to send PermissionResponse without verified session");
            }
            false
        }
        _ => false,
    }
}

/// Handle web client registration. Returns true if the connection should be closed.
#[allow(clippy::too_many_arguments)]
fn handle_web_register(
    app_state: &AppState,
    session_manager: &SessionManager,
    db_pool: &crate::db::DbPool,
    tx: &ClientSender,
    user_id: Uuid,
    session_id: Uuid,
    session_name: &str,
    replay_after: Option<String>,
    session_key: &mut Option<SessionId>,
    verified_session_id: &mut Option<Uuid>,
) -> bool {
    match super::auth::verify_session_access(app_state, session_id, user_id) {
        Ok(_session) => {
            let key = session_id.to_string();
            *session_key = Some(key.clone());
            *verified_session_id = Some(session_id);

            session_manager.add_web_client(key, tx.clone());
            info!(
                "Web client connected to session: {} ({}) for user {}",
                session_name, session_id, user_id
            );

            replay_history(db_pool, tx, session_id, replay_after);
            replay_pending_permission(db_pool, session_id, tx);
            false
        }
        Err(_) => {
            warn!(
                "User {} attempted to access session {} they don't own",
                user_id, session_id
            );
            let _ = tx.send(ProxyMessage::Error {
                message: "Access denied: you don't own this session".to_string(),
            });
            true // close connection
        }
    }
}

/// Send historical messages from DB to a newly connected web client
fn replay_history(
    db_pool: &crate::db::DbPool,
    tx: &ClientSender,
    session_id: Uuid,
    replay_after: Option<String>,
) {
    let mut conn = match db_pool.get() {
        Ok(conn) => conn,
        Err(e) => {
            error!(
                "Failed to get database connection for history replay: {}",
                e
            );
            return;
        }
    };

    use crate::schema::messages;

    let replay_after_time = replay_after.as_ref().and_then(|ts| {
        chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S"))
            .ok()
    });

    let history: Vec<crate::models::Message> = if let Some(after) = replay_after_time {
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
        history.len(),
        replay_after
    );

    if history.is_empty() {
        return;
    }

    let messages: Vec<serde_json::Value> = history
        .into_iter()
        .map(|msg| {
            serde_json::from_str::<serde_json::Value>(&msg.content).unwrap_or_else(|_| {
                let fallback = RawMessageFallback {
                    message_type: msg.role,
                    content: msg.content,
                };
                serde_json::to_value(&fallback).unwrap_or_default()
            })
        })
        .collect();

    let _ = tx.send(ProxyMessage::HistoryBatch { messages });
}

fn handle_web_input(
    session_manager: &SessionManager,
    db_pool: &crate::db::DbPool,
    session_key: &Option<SessionId>,
    verified_session_id: Option<Uuid>,
    content: serde_json::Value,
    send_mode: Option<SendMode>,
) {
    let Some(ref key) = session_key else {
        warn!("Web client tried to send ClaudeInput but no session_key set (not registered?)");
        return;
    };
    let Some(session_id) = verified_session_id else {
        warn!("Attempted ClaudeInput without verified session ownership");
        return;
    };

    info!("Web client sending ClaudeInput to session: {}", key);

    let seq = match db_pool.get() {
        Ok(mut conn) => {
            use crate::schema::{pending_inputs, sessions};

            let next_seq: i64 = diesel::update(sessions::table.find(session_id))
                .set(sessions::input_seq.eq(sessions::input_seq + 1))
                .returning(sessions::input_seq)
                .get_result(&mut conn)
                .unwrap_or(1);

            let new_input = NewPendingInput {
                session_id,
                seq_num: next_seq,
                content: serde_json::to_string(&content).unwrap_or_default(),
            };
            if let Err(e) = diesel::insert_into(pending_inputs::table)
                .values(&new_input)
                .execute(&mut conn)
            {
                error!("Failed to store pending input: {}", e);
            }
            next_seq
        }
        Err(e) => {
            error!("Failed to get db connection for pending input: {}", e);
            0
        }
    };

    if seq > 0 {
        if !session_manager.send_to_session(
            key,
            ProxyMessage::SequencedInput {
                session_id,
                seq,
                content,
                send_mode,
            },
        ) {
            warn!(
                "Failed to send to session '{}', session not found in SessionManager (input queued)",
                key
            );
        }
    } else if !session_manager
        .send_to_session(key, ProxyMessage::ClaudeInput { content, send_mode })
    {
        warn!(
            "Failed to send to session '{}', session not found in SessionManager",
            key
        );
    }
}
