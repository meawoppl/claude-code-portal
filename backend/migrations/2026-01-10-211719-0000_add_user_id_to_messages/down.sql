-- Remove indexes
DROP INDEX IF EXISTS idx_messages_user_created;
DROP INDEX IF EXISTS idx_messages_session_created;
DROP INDEX IF EXISTS idx_messages_user_id;

-- Remove user_id column
ALTER TABLE messages DROP COLUMN user_id;
