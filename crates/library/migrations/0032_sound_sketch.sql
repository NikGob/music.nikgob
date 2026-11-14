-- Migration 32 (r38-sound; renumber on merge if others land first).
--
-- What the background analysis's decode of a track tells beyond its
-- vectors, 40 bytes: the crest of the loudness of its analysis windows,
-- its head and tail (the first and last sounding seconds: loudness, a
-- cold cut or a fade, the silence, 12 pitch classes) and, when the neural
-- sound model heard it, a summary of the model's 400 Discogs styles.
-- Beside `analysis` and `sound_embedding`, never instead of them: a track
-- without one works as before.
--
-- sketch: the 40 bytes (nikgob_recs::SoundSketch; its first byte says
-- which parts are there). sketched_with: the version of the loudness and
-- the edges; another one is computed again. styled_with: the model and
-- the summary of the styles part; NULL while the model has not heard the
-- track (computed again once it runs). source: as analysis.source (a
-- file's always wins over a stream's).
--
-- Derived data keyed by the track; goes with it. Only a table is added:
-- no row of any other table changes.
CREATE TABLE IF NOT EXISTS sound_sketch (
    track_id      TEXT PRIMARY KEY,
    sketch        BLOB NOT NULL,
    sketched_with TEXT NOT NULL,
    styled_with   TEXT,
    source        TEXT NOT NULL DEFAULT 'file',   -- file | youtube
    sketched_at   INTEGER NOT NULL
) WITHOUT ROWID;
