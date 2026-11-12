//! What the user chose of an album, apart from what the album is (tables
//! `release_selection`, `release_track_selection`; docs/TODO.md 57).
//!
//! An album's catalog (its page's rows and holes) says what the album is.
//! The user's songs of it («Мои треки») are its rows that are library
//! tracks (`in_library`: a file, a like, a playlist, a song added here) and
//! not left out of this album by hand. Only decisions are stored:
//!
//! * per album: the whole album added to the library («Добавить в библиотеку»,
//!   `mode = 'whole'`: every track of it is a library track,
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
    /// Its like stands for the user's songs only, not the whole album: it
    /// came with the like of one of its songs (a single saved with its
    /// song), or the user kept only their songs of a saved album.
    #[serde(default)]
    pub mine: bool,
    /// What its page shows while only some of it is the user's:
    /// `Some(false)` «Мои треки», `Some(true)` «Весь альбом», `None` the
    /// setting.
    pub view: Option<bool>,
    /// Whether its other versions (instrumentals, karaoke, remixes the user
    /// did not choose) play with it: `None` as the setting says.
    #[serde(default)]
    pub versions: Option<bool>,
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

/// What an album is to the user as a whole (`release_selection.mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Its songs one by one; a like of the album (its own like row) makes
    /// it whole.
    Auto,
    /// Added whole («Добавить в библиотеку»).
    Whole,
    /// Its songs one by one even though it is saved: its like came with a
    /// like of one of its songs, or the user kept only their songs.
    Mine,
}

impl Mode {
    fn of(s: &str) -> Self {
        match s {
            "whole" => Self::Whole,
            "mine" => Self::Mine,
            _ => Self::Auto,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Whole => "whole",
            Self::Mine => "mine",
        }
    }

    fn of_choice(whole: bool, mine: bool) -> Self {
        if whole {
            Self::Whole
        } else if mine {
            Self::Mine
        } else {
            Self::Auto
        }
    }
}

/// A live choice of an album: its mode, page view and other versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Choice {
    mode: Mode,
    view: Option<bool>,
    versions: Option<bool>,
}

impl Choice {
    const AUTO: Self = Self { mode: Mode::Auto, view: None, versions: None };

    fn with_mode(self, mode: Mode) -> Self {
        Self { mode, ..self }
    }
}

/// The live choice of `release` ([`Choice::AUTO`] when none).
fn release_in(c: &Connection, release: &str) -> Result<Choice> {
    let row: Option<(String, String, String)> = c
        .query_row(
            "SELECT mode, view, versions FROM release_selection WHERE release_id = ?1 AND deleted = 0",
            [release],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.map_or(Choice::AUTO, |(mode, view, versions)| Choice {
        mode: Mode::of(&mode),
        view: view_of(&view),
        versions: versions_of(&versions),
    }))
}

fn versions_str(versions: Option<bool>) -> &'static str {
    match versions {
        Some(true) => "on",
        Some(false) => "off",
        None => "auto",
    }
}

fn versions_of(s: &str) -> Option<bool> {
    match s {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

fn put_release_in(c: &Connection, release: &str, choice: Choice, now: UnixMs, device: &str) -> Result<()> {
    let reset = choice == Choice::AUTO;
    c.execute(
        "INSERT INTO release_selection(release_id, mode, view, versions, updated_at, updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(release_id) DO UPDATE SET mode = excluded.mode, view = excluded.view,
            versions = excluded.versions, updated_at = excluded.updated_at, updated_by = excluded.updated_by,
            deleted = excluded.deleted",
        params![release, choice.mode.as_str(), view_str(choice.view), versions_str(choice.versions), now, device, reset],
    )?;
    Ok(())
}

/// The album `release` was saved or unsaved by hand: a like of it by hand
/// stands for the whole album again (a «mine» mark of it goes).
pub(crate) fn liked_by_hand_in(c: &Connection, release: &str, now: UnixMs, device: &str) -> Result<()> {
    let choice = release_in(c, release)?;
    if choice.mode == Mode::Mine {
        put_release_in(c, release, choice.with_mode(Mode::Auto), now, device)?;
    }
    Ok(())
}

fn release_liked_in(c: &Connection, release: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS (SELECT 1 FROM likes WHERE target_kind = 'release' AND target_id = ?1 AND deleted = 0)",
        [release],
        |r| r.get(0),
    )?)
}

/// What «Добавить в библиотеку» changed, to put back on
/// «Отменить» ([`Library::undo_release_whole`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WholeUndo {
    /// The album's like stood for the user's songs only.
    pub mine: bool,
    /// The songs left out of it by hand, in again now.
    pub excluded: Vec<TrackId>,
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
    /// library. Nothing downloads. Back to the songs of a saved album: its
    /// like stands for them only from then on. Adding it whole by hand is
    /// [`Self::add_release_whole`].
    pub fn set_release_whole(&self, release: ReleaseId, on: bool) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let choice = release_in(tx, &rid)?;
            let mode = if on {
                Mode::Whole
            } else if release_liked_in(tx, &rid)? {
                Mode::Mine
            } else {
                Mode::Auto
            };
            put_release_in(tx, &rid, choice.with_mode(mode), now, device.as_str())?;
            refresh_in_library_of_release(tx, &rid)
        })
    }

    /// «Добавить в библиотеку»: the whole album, the songs
    /// left out of it by hand too, is in the library (nothing downloads).
    /// Returns what to put back on «Отменить»; `None` when it already was
    /// added whole (nothing changed).
    pub fn add_release_whole(&self, release: ReleaseId) -> Result<Option<WholeUndo>> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let choice = release_in(tx, &rid)?;
            let mode = choice.mode;
            if mode == Mode::Whole {
                return Ok(None);
            }
            let excluded: Vec<TrackId> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT track_id FROM release_track_selection
                      WHERE release_id = ?1 AND choice = 'exclude' AND deleted = 0 ORDER BY track_id",
                )?;
                let rows = stmt.query_map([&rid], |r| crate::util::get_id(r, 0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for t in &excluded {
                put_track_in(tx, &rid, &t.to_string(), None, now, device.as_str())?;
            }
            put_release_in(tx, &rid, choice.with_mode(Mode::Whole), now, device.as_str())?;
            refresh_in_library_of_release(tx, &rid)?;
            Ok(Some(WholeUndo { mine: mode == Mode::Mine, excluded }))
        })
    }

    /// «Отменить» of [`Self::add_release_whole`]: the album is its songs
    /// one by one again, and the songs left out of it before are out again.
    pub fn undo_release_whole(&self, release: ReleaseId, undo: &WholeUndo) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let choice = release_in(tx, &rid)?;
            for t in &undo.excluded {
                put_track_in(tx, &rid, &t.to_string(), Some(false), now, device.as_str())?;
            }
            let mode = if undo.mine { Mode::Mine } else { Mode::Auto };
            put_release_in(tx, &rid, choice.with_mode(mode), now, device.as_str())?;
            refresh_in_library_of_release(tx, &rid)
        })
    }

    /// A single saved (`on`) or unsaved along with the like of one of its
    /// songs: its like stands for that song, not for the whole single (the
    /// other songs of it, an instrumental, stay out of the user's songs).
    pub fn save_release_with_song(&self, release: ReleaseId, on: bool) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let choice = release_in(tx, &rid)?;
            let mode = match (on, choice.mode) {
                (_, Mode::Whole) => Mode::Whole,
                (true, _) => Mode::Mine,
                (false, _) => Mode::Auto,
            };
            put_release_in(tx, &rid, choice.with_mode(mode), now, device.as_str())?;
            crate::playlists::set_like_in(tx, nikgob_core::model::LikeTarget::Release(release), !on, now, &device)
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
            let choice = release_in(tx, &rid)?;
            put_release_in(tx, &rid, Choice { view, ..choice }, now, device.as_str())
        })
    }

    /// `(whole, view)` of `release` (see [`Self::set_release_whole`] and
    /// [`Self::set_release_view`]).
    pub fn release_choice(&self, release: ReleaseId) -> Result<(bool, Option<bool>)> {
        self.read(|c| release_in(c, &release.to_string()).map(|ch| (ch.mode == Mode::Whole, ch.view)))
    }

    /// Whether the other versions of `release` (instrumentals, off vocal,
    /// karaoke, remixes the user did not choose) play with it: `Some(on)`
    /// for this album, `None` as the setting says.
    pub fn set_release_versions(&self, release: ReleaseId, versions: Option<bool>) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let choice = release_in(tx, &rid)?;
            put_release_in(tx, &rid, Choice { versions, ..choice }, now, device.as_str())
        })
    }

    /// [`Self::set_release_versions`] of `release`.
    pub fn release_versions(&self, release: ReleaseId) -> Result<Option<bool>> {
        self.read(|c| release_in(c, &release.to_string()).map(|ch| ch.versions))
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
            "SELECT release_id, mode, view, updated_at, updated_by, deleted, versions FROM release_selection
              ORDER BY release_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ReleaseChoice {
                release_id: crate::util::get_id(r, 0)?,
                whole: r.get::<_, String>(1)? == "whole",
                mine: r.get::<_, String>(1)? == "mine",
                view: view_of(&r.get::<_, String>(2)?),
                versions: versions_of(&r.get::<_, String>(6)?),
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
            "INSERT INTO release_selection(release_id, mode, view, versions, updated_at, updated_by, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(release_id) DO UPDATE SET mode = excluded.mode, view = excluded.view,
                versions = excluded.versions, updated_at = excluded.updated_at, updated_by = excluded.updated_by,
                deleted = excluded.deleted",
            params![
                rid,
                Mode::of_choice(r.whole, r.mine).as_str(),
                view_str(r.view),
                versions_str(r.versions),
                r.updated_at,
                r.updated_by,
                r.deleted
            ],
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
    // Put into the library by hand: the song that stays is (its own row
    // wins).
    tx.execute(
        "INSERT OR IGNORE INTO track_put(track_id, updated_at, updated_by, deleted)
         SELECT ?2, updated_at, updated_by, deleted FROM track_put WHERE track_id = ?1",
        params![from, into],
    )?;
    tx.execute("DELETE FROM track_put WHERE track_id = ?1", [from])?;
    Ok(())
}

/// A song put into the library by hand («В библиотеку»), for the export.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackAdded {
    pub track_id: TrackId,
    pub updated_at: UnixMs,
    pub updated_by: String,
    pub deleted: bool,
}

/// Every song put into the library by hand, tombstones included.
pub(crate) fn read_added(c: &Connection) -> Result<Vec<TrackAdded>> {
    let mut stmt = c.prepare_cached("SELECT track_id, updated_at, updated_by, deleted FROM track_put ORDER BY track_id")?;
    let rows = stmt.query_map([], |r| {
        Ok(TrackAdded { track_id: crate::util::get_id(r, 0)?, updated_at: r.get(1)?, updated_by: r.get(2)?, deleted: r.get(3)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Merges imported songs put into the library by last writer.
pub(crate) fn merge_added_in(tx: &Connection, added: &[TrackAdded], report: &mut ImportReport) -> Result<()> {
    for a in added {
        let tid = a.track_id.to_string();
        let old: Option<(i64, bool, String)> = tx
            .query_row("SELECT updated_at, deleted, updated_by FROM track_put WHERE track_id = ?1", [&tid], |x| {
                Ok((x.get(0)?, x.get(1)?, x.get(2)?))
            })
            .optional()?;
        let wins = old.as_ref().is_none_or(|(ts, deleted, by)| {
            crate::userdata::incoming_wins((a.updated_at, a.deleted, a.updated_by.as_str()), (*ts, *deleted, by.as_str()))
        });
        if !wins {
            report.skipped += 1;
            continue;
        }
        put_added_in(tx, &tid, !a.deleted, a.updated_at, &a.updated_by)?;
        report.applied += 1;
    }
    Ok(())
}

fn put_added_in(c: &Connection, track: &str, on: bool, now: UnixMs, device: &str) -> Result<()> {
    c.execute(
        "INSERT INTO track_put(track_id, updated_at, updated_by, deleted) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(track_id) DO UPDATE SET updated_at = excluded.updated_at, updated_by = excluded.updated_by,
            deleted = excluded.deleted",
        params![track, now, device, !on],
    )?;
    Ok(())
}

/// What «В библиотеку» / «Убрать из библиотеки» of a song found before it
/// changed anything, to put back on «Отменить»
/// ([`Library::restore_track_in_library`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackLibraryWas {
    /// It was put into the library by hand (`track_put`).
    pub put: bool,
    /// Its choice on its album: `Some(true)` added, `Some(false)` left out.
    pub choice: Option<bool>,
}

fn put_live_in(c: &Connection, track: &str) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM track_put WHERE track_id = ?1 AND deleted = 0)", [track], |r| {
        r.get(0)
    })?)
}

fn release_of_in(c: &Connection, track: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT release_id FROM track WHERE id = ?1", [track], |r| r.get(0)).optional()?.flatten())
}

fn in_library_in(c: &Connection, track: &str) -> Result<bool> {
    Ok(c.query_row("SELECT in_library FROM track WHERE id = ?1", [track], |r| r.get(0)).optional()?.unwrap_or(false))
}

impl Library {
    /// «В библиотеку» (`on`) of a song outside the library (a stream track
    /// of the wave, a radio, the queue, search): a library track from then
    /// on, no like, nothing downloads; one of the user's songs of its
    /// album (a choice that left it out of the album goes). «Убрать из
    /// библиотеки» (off) of a song without a file: what put it there by
    /// hand goes (this mark, its being added to the user's songs of its
    /// album); only when nothing of its own keeps it (a like, a playlist)
    /// and its album does (added whole, or saved) is it left out of the
    /// album's songs. Returns whether it is a library track now and what
    /// was there before, for «Отменить».
    pub fn set_track_in_library(&self, track: TrackId, on: bool) -> Result<(bool, TrackLibraryWas)> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let tid = track.to_string();
            let release = release_of_in(tx, &tid)?;
            let was = TrackLibraryWas {
                put: put_live_in(tx, &tid)?,
                choice: match &release {
                    Some(rid) => track_in(tx, rid, &tid)?,
                    None => None,
                },
            };
            put_added_in(tx, &tid, on, now, device.as_str())?;
            if let Some(rid) = &release {
                match (on, was.choice) {
                    // Put in by hand: no longer left out of its album.
                    (true, Some(false)) | (false, Some(true)) => put_track_in(tx, rid, &tid, None, now, device.as_str())?,
                    _ => {}
                }
                if !on {
                    let own: bool = tx.query_row(
                        &format!("SELECT {} FROM track WHERE track.id = ?1", crate::streams::OWN_EXPR),
                        [&tid],
                        |r| r.get(0),
                    )?;
                    let album_keeps: bool = tx.query_row(
                        concat!("SELECT ", release_whole_expr!(), " FROM release r WHERE r.id = ?1"),
                        [rid],
                        |r| r.get(0),
                    )?;
                    if album_keeps && !own {
                        put_track_in(tx, rid, &tid, Some(false), now, device.as_str())?;
                    }
                }
            }
            refresh_in_library(tx, &tid)?;
            Ok((in_library_in(tx, &tid)?, was))
        })
    }

    /// «Отменить» of [`Self::set_track_in_library`]: the mark and the
    /// album's choice of the song as they were.
    pub fn restore_track_in_library(&self, track: TrackId, was: TrackLibraryWas) -> Result<bool> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let tid = track.to_string();
            put_added_in(tx, &tid, was.put, now, device.as_str())?;
            if let Some(rid) = release_of_in(tx, &tid)? {
                put_track_in(tx, &rid, &tid, was.choice, now, device.as_str())?;
            }
            refresh_in_library(tx, &tid)?;
            in_library_in(tx, &tid)
        })
    }
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
        // The two choices merged: added whole on either side stays whole,
        // else `to`'s own mode and page view win over `from`'s (a view or a
        // reset of `to` never drops a whole album of `from`).
        let live = |id: &str| -> Result<Option<Choice>> {
            let deleted: Option<bool> = tx
                .query_row("SELECT deleted FROM release_selection WHERE release_id = ?1", [id], |r| r.get(0))
                .optional()?;
            Ok(match deleted {
                Some(false) => Some(release_in(tx, id)?),
                _ => None,
            })
        };
        if let Some(from_c) = live(from)? {
            let to_c = live(to)?.unwrap_or(Choice::AUTO);
            let mode = if from_c.mode == Mode::Whole || to_c.mode == Mode::Whole {
                Mode::Whole
            } else if to_c.mode != Mode::Auto {
                to_c.mode
            } else {
                from_c.mode
            };
            let merged = Choice { mode, view: to_c.view.or(from_c.view), versions: to_c.versions.or(from_c.versions) };
            put_release_in(tx, to, merged, now, device)?;
        }
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
    use crate::types::AlbumRules;
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
        // None of it the user's now, but chosen from: still an album of
        // the user's songs (none to play, none owed), not the whole album.
        assert!(p.partial());
        assert_eq!(p.lacking(), 0);
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert!(l.progress.partial());
        assert!(l.wanted_ids().is_empty());
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

    /// «Добавить весь альбом» takes in the songs left out of it by hand too;
    /// «Отменить» leaves them out again and the album is its songs one by
    /// one, as before.
    #[test]
    fn adding_a_whole_album_takes_in_the_songs_left_out() {
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Radio");
        let one = song(&lib, &r, 1, "One", true);
        let two = song(&lib, &r, 2, "Two", false);
        song(&lib, &r, 3, "Three", false);
        lib.set_track_choice(r.id, two.id, Some(true)).unwrap();
        lib.set_track_choice(r.id, two.id, Some(false)).unwrap();
        lib.set_track_choice(r.id, one.id, Some(false)).unwrap();
        let undo = lib.add_release_whole(r.id).unwrap().expect("not whole before");
        assert_eq!(undo.excluded.len(), 2);
        assert!(!undo.mine);
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert!(l.progress.whole && !l.progress.partial());
        assert_eq!(l.wanted.len(), 3, "the songs left out are in too");
        assert!(l.excluded.is_empty());
        assert_eq!(lib.tracks_of_release(r.id).unwrap().len(), 3);
        // Once more: nothing to undo.
        assert_eq!(lib.add_release_whole(r.id).unwrap(), None);

        lib.undo_release_whole(r.id, &undo).unwrap();
        assert_eq!(lib.release_choice(r.id).unwrap(), (false, None));
        assert_eq!(lib.track_choice(r.id, one.id).unwrap(), Some(false));
        assert_eq!(lib.track_choice(r.id, two.id).unwrap(), Some(false));
        let p = lib.get_release_row(r.id).unwrap().unwrap().progress();
        assert!(p.partial());
        assert_eq!(p.wanted, 0);
        // Only the song with the file is a library track again.
        assert_eq!(lib.tracks_of_release(r.id).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![one.id]);
    }

    /// A single saved with the like of its song: its like stands for that
    /// song, not for its instrumental (out of the library, the single stays
    /// one of the user's songs); saved by hand it is the whole single.
    #[test]
    fn a_single_saved_with_its_song_is_not_the_whole_single() {
        use nikgob_core::model::LikeTarget;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Song");
        let main = song(&lib, &r, 1, "Song", false);
        let inst = song(&lib, &r, 2, "Song (Instrumental)", false);
        lib.like(LikeTarget::Track(main.id)).unwrap();
        lib.save_release_with_song(r.id, true).unwrap();
        assert!(lib.is_liked(LikeTarget::Release(r.id)).unwrap());
        let row = lib.get_release_row(r.id).unwrap().unwrap();
        assert!(row.liked && !row.whole);
        assert!(row.progress().partial());
        assert_eq!(lib.tracks_of_release(r.id).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), vec![main.id]);
        let (releases, _) = lib.read(read_choices).unwrap();
        assert!(releases[0].mine && !releases[0].whole);
        // Worked out again (a tracklist found later): the same.
        lib.write(|tx| crate::streams::refresh_in_library_all(tx).map(|_| ())).unwrap();
        assert!(!lib.release_listing(r.id).unwrap().unwrap().wanted.contains(&inst.id));

        // Kept and saved again by hand: the whole single.
        lib.like_release(r.id, false).unwrap();
        lib.like_release(r.id, true).unwrap();
        let row = lib.get_release_row(r.id).unwrap().unwrap();
        assert!(row.whole && !row.progress().partial());
        assert_eq!(lib.tracks_of_release(r.id).unwrap().len(), 2);

        // Unsaved with its song: nothing of it stays chosen.
        lib.save_release_with_song(r.id, true).unwrap();
        lib.save_release_with_song(r.id, false).unwrap();
        assert!(!lib.is_liked(LikeTarget::Release(r.id)).unwrap());
        assert_eq!(lib.release_choice(r.id).unwrap(), (false, None));

        // A saved album kept to the user's songs: its like stands for them.
        lib.like_release(r.id, true).unwrap();
        lib.set_release_whole(r.id, false).unwrap();
        let row = lib.get_release_row(r.id).unwrap().unwrap();
        assert!(row.liked && !row.whole && row.progress().partial());
    }

    /// One album twice folded into one: a whole album on the side that goes
    /// stays whole, whatever the other side kept (a view, a reset).
    #[test]
    fn a_merge_keeps_an_album_added_whole() {
        for to_choice in ["view", "reset", "none"] {
            let lib = Library::open_in_memory().unwrap();
            let from = album(&lib, "Twice");
            let to = album(&lib, "Twice (copy)");
            song(&lib, &from, 1, "One", true);
            song(&lib, &from, 2, "Two", false);
            song(&lib, &to, 3, "Three", false);
            lib.set_release_whole(from.id, true).unwrap();
            match to_choice {
                "view" => lib.set_release_view(to.id, Some(false)).unwrap(),
                "reset" => {
                    lib.set_release_view(to.id, Some(true)).unwrap();
                    lib.set_release_view(to.id, None).unwrap();
                }
                _ => {}
            }
            lib.merge_releases(from.id, to.id).unwrap();
            let view = if to_choice == "view" { Some(false) } else { None };
            assert_eq!(lib.release_choice(to.id).unwrap(), (true, view), "{to_choice}");
            assert_eq!(lib.tracks_of_release(to.id).unwrap().len(), 3, "{to_choice}");
            assert_eq!(lib.release_choice(from.id).unwrap(), (false, None));
        }
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
        let plan = l.plan(AlbumRules::default());
        let m = &plan.membership;
        assert_eq!(m[&main.id], Membership::Wanted);
        assert_eq!(m[&inst.id], Membership::Variant);
        assert_eq!(m[&lone.id], Membership::Undecided);
        assert!(plan.split && !plan.opens_whole);
        assert_eq!(plan.mine, vec![main.id]);
        // The whole album without its instrumental, unless asked.
        assert_eq!(plan.whole, vec![main.id, lone.id]);
        assert_eq!(l.plays(AlbumRules::default(), None, None), (vec![main.id], false));
        assert_eq!(l.plays(AlbumRules::default(), Some(true), None), (vec![main.id, lone.id], true));
        // The instrumental picked by hand plays, the album from it.
        assert_eq!(l.plays(AlbumRules::default(), None, Some(inst.id)), (vec![main.id, inst.id, lone.id], true));
        let with = AlbumRules { other_versions: true, ..AlbumRules::default() };
        assert_eq!(l.plan(with).whole, vec![main.id, inst.id, lone.id]);
        lib.set_track_choice(r.id, inst.id, Some(true)).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.plan(AlbumRules::default()).membership[&inst.id], Membership::Wanted);
        assert_eq!(l.plan(AlbumRules::default()).mine, vec![main.id, inst.id]);
        assert_eq!(l.wanted_ids(), vec![main.id, inst.id]);
    }

    /// An album added to the library plays whole, its other versions only
    /// when asked: the setting brings the instrumental, never the karaoke;
    /// the album's own switch brings both; a version liked by hand is the
    /// user's song all along.
    #[test]
    fn an_added_album_plays_whole_without_its_other_versions() {
        use crate::types::Membership;
        use nikgob_core::model::LikeTarget;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Single");
        let a = song(&lib, &r, 1, "Song A", true);
        let b = song(&lib, &r, 2, "Song B", false);
        let inst = song(&lib, &r, 3, "Song A (Instrumental)", false);
        let kara = song(&lib, &r, 4, "Song A (オリジナル・カラオケ)", false);
        let remix = song(&lib, &r, 5, "Song B - Remix", false);
        let rules = AlbumRules::default();
        assert!(lib.release_listing(r.id).unwrap().unwrap().plan(rules).split);
        lib.add_release_whole(r.id).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        let plan = l.plan(rules);
        assert!(!plan.split);
        assert_eq!(plan.mine, vec![a.id, b.id]);
        assert_eq!(plan.whole, vec![a.id, b.id]);
        assert_eq!(plan.versions.len(), 3);
        assert!(!plan.versions_on);
        assert_eq!(l.plays(rules, None, None), (vec![a.id, b.id], false));
        let set = AlbumRules { other_versions: true, ..rules };
        assert_eq!(l.plan(set).whole, vec![a.id, b.id, inst.id, remix.id]);
        assert!(l.plan(set).versions_on);
        lib.set_release_versions(r.id, Some(true)).unwrap();
        assert_eq!(lib.release_versions(r.id).unwrap(), Some(true));
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.plan(rules).whole, vec![a.id, b.id, inst.id, kara.id, remix.id]);
        lib.set_release_versions(r.id, Some(false)).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.plan(set).whole, vec![a.id, b.id]);
        assert!(!l.plan(set).versions_on);
        // Liked: the instrumental is the user's song.
        lib.like(LikeTarget::Track(inst.id)).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.plan(rules).membership[&inst.id], Membership::Wanted);
        assert_eq!(l.plan(rules).whole, vec![a.id, b.id, inst.id]);
        // The album's switch survives an export and an import.
        let (releases, tracks) = lib.read(read_choices).unwrap();
        assert_eq!(releases[0].versions, Some(false));
        let other = Library::open_in_memory().unwrap();
        other.write(|tx| merge_choices_in(tx, &releases, &tracks, &mut ImportReport::default())).unwrap();
        assert_eq!(other.release_versions(r.id).unwrap(), Some(false));
        assert_eq!(other.release_choice(r.id).unwrap().0, true);
    }

    /// A version the user owns stays theirs on an album made whole (by the
    /// migration or by hand): a file, a like, a playlist, put in by hand.
    /// One that only came with the album waits for the switch.
    #[test]
    fn owned_versions_stay_the_users_on_a_whole_album() {
        use crate::types::Membership;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Light of Hope");
        let song_ = song(&lib, &r, 1, "Light of Hope", true);
        let inst = song(&lib, &r, 2, "Light of Hope (Instrumental)", true);
        let kara = song(&lib, &r, 3, "Light of Hope (Karaoke)", false);
        let rmx = song(&lib, &r, 4, "Light of Hope (Remix)", false);
        let pl = lib.create_playlist("P").unwrap();
        lib.playlist_add_many(pl.id, &[rmx.id]).unwrap();
        lib.add_release_whole(r.id).unwrap();
        let plan = lib.release_listing(r.id).unwrap().unwrap().plan(AlbumRules::default());
        assert_eq!(plan.membership[&inst.id], Membership::Wanted, "a file of it");
        assert_eq!(plan.membership[&rmx.id], Membership::Wanted, "in a playlist");
        assert_eq!(plan.membership[&kara.id], Membership::Variant, "came with the album");
        assert_eq!(plan.whole, vec![song_.id, inst.id, rmx.id]);
        // Put into the library by hand: the user's too.
        lib.set_track_in_library(kara.id, true).unwrap();
        let plan = lib.release_listing(r.id).unwrap().unwrap().plan(AlbumRules::default());
        assert_eq!(plan.membership[&kara.id], Membership::Wanted);
    }

    /// «Только мои песни» of an album added whole with none of its songs
    /// the user's own: an empty «Мои треки» (nothing plays), not the album.
    #[test]
    fn only_my_songs_of_an_added_album_without_any_is_empty() {
        use nikgob_core::model::AlbumMode;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "Streams");
        song(&lib, &r, 1, "One", false);
        song(&lib, &r, 2, "Two", false);
        lib.add_release_whole(r.id).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        let mine = AlbumRules { mode: AlbumMode::Mine, ..AlbumRules::default() };
        let plan = l.plan(mine);
        assert!(plan.split && plan.mine.is_empty());
        assert_eq!(l.plays(mine, None, None), (Vec::new(), false));
        assert_eq!(l.plays(mine, Some(true), None).0.len(), 2);
        assert_eq!(l.plays(AlbumRules::default(), None, None).0.len(), 2);
    }

    /// The album's switch on is kept even when the setting is on (karaoke
    /// plays only by the album's own on); off only against the setting.
    #[test]
    fn the_album_switch_is_kept_for_karaoke() {
        let off = AlbumRules::default();
        let on = AlbumRules { other_versions: true, ..off };
        assert_eq!(on.versions_choice(true), Some(true));
        assert_eq!(off.versions_choice(true), Some(true));
        assert_eq!(on.versions_choice(false), Some(false));
        assert_eq!(off.versions_choice(false), None);
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "歌");
        let a = song(&lib, &r, 1, "夏の歌", true);
        let k = song(&lib, &r, 2, "夏の歌 (オリジナルカラオケ)", false);
        let lone = song(&lib, &r, 3, "冬 (オリジナルカラオケ)", false);
        lib.add_release_whole(r.id).unwrap();
        lib.set_release_versions(r.id, on.versions_choice(true)).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        // Japanese karaoke without a dot is told; a mark without its song
        // beside it proves nothing.
        assert_eq!(l.version_kinds().keys().copied().collect::<Vec<_>>(), vec![k.id]);
        assert_eq!(l.plan(on).whole, vec![a.id, k.id, lone.id]);
        assert_eq!(l.plan(on).mine, vec![a.id, lone.id]);
    }

    /// «Всегда весь альбом»: an album only some songs of which are the
    /// user's opens and plays whole. «Только мои песни»: an album added
    /// whole shows the songs the user has or chose, not the ones that came
    /// with it.
    #[test]
    fn the_setting_says_how_albums_open() {
        use nikgob_core::model::AlbumMode;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "OST");
        let a = song(&lib, &r, 1, "One", true);
        let b = song(&lib, &r, 2, "Two", false);
        let l = lib.release_listing(r.id).unwrap().unwrap();
        let whole = AlbumRules { mode: AlbumMode::Whole, ..AlbumRules::default() };
        let plan = l.plan(whole);
        assert!(plan.split && plan.opens_whole);
        assert_eq!(l.plays(whole, None, None), (vec![a.id, b.id], true));
        assert_eq!(l.plays(whole, Some(false), None), (vec![a.id], false));

        lib.add_release_whole(r.id).unwrap();
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert!(!l.plan(AlbumRules::default()).split);
        let mine = AlbumRules { mode: AlbumMode::Mine, ..AlbumRules::default() };
        let plan = l.plan(mine);
        assert!(plan.split && !plan.opens_whole);
        assert_eq!(plan.mine, vec![a.id]);
        assert_eq!(l.plays(mine, None, None), (vec![a.id], false));
        // A queue of the user's songs takes in only theirs as the album
        // grows; a queue of the whole album all of it.
        assert_eq!(l.joins(mine, false), vec![a.id]);
        assert_eq!(l.joins(mine, true), vec![a.id, b.id]);
    }

    /// The migration to schema 28 keeps the albums the library has whole
    /// as albums added whole: every line a library track, two or more, no
    /// count asking for more; nothing else is written.
    #[test]
    fn the_migration_keeps_whole_albums_whole() {
        use nikgob_core::model::LikeTarget;
        let lib = Library::open_in_memory().unwrap();
        let counted = |lib: &Library, title: &str, n: u32| {
            let mut r = album(lib, title);
            r.total_tracks = Some(n);
            lib.upsert_release(&r).unwrap()
        };
        let full = counted(&lib, "Full", 2);
        song(&lib, &full, 1, "One", true);
        song(&lib, &full, 2, "Two", true);
        // Every song there, but how many it has is not known.
        let unknown = album(&lib, "Unknown count");
        song(&lib, &unknown, 1, "One", true);
        song(&lib, &unknown, 2, "Two", true);
        // Saved by hand, count unknown: whole by its like, not written.
        let saved = album(&lib, "Saved");
        song(&lib, &saved, 1, "One", true);
        lib.like(LikeTarget::Release(saved.id)).unwrap();
        let one = album(&lib, "One song");
        song(&lib, &one, 1, "Alone", true);
        let part = album(&lib, "Part");
        song(&lib, &part, 1, "Mine", true);
        song(&lib, &part, 2, "Not mine", false);
        let mut short = album(&lib, "Short");
        short.total_tracks = Some(3);
        let short = lib.upsert_release(&short).unwrap();
        song(&lib, &short, 1, "A", true);
        song(&lib, &short, 2, "B", true);
        let left = album(&lib, "Left");
        let l1 = song(&lib, &left, 1, "A", true);
        song(&lib, &left, 2, "B", true);
        lib.set_track_choice(left.id, l1.id, Some(false)).unwrap();
        let viewed = counted(&lib, "Viewed", 2);
        song(&lib, &viewed, 1, "A", true);
        song(&lib, &viewed, 2, "B", true);
        lib.set_release_view(viewed.id, Some(false)).unwrap();
        let in_lib = lib.count_tracks(crate::types::TrackFilter::All).unwrap();

        let sql = crate::migrations::MIGRATIONS[27];
        let insert = &sql[sql.find("INSERT INTO release_selection").unwrap()..];
        lib.write(|tx| Ok(tx.execute_batch(insert)?)).unwrap();
        let whole = |r: &Release| lib.release_choice(r.id).unwrap().0;
        assert!(whole(&full));
        assert!(whole(&viewed));
        assert_eq!(lib.release_choice(viewed.id).unwrap().1, Some(false));
        assert!(!whole(&one) && !whole(&part) && !whole(&short) && !whole(&left) && !whole(&unknown));
        // What the library showed before, it shows now: the album of an
        // unknown count is all its songs and plays them; the saved one is
        // whole; only these two of the six are albums on Главная.
        let row = |r: &Release| lib.get_release_row(r.id).unwrap().unwrap();
        assert!(!row(&unknown).progress().partial());
        assert!(row(&saved).whole && row(&saved).is_added());
        assert!(row(&full).is_added() && !row(&unknown).is_added() && !row(&part).is_added());
        let l = lib.release_listing(unknown.id).unwrap().unwrap();
        assert_eq!(l.plays(AlbumRules::default(), None, None).0.len(), 2);
        assert_eq!(lib.count_tracks(crate::types::TrackFilter::All).unwrap(), in_lib);
        // Run twice: nothing more.
        lib.write(|tx| Ok(tx.execute_batch(insert)?)).unwrap();
        assert_eq!(lib.read(read_choices).unwrap().0.iter().filter(|c| c.whole).count(), 2);
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

    /// «В библиотеку» of a stream song: a library track without a like,
    /// one of the user's songs of its album; «Убрать из библиотеки» takes
    /// it out again, out of an album added whole too, and both go through
    /// an export.
    #[test]
    fn a_stream_song_goes_into_the_library_and_out() {
        use nikgob_core::model::LikeTarget;
        let lib = Library::open_in_memory().unwrap();
        let r = album(&lib, "OST");
        let a = song(&lib, &r, 1, "One", true);
        let b = song(&lib, &r, 2, "Two", false);
        let in_lib = |lib: &Library| lib.tracks_of_release(r.id).unwrap().iter().map(|t| t.id).collect::<Vec<_>>();
        assert!(lib.set_track_in_library(b.id, true).unwrap().0);
        assert!(!lib.is_liked(LikeTarget::Track(b.id)).unwrap());
        assert_eq!(in_lib(&lib), vec![a.id, b.id]);
        let l = lib.release_listing(r.id).unwrap().unwrap();
        assert_eq!(l.plan(AlbumRules::default()).mine, vec![a.id, b.id]);
        assert!(l.picked.contains(&b.id) && l.own.contains(&b.id));

        let added = lib.read(read_added).unwrap();
        let other = Library::open_in_memory().unwrap();
        other.write(|tx| merge_added_in(tx, &added, &mut ImportReport::default())).unwrap();
        assert_eq!(other.read(read_added).unwrap(), added);

        assert!(!lib.set_track_in_library(b.id, false).unwrap().0);
        assert_eq!(in_lib(&lib), vec![a.id]);
        // Out of an album added whole: left out of it, the album stays;
        // «Отменить» brings the song back into the album's songs.
        lib.add_release_whole(r.id).unwrap();
        assert_eq!(in_lib(&lib), vec![a.id, b.id]);
        let (now, was) = lib.set_track_in_library(b.id, false).unwrap();
        assert!(!now);
        assert_eq!(was, TrackLibraryWas { put: false, choice: None });
        assert_eq!(in_lib(&lib), vec![a.id]);
        assert_eq!(lib.track_choice(r.id, b.id).unwrap(), Some(false));
        assert!(lib.release_choice(r.id).unwrap().0);
        assert!(lib.restore_track_in_library(b.id, was).unwrap());
        assert_eq!(lib.track_choice(r.id, b.id).unwrap(), None);
        assert_eq!(in_lib(&lib), vec![a.id, b.id]);
        // Left out again, then put in by hand: no longer left out.
        lib.set_track_in_library(b.id, false).unwrap();
        let (now, was) = lib.set_track_in_library(b.id, true).unwrap();
        assert!(now);
        assert_eq!(was.choice, Some(false));
        assert_eq!(lib.track_choice(r.id, b.id).unwrap(), None);
        // In a playlist: taking it out writes no exclusion (the playlist
        // keeps it), and nothing of the album changes.
        lib.set_track_in_library(b.id, false).unwrap();
        lib.restore_track_in_library(b.id, TrackLibraryWas::default()).unwrap();
        let pl = lib.create_playlist("P").unwrap();
        lib.playlist_add_many(pl.id, &[b.id]).unwrap();
        let (now, _) = lib.set_track_in_library(b.id, false).unwrap();
        assert!(now);
        assert_eq!(lib.track_choice(r.id, b.id).unwrap(), None);
        // A liked song stays: the like keeps it, no exclusion either.
        lib.like(LikeTarget::Track(b.id)).unwrap();
        assert!(lib.set_track_in_library(b.id, false).unwrap().0);
        assert_eq!(lib.track_choice(r.id, b.id).unwrap(), None);
    }
}
