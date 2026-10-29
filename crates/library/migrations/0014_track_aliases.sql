-- One song, two titles: «ベラフの子守歌» of a service and «Belaf's Lullaby»
-- of the file are one track. The title that went is kept here: search
-- finds the track by either.

CREATE TABLE track_alias (
    track_id TEXT NOT NULL,
    title    TEXT NOT NULL,
    key      TEXT NOT NULL,                  -- sort_key(title)
    source   TEXT NOT NULL DEFAULT 'merged', -- merged | import
    PRIMARY KEY (key, track_id)
) WITHOUT ROWID;
CREATE INDEX track_alias_track ON track_alias(track_id);

-- The search index of a track knows its aliases in its title.
DROP TRIGGER track_fts_ai;
DROP TRIGGER track_fts_au;
DROP TRIGGER track_artist_alias_ai;
CREATE TRIGGER track_fts_ai AFTER INSERT ON track BEGIN
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.seq,
        replace(replace(new.title || coalesce(' ' || (SELECT group_concat(title, ' ') FROM track_alias
                                                       WHERE track_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                    FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                   WHERE ta.track_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(coalesce((SELECT title FROM release WHERE id = new.release_id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER track_fts_au AFTER UPDATE ON track
WHEN old.title IS NOT new.title OR old.artist_display IS NOT new.artist_display
     OR old.release_id IS NOT new.release_id BEGIN
    DELETE FROM track_fts WHERE rowid = old.seq;
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.seq,
        replace(replace(new.title || coalesce(' ' || (SELECT group_concat(title, ' ') FROM track_alias
                                                       WHERE track_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                    FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                   WHERE ta.track_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(coalesce((SELECT title FROM release WHERE id = new.release_id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER track_artist_alias_ai AFTER INSERT ON track_artist
WHEN EXISTS (SELECT 1 FROM artist_alias WHERE artist_id = new.artist_id) BEGIN
    DELETE FROM track_fts WHERE rowid = (SELECT seq FROM track WHERE id = new.track_id);
    INSERT INTO track_fts(rowid, title, artist, album)
    SELECT t.seq,
           replace(replace(t.title || coalesce(' ' || (SELECT group_concat(title, ' ') FROM track_alias
                                                        WHERE track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(t.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                       FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                      WHERE ta.track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(coalesce((SELECT title FROM release WHERE id = t.release_id), ''), 'ё', 'е'), 'Ё', 'Е')
      FROM track t WHERE t.id = new.track_id;
END;
CREATE TRIGGER track_alias_ai AFTER INSERT ON track_alias BEGIN
    DELETE FROM track_fts WHERE rowid = (SELECT seq FROM track WHERE id = new.track_id);
    INSERT INTO track_fts(rowid, title, artist, album)
    SELECT t.seq,
           replace(replace(t.title || coalesce(' ' || (SELECT group_concat(title, ' ') FROM track_alias
                                                        WHERE track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(t.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                       FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                      WHERE ta.track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(coalesce((SELECT title FROM release WHERE id = t.release_id), ''), 'ё', 'е'), 'Ё', 'Е')
      FROM track t WHERE t.id = new.track_id;
END;
CREATE TRIGGER track_alias_gone AFTER DELETE ON track BEGIN
    DELETE FROM track_alias WHERE track_id = old.id;
END;
