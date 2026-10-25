//! One-time repairs of the catalog and what they share (docs/stage4.md
//! §12.4, §12.13): their `meta` marks, a backup before the first write,
//! the files of torrents, and dropping what a merge emptied.
//!
//! A repair ([`Library::repair`]) runs once per library: it counts what it
//! would change first (a trial run, rolled back, quiet); with nothing to do
//! it only sets its mark, else the library is copied to
//! `backup-before-<key>-<date>/library.sqlite` next to it (`VACUUM INTO`,
//! consistent under WAL), and every change and the mark go in one
//! transaction, each change logged.
//!
//! An album that loses its last track ([`drop_empty_release_in`]) hands its
//! ids in the services, its «Хочу скачать» items and its like to the
//! library's other release of the same album, when there is one (one title
//! of the same artist: [`same_album_release`]), else the ids go; an artist
//! left without tracks, albums and like goes ([`drop_orphan_artist_in`]).

use std::cell::Cell;
use std::path::PathBuf;

use nikgob_core::model::{ArtistId, ReleaseId, Source};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::catalog::{SOURCE_COLS, source_from_row};
use crate::db::Library;
use crate::error::Result;
use crate::streams::{albums_agree, credit_keys, song_key};
use crate::util::{meta_get, meta_set, now_ms};

/// `meta` key of [`Library::drop_orphans`].
pub(crate) const ORPHANS_KEY: &str = "orphans_dropped.v1";
/// `meta` key of [`Library::settle_downloaded_wishes`].
pub(crate) const WISHES_KEY: &str = "downloaded_wishes_settled.v1";

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

/// A repair is only counting its changes on this thread (see [`note!`]).
pub(crate) fn counting() -> bool {
    COUNTING.with(Cell::get)
}

/// `yyyymmdd` of a unix time (UTC).
fn ymd(ms: i64) -> String {
    // Days to the civil date (H. Hinnant's algorithm).
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}

/// Two releases are one album: titles that agree (one an extension of the
/// other, «Ugly Is Beautiful» and «Ugly Is Beautiful: … (Deluxe)») and a
/// shared artist.
fn same_album(conn: &Connection, a: &str, b: &str) -> Result<bool> {
    let row = |id: &str| -> Result<Option<(String, String)>> {
        Ok(conn
            .query_row("SELECT title, artist_display FROM release WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    };
    let (Some((ta, aa)), Some((tb, ab))) = (row(a)?, row(b)?) else { return Ok(false) };
    let shared: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM release_artist x JOIN release_artist y ON y.artist_id = x.artist_id
                        WHERE x.release_id = ?1 AND y.release_id = ?2)",
        params![a, b],
        |r| r.get(0),
    )?;
    let (ka, kb) = (credit_keys(&aa), credit_keys(&ab));
    Ok(albums_agree(&song_key(&ta), &song_key(&tb)) && (shared || ka.iter().any(|k| kb.contains(k))))
}

/// The library's other release of the album `release` with live tracks
/// (see [`same_album`]): the same title first, then the fuller one.
pub(crate) fn same_album_release(conn: &Connection, release: &str) -> Result<Option<String>> {
    let Some(title) = conn
        .query_row("SELECT title FROM release WHERE id = ?1", [release], |r| r.get::<_, String>(0))
        .optional()?
    else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT r.id, r.title, (SELECT count(*) FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL) n
           FROM release r WHERE r.id != ?1
            AND EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)",
    )?;
    let rows: Vec<(String, String, i64)> = stmt.query_map([release], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    let key = song_key(&title);
    let mut best: Option<(bool, i64, String)> = None;
    for (id, t, n) in rows {
        if !albums_agree(&key, &song_key(&t)) || !same_album(conn, release, &id)? {
            continue;
        }
        let rank = (song_key(&t) == key, n, id);
        if best.as_ref().is_none_or(|b| (rank.0, rank.1) > (b.0, b.1)) {
            best = Some(rank);
        }
    }
    Ok(best.map(|b| b.2))
}

/// The release with live tracks an album called `title` by `artist` is
/// (a «Хочу скачать» item whose release went): the same title first, then
/// one that agrees, the fuller one.
pub(crate) fn release_like(conn: &Connection, title: &str, artist: &str) -> Result<Option<String>> {
    let (key, artists) = (song_key(title), credit_keys(artist));
    if key.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn.prepare(
        "SELECT r.id, r.title, r.artist_display, (SELECT count(*) FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)
           FROM release r WHERE EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL)",
    )?;
    let rows: Vec<(String, String, String, i64)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut best: Option<(bool, i64, String)> = None;
    for (id, t, a, n) in rows {
        let k = song_key(&t);
        if !albums_agree(&key, &k) || !(artists.is_empty() || credit_keys(&a).iter().any(|x| artists.contains(x))) {
            continue;
        }
        if best.as_ref().is_none_or(|b| (k == key, n) > (b.0, b.1)) {
            best = Some((k == key, n, id));
        }
    }
    Ok(best.map(|b| b.2))
}

/// Removes an artist no track or album credits and nobody likes (its
/// photo, ids in the services and aliases go too). Returns whether it went.
pub(crate) fn drop_orphan_artist_in(conn: &Connection, artist: &str) -> Result<bool> {
    let orphan: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM artist WHERE id = ?1)
            AND NOT EXISTS(SELECT 1 FROM track_artist WHERE artist_id = ?1)
            AND NOT EXISTS(SELECT 1 FROM release_artist WHERE artist_id = ?1)
            AND NOT EXISTS(SELECT 1 FROM likes WHERE target_kind = 'artist' AND target_id = ?1 AND deleted = 0)",
        [artist],
        |r| r.get(0),
    )?;
    if !orphan {
        return Ok(false);
    }
    let name: String = conn.query_row("SELECT name FROM artist WHERE id = ?1", [artist], |r| r.get(0))?;
    conn.execute("DELETE FROM artist_photo WHERE artist_id = ?1", [artist])?;
    conn.execute("DELETE FROM external_id WHERE kind = 'artist' AND local_id = ?1", [artist])?;
    conn.execute("DELETE FROM artist_alias WHERE artist_id = ?1", [artist])?;
    conn.execute("DELETE FROM meta WHERE key = ?1", [format!("cache.discography.{artist}")])?;
    conn.execute("DELETE FROM artist WHERE id = ?1", [artist])?;
    note!("library: artist «{name}» {artist} has nothing left, removed");
    Ok(true)
}

/// Removes a release without tracks (see the module docs): its ids in the
/// services, «Хочу скачать» items and like (and its cover and year, when
/// that lacks them) go to `prefer` when that is the same album, else to
/// the library's other release of it, else the ids and
/// the items' links go; its artist links, palette and tracklist mark go,
/// and so do artists it leaves without anything. Returns whether it was removed.
pub(crate) fn drop_empty_release_in(conn: &Connection, release: &str, prefer: Option<&str>, device: &str, now: i64) -> Result<bool> {
    let empty: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM release WHERE id = ?1)
            AND NOT EXISTS(SELECT 1 FROM track WHERE release_id = ?1)",
        [release],
        |r| r.get(0),
    )?;
    if !empty {
        return Ok(false);
    }
    let target = match prefer.filter(|p| *p != release) {
        Some(p) if same_album(conn, release, p)? => Some(p.to_owned()),
        _ => same_album_release(conn, release)?,
    };
    let title: String = conn.query_row("SELECT title FROM release WHERE id = ?1", [release], |r| r.get(0))?;
    match &target {
        Some(t) => {
            conn.execute(
                "UPDATE release SET
                        cover_ref = coalesce(cover_ref, (SELECT cover_ref FROM release WHERE id = ?1)),
                        year = coalesce(year, (SELECT year FROM release WHERE id = ?1))
                  WHERE id = ?2",
                params![release, t],
            )?;
            conn.execute("UPDATE external_id SET local_id = ?2 WHERE kind = 'release' AND local_id = ?1", params![release, t])?;
            conn.execute("UPDATE wishlist SET release_id = ?2 WHERE release_id = ?1", params![release, t])?;
            conn.execute(
                "UPDATE likes SET target_id = ?2, updated_at = ?3, updated_by = ?4
                  WHERE target_kind = 'release' AND target_id = ?1
                    AND NOT EXISTS (SELECT 1 FROM likes WHERE target_kind = 'release' AND target_id = ?2)",
                params![release, t, now, device],
            )?;
        }
        None => {
            conn.execute("DELETE FROM external_id WHERE kind = 'release' AND local_id = ?1", [release])?;
            conn.execute("UPDATE wishlist SET release_id = NULL WHERE release_id = ?1", [release])?;
        }
    }
    conn.execute(
        "UPDATE likes SET deleted = 1, updated_at = ?2, updated_by = ?3
          WHERE target_kind = 'release' AND target_id = ?1 AND deleted = 0",
        params![release, now, device],
    )?;
    let artists: Vec<String> = {
        let mut stmt = conn.prepare("SELECT artist_id FROM release_artist WHERE release_id = ?1")?;
        let rows = stmt.query_map([release], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    conn.execute("DELETE FROM release_artist WHERE release_id = ?1", [release])?;
    conn.execute("DELETE FROM palette WHERE release_id = ?1", [release])?;
    conn.execute("DELETE FROM meta WHERE key = ?1", [format!("cache.tracklist.{release}")])?;
    conn.execute("DELETE FROM release WHERE id = ?1", [release])?;
    match &target {
        Some(t) => note!("library: album «{title}» {release} is empty, removed; its ids and items go to {t}"),
        None => note!("library: album «{title}» {release} is empty, removed"),
    }
    for a in artists {
        drop_orphan_artist_in(conn, &a)?;
    }
    Ok(true)
}

/// What [`Library::drop_orphans`] would do: dangling ids in the services
/// and «Хочу скачать» links, empty albums, artists without anything,
/// tracklist marks of albums that went.
#[derive(Debug, Default)]
struct Orphans {
    /// `(service, kind, remote id, gone local id)`.
    dangling_ids: Vec<(String, String, String, String)>,
    /// `(item, gone release)`, `(item, gone track)`.
    wish_releases: Vec<(String, String)>,
    wish_tracks: Vec<(String, String)>,
    empty_releases: Vec<String>,
    artists: Vec<String>,
    tracklist_marks: Vec<String>,
}

impl Orphans {
    fn find(conn: &Connection) -> Result<Self> {
        let strings = |sql: &str| -> Result<Vec<String>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        let pairs = |sql: &str| -> Result<Vec<(String, String)>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        let dangling_ids = {
            let mut stmt = conn.prepare(
                "SELECT service, kind, remote_id, local_id FROM external_id x
                  WHERE (kind = 'release' AND NOT EXISTS (SELECT 1 FROM release r WHERE r.id = x.local_id))
                     OR (kind = 'track' AND NOT EXISTS (SELECT 1 FROM track t WHERE t.id = x.local_id))
                     OR (kind = 'artist' AND NOT EXISTS (SELECT 1 FROM artist a WHERE a.id = x.local_id))
                  ORDER BY service, kind, remote_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Self {
            dangling_ids,
            wish_releases: pairs(
                "SELECT id, release_id FROM wishlist w WHERE release_id IS NOT NULL
                    AND NOT EXISTS (SELECT 1 FROM release r WHERE r.id = w.release_id) ORDER BY created_at, id",
            )?,
            wish_tracks: pairs(
                "SELECT id, track_id FROM wishlist w WHERE track_id IS NOT NULL
                    AND NOT EXISTS (SELECT 1 FROM track t WHERE t.id = w.track_id) ORDER BY created_at, id",
            )?,
            empty_releases: strings("SELECT id FROM release r WHERE NOT EXISTS (SELECT 1 FROM track t WHERE t.release_id = r.id) ORDER BY seq")?,
            artists: strings(
                "SELECT id FROM artist a WHERE NOT EXISTS (SELECT 1 FROM track_artist WHERE artist_id = a.id)
                    AND NOT EXISTS (SELECT 1 FROM release_artist WHERE artist_id = a.id)
                    AND NOT EXISTS (SELECT 1 FROM likes WHERE target_kind = 'artist' AND target_id = a.id AND deleted = 0)
                  ORDER BY seq",
            )?,
            tracklist_marks: strings(
                "SELECT key FROM meta m WHERE key LIKE 'cache.tracklist.%'
                    AND NOT EXISTS (SELECT 1 FROM release r WHERE r.id = substr(m.key, 17)) ORDER BY key",
            )?,
        })
    }
}

/// The live track a «Хочу скачать» item whose track went is: by its ids in
/// the services, else its ISRC.
fn wished_track(conn: &Connection, item: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT t.id FROM wishlist w JOIN external_id x ON x.kind = 'track'
                    AND ((x.service = 'youtube' AND x.remote_id = w.youtube_id)
                      OR (x.service = 'spotify' AND x.remote_id = w.spotify_id))
               JOIN track t ON t.id = x.local_id
              WHERE w.id = ?1
             UNION ALL
             SELECT t.id FROM wishlist w JOIN track t ON t.isrc = w.isrc AND t.deleted_at IS NULL
              WHERE w.id = ?1 AND w.isrc IS NOT NULL
             LIMIT 1",
            [item],
            |r| r.get(0),
        )
        .optional()?)
}

impl Library {
    /// A `meta` value (the marks of one-time repairs).
    pub fn meta_value(&self, key: &str) -> Result<Option<String>> {
        self.read(|c| meta_get(c, key))
    }

    pub fn set_meta_value(&self, key: &str, value: &str) -> Result<()> {
        self.write(|tx| meta_set(tx, key, value))
    }

    /// The library file (`None` in memory).
    pub fn db_path(&self) -> Result<Option<PathBuf>> {
        self.read(|c| {
            let file: Option<String> = c
                .query_row("SELECT file FROM pragma_database_list WHERE name = 'main'", [], |r| r.get(0))
                .optional()?;
            Ok(file.filter(|f| !f.is_empty()).map(PathBuf::from))
        })
    }

    /// Copies the library to `backup-before-<key>-<yyyymmdd>/library.sqlite`
    /// next to it (one consistent copy, WAL included). Nothing in memory, or
    /// when that copy exists. Returns where it went.
    pub fn backup_before(&self, key: &str) -> Result<Option<PathBuf>> {
        let Some(db) = self.db_path()? else { return Ok(None) };
        let dir = db.parent().map(PathBuf::from).unwrap_or_default().join(format!("backup-before-{key}-{}", ymd(now_ms())));
        let file = dir.join("library.sqlite");
        if file.exists() {
            return Ok(Some(file));
        }
        std::fs::create_dir_all(&dir)?;
        self.with_writer(|c| {
            c.execute("VACUUM INTO ?1", [file.to_string_lossy()])?;
            Ok(())
        })?;
        log::info!("library: backed up to {} before {key}", file.display());
        Ok(Some(file))
    }

    /// Runs `f` in a write transaction that is rolled back, with the
    /// catalog's change notes silent: what a repair would do.
    pub(crate) fn trial<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        struct Quiet;
        impl Drop for Quiet {
            fn drop(&mut self) {
                COUNTING.with(|c| c.set(false));
            }
        }
        COUNTING.with(|c| c.set(true));
        let _quiet = Quiet;
        self.write_rolled_back(f)
    }

    /// Runs the one-time repair `key` (see the module docs): `apply` makes
    /// its changes and says how many it made (run once as a trial to count
    /// them). `None` when it ran before or had nothing to do.
    pub(crate) fn repair<T>(&self, key: &str, apply: impl Fn(&Transaction<'_>) -> Result<(T, usize)>) -> Result<Option<T>> {
        if self.read(|c| meta_get(c, key))?.is_some() {
            return Ok(None);
        }
        let (_, n) = self.trial(&apply)?;
        let stamp = now_ms().to_string();
        if n == 0 {
            self.write(|tx| meta_set(tx, key, &stamp))?;
            return Ok(None);
        }
        log::info!("library: {key}: {n} changes");
        self.backup_before(key)?;
        self.write(|tx| {
            let (out, done) = apply(tx)?;
            meta_set(tx, key, &stamp)?;
            log::info!("library: {key}: {done} changes made");
            Ok(Some(out))
        })
    }

    /// Once per library (`downloaded_wishes_settled.v1`): «Хочу скачать»
    /// items that did not start although their track (every track of their
    /// album) has a file are «Скачано» (their file came another way, or the
    /// track they wanted was merged into the one with the file later), and
    /// an album item takes the history of the track items of its album it
    /// covers. Nothing goes. Returns how many items changed.
    pub fn settle_downloaded_wishes(&self) -> Result<usize> {
        Ok(self
            .repair(WISHES_KEY, |tx| {
                let settled = crate::masters::settle_downloaded_in(tx)?;
                let folded = crate::masters::fold_track_items_in(tx)?;
                note!("library: «Хочу скачать»: {settled} items downloaded already, {folded} albums took their tracks' items");
                Ok((settled + folded, settled + folded))
            })?
            .unwrap_or(0))
    }

    /// Live local files that came from torrents, oldest first.
    pub fn torrent_sources(&self) -> Result<Vec<Source>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SOURCE_COLS} FROM source
                  WHERE kind = 'local_file' AND origin_kind = 'torrent' AND trashed_at IS NULL
                  ORDER BY added_at, id"
            ))?;
            let rows = stmt.query_map([], source_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The release with tracks an album called `title` by `artist` is (a
    /// «Хочу скачать» item whose release went): the same title first, then
    /// one that agrees («Ugly Is Beautiful» and its deluxe), the fuller one.
    pub fn find_release_named(&self, title: &str, artist: &str) -> Result<Option<ReleaseId>> {
        self.read(|c| release_like(c, title, artist)?.map(|r| crate::streams::parse(&r)).transpose())
    }

    /// Removes a release without tracks (see the module docs). Returns
    /// whether it was removed.
    pub fn drop_empty_release(&self, release: ReleaseId) -> Result<bool> {
        let now = self.tick();
        let device = self.device_id();
        self.write(|tx| drop_empty_release_in(tx, &release.to_string(), None, device.as_str(), now))
    }

    /// Removes an artist no track or release credits any more and nobody
    /// likes (with its photo). Returns whether it was removed.
    pub fn drop_orphan_artist(&self, artist: ArtistId) -> Result<bool> {
        self.write(|tx| drop_orphan_artist_in(tx, &artist.to_string()))
    }

    /// Once per library (`orphans_dropped.v1`): ids in the services and
    /// «Хочу скачать» items that point at an album or a track that went
    /// find it again (an album item by its title and artist, a track item
    /// by its ids in the services; the album's ids follow the item), else
    /// they go; albums without tracks go (their ids to the same album
    /// elsewhere), artists without anything go, tracklist marks of gone
    /// albums go. Returns how many changes it made.
    pub fn drop_orphans(&self) -> Result<usize> {
        let device = self.device_id();
        let now = self.tick();
        Ok(self
            .repair(ORPHANS_KEY, |tx| {
                let o = Orphans::find(tx)?;
                let mut n = 0;
                // Albums of «Хочу скачать» items found again by name.
                for (item, gone) in &o.wish_releases {
                    let (title, artist, scope): (Option<String>, Option<String>, String) = tx.query_row(
                        "SELECT title, artist, scope FROM wishlist WHERE id = ?1",
                        [item],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                    let found = match (scope.as_str(), title) {
                        ("album", Some(t)) => release_like(tx, &t, artist.as_deref().unwrap_or_default())?,
                        _ => None,
                    };
                    match &found {
                        Some(r) => {
                            tx.execute("UPDATE wishlist SET release_id = ?2 WHERE release_id = ?1", params![gone, r])?;
                            tx.execute(
                                "UPDATE external_id SET local_id = ?2 WHERE kind = 'release' AND local_id = ?1",
                                params![gone, r],
                            )?;
                            note!("library: «Хочу скачать» {item}: its album {gone} went, it is {r}");
                        }
                        None => note!("library: «Хочу скачать» {item}: its album {gone} went"),
                    }
                    n += 1;
                }
                for (item, gone) in &o.wish_tracks {
                    let found = wished_track(tx, item)?;
                    tx.execute("UPDATE wishlist SET track_id = ?2 WHERE id = ?1", params![item, found])?;
                    if let Some(t) = &found {
                        tx.execute(
                            "UPDATE wishlist SET release_id = (SELECT release_id FROM track WHERE id = ?2)
                              WHERE id = ?1 AND (release_id IS NULL OR release_id NOT IN (SELECT id FROM release))",
                            params![item, t],
                        )?;
                    }
                    note!("library: «Хочу скачать» {item}: its track {gone} went, it is {found:?}");
                    n += 1;
                }
                // Items whose album found nothing keep no dangling link.
                tx.execute("UPDATE wishlist SET release_id = NULL WHERE release_id NOT IN (SELECT id FROM release)", [])?;
                // What still points nowhere goes.
                for (service, kind, remote, local) in &o.dangling_ids {
                    let gone = tx.execute(
                        "DELETE FROM external_id WHERE service = ?1 AND kind = ?2 AND remote_id = ?3
                            AND local_id = ?4 AND NOT EXISTS (SELECT 1 FROM release WHERE id = ?4)
                            AND NOT EXISTS (SELECT 1 FROM track WHERE id = ?4)
                            AND NOT EXISTS (SELECT 1 FROM artist WHERE id = ?4)",
                        params![service, kind, remote, local],
                    )?;
                    if gone > 0 {
                        note!("library: {service} {kind} {remote} pointed at {local}, gone, removed");
                    }
                    n += 1;
                }
                for r in &o.empty_releases {
                    if drop_empty_release_in(tx, r, None, device.as_str(), now)? {
                        n += 1;
                    }
                }
                for a in &o.artists {
                    if drop_orphan_artist_in(tx, a)? {
                        n += 1;
                    }
                }
                for key in &o.tracklist_marks {
                    n += tx.execute("DELETE FROM meta WHERE key = ?1", [key])?;
                }
                Ok((n, n))
            })?
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{
        Artist, ArtistCredit, Codec, Release, SourceKind, Track, TrackId, WishlistItem, WishlistScope,
        WishlistStatus,
    };
    use nikgob_core::streaming::{EntityKind, RemoteId, Service};

    use super::*;

    #[test]
    fn dates_of_backups() {
        assert_eq!(ymd(0), "19700101");
        assert_eq!(ymd(1_792_800_000_000), "20261024");
        assert_eq!(ymd(951_782_400_000), "20000229");
    }

    /// An album of `artist` with one file per title.
    fn album(lib: &Library, artist: &Artist, title: &str, songs: &[&str]) -> (ReleaseId, Vec<TrackId>) {
        let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let release = lib.upsert_release(&Release::new(title, credit.clone())).unwrap();
        let tracks = songs
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut t = Track::new(*s, credit.clone());
                t.release_id = Some(release.id);
                t.track_no = i as u32 + 1;
                t.duration_ms = Some(180_000 + i as u32 * 1_000);
                let t = lib.upsert_track(&t).unwrap();
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("Music/{title}/{s}.opus"), Codec::Opus))
                    .unwrap();
                t.id
            })
            .collect();
        (release.id, tracks)
    }

    fn deezer(id: &str) -> RemoteId {
        RemoteId::new(Service::Deezer, id)
    }

    #[test]
    fn a_merge_that_empties_an_album_hands_its_ids_and_items_to_the_album_it_went_into() {
        let lib = Library::open_in_memory().unwrap();
        let oliver = lib.upsert_artist(&Artist::new("Oliver Tree")).unwrap();
        let (standard, small) = album(&lib, &oliver, "Ugly Is Beautiful", &["Me, Myself & I"]);
        let (deluxe, big) = album(&lib, &oliver, "Ugly Is Beautiful: Shorter, Thicker & Uglier (Deluxe)", &["Me, Myself & I", "Alien Boy"]);
        lib.set_external_id(EntityKind::Album, &deezer("160736802"), standard.0).unwrap();
        let mut wish = WishlistItem::new(WishlistStatus::Done, 1);
        wish.title = Some("Ugly Is Beautiful".into());
        wish.artist = Some("Oliver Tree".into());
        wish.scope = WishlistScope::Album;
        wish.release_id = Some(standard);
        lib.put_wishlist_item(&wish).unwrap();
        // Its own artist, credited nowhere else.
        let lonely = lib.upsert_artist(&Artist::new("Lonely Guest")).unwrap();
        lib.write(|tx| {
            tx.execute(
                "INSERT INTO track_artist(track_id, artist_id, position) VALUES (?1, ?2, 1)",
                params![small[0].to_string(), lonely.id.to_string()],
            )?;
            Ok(())
        })
        .unwrap();

        assert!(lib.merge_track_into(small[0], big[0]).unwrap());
        assert!(lib.get_release(standard).unwrap().is_none(), "the empty album went");
        assert_eq!(lib.release_by_remote(&deezer("160736802")).unwrap(), Some(deluxe), "its deezer id followed");
        assert_eq!(lib.get_wishlist_item(wish.id).unwrap().unwrap().release_id, Some(deluxe), "and its item");
        assert!(lib.get_artist(lonely.id).unwrap().is_none(), "an artist left without anything went");
        assert!(lib.get_artist(oliver.id).unwrap().is_some());
    }

    #[test]
    fn an_album_emptied_without_a_twin_drops_its_ids_and_a_purged_track_its_links() {
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let (release, tracks) = album(&lib, &a, "Звезда по имени Солнце", &["Кукушка"]);
        lib.set_external_id(EntityKind::Album, &deezer("1"), release.0).unwrap();
        lib.set_external_id(EntityKind::Track, &deezer("11"), tracks[0].0).unwrap();
        let mut wish = WishlistItem::new(WishlistStatus::Done, 1);
        wish.track_id = Some(tracks[0]);
        wish.release_id = Some(release);
        lib.put_wishlist_item(&wish).unwrap();
        lib.mark_track_deleted(tracks[0], 1).unwrap();
        lib.purge_track(tracks[0]).unwrap();
        assert!(lib.get_release(release).unwrap().is_none());
        assert_eq!(lib.external_id_local(EntityKind::Album, &deezer("1")).unwrap(), None);
        assert_eq!(lib.external_id_local(EntityKind::Track, &deezer("11")).unwrap(), None);
        let wish = lib.get_wishlist_item(wish.id).unwrap().unwrap();
        assert_eq!((wish.track_id, wish.release_id), (None, None));
        assert!(lib.get_artist(a.id).unwrap().is_none());
    }

    #[test]
    fn orphans_find_their_album_again_or_go_once() {
        let lib = Library::open_in_memory().unwrap();
        let oliver = lib.upsert_artist(&Artist::new("Oliver Tree")).unwrap();
        let (deluxe, big) = album(&lib, &oliver, "Ugly Is Beautiful: Shorter, Thicker & Uglier (Deluxe)", &["Alien Boy"]);
        let gone = ReleaseId::new();
        let gone_track = TrackId::new();
        lib.set_external_id(EntityKind::Album, &deezer("160736802"), gone.0).unwrap();
        lib.set_external_id(EntityKind::Album, &deezer("999"), ReleaseId::new().0).unwrap();
        lib.set_external_id(EntityKind::Track, &RemoteId::new(Service::Youtube, "xsdTzjogYBM"), big[0].0).unwrap();
        let mut by_album = WishlistItem::new(WishlistStatus::Done, 1);
        by_album.title = Some("Ugly Is Beautiful".into());
        by_album.artist = Some("Oliver Tree".into());
        by_album.scope = WishlistScope::Album;
        by_album.release_id = Some(gone);
        lib.put_wishlist_item(&by_album).unwrap();
        let mut by_track = WishlistItem::new(WishlistStatus::Done, 2);
        by_track.title = Some("Alien Boy".into());
        by_track.youtube_id = Some("xsdTzjogYBM".into());
        by_track.track_id = Some(gone_track);
        by_track.release_id = Some(gone);
        lib.put_wishlist_item(&by_track).unwrap();
        // An album left empty by an old merge, and its artist.
        let other = lib.upsert_artist(&Artist::new("Crunchyroll")).unwrap();
        let empty = lib.upsert_release(&Release::new("Solo Leveling", ArtistCredit::single("Crunchyroll", Some(other.id)))).unwrap();
        lib.set_meta_value(&format!("cache.tracklist.{gone}"), "1|found").unwrap();

        assert!(lib.drop_orphans().unwrap() >= 6);
        assert_eq!(lib.release_by_remote(&deezer("160736802")).unwrap(), Some(deluxe), "found again by its item");
        assert_eq!(lib.external_id_local(EntityKind::Album, &deezer("999")).unwrap(), None, "a dangling id went");
        assert_eq!(lib.get_wishlist_item(by_album.id).unwrap().unwrap().release_id, Some(deluxe));
        let by_track = lib.get_wishlist_item(by_track.id).unwrap().unwrap();
        assert_eq!((by_track.track_id, by_track.release_id), (Some(big[0]), Some(deluxe)), "found by its video");
        assert!(lib.get_release(empty.id).unwrap().is_none());
        assert!(lib.get_artist(other.id).unwrap().is_none());
        assert_eq!(lib.meta_value(&format!("cache.tracklist.{gone}")).unwrap(), None);
        assert!(lib.meta_value(ORPHANS_KEY).unwrap().is_some());
        assert_eq!(lib.drop_orphans().unwrap(), 0, "once");
    }

    #[test]
    fn a_repair_with_nothing_to_do_only_sets_its_mark() {
        let lib = Library::open_in_memory().unwrap();
        assert_eq!(lib.drop_orphans().unwrap(), 0);
        assert!(lib.meta_value(ORPHANS_KEY).unwrap().is_some());
        assert_eq!(lib.backup_before("x").unwrap(), None, "nothing to copy in memory");
    }

    #[test]
    fn a_backup_is_one_copy_of_the_library_next_to_it() {
        let dir = std::env::temp_dir().join(format!("nikgob-backup-{}", ReleaseId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = Library::open(dir.join("library.sqlite")).unwrap();
        let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let file = lib.backup_before("test.v1").unwrap().unwrap();
        assert!(file.starts_with(&dir) && file.exists());
        let copy = Library::open(&file).unwrap();
        assert!(copy.get_artist(a.id).unwrap().is_some());
        assert_eq!(lib.backup_before("test.v1").unwrap(), Some(file), "once a day");
        drop((lib, copy));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
