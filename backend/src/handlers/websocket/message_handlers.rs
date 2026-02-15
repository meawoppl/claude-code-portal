use super::{ClientSender, SessionManager};
use crate::db::DbPool;
use diesel::prelude::*;
use shared::ProxyMessage;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Replay pending inputs from the database to a reconnected proxy.
/// Returns the number of inputs replayed.
pub fn replay_pending_inputs_from_db(
    db_pool: &DbPool,
    session_id: Uuid,
    sender: &ClientSender,
) -> usize {
    use crate::schema::pending_inputs;

    let mut conn = match db_pool.get() {
        Ok(conn) => conn,
        Err(e) => {
            error!(
                "Failed to get DB connection for pending inputs replay: {}",
                e
            );
            return 0;
        }
    };

    let pending: Vec<crate::models::PendingInput> = match pending_inputs::table
        .filter(pending_inputs::session_id.eq(session_id))
        .order(pending_inputs::seq_num.asc())
        .load(&mut conn)
    {
        Ok(inputs) => inputs,
        Err(e) => {
            error!(
                "Failed to load pending inputs for session {}: {}",
                session_id, e
            );
            return 0;
        }
    };

    let mut replayed = 0;
    for input in pending {
        let content: serde_json::Value = match serde_json::from_str(&input.content) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to parse pending input content: {}", e);
                continue;
            }
        };

        let msg = ProxyMessage::SequencedInput {
            session_id,
            seq: input.seq_num,
            content,
            send_mode: None,
        };

        if sender.send(msg).is_ok() {
            replayed += 1;
        } else {
            warn!("Failed to send pending input to proxy, channel closed");
            break;
        }
    }

    if replayed > 0 {
        info!(
            "Replayed {} pending inputs to reconnected proxy for session {}",
            replayed, session_id
        );
    }

    replayed
}

/// Handle Claude output (both legacy ClaudeOutput and new SequencedOutput).
/// Broadcasts to web clients, deduplicates sequenced messages, stores in DB,
/// and sends acknowledgments.
pub fn handle_claude_output(
    session_manager: &SessionManager,
    session_key: &Option<String>,
    db_session_id: Option<Uuid>,
    db_pool: &DbPool,
    tx: &ClientSender,
    content: serde_json::Value,
    seq: Option<u64>,
) {
    // Broadcast output to all web clients
    if let Some(ref key) = session_key {
        session_manager.broadcast_to_web_clients(
            key,
            ProxyMessage::ClaudeOutput {
                content: content.clone(),
            },
        );
    }

    // Deduplicate sequenced messages
    if let (Some(session_id), Some(seq_num)) = (db_session_id, seq) {
        let last_ack = session_manager
            .last_ack_seq
            .get(&session_id)
            .map(|v| *v)
            .unwrap_or(0);

        if seq_num <= last_ack {
            info!(
                "Skipping duplicate message seq={} (last_ack={})",
                seq_num, last_ack
            );
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

        if let Ok(session) = sessions::table
            .find(session_id)
            .first::<crate::models::Session>(&mut conn)
        {
            let role = shared::MessageRole::from_type_str(
                content
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("assistant"),
            );

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

            if role == shared::MessageRole::Result {
                store_result_metadata(&mut conn, session_id, &content);
            }

            // Inject portal messages for images in tool results
            if role == shared::MessageRole::User {
                let portal_messages = extract_image_portal_messages(&content);
                for portal_msg in portal_messages {
                    let portal_json = portal_msg.to_json();

                    // Store portal message in DB
                    let portal_db_msg = crate::models::NewMessage {
                        session_id,
                        role: shared::MessageRole::Portal.to_string(),
                        content: portal_json.to_string(),
                        user_id: session.user_id,
                    };
                    if let Err(e) = diesel::insert_into(messages::table)
                        .values(&portal_db_msg)
                        .execute(&mut conn)
                    {
                        error!("Failed to store portal image message: {}", e);
                    }

                    // Broadcast to web clients
                    if let Some(ref key) = session_key {
                        session_manager.broadcast_to_web_clients(
                            key,
                            ProxyMessage::ClaudeOutput {
                                content: portal_json,
                            },
                        );
                    }
                }
            }

            session_manager.queue_truncation(session_id);
        }

        // Update last_activity
        let _ = diesel::update(sessions::table.find(session_id))
            .set(sessions::last_activity.eq(diesel::dsl::now))
            .execute(&mut conn);

        // Update last_ack tracker and send acknowledgment
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

            let _ = tx.send(ProxyMessage::OutputAck {
                session_id,
                ack_seq: seq_num,
            });
        }
    }
}

/// Extract and store cost and token usage from result messages
fn store_result_metadata(
    conn: &mut diesel::PgConnection,
    session_id: Uuid,
    content: &serde_json::Value,
) {
    use crate::schema::sessions;

    let cost = content.get("total_cost_usd").and_then(|c| c.as_f64());
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

    if let Some(cost_val) = cost {
        if let Err(e) = diesel::update(sessions::table.find(session_id))
            .set(sessions::total_cost_usd.eq(cost_val))
            .execute(conn)
        {
            error!("Failed to update session cost: {}", e);
        }
    }

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
            .execute(conn)
        {
            error!("Failed to update session tokens: {}", e);
        }
    }
}

const ALLOWED_IMAGE_MEDIA_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
];

/// 2 MB limit on base64 image data we'll inject as portal messages.
const MAX_IMAGE_BASE64_BYTES: usize = 2 * 1024 * 1024;

/// Scan a "user" message's tool result blocks for images and return
/// a `PortalMessage` for each one found.
fn extract_image_portal_messages(content: &serde_json::Value) -> Vec<shared::PortalMessage> {
    let mut portal_messages = Vec::new();

    let blocks = content
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());

    let Some(blocks) = blocks else {
        return portal_messages;
    };

    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
            continue;
        }

        // Handle Structured tool result content
        let structured_blocks = block
            .get("content")
            .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("Structured"))
            .and_then(|c| c.get("value"))
            .and_then(|v| v.as_array());

        let Some(structured_blocks) = structured_blocks else {
            continue;
        };

        for item in structured_blocks {
            if item.get("type").and_then(|t| t.as_str()) != Some("image") {
                continue;
            }

            let source = match item.get("source") {
                Some(s) => s,
                None => continue,
            };

            let media_type = source
                .get("media_type")
                .and_then(|m| m.as_str())
                .unwrap_or("image/png");

            if !ALLOWED_IMAGE_MEDIA_TYPES.contains(&media_type) {
                continue;
            }

            let data = match source.get("data").and_then(|d| d.as_str()) {
                Some(d) => d,
                None => continue,
            };

            if data.len() > MAX_IMAGE_BASE64_BYTES {
                let size_mb = data.len() as f64 / (1024.0 * 1024.0);
                let limit_mb = MAX_IMAGE_BASE64_BYTES as f64 / (1024.0 * 1024.0);
                portal_messages.push(shared::PortalMessage::text(format!(
                    "Image too large to display: **{:.1} MB** (limit is {:.0} MB)",
                    size_mb, limit_mb
                )));
                continue;
            }

            portal_messages.push(shared::PortalMessage::image(
                media_type.to_string(),
                data.to_string(),
            ));
        }
    }

    portal_messages
}
