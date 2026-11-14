-- One artist, several spellings: «Ohara Yuiko» is
-- 大原ゆい子, «Fujisawa Yoshiaki» and «Yoshiaki Fujisawa» are 藤澤慶昌. An
-- artist merged into another leaves its name here: services and files that
-- still say it find the artist, and search finds it by every spelling.

CREATE TABLE artist_alias (
    artist_id TEXT NOT NULL,
    name      TEXT NOT NULL,
    key       TEXT NOT NULL,                  -- nikgob_core::names::name_key(name)
    source    TEXT NOT NULL DEFAULT 'merged', -- merged | user
    PRIMARY KEY (key, artist_id)
) WITHOUT ROWID;
CREATE INDEX artist_alias_artist ON artist_alias(artist_id);

-- The search index of an artist and of its tracks knows the aliases too.
DROP TRIGGER artist_fts_ai;
DROP TRIGGER artist_fts_au;
CREATE TRIGGER artist_fts_ai AFTER INSERT ON artist BEGIN
    INSERT INTO artist_fts(rowid, name, alt) VALUES (
        new.seq,
        replace(replace(new.name, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.sort_name || coalesce(' ' || (SELECT group_concat(name, ' ') FROM artist_alias
                                                           WHERE artist_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER artist_fts_au AFTER UPDATE ON artist
WHEN old.name IS NOT new.name OR old.sort_name IS NOT new.sort_name BEGIN
    DELETE FROM artist_fts WHERE rowid = old.seq;
    INSERT INTO artist_fts(rowid, name, alt) VALUES (
        new.seq,
        replace(replace(new.name, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.sort_name || coalesce(' ' || (SELECT group_concat(name, ' ') FROM artist_alias
                                                           WHERE artist_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER artist_alias_ai AFTER INSERT ON artist_alias BEGIN
    DELETE FROM artist_fts WHERE rowid = (SELECT seq FROM artist WHERE id = new.artist_id);
    INSERT INTO artist_fts(rowid, name, alt)
    SELECT seq, replace(replace(name, 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(sort_name || coalesce(' ' || (SELECT group_concat(name, ' ') FROM artist_alias
                                                          WHERE artist_id = new.artist_id), ''), 'ё', 'е'), 'Ё', 'Е')
      FROM artist WHERE id = new.artist_id;
END;

DROP TRIGGER track_fts_ai;
DROP TRIGGER track_fts_au;
CREATE TRIGGER track_fts_ai AFTER INSERT ON track BEGIN
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.seq,
        replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е'),
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
        replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                    FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                   WHERE ta.track_id = new.id), ''), 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(coalesce((SELECT title FROM release WHERE id = new.release_id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
-- A track linked to an artist with aliases (its links are written after
-- the track row).
CREATE TRIGGER track_artist_alias_ai AFTER INSERT ON track_artist
WHEN EXISTS (SELECT 1 FROM artist_alias WHERE artist_id = new.artist_id) BEGIN
    DELETE FROM track_fts WHERE rowid = (SELECT seq FROM track WHERE id = new.track_id);
    INSERT INTO track_fts(rowid, title, artist, album)
    SELECT t.seq, replace(replace(t.title, 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(t.artist_display || coalesce(' ' || (SELECT group_concat(al.name, ' ')
                       FROM track_artist ta JOIN artist_alias al ON al.artist_id = ta.artist_id
                      WHERE ta.track_id = t.id), ''), 'ё', 'е'), 'Ё', 'Е'),
           replace(replace(coalesce((SELECT title FROM release WHERE id = t.release_id), ''), 'ё', 'е'), 'Ё', 'Е')
      FROM track t WHERE t.id = new.track_id;
END;
