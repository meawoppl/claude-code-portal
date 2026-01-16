-- Create table for pending permission requests
-- These are stored when a proxy sends a PermissionRequest and cleared when response is received
CREATE TABLE pending_permission_requests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    request_id VARCHAR(255) NOT NULL,
    tool_name VARCHAR(255) NOT NULL,
    input JSONB NOT NULL,
    permission_suggestions JSONB,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),

    -- Only one pending permission per session at a time
    UNIQUE(session_id)
);

-- Index for quick lookup by session
CREATE INDEX idx_pending_permission_requests_session_id ON pending_permission_requests(session_id);
