-- The user's answer about a file the spectrum check found made from a lossy
-- one: 1 = keep it as it is, never offered as an MP3 again.
ALTER TABLE source_quality ADD COLUMN keep INTEGER NOT NULL DEFAULT 0;
