-- What the spectrum check found about each local source's sound: the
-- verdict (a "FLAC" made from an MP3 is `transcoded`), the measured band,
-- the likely origin and the averaged spectrum the quality panel draws.
-- Keyed by source id without a foreign key: a check of a source that is
-- gone is only an orphan row, dropped by `drop_orphan_checks`.
CREATE TABLE source_quality (
    source_id    TEXT PRIMARY KEY,
    verdict      TEXT NOT NULL,                  -- genuine | transcoded | upsampled | unknown
    cutoff_hz    REAL NOT NULL DEFAULT 0,
    steepness_db REAL NOT NULL DEFAULT 0,
    origin       TEXT NOT NULL,                  -- lossless | lossy:<kbps> | upsampled | unknown
    sample_rate  INTEGER NOT NULL DEFAULT 0,
    spectrum     BLOB,                           -- dB x 10 as i16 LE, bands 0..rate/2
    made_from    TEXT,                           -- codec of the lossless file it was made from
    file_size    INTEGER NOT NULL DEFAULT 0,     -- the file when checked: another one is
    file_mtime   INTEGER NOT NULL DEFAULT 0,     -- checked again
    checked_at   INTEGER NOT NULL,
    failed       TEXT                            -- why it could not be checked
) WITHOUT ROWID;
