//! Songs the user deleted from an album are «не нужны» (table `unwanted`,
//! migration 0019): the deletion is a strong «not for me» for that song,
//! never for its artist or the rest of its album.
//!
//! * [`note_unwanted_in`]: a track deleted into the Trash (or purged at
//!   once) leaves a row keyed by its album and title, kept after the track
//!   is purged; restoring the track or liking the song takes it away
//!   ([`Library::forget_unwanted_song`]).
//! * A live track of the album that is the song (in the library, or with a
//!   file) outweighs the row: the song is wanted after all.
//! * What reads it: the album's count and holes ([`unwanted_places_in`],
//!   the page and «Скачано N из M» of «Хочу скачать»), the tracklist of a
//!   service ([`unwanted_stream_in`]: no new line for the song), a download
//!   of the album ([`Library::unwanted_of_release`]: the importer leaves
//!   its file out), the recommendations ([`Library::unwanted_songs`]), and
//!   whoever looks for a better copy ([`Library::is_track_unwanted`]).

use nikgob_core::model::{ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::StreamTrack;
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::streams::{SLACK_MS, normal_isrc, song_key};
use crate::util::now_ms;

/// One song deleted from an album.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unwanted {
    /// `None` for a song without an album.
    pub release_id: Option<ReleaseId>,
    pub title: String,
    /// The credit as it was displayed.
    pub artist: String,
    pub disc_no: u32,
    /// 0 = it had no place.
    pub track_no: u32,
    pub duration_ms: Option<u32>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    /// The deleted track while it is in the Trash.
    pub track_id: Option<TrackId>,
    pub at: UnixMs,
}

impl Unwanted {
    /// Whether a song (its title, length, ISRC and place on the album) is
    /// this one: the same ISRC or recording, or the same title (folded as
    /// the catalog folds titles) with a length within the slack, or the
    /// same place with such a length.
    pub fn is(&self, title: &str, duration_ms: Option<u32>, isrc: Option<&str>, place: (u32, u32)) -> bool {
        if let (Some(a), Some(b)) = (normal_isrc(self.isrc.as_deref()), normal_isrc(isrc))
            && a == b
        {
            return true;
        }
        let close = match (self.duration_ms, duration_ms) {
            (Some(a), Some(b)) => (i64::from(a) - i64::from(b)).abs() <= SLACK_MS,
            _ => true,
        };
        if !close {
            return false;
        }
        let key = song_key(title);
        let same_title = !key.is_empty() && key == song_key(&self.title);
        let same_place = self.track_no > 0 && place == (self.disc_no.max(1), self.track_no) && duration_ms.is_some() && self.duration_ms.is_some();
        same_title || same_place
    }
}

const COLS: &str = "release_id, title, artist, disc_no, track_no, duration_ms, isrc, recording_mbid, track_id, at";

/// A live track of the album is the song (in the library, or with a file):
/// the row does not count. Alias `u`.
const OUTWEIGHED: &str = "EXISTS (SELECT 1 FROM track o WHERE o.release_id = u.release_id AND o.title_key = u.title_key
                                  AND o.deleted_at IS NULL AND (o.in_library = 1 OR o.has_local = 1))";

fn unwanted_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Unwanted> {
    let release: String = r.get(0)?;
    let track: Option<String> = r.get(8)?;
    Ok(Unwanted {
        release_id: release.parse().ok(),
        title: r.get(1)?,
        artist: r.get(2)?,
        disc_no: r.get::<_, i64>(3)?.max(1) as u32,
        track_no: r.get::<_, i64>(4)?.max(0) as u32,
        duration_ms: r.get::<_, Option<i64>>(5)?.and_then(|d| u32::try_from(d).ok()),
        isrc: r.get(6)?,
        recording_mbid: r.get(7)?,
        track_id: track.and_then(|t| t.parse().ok()),
        at: r.get(9)?,
    })
}

/// The track `track` was deleted at `at`: its song is «не нужна» on its
/// album (a row of the same song is replaced).
pub(crate) fn note_unwanted_in(conn: &Connection, track: &str, at: UnixMs) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO unwanted(release_id, title_key, title, artist, disc_no, track_no,
                                         duration_ms, isrc, recording_mbid, track_id, at)
         SELECT coalesce(release_id, ''), title_key, title, artist_display, max(disc_no, 1), track_no,
                duration_ms, isrc, recording_mbid, id, ?2
           FROM track WHERE id = ?1",
        params![track, at],
    )?;
    Ok(())
}

/// The track is purged: its row stays, without the track.
pub(crate) fn unlink_unwanted_in(conn: &Connection, track: &str) -> Result<()> {
    conn.execute("UPDATE unwanted SET track_id = NULL WHERE track_id = ?1", [track])?;
    Ok(())
}

/// The song of `track` is wanted again (the track came back from the
/// Trash, or the user liked it): its row goes. Returns whether one went.
pub(crate) fn forget_unwanted_in(conn: &Connection, track: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM unwanted
          WHERE track_id = ?1
             OR (release_id, title_key) = (SELECT coalesce(release_id, ''), title_key FROM track WHERE id = ?1)",
        [track],
    )?;
    Ok(n > 0)
}

/// The rows of `release` no live track outweighs.
fn of_release_in(conn: &Connection, release: &str) -> Result<Vec<Unwanted>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {COLS} FROM unwanted u WHERE u.release_id = ?1 AND NOT {OUTWEIGHED} ORDER BY u.disc_no, u.track_no, u.title_key"
    ))?;
    let rows = stmt.query_map([release], unwanted_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The places on `release` whose songs are «не нужны» and that no live
/// track takes: not holes of the album, not counted in its tracks.
pub(crate) fn unwanted_places_in(conn: &Connection, release: &str) -> Result<Vec<(u32, u32)>> {
    Ok(of_release_in(conn, release)?
        .into_iter()
        .filter(|u| u.track_no > 0)
        .map(|u| (u.disc_no.max(1), u.track_no))
        .collect())
}

/// Whether the stream track `t` of the album `release` (its service's
/// tracklist) is a song deleted from it.
pub(crate) fn unwanted_stream_in(conn: &Connection, release: &str, t: &StreamTrack, place: (u32, u32)) -> Result<bool> {
    let duration = t.duration_ms.and_then(|d| u32::try_from(d).ok());
    Ok(of_release_in(conn, release)?.iter().any(|u| u.is(&t.title, duration, t.isrc.as_deref(), place)))
}

impl Library {
    /// The songs deleted from `release` that no live track of it is (see
    /// the module docs).
    pub fn unwanted_of_release(&self, release: ReleaseId) -> Result<Vec<Unwanted>> {
        self.read(|c| of_release_in(c, &release.to_string()))
    }

    /// Every song deleted from an album (or a single) that no live track of
    /// its album is: the recommendations leave them out.
    pub fn unwanted_songs(&self) -> Result<Vec<Unwanted>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!("SELECT {COLS} FROM unwanted u WHERE NOT {OUTWEIGHED} ORDER BY u.at DESC"))?;
            let rows = stmt.query_map([], unwanted_from_row)?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Whether the user does not want `track`: it is deleted (in the Trash),
    /// or it is a line of an album outside the library, without a file, of
    /// a song the user deleted from that album. A search for a better copy
    /// skips such a track.
    pub fn is_track_unwanted(&self, track: TrackId) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT t.deleted_at IS NOT NULL
                        OR (t.in_library = 0 AND t.has_local = 0 AND EXISTS (
                            SELECT 1 FROM unwanted u WHERE u.release_id = coalesce(t.release_id, '')
                               AND u.title_key = t.title_key))
                   FROM track t WHERE t.id = ?1",
                [track.to_string()],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
        })
    }

    /// The song of `track` is wanted again (a like, a restore): its row
    /// goes. Returns whether there was one.
    pub fn forget_unwanted_song(&self, track: TrackId) -> Result<bool> {
        self.write(|tx| forget_unwanted_in(tx, &track.to_string()))
    }

    /// Marks the song of `track` «не нужна» on its album now (the track is
    /// deleted another way than [`Self::mark_track_deleted`]).
    pub fn note_unwanted(&self, track: TrackId) -> Result<()> {
        self.write(|tx| note_unwanted_in(tx, &track.to_string(), now_ms()))
    }
}


#[cfg(test)]
mod tests {
    use nikgob_core::model::{ArtistCredit, Codec, Release, Source, SourceKind, Track};
    use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamAlbum, StreamTrack};

    use super::*;

    const SONGS: [(&str, u32); 3] = [("Пошлая Молли", 188_000), ("Ханнамонтана", 215_000), ("Тмстс", 160_000)];

    /// An album of three songs with files.
    fn album(lib: &Library) -> (ReleaseId, Vec<TrackId>) {
        let credit = ArtistCredit::single("Пошлая Молли", None);
        let mut r = Release::new("Альбом", credit.clone());
        r.total_tracks = Some(3);
        let release = lib.upsert_release(&r).unwrap().id;
        let tracks = SONGS
            .iter()
            .enumerate()
            .map(|(i, (title, ms))| {
                let mut t = Track::new(*title, credit.clone());
                t.release_id = Some(release);
                t.track_no = i as u32 + 1;
                t.duration_ms = Some(*ms);
                let t = lib.upsert_track(&t).unwrap();
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("M/a/{title}.opus"), Codec::Opus)).unwrap();
                t.id
            })
            .collect();
        (release, tracks)
    }

    /// The album in a service, every song of it.
    fn in_service() -> StreamAlbum {
        let artists = vec![ArtistRef::new(None, "Пошлая Молли")];
        StreamAlbum {
            id: RemoteId::new(Service::Youtube, "MPREb_album"),
            title: "Альбом".into(),
            artists: artists.clone(),
            year: None,
            release_date: None,
            release_type: Default::default(),
            cover_url: None,
            total_tracks: Some(3),
            tracks: SONGS
                .iter()
                .enumerate()
                .map(|(i, (title, ms))| {
                    let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, format!("v{i}")), *title);
                    t.artists = artists.clone();
                    t.track_no = Some(i as u32 + 1);
                    t.duration_ms = Some(u64::from(*ms) + 500);
                    t
                })
                .collect(),
        }
    }

    fn counts(lib: &Library, release: ReleaseId) -> (u32, u32, usize) {
        let l = lib.release_listing(release).unwrap().unwrap();
        (l.progress.local, l.progress.total, l.holes())
    }

    #[test]
    fn a_song_deleted_from_an_album_is_no_hole_of_it_and_never_comes_back() {
        let lib = Library::open_in_memory().unwrap();
        let (release, tracks) = album(&lib);
        assert_eq!(counts(&lib, release), (3, 3, 0));

        lib.mark_track_deleted(tracks[1], 1).unwrap();
        assert!(lib.is_track_unwanted(tracks[1]).unwrap());
        assert!(!lib.is_track_unwanted(tracks[0]).unwrap());
        assert_eq!(counts(&lib, release), (2, 2, 0), "«не нужна»: not missing");
        let row = lib.releases(0, 10, crate::ReleaseSort::asc(Default::default())).unwrap();
        assert_eq!(row[0].progress().total, 2, "the album's card counts the same");
        let gone = lib.unwanted_of_release(release).unwrap();
        assert_eq!((gone.len(), gone[0].title.as_str(), gone[0].track_no), (1, "Ханнамонтана", 2));
        assert!(gone[0].is("ханнамонтана", Some(215_900), None, (1, 7)), "its file, whatever its number");
        assert!(!gone[0].is("Ханнамонтана", Some(250_000), None, (1, 2)), "another length is another recording");

        // Gone for good: the tracklist of the service does not bring it back.
        lib.purge_track(tracks[1]).unwrap();
        lib.adopt_stream_album(release, &in_service()).unwrap();
        let titles: Vec<String> = lib.tracks_of_release_all(release).unwrap().into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["Пошлая Молли", "Тмстс"]);
        assert_eq!(counts(&lib, release), (2, 2, 0));
        assert_eq!(lib.unwanted_songs().unwrap().len(), 1, "the recommendations leave it out");
    }

    #[test]
    fn a_restored_or_liked_song_is_wanted_again() {
        let lib = Library::open_in_memory().unwrap();
        let (release, tracks) = album(&lib);
        lib.mark_track_deleted(tracks[2], 1).unwrap();
        lib.undelete_track(tracks[2]).unwrap();
        assert!(lib.unwanted_of_release(release).unwrap().is_empty(), "restored");
        assert_eq!(counts(&lib, release), (3, 3, 0));

        lib.mark_track_deleted(tracks[0], 1).unwrap();
        assert_eq!(counts(&lib, release), (2, 2, 0));
        assert!(lib.forget_unwanted_song(tracks[0]).unwrap(), "liked on purpose");
        assert_eq!(counts(&lib, release), (2, 3, 1), "a hole again: it may be downloaded");
    }

    #[test]
    fn a_track_deleted_at_once_is_unwanted_too() {
        let lib = Library::open_in_memory().unwrap();
        let (release, tracks) = album(&lib);
        lib.purge_track(tracks[2]).unwrap();
        let gone = lib.unwanted_of_release(release).unwrap();
        assert_eq!((gone.len(), gone[0].track_id), (1, None));
        assert_eq!(counts(&lib, release), (2, 2, 0));
    }
}
