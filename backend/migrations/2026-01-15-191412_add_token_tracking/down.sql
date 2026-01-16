-- Remove token tracking columns from sessions table
ALTER TABLE sessions
    DROP COLUMN input_tokens,
    DROP COLUMN output_tokens,
    DROP COLUMN cache_creation_tokens,
    DROP COLUMN cache_read_tokens;

-- Remove token tracking columns from deleted_session_costs table
ALTER TABLE deleted_session_costs
    DROP COLUMN input_tokens,
    DROP COLUMN output_tokens,
    DROP COLUMN cache_creation_tokens,
    DROP COLUMN cache_read_tokens;
