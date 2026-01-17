-- Performance indices for common query patterns

-- Critical: Email lookups (every dev mode request does this)
CREATE INDEX idx_users_email ON users(email);

-- Critical: Google ID lookups (every OAuth login)
-- Note: google_id has UNIQUE constraint which creates implicit index,
-- but being explicit ensures optimizer uses it
CREATE INDEX idx_users_google_id ON users(google_id);

-- High: Combined user_id + status filter (admin dashboard, session listings)
CREATE INDEX idx_sessions_user_status ON sessions(user_id, status);

-- Medium: Token listing by user with creation order
CREATE INDEX idx_proxy_auth_tokens_user_created ON proxy_auth_tokens(user_id, created_at DESC);

-- Low: Session creation time for time-range queries
CREATE INDEX idx_sessions_created_at ON sessions(created_at DESC);
