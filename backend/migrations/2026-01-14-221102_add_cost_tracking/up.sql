-- Add cost tracking to sessions table
-- Stores the cumulative total cost from Claude for each session
ALTER TABLE sessions ADD COLUMN total_cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0;

-- Create index for efficient user spend aggregation queries
CREATE INDEX idx_sessions_user_cost ON sessions(user_id, total_cost_usd);
