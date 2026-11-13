//! What the bento's tiles read beyond the recommendations' tracks, each in
//! one pass: per track its duration, when
//! it was added, its release's year and title, its place on the album,
//! how much lyrics it has and whether it has a file; and the user's
//! playlists with their tracks.

use nikgob_core::model::{PlaylistId, TrackId, UnixMs};

use crate::db::Library;
use crate::error::Result;
use crate::util::get_id;

/// How much lyrics a track has: none, plain, synced by lines, by words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsLevel {
    #[default]
    None,
    Plain,
    Lines,
    Words,
}

/// A track as the tiles see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackFactsRow {
    pub id: TrackId,
    pub duration_ms: Option<u32>,
    pub added_at: UnixMs,
    pub year: Option<i32>,
    pub album: Option<String>,
    pub disc_no: u32,
    pub track_no: u32,
    pub lyrics: LyricsLevel,
    pub has_local: bool,
}

/// The start of a track's lyrics, to tell the language they are sung in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsSample {
    pub id: TrackId,
    /// Marked instrumental.
    pub instrumental: bool,
    /// The start of the text as kept (LRC, plain, or an online source's
    /// raw body: JSON lines or TTML); empty without any.
    pub text: String,
}

/// A live playlist of the user's with its live tracks, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistTracks {
    pub id: PlaylistId,
    pub name: String,
    pub tracks: Vec<TrackId>,
}

impl Library {
    /// Every track not deleted, with what the tiles read of it. Lyrics by
    /// words are any online source's lines with word timing; synced, LRC
    /// or any online source's lines; an instrumental mark is none.
    pub fn track_facts(&self) -> Result<Vec<TrackFactsRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT t.id, t.duration_ms, t.added_at, r.year, r.title, t.disc_no, t.track_no, t.has_local,
                        COALESCE(l.instrumental, 0),
                        COALESCE(l.synced, '') != '',
                        COALESCE(l.plain, '') != '',
                        EXISTS (SELECT 1 FROM lyrics_text x WHERE x.track_id = t.id
                                  AND x.body LIKE '%\"words\":[{%'),
                        EXISTS (SELECT 1 FROM lyrics_text x WHERE x.track_id = t.id
                                  AND x.body IS NOT NULL AND x.body != '[]')
                   FROM track t
                   LEFT JOIN release r ON r.id = t.release_id
                   LEFT JOIN lyrics l ON l.track_id = t.id
                  WHERE t.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                let instrumental: bool = r.get(8)?;
                let (synced, plain, words, lines): (bool, bool, bool, bool) = (r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?);
                let lyrics = if instrumental {
                    LyricsLevel::None
                } else if words {
                    LyricsLevel::Words
                } else if synced || lines {
                    LyricsLevel::Lines
                } else if plain {
                    LyricsLevel::Plain
                } else {
                    LyricsLevel::None
                };
                Ok(TrackFactsRow {
                    id: get_id(r, 0)?,
                    duration_ms: r.get::<_, Option<i64>>(1)?.map(|d| d.clamp(0, i64::from(u32::MAX)) as u32),
                    added_at: r.get(2)?,
                    year: r.get(3)?,
                    album: r.get(4)?,
                    disc_no: r.get(5)?,
                    track_no: r.get(6)?,
                    lyrics,
                    has_local: r.get(7)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every track with a lyrics row: the instrumental mark and the start
    /// of the text (the language of «Миксы для тебя»).
    pub fn lyrics_samples(&self) -> Result<Vec<LyricsSample>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT t.id, COALESCE(l.instrumental, 0),
                        COALESCE(substr(NULLIF(l.synced, ''), 1, 1500), substr(NULLIF(l.plain, ''), 1, 1500),
                                 (SELECT substr(x.body, 1, 6000) FROM lyrics_text x
                                   WHERE x.track_id = t.id AND x.body IS NOT NULL AND x.body != '[]' LIMIT 1), '')
                   FROM track t
                   LEFT JOIN lyrics l ON l.track_id = t.id
                  WHERE t.deleted_at IS NULL
                    AND (l.track_id IS NOT NULL OR EXISTS (SELECT 1 FROM lyrics_text x WHERE x.track_id = t.id))",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(LyricsSample {
                    id: get_id(r, 0)?,
                    instrumental: r.get(1)?,
                    text: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The user's live playlists with their live tracks, in playlist order.
    pub fn playlist_tracks(&self) -> Result<Vec<PlaylistTracks>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT p.id, p.name, e.track_id FROM playlist p
                   JOIN playlist_entry e ON e.playlist_id = p.id AND e.deleted = 0
                  WHERE p.deleted = 0
                  ORDER BY p.seq, e.position, e.id",
            )?;
            let rows = stmt.query_map([], |r| Ok((get_id::<PlaylistId>(r, 0)?, r.get::<_, String>(1)?, get_id::<TrackId>(r, 2)?)))?;
            let mut out: Vec<PlaylistTracks> = Vec::new();
            for row in rows {
                let (id, name, track) = row?;
                match out.last_mut() {
                    Some(p) if p.id == id => p.tracks.push(track),
                    _ => out.push(PlaylistTracks { id, name, tracks: vec![track] }),
                }
            }
            Ok(out)
        })
    }
}
