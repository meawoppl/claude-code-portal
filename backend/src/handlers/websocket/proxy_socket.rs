use super::message_handlers::{handle_claude_output, replay_pending_inputs_from_db};
use super::permissions::handle_permission_request;
use super::registration::{register_or_update_session, RegistrationParams};
use super::{ClientSender, SessionId, SessionManager};
use crate::AppState;
use axum::extract::ws::{Message, WebSocket};
use diesel::prelude::*;
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

pub async fn handle_session_socket(socket: WebSocket, app_state: Arc<AppState>) {
    let session_manager = app_state.session_manager.clone();
    let db_pool = app_state.db_pool.clone();
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ProxyMessage>();

    let mut session_key: Option<SessionId> = None;
    let mut db_session_id: Option<Uuid> = None;

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
                    handle_proxy_message(
                        proxy_msg,
                        &app_state,
                        &session_manager,
                        &db_pool,
                        &tx,
                        &mut session_key,
                        &mut db_session_id,
                    );
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
        match db_pool.get() {
            Ok(mut conn) => {
                use crate::schema::sessions;
                let _ = diesel::update(sessions::table.find(session_id))
                    .set(sessions::status.eq("disconnected"))
                    .execute(&mut conn);
            }
            Err(e) => {
                error!(
                    "Failed to get database connection for session disconnect cleanup: {}",
                    e
                );
            }
        }
    }

    if let Some(key) = session_key {
        session_manager.unregister_session(&key);
    }

    send_task.abort();
}

#[allow(clippy::too_many_arguments)]
fn handle_proxy_message(
    proxy_msg: ProxyMessage,
    app_state: &AppState,
    session_manager: &SessionManager,
    db_pool: &crate::db::DbPool,
    tx: &ClientSender,
    session_key: &mut Option<SessionId>,
    db_session_id: &mut Option<Uuid>,
) {
    match proxy_msg {
        ProxyMessage::Register {
            session_id: claude_session_id,
            session_name,
            auth_token,
            working_directory,
            resuming,
            git_branch,
            replay_after: _,
            client_version,
            replaces_session_id,
            hostname,
            launcher_id,
        } => {
            let key = claude_session_id.to_string();
            *session_key = Some(key.clone());

            session_manager.register_session(key.clone(), tx.clone());

            let params = RegistrationParams {
                claude_session_id,
                session_name: &session_name,
                auth_token: auth_token.as_deref(),
                working_directory: &working_directory,
                resuming,
                git_branch: &git_branch,
                client_version: &client_version,
                session_key: &key,
                replaces_session_id,
                hostname: hostname.as_deref().unwrap_or("unknown"),
                launcher_id,
            };
            let result = register_or_update_session(app_state, &params);

            *db_session_id = result.session_id;

            let _ = tx.send(ProxyMessage::RegisterAck {
                success: result.success,
                session_id: claude_session_id,
                error: result.error,
            });

            info!(
                "Session registered: {} ({}) - success: {}, client_version: {:?}",
                session_name, claude_session_id, result.success, client_version
            );

            if result.success {
                if let Some(session_id) = *db_session_id {
                    replay_pending_inputs_from_db(db_pool, session_id, tx);
                }
            }
        }
        ProxyMessage::ClaudeOutput { content } => {
            handle_claude_output(
                session_manager,
                session_key,
                *db_session_id,
                db_pool,
                tx,
                content,
                None,
            );
        }
        ProxyMessage::SequencedOutput { seq, content } => {
            handle_claude_output(
                session_manager,
                session_key,
                *db_session_id,
                db_pool,
                tx,
                content,
                Some(seq),
            );
        }
        ProxyMessage::Heartbeat => {
            let _ = tx.send(ProxyMessage::Heartbeat);
        }
        ProxyMessage::PermissionRequest {
            request_id,
            tool_name,
            input,
            permission_suggestions,
        } => {
            handle_permission_request(
                session_manager,
                session_key,
                *db_session_id,
                db_pool,
                request_id,
                tool_name,
                input,
                permission_suggestions,
            );
        }
        ProxyMessage::SessionUpdate {
            session_id: update_session_id,
            git_branch,
        } => {
            handle_session_update(
                session_manager,
                session_key,
                *db_session_id,
                db_pool,
                update_session_id,
                git_branch,
            );
        }
        ProxyMessage::InputAck {
            session_id: ack_session_id,
            ack_seq,
        } => {
            handle_input_ack(*db_session_id, db_pool, ack_session_id, ack_seq);
        }
        _ => {}
    }
}

fn handle_session_update(
    session_manager: &SessionManager,
    session_key: &Option<SessionId>,
    db_session_id: Option<Uuid>,
    db_pool: &crate::db::DbPool,
    update_session_id: Uuid,
    git_branch: Option<String>,
) {
    let Some(current_session_id) = db_session_id else {
        return;
    };
    let mut conn = match db_pool.get() {
        Ok(conn) => conn,
        Err(e) => {
            error!(
                "Failed to get database connection for session update: {}",
                e
            );
            return;
        }
    };

    if current_session_id != update_session_id {
        warn!(
            "SessionUpdate session_id mismatch: {} != {}",
            update_session_id, current_session_id
        );
        return;
    }

    use crate::schema::sessions;
    if let Err(e) = diesel::update(sessions::table.find(current_session_id))
        .set(sessions::git_branch.eq(&git_branch))
        .execute(&mut conn)
    {
        error!("Failed to update git_branch: {}", e);
    } else {
        info!(
            "Updated git_branch for session {}: {:?}",
            current_session_id, git_branch
        );

        if let Some(ref key) = session_key {
            session_manager.broadcast_to_web_clients(
                key,
                ProxyMessage::SessionUpdate {
                    session_id: current_session_id,
                    git_branch,
                },
            );
        }
    }
}

fn handle_input_ack(
    db_session_id: Option<Uuid>,
    db_pool: &crate::db::DbPool,
    ack_session_id: Uuid,
    ack_seq: i64,
) {
    let Some(current_session_id) = db_session_id else {
        return;
    };

    if ack_session_id != current_session_id {
        warn!(
            "InputAck session_id mismatch: {} != {}",
            ack_session_id, current_session_id
        );
        return;
    }

    let mut conn = match db_pool.get() {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to get database connection for input ack: {}", e);
            return;
        }
    };

    use crate::schema::pending_inputs;
    let deleted = diesel::delete(
        pending_inputs::table
            .filter(pending_inputs::session_id.eq(current_session_id))
            .filter(pending_inputs::seq_num.le(ack_seq)),
    )
    .execute(&mut conn);

    match deleted {
        Ok(count) => {
            info!(
                "Deleted {} pending inputs for session {} (ack_seq={})",
                count, current_session_id, ack_seq
            );
        }
        Err(e) => {
            error!("Failed to delete pending inputs: {}", e);
        }
    }
}
