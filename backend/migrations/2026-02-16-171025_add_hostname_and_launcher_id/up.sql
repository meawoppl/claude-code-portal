ALTER TABLE sessions ADD COLUMN hostname VARCHAR(255) NOT NULL DEFAULT 'unknown';
ALTER TABLE sessions ADD COLUMN launcher_id UUID;

-- Backfill hostname from session_name (format: "hostname-YYYYMMDD-HHMMSS")
-- Strip the trailing -YYYYMMDD-HHMMSS pattern to extract the hostname
UPDATE sessions
SET hostname = regexp_replace(session_name, '-\d{8}-\d{6}$', '')
WHERE session_name ~ '-\d{8}-\d{6}$';
