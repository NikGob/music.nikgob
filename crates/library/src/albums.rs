//! One-time repairs of albums (docs/stage4.md §12.13), each through
//! [`Library::repair`] (a mark, a backup, one transaction), run in order by
//! [`Library::repair_catalog`]:
//!
//! * `various_artists_unbound.v1`: «Various Artists» of a compilation is
//!   its own artist, not the artist whose MusicBrainz id a track gave it;
//! * `edition_places_fixed.v1`: two songs with files at one place of an
//!   album (two discs folded into disc 1, a file of another edition's
//!   folder) are renumbered ([`place_moves`]);
//! * `album_editions_folded.v1`: an album whose every song is a song of
//!   another, fuller album of the same artist and title («Ugly Is
//!   Beautiful» and its deluxe) is folded into it: its tracks merge into
//!   the deluxe's (their files spare copies), its ids in the services and
//!   «Хочу скачать» items follow;
//! * `tracklists_reidentified.v1`: an album whose rows all came from the
//!   service's tracklist yet counts more tracks (MusicBrainz knew a 51-song
//!   edition of «El Camino», YouTube Music gives 11) is the service's
//!   edition: the count is its rows, the MusicBrainz id goes; other albums
//!   short of their count are asked for their tracklist again.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use nikgob_core::model::{Artist, ArtistCredit};
use nikgob_core::names::{disc_of_folder, name_key, voiceless};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::catalog::{merge_track_in, upsert_artist_in, write_credit_links};
use crate::db::Library;
use crate::error::Result;
use crate::streams::{albums_agree, parse, song_key};
use crate::util::credit_columns;

pub(crate) const VARIOUS_KEY: &str = "various_artists_unbound.v1";
pub(crate) const PLACES_KEY: &str = "edition_places_fixed.v1";
pub(crate) const EDITIONS_KEY: &str = "album_editions_folded.v1";
pub(crate) const TRACKLISTS_KEY: &str = "tracklists_reidentified.v1";

/// MusicBrainz id of «Various Artists».
pub(crate) const VARIOUS_ARTISTS_MBID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

fn is_various(name: &str) -> bool {
    name_key(name) == "variousartists"
}

/// The library's «Various Artists» (made when missing).
fn various_artist(conn: &Connection) -> Result<String> {
    let found: Option<String> = conn
        .query_row(
            "SELECT id FROM artist WHERE mbid = ?1
             UNION ALL SELECT id FROM artist WHERE lower(replace(name, ' ', '')) = 'variousartists'
             LIMIT 1",
            [VARIOUS_ARTISTS_MBID],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    let mut va = Artist::new("Various Artists");
    va.mbid = Some(VARIOUS_ARTISTS_MBID.to_owned());
    Ok(upsert_artist_in(conn, &va)?.id.to_string())
}

fn unbind_various(tx: &Transaction<'_>) -> Result<((), usize)> {
    let names: HashMap<String, String> = {
        let mut stmt = tx.prepare_cached("SELECT id, name FROM artist")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut va: Option<String> = None;
    let mut n = 0;
    let mut left = Vec::new();
    for (table, links, owner) in [("release", "release_artist", "release_id"), ("track", "track_artist", "track_id")] {
        let rows: Vec<(String, String, String)> = {
            let mut stmt = tx.prepare_cached(&format!("SELECT id, title, artist_credit FROM {table} WHERE artist_credit LIKE '%arious%'"))?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, title, json) in rows {
            let Ok(mut credit) = serde_json::from_str::<ArtistCredit>(&json) else { continue };
            let mut changed = false;
            for p in &mut credit.0 {
                let bound = p.artist_id.and_then(|a| names.get(&a.to_string()));
                if is_various(&p.name) && !bound.is_some_and(|b| is_various(b)) {
                    let id = match &va {
                        Some(v) => v.clone(),
                        None => va.insert(various_artist(tx)?).clone(),
                    };
                    if let Some(old) = p.artist_id {
                        left.push(old.to_string());
                        note!("library: «{title}»: «Various Artists» was «{}», now its own artist", bound.cloned().unwrap_or_default());
                    }
                    p.artist_id = Some(parse(&id)?);
                    changed = true;
                }
            }
            if !changed {
                continue;
            }
            let (json, display, key) = credit_columns(&credit)?;
            tx.execute(
                &format!("UPDATE {table} SET artist_credit = ?2, artist_display = ?3, artist_key = ?4 WHERE id = ?1"),
                params![id, json, display, key],
            )?;
            write_credit_links(tx, links, owner, &id, &credit)?;
            n += 1;
        }
    }
    for a in left {
        crate::repairs::drop_orphan_artist_in(tx, &a)?;
    }
    Ok(((), n))
}

/// A track with a file as [`fix_places`] sees it.
struct Placed {
    id: String,
    title: String,
    disc: u32,
    no: u32,
    /// The folders of its files, the playing one first.
    dirs: Vec<String>,
}

/// The places of `release`'s tracks with files to change: `(track, title,
/// from, to)` (see the module docs). Most places taken twice: two discs
/// folded into one, split again in the order the files came (the first
/// at each number keeps disc 1). A few: the files of the album's own
/// folder (the one named after it, else the one most of its tracks have
/// a file in) keep their place, the others take the numbers after the
/// last of the disc. A folder that names a disc (`CD2`) goes first.
fn place_moves(conn: &Connection, release: &str) -> Result<Vec<(String, String, (u32, u32), (u32, u32))>> {
    let album: String = conn.query_row("SELECT title FROM release WHERE id = ?1", [release], |r| r.get(0))?;
    let mut tracks: Vec<Placed> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id, t.title, t.disc_no, t.track_no, s.locator FROM track t
               JOIN source s ON s.track_id = t.id AND s.kind = 'local_file' AND s.trashed_at IS NULL
              WHERE t.release_id = ?1 AND t.deleted_at IS NULL AND t.has_local = 1
              ORDER BY t.seq, s.is_active DESC",
        )?;
        let rows = stmt.query_map([release], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?, r.get::<_, u32>(3)?, r.get::<_, String>(4)?))
        })?;
        for row in rows {
            let (id, title, disc, no, locator) = row?;
            let dir = Path::new(&locator).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            match tracks.last_mut() {
                Some(t) if t.id == id => t.dirs.push(dir),
                _ => tracks.push(Placed { id, title, disc: disc.max(1), no, dirs: vec![dir] }),
            }
        }
    }
    let mut count: HashMap<(u32, u32), usize> = HashMap::new();
    for t in tracks.iter().filter(|t| t.no > 0) {
        *count.entry((t.disc, t.no)).or_default() += 1;
    }
    let crowded_places = count.values().filter(|n| **n > 1).count();
    if crowded_places == 0 {
        return Ok(Vec::new());
    }
    let folded_discs = crowded_places * 2 >= count.len();
    // The album's own folder.
    let key = song_key(&album);
    let named = |d: &str| Path::new(d).file_name().is_some_and(|n| song_key(&n.to_string_lossy()).ends_with(&key));
    let mut by: HashMap<&str, usize> = HashMap::new();
    for t in &tracks {
        let mine: HashSet<&str> = t.dirs.iter().map(String::as_str).collect();
        for d in mine {
            *by.entry(d).or_default() += 1;
        }
    }
    let home = by
        .iter()
        .max_by_key(|(d, n)| (!key.is_empty() && named(d), **n, std::cmp::Reverse(**d)))
        .map(|(d, _)| (*d).to_owned())
        .unwrap_or_default();
    let at_home = |t: &Placed| t.dirs.iter().any(|d| *d == home);
    let crowded = |t: &Placed| t.no > 0 && count.get(&(t.disc, t.no)).is_some_and(|n| *n > 1);
    let mut taken: HashSet<(u32, u32)> = tracks.iter().filter(|t| t.no > 0 && !crowded(t)).map(|t| (t.disc, t.no)).collect();
    let mut moves = Vec::new();
    let mut rest: Vec<&Placed> = Vec::new();
    for t in tracks.iter().filter(|t| crowded(t)) {
        let disc = Path::new(&t.dirs[0]).file_name().and_then(|n| disc_of_folder(&n.to_string_lossy()));
        match disc {
            Some(d) if d != t.disc && !taken.contains(&(d, t.no)) => {
                taken.insert((d, t.no));
                moves.push((t.id.clone(), t.title.clone(), (t.disc, t.no), (d, t.no)));
            }
            _ => rest.push(t),
        }
    }
    if !folded_discs {
        // A stable sort: the album's own files first, each in the order it came.
        rest.sort_by_key(|t| !at_home(t));
    }
    for t in rest {
        if !taken.contains(&(t.disc, t.no)) {
            taken.insert((t.disc, t.no));
            continue;
        }
        let to = if folded_discs {
            ((1..).find(|d| !taken.contains(&(*d, t.no))).unwrap_or(1), t.no)
        } else {
            (t.disc, taken.iter().filter(|p| p.0 == t.disc).map(|p| p.1).max().unwrap_or(0) + 1)
        };
        taken.insert(to);
        moves.push((t.id.clone(), t.title.clone(), (t.disc, t.no), to));
    }
    Ok(moves)
}

fn fix_places(tx: &Transaction<'_>) -> Result<((), usize)> {
    let releases: Vec<(String, String)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT DISTINCT t.release_id, r.title FROM track t JOIN release r ON r.id = t.release_id
              WHERE t.deleted_at IS NULL AND t.has_local = 1 AND t.track_no > 0
              GROUP BY t.release_id, max(t.disc_no, 1), t.track_no HAVING count(*) > 1",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut n = 0;
    for (release, album) in releases {
        for (id, title, from, to) in place_moves(tx, &release)? {
            tx.execute("UPDATE track SET disc_no = ?2, track_no = ?3 WHERE id = ?1", params![id, to.0, to.1])?;
            note!("library: «{album}»: «{title}» {}-{} → {}-{}", from.0, from.1, to.0, to.1);
            n += 1;
        }
    }
    Ok(((), n))
}

/// A track of an album as [`plan_editions`] compares it.
struct Tune {
    id: String,
    key: String,
    voiceless: String,
    duration: Option<i64>,
    isrc: Option<String>,
    recording: Option<String>,
}

impl Tune {
    fn same(&self, o: &Tune) -> bool {
        let eq = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
        if eq(&self.recording, &o.recording) || eq(&self.isrc, &o.isrc) {
            return true;
        }
        let close = self.duration.zip(o.duration).is_some_and(|(a, b)| (a - b).abs() <= crate::streams::SLACK_MS);
        close && !self.key.is_empty() && (self.key == o.key || self.voiceless == o.voiceless)
    }
}

struct Album {
    id: String,
    key: String,
    total: usize,
    seq: i64,
    artists: HashSet<String>,
    tunes: Vec<Tune>,
}

/// Albums to fold: `(from, into, [(track, into track)])` (see the module
/// docs).
fn plan_editions(conn: &Connection) -> Result<Vec<(String, String, Vec<(String, String)>)>> {
    let mut albums: Vec<Album> = {
        let mut stmt = conn.prepare_cached(
            "SELECT r.id, r.title, coalesce(r.total_tracks, 0), r.seq,
                    (SELECT group_concat(artist_id) FROM release_artist WHERE release_id = r.id)
               FROM release r
              WHERE r.release_type = 'album'
                AND EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)
              ORDER BY r.seq",
        )?;
        let rows = stmt.query_map([], |r| {
            let title: String = r.get(1)?;
            let artists: Option<String> = r.get(4)?;
            Ok(Album {
                id: r.get(0)?,
                key: song_key(&title),
                total: r.get::<_, i64>(2)? as usize,
                seq: r.get(3)?,
                artists: artists.map(|a| a.split(',').map(str::to_owned).collect()).unwrap_or_default(),
                tunes: Vec::new(),
            })
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut stmt = conn.prepare_cached(
        "SELECT id, title, duration_ms, isrc, recording_mbid FROM track
          WHERE release_id = ?1 AND deleted_at IS NULL ORDER BY disc_no, track_no, seq",
    )?;
    for a in &mut albums {
        let rows = stmt.query_map([&a.id], |r| {
            let title: String = r.get(1)?;
            Ok(Tune {
                id: r.get(0)?,
                key: song_key(&title),
                voiceless: song_key(&voiceless(&title)),
                duration: r.get(2)?,
                isrc: r.get(3)?,
                recording: r.get(4)?,
            })
        })?;
        a.tunes = rows.collect::<rusqlite::Result<_>>()?;
    }
    let mut out = Vec::new();
    let mut gone: HashSet<&str> = HashSet::new();
    for a in &albums {
        let mut best: Option<(&Album, Vec<(String, String)>)> = None;
        for b in &albums {
            let fuller = b.tunes.len() > a.tunes.len() || (b.tunes.len() == a.tunes.len() && b.seq < a.seq);
            if a.id == b.id
                || gone.contains(b.id.as_str())
                || !fuller
                || a.total.max(a.tunes.len()) > b.tunes.len()
                || a.artists.is_disjoint(&b.artists)
                || a.key.is_empty()
                || !albums_agree(&a.key, &b.key)
            {
                continue;
            }
            // Every song of `a` is a song of `b`, each a song of its own.
            let mut used: HashSet<&str> = HashSet::new();
            let mut pairs = Vec::new();
            for t in &a.tunes {
                let Some(m) = b.tunes.iter().find(|u| !used.contains(u.id.as_str()) && t.same(u)) else { break };
                used.insert(&m.id);
                pairs.push((t.id.clone(), m.id.clone()));
            }
            if pairs.len() == a.tunes.len() && best.as_ref().is_none_or(|(x, _)| b.tunes.len() > x.tunes.len()) {
                best = Some((b, pairs));
            }
        }
        if let Some((b, pairs)) = best {
            gone.insert(&a.id);
            out.push((a.id.clone(), b.id.clone(), pairs));
        }
    }
    Ok(out)
}

fn fold_editions(tx: &Transaction<'_>, device: &str, now: i64) -> Result<((), usize)> {
    let mut n = 0;
    for (from, into, pairs) in plan_editions(tx)? {
        let title = |id: &str| -> Result<String> { Ok(tx.query_row("SELECT title FROM release WHERE id = ?1", [id], |r| r.get(0))?) };
        let (a, b) = (title(&from)?, title(&into)?);
        for (t, u) in &pairs {
            merge_track_in(tx, parse(t)?, parse(u)?, device, now)?;
        }
        // Whatever it had that no song of the other matched stays its own.
        crate::repairs::drop_empty_release_in(tx, &from, Some(&into), device, now)?;
        note!("library: album «{a}» {from} is an edition of «{b}» {into}: {} tracks folded", pairs.len());
        n += 1;
    }
    Ok(((), n))
}

fn reidentify(tx: &Transaction<'_>) -> Result<((), usize)> {
    let short: Vec<(String, String, i64, i64, i64, Option<String>, bool)> = {
        let mut stmt = tx.prepare_cached(
            "SELECT r.id, r.title, r.total_tracks,
                    (SELECT count(*) FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL) n,
                    (SELECT count(*) FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL
                        AND EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind = 'youtube'
                                       AND s.trashed_at IS NULL)) yt,
                    (SELECT value FROM meta WHERE key = 'cache.tracklist.' || r.id),
                    EXISTS (SELECT 1 FROM external_id x WHERE x.kind = 'release' AND x.service = 'youtube'
                               AND x.local_id = r.id)
               FROM release r
              WHERE r.total_tracks > (SELECT count(*) FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)
                AND EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut n = 0;
    for (id, title, total, rows, yt, mark, linked) in short {
        let found = mark.as_deref().is_some_and(|m| m.ends_with("|found"));
        if found && linked && yt == rows {
            tx.execute("UPDATE release SET total_tracks = ?2, mbid = NULL WHERE id = ?1", params![id, rows])?;
            note!("library: «{title}» {id} is the service's edition: {rows} tracks, not {total}");
            n += 1;
        } else if mark.is_some() {
            tx.execute("DELETE FROM meta WHERE key = ?1", [format!("cache.tracklist.{id}")])?;
            note!("library: «{title}» {id} has {rows} of {total} tracks: its tracklist is asked again");
            n += 1;
        }
    }
    Ok(((), n))
}

impl Library {
    /// Once (`various_artists_unbound.v1`, see the module docs). Returns
    /// whether it changed anything.
    pub fn unbind_various_artists(&self) -> Result<bool> {
        Ok(self.repair(VARIOUS_KEY, unbind_various)?.is_some())
    }

    /// Once (`edition_places_fixed.v1`, see the module docs).
    pub fn fix_edition_places(&self) -> Result<bool> {
        Ok(self.repair(PLACES_KEY, fix_places)?.is_some())
    }

    /// Once (`album_editions_folded.v1`, see the module docs).
    pub fn fold_album_editions(&self) -> Result<bool> {
        let device = self.device_id();
        let now = self.tick();
        Ok(self.repair(EDITIONS_KEY, |tx| fold_editions(tx, device.as_str(), now))?.is_some())
    }

    /// Once (`tracklists_reidentified.v1`, see the module docs).
    pub fn reidentify_tracklists(&self) -> Result<bool> {
        Ok(self.repair(TRACKLISTS_KEY, reidentify)?.is_some())
    }

    /// Every one-time repair of the catalog, in order: what went first,
    /// then credits and artists, then duplicates, places and editions.
    /// A repair that fails is logged and the next one runs.
    pub fn repair_catalog(&self) {
        let steps: [(&str, &dyn Fn() -> Result<usize>); 8] = [
            ("orphans", &|| self.drop_orphans()),
            ("combined credits", &|| self.split_combined_credits()),
            ("various artists", &|| self.unbind_various_artists().map(usize::from)),
            ("artist doubles", &|| self.merge_artist_doubles().map(|m| m.len())),
            ("duplicate tracks", &|| self.merge_duplicate_stream_tracks()),
            ("edition places", &|| self.fix_edition_places().map(usize::from)),
            ("album editions", &|| self.fold_album_editions().map(usize::from)),
            ("tracklists", &|| self.reidentify_tracklists().map(usize::from)),
        ];
        for (what, step) in steps {
            match step() {
                Ok(0) => {}
                Ok(n) => log::info!("library: repair of {what}: {n}"),
                Err(e) => log::warn!("library: repair of {what}: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{
        Codec, ReleaseId, ReleaseType, Source, SourceKind, Track, TrackId, WishlistItem, WishlistScope, WishlistStatus,
    };
    use nikgob_core::streaming::{EntityKind, RemoteId, Service};

    use super::*;
    use crate::db::Library;
    use nikgob_core::model::Release;

    fn artist(lib: &Library, name: &str) -> Artist {
        lib.upsert_artist(&Artist::new(name)).unwrap()
    }

    fn release(lib: &Library, title: &str, by: &Artist, total: Option<u32>) -> ReleaseId {
        let mut r = Release::new(title, ArtistCredit::single(by.name.clone(), Some(by.id)));
        r.total_tracks = total;
        lib.upsert_release(&r).unwrap().id
    }

    /// A track at `disc`-`no` of `release`, with a file at `path` when given.
    fn track(lib: &Library, release: ReleaseId, by: &Artist, title: &str, (disc, no): (u32, u32), ms: u32, path: Option<&str>) -> TrackId {
        let mut t = Track::new(title, ArtistCredit::single(by.name.clone(), Some(by.id)));
        t.release_id = Some(release);
        t.disc_no = disc;
        t.track_no = no;
        t.duration_ms = Some(ms);
        let t = lib.upsert_track(&t).unwrap();
        if let Some(p) = path {
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, p, Codec::Opus)).unwrap();
        }
        t.id
    }

    fn places(lib: &Library, release: ReleaseId) -> Vec<(u32, u32, String)> {
        lib.read(|c| {
            let mut stmt =
                c.prepare_cached("SELECT disc_no, track_no, title FROM track WHERE release_id = ?1 AND deleted_at IS NULL ORDER BY disc_no, track_no")?;
            let rows = stmt.query_map([release.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap()
    }

    #[test]
    fn various_artists_of_a_compilation_is_its_own_artist() {
        let lib = Library::open_in_memory().unwrap();
        let mut ado = Artist::new("Ado");
        ado.mbid = Some("e134b52f-2e9e-4734-9bc3-bea9648d1fa1".into());
        let ado = lib.upsert_artist(&ado).unwrap();
        // The compilation's credit says «Various Artists», its link is Ado.
        let credit = ArtistCredit::single("Various Artists", Some(ado.id));
        let comp = lib.upsert_release(&Release::new("NO SONG NO LIFE", credit)).unwrap().id;
        track(&lib, comp, &ado, "Song", (1, 1), 200_000, Some("M/comp/01.opus"));

        assert!(lib.unbind_various_artists().unwrap());
        let r = lib.get_release(comp).unwrap().unwrap();
        let va = r.artist_credit.0[0].artist_id.unwrap();
        assert_ne!(va, ado.id);
        let va = lib.get_artist(va).unwrap().unwrap();
        assert_eq!((va.name.as_str(), va.mbid.as_deref()), ("Various Artists", Some(VARIOUS_ARTISTS_MBID)));
        assert_eq!(lib.get_artist(ado.id).unwrap().unwrap().name, "Ado", "Ado keeps its songs");
        assert!(!lib.unbind_various_artists().unwrap(), "once");
    }

    #[test]
    fn two_discs_folded_into_one_are_split_again_and_strays_take_free_numbers() {
        let lib = Library::open_in_memory().unwrap();
        let by = artist(&lib, "藤澤慶昌");
        // Both discs in one folder, tagged disc 1.
        let zero = release(&lib, "NO GAME NO LIFE ZERO Original Soundtrack", &by, None);
        let dir = "M/2018 - NO GAME NO LIFE ZERO Original Soundtrack";
        for (no, title) in [(1, "Prologue"), (2, "Let's Play"), (3, "Myth")] {
            track(&lib, zero, &by, title, (1, no), 100_000 + no * 1000, Some(&format!("{dir}/{no:02} - {title}.opus")));
        }
        // Disc 2; one of its files in a folder of the album's other title.
        for (no, title) in [(1, "A Sign"), (2, "Now"), (3, "Stay")] {
            let d = if no == 2 { "M/2018 - ノーゲーム･ノーライフ ゼロ Original Soundtrack" } else { dir };
            track(&lib, zero, &by, title, (1, no), 200_000 + no * 1000, Some(&format!("{d}/{no:02} - {title}.opus")));
        }
        // A special edition with a few files of the standard one at its numbers.
        let se = release(&lib, "Original Soundtrack 1 Special Edition", &by, None);
        let home = "M/2021 - Original Soundtrack 1 Special Edition";
        let other = "M/2021 - Original Soundtrack 1";
        let stray = track(&lib, se, &by, "ギレーヌと二人きり", (1, 2), 90_000, Some(&format!("{other}/02 - ギレーヌと二人きり.opus")));
        for (no, title) in [(1, "魔術の授業"), (2, "ロキシーからの手紙"), (3, "友達")] {
            let t = track(&lib, se, &by, title, (1, no), 120_000 + no * 1000, Some(&format!("{home}/{no:02} - {title}.opus")));
            // The standard's copy of it, playing.
            lib.add_source(&Source::new(t, SourceKind::LocalFile, format!("{other}/x{no}.opus"), Codec::Opus)).unwrap();
        }
        // A file of a folder that names its disc.
        let box_set = release(&lib, "Box", &by, None);
        track(&lib, box_set, &by, "One", (1, 1), 100_000, Some("M/Box/CD1/01.opus"));
        track(&lib, box_set, &by, "Two", (1, 1), 110_000, Some("M/Box/CD2/01.opus"));

        assert!(lib.fix_edition_places().unwrap());
        let titles = |r| places(&lib, r).into_iter().map(|(d, n, t)| format!("{d}-{n} {t}")).collect::<Vec<_>>();
        assert_eq!(titles(zero), ["1-1 Prologue", "1-2 Let's Play", "1-3 Myth", "2-1 A Sign", "2-2 Now", "2-3 Stay"]);
        assert_eq!(titles(se), ["1-1 魔術の授業", "1-2 ロキシーからの手紙", "1-3 友達", "1-4 ギレーヌと二人きり"]);
        assert_eq!(lib.get_track(stray).unwrap().unwrap().track_no, 4);
        assert_eq!(titles(box_set), ["1-1 One", "2-1 Two"]);
        assert!(!lib.fix_edition_places().unwrap(), "once");
    }

    #[test]
    fn an_album_whose_songs_are_all_on_its_deluxe_is_folded_into_it() {
        let lib = Library::open_in_memory().unwrap();
        let oliver = artist(&lib, "Oliver Tree");
        let standard = release(&lib, "Ugly Is Beautiful", &oliver, Some(3));
        let deluxe = release(&lib, "Ugly Is Beautiful: Shorter, Thicker & Uglier (Deluxe)", &oliver, Some(4));
        let songs = [("Me, Myself & I", 177_000), ("Cash Machine", 159_000), ("Alien Boy", 160_000)];
        let mut standard_tracks = Vec::new();
        for (i, (title, ms)) in songs.iter().enumerate() {
            let no = i as u32 + 1;
            standard_tracks.push(track(&lib, standard, &oliver, title, (1, no), *ms, Some(&format!("M/std/{no}.flac"))));
        }
        track(&lib, deluxe, &oliver, "Life Goes On", (1, 1), 150_000, Some("M/dlx/1.opus"));
        let mut deluxe_tracks = Vec::new();
        for (i, (title, ms)) in songs.iter().enumerate() {
            let no = i as u32 + 2;
            deluxe_tracks.push(track(&lib, deluxe, &oliver, title, (1, no), *ms + 400, Some(&format!("M/dlx/{no}.opus"))));
        }
        lib.set_external_id(EntityKind::Album, &RemoteId::new(Service::Deezer, "160736802"), standard.0).unwrap();
        let mut wish = WishlistItem::new(WishlistStatus::Done, 1);
        wish.scope = WishlistScope::Album;
        wish.release_id = Some(standard);
        lib.put_wishlist_item(&wish).unwrap();
        // Not editions: a single of the title, an album of another artist.
        let single = {
            let mut r = Release::new("Ugly Is Beautiful", ArtistCredit::single("Oliver Tree", Some(oliver.id)));
            r.release_type = ReleaseType::Single;
            lib.upsert_release(&r).unwrap().id
        };
        track(&lib, single, &oliver, "Alien Boy", (1, 1), 160_000, Some("M/single/1.opus"));
        let other = artist(&lib, "Someone");
        let cover = release(&lib, "Ugly Is Beautiful", &other, None);
        track(&lib, cover, &other, "Alien Boy", (1, 1), 160_000, Some("M/cover/1.opus"));

        assert!(lib.fold_album_editions().unwrap());
        assert!(lib.get_release(standard).unwrap().is_none());
        assert_eq!(places(&lib, deluxe).len(), 4, "21 of 21 in small");
        for (s, d) in standard_tracks.iter().zip(&deluxe_tracks) {
            assert!(lib.get_track(*s).unwrap().is_none());
            let files = lib.sources_of_track(*d).unwrap().iter().filter(|x| x.kind == SourceKind::LocalFile).count();
            assert_eq!(files, 2, "the standard's file is a spare copy");
        }
        assert_eq!(lib.release_by_remote(&RemoteId::new(Service::Deezer, "160736802")).unwrap(), Some(deluxe));
        assert_eq!(lib.get_wishlist_item(wish.id).unwrap().unwrap().release_id, Some(deluxe));
        assert!(lib.get_release(single).unwrap().is_some() && lib.get_release(cover).unwrap().is_some());
        assert!(!lib.fold_album_editions().unwrap(), "once");
    }

    #[test]
    fn an_album_of_the_services_tracklist_is_that_edition() {
        let lib = Library::open_in_memory().unwrap();
        let keys = artist(&lib, "The Black Keys");
        let camino = release(&lib, "El Camino", &keys, Some(51));
        lib.write(|tx| {
            tx.execute("UPDATE release SET mbid = '73b233c3-e612-4bf0-9813-114dc89245fe' WHERE id = ?1", [camino.to_string()])?;
            Ok(())
        })
        .unwrap();
        lib.set_external_id(EntityKind::Album, &RemoteId::new(Service::Youtube, "MPREb_camino"), camino.0).unwrap();
        for (no, title) in [(1, "Lonely Boy"), (2, "Dead and Gone"), (3, "Gold on the Ceiling")] {
            let t = track(&lib, camino, &keys, title, (1, no), 200_000, (no == 1).then_some("M/camino/01.opus"));
            lib.add_source(&Source::new(t, SourceKind::Youtube, format!("yt-{no}"), Codec::Opus)).unwrap();
        }
        lib.cache_put(&format!("tracklist.{camino}"), 1, "found").unwrap();
        // Short of its count, its tracklist not from a service: asked again.
        let mercury = release(&lib, "Mercury – Act 1", &keys, Some(18));
        track(&lib, mercury, &keys, "Wrecked", (1, 1), 200_000, Some("M/mercury/01.opus"));
        lib.cache_put(&format!("tracklist.{mercury}"), 1, "miss").unwrap();

        assert!(lib.reidentify_tracklists().unwrap());
        let r = lib.get_release(camino).unwrap().unwrap();
        assert_eq!((r.total_tracks, r.mbid), (Some(3), None), "3 of 3, no 48 missing");
        assert_eq!(lib.cache_get(&format!("tracklist.{mercury}")).unwrap(), None);
        assert_eq!(lib.get_release(mercury).unwrap().unwrap().total_tracks, Some(18));
        assert!(!lib.reidentify_tracklists().unwrap(), "once");
    }
}
