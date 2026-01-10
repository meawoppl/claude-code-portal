-- Add user_id column to messages for direct indexing
-- This denormalizes the data but allows efficient queries by user
ALTER TABLE messages ADD COLUMN user_id UUID REFERENCES users(id) ON DELETE CASCADE;

-- Backfill user_id from sessions table
UPDATE messages m
SET user_id = s.user_id
FROM sessions s
WHERE m.session_id = s.id;

-- Make user_id NOT NULL after backfill
ALTER TABLE messages ALTER COLUMN user_id SET NOT NULL;

-- Add index on user_id for efficient user-based queries
CREATE INDEX idx_messages_user_id ON messages(user_id);

-- Add composite index for efficient truncation queries (get oldest messages per session)
CREATE INDEX idx_messages_session_created ON messages(session_id, created_at DESC);

-- Add composite index for user + time queries
CREATE INDEX idx_messages_user_created ON messages(user_id, created_at DESC);
