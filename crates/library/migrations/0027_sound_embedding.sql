-- Migration 27 (r30-sound; renumber on merge if another 0027 lands first).
--
-- The neural sound model's vector of a track (docs/recommendations.md §5):
-- Discogs-EffNet's embedding of a few windows of the audio, projected to
-- 256 dimensions, L2-normalised and quantised to i8 with one scale. Beside
-- bliss's vector in `analysis`, never instead of it: a track without one
-- is compared by bliss.
--
-- vector: the scale (f32 little endian) and 256 i8, 260 bytes; NULL when
-- the model cannot make one of this track's audio (too short): not tried
-- again with the same `embedded_with`.
-- embedded_with: the model and the projection; another one is computed
-- again. source: as analysis.source (a file's always wins over a stream's
-- of the same model).
--
-- Derived data keyed by the track; goes with it.
CREATE TABLE IF NOT EXISTS sound_embedding (
    track_id      TEXT PRIMARY KEY,
    vector        BLOB,
    embedded_with TEXT NOT NULL,
    source        TEXT NOT NULL DEFAULT 'file',   -- file | youtube
    embedded_at   INTEGER NOT NULL
) WITHOUT ROWID;
