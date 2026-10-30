//! The graph of similar music (docs/recs-graph.md, migration 0016): what a
//! source said about a library track or artist — its list of similar songs
//! or artists as the source named them, an artist's tags — and when it was
//! asked. The runtime resolves the names to the library at every build.

use rusqlite::params;

use crate::db::Library;
use crate::error::Result;

/// One entry of a source's list.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphEntry {
    /// `track` or `artist`.
    pub kind: String,
    pub name: String,
    /// The entry's artist (a track's), empty for an artist.
    pub artist: String,
    /// What the source calls it: a service's id, a MusicBrainz id.
    pub remote_id: Option<String>,
}

/// One kept entry with what it belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphRow {
    pub subject_kind: String,
    pub subject_id: String,
    pub source: String,
    /// Its place in the list (0 first) of `total`.
    pub rank: u32,
    pub total: u32,
    pub entry: GraphEntry,
}

/// What was asked of a source about a subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphFetch {
    pub subject_kind: String,
    pub subject_id: String,
    pub source: String,
    pub fetched_at: i64,
    /// `ok`, `none` or `error`.
    pub status: String,
    pub remote_id: Option<String>,
}

/// An artist's tag.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphTag {
    pub subject_kind: String,
    pub subject_id: String,
    pub tag: String,
    pub weight: f32,
}

impl Library {
    /// Keeps what `source` said about a subject at `at`: its list (in
    /// order) and tags take the place of what it said before; a status of
    /// `error` keeps the old list and tags and only marks the time.
    #[allow(clippy::too_many_arguments)]
    pub fn graph_put(
        &self,
        subject_kind: &str,
        subject_id: &str,
        source: &str,
        status: &str,
        remote_id: Option<&str>,
        entries: &[GraphEntry],
        tags: &[(String, f32)],
        at: i64,
    ) -> Result<()> {
        self.write(|tx| {
            if status != "error" {
                tx.execute(
                    "DELETE FROM graph_edge WHERE subject_kind = ?1 AND subject_id = ?2 AND source = ?3",
                    params![subject_kind, subject_id, source],
                )?;
                let mut ins = tx.prepare_cached(
                    "INSERT INTO graph_edge(subject_kind, subject_id, source, rank, total, kind, name, artist, remote_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?;
                for (rank, e) in entries.iter().enumerate() {
                    ins.execute(params![subject_kind, subject_id, source, rank as i64, entries.len() as i64, e.kind, e.name, e.artist, e.remote_id])?;
                }
                if !tags.is_empty() || source == "musicbrainz" {
                    tx.execute("DELETE FROM graph_tag WHERE subject_kind = ?1 AND subject_id = ?2", params![subject_kind, subject_id])?;
                    let mut ins = tx.prepare_cached(
                        "INSERT OR REPLACE INTO graph_tag(subject_kind, subject_id, tag, weight) VALUES (?1, ?2, ?3, ?4)",
                    )?;
                    for (tag, w) in tags {
                        ins.execute(params![subject_kind, subject_id, tag, f64::from(*w)])?;
                    }
                }
            }
            tx.execute(
                "INSERT INTO graph_fetch(subject_kind, subject_id, source, fetched_at, status, remote_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(subject_kind, subject_id, source) DO UPDATE SET fetched_at = excluded.fetched_at,
                    status = excluded.status, remote_id = coalesce(excluded.remote_id, graph_fetch.remote_id)",
                params![subject_kind, subject_id, source, at, status, remote_id],
            )?;
            Ok(())
        })
    }

    /// Everything asked so far.
    pub fn graph_fetches(&self) -> Result<Vec<GraphFetch>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT subject_kind, subject_id, source, fetched_at, status, remote_id FROM graph_fetch")?;
            let rows = stmt.query_map([], |r| {
                Ok(GraphFetch {
                    subject_kind: r.get(0)?,
                    subject_id: r.get(1)?,
                    source: r.get(2)?,
                    fetched_at: r.get(3)?,
                    status: r.get(4)?,
                    remote_id: r.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every kept entry, each list in its order.
    pub fn graph_edges(&self) -> Result<Vec<GraphRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT subject_kind, subject_id, source, rank, total, kind, name, artist, remote_id FROM graph_edge
                  ORDER BY subject_kind, subject_id, source, rank",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(GraphRow {
                    subject_kind: r.get(0)?,
                    subject_id: r.get(1)?,
                    source: r.get(2)?,
                    rank: r.get(3)?,
                    total: r.get(4)?,
                    entry: GraphEntry { kind: r.get(5)?, name: r.get(6)?, artist: r.get(7)?, remote_id: r.get(8)? },
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every kept tag.
    pub fn graph_tags(&self) -> Result<Vec<GraphTag>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT subject_kind, subject_id, tag, weight FROM graph_tag")?;
            let rows = stmt.query_map([], |r| {
                Ok(GraphTag { subject_kind: r.get(0)?, subject_id: r.get(1)?, tag: r.get(2)?, weight: r.get::<_, f64>(3)? as f32 })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The ids the services know the library's tracks and artists by:
    /// `(service, kind, remote id, local id)`.
    pub fn graph_remote_ids(&self) -> Result<Vec<(String, String, String, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT service, kind, remote_id, local_id FROM external_id WHERE kind IN ('track', 'artist')",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The MusicBrainz ids of the library's artists and tracks (recordings)
    /// that have one: `(kind, id, mbid)`.
    pub fn graph_mbids(&self) -> Result<Vec<(String, String, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT 'artist', id, mbid FROM artist WHERE mbid IS NOT NULL AND mbid != ''
                 UNION ALL
                 SELECT 'track', id, recording_mbid FROM track
                  WHERE recording_mbid IS NOT NULL AND recording_mbid != '' AND deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> GraphEntry {
        GraphEntry { kind: "track".into(), name: name.into(), artist: "A".into(), remote_id: Some(format!("yt-{name}")) }
    }

    /// A list takes the place of the one before, an error keeps it, tags
    /// come and go with MusicBrainz.
    #[test]
    fn a_list_is_replaced_and_an_error_keeps_it() {
        let lib = Library::open_in_memory().unwrap();
        lib.graph_put("track", "t1", "youtube_radio", "ok", None, &[entry("a"), entry("b")], &[], 10).unwrap();
        lib.graph_put("track", "t1", "youtube_radio", "ok", Some("vid"), &[entry("c")], &[], 20).unwrap();
        let rows = lib.graph_edges().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].entry.name.as_str(), rows[0].rank, rows[0].total), ("c", 0, 1));
        lib.graph_put("track", "t1", "youtube_radio", "error", None, &[], &[], 30).unwrap();
        assert_eq!(lib.graph_edges().unwrap().len(), 1);
        let f = lib.graph_fetches().unwrap();
        assert_eq!((f[0].fetched_at, f[0].status.as_str(), f[0].remote_id.as_deref()), (30, "error", Some("vid")));
        lib.graph_put("artist", "a1", "musicbrainz", "ok", Some("mbid"), &[], &[("rock".into(), 3.0), ("indie".into(), 1.0)], 40).unwrap();
        assert_eq!(lib.graph_tags().unwrap().len(), 2);
        lib.graph_put("artist", "a1", "musicbrainz", "none", None, &[], &[], 50).unwrap();
        assert!(lib.graph_tags().unwrap().is_empty());
        assert!(lib.graph_mbids().unwrap().is_empty());
    }
}
