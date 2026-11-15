-- Migration 33.
--
-- An artist photo lookup that Deezer could not answer for now (a 404 in
-- its lookup or its image, a lookup out of time) is asked again after a
-- day, then two, four, eight, at most the fourteen days of a miss: `tries`
-- counts these in a row (source 'later').
ALTER TABLE artist_photo ADD COLUMN tries INTEGER NOT NULL DEFAULT 0;

-- Every url in an artist's images comes from a service's artist page, and
-- the only Google-hosted ones are YouTube Music's: one asked wider than
-- tall (`…=w2880-h1200-…`) is the channel banner, no photo of the artist.
-- Those entries are dropped; every other entry stays where it was.
WITH parts AS (
    SELECT a.id AS id, e.key AS k, substr(e.value, instr(e.value, '=w') + 2) AS rest
      FROM artist a, json_each(CASE WHEN json_valid(a.images) THEN a.images ELSE '[]' END) e
     WHERE e.type = 'text'
       AND (e.value LIKE 'https://%.googleusercontent.com/%' OR e.value LIKE 'https://%.ggpht.com/%')
       AND instr(e.value, '=w') > 0
), banners AS (
    SELECT id, k FROM parts
     WHERE instr(rest, '-h') > 0
       AND CAST(substr(rest, instr(rest, '-h') + 2) AS INTEGER) > 0
       AND CAST(rest AS INTEGER) * 10 > CAST(substr(rest, instr(rest, '-h') + 2) AS INTEGER) * 13
)
UPDATE artist SET images = (
    SELECT json_group_array(value) FROM (
        SELECT e.value AS value FROM json_each(artist.images) e
         WHERE NOT EXISTS (SELECT 1 FROM banners b WHERE b.id = artist.id AND b.k = e.key)
         ORDER BY e.key)
) WHERE id IN (SELECT id FROM banners);
