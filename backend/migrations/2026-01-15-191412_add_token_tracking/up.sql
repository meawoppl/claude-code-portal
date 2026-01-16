-- Add token tracking columns to sessions table
ALTER TABLE sessions
    ADD COLUMN input_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN output_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN cache_creation_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN cache_read_tokens BIGINT NOT NULL DEFAULT 0;

-- Add token tracking columns to deleted_session_costs table
ALTER TABLE deleted_session_costs
    ADD COLUMN input_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN output_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN cache_creation_tokens BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN cache_read_tokens BIGINT NOT NULL DEFAULT 0;
