-- Track costs from deleted sessions
-- This table accumulates costs per user when sessions are deleted
CREATE TABLE deleted_session_costs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    session_count INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One row per user to track aggregate deleted costs
CREATE UNIQUE INDEX deleted_session_costs_user_id_idx ON deleted_session_costs(user_id);
