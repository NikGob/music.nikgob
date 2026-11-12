-- Migration 28 (r35-albums; renumber on merge if another 0028 lands first).
--
-- «Добавить в библиотеку» of an album (docs/TODO.md 60): an album added to
-- the library is the whole album (`release_selection.mode = 'whole'`, as
-- since 0025); one not added is the user's songs of it. Only data is added
-- here: no track, source, like, playlist or file changes, and `in_library`
-- of no track changes (every row counted below already is a library track).
--
-- 1. Per album: whether its other versions (instrumentals, off vocal,
--    karaoke, remixes the user did not choose) play with it: 'on', 'off',
--    or 'auto' (the setting «Другие версии»).
ALTER TABLE release_selection ADD COLUMN versions TEXT NOT NULL DEFAULT 'auto';

-- 2. A song put into the library by hand («В библиотеку» of a stream track:
--    no like, nothing downloads); a reset is a tombstone (`deleted = 1`),
--    last writer wins on sync as with likes.
CREATE TABLE track_put (
    track_id    TEXT PRIMARY KEY,
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;

-- 3. The albums the library already has whole stay whole: every line of
--    its page (two or more; a twin stream row at the place of a file is no
--    line, as `page_row!` says) is a library track not left out by hand,
--    and its track count asks for no more. Their tracklist found later then
--    joins them, as it does for an album added by hand (a saved album, its
--    own like, is whole anyway). An album of one song, one with a song left
--    out, one short of its count stays the user's songs. A choice of the
--    user wins: only a missing row, a reset or an 'auto' one is set. The time is the earliest possible, so
--    any choice made on another device wins over it on sync.
INSERT INTO release_selection(release_id, mode, view, updated_at, updated_by, deleted, versions)
SELECT r.id, 'whole', 'auto', 1, coalesce((SELECT value FROM meta WHERE key = 'device_id'), ''), 0, 'auto'
  FROM release r
  JOIN (SELECT t.release_id AS rid, COUNT(*) AS n,
               SUM(t.in_library = 1 AND NOT EXISTS (
                   SELECT 1 FROM release_track_selection x WHERE x.release_id = t.release_id
                      AND x.track_id = t.id AND x.choice = 'exclude' AND x.deleted = 0)) AS mine
          FROM track t
         WHERE t.release_id IS NOT NULL AND t.deleted_at IS NULL
           AND NOT (t.has_local = 0 AND t.in_library = 0 AND t.track_no > 0 AND EXISTS (
               SELECT 1 FROM track o WHERE o.release_id = t.release_id AND o.id != t.id
                  AND o.deleted_at IS NULL AND o.has_local = 1
                  AND o.disc_no = t.disc_no AND o.track_no = t.track_no
                  AND abs(o.duration_ms - t.duration_ms) <= 2000))
         GROUP BY t.release_id) c ON c.rid = r.id
 WHERE c.n >= 2 AND c.mine = c.n AND coalesce(r.total_tracks, 0) <= c.n
   AND NOT EXISTS (SELECT 1 FROM release_selection s WHERE s.release_id = r.id AND s.deleted = 0 AND s.mode != 'auto')
ON CONFLICT(release_id) DO UPDATE SET mode = 'whole', deleted = 0;
