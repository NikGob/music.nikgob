-- Migration 26: registered after 0025_album_selection.sql (r29-albums).
--
-- A readable name for an artist named in a script the user may not read
-- (kanji, kana, hangul…): its Latin name from MusicBrainz, Wikidata or a
-- service where the artist was proved to be the library's, else its kana
-- read in romaji. Shown beside the artist's own name and searched; it never
-- decides which artist another credit is. And whether the credit is a role
-- (an anime character a voice actor sings as): a role gets no photo.
--
-- A cache keyed by the artist id with no foreign key, like artist_photo:
-- it can be dropped and looked up again at any time.
CREATE TABLE IF NOT EXISTS artist_latin (
    artist_id     TEXT PRIMARY KEY,
    name          TEXT,                        -- NULL: none found (looked up again later)
    source        TEXT NOT NULL DEFAULT '',    -- musicbrainz | wikidata | youtube | spotify | romaji
    character     INTEGER NOT NULL DEFAULT 0,  -- 1: a role, not a person
    mbid          TEXT,                        -- the MusicBrainz artist it was proved to be
    wikidata      TEXT,                        -- that artist's Wikidata item (Q…)
    identified_at INTEGER NOT NULL DEFAULT 0,  -- the last MusicBrainz lookup (unix ms); 0: never
    fetched_at    INTEGER NOT NULL DEFAULT 0   -- the last complete name lookup (unix ms); 0: never
) WITHOUT ROWID;
