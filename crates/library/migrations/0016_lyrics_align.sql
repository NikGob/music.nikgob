-- What the player made of a lyrics text against the track's own recording
-- (docs/lyrics-align.md): how well it fits, how its line times sit, and the
-- word timing made from it. A cache keyed by catalog ids, no foreign keys:
-- dropping it only means the work is done again.

-- One row a text: `text_hash` is the text it was made from (a changed text
-- is checked again). `body` NULL = nothing to show from it (it does not fit).
CREATE TABLE lyrics_align (
    track_id  TEXT NOT NULL,
    source    TEXT NOT NULL,                -- LyricsSource key
    text_hash TEXT NOT NULL,
    fit       REAL NOT NULL,                -- how well the recording spells it, 0..1
    timing    TEXT NOT NULL,                -- none | fits | shifted | elsewhere
    offset_ms INTEGER NOT NULL DEFAULT 0,   -- source line time minus the voice
    covered   REAL NOT NULL DEFAULT 1,      -- share of the sung time the text covers
    body      TEXT,                         -- JSON lines with the made word timing
    made_by   TEXT NOT NULL,                -- aligner and version
    made_at   INTEGER NOT NULL,
    saved     INTEGER NOT NULL DEFAULT 0,   -- 1: made and kept by the user
    PRIMARY KEY (track_id, source)
) WITHOUT ROWID;

-- Tracks the background pass has been through (state done | no_text |
-- failed); a failure is tried again after a while.
CREATE TABLE lyrics_align_done (
    track_id TEXT PRIMARY KEY,
    state    TEXT NOT NULL,
    detail   TEXT,
    made_by  TEXT NOT NULL,
    at       INTEGER NOT NULL
) WITHOUT ROWID;
