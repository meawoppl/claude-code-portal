-- Pending inputs table for reliable frontend->proxy message delivery
-- Messages are stored here until acknowledged by the proxy
CREATE TABLE pending_inputs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq_num BIGINT NOT NULL,
    content TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),

    -- Each session has its own sequence of inputs
    UNIQUE(session_id, seq_num)
);

-- Index for efficient lookup of pending inputs by session
CREATE INDEX idx_pending_inputs_session_id ON pending_inputs(session_id);

-- Index for ordering by sequence number within a session
CREATE INDEX idx_pending_inputs_session_seq ON pending_inputs(session_id, seq_num);

-- Add input_seq column to sessions to track next sequence number
ALTER TABLE sessions ADD COLUMN input_seq BIGINT NOT NULL DEFAULT 0;
