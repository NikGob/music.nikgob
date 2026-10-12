-- Artist photos: a manual lock, and re-resolution of photos found by plain name search.
--
-- photo_locked = 1: the user chose this photo (source = 'custom'); background
-- enrichment never overwrites or clears it.
-- Photos recorded so far came from an unverified name search (wrong namesakes
-- were possible); they are dropped to "never tried" (fetched_at = 0) so the
-- worker re-resolves them through the track-anchored / verified lookup.
ALTER TABLE artist_photo ADD COLUMN photo_locked INTEGER NOT NULL DEFAULT 0;
UPDATE artist_photo SET path = NULL, fetched_at = 0 WHERE path IS NOT NULL;
