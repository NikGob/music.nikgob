-- What the user chose of an album, apart from what the album is
-- («Альбом, из которого нравится одна песня»). Only
-- decisions are kept; which songs of an album are the user's is worked out
-- from them and the library (`in_library`). Both tables start empty: an
-- album keeps the songs it has now, nothing is added, removed or
-- downloaded by the migration. Last writer wins on sync, a reset is a
-- tombstone (`deleted = 1`).

-- Per album: `mode` 'whole' = the whole album was added to the library
-- («Добавить весь альбом в библиотеку»: all its tracks are library tracks,
-- the ones that only stream too), 'auto' = the songs chosen one by one;
-- `view` 'mine' / 'all' = what its page shows and «Слушать» plays when only
-- some of it is the user's («Мои треки» / «Весь альбом»), 'auto' = as the
-- setting says.
CREATE TABLE release_selection (
    release_id  TEXT PRIMARY KEY,
    mode        TEXT NOT NULL DEFAULT 'auto',
    view        TEXT NOT NULL DEFAULT 'auto',
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

-- Per song of an album: 'include' = one of the user's songs of this album
-- (a library track), 'exclude' = not one of them here (its file, like and
-- playlists stay; only this album's «Мои треки» leave it out). Keyed by
-- the track, never by its title: two songs of one name are two rows.
CREATE TABLE release_track_selection (
    release_id  TEXT NOT NULL,
    track_id    TEXT NOT NULL,
    choice      TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (release_id, track_id)
) WITHOUT ROWID;
CREATE INDEX release_track_selection_track ON release_track_selection(track_id);
