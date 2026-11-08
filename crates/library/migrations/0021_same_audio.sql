-- A file that holds another song's audio (docs/TODO.md 32).

-- The Chromaprint of each local file (its first 120 s), so an import
-- compares a new file with the album's songs without decoding them again,
-- and the background check knows which files it has heard. Keyed by source
-- id without a foreign key: a print of a source that is gone is an orphan,
-- dropped by `drop_orphan_prints`.
CREATE TABLE audio_print (
    source_id   TEXT PRIMARY KEY,
    track_id    TEXT NOT NULL,
    print       BLOB,                   -- raw items, u32 LE; NULL: the file could not be read
    duration_ms INTEGER,                -- the whole file
    came_hash   TEXT,                   -- BLAKE3 of the file as it came (before tags or conversion)
    came_from   TEXT,                   -- where it came from, one per line: torrent:<info hash>, soulseek:<user\path>, ...
    failed      TEXT,
    at          INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX audio_print_track ON audio_print(track_id);

-- Files that held another song's audio, per track: their hash
-- (`hash:<blake3>`) and where they came from (`from:<key>`) are never taken
-- for that track again; the print (on one row of each file) catches the
-- same audio from anywhere else.
CREATE TABLE wrong_audio (
    track_id    TEXT NOT NULL,
    key         TEXT NOT NULL,
    sounds_like TEXT,                   -- the track whose song it holds, when known
    print       BLOB,
    at          INTEGER NOT NULL,
    PRIMARY KEY (track_id, key)
) WITHOUT ROWID;

-- Two files that sound the same under different titles, found by the
-- background check (or by an import of both at once): offered in
-- «Подтверди» until the user answers. Keyed by the files, so a file that is
-- replaced is a new question; `kept` = the user said both are right.
CREATE TABLE same_audio (
    source_id    TEXT NOT NULL,
    other_source TEXT NOT NULL,
    track_id     TEXT NOT NULL,
    other_track  TEXT NOT NULL,
    similarity   REAL NOT NULL,
    kept         INTEGER NOT NULL DEFAULT 0,
    at           INTEGER NOT NULL,
    PRIMARY KEY (source_id, other_source)
) WITHOUT ROWID;
