use crate::process_manager::{ProcessManager, SessionExited};
use futures_util::{SinkExt, StreamExt};
use shared::ProxyMessage;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub async fn run_launcher_loop(
    backend_url: &str,
    launcher_id: Uuid,
    launcher_name: &str,
    auth_token: Option<&str>,
    mut process_manager: ProcessManager,
    mut exit_rx: mpsc::UnboundedReceiver<SessionExited>,
) -> anyhow::Result<()> {
    let mut backoff = Duration::from_secs(1);

    loop {
        let ws_url = format!("{}/ws/launcher", backend_url);
        info!("Connecting to backend: {}", ws_url);

        match connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                info!("Connected to backend");
                backoff = Duration::from_secs(1);

                let (mut write, mut read) = ws_stream.split();

                // Send registration
                let register = ProxyMessage::LauncherRegister {
                    launcher_id,
                    launcher_name: launcher_name.to_string(),
                    auth_token: auth_token.map(|s| s.to_string()),
                    hostname: hostname::get()
                        .map(|h| h.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                };
                let json = serde_json::to_string(&register)?;
                write.send(Message::Text(json)).await?;

                // Wait for RegisterAck
                let ack_ok = loop {
                    match read.next().await {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(ProxyMessage::LauncherRegisterAck {
                                success, error, ..
                            }) = serde_json::from_str(&text)
                            {
                                if success {
                                    info!("Registration successful");
                                    break true;
                                } else {
                                    error!("Registration failed: {}", error.unwrap_or_default());
                                    break false;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break false,
                        _ => continue,
                    }
                };

                if !ack_ok {
                    warn!("Registration failed, will retry");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }

                // Main loop
                let mut heartbeat_timer = tokio::time::interval(HEARTBEAT_INTERVAL);
                let start = Instant::now();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    handle_message(
                                        &text,
                                        &mut write,
                                        &mut process_manager,
                                    ).await;
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    info!("WebSocket closed by server");
                                    break;
                                }
                                Some(Err(e)) => {
                                    error!("WebSocket error: {}", e);
                                    break;
                                }
                                _ => {}
                            }
                        }

                        _ = heartbeat_timer.tick() => {
                            let hb = ProxyMessage::LauncherHeartbeat {
                                launcher_id,
                                running_sessions: process_manager.running_session_ids(),
                                uptime_secs: start.elapsed().as_secs(),
                            };
                            if let Ok(json) = serde_json::to_string(&hb) {
                                if write.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }

                        Some(exited) = exit_rx.recv() => {
                            info!(
                                "Session {} exited with code {:?}",
                                exited.session_id, exited.exit_code
                            );
                            process_manager.remove_finished(&exited.session_id);
                            let msg = ProxyMessage::SessionExited {
                                session_id: exited.session_id,
                                exit_code: exited.exit_code,
                            };
                            if let Ok(json) = serde_json::to_string(&msg) {
                                if write.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect: {}", e);
            }
        }

        info!("Reconnecting in {:?}...", backoff);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

type WsWrite = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn handle_message(text: &str, write: &mut WsWrite, process_manager: &mut ProcessManager) {
    let msg: ProxyMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => return,
    };

    match msg {
        ProxyMessage::LaunchSession {
            request_id,
            auth_token,
            working_directory,
            session_name,
            claude_args,
            ..
        } => {
            info!(
                "Launch request: dir={}, name={:?}",
                working_directory, session_name
            );

            let result = process_manager
                .spawn(
                    &auth_token,
                    &working_directory,
                    session_name.as_deref(),
                    &claude_args,
                )
                .await;

            let response = match result {
                Ok(spawn_result) => ProxyMessage::LaunchSessionResult {
                    request_id,
                    success: true,
                    session_id: Some(spawn_result.session_id),
                    pid: None,
                    error: None,
                },
                Err(e) => {
                    error!("Failed to spawn: {}", e);
                    ProxyMessage::LaunchSessionResult {
                        request_id,
                        success: false,
                        session_id: None,
                        pid: None,
                        error: Some(e.to_string()),
                    }
                }
            };

            if let Ok(json) = serde_json::to_string(&response) {
                let _ = write.send(Message::Text(json)).await;
            }
        }
        ProxyMessage::StopSession { session_id } => {
            info!("Stop request for session {}", session_id);
            process_manager.stop(&session_id).await;
        }
        ProxyMessage::ServerShutdown { reason, .. } => {
            info!("Server shutting down: {}", reason);
        }
        _ => {}
    }
}
