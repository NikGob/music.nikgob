//! Full-text search over tracks, releases, artists and playlists (FTS5).
//!
//! Behaviour (all covered by tests):
//! * case-insensitive for every script (`Кино` finds `кино`);
//! * `ё` and `е` are interchangeable in both the index and the query
//!   (`звёзд` finds `Звезда`, `звезд` finds `Звёзды`); the index text is folded
//!   in SQL by the triggers, the query is folded here;
//! * Latin diacritics are removed (`beyonce` finds `Beyoncé`) by the tokenizer
//!   (`unicode61 remove_diacritics 2`);
//! * every query word is a prefix match and all words must match
//!   (`pink fl` finds `Pink Floyd`); punctuation in the query is ignored, so FTS
//!   syntax characters can never cause an error.

use rusqlite::params;

use crate::catalog::{
    ARTIST_ROW_SELECT, RELEASE_ROW_SELECT, TRACK_ROW_SELECT, artist_row, release_row, track_row,
};
use crate::db::Library;
use crate::error::Result;
use crate::playlists::{PLAYLIST_SUMMARY_SELECT, playlist_summary};
use crate::types::SearchResults;
use crate::util::fold;

/// Most words of a query that are used.
const MAX_TERMS: usize = 8;

/// Turns user input into an FTS5 MATCH expression: `"звезд"* "кино"*`.
/// `None` when the input has no searchable words.
pub fn match_expression(query: &str) -> Option<String> {
    let terms: Vec<String> = fold(query)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(MAX_TERMS)
        .map(|t| format!("\"{t}\"*"))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

impl Library {
    /// Searches everything; each category returns at most `limit` best matches.
    pub fn search(&self, query: &str, limit: u32) -> Result<SearchResults> {
        let Some(expr) = match_expression(query) else {
            return Ok(SearchResults::default());
        };
        self.read(|c| {
            let tracks = {
                let mut stmt = c.prepare_cached(&format!(
                    "{TRACK_ROW_SELECT} JOIN track_fts ON track_fts.rowid = t.seq
                      WHERE track_fts MATCH ?1 AND t.deleted_at IS NULL AND t.in_library = 1
                      ORDER BY bm25(track_fts, 8.0, 4.0, 2.0), t.has_local DESC, t.seq LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![expr, limit], track_row)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let releases = {
                let mut stmt = c.prepare_cached(&format!(
                    "{RELEASE_ROW_SELECT} JOIN release_fts ON release_fts.rowid = r.seq
                      WHERE release_fts MATCH ?1
                      ORDER BY bm25(release_fts, 8.0, 4.0), r.seq LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![expr, limit], release_row)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let artists = {
                let mut stmt = c.prepare_cached(&format!(
                    "{ARTIST_ROW_SELECT} JOIN artist_fts ON artist_fts.rowid = a.seq
                      WHERE artist_fts MATCH ?1
                      ORDER BY bm25(artist_fts, 8.0, 2.0), a.seq LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![expr, limit], artist_row)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let playlists = {
                let mut stmt = c.prepare_cached(&format!(
                    "{PLAYLIST_SUMMARY_SELECT} JOIN playlist_fts ON playlist_fts.rowid = p.seq
                      WHERE playlist_fts MATCH ?1 AND p.deleted = 0
                      ORDER BY bm25(playlist_fts), p.seq LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![expr, limit], playlist_summary)?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(SearchResults {
                tracks,
                releases,
                artists,
                playlists,
            })
        })
    }

    /// Rebuilds all FTS tables from the base tables (repair after manual edits).
    pub fn rebuild_search_index(&self) -> Result<()> {
        self.write(|tx| {
            tx.execute_batch(
                "DELETE FROM track_fts; DELETE FROM release_fts;
                 DELETE FROM artist_fts; DELETE FROM playlist_fts;
                 INSERT INTO track_fts(rowid, title, artist, album)
                   SELECT t.seq,
                          replace(replace(t.title, 'ё', 'е'), 'Ё', 'Е'),
                          replace(replace(t.artist_display, 'ё', 'е'), 'Ё', 'Е'),
                          replace(replace(coalesce(r.title, ''), 'ё', 'е'), 'Ё', 'Е')
                     FROM track t LEFT JOIN release r ON r.id = t.release_id;
                 INSERT INTO release_fts(rowid, title, artist)
                   SELECT seq, replace(replace(title, 'ё', 'е'), 'Ё', 'Е'),
                          replace(replace(artist_display, 'ё', 'е'), 'Ё', 'Е')
                     FROM release;
                 INSERT INTO artist_fts(rowid, name, alt)
                   SELECT seq, replace(replace(name, 'ё', 'е'), 'Ё', 'Е'),
                          replace(replace(sort_name, 'ё', 'е'), 'Ё', 'Е')
                     FROM artist;
                 INSERT INTO playlist_fts(rowid, name)
                   SELECT seq, replace(replace(name, 'ё', 'е'), 'Ё', 'Е')
                     FROM playlist WHERE deleted = 0;",
            )?;
            Ok(())
        })
    }
}
