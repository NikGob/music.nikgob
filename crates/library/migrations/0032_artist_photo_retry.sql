-- Migration 32.
--
-- Artist photo misses recorded before the lookup learned to prove an
-- artist by its own track, to tell «not proved» from «no photo», to ask
-- with the artist's Latin name and to keep a 404 from being a miss: every
-- miss the user did not lock is asked again at the next start (0 = never
-- tried). Only the photo cache is touched.
UPDATE artist_photo SET fetched_at = 0 WHERE path IS NULL AND photo_locked = 0;
