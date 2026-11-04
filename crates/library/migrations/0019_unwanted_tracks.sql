-- Songs the user deleted from an album: «не нужны». A row outlives the
-- track (the Trash empties after 30 days), so a download of the album, the
-- album's tracklist from a service and the recommendations do not bring
-- the song back. Keyed by the album and the song's title (`title_key` as in
-- `track`); the place, length and ids tell the song among the files of a
-- download. Restoring the track, or liking the song, takes its row away.
CREATE TABLE unwanted (
    release_id     TEXT NOT NULL,                -- '' for a song without an album
    title_key      TEXT NOT NULL,
    title          TEXT NOT NULL,
    artist         TEXT NOT NULL DEFAULT '',     -- the credit as displayed
    disc_no        INTEGER NOT NULL DEFAULT 1,
    track_no       INTEGER NOT NULL DEFAULT 0,
    duration_ms    INTEGER,
    isrc           TEXT,
    recording_mbid TEXT,
    track_id       TEXT,                         -- the deleted track, until it is purged
    at             INTEGER NOT NULL,
    PRIMARY KEY (release_id, title_key)
) WITHOUT ROWID;
CREATE INDEX unwanted_track ON unwanted(track_id) WHERE track_id IS NOT NULL;

-- Tracks deleted before this version are such songs too.
INSERT OR IGNORE INTO unwanted(release_id, title_key, title, artist, disc_no, track_no,
                               duration_ms, isrc, recording_mbid, track_id, at)
SELECT coalesce(release_id, ''), title_key, title, artist_display, max(disc_no, 1), track_no,
       duration_ms, isrc, recording_mbid, id, deleted_at
  FROM track
 WHERE deleted_at IS NOT NULL
 ORDER BY deleted_at DESC;
