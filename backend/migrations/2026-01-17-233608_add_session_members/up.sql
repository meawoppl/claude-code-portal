-- Session members table for sharing sessions between users
CREATE TABLE session_members (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role VARCHAR(20) NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    UNIQUE(session_id, user_id)
);

-- Indices for efficient lookups
CREATE INDEX idx_session_members_user ON session_members(user_id);
CREATE INDEX idx_session_members_session ON session_members(session_id);

-- Migrate existing sessions: current owner becomes owner in session_members
INSERT INTO session_members (session_id, user_id, role)
SELECT id, user_id, 'owner' FROM sessions;
