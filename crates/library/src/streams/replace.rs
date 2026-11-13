//! A stream track replaced by hand with the song the user picked («Найти
//! песню…»): a liked clip that has no song found by
//! itself, a wrong YouTube match of another stream track.
//!
//! [`Library::replace_with_song`] makes the track the picked song in place
//! (no merge, so it can be undone): its title, artists, album, cover and
//! length are the song's, its YouTube sources (and Deezer's, for a Deezer
//! pick) give way to the song's, and the ids of the services that are the
//! identity of a remote like stay linked to it, so an unlike here still
//! goes to what was liked there. Lyrics and analysis of the old audio go.
//! The [`TrackSnapshot`] it returns puts every row it touched back
//! ([`Library::restore_track`]).

use nikgob_core::model::{Source, TrackId};
use nikgob_core::streaming::{EntityKind, Service, StreamTrack, TrackKind};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use super::{exists, has_local_source, refresh_in_library, set_external, stream_score, write_stream_meta};
use crate::catalog::{add_source_in, refresh_active};
use crate::db::Library;
use crate::error::{LibraryError, Result};

/// Rows of one table, as they were.
#[derive(Clone, Debug, PartialEq)]
struct Rows {
    table: &'static str,
    /// What selected them (and deletes their successors on a restore).
    filter: String,
    args: Vec<String>,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

/// What [`Library::replace_with_song`] changed, to put back.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackSnapshot {
    pub track: TrackId,
    /// The `track` row (every column but `seq` and `id`).
    columns: Vec<String>,
    values: Vec<Value>,
    rows: Vec<Rows>,
    /// The release the replacement gave the track, dropped on a restore
    /// when nothing else is on it.
    made_release: Option<String>,
    /// Other tracks whose rows were touched (a preview's «seen» track that
    /// gave its source up).
    others: Vec<String>,
}

fn capture(c: &Connection, table: &'static str, filter: &str, args: Vec<String>) -> Result<Rows> {
    let mut stmt = c.prepare_cached(&format!("SELECT * FROM {table} WHERE {filter}"))?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| (*s).to_owned()).collect();
    let n = columns.len();
    let rows = stmt
        .query_map(params_from_iter(args.iter()), |r| (0..n).map(|i| r.get::<_, Value>(i)).collect::<rusqlite::Result<Vec<_>>>())?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Rows {
        table,
        filter: filter.to_owned(),
        args,
        columns,
        rows,
    })
}

/// The tables keyed by a track that a replacement changes.
const TRACK_TABLES: &[(&str, &str)] = &[
    ("track_artist", "track_id = ?1"),
    ("source", "track_id = ?1"),
    ("external_id", "kind = 'track' AND local_id = ?1"),
    ("remote_like", "track_id = ?1"),
    ("lyrics", "track_id = ?1"),
    ("lyrics_text", "track_id = ?1"),
    ("lyrics_pick", "track_id = ?1"),
    ("analysis", "track_id = ?1"),
    ("sound_embedding", "track_id = ?1"),
];

impl Library {
    /// Makes the stream track `track` the song `songs[0]` (see the module
    /// docs); the other `songs` are more sources of it (the YouTube song a
    /// Deezer pick plays from). A track with a file is refused.
    pub fn replace_with_song(&self, track: TrackId, songs: &[StreamTrack]) -> Result<TrackSnapshot> {
        let Some(meta) = songs.first() else {
            return Err(LibraryError::Invalid("no song to replace with".into()));
        };
        self.write(|tx| {
            let tid = track.to_string();
            if !exists(tx, "track", &tid)? {
                return Err(LibraryError::not_found("track", tid));
            }
            if has_local_source(tx, &tid)? {
                return Err(LibraryError::Invalid(format!("{tid} has a file")));
            }
            let snap = snapshot_in(tx, track, songs)?;
            let deezer = songs.iter().any(|s| s.id.service == Service::Deezer);
            let mut gone = vec![Service::Youtube.source_kind().as_str()];
            if deezer {
                gone.push(Service::Deezer.source_kind().as_str());
            }
            for kind in &gone {
                tx.execute("DELETE FROM source WHERE track_id = ?1 AND kind = ?2", [&tid, *kind])?;
            }
            // Links of the old audio go, but a remote like's identity stays.
            for service in [Service::Youtube, Service::Deezer] {
                if service == Service::Deezer && !deezer {
                    continue;
                }
                tx.execute(
                    "DELETE FROM external_id
                      WHERE kind = 'track' AND local_id = ?1 AND service = ?2
                        AND remote_id NOT IN (SELECT remote_id FROM remote_like
                                               WHERE service = ?2 AND track_id = ?1)",
                    [&tid, service.as_str()],
                )?;
            }
            for table in ["lyrics", "lyrics_text", "lyrics_pick", "analysis", "sound_embedding"] {
                tx.execute(&format!("DELETE FROM {table} WHERE track_id = ?1"), [&tid])?;
            }
            let mut meta = meta.clone();
            meta.kind = TrackKind::Song;
            write_stream_meta(tx, &meta, Some(track), false)?;
            for song in songs {
                let service = song.id.service;
                let mut source = Source::new(track, service.source_kind(), song.id.id.clone(), service.codec());
                source.bitrate = Some(service.bitrate_kbps());
                let mut as_song = song.clone();
                as_song.kind = TrackKind::Song;
                source.quality_score = stream_score(&as_song);
                source.available = song.playable;
                add_source_in(tx, &source)?;
                set_external(tx, service, EntityKind::Track, &song.id.id, &tid)?;
            }
            refresh_active(tx, track)?;
            refresh_in_library(tx, &tid)?;
            for other in &snap.others {
                refresh_active(tx, other.parse().map_err(|_| LibraryError::Invalid(other.clone()))?)?;
                refresh_in_library(tx, other)?;
            }
            let made: Option<String> = tx.query_row("SELECT release_id FROM track WHERE id = ?1", [&tid], |r| r.get(0))?;
            let before = snap
                .columns
                .iter()
                .position(|c| c == "release_id")
                .and_then(|i| match &snap.values[i] {
                    Value::Text(t) => Some(t.clone()),
                    _ => None,
                });
            Ok(TrackSnapshot {
                made_release: made.filter(|m| Some(m) != before.as_ref()),
                ..snap
            })
        })
    }

    /// Puts back what a [`Self::replace_with_song`] changed.
    pub fn restore_track(&self, snap: &TrackSnapshot) -> Result<()> {
        self.write(|tx| {
            let tid = snap.track.to_string();
            if !exists(tx, "track", &tid)? {
                return Err(LibraryError::not_found("track", tid));
            }
            let sets: Vec<String> = snap.columns.iter().enumerate().map(|(i, c)| format!("{c} = ?{}", i + 2)).collect();
            let mut args: Vec<Value> = vec![Value::Text(tid.clone())];
            args.extend(snap.values.iter().cloned());
            tx.execute(&format!("UPDATE track SET {} WHERE id = ?1", sets.join(", ")), params_from_iter(args.iter()))?;
            for r in &snap.rows {
                tx.execute(&format!("DELETE FROM {} WHERE {}", r.table, r.filter), params_from_iter(r.args.iter()))?;
            }
            for r in &snap.rows {
                let cols = r.columns.join(", ");
                let marks: Vec<String> = (1..=r.columns.len()).map(|i| format!("?{i}")).collect();
                let active = r.columns.iter().position(|c| c == "is_active").filter(|_| r.table == "source");
                for row in &r.rows {
                    let mut row = row.clone();
                    // One active source per track: chosen again below.
                    if let Some(i) = active {
                        row[i] = Value::Integer(0);
                    }
                    tx.execute(
                        &format!("INSERT OR REPLACE INTO {} ({cols}) VALUES ({})", r.table, marks.join(", ")),
                        params_from_iter(row.iter()),
                    )?;
                }
            }
            for t in std::iter::once(&tid).chain(&snap.others) {
                if exists(tx, "track", t)? {
                    refresh_active(tx, t.parse().map_err(|_| LibraryError::Invalid(t.clone()))?)?;
                    refresh_in_library(tx, t)?;
                }
            }
            if let Some(rel) = &snap.made_release {
                let empty: bool = tx.query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM track WHERE release_id = ?1)",
                    [rel],
                    |r| r.get(0),
                )?;
                if empty {
                    tx.execute("DELETE FROM release_artist WHERE release_id = ?1", [rel])?;
                    tx.execute("DELETE FROM release WHERE id = ?1", [rel])?;
                }
            }
            Ok(())
        })
    }
}

/// The rows of `track` and of the ids of `songs` (their sources and links
/// on other tracks too) as they are now.
fn snapshot_in(tx: &Connection, track: TrackId, songs: &[StreamTrack]) -> Result<TrackSnapshot> {
    let tid = track.to_string();
    let row = capture(tx, "track", "id = ?1", vec![tid.clone()])?;
    let Some(values) = row.rows.into_iter().next() else {
        return Err(LibraryError::not_found("track", tid));
    };
    let (columns, values): (Vec<String>, Vec<Value>) = row
        .columns
        .into_iter()
        .zip(values)
        .filter(|(c, _)| c != "seq" && c != "id")
        .unzip();
    let mut rows = Vec::new();
    for (table, filter) in TRACK_TABLES {
        rows.push(capture(tx, table, filter, vec![tid.clone()])?);
    }
    let mut others = Vec::new();
    for s in songs {
        let kind = s.id.service.source_kind().as_str().to_owned();
        let src = capture(tx, "source", "kind = ?1 AND locator = ?2 AND track_id != ?3", vec![kind, s.id.id.clone(), tid.clone()])?;
        let link = capture(
            tx,
            "external_id",
            "service = ?1 AND kind = 'track' AND remote_id = ?2 AND local_id != ?3",
            vec![s.id.service.as_str().to_owned(), s.id.id.clone(), tid.clone()],
        )?;
        let track_col = src.columns.iter().position(|c| c == "track_id");
        for r in &src.rows {
            if let Some(Value::Text(t)) = track_col.map(|i| &r[i])
                && !others.contains(t)
            {
                others.push(t.clone());
            }
        }
        rows.push(src);
        rows.push(link);
    }
    Ok(TrackSnapshot {
        track,
        columns,
        values,
        rows,
        made_release: None,
        others,
    })
}
