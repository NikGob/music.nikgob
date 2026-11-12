//! SQL fragments shared by several modules (macros so they can be `concat!`ed).

/// `log::info!` of a change to the catalog; silent while a one-time repair
/// only counts what it would change ([`crate::repairs::counting`]).
macro_rules! note {
    ($($arg:tt)*) => {
        if !crate::repairs::counting() {
            log::info!($($arg)*);
        }
    };
}

/// The 18 columns of a [`crate::TrackRow`]; needs aliases `t` (track) and `r` (release).
macro_rules! track_row_cols {
    () => {
        "t.id, t.title, t.artist_display, r.title, t.release_id, r.cover_ref, t.disc_no, \
         t.track_no, t.duration_ms, t.active_quality, t.active_codec, t.active_kind, \
         t.has_local, \
         EXISTS(SELECT 1 FROM likes lk WHERE lk.target_kind = 'track' \
                AND lk.target_id = t.id AND lk.deleted = 0),          t.artist_credit,          (SELECT s.locator FROM source s WHERE s.track_id = t.id AND s.is_active = 1                 AND s.kind = 'youtube'),          (SELECT q.verdict || '|' || q.origin || '|' || coalesce(q.made_from, '') || '|'                    || CAST(q.cutoff_hz AS INTEGER)             FROM source s JOIN source_quality q ON q.source_id = s.id            WHERE s.track_id = t.id AND s.is_active = 1 AND q.failed IS NULL),          t.in_library"
    };
}

/// A row of an album's page (alias `t`): live, and not a stream track
/// outside the library at the place of a file as long within 2 s (that
/// file's song under another title, see `streams::same`).
macro_rules! page_row {
    () => {
        "t.deleted_at IS NULL
         AND NOT (t.has_local = 0 AND t.in_library = 0 AND t.track_no > 0 AND EXISTS (
             SELECT 1 FROM track o WHERE o.release_id = t.release_id AND o.id != t.id
                AND o.deleted_at IS NULL AND o.has_local = 1
                AND o.disc_no = t.disc_no AND o.track_no = t.track_no
                AND abs(o.duration_ms - t.duration_ms) <= 2000))"
    };
}

/// A row of an album's page (alias `t`) the user left out of their songs
/// of that album by hand ([`crate::selection`]).
macro_rules! excluded_expr {
    () => {
        "EXISTS (SELECT 1 FROM release_track_selection x WHERE x.release_id = t.release_id
                    AND x.track_id = t.id AND x.choice = 'exclude' AND x.deleted = 0)"
    };
}

/// The whole of an album (alias `r`) is the user's: added whole
/// ([`crate::Library::add_release_whole`]) or saved (its own like row; a
/// heart only worked out from its tracks' likes does not count, nor a like
/// that came with a like of one of its songs or after which the user kept
/// only their songs: mode `mine`).
macro_rules! release_whole_expr {
    () => {
        "(EXISTS (SELECT 1 FROM release_selection s WHERE s.release_id = r.id AND s.mode = 'whole' AND s.deleted = 0)
          OR (EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'release' AND l.target_id = r.id AND l.deleted = 0)
              AND NOT EXISTS (SELECT 1 FROM release_selection s WHERE s.release_id = r.id AND s.mode = 'mine'
                                 AND s.deleted = 0)))"
    };
}

/// Whether an album (alias `r`) counts as liked: it is saved (its own like
/// row); when album likes like the tracks too
/// ([`crate::Library::set_album_likes_tracks`]), every one of its live tracks
/// is liked instead (the local ones, when it has any), and an album without
/// tracks falls back to its own like row.
macro_rules! release_liked_expr {
    () => {
        "(SELECT CASE WHEN NOT album_likes_tracks()
                     THEN EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'release'
                                     AND l.target_id = r.id AND l.deleted = 0)
                     WHEN COUNT(*) = 0
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
