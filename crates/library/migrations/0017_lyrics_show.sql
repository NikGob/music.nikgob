-- How the user chose to see a track's lyrics under the lyrics control:
-- `word`, `line` or `plain`, as far as the shown
-- text has that timing. A display choice keyed by catalog ids, no foreign
-- keys: dropping it only means the automatic choice again.
CREATE TABLE lyrics_show (
    track_id TEXT PRIMARY KEY,
    sync     TEXT NOT NULL                   -- word | line | plain
) WITHOUT ROWID;
