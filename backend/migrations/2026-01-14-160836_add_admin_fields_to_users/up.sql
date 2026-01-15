-- Add admin and disabled fields to users table
ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE users ADD COLUMN disabled BOOLEAN NOT NULL DEFAULT FALSE;

-- Create index for quick admin lookup
CREATE INDEX idx_users_is_admin ON users(is_admin) WHERE is_admin = TRUE;
