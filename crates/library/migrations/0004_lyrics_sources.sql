-- Lyrics from more than one place (LRCLIB stays in `lyrics`) and the source the
-- user picked per track. Caches keyed by catalog ids, no foreign keys.

-- body NULL = looked up, nothing found (retried after a while).
CREATE TABLE lyrics_text (
    track_id   TEXT NOT NULL,
    source     TEXT NOT NULL,                -- online source key
    body       TEXT,                         -- JSON of the parsed lines
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (track_id, source)
) WITHOUT ROWID;

CREATE TABLE lyrics_pick (
    track_id TEXT PRIMARY KEY,
    source   TEXT NOT NULL                   -- lyrics source key
) WITHOUT ROWID;
