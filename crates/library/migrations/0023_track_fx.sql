-- How a track is played when it is not played as recorded: «Скорость» of
-- its menu (faster, nightcore, slowed, slowed with an echo). One row per
-- track that has a choice; no row means as recorded. The choice belongs to
-- the track whatever it plays from (a file or a stream), so it is not in
-- `source`, and it is nothing about the song itself, so it is not in
-- `track` nor in the file's tags.
CREATE TABLE track_fx (
    track_id   TEXT PRIMARY KEY,
    variant    TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;
