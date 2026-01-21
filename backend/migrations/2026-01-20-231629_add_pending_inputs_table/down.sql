ALTER TABLE sessions DROP COLUMN input_seq;
DROP INDEX IF EXISTS idx_pending_inputs_session_seq;
DROP INDEX IF EXISTS idx_pending_inputs_session_id;
DROP TABLE IF EXISTS pending_inputs;
