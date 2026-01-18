-- Remove deduplication index and column
DROP INDEX IF EXISTS idx_raw_message_log_dedup;
ALTER TABLE raw_message_log DROP COLUMN content_hash;
