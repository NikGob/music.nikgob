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
    create_dir_all, move_file, prune_empty_dirs, remove_file, reserve_unique, unix_secs, ymd_from_unix,
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
/// lossless master, if any, is left alone until the track is purged.
pub fn delete_track_to_trash(
    library: &Library,
    trash_dir: &Path,
    _library_dir: &Path,
    track: TrackId,
) -> Result<Vec<PathBuf>> {
    let at = nikgob_library::now_ms();
    let mut moved = Vec::new();
    for source in library.sources_of_track(track)? {
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

    #[test]
    fn only_trash_names_are_purged() {
        assert!(is_trash_name(Path::new("C:/Trash/2026-10-10 01 - Song.opus")));
        assert!(!is_trash_name(Path::new("C:/Music/A/B/01 - Song.opus")));
        assert!(!is_trash_name(Path::new("C:/Trash/2026-10-10")));
    }
}
