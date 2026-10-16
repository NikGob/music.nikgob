-- The liked tracks of each streaming service as last seen (stage 3, likes
-- both ways): a pull compares the service's list with this one to tell a new
-- remote like from a remote unlike, and `pulled` says whether the local like
-- came from that service (only such a like goes when the service unlikes).
CREATE TABLE remote_like (
    service   TEXT NOT NULL,                 -- spotify | youtube
    remote_id TEXT NOT NULL,
    track_id  TEXT NOT NULL,
    pulled    INTEGER NOT NULL DEFAULT 0,    -- the local like was made by a pull
    seen_at   INTEGER NOT NULL,
    PRIMARY KEY (service, remote_id)
) WITHOUT ROWID;
CREATE INDEX remote_like_track ON remote_like(track_id);
