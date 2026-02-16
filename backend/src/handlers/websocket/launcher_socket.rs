use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::LauncherConnection;
use crate::AppState;

pub async fn handle_launcher_socket(socket: WebSocket, app_state: Arc<AppState>) {
    let (mut ws_write, mut ws_read) = socket.split();

    // Wait for LauncherRegister message
    let (launcher_id, launcher_name, hostname, user_id) = loop {
        match ws_read.next().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(ProxyMessage::LauncherRegister {
                    launcher_id,
                    launcher_name,
                    auth_token,
                    hostname,
                    ..
                }) = serde_json::from_str(&text)
                {
                    // Authenticate
                    let user_id = if let Some(ref token) = auth_token {
                        match app_state.db_pool.get() {
                            Ok(mut conn) => {
                                match crate::handlers::proxy_tokens::verify_and_get_user(
                                    &app_state, &mut conn, token,
                                ) {
                                    Ok((uid, email)) => {
                                        info!("Launcher authenticated as {} ({})", email, uid);
                                        uid
                                    }
                                    Err(_) => {
                                        if app_state.dev_mode {
                                            get_dev_user_id(&app_state)
                                        } else {
                                            send_register_ack(
                                                &mut ws_write,
                                                launcher_id,
                                                false,
                                                Some("Authentication failed"),
                                            )
                                            .await;
                                            return;
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                send_register_ack(
                                    &mut ws_write,
                                    launcher_id,
                                    false,
                                    Some("Database error"),
                                )
                                .await;
                                return;
                            }
                        }
                    } else if app_state.dev_mode {
                        get_dev_user_id(&app_state)
                    } else {
                        send_register_ack(
                            &mut ws_write,
                            launcher_id,
                            false,
                            Some("No auth token provided"),
                        )
                        .await;
                        return;
                    };

                    break (launcher_id, launcher_name, hostname, user_id);
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            _ => continue,
        }
    };

    // Send RegisterAck
    send_register_ack(&mut ws_write, launcher_id, true, None).await;

    // Create channel for sending messages to this launcher
    let (tx, mut rx) = mpsc::unbounded_channel::<ProxyMessage>();

    app_state.session_manager.register_launcher(
        launcher_id,
        LauncherConnection {
            sender: tx,
            launcher_name: launcher_name.clone(),
            hostname,
            user_id,
            running_sessions: Vec::new(),
        },
    );

    info!(
        "Launcher '{}' registered for user {}",
        launcher_name, user_id
    );

    // Main message loop
    loop {
        tokio::select! {
            // Messages from the launcher
            msg = ws_read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_launcher_message(
                            &text,
                            launcher_id,
                            user_id,
                            &app_state,
                        );
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("Launcher '{}' disconnected", launcher_name);
                        break;
                    }
                    Some(Err(e)) => {
                        error!("Launcher WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // Messages to forward to the launcher
            Some(msg) = rx.recv() => {
                if let Ok(json) = serde_json::to_string(&msg) {
                    if ws_write.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    app_state.session_manager.unregister_launcher(&launcher_id);
}

fn handle_launcher_message(text: &str, launcher_id: Uuid, user_id: Uuid, app_state: &AppState) {
    let msg: ProxyMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => return,
    };

    match msg {
        ProxyMessage::LaunchSessionResult {
            request_id,
            success,
            session_id,
            pid,
            ref error,
        } => {
            if success {
                info!(
                    "Launch succeeded: request={}, session={:?}, pid={:?}",
                    request_id, session_id, pid
                );
            } else {
                warn!("Launch failed: request={}, error={:?}", request_id, error);
            }
            // Broadcast result to the user's web clients
            app_state.session_manager.broadcast_to_user(&user_id, msg);
        }
        ProxyMessage::LauncherHeartbeat {
            running_sessions, ..
        } => {
            // Update the launcher's running sessions
            if let Some(mut launcher) = app_state.session_manager.launchers.get_mut(&launcher_id) {
                launcher.running_sessions = running_sessions;
            }
        }
        ProxyMessage::ProxyLog {
            session_id,
            level,
            ref message,
            ..
        } => match level.as_str() {
            "error" => tracing::error!(session_id = %session_id, "[proxy] {}", message),
            "warn" => tracing::warn!(session_id = %session_id, "[proxy] {}", message),
            "debug" => tracing::debug!(session_id = %session_id, "[proxy] {}", message),
            _ => tracing::info!(session_id = %session_id, "[proxy] {}", message),
        },
        ProxyMessage::SessionExited {
            session_id,
            exit_code,
        } => {
            info!("Proxy exited: session={}, code={:?}", session_id, exit_code);
            app_state.session_manager.broadcast_to_user(&user_id, msg);
        }
        ProxyMessage::ListDirectoriesResult { request_id, .. } => {
            app_state
                .session_manager
                .complete_dir_request(request_id, msg);
        }
        _ => {}
    }
}

type WsSink = futures_util::stream::SplitSink<WebSocket, Message>;

async fn send_register_ack(
    ws_write: &mut WsSink,
    launcher_id: Uuid,
    success: bool,
    error: Option<&str>,
) {
    let ack = ProxyMessage::LauncherRegisterAck {
        success,
        launcher_id,
        error: error.map(|s| s.to_string()),
    };
    if let Ok(json) = serde_json::to_string(&ack) {
        let _ = ws_write.send(Message::Text(json)).await;
    }
}

fn get_dev_user_id(app_state: &AppState) -> Uuid {
    use crate::schema::users;
    use diesel::prelude::*;

    let mut conn = app_state.db_pool.get().expect("DB connection for dev mode");
    let user: crate::models::User = users::table
        .filter(users::email.eq("testing@testing.local"))
        .first(&mut conn)
        .expect("Test user must exist in dev mode");
    user.id
}
