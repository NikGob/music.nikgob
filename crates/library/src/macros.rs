//! SQL fragments shared by several modules (macros so they can be `concat!`ed).

/// The 16 columns of a [`crate::TrackRow`]; needs aliases `t` (track) and `r` (release).
macro_rules! track_row_cols {
    () => {
        "t.id, t.title, t.artist_display, r.title, t.release_id, r.cover_ref, t.disc_no, \
         t.track_no, t.duration_ms, t.active_quality, t.active_codec, t.active_kind, \
         t.has_local, \
         EXISTS(SELECT 1 FROM likes lk WHERE lk.target_kind = 'track' \
                AND lk.target_id = t.id AND lk.deleted = 0),          (SELECT ta.artist_id FROM track_artist ta WHERE ta.track_id = t.id                 ORDER BY ta.position LIMIT 1),          (SELECT s.locator FROM source s WHERE s.track_id = t.id AND s.is_active = 1                 AND s.kind = 'youtube')"
    };
}

/// Whether an album (alias `r`) counts as liked: every one of its live tracks
/// is liked (the local ones, when it has any); an album without tracks falls
/// back to its own like row.
macro_rules! release_liked_expr {
    () => {
        "(SELECT CASE WHEN COUNT(*) = 0
                     THEN EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'release'
                                     AND l.target_id = r.id AND l.deleted = 0)
                     ELSE COUNT(*) = SUM(EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'track'
                                                    AND l.target_id = t.id AND l.deleted = 0))
                 END
            FROM track t
           WHERE t.release_id = r.id AND t.deleted_at IS NULL AND t.in_library = 1
             AND (t.has_local = 1 OR NOT EXISTS (
                     SELECT 1 FROM track t2 WHERE t2.release_id = r.id
                        AND t2.deleted_at IS NULL AND t2.has_local = 1)))"
    };
}

macro_rules! track_row_from {
    () => {
        "FROM track t LEFT JOIN release r ON r.id = t.release_id"
    };
}
