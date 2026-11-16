-- Syncing the library between the owner's devices.
--
-- * sync_clock: one row. `hlc` is this database's hybrid logical clock (unix
--   ms that never goes back and moves past every time seen from another
--   device), `seq` the last change number given out, `node` this device's id
--   (stamped into the change log), `applying` set while rows from another
--   device are written (the change triggers then stay quiet: the writer logs
--   those rows itself, with their own versions).
-- * change_log: the last change of every synced row, by table and key (the
--   key as `json_array` of the key columns): its number (`seq`, growing over
--   the whole database), its version (`hlc`, `node`, `gone` for a row deleted)
--   and the device it came from when it was taken from one (`came_from`: not
--   offered back to that device). Filled by triggers the library installs
--   from its list of synced tables.
-- * sync_peer: how far this device has taken each table of each device.
-- * sync_node: per device: syncing turned off, the other side's database
--   (its epoch: a new one starts from the beginning), the last good pull.
-- * node_file: which device has which file (local sources minus their paths),
--   kept by the triggers below; synced like the rest.
-- * event.actor: who started the play (owner | assistant | agent | guest;
--   NULL in older rows = the owner).
-- * lyrics_align.fits: whether the text fits the recording (the word timing
--   of a text that does not is kept too now, but never shown as fitting).

CREATE TABLE sync_clock (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    seq      INTEGER NOT NULL DEFAULT 0,
    hlc      INTEGER NOT NULL DEFAULT 0,
    node     TEXT NOT NULL DEFAULT '',
    applying INTEGER NOT NULL DEFAULT 0
);
INSERT INTO sync_clock(id, seq, hlc, node, applying)
VALUES (1, 0, 0, coalesce((SELECT value FROM meta WHERE key = 'device_id'), ''), 0);

CREATE TABLE change_log (
    tbl       TEXT NOT NULL,
    key       TEXT NOT NULL,
    seq       INTEGER NOT NULL,
    hlc       INTEGER NOT NULL,
    node      TEXT NOT NULL,
    gone      INTEGER NOT NULL DEFAULT 0,
    came_from TEXT,
    PRIMARY KEY (tbl, key)
) WITHOUT ROWID;
CREATE INDEX change_log_seq ON change_log(tbl, seq);

CREATE TABLE sync_peer (
    node TEXT NOT NULL,
    tbl  TEXT NOT NULL,
    seq  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (node, tbl)
) WITHOUT ROWID;

CREATE TABLE sync_node (
    node     TEXT PRIMARY KEY,
    off      INTEGER NOT NULL DEFAULT 0,
    epoch    TEXT,
    last_ok  INTEGER
) WITHOUT ROWID;

CREATE TABLE node_file (
    node_id       TEXT NOT NULL,
    source_id     TEXT NOT NULL,
    track_id      TEXT NOT NULL,
    file_hash     TEXT,
    codec         TEXT NOT NULL,
    size_bytes    INTEGER,
    quality_score INTEGER NOT NULL DEFAULT 0,
    bitrate       INTEGER,
    sample_rate   INTEGER,
    bit_depth     INTEGER,
    PRIMARY KEY (node_id, source_id)
) WITHOUT ROWID;
CREATE INDEX node_file_hash ON node_file(file_hash) WHERE file_hash IS NOT NULL;
CREATE INDEX node_file_track ON node_file(track_id);

CREATE TRIGGER node_file_ai AFTER INSERT ON source
WHEN new.kind = 'local_file' AND new.trashed_at IS NULL BEGIN
    INSERT OR IGNORE INTO node_file(node_id, source_id, track_id, file_hash, codec, size_bytes, quality_score,
                                    bitrate, sample_rate, bit_depth)
    SELECT node, new.id, new.track_id, new.file_hash, new.codec, new.size_bytes, new.quality_score,
           new.bitrate, new.sample_rate, new.bit_depth
      FROM sync_clock WHERE id = 1;
END;

CREATE TRIGGER node_file_au AFTER UPDATE ON source
WHEN new.kind = 'local_file' OR old.kind = 'local_file' BEGIN
    DELETE FROM node_file
     WHERE source_id = old.id AND node_id = (SELECT node FROM sync_clock WHERE id = 1)
       AND (new.trashed_at IS NOT NULL OR new.kind <> 'local_file');
    UPDATE node_file SET track_id = new.track_id, file_hash = new.file_hash, codec = new.codec,
           size_bytes = new.size_bytes, quality_score = new.quality_score, bitrate = new.bitrate,
           sample_rate = new.sample_rate, bit_depth = new.bit_depth
     WHERE source_id = new.id AND node_id = (SELECT node FROM sync_clock WHERE id = 1)
       AND new.kind = 'local_file' AND new.trashed_at IS NULL;
    INSERT OR IGNORE INTO node_file(node_id, source_id, track_id, file_hash, codec, size_bytes, quality_score,
                                    bitrate, sample_rate, bit_depth)
    SELECT node, new.id, new.track_id, new.file_hash, new.codec, new.size_bytes, new.quality_score,
           new.bitrate, new.sample_rate, new.bit_depth
      FROM sync_clock WHERE id = 1 AND new.kind = 'local_file' AND new.trashed_at IS NULL;
END;

CREATE TRIGGER node_file_ad AFTER DELETE ON source
WHEN old.kind = 'local_file' BEGIN
    DELETE FROM node_file WHERE source_id = old.id AND node_id = (SELECT node FROM sync_clock WHERE id = 1);
END;

INSERT OR IGNORE INTO node_file(node_id, source_id, track_id, file_hash, codec, size_bytes, quality_score,
                      bitrate, sample_rate, bit_depth)
SELECT c.node, s.id, s.track_id, s.file_hash, s.codec, s.size_bytes, s.quality_score,
       s.bitrate, s.sample_rate, s.bit_depth
  FROM source s, sync_clock c
 WHERE s.kind = 'local_file' AND s.trashed_at IS NULL AND c.id = 1;

ALTER TABLE event ADD COLUMN actor TEXT;

ALTER TABLE lyrics_align ADD COLUMN fits INTEGER NOT NULL DEFAULT 0;
UPDATE lyrics_align SET fits = (body IS NOT NULL);
