-- Set any NULL working_directory to empty string, then make NOT NULL
UPDATE sessions SET working_directory = '' WHERE working_directory IS NULL;
ALTER TABLE sessions ALTER COLUMN working_directory SET NOT NULL;
ALTER TABLE sessions ALTER COLUMN working_directory SET DEFAULT '';
