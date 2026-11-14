-- Torrent → library rework.

-- «Подтверди»: files the importer could not identify alone wait here for
-- the user and survive a restart (they used to live in memory only, the
-- files stayed in Downloads). `payload` is the importer's own (json: the
-- file's origin, its targets, the options offered); `wishlist_id` is the
-- «Хочу скачать» item the file was downloaded for. Not user data: not
-- exported, not synced.
CREATE TABLE confirm_item (
    id          INTEGER PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    wishlist_id TEXT,
    payload     TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX confirm_item_wishlist ON confirm_item(wishlist_id) WHERE wishlist_id IS NOT NULL;

-- One song, one track: the same song is looked up by its length (with any
-- credited artist, not only the main one).
CREATE INDEX track_duration ON track(duration_ms) WHERE deleted_at IS NULL;
