-- Migration 33.
--
-- An artist photo lookup that Deezer could not answer for now (a 404 in
-- its lookup or its image, a lookup out of time) is asked again after a
-- day, then two, four, eight, at most the fourteen days of a miss: `tries`
-- counts these in a row (source 'later').
ALTER TABLE artist_photo ADD COLUMN tries INTEGER NOT NULL DEFAULT 0;

-- YouTube Music channel banners stored as an artist's image (a Google
-- image asked wider than tall, `…=w2880-h1200-…`): no photo of the artist.
-- Only an artist whose images are that one web url loses it; a file, a
-- picture the user chose and a square service image stay.
WITH one AS (
    SELECT id, json_extract(images, '$[0]') AS u FROM artist
     WHERE json_valid(images) AND json_array_length(images) = 1
), google AS (
    SELECT id, substr(u, instr(u, '=w') + 2) AS rest FROM one
     WHERE (u LIKE 'https://%.googleusercontent.com/%' OR u LIKE 'https://%.ggpht.com/%')
       AND instr(u, '=w') > 0
), sized AS (
    SELECT id, CAST(rest AS INTEGER) AS w, CAST(substr(rest, instr(rest, '-h') + 2) AS INTEGER) AS h
      FROM google WHERE instr(rest, '-h') > 0
)
UPDATE artist SET images = '[]' WHERE id IN (SELECT id FROM sized WHERE h > 0 AND w * 10 > h * 13);
