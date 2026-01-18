//! Message retention and cleanup logic

use crate::schema::messages;
use chrono::Utc;
use diesel::prelude::*;
use tracing::{error, info};
use uuid::Uuid;

/// Configuration for message retention policy
#[derive(Clone, Copy, Debug)]
pub struct RetentionConfig {
    /// Maximum messages to keep per session
    pub max_messages_per_session: i64,
    /// Days to retain messages (0 = disabled)
    pub retention_days: u32,
}

impl RetentionConfig {
    pub fn new(max_messages_per_session: i64, retention_days: u32) -> Self {
        Self {
            max_messages_per_session,
            retention_days,
        }
    }
}

/// Truncate messages for a single session to the configured maximum
/// Returns the number of deleted messages
pub fn truncate_session_messages(
    conn: &mut diesel::pg::PgConnection,
    session_id: Uuid,
    config: RetentionConfig,
) -> Result<usize, diesel::result::Error> {
    let total_count: i64 = messages::table
        .filter(messages::session_id.eq(session_id))
        .count()
        .get_result(conn)?;

    if total_count <= config.max_messages_per_session {
        return Ok(0);
    }

    let to_delete = total_count - config.max_messages_per_session;

    // Get the IDs of the oldest messages to delete
    let ids_to_delete: Vec<Uuid> = messages::table
        .filter(messages::session_id.eq(session_id))
        .order(messages::created_at.asc())
        .limit(to_delete)
        .select(messages::id)
        .load(conn)?;

    if ids_to_delete.is_empty() {
        return Ok(0);
    }

    let deleted = diesel::delete(messages::table.filter(messages::id.eq_any(&ids_to_delete)))
        .execute(conn)?;

    info!(
        "Truncated session {}: deleted {} old messages, keeping last {}",
        session_id, deleted, config.max_messages_per_session
    );

    Ok(deleted)
}

/// Delete all messages older than the configured retention period
/// Uses a single bulk delete query for efficiency
/// Returns the number of deleted messages
pub fn delete_old_messages(
    conn: &mut diesel::pg::PgConnection,
    config: RetentionConfig,
) -> Result<usize, diesel::result::Error> {
    if config.retention_days == 0 {
        return Ok(0);
    }

    let cutoff = Utc::now().naive_utc() - chrono::Duration::days(config.retention_days as i64);

    let deleted =
        diesel::delete(messages::table.filter(messages::created_at.lt(cutoff))).execute(conn)?;

    if deleted > 0 {
        info!(
            "Retention cleanup: deleted {} messages older than {} days",
            deleted, config.retention_days
        );
    }

    Ok(deleted)
}

/// Run the full retention cleanup process:
/// 1. Delete messages older than retention_days
/// 2. Truncate per-session message counts
pub fn run_retention_cleanup(
    conn: &mut diesel::pg::PgConnection,
    pending_session_ids: Vec<Uuid>,
    config: RetentionConfig,
) -> (usize, usize) {
    let mut age_deleted = 0;
    let mut count_deleted = 0;

    // First, bulk delete old messages
    match delete_old_messages(conn, config) {
        Ok(deleted) => age_deleted = deleted,
        Err(e) => error!("Failed to delete old messages: {:?}", e),
    }

    // Then truncate per-session counts
    for session_id in pending_session_ids {
        match truncate_session_messages(conn, session_id, config) {
            Ok(deleted) => count_deleted += deleted,
            Err(e) => error!("Failed to truncate session {}: {:?}", session_id, e),
        }
    }

    (age_deleted, count_deleted)
}
