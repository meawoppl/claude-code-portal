-- Add index on messages.created_at for efficient retention queries
-- This supports queries that delete messages older than a certain date
CREATE INDEX idx_messages_created_at ON messages (created_at);
