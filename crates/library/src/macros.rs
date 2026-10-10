//! SQL fragments shared by several modules (macros so they can be `concat!`ed).

/// The 15 columns of a [`crate::TrackRow`]; needs aliases `t` (track) and `r` (release).
macro_rules! track_row_cols {
    () => {
        "t.id, t.title, t.artist_display, r.title, t.release_id, r.cover_ref, t.disc_no, \
         t.track_no, t.duration_ms, t.active_quality, t.active_codec, t.active_kind, \
         t.has_local, \
         EXISTS(SELECT 1 FROM likes lk WHERE lk.target_kind = 'track' \
                AND lk.target_id = t.id AND lk.deleted = 0),          (SELECT ta.artist_id FROM track_artist ta WHERE ta.track_id = t.id                 ORDER BY ta.position LIMIT 1)"
    };
}

macro_rules! track_row_from {
    () => {
        "FROM track t LEFT JOIN release r ON r.id = t.release_id"
    };
}
