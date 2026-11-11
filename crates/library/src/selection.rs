//! What the user chose of an album, apart from what the album is (tables
//! `release_selection`, `release_track_selection`; docs/TODO.md 57).
//!
//! An album's catalog (its page's rows and holes) says what the album is.
//! The user's songs of it («Мои треки») are its rows that are library
//! tracks (`in_library`: a file, a like, a playlist, a song added here) and
//! not left out of this album by hand. Only decisions are stored:
//!
//! * per album: the whole album added to the library («Добавить весь альбом
//!   в библиотеку», `mode = 'whole'`: every track of it is a library track,
//!   the ones that only stream too, nothing downloads), and what its page
//!   shows and «Слушать» plays while only some of it is the user's
//!   (`view`: «Мои треки» / «Весь альбом», `auto` = the setting);
//! * per song of an album: added to the user's songs of it (`include`: a
//!   library track from then on) or left out of them (`exclude`: its file,
//!   like and playlists stay).
//!
//! A reset is a tombstone; sync merges by last writer as likes do
//! ([`crate::userdata`]). Nothing here is guessed from plays, skips,
//! titles or where a file came from.

use nikgob_core::model::{ReleaseId, TrackId, UnixMs};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Library;
use crate::error::Result;
use crate::streams::{refresh_in_library, refresh_in_library_of_release};
use crate::types::ImportReport;

/// The stored choice of an album.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseChoice {
    pub release_id: ReleaseId,
    /// The whole album is in the library.
    pub whole: bool,
    /// What its page shows while only some of it is the user's:
    /// `Some(false)` «Мои треки», `Some(true)` «Весь альбом», `None` the
    /// setting.
    pub view: Option<bool>,
    pub updated_at: UnixMs,
    pub updated_by: String,
    pub deleted: bool,
}

/// The stored choice of one song of an album.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackChoice {
    pub release_id: ReleaseId,
    pub track_id: TrackId,
    /// One of the user's songs of the album (true) or left out of them.
    pub include: bool,
    pub updated_at: UnixMs,
    pub updated_by: String,
    pub deleted: bool,
}

fn view_str(view: Option<bool>) -> &'static str {
    match view {
        Some(true) => "all",
        Some(false) => "mine",
        None => "auto",
    }
}

fn view_of(s: &str) -> Option<bool> {
    match s {
        "all" => Some(true),
        "mine" => Some(false),
        _ => None,
    }
}

/// `(whole, view)` of a live choice of `release`.
fn release_in(c: &Connection, release: &str) -> Result<(bool, Option<bool>)> {
    let row: Option<(String, String)> = c
        .query_row(
            "SELECT mode, view FROM release_selection WHERE release_id = ?1 AND deleted = 0",
            [release],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map_or((false, None), |(mode, view)| (mode == "whole", view_of(&view))))
}

fn put_release_in(c: &Connection, release: &str, whole: bool, view: Option<bool>, now: UnixMs, device: &str) -> Result<()> {
    let reset = !whole && view.is_none();
    c.execute(
        "INSERT INTO release_selection(release_id, mode, view, updated_at, updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(release_id) DO UPDATE SET mode = excluded.mode, view = excluded.view,
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, deleted = excluded.deleted",
        params![release, if whole { "whole" } else { "auto" }, view_str(view), now, device, reset],
    )?;
    Ok(())
}

/// The live choice of a song of an album: `Some(true)` added, `Some(false)`
/// left out.
fn track_in(c: &Connection, release: &str, track: &str) -> Result<Option<bool>> {
    let choice: Option<String> = c
        .query_row(
            "SELECT choice FROM release_track_selection WHERE release_id = ?1 AND track_id = ?2 AND deleted = 0",
            params![release, track],
            |r| r.get(0),
        )
        .optional()?;
    Ok(choice.map(|c| c == "include"))
}

fn put_track_in(c: &Connection, release: &str, track: &str, choice: Option<bool>, now: UnixMs, device: &str) -> Result<()> {
    let Some(include) = choice else {
        // A reset: a tombstone of what is stored (nothing stored, nothing
        // written).
        c.execute(
            "UPDATE release_track_selection SET deleted = 1, updated_at = ?3, updated_by = ?4
              WHERE release_id = ?1 AND track_id = ?2 AND deleted = 0",
            params![release, track, now, device],
        )?;
        return Ok(());
    };
    c.execute(
        "INSERT INTO release_track_selection(release_id, track_id, choice, updated_at, updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)
         ON CONFLICT(release_id, track_id) DO UPDATE SET choice = excluded.choice,
            updated_at = excluded.updated_at, updated_by = excluded.updated_by, deleted = 0",
        params![release, track, if include { "include" } else { "exclude" }, now, device],
    )?;
    Ok(())
}

impl Library {
    /// The whole album into the library (`on`), or back to the songs
    /// chosen one by one: its tracks that only stream join (or leave) the
    /// library. Nothing downloads.
    pub fn set_release_whole(&self, release: ReleaseId, on: bool) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let (_, view) = release_in(tx, &rid)?;
            put_release_in(tx, &rid, on, view, now, device.as_str())?;
            refresh_in_library_of_release(tx, &rid)
        })
    }

    /// What the page of `release` shows and «Слушать» plays while only some
    /// of it is the user's: `Some(true)` the whole album, `Some(false)` the
    /// user's songs, `None` as the setting says.
    pub fn set_release_view(&self, release: ReleaseId, view: Option<bool>) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let (whole, _) = release_in(tx, &rid)?;
            put_release_in(tx, &rid, whole, view, now, device.as_str())
        })
    }

    /// `(whole, view)` of `release` (see [`Self::set_release_whole`] and
    /// [`Self::set_release_view`]).
    pub fn release_choice(&self, release: ReleaseId) -> Result<(bool, Option<bool>)> {
        self.read(|c| release_in(c, &release.to_string()))
    }

    /// A song of `release` added to the user's songs of it (`Some(true)`:
    /// a library track from then on), left out of them (`Some(false)`: its
    /// file, like and playlists stay), or back to as it comes (`None`).
    pub fn set_track_choice(&self, release: ReleaseId, track: TrackId, choice: Option<bool>) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let (rid, tid) = (release.to_string(), track.to_string());
            put_track_in(tx, &rid, &tid, choice, now, device.as_str())?;
            refresh_in_library(tx, &tid)
        })
    }

    /// The stored choice of a song of `release` ([`Self::set_track_choice`]).
    pub fn track_choice(&self, release: ReleaseId, track: TrackId) -> Result<Option<bool>> {
        self.read(|c| track_in(c, &release.to_string(), &track.to_string()))
    }
}

/// Every choice, tombstones included, for the export.
pub(crate) fn read_choices(c: &Connection) -> Result<(Vec<ReleaseChoice>, Vec<TrackChoice>)> {
    let releases = {
        let mut stmt = c.prepare_cached(
            "SELECT release_id, mode, view, updated_at, updated_by, deleted FROM release_selection ORDER BY release_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ReleaseChoice {
                release_id: crate::util::get_id(r, 0)?,
                whole: r.get::<_, String>(1)? == "whole",
                view: view_of(&r.get::<_, String>(2)?),
                updated_at: r.get(3)?,
                updated_by: r.get(4)?,
                deleted: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let tracks = {
        let mut stmt = c.prepare_cached(
            "SELECT release_id, track_id, choice, updated_at, updated_by, deleted FROM release_track_selection
              ORDER BY release_id, track_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TrackChoice {
                release_id: crate::util::get_id(r, 0)?,
                track_id: crate::util::get_id(r, 1)?,
                include: r.get::<_, String>(2)? == "include",
                updated_at: r.get(3)?,
                updated_by: r.get(4)?,
                deleted: r.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok((releases, tracks))
}

/// Merges imported choices by last writer (a tombstone wins a tie, then
/// the larger device), as likes are.
pub(crate) fn merge_choices_in(
    tx: &Connection,
    releases: &[ReleaseChoice],
    tracks: &[TrackChoice],
    report: &mut ImportReport,
) -> Result<()> {
    for r in releases {
        let rid = r.release_id.to_string();
        let old: Option<(i64, bool, String)> = tx
            .query_row(
                "SELECT updated_at, deleted, updated_by FROM release_selection WHERE release_id = ?1",
                [&rid],
                |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?)),
            )
            .optional()?;
        let wins = old.as_ref().is_none_or(|(ts, deleted, by)| {
            crate::userdata::incoming_wins((r.updated_at, r.deleted, r.updated_by.as_str()), (*ts, *deleted, by.as_str()))
        });
        if !wins {
            report.skipped += 1;
            continue;
        }
        tx.execute(
            "INSERT INTO release_selection(release_id, mode, view, updated_at, updated_by, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(release_id) DO UPDATE SET mode = excluded.mode, view = excluded.view,
                updated_at = excluded.updated_at, updated_by = excluded.updated_by, deleted = excluded.deleted",
            params![rid, if r.whole { "whole" } else { "auto" }, view_str(r.view), r.updated_at, r.updated_by, r.deleted],
        )?;
        report.applied += 1;
    }
    for t in tracks {
        let (rid, tid) = (t.release_id.to_string(), t.track_id.to_string());
        let old: Option<(i64, bool, String)> = tx
            .query_row(
                "SELECT updated_at, deleted, updated_by FROM release_track_selection
                  WHERE release_id = ?1 AND track_id = ?2",
                params![rid, tid],
                |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?)),
            )
            .optional()?;
        let wins = old.as_ref().is_none_or(|(ts, deleted, by)| {
            crate::userdata::incoming_wins((t.updated_at, t.deleted, t.updated_by.as_str()), (*ts, *deleted, by.as_str()))
        });
        if !wins {
            report.skipped += 1;
            continue;
        }
        tx.execute(
            "INSERT INTO release_track_selection(release_id, track_id, choice, updated_at, updated_by, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(release_id, track_id) DO UPDATE SET choice = excluded.choice,
                updated_at = excluded.updated_at, updated_by = excluded.updated_by, deleted = excluded.deleted",
            params![rid, tid, if t.include { "include" } else { "exclude" }, t.updated_at, t.updated_by, t.deleted],
        )?;
        report.applied += 1;
    }
    Ok(())
}

/// A track merged into another ([`crate::catalog::merge_track_in`]): its
/// choices on the album the other is on go with it (the other's own win),
/// the rest go.
pub(crate) fn carry_track_choices_in(tx: &Connection, from: &str, into: &str) -> Result<()> {
    tx.execute(
        "UPDATE OR IGNORE release_track_selection SET track_id = ?2
          WHERE track_id = ?1 AND release_id = (SELECT release_id FROM track WHERE id = ?2)",
        params![from, into],
    )?;
    tx.execute("DELETE FROM release_track_selection WHERE track_id = ?1", [from])?;
    Ok(())
}

/// The tracks of the album `from` are about to move onto `to`. One album
/// twice (`edition` false): its choices go with them (`to`'s own win).
/// Another edition folded in: the choice of each song goes with it, but a
/// whole album is no whole other edition, so its songs in the library
/// become songs added one by one (nothing leaves the library, and nothing
/// of the other edition joins it).
pub(crate) fn carry_release_choices_in(
    tx: &Connection,
    from: &str,
    to: &str,
    edition: bool,
    now: UnixMs,
    device: &str,
) -> Result<()> {
    if edition {
        tx.execute(
            "INSERT OR IGNORE INTO release_track_selection(release_id, track_id, choice, updated_at, updated_by, deleted)
             SELECT ?2, t.id, 'include', ?3, ?4, 0 FROM track t
              WHERE t.release_id = ?1 AND t.deleted_at IS NULL AND t.in_library = 1
                AND EXISTS (SELECT 1 FROM release_selection s WHERE s.release_id = ?1
                               AND s.mode = 'whole' AND s.deleted = 0)",
            params![from, to, now, device],
        )?;
    } else {
        tx.execute(
            "UPDATE OR IGNORE release_selection SET release_id = ?2 WHERE release_id = ?1",
            params![from, to],
        )?;
    }
    tx.execute("DELETE FROM release_selection WHERE release_id = ?1", [from])?;
    tx.execute(
        "UPDATE OR IGNORE release_track_selection SET release_id = ?2 WHERE release_id = ?1",
        params![from, to],
    )?;
    tx.execute("DELETE FROM release_track_selection WHERE release_id = ?1", [from])?;
    Ok(())
}

/// An album removed for good: its choices go.
pub(crate) fn drop_release_choices_in(c: &Connection, release: &str) -> Result<()> {
    c.execute("DELETE FROM release_selection WHERE release_id = ?1", [release])?;
    c.execute("DELETE FROM release_track_selection WHERE release_id = ?1", [release])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::{ArtistCredit, Codec, Release, Source, SourceKind, Track};

    fn album(lib: &Library, title: &str) -> Release {
        lib.upsert_release(&Release::new(title, ArtistCredit::single("A", None))).unwrap()
    }

    fn song(lib: &Library, release: &Release, no: u32, title: &str, file: bool) -> Track {
        let mut t = Track::new(title, ArtistCredit::single("A", None));
        t.release_id = Some(release.id);
        t.track_no = no;
        let t = lib.upsert_track(&t).unwrap();
        let (kind, locator, codec) = if file {
            (SourceKind::LocalFile, format!("A/{title}.flac"), Codec::Flac)
        } else {
            (SourceKind::Youtube, format!("yt-{title}"), Codec::Opus)
        };
        lib.add_source(&Source::new(t.id, kind, locator, codec)).unwrap();
        t
    }

    /// One song of thirteen with a file: one of the user's songs, none
    /// lacking, the album still counts its thirteen; a stream song added
    /// by hand lacks a file; a hole is nobody's song.
    #[test]
    fn the_users_songs_are_counted_apart_from_the_album() {
        let lib = Library::open_in_memory().unwrap();
        let mut r = album(&lib, "OST");
        r.total_tracks = Some(14);
        let r = lib.upsert_release(&r).unwrap();
        let kept = song(&lib, &r, 1, "Main", true);
        let others: Vec<Track> = (2..=13).map(|n| song(&lib, &r, n, &format!("Song {n}"), false)).collect();
        let row = lib.get_release_row(r.id).unwrap().unwrap();
        let p = row.progress();
        assert_eq!((p.total, p.local, p.wanted, p.wanted_local), (14, 1, 1, 1));
        assert!(p.partial());
        assert_eq!(p.wanted_missing(), 0);
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.wanted_ids(), vec![kept.id]);
        assert_eq!(l.holes(), 1);

        // A stream song added to the user's songs: a library track, no file.
        lib.set_track_choice(r.id, others[0].id, Some(true)).unwrap();
        let p = lib.get_release_row(r.id).unwrap().unwrap().progress();
        assert_eq!((p.wanted, p.wanted_local, p.wanted_missing()), (2, 1, 1));
        assert_eq!(lib.tracks_of_release(r.id).unwrap().len(), 2);
        // Left out again: back out of the library.
        lib.set_track_choice(r.id, others[0].id, None).unwrap();
        assert_eq!(lib.get_release_row(r.id).unwrap().unwrap().progress().wanted, 1);

        // The song with the file left out of this album: still a library
        // track, no longer one of the album's.
        lib.set_track_choice(r.id, kept.id, Some(false)).unwrap();
        let p = lib.get_release_row(r.id).unwrap().unwrap().progress();
        assert_eq!((p.wanted, p.local), (0, 1));
        assert_eq!(lib.track_choice(r.id, kept.id).unwrap(), Some(false));
        lib.set_track_choice(r.id, kept.id, None).unwrap();
        assert_eq!(lib.track_choice(r.id, kept.id).unwrap(), None);
    }

    /// The whole album into the library: every song of it is the user's,
    /// the ones that only stream too, nothing else changes; back, and
    /// they leave.
    #[test]
    fn a_whole_album_joins_the_library_and_leaves_it() {
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Radio");
        song(&lib, &r, 1, "One", true);
        song(&lib, &r, 2, "Two", false);
        song(&lib, &r, 3, "Three", false);
        let in_lib = || lib.tracks_of_release(r.id).unwrap().len();
        assert_eq!(in_lib(), 1);
        assert!(lib.get_release_row(r.id).unwrap().unwrap().progress().partial());
        lib.set_release_whole(r.id, true).unwrap();
        assert_eq!(in_lib(), 3);
        let p = lib.get_release_row(r.id).unwrap().unwrap().progress();
        assert!(p.whole && !p.partial());
        assert_eq!((p.wanted, p.wanted_local, p.wanted_missing()), (3, 1, 2));
        assert_eq!(lib.release_choice(r.id).unwrap(), (true, None));
        lib.set_release_view(r.id, Some(true)).unwrap();
        assert_eq!(lib.release_choice(r.id).unwrap(), (true, Some(true)));
        lib.set_release_whole(r.id, false).unwrap();
        assert_eq!(in_lib(), 1);
        assert_eq!(lib.release_choice(r.id).unwrap(), (false, Some(true)));
    }

    /// «SugaRiddle (Instrumental)» beside «SugaRiddle»: another version,
    /// apart from the song; chosen, it is the user's; a version without its
    /// song beside it proves nothing.
    #[test]
    fn versions_of_a_song_are_told_apart() {
        use crate::types::Membership;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "SugaRiddle");
        let main = song(&lib, &r, 1, "SugaRiddle", true);
        let inst = song(&lib, &r, 2, "SugaRiddle (Instrumental)", false);
        let lone = song(&lib, &r, 3, "Other (Remix)", false);
        let l = lib.release_listing(r.id).unwrap().unwrap();
        let m = l.membership(true);
        assert_eq!(m[&main.id], Membership::Wanted);
        assert_eq!(m[&inst.id], Membership::Variant);
        assert_eq!(m[&lone.id], Membership::Undecided);
        assert_eq!(l.membership(false)[&inst.id], Membership::Undecided);
        assert!(l.progress.partial());
        assert_eq!(l.wanted_ids(), vec![main.id]);
        lib.set_track_choice(r.id, inst.id, Some(true)).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.membership(true)[&inst.id], Membership::Wanted);
        assert_eq!(l.wanted_ids(), vec![main.id, inst.id]);
    }

    /// Two copies of one song folded into one: the choice goes with it.
    #[test]
    fn a_choice_follows_its_track_into_a_merge() {
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "X");
        let a = song(&lib, &r, 1, "Song", true);
        let b = song(&lib, &r, 2, "Song B", false);
        lib.set_track_choice(r.id, b.id, Some(true)).unwrap();
        assert!(lib.merge_track_into(b.id, a.id).unwrap());
        assert_eq!(lib.track_choice(r.id, a.id).unwrap(), Some(true));
        assert_eq!(lib.track_choice(r.id, b.id).unwrap(), None);
    }

    /// The choices go into the export and come back from it, a reset too.
    #[test]
    fn choices_survive_an_export_and_an_import() {
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Single");
        let a = song(&lib, &r, 1, "Song", true);
        let b = song(&lib, &r, 2, "Song (Instrumental)", false);
        lib.set_track_choice(r.id, a.id, Some(false)).unwrap();
        lib.set_track_choice(r.id, b.id, Some(true)).unwrap();
        lib.set_release_view(r.id, Some(false)).unwrap();
        let (releases, tracks) = lib.read(read_choices).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(tracks.len(), 2);

        let other = Library::open_in_memory().unwrap();
        let mut report = ImportReport::default();
        other.write(|tx| merge_choices_in(tx, &releases, &tracks, &mut report)).unwrap();
        assert_eq!(report.applied, 3);
        assert_eq!(other.release_choice(r.id).unwrap(), (false, Some(false)));
        assert_eq!(other.read(|c| track_in(c, &r.id.to_string(), &a.id.to_string())).unwrap(), Some(false));
        // An older copy loses; a newer reset wins.
        let mut report = ImportReport::default();
        other.write(|tx| merge_choices_in(tx, &releases, &tracks, &mut report)).unwrap();
        assert_eq!(report.applied, 0);
        lib.set_track_choice(r.id, a.id, None).unwrap();
        let (releases, tracks) = lib.read(read_choices).unwrap();
        other.write(|tx| merge_choices_in(tx, &releases, &tracks, &mut ImportReport::default())).unwrap();
        assert_eq!(other.read(|c| track_in(c, &r.id.to_string(), &a.id.to_string())).unwrap(), None);
    }
}
