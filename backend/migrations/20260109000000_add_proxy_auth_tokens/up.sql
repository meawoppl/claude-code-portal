-- Proxy authentication tokens for CLI access
-- These are JWT-backed tokens that allow the proxy CLI to authenticate
-- without going through the device flow each time.

CREATE TABLE proxy_auth_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Human-readable name for the token (e.g., "My laptop", "CI runner")
    name VARCHAR(255) NOT NULL,
    -- SHA256 hash of the JWT for quick lookup during revocation checks
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMP,
    expires_at TIMESTAMP NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE
);

-- Index for looking up tokens by user
CREATE INDEX idx_proxy_auth_tokens_user_id ON proxy_auth_tokens(user_id);

-- Index for looking up by token hash (used during JWT verification)
CREATE INDEX idx_proxy_auth_tokens_token_hash ON proxy_auth_tokens(token_hash);
