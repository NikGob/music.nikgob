-- The graph of similar music: what other listeners
-- know of the library's tracks and artists, fetched in the background and
-- kept here so the recommendations work offline. New tables only.

-- What was asked of a source and when: `status` ok | none (nothing found)
-- | error (asked again later); `remote_id` what the subject is in that
-- source when a search had to find it (a MusicBrainz id, a service's id).
CREATE TABLE IF NOT EXISTS graph_fetch (
    subject_kind TEXT NOT NULL,              -- track | artist
    subject_id   TEXT NOT NULL,              -- the library's id
    source       TEXT NOT NULL,              -- youtube_radio | youtube_artists | deezer_artists | listenbrainz | lastfm | musicbrainz | popularity
    fetched_at   INTEGER NOT NULL,
    status       TEXT NOT NULL,
    remote_id    TEXT,
    PRIMARY KEY (subject_kind, subject_id, source)
) WITHOUT ROWID;

-- A source's list for a subject, as the source named its entries: they
-- are resolved to the library at every build, so songs that arrive later
-- join by themselves. `kind` is what the entry is (track | artist).
CREATE TABLE IF NOT EXISTS graph_edge (
    subject_kind TEXT NOT NULL,
    subject_id   TEXT NOT NULL,
    source       TEXT NOT NULL,
    rank         INTEGER NOT NULL,
    total        INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    name         TEXT NOT NULL,
    artist       TEXT NOT NULL DEFAULT '',
    remote_id    TEXT,
    PRIMARY KEY (subject_kind, subject_id, source, rank)
) WITHOUT ROWID;

-- The tags of an artist (MusicBrainz's tags and genres, their counts).
CREATE TABLE IF NOT EXISTS graph_tag (
    subject_kind TEXT NOT NULL,
    subject_id   TEXT NOT NULL,
    tag          TEXT NOT NULL,
    weight       REAL NOT NULL,
    PRIMARY KEY (subject_kind, subject_id, tag)
) WITHOUT ROWID;
