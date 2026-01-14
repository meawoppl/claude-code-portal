-- Revert cost tracking changes
DROP INDEX IF EXISTS idx_sessions_user_cost;
ALTER TABLE sessions DROP COLUMN IF EXISTS total_cost_usd;
