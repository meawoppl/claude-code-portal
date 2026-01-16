-- Add voice_enabled field to users table
-- Default to false so voice is disabled by default for all users
ALTER TABLE users ADD COLUMN voice_enabled BOOLEAN NOT NULL DEFAULT FALSE;
