//! «В файле другая песня» on a local track (docs/TODO.md 32): the file goes
//! to the library's Trash (with its lyrics, and the lossless original it was
//! made from), the track stays and is looked for again, and what the file
//! was (its hash as it came, its sound) and where it came from (the torrent,
//! the Soulseek peer's file, the service) are never taken for the track
//! again. [`undo_wrong_file`] takes all of it back.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use nikgob_core::model::{Master, SourceId, SourceOrigin, TrackId};
use nikgob_library::Library;

use crate::config::ImportConfig;
use crate::error::{ImportError, Result};
use crate::fsutil::{create_dir_all, move_file};
use crate::same_audio::{file_keys, print_file};
use crate::sanitize::long_path;
use crate::trash::trash_file;

/// What [`mark_wrong_file`] did, for its undo.
#[derive(Clone, Debug)]
pub struct WrongFile {
    pub track: TrackId,
    /// The file's source, now in the Trash.
    pub source: SourceId,
    /// `(original place, place in Trash)` of the file (`None`: it was
    /// already missing).
    pub file: Option<(PathBuf, PathBuf)>,
    /// The same of what went with it: its lyrics, the lossless original in
    /// the masters folder.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// The track's master row, taken out.
    pub master: Option<Master>,
    /// What is not taken for the track again (`hash:…`, `from:…`).
    pub keys: Vec<String>,
}

/// The lossless original of `hash` in the masters folder (`<hash>.<ext>`,
/// not its `.json` nor the `.cue` sheet kept with an image).
fn master_file(cfg: &ImportConfig, hash: &str) -> Option<PathBuf> {
    let rd = std::fs::read_dir(long_path(&cfg.masters_dir())).ok()?;
    rd.flatten().map(|e| e.path()).find(|p| {
        p.file_stem().is_some_and(|s| s.to_string_lossy() == hash)
            && !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
            && !crate::fsutil::is_cue_file(p)
    })
}

/// The user says `track`'s file holds another song (`sounds_like`, when
/// known): see the module docs. Nothing is deleted; every file goes to the
/// Trash.
pub fn mark_wrong_file(
    lib: &Library,
    cfg: &ImportConfig,
    track: TrackId,
    sounds_like: Option<TrackId>,
) -> Result<WrongFile> {
    let src = lib
        .active_source(track)?
        .filter(|s| s.kind.is_local() && s.trashed_at.is_none())
        .ok_or_else(|| ImportError::Other(format!("track {track} has no file")))?;
    let path = PathBuf::from(&src.locator);
    let master = lib.get_master(track)?;
    let stored = lib.audio_print(src.id)?;
    // What the file is: its print as stored, else heard now.
    let print = match stored.as_ref().and_then(|p| p.print.clone()) {
        Some(p) => Some(p),
        None => print_file(&path, &AtomicBool::new(false)).ok().map(|(p, _)| p),
    };
    // Its hash as it came (a lossless original's is the master's).
    let came_hash = stored
        .as_ref()
        .and_then(|p| p.came_hash.clone())
        .or_else(|| master.as_ref().map(|m| m.file_hash.clone()));
    let mut came_from = stored.as_ref().map(|p| p.came_from.clone()).unwrap_or_default();
    if let SourceOrigin::Torrent { info_hash, .. } = &src.origin
        && !info_hash.is_empty()
    {
        let key = format!("torrent:{info_hash}");
        if !came_from.contains(&key) {
            came_from.push(key);
        }
    }
    let mut keys = file_keys(came_hash.as_deref(), &came_from);
    if let Some(h) = src.file_hash.as_deref().filter(|h| Some(*h) != came_hash.as_deref()) {
        keys.push(format!("hash:{h}"));
    }

    // What is never taken again goes down first: it is the write most
    // likely to fail (the library busy with an import), and while no file
    // has moved a failure leaves everything as it was.
    lib.put_wrong_audio(track, &keys, sounds_like, print.as_deref())?;
    let forget = || {
        if let Err(e) = lib.forget_wrong_audio(track, &keys) {
            log::warn!("wrong file: the marks of {track} stay: {e}");
        }
    };

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let file = if long_path(&path).exists() {
        let dest = match trash_file(&cfg.trash_dir, &path) {
            Ok(d) => d,
            Err(e) => {
                forget();
                return Err(e);
            }
        };
        let lrc = path.with_extension("lrc");
        if long_path(&lrc).exists()
            && let Ok(to) = trash_file(&cfg.trash_dir, &lrc)
        {
            moved.push((lrc, to));
        }
        Some((path.clone(), dest))
    } else {
        None
    };
    let dest = file.as_ref().map(|(_, d)| d.to_string_lossy().into_owned());
    if let Err(e) = lib.trash_source(src.id, dest.as_deref()) {
        put_back(file.as_slice());
        put_back(&moved);
        forget();
        return Err(e.into());
    }
    // The lossless original holds the same sound: it goes too (one still
    // seeding in Downloads stays with its torrent, only its row goes). Its
    // row goes only with its file, so neither is left without the other.
    let master = master.filter(|m| take_master(lib, cfg, track, m, &mut moved));
    log::info!(
        "wrong file: {} of {track} is in the Trash, not taken again: {}",
        path.display(),
        keys.join(", ")
    );
    Ok(WrongFile {
        track,
        source: src.id,
        file,
        moved,
        master,
        keys,
    })
}

/// Takes the track's master out with its lossless original (when that is in
/// the masters folder): the file to the Trash (noted in `moved`), then the
/// row. False when the master stays (the file could not move, or the row
/// could not go and the file came back).
fn take_master(
    lib: &Library,
    cfg: &ImportConfig,
    track: TrackId,
    m: &Master,
    moved: &mut Vec<(PathBuf, PathBuf)>,
) -> bool {
    // An image other tracks have as their master too stays for them: only
    // this track's row goes.
    let shared = lib.master_users(&m.file_hash).unwrap_or(1) > 1;
    let file = match master_file(cfg, &m.file_hash).filter(|_| !shared) {
        Some(file) => match trash_file(&cfg.trash_dir, &file) {
            Ok(to) => Some((file, to)),
            Err(e) => {
                log::warn!("wrong file: the original {} stays with its row: {e}", file.display());
                return false;
            }
        },
        None => None,
    };
    if let Err(e) = lib.delete_master(track) {
        log::warn!("wrong file: the master of {track} stays: {e}");
        put_back(file.as_slice());
        return false;
    }
    moved.extend(file);
    true
}

/// Moves files back from the Trash (never over a file that took the place).
fn put_back(moved: &[(PathBuf, PathBuf)]) -> usize {
    let mut n = 0;
    for (orig, in_trash) in moved {
        if !long_path(in_trash).exists() || long_path(orig).exists() {
            log::warn!("wrong file: {} cannot go back to {}", in_trash.display(), orig.display());
            continue;
        }
        if let Some(dir) = orig.parent() {
            let _ = create_dir_all(dir);
        }
        match move_file(in_trash, orig) {
            Ok(()) => n += 1,
            Err(e) => log::warn!("wrong file: {} stays in the Trash: {e}", in_trash.display()),
        }
    }
    n
}

/// Takes back [`mark_wrong_file`]: the files come back from the Trash, the
/// track plays its file again, and the file and its place may be taken
/// again. False when the file itself could not come back.
pub fn undo_wrong_file(lib: &Library, w: &WrongFile) -> Result<bool> {
    // A file that was already missing has nothing to bring back.
    let file_back = w.file.is_none() || put_back(w.file.as_slice()) == 1;
    put_back(&w.moved);
    if file_back {
        lib.restore_source(w.source)?;
    }
    if let Some(m) = &w.master {
        lib.put_master(m)?;
    }
    lib.forget_wrong_audio(w.track, &w.keys)?;
    Ok(file_back)
}
