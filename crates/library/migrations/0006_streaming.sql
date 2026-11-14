-- Streaming (stage 3): tracks outside the library, ids in the
-- services, playlist sync state and the extended wishlist.

-- track.in_library: shown in the library. A track that only has stream
-- sources (played once from Spotify / YouTube) gets a permanent row for its
-- history but stays out of every library listing until it is liked or put in
-- a live playlist. Maintained by the library (`refresh_in_library`): 1 for
-- any track with a local source row or without stream sources, otherwise
-- liked OR in a live playlist. Every existing track is a library track.
ALTER TABLE track ADD COLUMN in_library INTEGER NOT NULL DEFAULT 1;
CREATE INDEX track_outside_library ON track(in_library) WHERE in_library = 0;

-- What a catalog / user-data row is called in a service. No foreign keys
-- (local_id is the uuid of a track, release, artist or playlist row).
CREATE TABLE external_id (
    service   TEXT NOT NULL,                 -- spotify | youtube
    kind      TEXT NOT NULL,                 -- track | release | artist | playlist
    remote_id TEXT NOT NULL,
    local_id  TEXT NOT NULL,
    PRIMARY KEY (service, kind, remote_id)
) WITHOUT ROWID;
CREATE INDEX external_id_local ON external_id(kind, local_id);

-- A library playlist linked to a playlist in a service.
CREATE TABLE playlist_remote (
    playlist_id    TEXT PRIMARY KEY,
    service        TEXT NOT NULL,
    remote_id      TEXT NOT NULL,
    sync_mode      TEXT NOT NULL DEFAULT 'follow',   -- two_way | follow | once
    remote_version TEXT,                             -- snapshot id / etag last seen
    synced_at      INTEGER,
    base_json      TEXT                              -- last synced content (3-way merge base)
) WITHOUT ROWID;
CREATE UNIQUE INDEX playlist_remote_by_remote ON playlist_remote(service, remote_id);

-- Wishlist items point at library rows and remember why and how often.
-- Status values gain waiting | no_torrent | dismissed (text, no change needed).
ALTER TABLE wishlist ADD COLUMN track_id TEXT;
ALTER TABLE wishlist ADD COLUMN release_id TEXT;
ALTER TABLE wishlist ADD COLUMN scope TEXT NOT NULL DEFAULT 'track';     -- track | album
ALTER TABLE wishlist ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual';   -- like | import | sync | manual
ALTER TABLE wishlist ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE wishlist ADD COLUMN next_try_at INTEGER;
ALTER TABLE wishlist ADD COLUMN note TEXT;
CREATE INDEX wishlist_track ON wishlist(track_id) WHERE track_id IS NOT NULL;
CREATE INDEX wishlist_release ON wishlist(release_id) WHERE release_id IS NOT NULL;
CREATE INDEX wishlist_due ON wishlist(status, next_try_at);
