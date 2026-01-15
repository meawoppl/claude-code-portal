-- Remove admin fields from users table
DROP INDEX IF EXISTS idx_users_is_admin;
ALTER TABLE users DROP COLUMN IF EXISTS disabled;
ALTER TABLE users DROP COLUMN IF EXISTS is_admin;
