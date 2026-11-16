-- Migration 37.
--
-- A master file on its way to the Trash is noted before it moves
-- (`state` 'moving', with where it was and where it goes) and marked done
-- once it has: a file never sits in the Trash without its note, and a
-- move cut short is finished or undone at the next start.
ALTER TABLE master_trash ADD COLUMN state TEXT NOT NULL DEFAULT 'done';

-- What a file hashed to (with its size and change time then); here too
-- for a database that took an early migration 36 without it. Not synced.
CREATE TABLE IF NOT EXISTS file_stamp (
    path       TEXT PRIMARY KEY,
    size_bytes INTEGER NOT NULL,
    mtime_ms   INTEGER NOT NULL,
    file_hash  TEXT NOT NULL,
    at         INTEGER NOT NULL
);
