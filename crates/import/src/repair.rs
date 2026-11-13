//! One-time repair of albums a torrent brought in pieces
//! (`torrent_identity_repair.v1`).
//!
//! Before folders were identified as albums, each file of a torrent was
//! looked up alone, and the confirm queue offered strangers' recordings of
//! the same title: «Ugly Is Beautiful» by Oliver Tree ended up as fourteen
//! singles of fourteen other artists. The repair:
//! * takes the files of one torrent folder; what their tags (read from the
//!   payload when it is still in Downloads), else the folder's, torrent's
//!   and files' names say about them gives the artist X and the album Y
//!   ([`Evidence`]);
//! * a track of the folder credited to artists that share nothing with X
//!   goes back to X / Y: onto the «Хочу скачать» release the torrent was
//!   fetched for (onto its stream track of the same number / title /
//!   length when it has one), else onto the release of X / Y; strangers'
//!   releases and artists it leaves empty go;
//! * the files of a torrent fetched for an album that landed on another
//!   release of the same title (a MusicBrainz edition next to the wishlist's
//!   one) join the wishlist's release, one album, not two;
//! * a «Хочу скачать» album whose tracks now all have files is «Скачано».
//!
//! With `move_files` the files follow their tracks (tags rewritten, moved
//! where PLAN §5 puts them, through [`Importer::retag`]); without it only
//! the database changes (a dry run on a copy of a library).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use nikgob_core::model::{
    ArtistId, Release, ReleaseId, Source, SourceOrigin, TrackId, WishlistItem, WishlistOrigin, WishlistScope,
    WishlistStatus,
};
use nikgob_library::{Library, TrackRow};
use nikgob_metadata::text::album_key;

use crate::album::{Evidence, Slot, file_is_track, names_of};
use crate::error::Result;
use crate::folder::{album_group_dir, same_title};
use crate::identity::{Identity, identity_from_tags, shares_artist};
use crate::importer::Importer;
use crate::retag::TagChanges;
use crate::sanitize::long_path;
use crate::store::Store;
use crate::tags::{self, TagSet};

/// `meta` key: the repair ran.
pub const TORRENT_IDENTITY_REPAIR: &str = "torrent_identity_repair.v1";

/// `meta` key: the artists `v1` took from tracks of a soundtrack (its rule
/// counted a composer other than the album artist as a stranger and gave
/// the track the album artist) are back ([`Importer::repair_torrent_artists`]).
pub const TORRENT_ARTIST_REPAIR: &str = "torrent_identity_repair.v2";

/// One track whose own artist came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistBack {
    pub track: TrackId,
    pub title: String,
    /// The album artist it had been given, and its own artist again.
    pub was: String,
    pub now: String,
}

/// One track the repair put back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
    pub track: TrackId,
    /// Title, artist and album it had.
    pub was: (String, String, String),
    /// Title, artist, album and number it has now.
    pub now: (String, String, String, u32),
    /// It became this track of the release (a stream track waiting for it).
    pub merged_into: Option<TrackId>,
}

/// What [`Importer::repair_torrent_identities`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityRepair {
    pub moved: Vec<Moved>,
    /// Releases of another title folded into the wishlist's release:
    /// `(from, into)` titles, and how many tracks became its tracks.
    pub folded: Vec<(String, String, usize)>,
    pub releases_removed: Vec<String>,
    pub artists_removed: Vec<String>,
    /// «Хочу скачать» albums now «Скачано».
    pub wishlist_done: Vec<String>,
    /// Files moved on disk.
    pub files_moved: usize,
}

/// One file of a torrent folder.
struct FolderFile {
    source: Source,
    track: TrackRow,
    /// What its tags, else its name, say.
    said: Identity,
}

/// A track of a torrent folder the library filed under another album than
/// the folder's `album`, credited (track and release) to nobody of
/// `artists`.
fn scattered(lib: &Library, f: &FolderFile, album: &str, artists: &[String]) -> bool {
    let on = f.track.album.as_deref().map(album_key).unwrap_or_default();
    let want = album_key(album);
    if on == want || same_title(&on, &want) {
        return false;
    }
    let release = f.track.release_id.and_then(|r| lib.get_release(r).ok().flatten());
    let mut lines = credit_lines(&f.track);
    lines.extend(release.map(|r| r.artist_credit.display()));
    let mut theirs = artists.to_vec();
    if f.said.artist_line != "Unknown Artist" {
        theirs.push(f.said.artist_line.clone());
    }
    !shares_artist(&lines, &theirs)
}

/// The release of a «Хочу скачать» item; one that went is found again by
/// the item's album title and artist, and the item points at it again.
pub fn wished_release(lib: &Library, item: &WishlistItem) -> Result<Option<Release>> {
    if let Some(r) = item.release_id.and_then(|r| lib.get_release(r).ok().flatten()) {
        return Ok(Some(r));
    }
    if item.scope != WishlistScope::Album {
        return Ok(None);
    }
    let Some(title) = item.title.as_deref() else { return Ok(None) };
    let Some(found) = lib.find_release_named(title, item.artist.as_deref().unwrap_or_default())? else { return Ok(None) };
    let mut relinked = item.clone();
    relinked.release_id = Some(found);
    lib.put_wishlist_item(&relinked)?;
    log::info!("torrent repair: «Хочу скачать» «{title}» points at {found} again");
    lib.get_release(found).map_err(Into::into)
}

/// A file of the folder's album that landed on another edition of it by
/// the same artist (the deluxe next to the standard) while its number in
/// the folder is free on `target`: it belongs to its own album's place.
fn misfiled(f: &FolderFile, album: &str, target: &Release, slots: &[TrackRow]) -> bool {
    let on = f.track.album.as_deref().map(album_key).unwrap_or_default();
    let want = album_key(album);
    if f.track.release_id == Some(target.id) || on == want || !same_title(&on, &want) || f.said.track_no == 0 {
        return false;
    }
    let place = (f.said.disc_no.max(1), f.said.track_no);
    !slots.iter().any(|s| s.has_local && (s.disc_no, s.track_no) == place)
}

/// The tags of a torrent file still in Downloads, else what its name says.
fn said_of(downloads: &Path, path_in_torrent: &str) -> Identity {
    let file = downloads.join(path_in_torrent);
    if long_path(&file).exists()
        && let Ok((tags, _)) = tags::read_tags(&file)
    {
        return identity_from_tags(&tags, &file);
    }
    identity_from_tags(&TagSet::default(), Path::new(path_in_torrent))
}

/// The credit lines of a track (display and every name).
fn credit_lines(t: &TrackRow) -> Vec<String> {
    let mut lines = vec![t.artist.clone()];
    lines.extend(t.credit.0.iter().map(|p| p.name.clone()));
    lines
}

/// Lines of `release` with a live file, and all the lines its page has
/// (its tracks, outside the library too, and the places none has): «13 из
/// 14» of a «Хочу скачать» album, as its page and card count.
pub fn release_files(lib: &Library, release: ReleaseId) -> Result<(usize, usize)> {
    Ok(lib
        .release_listing(release)?
        .map_or((0, 0), |l| (l.progress.local as usize, l.progress.total as usize)))
}

impl Importer {
    /// See the module docs. Runs once per library (a `meta` mark).
    pub fn repair_torrent_identities(&self, move_files: bool) -> Result<IdentityRepair> {
        let lib = self.library().clone();
        if lib.meta_value(TORRENT_IDENTITY_REPAIR)?.is_some() {
            return Ok(IdentityRepair::default());
        }
        let report = self.repair_identities(&lib, move_files)?;
        lib.set_meta_value(TORRENT_IDENTITY_REPAIR, &nikgob_library::now_ms().to_string())?;
        // Releases and artists went: the importer's name indexes start over.
        *self.inner.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Store::new(lib);
        Ok(report)
    }

    fn repair_identities(&self, lib: &Library, move_files: bool) -> Result<IdentityRepair> {
        let cfg = self.config();
        let mut report = IdentityRepair::default();
        // Torrent folders: (info hash, folder in the torrent) → files.
        let mut folders: BTreeMap<(String, PathBuf), Vec<FolderFile>> = BTreeMap::new();
        for source in lib.torrent_sources()? {
            let SourceOrigin::Torrent { info_hash, path_in_torrent } = &source.origin else { continue };
            if info_hash.is_empty() || path_in_torrent.is_empty() {
                continue;
            }
            let Some(track) = lib.get_track_row(source.track_id)? else { continue };
            let said = said_of(&cfg.downloads_dir, path_in_torrent);
            let dir = album_group_dir(Path::new(path_in_torrent));
            folders
                .entry((info_hash.clone(), dir))
                .or_default()
                .push(FolderFile { source, track, said });
        }
        let mut touched: Vec<TrackId> = Vec::new();
        let mut emptied: Vec<ReleaseId> = Vec::new();
        let mut credited: Vec<ArtistId> = Vec::new();
        let mut store = Store::new(lib.clone());
        for ((info_hash, dir), files) in &folders {
            if files.len() < 2 {
                continue;
            }
            let bases: Vec<&Identity> = files.iter().map(|f| &f.said).collect();
            let top = files
                .iter()
                .find_map(|f| match &f.source.origin {
                    SourceOrigin::Torrent { path_in_torrent, .. } => Some(path_in_torrent.clone()),
                    _ => None,
                });
            let names = names_of(dir, top.as_deref());
            let ev = Evidence::new(&bases, &names.iter().map(String::as_str).collect::<Vec<_>>());
            let (Some(artist), Some(album)) = (ev.album_artist.clone(), ev.album.clone()) else { continue };
            if ev.compilation {
                continue;
            }
            // Scattered: on a release of another album, under artists that
            // share nothing with the folder's. A track of the folder's own
            // album by another artist (a soundtrack's composer, a guest) is
            // where it belongs.
            let mut strangers: Vec<&FolderFile> = files
                .iter()
                .filter(|f| scattered(lib, f, &album, &ev.artists))
                .collect();
            // Files of the folder on another edition of its album.
            let editions = files.iter().any(|f| {
                let on = f.track.album.as_deref().map(album_key).unwrap_or_default();
                on != album_key(&album) && same_title(&on, &album_key(&album))
            });
            if strangers.is_empty() && !editions {
                continue;
            }
            // Where they belong: the wishlist's release, else X / Y.
            let item = lib
                .get_download(info_hash)?
                .and_then(|d| d.wishlist_id)
                .and_then(|w| lib.get_wishlist_item(w).ok().flatten());
            let wished = match &item {
                Some(item) => wished_release(lib, item)?,
                None => None,
            }
            .filter(|r| {
                let (a, b) = (album_key(&r.title), album_key(&album));
                (a == b || same_title(&a, &b)) && shares_artist(&[r.artist_credit.display()], &ev.artists)
            });
            let mut folder_id = ev_identity(&ev, &files[0].said, &artist, &album);
            let target = match wished {
                Some(r) => r,
                None => {
                    folder_id.release_id = None;
                    store.ensure_release(&folder_id, None)?
                }
            };
            let mut slots = lib.tracks_of_release_all(target.id)?;
            for f in files {
                if !strangers.iter().any(|s| s.source.id == f.source.id) && misfiled(f, &album, &target, &slots) {
                    strangers.push(f);
                }
            }
            for f in strangers {
                let mut said = f.said.clone();
                ev.fill(&mut said);
                let was = (
                    f.track.title.clone(),
                    f.track.artist.clone(),
                    f.track.album.clone().unwrap_or_default(),
                );
                if let Some(r) = f.track.release_id {
                    emptied.push(r);
                }
                credited.extend(f.track.credit.0.iter().filter_map(|p| p.artist_id));
                // A track of the release waiting for this file.
                let slot = slots
                    .iter()
                    .filter(|s| s.id != f.track.id && !s.has_local)
                    .filter_map(|s| Some((file_is_track(&said, f.track.duration_ms, Slot::from(s))?, s.id)))
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, id)| id);
                // A track with another file of its own (the deluxe's) keeps
                // it: only this file goes, onto a track of its own.
                let shared = lib
                    .sources_of_track(f.track.id)?
                    .iter()
                    .any(|s| s.id != f.source.id && s.kind.is_local() && s.trashed_at.is_none());
                let id = match slot {
                    Some(into) if shared => {
                        let mut s = f.source.clone();
                        s.track_id = into;
                        lib.add_source(&s)?;
                        into
                    }
                    Some(into) => {
                        lib.merge_track_into(f.track.id, into)?;
                        into
                    }
                    None if shared => {
                        let mut t = nikgob_core::model::Track::new(said.title.clone(), store.track_credit(&said)?);
                        t.release_id = Some(target.id);
                        t.disc_no = said.disc_no.max(1);
                        t.track_no = said.track_no;
                        t.duration_ms = f.track.duration_ms;
                        let t = lib.upsert_track(&t)?;
                        let mut s = f.source.clone();
                        s.track_id = t.id;
                        lib.add_source(&s)?;
                        t.id
                    }
                    None => {
                        let Some(mut t) = lib.get_track(f.track.id)? else { continue };
                        // The file's own artist (the folder's only when
                        // its tags name none).
                        t.title = said.title.clone();
                        t.artist_credit = store.track_credit(&said)?;
                        t.release_id = Some(target.id);
                        t.disc_no = said.disc_no.max(1);
                        t.track_no = said.track_no;
                        lib.upsert_track(&t)?;
                        t.id
                    }
                };
                touched.push(id);
                let row = lib.get_track_row(id)?;
                let now = row.map_or_else(
                    || (said.title.clone(), artist.clone(), target.title.clone(), said.track_no),
                    |r| (r.title, r.artist, r.album.unwrap_or_default(), r.track_no),
                );
                report.moved.push(Moved { track: f.track.id, was, now, merged_into: slot });
                slots = lib.tracks_of_release_all(target.id)?;
            }
        }
        // Albums of the wishlist split from their downloads.
        for d in lib.list_downloads()? {
            let Some(item) = d.wishlist_id.and_then(|w| lib.get_wishlist_item(w).ok().flatten()) else { continue };
            if item.scope != WishlistScope::Album {
                continue;
            }
            let Some(wished) = wished_release(lib, &item)? else { continue };
            let mut landed: Vec<ReleaseId> = Vec::new();
            for s in lib.torrent_sources()? {
                if matches!(&s.origin, SourceOrigin::Torrent { info_hash, .. } if *info_hash == d.info_hash)
                    && let Some(r) = lib.get_track(s.track_id)?.and_then(|t| t.release_id)
                    && r != wished.id
                    && !landed.contains(&r)
                {
                    landed.push(r);
                }
            }
            for r in landed {
                let Some(other) = lib.get_release(r)? else { continue };
                let same = album_key(&other.title) == album_key(&wished.title)
                    && shares_artist(&[other.artist_credit.display()], &[wished.artist_credit.display()]);
                if !same {
                    continue;
                }
                let theirs = lib.tracks_of_release_all(other.id)?;
                let mut slots = lib.tracks_of_release_all(wished.id)?;
                let mut merged = 0;
                for t in &theirs {
                    let said = identity_from_tags(
                        &TagSet {
                            title: Some(t.title.clone()),
                            track_no: Some(t.track_no),
                            disc_no: Some(t.disc_no),
                            ..TagSet::default()
                        },
                        Path::new("x"),
                    );
                    let slot = slots
                        .iter()
                        .filter(|s| !s.has_local)
                        .filter_map(|s| Some((file_is_track(&said, t.duration_ms, Slot::from(s))?, s.id)))
                        .max_by(|a, b| a.0.total_cmp(&b.0))
                        .map(|(_, id)| id);
                    let id = match slot {
                        Some(into) if lib.merge_track_into(t.id, into)? => {
                            merged += 1;
                            into
                        }
                        _ => t.id,
                    };
                    touched.push(id);
                    slots = lib.tracks_of_release_all(wished.id)?;
                }
                if lib.merge_releases(other.id, wished.id).is_err() {
                    lib.absorb_edition(other.id, wished.id)?;
                }
                report.folded.push((other.title.clone(), wished.title.clone(), merged));
            }
        }
        // Whatever the strangers left empty goes.
        emptied.sort();
        emptied.dedup();
        for r in emptied {
            let title = lib.get_release(r)?.map(|r| r.title);
            if lib.drop_empty_release(r)?
                && let Some(t) = title
            {
                report.releases_removed.push(t);
            }
        }
        let mut seen = HashSet::new();
        for a in credited {
            if !seen.insert(a) {
                continue;
            }
            let name = lib.get_artist(a)?.map(|a| a.name);
            if lib.drop_orphan_artist(a)?
                && let Some(n) = name
            {
                report.artists_removed.push(n);
            }
        }
        // «Скачано» once every track of the album has its file (an album
        // that looks for a better copy has its files from the start).
        for item in lib.list_wishlist()? {
            if item.scope != WishlistScope::Album
                || item.origin == WishlistOrigin::Upgrade
                || matches!(item.status, WishlistStatus::Done | WishlistStatus::Dismissed)
            {
                continue;
            }
            let Some(r) = wished_release(lib, &item)?.map(|r| r.id) else { continue };
            let (have, all) = release_files(lib, r)?;
            if all > 0 && have == all && touched.iter().any(|t| lib.get_track(*t).ok().flatten().and_then(|t| t.release_id) == Some(r)) {
                lib.set_wishlist_status(item.id, WishlistStatus::Done, Some("Скачано"))?;
                report.wishlist_done.push(item.title.clone().unwrap_or_default());
            }
        }
        if move_files {
            touched.sort();
            touched.dedup();
            for t in touched {
                match self.place_file(lib, t) {
                    Ok(true) => report.files_moved += 1,
                    Ok(false) => {}
                    Err(e) => log::warn!("torrent repair: the file of {t} stays where it is: {e}"),
                }
            }
        }
        Ok(report)
    }

    /// Undoes what the first rule of [`Self::repair_torrent_identities`]
    /// did to soundtracks and albums with guests: in a torrent folder whose
    /// tracks all sit on one release, a track credited to that release's
    /// artist whose original file (the torrent's, still in Downloads) names
    /// an artist that shares nothing with it gets that artist back, in the
    /// library and (with `move_files`) in its file's tags. Runs once per
    /// library (a `meta` mark), whether or not `v1` ran.
    pub fn repair_torrent_artists(&self, move_files: bool) -> Result<Vec<ArtistBack>> {
        let lib = self.library().clone();
        if lib.meta_value(TORRENT_ARTIST_REPAIR)?.is_some() {
            return Ok(Vec::new());
        }
        let back = self.restore_artists(&lib, move_files)?;
        lib.set_meta_value(TORRENT_ARTIST_REPAIR, &nikgob_library::now_ms().to_string())?;
        *self.inner.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Store::new(lib);
        Ok(back)
    }

    fn restore_artists(&self, lib: &Library, move_files: bool) -> Result<Vec<ArtistBack>> {
        let cfg = self.config();
        let mut folders: BTreeMap<(String, PathBuf), Vec<(Source, TrackRow)>> = BTreeMap::new();
        for source in lib.torrent_sources()? {
            let SourceOrigin::Torrent { info_hash, path_in_torrent } = &source.origin else { continue };
            if path_in_torrent.is_empty() {
                continue;
            }
            let Some(track) = lib.get_track_row(source.track_id)? else { continue };
            let dir = album_group_dir(Path::new(path_in_torrent));
            folders.entry((info_hash.clone(), dir)).or_default().push((source, track));
        }
        let mut back = Vec::new();
        let mut store = Store::new(lib.clone());
        for files in folders.values() {
            let Some(release) = files[0].1.release_id else { continue };
            if files.iter().any(|(_, t)| t.release_id != Some(release)) {
                continue;
            }
            let Some(r) = lib.get_release(release)? else { continue };
            let album_artist = r.artist_credit.display();
            for (source, track) in files {
                let SourceOrigin::Torrent { path_in_torrent, .. } = &source.origin else { continue };
                if nikgob_metadata::text::artist_key(&track.artist) != nikgob_metadata::text::artist_key(&album_artist) {
                    continue;
                }
                let file = cfg.downloads_dir.join(path_in_torrent);
                if !long_path(&file).exists() {
                    continue;
                }
                let Ok((tags, _)) = tags::read_tags(&file) else { continue };
                let Some(own) = tags.artist.clone().filter(|a| !a.trim().is_empty()) else { continue };
                if shares_artist(std::slice::from_ref(&own), std::slice::from_ref(&track.artist)) {
                    continue;
                }
                if move_files && lib.active_source(track.id)?.is_some_and(|s| s.kind.is_local() && long_path(Path::new(&s.locator)).exists()) {
                    let changes = TagChanges {
                        artist: Some(own.clone()),
                        album_artist: Some(album_artist.clone()),
                        keep_album_artist: true,
                        ..TagChanges::default()
                    };
                    self.retag(track.id, changes)?;
                } else {
                    let Some(mut t) = lib.get_track(track.id)? else { continue };
                    let mut id = identity_from_tags(&tags, &file);
                    id.artist_line = own.clone();
                    t.artist_credit = store.track_credit(&id)?;
                    lib.upsert_track(&t)?;
                }
                back.push(ArtistBack { track: track.id, title: track.title.clone(), was: track.artist.clone(), now: own });
            }
        }
        Ok(back)
    }

    /// Rewrites the tags of a track's file from the library (its title,
    /// album, place and the album's count; it stays on its album) and
    /// moves it where PLAN §5 puts it. Returns whether the file moved.
    pub fn place_track_file(&self, track: TrackId) -> Result<bool> {
        let lib = self.library().clone();
        self.place_file(&lib, track)
    }

    fn place_file(&self, lib: &Library, track: TrackId) -> Result<bool> {
        let Some(row) = lib.get_track_row(track)? else { return Ok(false) };
        let Some(src) = lib.active_source(track)?.filter(|s| s.kind.is_local()) else { return Ok(false) };
        if !long_path(Path::new(&src.locator)).exists() {
            return Ok(false);
        }
        let release = row.release_id.and_then(|r| lib.get_release(r).ok().flatten());
        let changes = TagChanges {
            title: Some(row.title.clone()),
            artist: Some(row.artist.clone()),
            album: release.as_ref().map(|r| r.title.clone()),
            album_artist: release.as_ref().map(|r| r.artist_credit.display()),
            year: release.as_ref().map(|r| r.year),
            track_no: Some((row.track_no > 0).then_some(row.track_no)),
            track_total: release.as_ref().and_then(|r| r.total_tracks).map(Some),
            disc_no: Some(Some(row.disc_no.max(1))),
            release_mbid: release.as_ref().map(|r| r.mbid.clone().unwrap_or_default()),
            release: row.release_id,
            keep_album_artist: true,
            ..TagChanges::default()
        };
        let new = self.retag(track, changes)?;
        Ok(new.as_path() != Path::new(&src.locator))
    }
}

/// The identity of the folder's album (X / Y) for finding or making its
/// release.
fn ev_identity(ev: &Evidence, first: &Identity, artist: &str, album: &str) -> Identity {
    let mut id = first.clone();
    ev.fill(&mut id);
    id.album = Some(album.to_owned());
    id.album_artist = Some(artist.to_owned());
    id.artist_line = artist.to_owned();
    id.artists = vec![artist.to_owned()];
    id.release_type = nikgob_core::model::ReleaseType::Album;
    id.compilation = false;
    id.release_mbid = None;
    id.release_group_mbid = None;
    id
}
