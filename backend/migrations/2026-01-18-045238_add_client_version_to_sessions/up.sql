-- Add client_version column to sessions table (nullable for backwards compatibility)
ALTER TABLE sessions ADD COLUMN client_version VARCHAR(32);
