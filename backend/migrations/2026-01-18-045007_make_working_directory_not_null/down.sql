-- Revert working_directory to nullable
ALTER TABLE sessions ALTER COLUMN working_directory DROP DEFAULT;
ALTER TABLE sessions ALTER COLUMN working_directory DROP NOT NULL;
