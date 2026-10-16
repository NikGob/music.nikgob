-- Deleted tracks ("Корзина") and a persistent log of finished jobs.
--
-- track.deleted_at: the user deleted the track. It is hidden from every library
-- listing (and so are releases and artists left with deleted tracks only) until
-- it is restored or purged for good. Its sources are trashed at the same moment
-- (source.trashed_at = track.deleted_at), so a restore knows which files to
-- bring back and leaves older upgrade leftovers in Trash.
ALTER TABLE track ADD COLUMN deleted_at INTEGER;
CREATE INDEX track_deleted ON track(deleted_at) WHERE deleted_at IS NOT NULL;

-- Tracks deleted before this column existed: every source trashed, none live
-- (the "0/1" albums). They move to the Trash view as of their last trashing.
UPDATE track SET deleted_at = (SELECT MAX(s.trashed_at) FROM source s WHERE s.track_id = track.id)
 WHERE EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id AND s.trashed_at IS NOT NULL)
   AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id AND s.trashed_at IS NULL);
-- ... and those whose trashed files were already purged (no source at all).
UPDATE track SET deleted_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000
 WHERE deleted_at IS NULL AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id);

-- Finished imports and downloads ("Загрузки → История"), newest last. A plain
-- log: no ids of other tables, trimmed by the runtime.
CREATE TABLE job_log (
    seq      INTEGER PRIMARY KEY,
    kind     TEXT NOT NULL,                  -- 'import' | 'torrent'
    title    TEXT NOT NULL,
    subtitle TEXT NOT NULL,
    error    TEXT,
    at       INTEGER NOT NULL
);
