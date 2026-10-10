-- nikgob library schema, version 1 (PLAN §4 in full, including the sync/backup tails).
--
-- Conventions
--   * ids are uuid text; `seq` is a stable integer rowid used only to key the FTS tables;
--   * timestamps are unix milliseconds;
--   * user data (playlist, playlist_entry, likes) has updated_at / updated_by / deleted and is
--     never hard-deleted; it carries NO foreign keys, so userdata can be imported before the
--     catalog is rebuilt and sync can deliver rows in any order;
--   * FTS text is "folded" in SQL (ё -> е) so е/ё are interchangeable; the tokenizer lowercases.

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

-- ---------------------------------------------------------------- catalog

CREATE TABLE artist (
    seq       INTEGER PRIMARY KEY,
    id        TEXT NOT NULL UNIQUE,
    mbid      TEXT UNIQUE,
    name      TEXT NOT NULL,
    sort_name TEXT NOT NULL,
    sort_key  TEXT NOT NULL,
    images    TEXT NOT NULL DEFAULT '[]',
    added_at  INTEGER NOT NULL
);
CREATE INDEX artist_sort ON artist(sort_key, seq);
CREATE INDEX artist_added ON artist(added_at, seq);

CREATE TABLE release (
    seq                INTEGER PRIMARY KEY,
    id                 TEXT NOT NULL UNIQUE,
    mbid               TEXT UNIQUE,
    release_group_mbid TEXT,
    title              TEXT NOT NULL,
    title_key          TEXT NOT NULL,
    artist_credit      TEXT NOT NULL,   -- json: ArtistCredit
    artist_display     TEXT NOT NULL,
    artist_key         TEXT NOT NULL,
    year               INTEGER,
    release_type       TEXT NOT NULL DEFAULT 'album',
    cover_ref          TEXT,
    total_tracks       INTEGER,
    added_at           INTEGER NOT NULL
);
CREATE INDEX release_title ON release(title_key, seq);
CREATE INDEX release_by_artist_key ON release(artist_key, year, title_key, seq);
CREATE INDEX release_year ON release(year, seq);
CREATE INDEX release_added ON release(added_at, seq);

CREATE TABLE release_artist (
    release_id TEXT NOT NULL,
    artist_id  TEXT NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (release_id, artist_id)
) WITHOUT ROWID;
CREATE INDEX release_artist_by_artist ON release_artist(artist_id);

CREATE TABLE track (
    seq            INTEGER PRIMARY KEY,
    id             TEXT NOT NULL UNIQUE,
    recording_mbid TEXT,
    isrc           TEXT,
    acoustid       TEXT,
    title          TEXT NOT NULL,
    title_key      TEXT NOT NULL,
    artist_credit  TEXT NOT NULL,       -- json: ArtistCredit
    artist_display TEXT NOT NULL,
    artist_key     TEXT NOT NULL,
    release_id     TEXT REFERENCES release(id),
    album_key      TEXT NOT NULL DEFAULT '',   -- denormalised release title key, for sorting
    disc_no        INTEGER NOT NULL DEFAULT 1,
    track_no       INTEGER NOT NULL DEFAULT 0,
    duration_ms    INTEGER,
    added_at       INTEGER NOT NULL,
    -- denormalised from the active source; maintained by the library only
    active_kind    TEXT,
    active_codec   TEXT,
    active_quality INTEGER,
    has_local      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX track_title ON track(title_key, seq);
CREATE INDEX track_by_artist_key ON track(artist_key, album_key, disc_no, track_no, seq);
CREATE INDEX track_album ON track(album_key, disc_no, track_no, seq);
CREATE INDEX track_added ON track(added_at, seq);
CREATE INDEX track_release ON track(release_id, disc_no, track_no);
CREATE INDEX track_recording ON track(recording_mbid) WHERE recording_mbid IS NOT NULL;
CREATE INDEX track_isrc ON track(isrc) WHERE isrc IS NOT NULL;
CREATE INDEX track_acoustid ON track(acoustid) WHERE acoustid IS NOT NULL;

CREATE TABLE track_artist (
    track_id  TEXT NOT NULL,
    artist_id TEXT NOT NULL,
    position  INTEGER NOT NULL,
    PRIMARY KEY (track_id, artist_id)
) WITHOUT ROWID;
CREATE INDEX track_artist_by_artist ON track_artist(artist_id);

CREATE TABLE source (
    id               TEXT PRIMARY KEY,
    track_id         TEXT NOT NULL REFERENCES track(id),
    kind             TEXT NOT NULL,           -- local_file | spotify | youtube
    locator          TEXT NOT NULL,           -- path | uri | video id
    codec            TEXT NOT NULL,
    bitrate          INTEGER,                 -- kbps
    sample_rate      INTEGER,
    bit_depth        INTEGER,
    quality_score    INTEGER NOT NULL DEFAULT 0,
    is_active        INTEGER NOT NULL DEFAULT 0,
    file_hash        TEXT,
    size_bytes       INTEGER,
    origin_kind      TEXT NOT NULL DEFAULT 'inbox',   -- torrent | inbox | stream
    origin_info_hash TEXT,
    origin_path      TEXT,
    available        INTEGER NOT NULL DEFAULT 1,
    added_at         INTEGER NOT NULL,
    trashed_at       INTEGER,
    trash_path       TEXT
);
CREATE INDEX source_track ON source(track_id);
CREATE UNIQUE INDEX source_one_active ON source(track_id) WHERE is_active = 1;
CREATE UNIQUE INDEX source_locator ON source(kind, locator) WHERE trashed_at IS NULL;
CREATE INDEX source_hash ON source(file_hash) WHERE file_hash IS NOT NULL;
CREATE INDEX source_trashed ON source(trashed_at) WHERE trashed_at IS NOT NULL;

-- ------------------------------------------------------- backup / network tails

CREATE TABLE node (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    kind      TEXT NOT NULL,                  -- pc | pi | server | phone
    last_seen INTEGER
) WITHOUT ROWID;

CREATE TABLE master (
    track_id         TEXT PRIMARY KEY,
    codec            TEXT NOT NULL,
    file_hash        TEXT NOT NULL,
    size_bytes       INTEGER NOT NULL,
    origin_kind      TEXT,
    origin_info_hash TEXT,
    origin_path      TEXT
) WITHOUT ROWID;

CREATE TABLE replica (
    file_hash   TEXT NOT NULL,
    node_id     TEXT NOT NULL,
    verified_at INTEGER,
    PRIMARY KEY (file_hash, node_id)
) WITHOUT ROWID;

CREATE TABLE wishlist (
    id              TEXT PRIMARY KEY,
    title           TEXT,
    artist          TEXT,
    recording_mbid  TEXT,
    isrc            TEXT,
    spotify_id      TEXT,
    youtube_id      TEXT,
    status          TEXT NOT NULL DEFAULT 'new',
    candidates_json TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX wishlist_status ON wishlist(status);

CREATE TABLE download (
    info_hash      TEXT PRIMARY KEY,
    magnet         TEXT,
    torrent_file   TEXT,
    selected_files TEXT NOT NULL DEFAULT '[]',
    status         TEXT NOT NULL DEFAULT 'queued',
    seed_policy    TEXT NOT NULL DEFAULT 'standard',
    bytes_up       INTEGER NOT NULL DEFAULT 0,
    bytes_down     INTEGER NOT NULL DEFAULT 0,
    wishlist_id    TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
) WITHOUT ROWID;

-- ------------------------------------------------------------ derived data

CREATE TABLE analysis (
    track_id      TEXT PRIMARY KEY,
    bliss_vector  BLOB NOT NULL,              -- f32 little endian
    analyzed_with TEXT NOT NULL,
    analyzed_at   INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE palette (
    release_id   TEXT PRIMARY KEY,
    seed_argb    INTEGER NOT NULL,
    scheme_blob  BLOB,
    ambient_blur BLOB,
    computed_at  INTEGER NOT NULL
) WITHOUT ROWID;

-- --------------------------------------------------- user data (CRDT-friendly)

CREATE TABLE playlist (
    seq        INTEGER PRIMARY KEY,
    id         TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    cover      TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    deleted    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX playlist_list ON playlist(deleted, updated_at);

CREATE TABLE playlist_entry (
    id          TEXT PRIMARY KEY,
    playlist_id TEXT NOT NULL,
    track_id    TEXT NOT NULL,
    position    TEXT NOT NULL,                -- fractional index, base-62, binary order
    added_at    INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE INDEX playlist_entry_order ON playlist_entry(playlist_id, position, id) WHERE deleted = 0;
CREATE INDEX playlist_entry_track ON playlist_entry(track_id) WHERE deleted = 0;

-- LWW-element-set keyed by target.
CREATE TABLE likes (
    target_kind TEXT NOT NULL,                -- track | release | artist
    target_id   TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,
    updated_by  TEXT NOT NULL,
    deleted     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (target_kind, target_id)
) WITHOUT ROWID;
CREATE INDEX likes_live ON likes(target_kind, updated_at) WHERE deleted = 0;

-- Append-only.
CREATE TABLE event (
    id          TEXT PRIMARY KEY,
    ts          INTEGER NOT NULL,
    track_id    TEXT NOT NULL,
    source_kind TEXT,
    type        TEXT NOT NULL,
    position_ms INTEGER,
    context     TEXT NOT NULL DEFAULT 'other',
    device      TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX event_ts ON event(ts);
CREATE INDEX event_track ON event(track_id, ts);

-- ------------------------------------------------------------------ search
-- One FTS table per entity, rowid = the entity's `seq`. Standalone (not external-content) so the
-- folded text can differ from the stored text.

CREATE VIRTUAL TABLE track_fts USING fts5(
    title, artist, album,
    tokenize = "unicode61 remove_diacritics 2", prefix = '2 3'
);
CREATE VIRTUAL TABLE release_fts USING fts5(
    title, artist,
    tokenize = "unicode61 remove_diacritics 2", prefix = '2 3'
);
CREATE VIRTUAL TABLE artist_fts USING fts5(
    name, alt,
    tokenize = "unicode61 remove_diacritics 2", prefix = '2 3'
);
CREATE VIRTUAL TABLE playlist_fts USING fts5(
    name,
    tokenize = "unicode61 remove_diacritics 2", prefix = '2 3'
);

-- fold(x) = replace(replace(x, 'ё', 'е'), 'Ё', 'Е')

CREATE TRIGGER track_fts_ai AFTER INSERT ON track BEGIN
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.seq,
        replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(coalesce((SELECT title FROM release WHERE id = new.release_id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER track_fts_au AFTER UPDATE ON track
WHEN old.title IS NOT new.title OR old.artist_display IS NOT new.artist_display
     OR old.release_id IS NOT new.release_id BEGIN
    DELETE FROM track_fts WHERE rowid = old.seq;
    INSERT INTO track_fts(rowid, title, artist, album) VALUES (
        new.seq,
        replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(coalesce((SELECT title FROM release WHERE id = new.release_id), ''), 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER track_fts_ad AFTER DELETE ON track BEGIN
    DELETE FROM track_fts WHERE rowid = old.seq;
END;

CREATE TRIGGER release_fts_ai AFTER INSERT ON release BEGIN
    INSERT INTO release_fts(rowid, title, artist) VALUES (
        new.seq,
        replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display, 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER release_fts_au AFTER UPDATE ON release
WHEN old.title IS NOT new.title OR old.artist_display IS NOT new.artist_display BEGIN
    DELETE FROM release_fts WHERE rowid = old.seq;
    INSERT INTO release_fts(rowid, title, artist) VALUES (
        new.seq,
        replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.artist_display, 'ё', 'е'), 'Ё', 'Е'));
    UPDATE track_fts SET album = replace(replace(new.title, 'ё', 'е'), 'Ё', 'Е')
     WHERE rowid IN (SELECT seq FROM track WHERE release_id = new.id);
END;
CREATE TRIGGER release_fts_ad AFTER DELETE ON release BEGIN
    DELETE FROM release_fts WHERE rowid = old.seq;
END;

CREATE TRIGGER artist_fts_ai AFTER INSERT ON artist BEGIN
    INSERT INTO artist_fts(rowid, name, alt) VALUES (
        new.seq,
        replace(replace(new.name, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.sort_name, 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER artist_fts_au AFTER UPDATE ON artist
WHEN old.name IS NOT new.name OR old.sort_name IS NOT new.sort_name BEGIN
    DELETE FROM artist_fts WHERE rowid = old.seq;
    INSERT INTO artist_fts(rowid, name, alt) VALUES (
        new.seq,
        replace(replace(new.name, 'ё', 'е'), 'Ё', 'Е'),
        replace(replace(new.sort_name, 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER artist_fts_ad AFTER DELETE ON artist BEGIN
    DELETE FROM artist_fts WHERE rowid = old.seq;
END;

-- Tombstoned playlists leave the index.
CREATE TRIGGER playlist_fts_ai AFTER INSERT ON playlist WHEN new.deleted = 0 BEGIN
    INSERT INTO playlist_fts(rowid, name) VALUES (
        new.seq, replace(replace(new.name, 'ё', 'е'), 'Ё', 'Е'));
END;
CREATE TRIGGER playlist_fts_au AFTER UPDATE ON playlist
WHEN old.name IS NOT new.name OR old.deleted IS NOT new.deleted BEGIN
    DELETE FROM playlist_fts WHERE rowid = old.seq;
    INSERT INTO playlist_fts(rowid, name)
    SELECT new.seq, replace(replace(new.name, 'ё', 'е'), 'Ё', 'Е') WHERE new.deleted = 0;
END;
CREATE TRIGGER playlist_fts_ad AFTER DELETE ON playlist BEGIN
    DELETE FROM playlist_fts WHERE rowid = old.seq;
END;
