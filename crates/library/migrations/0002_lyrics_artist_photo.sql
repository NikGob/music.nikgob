-- Lyrics fetched online (LRCLIB) and artist photos (Deezer).
--
-- Both tables are caches keyed by catalog ids and carry NO foreign keys: they can be
-- dropped and refetched at any time, and rows of removed tracks/artists are harmless.

-- A row with synced AND plain NULL (and instrumental = 0) is the "looked up, nothing
-- found" marker; it is retried after a while (fetched_at).
CREATE TABLE lyrics (
    track_id     TEXT PRIMARY KEY,
    synced       TEXT,                       -- raw LRC text
    plain        TEXT,
    instrumental INTEGER NOT NULL DEFAULT 0,
    source       TEXT NOT NULL DEFAULT 'lrclib',
    fetched_at   INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX lyrics_fetched ON lyrics(fetched_at);

-- path NULL = looked up, no photo found (retried after a while).
CREATE TABLE artist_photo (
    artist_id  TEXT PRIMARY KEY,
    path       TEXT,                         -- relative to the library root, or absolute
    source     TEXT NOT NULL DEFAULT 'deezer',
    fetched_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX artist_photo_fetched ON artist_photo(fetched_at);
