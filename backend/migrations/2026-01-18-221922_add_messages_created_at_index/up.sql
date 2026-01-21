-- Add index on messages.created_at for efficient retention queries
-- This supports queries that delete messages older than a certain date
-- Note: Using IF NOT EXISTS because this index may already exist in initial_setup
CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages (created_at);
