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
    check_inside, create_dir_all, move_file, move_new_verified, prune_empty_dirs, remove_file, reserve_unique, unix_secs, ymd_from_unix,
};
use crate::sanitize::{long_path, sanitize_component, truncate_chars};

/// Retention of trashed files, days.
pub const TRASH_DAYS: u32 = 30;

/// `file`'s name in the Trash: today's date before it (`YYYY-MM-DD name`).
fn dated_name(file: &Path) -> String {
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
    format!("{y:04}-{m:02}-{d:02} {name}")
}

/// Moves `file` into `trash_dir` with a date prefix; returns the new location.
pub fn trash_file(trash_dir: &Path, file: &Path) -> Result<PathBuf> {
    create_dir_all(trash_dir)?;
    // The name is claimed atomically: two files with the same name trashed at
    // once (two Inbox duplicates called `01.flac`) must not overwrite each other.
    let target = reserve_unique(&trash_dir.join(dated_name(file)))?;
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

/// What moving masters to the Trash did: the notes of the files moved
/// (every one, also when a later one failed) and the first failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MasterMoves {
    pub ids: Vec<i64>,
    pub error: Option<String>,
}

/// The caller's lock around the last look before a master file goes and
/// its removal: what voids the move (a device forgotten, another choice)
/// takes the same lock, so it lands before or after a removal, never
/// between the look and it.
pub trait MoveGate {
    /// Runs `step` holding the lock when the look under it still lets the
    /// file go; None when it does not.
    fn through(&self, step: &mut dyn FnMut() -> std::io::Result<()>) -> Option<std::io::Result<()>>;
}

/// A plain look (tests, a caller without a lock): the step runs when it
/// says yes.
impl<F: Fn() -> bool> MoveGate for F {
    fn through(&self, step: &mut dyn FnMut() -> std::io::Result<()>) -> Option<std::io::Result<()>> {
        if self() { Some(step()) } else { None }
    }
}

/// Moves exactly `files`, the lossless master files of `file_hash` the
/// owner confirmed, to the Trash: «Отменить» puts them back
/// ([`restore_trashed_master`]) and the Trash's days end them
/// ([`purge_trashed_masters`]). The master rows stay: other devices hold
/// the file. Each one must be a plain file inside the masters folder that
/// still is that very file; it is noted first (on its way), put in the
/// Trash (never over a file there; a hard link, or a copy checked by its
/// BLAKE3), and only then, through `gate`, the original goes and the note
/// is marked done. The `.json` sidecar and a disc image's `.cue` stay
/// where they are.
pub fn trash_master_files(library: &Library, trash_dir: &Path, masters_dir: &Path, file_hash: &str, files: &[PathBuf], gate: &dyn MoveGate) -> MasterMoves {
    // Under the masters' lock: an import does not take or lay out a master
    // of this file meanwhile.
    let _writes = crate::importer::master_writes();
    let mut out = MasterMoves::default();
    if let Err(e) = create_dir_all(trash_dir) {
        out.error = Some(e.to_string());
        return out;
    }
    for path in files {
        if !crate::fsutil::plain_file_inside(path, masters_dir) {
            log::warn!("not moving {} to the Trash: it is not a file inside the masters folder", path.display());
            continue;
        }
        if crate::fsutil::hash_file(path).ok().as_deref() != Some(file_hash) {
            log::warn!("not moving {} to the Trash: it is not the master it was", path.display());
            continue;
        }
        match trash_one(library, trash_dir, path, file_hash, gate) {
            Ok(Some(id)) => out.ids.push(id),
            Ok(None) => {
                out.error = Some("копий на устройствах уже меньше нужного или выбор изменился".to_owned());
                break;
            }
            Err(e) => {
                out.error = Some(format!("{}: {e}", path.display()));
                break;
            }
        }
    }
    out
}

/// The name of the link a file is put in the Trash through.
fn link_name(dest: &Path) -> PathBuf {
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dest.with_file_name(format!(".{name}.nikgob-link"))
}

/// Puts the content of `src` into `dest` (an empty file claimed for it): a
/// hard link put over the claim on one volume (made only where nothing has
/// the link's name); else a copy into it, flushed and checked by its size
/// and BLAKE3.
fn fill(src: &Path, dest: &Path, hash: &str) -> std::io::Result<()> {
    let link = link_name(dest);
    if std::fs::hard_link(long_path(src), long_path(&link)).is_ok() {
        return std::fs::rename(long_path(&link), long_path(dest)).inspect_err(|_| {
            let _ = std::fs::remove_file(long_path(&link));
        });
    }
    let size = std::fs::metadata(long_path(src))?.len();
    {
        let mut w = std::fs::OpenOptions::new().write(true).truncate(true).open(long_path(dest))?;
        let mut r = std::fs::File::open(long_path(src))?;
        std::io::copy(&mut r, &mut w)?;
        w.sync_all()?;
    }
    let same = std::fs::metadata(long_path(dest)).map(|m| m.len()).ok() == Some(size)
        && crate::fsutil::hash_file(dest).ok().is_some_and(|h| h.eq_ignore_ascii_case(hash));
    if same { Ok(()) } else { Err(std::io::Error::other("проверка хеша не прошла")) }
}

/// One master file to the Trash: a name there claimed, the note written
/// (on its way), the file put there, and then, through `gate`, the
/// original removed (still the file looked at) and the note marked done.
/// Anything short of the removal is undone (the claimed name and the note
/// go). The note's id; None when the gate said no.
fn trash_one(library: &Library, trash_dir: &Path, path: &Path, hash: &str, gate: &dyn MoveGate) -> std::result::Result<Option<i64>, String> {
    let meta = std::fs::symlink_metadata(long_path(path)).map_err(|e| e.to_string())?;
    let (size, mtime) = (meta.len(), meta.modified().ok());
    let dest = reserve_unique(&trash_dir.join(dated_name(path))).map_err(|e| e.to_string())?;
    let id = match library.begin_master_trash(hash, &path.to_string_lossy(), &dest.to_string_lossy(), size) {
        Ok(id) => id,
        Err(e) => {
            let _ = std::fs::remove_file(long_path(&dest));
            return Err(e.to_string());
        }
    };
    let undo = || {
        let _ = std::fs::remove_file(long_path(&dest));
        if let Err(e) = library.remove_master_trash(id) {
            log::warn!("the note of {} stays (on its way; settled at the next start): {e}", dest.display());
        }
    };
    if let Err(e) = fill(path, &dest, hash) {
        undo();
        return Err(e.to_string());
    }
    let mut step = || -> std::io::Result<()> {
        let now = std::fs::symlink_metadata(long_path(path))?;
        if !now.file_type().is_file() || now.len() != size || now.modified().ok() != mtime {
            return Err(std::io::Error::other("файл изменился"));
        }
        std::fs::remove_file(long_path(path))?;
        library.finish_master_trash(id).map_err(|e| std::io::Error::other(e.to_string()))
    };
    match gate.through(&mut step) {
        Some(Ok(())) => Ok(Some(id)),
        // The original went, the note was not marked: the file is in the
        // Trash with its note, settled as done at the next start.
        Some(Err(e)) if !long_path(path).exists() => {
            log::warn!("{} is in the Trash; its note is settled at the next start: {e}", dest.display());
            Ok(Some(id))
        }
        Some(Err(e)) => {
            undo();
            Err(e.to_string())
        }
        None => {
            undo();
            Ok(None)
        }
    }
}

/// A file as the settling of a cut move finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    /// That very file (a plain file, its BLAKE3 the note's).
    That,
    /// Something else has the name.
    Other,
    /// Nothing has the name.
    Gone,
    /// Not known: not readable, its disk away…
    Unknown,
}

/// What is at `path`, against the BLAKE3 `hash`.
fn found_at(path: &Path, hash: &str) -> Found {
    match std::fs::symlink_metadata(long_path(path)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Gone,
        Err(_) => Found::Unknown,
        Ok(m) if !m.file_type().is_file() => Found::Other,
        Ok(_) => match crate::fsutil::hash_file(path) {
            Ok(h) if h.eq_ignore_ascii_case(hash) => Found::That,
            Ok(_) => Found::Other,
            Err(_) => Found::Unknown,
        },
    }
}

/// Settles the master files left on their way to the Trash (a move cut
/// short), from what can be verified now:
/// - the original there, still that very file → the file put in the Trash
///   for it and the note go;
/// - the original's name taken by another file, or nothing there, and the
///   Trash file that very file → the note is done (a Trash entry with
///   «Отменить»);
/// - anything else (a file not readable, a disk away, the Trash folder
///   not as it should be) → the note stays on its way, told in the log,
///   looked at again at the next start; nothing is deleted.
///
/// The link a move went through goes only by its name derived from such a
/// note, a plain file right in the Trash folder (itself no link), and only
/// when that very file is still in one of the two places. How many notes
/// were settled.
pub fn recover_master_moves(library: &Library, trash_dir: &Path) -> Result<usize> {
    let mut n = 0;
    for row in library.master_trash_moving()? {
        let (src, dest) = (Path::new(&row.path), Path::new(&row.trash_path));
        let original = found_at(src, &row.file_hash);
        let in_place = trash_root_ok(trash_dir) && dest.parent() == Some(trash_dir);
        let trashed = match found_at(dest, &row.file_hash) {
            // A file there counts only as the Trash's own.
            Found::That | Found::Other if !in_trash(dest, trash_dir) => Found::Unknown,
            Found::Gone if !in_place => Found::Unknown,
            f => f,
        };
        if original == Found::That || trashed == Found::That {
            let link = link_name(dest);
            if in_trash_dir(&link, trash_dir) {
                let _ = std::fs::remove_file(long_path(&link));
            }
        }
        let settled = match (original, trashed) {
            (Found::That, Found::That | Found::Other) => match std::fs::remove_file(long_path(dest)) {
                Ok(()) => library.remove_master_trash(row.id)?,
                Err(e) => {
                    log::warn!("{} stays on its way to the Trash: {} not removed: {e}", row.path, row.trash_path);
                    false
                }
            },
            (Found::That, Found::Gone) => library.remove_master_trash(row.id)?,
            (Found::Other | Found::Gone, Found::That) => {
                library.finish_master_trash(row.id)?;
                true
            }
            (original, trashed) => {
                log::warn!(
                    "{} stays on its way to the Trash (the original {original:?}, the Trash file {} {trashed:?}); looked at again at the next start",
                    row.path,
                    row.trash_path
                );
                false
            }
        };
        if settled {
            n += 1;
        }
    }
    Ok(n)
}

/// Puts the master file of note `id` back where it was. False when there
/// is nothing to bring back (no such note, the file gone from the Trash)
/// or something else took its place: never replaced (the place is claimed
/// atomically), the file stays in the Trash, noted.
pub fn restore_trashed_master(library: &Library, id: i64) -> Result<bool> {
    let Some(row) = library.master_trash_row(id)? else {
        return Ok(false);
    };
    let (from, to) = (PathBuf::from(&row.trash_path), PathBuf::from(&row.path));
    if !long_path(&from).exists() {
        library.remove_master_trash(id)?;
        return Ok(false);
    }
    if let Some(dir) = to.parent() {
        create_dir_all(dir)?;
    }
    match move_new_verified(&from, &to, &row.file_hash) {
        Ok(()) => {
            library.remove_master_trash(id)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(crate::error::ImportError::io(&to, e)),
    }
}

/// Whether the Trash folder at `trash_dir` is a folder itself (not a link
/// or a junction).
fn trash_root_ok(trash_dir: &Path) -> bool {
    std::fs::symlink_metadata(long_path(trash_dir)).is_ok_and(|m| m.is_dir())
}

/// Whether `path` is a plain file (not a link) right in the Trash folder
/// at `trash_dir` ([`trash_root_ok`]), resolved still inside it.
fn in_trash_dir(path: &Path, trash_dir: &Path) -> bool {
    trash_root_ok(trash_dir)
        && path.parent() == Some(trash_dir)
        && std::fs::symlink_metadata(long_path(path)).is_ok_and(|m| m.file_type().is_file())
        && match (std::fs::canonicalize(long_path(path)), std::fs::canonicalize(long_path(trash_dir))) {
            (Ok(p), Ok(r)) => p.starts_with(r),
            _ => false,
        }
}

/// Whether `path` is a file the Trash at `trash_dir` holds: a plain file
/// with a Trash name right in that folder ([`in_trash_dir`]).
fn in_trash(path: &Path, trash_dir: &Path) -> bool {
    is_trash_name(path) && in_trash_dir(path, trash_dir)
}

/// Deletes the master files moved to the Trash more than `days` ago and
/// their notes: only a file that is in the Trash at `trash_dir`
/// ([`in_trash`]); a note whose file is elsewhere stays.
pub fn purge_trashed_masters(library: &Library, trash_dir: &Path, days: u32) -> Result<PurgeReport> {
    let cutoff = nikgob_library::now_ms() - i64::from(days) * 24 * 60 * 60 * 1000;
    let mut report = PurgeReport::default();
    for row in library.master_trash_before(cutoff)? {
        let path = Path::new(&row.trash_path);
        match std::fs::symlink_metadata(long_path(path)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => report.files_missing += 1,
            Err(e) => {
                log::warn!("cannot read {}: {e}", row.trash_path);
                report.files_failed += 1;
                continue;
            }
            Ok(_) if !in_trash(path, trash_dir) => {
                log::warn!("not deleting {}: it is not in the Trash", row.trash_path);
                report.files_failed += 1;
                continue;
            }
            Ok(_) => match std::fs::remove_file(long_path(path)) {
                Ok(()) => report.files_deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => report.files_missing += 1,
                Err(e) => {
                    log::warn!("cannot delete {}: {e}", row.trash_path);
                    report.files_failed += 1;
                    continue;
                }
            },
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

        let yes = || true;
        // Only the files confirmed move: a second copy not on the card stays.
        let copy = masters.join("A").join("2001 - B").join("01 - C (2).flac");
        std::fs::write(&copy, &song).unwrap();
        let moved = trash_master_files(&lib, &trash, &masters, &hash, std::slice::from_ref(&named), &yes);
        assert_eq!((moved.ids.len(), moved.error.as_deref()), (1, None));
        let ids = moved.ids;
        assert!(!named.exists());
        assert!(copy.exists(), "not on the card, not moved");
        let row = lib.master_trash_row(ids[0]).unwrap().unwrap();
        assert!(is_trash_name(Path::new(&row.trash_path)));
        assert_eq!(std::fs::read(&row.trash_path).unwrap(), song);
        assert!(lib.get_master(t.id).unwrap().is_some(), "the master row stays");
        assert!(trash_master_files(&lib, &trash, &masters, &hash, std::slice::from_ref(&named), &yes).ids.is_empty(), "nothing left to move");

        std::fs::write(&named, b"something new").unwrap();
        assert!(!restore_trashed_master(&lib, ids[0]).unwrap(), "never over another file");
        assert!(lib.master_trash_row(ids[0]).unwrap().is_some(), "it stays in the Trash, noted");
        std::fs::remove_file(&named).unwrap();
        assert!(restore_trashed_master(&lib, ids[0]).unwrap());
        assert_eq!(std::fs::read(&named).unwrap(), song);
        assert!(lib.master_trash_row(ids[0]).unwrap().is_none());

        // A file that is not the master any more stays where it is.
        std::fs::write(&named, b"changed").unwrap();
        assert!(trash_master_files(&lib, &trash, &masters, &hash, std::slice::from_ref(&named), &yes).ids.is_empty());
        assert!(named.exists());

        // The look right before each move: once it fails, nothing more
        // moves, and what moved before is told with the failure.
        std::fs::write(&named, &song).unwrap();
        let looks = std::cell::Cell::new(0);
        let once = || {
            looks.set(looks.get() + 1);
            looks.get() == 1
        };
        let both = [named.clone(), copy.clone()];
        let part = trash_master_files(&lib, &trash, &masters, &hash, &both, &once);
        assert_eq!(part.ids.len(), 1, "the first moved, its note kept for «Отменить»");
        assert!(part.error.is_some());
        assert!(!named.exists() && copy.exists());
    }

    /// A move cut short is settled at the next start from what can be
    /// verified: the original still that very file → the Trash file, its
    /// link and the note go; the original gone or its name another file's
    /// and the Trash file that very file → done, both kept; anything not
    /// verified → the note stays on its way and nothing is deleted. A file
    /// that only looks like a link is never touched.
    #[test]
    fn a_master_move_cut_short_is_settled() {
        let root = tempfile::tempdir().unwrap();
        let (masters, trash) = (root.path().join("masters"), root.path().join("Trash"));
        std::fs::create_dir_all(&masters).unwrap();
        std::fs::create_dir_all(&trash).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let song = b"fLaC".to_vec();
        let hash = blake3::hash(&song).to_hex().to_string();
        let note = |src: &Path, dest: &Path| lib.begin_master_trash(&hash, &src.to_string_lossy(), &dest.to_string_lossy(), 4).unwrap();
        let moving = |id: i64| lib.master_trash_row(id).unwrap().map(|r| r.moving);

        let kept = masters.join("01.flac");
        std::fs::write(&kept, &song).unwrap();
        let half = trash.join("2026-11-16 01.flac");
        std::fs::write(&half, b"fL").unwrap();
        let link = trash.join(".2026-11-16 01.flac.nikgob-link");
        std::fs::write(&link, &song).unwrap();
        let a = note(kept.as_path(), half.as_path());

        let gone = masters.join("02.flac");
        let done = trash.join("2026-11-16 02.flac");
        std::fs::write(&done, &song).unwrap();
        let b = note(gone.as_path(), done.as_path());

        let taken = masters.join("03.flac");
        std::fs::write(&taken, b"another file").unwrap();
        let safe = trash.join("2026-11-16 03.flac");
        std::fs::write(&safe, &song).unwrap();
        let c = note(taken.as_path(), safe.as_path());

        let lost = trash.join("2026-11-16 04.flac");
        let d = note(masters.join("04.flac").as_path(), lost.as_path());

        let wrong = trash.join("2026-11-16 05.flac");
        std::fs::write(&wrong, b"not it").unwrap();
        let e = note(masters.join("05.flac").as_path(), wrong.as_path());

        let stray = trash.join(".2026-11-16 09.flac.nikgob-link");
        std::fs::write(&stray, &song).unwrap();

        assert_eq!(recover_master_moves(&lib, &trash).unwrap(), 3);
        assert!(kept.exists() && !half.exists() && !link.exists(), "undone: the original stays");
        assert_eq!(moving(a), None);
        assert!(done.exists());
        assert_eq!(moving(b), Some(false), "finished");
        assert_eq!(std::fs::read(&taken).unwrap(), b"another file");
        assert!(safe.exists());
        assert_eq!(moving(c), Some(false), "the name is another file's: a Trash entry with undo");
        assert_eq!(moving(d), Some(true), "nothing to verify: it stays on its way");
        assert_eq!(moving(e), Some(true));
        assert!(wrong.exists(), "nothing deleted on a check that says nothing");
        assert!(stray.exists(), "no note names it");

        // The Trash folder not there (its disk away): nothing settled.
        let away = root.path().join("Away");
        let f = lib.begin_master_trash(&hash, &kept.to_string_lossy(), &away.join("2026-11-16 01.flac").to_string_lossy(), 4).unwrap();
        assert_eq!(recover_master_moves(&lib, &away).unwrap(), 0);
        assert_eq!(moving(f), Some(true));
        assert!(kept.exists());
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
        // A note whose file is not in the Trash (a date-shaped name
        // elsewhere): never deleted, the note stays.
        let other = root.path().join("Elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        let stranger = other.join("2026-10-01 01 - C.flac");
        std::fs::write(&stranger, b"y").unwrap();
        let kept = lib.put_master_trash("h", "/m/01 - C.flac", &stranger.to_string_lossy(), 1).unwrap();
        assert_eq!(purge_trashed_masters(&lib, &trash, 30).unwrap().rows, 0, "still in its days");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let r = purge_trashed_masters(&lib, &trash, 0).unwrap();
        assert_eq!((r.rows, r.files_deleted, r.files_failed), (1, 1, 1));
        assert!(!file.exists());
        assert!(lib.master_trash_row(id).unwrap().is_none());
        assert!(stranger.exists(), "outside the Trash");
        assert!(lib.master_trash_row(kept).unwrap().is_some());
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
