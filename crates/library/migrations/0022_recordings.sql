-- One song on two albums (an OST and its special edition): both tracks
-- stay whole, each on its album with its own number, tags and file, and
-- become one recording. Rows only: no file is moved or deleted.

-- The tracks of one recording share `recording` (the id of the track the
-- group began with). `how`: `ids` (the same ISRC, recording MBID or
-- AcoustID), `audio` (near bit-exact audio under one title), `user` (the
-- user said so on «Подтверди»).
CREATE TABLE recording_link (
    track_id  TEXT PRIMARY KEY,
    recording TEXT NOT NULL,
    how       TEXT NOT NULL,
    at        INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX recording_link_group ON recording_link(recording);

-- Two tracks the user said are different songs («Разные песни»): never
-- linked again, whatever finds them. Stored with the smaller id first.
CREATE TABLE recording_apart (
    track_id    TEXT NOT NULL,
    other_track TEXT NOT NULL,
    at          INTEGER NOT NULL,
    PRIMARY KEY (track_id, other_track)
) WITHOUT ROWID;

-- The copy of each recording that stands for it in «Любимые», the search,
-- the history and the mixes: the one with a file, in the library, of the
-- better quality, on the album played more, the oldest. Every live linked
-- track with the copy it folds into (itself for that copy).
CREATE VIEW recording_canon AS
SELECT track_id, recording,
       first_value(track_id) OVER (
           PARTITION BY recording
           ORDER BY has_local DESC, in_library DESC, quality DESC, album_plays DESC, seq
       ) AS canon
  FROM (SELECT l.track_id, l.recording, t.has_local, t.in_library,
               coalesce(t.active_quality, 0) AS quality, t.seq,
               CASE WHEN t.release_id IS NULL
                    THEN (SELECT count(*) FROM event e WHERE e.track_id = t.id AND e.type = 'start')
                    ELSE (SELECT count(*) FROM event e JOIN track t2 ON t2.id = e.track_id
                           WHERE t2.release_id = t.release_id AND e.type = 'start')
               END AS album_plays
          FROM recording_link l JOIN track t ON t.id = l.track_id
         WHERE t.deleted_at IS NULL);

-- The other copies: a row each, never shown twice where one row stands
-- for the song.
CREATE VIEW recording_spare AS
SELECT track_id, canon FROM recording_canon WHERE track_id != canon;

-- Which stored prints were compared for a recording on another album
-- (NULL: not yet; a new print of the file is compared again).
ALTER TABLE audio_print ADD COLUMN linked_at INTEGER;
