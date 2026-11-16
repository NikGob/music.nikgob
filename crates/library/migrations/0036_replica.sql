-- Migration 36.
--
-- The lossless originals this computer let go of once other devices hold
-- verified copies of them («Оригиналы FLAC на ПК»): each file moved to the
-- Trash is noted with where it was, so «Отменить» puts it back and the
-- Trash's 30 days end it. The master rows themselves stay: the file still
-- exists, on the devices that hold its copies.
CREATE TABLE IF NOT EXISTS master_trash (
    id         INTEGER PRIMARY KEY,
    file_hash  TEXT NOT NULL,
    path       TEXT NOT NULL,
    trash_path TEXT NOT NULL UNIQUE,
    size_bytes INTEGER NOT NULL,
    trashed_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS master_trash_at ON master_trash(trashed_at);
