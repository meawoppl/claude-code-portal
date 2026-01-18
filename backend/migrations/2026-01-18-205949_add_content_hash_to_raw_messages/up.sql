-- Add content_hash column for deduplication
ALTER TABLE raw_message_log ADD COLUMN content_hash VARCHAR(64);

-- Populate existing rows with hash of message_content
UPDATE raw_message_log SET content_hash = md5(message_content::text);

-- Make it NOT NULL after populating
ALTER TABLE raw_message_log ALTER COLUMN content_hash SET NOT NULL;

-- Add unique constraint on session_id + content_hash to prevent duplicates
CREATE UNIQUE INDEX idx_raw_message_log_dedup
ON raw_message_log (session_id, content_hash)
WHERE session_id IS NOT NULL;
