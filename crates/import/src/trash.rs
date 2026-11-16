//! Trash: nothing is deleted at once.
//!
//! Files move to `Trash/<YYYY-MM-DD> <name>`; the library marks the source as
//! trashed; [`trash_purge`] removes both after the retention period. A track
//! the user deletes leaves the library at once ([`delete_track_to_trash`]),
//! sits in the Trash view until [`restore_track`] or [`purge_track`], and
//! [`purge_deleted_tracks`] removes it after the retention period.

use std::path::{Path, PathBuf};

use nikgob_core::model::TrackId;
use nikgob_library::Library;

use crate::config::ImportConfig;
use crate::error::Result;
use crate::fsutil::{
    check_inside, create_dir_all, move_file, prune_empty_dirs, remove_file, reserve_unique, unix_secs, ymd_from_unix,
};
use crate::sanitize::{long_path, sanitize_component, truncate_chars};

/// Retention of trashed files, days.
pub const TRASH_DAYS: u32 = 30;

/// Moves `file` into `trash_dir` with a date prefix; returns the new location.
pub fn trash_file(trash_dir: &Path, file: &Path) -> Result<PathBuf> {
    create_dir_all(trash_dir)?;
    let (y, m, d) = ymd_from_unix(unix_secs());
    let name = file
        .file_name()
        .map(|n| sanitize_component(&n.to_string_lossy()))
        .unwrap_or_else(|| "file".to_owned());
    // The date prefix and a " (n)" suffix must still fit in one path component
    // (255 UTF-16 units on NTFS).
    let name = match name.rsplit_once('.') {
        Some((stem, ext)) if ext.len() <= 8 => format!("{}.{ext}", truncate_chars(stem, 150)),
        _ => truncate_chars(&name, 150),
    };
    // The name is claimed atomically: two files with the same name trashed at
    // once (two Inbox duplicates called `01.flac`) must not overwrite each other.
    let target = reserve_unique(&trash_dir.join(format!("{y:04}-{m:02}-{d:02} {name}")))?;
    if let Err(e) = move_file(file, &target) {
        let _ = remove_file(&target);
        return Err(e);
    }
    Ok(target)
}

/// Deletes a track into Trash: every live local file moves there, the sources
/// are marked trashed and the track deleted, all with one timestamp, so the
/// track leaves the library and [`restore_track`] knows what to bring back.
/// Returns the new locations. Lyrics next to a file go along. The album folder
/// keeps its cover (a restore needs it; [`purge_track`] tidies up). The
/// lossless master, if any, is left alone until the track is purged. A file
/// (or its lyrics) that resolves outside `library_dir` refuses the whole
/// track before anything moves.
pub fn delete_track_to_trash(
    library: &Library,
    trash_dir: &Path,
    library_dir: &Path,
    track: TrackId,
) -> Result<Vec<PathBuf>> {
    let at = nikgob_library::now_ms();
    let sources = library.sources_of_track(track)?;
    for path in trash_touches(&sources) {
        check_inside(&path, &[library_dir])?;
    }
    let mut moved = Vec::new();
    for source in sources {
        if !source.kind.is_local() {
            continue;
        }
        let path = PathBuf::from(&source.locator);
        let dest = if long_path(&path).exists() {
            let dest = trash_file(trash_dir, &path)?;
            let lrc = path.with_extension("lrc");
            if long_path(&lrc).exists() {
                let _ = trash_file(trash_dir, &lrc);
            }
            Some(dest)
        } else {
            None
        };
        library.trash_source_at(
            source.id,
            dest.as_ref().map(|d| d.to_string_lossy().into_owned()).as_deref(),
            at,
        )?;
        moved.extend(dest);
    }
    library.mark_track_deleted(track, at)?;
    Ok(moved)
}

/// Every file [`delete_track_to_trash`] would move for these sources: the
/// live local files and the lyrics next to them.
pub fn trash_touches(sources: &[nikgob_core::model::Source]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for source in sources.iter().filter(|s| s.kind.is_local()) {
        let path = PathBuf::from(&source.locator);
        if !long_path(&path).exists() {
            continue;
        }
        let lrc = path.with_extension("lrc");
        if long_path(&lrc).exists() {
            out.push(lrc);
        }
        out.push(path);
    }
    out
}

/// What [`restore_track`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    pub files_restored: usize,
    /// Files that could not go back (gone from Trash, or something else took
    /// their place in the library); their sources stay in Trash.
    pub files_failed: usize,
}

/// Takes a deleted track out of Trash: its files go back where they were and
/// the track shows up in the library again (without a file, if none could
/// return).
pub fn restore_track(library: &Library, track: TrackId) -> Result<RestoreReport> {
    let mut report = RestoreReport::default();
    for entry in library.deleted_track_sources(track)? {
        let Some(in_trash) = entry.trash_path.as_deref().map(PathBuf::from) else {
            // The file was already missing when it was deleted.
            continue;
        };
        let orig = PathBuf::from(&entry.source.locator);
        if let Some(dir) = orig.parent() {
            let _ = create_dir_all(dir);
        }
        // Something new took the old path (an upgrade): never overwrite it.
        let moved = long_path(&in_trash).exists()
            && !long_path(&orig).exists()
            && move_file(&in_trash, &orig).is_ok();
        if !moved {
            report.files_failed += 1;
            continue;
        }
        let lrc = in_trash.with_extension("lrc");
        if long_path(&lrc).exists() {
            let _ = move_file(&lrc, &orig.with_extension("lrc"));
        }
        library.restore_source(entry.source.id)?;
        report.files_restored += 1;
    }
    library.undelete_track(track)?;
    Ok(report)
}

/// Removes a track for good: its files (in Trash or still in the library, with
/// their lyrics), its lossless master in `.musiclib/masters/` and every row
/// ([`Library::purge_track`]). Nothing is left for a rescan to bring back.
pub fn purge_track(library: &Library, cfg: &ImportConfig, track: TrackId) -> Result<PurgeReport> {
    let purged = library.purge_track(track)?;
    let mut report = PurgeReport {
        rows: 1,
        ..PurgeReport::default()
    };
    let delete = |path: &Path, report: &mut PurgeReport| match std::fs::remove_file(long_path(path)) {
        Ok(()) => report.files_deleted += 1,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => report.files_missing += 1,
        Err(e) => {
            log::warn!("cannot delete {}: {e}", path.display());
            report.files_failed += 1;
        }
    };
    for p in &purged.trash_files {
        let path = Path::new(p);
        if !is_trash_name(path) {
            log::warn!("not deleting {p}: it is not a Trash file");
            continue;
        }
        delete(path, &mut report);
        let lrc = path.with_extension("lrc");
        if long_path(&lrc).exists() {
            let _ = std::fs::remove_file(long_path(&lrc));
        }
    }
    for p in &purged.live_files {
        let path = Path::new(p);
        if !path.starts_with(&cfg.library_dir) {
            log::warn!("not deleting {p}: it is outside the library folder");
            continue;
        }
        delete(path, &mut report);
        let lrc = path.with_extension("lrc");
        if long_path(&lrc).exists() {
            let _ = std::fs::remove_file(long_path(&lrc));
        }
    }
    // Album folders left without music (cover and lyrics only) go too.
    for p in &purged.places {
        if let Some(dir) = Path::new(p).parent()
            && dir.starts_with(&cfg.library_dir)
        {
            prune_empty_dirs(dir, &cfg.library_dir);
        }
    }
    if let Some(hash) = &purged.master_hash {
        let masters = cfg.masters_dir();
        // Under the masters' lock, and looked at again: an import may have
        // taken this very file (or a file of this hash) as its master since
        // the rows went, and keeps it then.
        let _writes = crate::importer::master_writes();
        if library.master_users(hash).unwrap_or(1) > 0 {
            log::info!("the master {hash} is another track's now: its files stay");
            return Ok(report);
        }
        let same_file = |path: &Path| crate::fsutil::hash_file(path).ok().as_deref() == Some(hash.as_str());
        // The `<hash>.json` sidecar always; a master kept the older way is
        // `<hash>.<ext>` (only while it holds that very file) with an
        // image's `<hash>.cue`. A master kept by name owns no flat file:
        // one there is another copy and stays. A master still seeding in
        // Downloads goes with the torrent's cleanup (no master row keeps it).
        if let Ok(rd) = std::fs::read_dir(long_path(&masters)) {
            for e in rd.flatten() {
                let path = e.path();
                if !path.file_stem().is_some_and(|s| s.to_string_lossy() == hash.as_str()) {
                    continue;
                }
                let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if ext == "json" || (purged.master_file.is_none() && ext == "cue") {
                    delete(&path, &mut report);
                } else if purged.master_file.is_none() {
                    if same_file(&path) {
                        delete(&path, &mut report);
                    } else {
                        log::warn!("not deleting {}: it is not the master it was", path.display());
                    }
                }
            }
        }
        // A master kept by name: only inside the masters folder and only
        // while it is still this very file; its album folder goes when
        // nothing but its cover is left.
        if let Some(file) = purged.master_file.as_deref().map(Path::new) {
            if !file.starts_with(&masters) {
                log::warn!("not deleting {}: it is outside the masters folder", file.display());
            } else if !long_path(file).exists() {
                report.files_missing += 1;
            } else if !same_file(file) {
                log::warn!("not deleting {}: it is not the master it was", file.display());
            } else {
                delete(file, &mut report);
                if let Some(dir) = file.parent() {
                    prune_empty_dirs(dir, &masters);
                }
            }
        }
    }
    Ok(report)
}

/// Purges tracks deleted more than `days` ago (the Trash retention).
pub fn purge_deleted_tracks(library: &Library, cfg: &ImportConfig, days: u32) -> Result<PurgeReport> {
    let cutoff = nikgob_library::now_ms() - i64::from(days) * 24 * 60 * 60 * 1000;
    let mut total = PurgeReport::default();
    for track in library.deleted_tracks_before(cutoff)? {
        let r = purge_track(library, cfg, track)?;
        total.rows += r.rows;
        total.files_deleted += r.files_deleted;
        total.files_missing += r.files_missing;
        total.files_failed += r.files_failed;
    }
    Ok(total)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    /// Rows removed from the database.
    pub rows: usize,
    /// Files deleted from disk.
    pub files_deleted: usize,
    /// Files that were already gone.
    pub files_missing: usize,
    pub files_failed: usize,
}

/// Does `path` look like something [`trash_file`] produced (`YYYY-MM-DD name`)?
///
/// A source trashed while its file was already missing has no trash path, and
/// the library then reports its *original* location. Something else (an
/// upgrade, a re-import) may live at that location by now and must not be
/// deleted.
fn is_trash_name(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy()) else {
        return false;
    };
    let b = name.as_bytes();
    b.len() > 11
        && b[..10]
            .iter()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
        && b[10] == b' '
}

/// Moves the lossless master files of `file_hash` this computer keeps (by
/// name, or flat as `{hash}.<ext>` in `masters_dir`) to the Trash, each
/// noted with where it was (`master_trash`): «Отменить» puts it back
/// ([`restore_trashed_master`]) and the Trash's days end it
/// ([`purge_trashed_masters`]). The master rows stay: other devices hold
/// the file. Only a file inside the masters folder that still is that very
/// file moves; the `.json` sidecar and a disc image's `.cue` stay where
/// they are. The ids of the notes.
pub fn trash_master_files(library: &Library, trash_dir: &Path, masters_dir: &Path, file_hash: &str) -> Result<Vec<i64>> {
    // Under the masters' lock: an import does not take or lay out a master
    // of this file meanwhile.
    let _writes = crate::importer::master_writes();
    let mut files: Vec<PathBuf> = library
        .master_files_of_hash(file_hash)?
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| long_path(p).is_file())
        .collect();
    if let Some(flat) = crate::master_files::flat_master(masters_dir, file_hash)
        && !files.contains(&flat)
    {
        files.push(flat);
    }
    let mut ids = Vec::new();
    for path in files {
        if !crate::fsutil::inside_roots(&path, &[masters_dir]) {
            log::warn!("not moving {} to the Trash: it is outside the masters folder", path.display());
            continue;
        }
        if crate::fsutil::hash_file(&path).ok().as_deref() != Some(file_hash) {
            log::warn!("not moving {} to the Trash: it is not the master it was", path.display());
            continue;
        }
        let size = crate::fsutil::file_size(&path)?;
        let dest = trash_file(trash_dir, &path)?;
        match library.put_master_trash(file_hash, &path.to_string_lossy(), &dest.to_string_lossy(), size) {
            Ok(id) => ids.push(id),
            Err(e) => {
                // A file in the Trash nothing knows of could never come
                // back: it goes back to its place at once.
                if let Err(back) = move_file(&dest, &path) {
                    log::error!("{} stays in the Trash as {}: {back}", path.display(), dest.display());
                }
                return Err(e.into());
            }
        }
    }
    Ok(ids)
}

/// Puts the master file of note `id` back where it was. False when there
/// is nothing to bring back (no such note, the file gone from the Trash)
/// or something else took its place (never overwritten: it stays in the
/// Trash, noted).
pub fn restore_trashed_master(library: &Library, id: i64) -> Result<bool> {
    let Some(row) = library.master_trash_row(id)? else {
        return Ok(false);
    };
    let (from, to) = (PathBuf::from(&row.trash_path), PathBuf::from(&row.path));
    if !long_path(&from).exists() {
        library.remove_master_trash(id)?;
        return Ok(false);
    }
    if long_path(&to).exists() {
        return Ok(false);
    }
    if let Some(dir) = to.parent() {
        create_dir_all(dir)?;
    }
    move_file(&from, &to)?;
    library.remove_master_trash(id)?;
    Ok(true)
}

/// Deletes the master files moved to the Trash more than `days` ago (only
/// a Trash file's name is ever deleted) and their notes.
pub fn purge_trashed_masters(library: &Library, days: u32) -> Result<PurgeReport> {
    let cutoff = nikgob_library::now_ms() - i64::from(days) * 24 * 60 * 60 * 1000;
    let mut report = PurgeReport::default();
    for row in library.master_trash_before(cutoff)? {
        let path = Path::new(&row.trash_path);
        if !is_trash_name(path) {
            log::warn!("not deleting {}: it is not a Trash file", row.trash_path);
        } else {
            match std::fs::remove_file(long_path(path)) {
                Ok(()) => report.files_deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => report.files_missing += 1,
                Err(e) => {
                    log::warn!("cannot delete {}: {e}", row.trash_path);
                    report.files_failed += 1;
                    continue;
                }
            }
        }
        library.remove_master_trash(row.id)?;
        report.rows += 1;
    }
    Ok(report)
}

/// Deletes trashed sources older than `days` and their files.
pub fn trash_purge(library: &Library, days: u32) -> Result<PurgeReport> {
    let purged = library.purge_trash(days)?;
    let mut report = PurgeReport {
        rows: purged.len(),
        ..PurgeReport::default()
    };
    for p in purged {
        let Some(path) = p.path else {
            continue;
        };
        if !is_trash_name(Path::new(&path)) {
            log::warn!("not deleting {path}: it is not a Trash file");
            continue;
        }
        match std::fs::remove_file(long_path(Path::new(&path))) {
            Ok(()) => report.files_deleted += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => report.files_missing += 1,
            Err(e) => {
                log::warn!("cannot delete {path}: {e}");
                report.files_failed += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A master let go of moves to the Trash (only while it is that file,
    /// only from the masters folder), comes back with «Отменить» (never
    /// over a file that took its place), and its rows stay throughout.
    #[test]
    fn a_master_goes_to_the_trash_and_comes_back() {
        let root = tempfile::tempdir().unwrap();
        let (masters, trash) = (root.path().join("masters"), root.path().join("Trash"));
        let named = masters.join("A").join("2001 - B").join("01 - C.flac");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        let song = b"fLaC master".to_vec();
        std::fs::write(&named, &song).unwrap();
        let hash = crate::fsutil::hash_file(&named).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let artist = lib.upsert_artist(&nikgob_core::model::Artist::new("A")).unwrap();
        let credit = nikgob_core::model::ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let t = lib.upsert_track(&nikgob_core::model::Track::new("C", credit)).unwrap();
        lib.put_master(&nikgob_core::model::Master {
            track_id: t.id,
            codec: nikgob_core::model::Codec::Flac,
            file_hash: hash.clone(),
            size_bytes: song.len() as u64,
            origin: None,
            file_path: Some(named.to_string_lossy().into_owned()),
            replicas: Vec::new(),
        })
        .unwrap();

        let ids = trash_master_files(&lib, &trash, &masters, &hash).unwrap();
        assert_eq!(ids.len(), 1);
        assert!(!named.exists());
        let row = lib.master_trash_row(ids[0]).unwrap().unwrap();
        assert!(is_trash_name(Path::new(&row.trash_path)));
        assert_eq!(std::fs::read(&row.trash_path).unwrap(), song);
        assert!(lib.get_master(t.id).unwrap().is_some(), "the master row stays");
        assert!(trash_master_files(&lib, &trash, &masters, &hash).unwrap().is_empty(), "nothing left to move");

        std::fs::write(&named, b"something new").unwrap();
        assert!(!restore_trashed_master(&lib, ids[0]).unwrap(), "never over another file");
        std::fs::remove_file(&named).unwrap();
        assert!(restore_trashed_master(&lib, ids[0]).unwrap());
        assert_eq!(std::fs::read(&named).unwrap(), song);
        assert!(lib.master_trash_row(ids[0]).unwrap().is_none());

        // A file that is not the master any more stays where it is.
        std::fs::write(&named, b"changed").unwrap();
        assert!(trash_master_files(&lib, &trash, &masters, &hash).unwrap().is_empty());
        assert!(named.exists());
    }

    /// The Trash's days end a master let go of: its file and its note.
    #[test]
    fn masters_in_the_trash_go_after_their_days() {
        let root = tempfile::tempdir().unwrap();
        let trash = root.path().join("Trash");
        std::fs::create_dir_all(&trash).unwrap();
        let file = trash.join("2026-10-01 01 - C.flac");
        std::fs::write(&file, b"x").unwrap();
        let lib = Library::open_in_memory().unwrap();
        let id = lib.put_master_trash("h", "/m/01 - C.flac", &file.to_string_lossy(), 1).unwrap();
        assert_eq!(purge_trashed_masters(&lib, 30).unwrap().rows, 0, "still in its days");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let r = purge_trashed_masters(&lib, 0).unwrap();
        assert_eq!((r.rows, r.files_deleted), (1, 1));
        assert!(!file.exists());
        assert!(lib.master_trash_row(id).unwrap().is_none());
    }

    #[test]
    fn same_named_files_never_overwrite_each_other_in_trash() {
        let root = tempfile::tempdir().unwrap();
        let trash = root.path().join("Trash");
        let mut handles = Vec::new();
        for i in 0..8 {
            let dir = root.path().join(format!("album{i}"));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("01.flac");
            std::fs::write(&file, format!("content {i}")).unwrap();
            let trash = trash.clone();
            handles.push(std::thread::spawn(move || trash_file(&trash, &file).unwrap()));
        }
        let moved: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let mut contents: Vec<String> = moved
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        contents.sort();
        let expected: Vec<String> = (0..8).map(|i| format!("content {i}")).collect();
        assert_eq!(contents, expected);
        assert!(moved.iter().all(|p| is_trash_name(p)));
    }

    /// A purge deletes a master's own file only: one kept by name leaves a
    /// flat file of the same hash alone, and a flat file that no longer
    /// holds the master's content stays.
    #[test]
    fn a_purge_deletes_only_the_master_s_own_file() {
        use nikgob_core::model::{ArtistCredit, Codec, Master, SourceOrigin, Track};
        let dir = tempfile::tempdir().unwrap();
        let cfg = ImportConfig::with_root(dir.path());
        let masters = cfg.masters_dir();
        let lib = Library::open_in_memory().unwrap();
        let song = b"the song".to_vec();
        let hash = blake3::hash(&song).to_hex().to_string();
        let named = masters.join("A").join("2001 - B").join("01 - C.flac");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        std::fs::write(&named, &song).unwrap();
        std::fs::write(named.with_file_name("cover.jpg"), b"jpeg").unwrap();
        let flat = masters.join(format!("{hash}.flac"));
        std::fs::write(&flat, &song).unwrap();
        std::fs::write(masters.join(format!("{hash}.json")), b"{}").unwrap();
        let master = |track: TrackId, file_path: Option<String>| Master {
            track_id: track,
            codec: Codec::Flac,
            file_hash: hash.clone(),
            size_bytes: 8,
            origin: Some(SourceOrigin::Inbox),
            file_path,
            replicas: Vec::new(),
        };
        let t = lib.upsert_track(&Track::new("C", ArtistCredit::single("A", None))).unwrap();
        lib.put_master(&master(t.id, Some(named.to_string_lossy().into_owned()))).unwrap();
        purge_track(&lib, &cfg, t.id).unwrap();
        assert!(!named.exists());
        assert!(!masters.join("A").exists(), "the album folder with only its cover goes");
        assert!(!masters.join(format!("{hash}.json")).exists());
        assert_eq!(std::fs::read(&flat).unwrap(), song, "another copy, not this master's");

        // A flat master whose file changed: it stays.
        std::fs::write(&flat, b"damaged").unwrap();
        let t = lib.upsert_track(&Track::new("D", ArtistCredit::single("A", None))).unwrap();
        lib.put_master(&master(t.id, None)).unwrap();
        purge_track(&lib, &cfg, t.id).unwrap();
        assert_eq!(std::fs::read(&flat).unwrap(), b"damaged");
        // Its very file goes.
        std::fs::write(&flat, &song).unwrap();
        let t = lib.upsert_track(&Track::new("E", ArtistCredit::single("A", None))).unwrap();
        lib.put_master(&master(t.id, None)).unwrap();
        purge_track(&lib, &cfg, t.id).unwrap();
        assert!(!flat.exists());
    }

    /// A purge whose rows are gone waits for the masters' lock; an import
    /// that took the very file as another track's master meanwhile (under
    /// that lock) keeps it: the purge looks again and leaves the file, its
    /// sidecar and the new row alone.
    #[test]
    fn a_purge_leaves_a_master_an_import_took_meanwhile() {
        use nikgob_core::model::{ArtistCredit, Codec, Master, SourceOrigin, Track};
        let dir = tempfile::tempdir().unwrap();
        let cfg = ImportConfig::with_root(dir.path());
        let masters = cfg.masters_dir();
        let lib = Library::open(dir.path().join("library.sqlite")).unwrap();
        let song = b"the song".to_vec();
        let hash = blake3::hash(&song).to_hex().to_string();
        let file = masters.join("A").join("2001 - B").join("01 - C.flac");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &song).unwrap();
        let sidecar = masters.join(format!("{hash}.json"));
        std::fs::write(&sidecar, b"{}").unwrap();
        let master = |track: TrackId| Master {
            track_id: track,
            codec: Codec::Flac,
            file_hash: hash.clone(),
            size_bytes: 8,
            origin: Some(SourceOrigin::Inbox),
            file_path: Some(file.to_string_lossy().into_owned()),
            replicas: Vec::new(),
        };
        let gone = lib.upsert_track(&Track::new("C", ArtistCredit::single("A", None))).unwrap();
        let other = lib.upsert_track(&Track::new("C (live)", ArtistCredit::single("A", None))).unwrap();
        lib.put_master(&master(gone.id)).unwrap();

        // The import holds the lock, as a registration does.
        let writes = crate::importer::master_writes();
        let purge = {
            let (lib, cfg) = (lib.clone(), cfg.clone());
            std::thread::spawn(move || purge_track(&lib, &cfg, gone.id).unwrap())
        };
        // The purge's rows are gone; it waits for the lock now.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while lib.get_master(gone.id).unwrap().is_some() {
            assert!(std::time::Instant::now() < deadline, "the purge did not start");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // The import finds the file there and takes it as its master.
        lib.put_master(&master(other.id)).unwrap();
        drop(writes);
        purge.join().unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), song, "the other track's master stays");
        assert!(sidecar.exists());
        assert_eq!(lib.get_master(other.id).unwrap(), Some(master(other.id)));
    }

    #[test]
    fn only_trash_names_are_purged() {
        assert!(is_trash_name(Path::new("C:/Trash/2026-10-10 01 - Song.opus")));
        assert!(!is_trash_name(Path::new("C:/Music/A/B/01 - Song.opus")));
        assert!(!is_trash_name(Path::new("C:/Trash/2026-10-10")));
    }
}
