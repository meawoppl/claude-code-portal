-- Drop performance indices
DROP INDEX IF EXISTS idx_users_email;
DROP INDEX IF EXISTS idx_users_google_id;
DROP INDEX IF EXISTS idx_sessions_user_status;
DROP INDEX IF EXISTS idx_proxy_auth_tokens_user_created;
DROP INDEX IF EXISTS idx_sessions_created_at;
