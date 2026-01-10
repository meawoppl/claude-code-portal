-- Remove proxy authentication tokens table
DROP INDEX IF EXISTS idx_proxy_auth_tokens_token_hash;
DROP INDEX IF EXISTS idx_proxy_auth_tokens_user_id;
DROP TABLE IF EXISTS proxy_auth_tokens;
