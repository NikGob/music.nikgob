//! Where new lossless masters go, and how they are written there.
//!
//! ```text
//! <masters>/<Album artist>/<YYYY - Album>/<NN - Title>.flac
//! <masters>/<Album artist>/<YYYY - Album [24-96]>/...         (hi-res files)
//! <masters>/<Artist>/Singles/<YYYY - Title>/01 - Title.flac    (no album)
//! <masters>/Various Artists/<YYYY - Album>/<NN - Artist - Title>.flac
//! <masters>/<...album folder...>/cover.jpg
//! <masters>/.nikgob-backup/library-YYYY-MM-DD.sqlite            (the last 7 days)
//! ```
//!
//! Only masters written from now on go there: the older `{file_hash}.<ext>`
//! files right in the folder are never moved, renamed or deleted here, and
//! are found as before ([`master_file`]). A master is written to a temporary
//! file of its own in its album folder, flushed and checked (size and
//! BLAKE3), then takes its name without ever replacing a file (a hard link;
//! on Windows, where the disk has no links, a move the system refuses onto
//! a taken name; elsewhere such a disk gets nothing and the source stays),
//! its folders are flushed, and only then is its path recorded in the
//! library. A crash leaves at worst a temporary file, removed at a later
//! start of a player on the same computer ([`clean_stale_temps`]). A name
//! that holds another file is never written over: the next free one
//! (`name (2).flac`, …) is taken.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};


use nikgob_core::model::{Master, ReleaseType, TrackId};
use nikgob_library::Library;

use crate::config::ImportConfig;
use crate::error::{ImportError, Result};
use crate::fsutil;
use crate::layout::{LayoutInput, is_various_artists};
use crate::sanitize::{fit_stem, long_path, numbered_path, truncate_chars};

/// Longest path a master gets (UTF-16 units with the closing NUL, as
/// Windows counts them): Explorer and other programs open it as it is.
const MAX_MASTER_PATH: usize = 240;
/// Longest folder name in bytes (Linux allows 255 bytes per name; a
/// Japanese letter takes 3).
const MAX_DIR_BYTES: usize = 200;
/// Longest file stem in bytes: room for ` (99)` and the extension.
const MAX_STEM_BYTES: usize = 230;
/// A name is looked for this many times (` (2)` … ` (999)`) at most.
const MAX_NAMES: u32 = 999;
/// Room a numbered name takes at most (` (999)`).
const SUFFIX_ROOM: usize = 6;
/// A folder or title is never shortened below this many letters.
const MIN_NAME: usize = 8;

/// The album folder's cover.
pub const COVER_FILE: &str = "cover.jpg";
/// The folder of the database copies, in the masters folder.
pub const BACKUP_DIR: &str = ".nikgob-backup";
/// Database copies kept.
pub const BACKUP_KEEP: usize = 7;

/// Temporary files of a master (or a cover) on its way into its name.
const MASTER_TEMP: &str = ".nikgob-master-";
/// Temporary files of a database copy.
const DB_TEMP: &str = ".nikgob-db-";
const TEMP_END: &str = ".part";

/// `[24-96]` for a hi-res file (more than 16 bits, or more than 48 kHz),
/// none for CD quality and below.
pub fn hi_res_tag(bit_depth: Option<u8>, sample_rate: Option<u32>) -> Option<String> {
    let bits = bit_depth.unwrap_or(16);
    if bits <= 16 && sample_rate.is_none_or(|r| r <= 48_000) {
        return None;
    }
    Some(match sample_rate {
        Some(r) if r % 1000 == 0 => format!("[{bits}-{}]", r / 1000),
        Some(r) => format!("[{bits}-{}.{}]", r / 1000, (r % 1000) / 100),
        None => format!("[{bits}]"),
    })
}

/// At most `max` bytes of `s`, cut between letters, never ending in a dot
/// or a space.
fn cap_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let cut = s[..end].trim_end_matches(['.', ' ']);
    if cut.is_empty() { "_".to_owned() } else { cut.to_owned() }
}

/// Where a new master of `layout` goes in `root` (no name taken yet): the
/// library's own layout ([`LayoutInput`]) with the format in the album
/// folder's name for a hi-res file (`tag`, [`hi_res_tag`]) and, on a
/// compilation, the track's artist in the file name. Every name is safe on
/// Windows and Linux and capped in letters and bytes; the whole path, with
/// room for a ` (999)` a taken name may get, fits [`MAX_MASTER_PATH`]: the
/// longest folder names are shortened first, then the title. None when even
/// the shortest names do not fit (a very long masters folder).
pub fn master_target(root: &Path, layout: &LayoutInput, tag: Option<&str>, ext: &str) -> Option<PathBuf> {
    let units = |s: &str| s.encode_utf16().count();
    let album = layout.album_dir(Path::new(""));
    let fixed: Vec<String> = album.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let singles = fixed.len() == 3;
    // (the name, whether it may be shortened); the tag goes after the last.
    let mut parts: Vec<(String, bool)> = fixed
        .iter()
        .enumerate()
        .map(|(i, p)| (cap_bytes(p, MAX_DIR_BYTES), !(singles && i == 1)))
        .collect();
    let tag = tag.map(|t| format!(" {t}")).unwrap_or_default();

    let mut named = layout.clone();
    let artist = layout.artist.trim();
    if layout.is_compilation() && !artist.is_empty() && !is_various_artists(artist) {
        let title = if layout.title.trim().is_empty() { "Untitled" } else { layout.title.trim() };
        named.title = format!("{artist} - {title}");
    }
    let (stem, prefix) = named.file_stem();
    let stem = cap_bytes(&stem, MAX_STEM_BYTES);
    let min_stem = prefix + MIN_NAME;
    // Usable units: the limit less the closing NUL and a numbered suffix.
    let budget = MAX_MASTER_PATH - 1 - SUFFIX_ROOM;
    let dirs_len = |parts: &[(String, bool)]| -> usize {
        units(&root.to_string_lossy()) + parts.iter().map(|(p, _)| 1 + units(p)).sum::<usize>() + units(&tag)
    };
    let tail = 1 + min_stem.min(units(&stem)) + 1 + units(ext);
    while dirs_len(&parts) + tail > budget {
        let longest = parts
            .iter_mut()
            .filter(|(p, may)| *may && p.chars().count() > MIN_NAME)
            .max_by_key(|(p, _)| p.chars().count())?;
        let n = longest.0.chars().count() - 1;
        longest.0 = truncate_chars(&longest.0, n);
    }
    let mut dir = root.to_path_buf();
    let last = parts.len().saturating_sub(1);
    for (i, (part, _)) in parts.iter().enumerate() {
        dir.push(if i == last { format!("{part}{tag}") } else { part.clone() });
    }
    let stem = fit_stem(&dir, &stem, ext, MAX_MASTER_PATH - SUFFIX_ROOM, prefix);
    let path = dir.join(format!("{stem}.{ext}"));
    (crate::sanitize::win_len(&path) + SUFFIX_ROOM < MAX_MASTER_PATH).then_some(path)
}

/// The layout of a track as the library has it now (its album, artists,
/// place), for a master that came without an import's identity (a
/// torrent's file taken out of `Downloads/`). None when the track is not
/// there.
pub fn layout_of_track(lib: &Library, track: TrackId) -> Option<LayoutInput> {
    let t = lib.get_track(track).ok()??;
    let release = t.release_id.and_then(|r| lib.get_release(r).ok().flatten());
    let album_artist = release.as_ref().map(|r| r.artist_credit.display()).unwrap_or_default();
    let disc_total = t
        .release_id
        .and_then(|r| lib.tracks_of_release(r).ok())
        .and_then(|rows| rows.iter().map(|r| r.disc_no).max())
        .filter(|d| *d > 1);
    let release_type = release.as_ref().map_or(ReleaseType::Single, |r| r.release_type);
    Some(LayoutInput {
        compilation: release_type == ReleaseType::Compilation || is_various_artists(&album_artist),
        album_artist,
        artist: t.artist_credit.display(),
        album: release.as_ref().map(|r| r.title.clone()),
        title: t.title.clone(),
        year: release.as_ref().and_then(|r| r.year),
        disc_no: t.disc_no,
        disc_total,
        track_no: t.track_no,
        release_type,
    })
}

/// The names in `dir`, in lower case (a name is taken whatever its case:
/// Windows sees `Song.flac` and `song.flac` as one file).
fn names_in(dir: &Path) -> HashSet<String> {
    fs::read_dir(long_path(dir))
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
        .unwrap_or_default()
}

fn taken(path: &Path, names: &HashSet<String>) -> bool {
    long_path(path).exists()
        || path.file_name().is_some_and(|n| names.contains(&n.to_string_lossy().to_lowercase()))
}

/// `path` holds exactly the file of `size` bytes and BLAKE3 `hash`.
pub(crate) fn same_content(path: &Path, size: u64, hash: &str) -> bool {
    fsutil::file_size(path).ok() == Some(size) && fsutil::hash_file(path).ok().is_some_and(|h| h.eq_ignore_ascii_case(hash))
}

/// `target`, then `target (2)`, … up to [`MAX_NAMES`].
fn candidate(target: &Path, n: u32) -> PathBuf {
    if n == 1 { target.to_path_buf() } else { numbered_path(target, n) }
}

/// Flushes the folder's entries to the disk where that is a thing one can
/// ask for (not on Windows, where NTFS journals its names and a file's own
/// flush is what there is). A flush that fails is an error.
fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir).and_then(|d| d.sync_all()).map_err(fsutil::io_err(dir))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Flushes `dir` and every folder above it up to `top` (the first one that
/// was there before `dir` was made): a new name and the new folders that
/// lead to it are on the disk before the library records it.
fn sync_dirs_up_to(dir: &Path, top: &Path) -> Result<()> {
    let mut at = Some(dir);
    while let Some(d) = at {
        sync_dir(d)?;
        if d == top {
            break;
        }
        at = d.parent();
    }
    Ok(())
}

/// The nearest folder of `dir` (itself or above) that is there now.
fn existing_ancestor(dir: &Path) -> PathBuf {
    dir.ancestors().find(|d| long_path(d).is_dir()).unwrap_or(dir).to_path_buf()
}

/// This computer in our temporary files' names: 8 hex digits of its name's
/// BLAKE3. None when the computer's name cannot be told (its temporary
/// files are then never cleaned up: whose they are is not known).
fn host_tag() -> Option<&'static str> {
    static TAG: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    TAG.get_or_init(|| {
        let name = std::env::var("COMPUTERNAME")
            .ok()
            .or_else(|| fs::read_to_string("/etc/hostname").ok())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty())?;
        Some(blake3::hash(name.as_bytes()).to_hex()[..8].to_owned())
    })
    .as_deref()
}

/// The host part of our temporary names: [`host_tag`], or zeros when the
/// computer's name is not known.
fn host_part() -> &'static str {
    host_tag().unwrap_or("00000000")
}

/// The file a running player holds locked for as long as it lives
/// (`nikgob-live-<pid>.lock` in the system's temporary folder): a
/// temporary file named with that pid is someone's work in progress while
/// the lock is held ([`pid_alive`]).
fn live_lock_path(pid: u32) -> PathBuf {
    std::env::temp_dir().join(format!("nikgob-live-{pid}.lock"))
}

/// Takes this process's live lock (once; kept until the process ends).
/// Every temporary file of ours is made only while it is held, so no other
/// player takes one of ours for a crash's leftover.
fn hold_live_lock() -> Result<()> {
    static LIVE: std::sync::OnceLock<std::result::Result<File, String>> = std::sync::OnceLock::new();
    let held = LIVE.get_or_init(|| {
        let p = live_lock_path(std::process::id());
        let f = File::options().create(true).truncate(false).write(true).open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        f.lock().map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(f)
    });
    held.as_ref().map(|_| ()).map_err(|e| ImportError::Other(format!("masters: no live lock, nothing written: {e}")))
}

/// Whether the player of this computer with this pid may still be writing
/// its temporary files: this very process always; another one unless its
/// live lock file is there and no one holds it. When that cannot be told
/// (no lock file: it may live where this player does not look), yes:
/// nothing of it is touched.
fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    match File::options().write(true).open(live_lock_path(pid)) {
        Err(_) => true,
        Ok(f) => match f.try_lock() {
            Ok(()) => false,
            Err(_) => true,
        },
    }
}

/// A fresh temporary file of ours in `dir` (`<prefix><host>-<pid>-<n>.part`),
/// made with `create_new` (never a file that is there) while this
/// process's live lock is held.
fn new_temp(dir: &Path, prefix: &str) -> Result<(PathBuf, File)> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    hold_live_lock()?;
    for _ in 0..1000 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let p = dir.join(format!("{prefix}{}-{}-{n}{TEMP_END}", host_part(), std::process::id()));
        match File::options().write(true).create_new(true).open(long_path(&p)) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(ImportError::io(&p, e)),
        }
    }
    Err(ImportError::Other(format!("{}: no free temporary name", dir.display())))
}

/// Copies `src` into a temporary file of ours in `dir`, flushed to the disk
/// and checked: `size` bytes and, when given, BLAKE3 `hash`. Nothing of it
/// is left when it fails.
fn stage_temp(src: &Path, dir: &Path, size: u64, hash: Option<&str>) -> Result<PathBuf> {
    let (tmp, mut out) = new_temp(dir, MASTER_TEMP)?;
    let written = (|| -> Result<()> {
        let mut from = File::open(long_path(src)).map_err(fsutil::io_err(src))?;
        std::io::copy(&mut from, &mut out).map_err(fsutil::io_err(&tmp))?;
        out.sync_all().map_err(fsutil::io_err(&tmp))?;
        drop(out);
        let got = fsutil::file_size(&tmp)?;
        let ok = got == size && hash.is_none_or(|h| fsutil::hash_file(&tmp).ok().is_some_and(|g| g.eq_ignore_ascii_case(h)));
        if !ok {
            return Err(ImportError::Other(format!(
                "проверка хеша не прошла для {}: копия {} не совпала с оригиналом",
                tmp.display(),
                src.display()
            )));
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fsutil::remove_file(&tmp);
        return Err(e);
    }
    Ok(tmp)
}

/// Gives the temporary file `tmp` the name `to` unless the name is there
/// already, never replacing a file, whoever made it, in one step the system
/// refuses when the name is taken: a hard link (then the temporary name
/// goes) or, on a Windows disk without links (FAT, exFAT), a move that does
/// not replace ([`move_new`]). Where neither is there, an error: nothing is
/// published and `tmp` stays for the caller. The folder is flushed after
/// ([`sync_dir`]): a flush that fails takes the new name back (no row has
/// it yet) and is an error. False when the name is taken; `tmp` stays for
/// the caller then.
fn link_new(tmp: &Path, to: &Path) -> Result<bool> {
    match fs::hard_link(long_path(tmp), long_path(to)) {
        Ok(()) => {
            if let Err(e) = fs::remove_file(long_path(tmp)) {
                // A second name of the same file: a later start removes it.
                log::warn!("masters: the temporary {} stays: {e}", tmp.display());
            }
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(link) => {
            if !move_new(tmp, to).map_err(|e| ImportError::Other(format!("{}: no link ({link}), no move: {e}", to.display())))? {
                return Ok(false);
            }
        }
    }
    if let Some(dir) = to.parent()
        && let Err(e) = sync_dir(dir)
    {
        let _ = fs::remove_file(long_path(to));
        return Err(e);
    }
    Ok(true)
}

/// Moves `tmp` to `to` only while `to` is not there, the check and the move
/// one step of the system's (Windows: `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING`, written through). False when the name is
/// taken. Elsewhere there is no such step a disk without links has: an
/// error, `tmp` stays.
#[allow(unsafe_code)]
fn move_new(tmp: &Path, to: &Path) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let wide = |p: &Path| long_path(p).as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (from, into) = (wide(tmp), wide(to));
        // SAFETY: both are NUL-terminated wide strings that outlive the call.
        if unsafe { MoveFileExW(from.as_ptr(), into.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
            return Ok(true);
        }
        let e = std::io::Error::last_os_error();
        if e.kind() == ErrorKind::AlreadyExists { Ok(false) } else { Err(e) }
    }
    #[cfg(not(windows))]
    {
        let _ = (tmp, to);
        Err(std::io::Error::new(ErrorKind::Unsupported, "the disk has no hard links"))
    }
}

/// What [`place_master`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Where the master is now.
    pub path: PathBuf,
    /// It was written now (false: the very file was there already).
    pub written: bool,
}

/// Puts a copy of the file `src` (BLAKE3 `hash`) at `target` or the first
/// free numbered name after it, see the module docs. The same file under
/// one of those names already is taken as it is. `src` itself is never
/// touched: the caller lets it go once the library records the copy.
pub fn place_master(src: &Path, target: &Path, hash: &str) -> Result<Placed> {
    let dir = target.parent().ok_or_else(|| ImportError::Other(format!("{} has no folder", target.display())))?;
    let top = existing_ancestor(dir);
    fsutil::create_dir_all(dir)?;
    let size = fsutil::file_size(src)?;
    let mut tmp: Option<PathBuf> = None;
    let mut placed = None;
    for n in 1..=MAX_NAMES {
        let to = candidate(target, n);
        let names = names_in(dir);
        if taken(&to, &names) {
            if same_content(&to, size, hash) {
                placed = Some(Placed { path: to, written: false });
                break;
            }
            continue;
        }
        let staged = match &tmp {
            Some(t) => t.clone(),
            None => {
                let t = stage_temp(src, dir, size, Some(hash))?;
                tmp = Some(t.clone());
                t
            }
        };
        match link_new(&staged, &to) {
            Ok(true) => {
                tmp = None;
                // The folders made for it are on the disk too, or the copy
                // goes again (no row has it yet) and the source stays.
                if let Some(parent) = dir.parent()
                    && dir != top
                    && let Err(e) = sync_dirs_up_to(parent, &top)
                {
                    let _ = fs::remove_file(long_path(&to));
                    return Err(e);
                }
                placed = Some(Placed { path: to, written: true });
                break;
            }
            // Taken meanwhile: the next name (or this one, when it is the
            // very file) is looked at.
            Ok(false) => {
                if same_content(&to, size, hash) {
                    placed = Some(Placed { path: to, written: false });
                    break;
                }
            }
            Err(e) => {
                let _ = fsutil::remove_file(&staged);
                return Err(e);
            }
        }
    }
    if let Some(t) = tmp {
        let _ = fsutil::remove_file(&t);
    }
    placed.ok_or_else(|| ImportError::Other(format!("{}: no free name", target.display())))
}

/// Writes `bytes` as the file `path` (a master's sidecar), over what is
/// there: through a temporary file of ours beside it, flushed, then renamed
/// into place, so a crash leaves the old file or the new one whole, never a
/// cut one.
pub(crate) fn write_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().ok_or_else(|| ImportError::Other(format!("{} has no folder", path.display())))?;
    let (tmp, mut out) = new_temp(dir, MASTER_TEMP)?;
    let written = (|| -> Result<()> {
        out.write_all(bytes).and_then(|()| out.sync_all()).map_err(fsutil::io_err(&tmp))?;
        drop(out);
        fs::rename(long_path(&tmp), long_path(path)).map_err(fsutil::io_err(path))?;
        sync_dir(dir)
    })();
    if written.is_err() {
        let _ = fs::remove_file(long_path(&tmp));
    }
    written
}

/// Puts a copy of `cover` into `album_dir` as `cover.jpg` when the folder
/// has none (never over one). Best effort: a cover that fails is logged.
pub fn keep_cover(album_dir: &Path, cover: &Path) {
    let to = album_dir.join(COVER_FILE);
    if taken(&to, &names_in(album_dir)) || !long_path(cover).is_file() {
        return;
    }
    let kept = (|| -> Result<()> {
        let size = fsutil::file_size(cover)?;
        let tmp = stage_temp(cover, album_dir, size, None)?;
        let linked = link_new(&tmp, &to);
        if !matches!(linked, Ok(true)) {
            let _ = fsutil::remove_file(&tmp);
        }
        linked.map(|_| ())
    })();
    if let Err(e) = kept {
        log::warn!("masters: the cover of {} is not kept: {e}", album_dir.display());
    }
}

/// The lossless file of `m` in the masters folder: its own path when it is
/// kept by name (none when that file is gone: a flat file of the same hash
/// is not this master's), else `{file_hash}.<ext>` right in the folder (not
/// the `.json` beside it, not the `.cue` kept with an image).
pub fn master_file(cfg: &ImportConfig, m: &Master) -> Option<PathBuf> {
    match m.file_path.as_deref().map(PathBuf::from) {
        Some(p) => long_path(&p).is_file().then_some(p),
        None => flat_master(&cfg.masters_dir(), &m.file_hash),
    }
}

/// `{hash}.<ext>` right in `dir`, the way masters were kept before.
pub fn flat_master(dir: &Path, hash: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(long_path(dir)).ok()?;
    rd.flatten().map(|e| e.path()).find(|p| {
        p.file_stem().is_some_and(|s| s.to_string_lossy() == hash)
            && !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
            && !fsutil::is_cue_file(p)
    })
}

#[derive(serde::Deserialize)]
struct SidecarPath {
    path: String,
}

/// A copy of the file `hash` (`size` bytes) the masters folder keeps by
/// name already: the one its sidecar names, or one the library has a
/// master row of. Checked by content.
pub(crate) fn existing_copy(lib: &Library, root: &Path, hash: &str, size: u64) -> Option<PathBuf> {
    let from_sidecar = fs::read(long_path(&root.join(format!("{hash}.json"))))
        .ok()
        .and_then(|b| serde_json::from_slice::<SidecarPath>(&b).ok())
        .map(|s| PathBuf::from(s.path))
        .filter(|p| p.starts_with(root) && p.parent() != Some(root));
    from_sidecar
        .into_iter()
        .chain(lib.master_files_of_hash(hash).unwrap_or_default().into_iter().map(PathBuf::from))
        .find(|p| long_path(p).is_file() && same_content(p, size, hash))
}

/// The file `hash` is a disc image (a single-file rip a cue sheet cuts
/// into tracks), whatever number of its tracks the library has now: its
/// sheet is kept as `<root>/<hash>.cue`, or its sidecar names one. Images
/// stay `{hash}.<ext>` right in the masters folder.
pub fn is_image_master(root: &Path, hash: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct SidecarCue {
        #[serde(default)]
        cue: Option<String>,
    }
    long_path(&root.join(format!("{hash}.cue"))).exists()
        || fs::read(long_path(&root.join(format!("{hash}.json"))))
            .ok()
            .and_then(|b| serde_json::from_slice::<SidecarCue>(&b).ok())
            .is_some_and(|s| s.cue.is_some())
}

/// A torrent's payload file `from` that is the master of the one track of
/// `m`, kept by name in the masters folder: copied there (the payload is
/// left to the caller), its row pointed at the copy. A copy an earlier try
/// made is taken as it is. Returns where it is kept; None when it is not
/// one to keep by name (a disc image, a track the library cannot name, a
/// masters folder too long for a name): the caller keeps it the older way.
pub fn keep_torrent_master(lib: &Library, cfg: &ImportConfig, from: &Path, m: &Master) -> Result<Option<PathBuf>> {
    let size = fsutil::file_size(from)?;
    if let Some(p) = m.file_path.as_deref().map(PathBuf::from)
        && long_path(&p).is_file()
        && same_content(&p, size, &m.file_hash)
    {
        return Ok(Some(p));
    }
    let root = cfg.masters_dir();
    if is_image_master(&root, &m.file_hash) {
        return Ok(None);
    }
    let Some(layout) = layout_of_track(lib, m.track_id) else { return Ok(None) };
    let info = crate::tags::probe_info(from).ok();
    let tag = info.as_ref().and_then(|i| hi_res_tag(i.bit_depth, i.sample_rate));
    let ext = from.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "flac".to_owned());
    let Some(target) = master_target(&root, &layout, tag.as_deref(), &ext) else { return Ok(None) };
    let placed = place_master(from, &target, &m.file_hash)?;
    if let Err(e) = lib.set_master_file(m.track_id, &placed.path.to_string_lossy()) {
        // The payload stays the master: a copy written now goes again.
        if placed.written {
            let _ = fsutil::remove_file(&placed.path);
        }
        return Err(e.into());
    }
    if placed.written
        && let Some(dir) = placed.path.parent()
        && let Some(cover) = release_cover(lib, cfg, m.track_id)
    {
        keep_cover(dir, &cover);
    }
    Ok(Some(placed.path))
}

/// The library's `cover.jpg` of the track's album, when it has one on disk.
fn release_cover(lib: &Library, cfg: &ImportConfig, track: TrackId) -> Option<PathBuf> {
    let release = lib.get_track(track).ok()??.release_id?;
    let cover = lib.get_release(release).ok()??.cover_ref?;
    if nikgob_library::is_web_ref(&cover) {
        return None;
    }
    let p = cfg.library_dir.join(cover);
    long_path(&p).is_file().then_some(p)
}

/// A temporary file of ours, by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Temp<'a> {
    prefix: &'static str,
    /// The computer it was made on ([`host_tag`]).
    host: &'a str,
    /// The player that made it.
    pid: u32,
}

/// The temporary file of ours `name` is: exactly
/// `<prefix><host>-<pid>-<n>.part`, the host 8 lower-case hex digits, both
/// numbers decimal.
fn temp_of(name: &str) -> Option<Temp<'_>> {
    [MASTER_TEMP, DB_TEMP].into_iter().find_map(|prefix| {
        let mid = name.strip_prefix(prefix)?.strip_suffix(TEMP_END)?;
        let (host, rest) = mid.split_once('-')?;
        let (pid, n) = rest.split_once('-')?;
        let digits = |s: &str| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit());
        let hex = host.len() == 8 && host.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !(hex && digits(pid) && digits(n)) {
            return None;
        }
        Some(Temp { prefix, host, pid: pid.parse().ok()? })
    })
}

/// What [`clean_stale_temps`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// Temporary files a crash left, removed.
    pub removed: usize,
    /// Temporary files of a player still running (this one or another),
    /// left alone.
    pub live: usize,
}

/// Removes the temporary files a crash left in the masters folder `root`:
/// only names of exactly our pattern ([`temp_of`]), a master's in the
/// folder itself (an image, a flat master) or an album folder (four levels
/// down at most, as deep as a single's goes), a database copy's only in
/// `.nikgob-backup`, and only one made on this computer by a player that is
/// gone ([`pid_alive`]): work in progress is never touched, however old,
/// and neither is another computer's (a masters disk shared over the
/// network: whether that player still runs cannot be told from here).
/// Nothing else is touched; a computer whose name is not known cleans
/// nothing.
pub fn clean_stale_temps(root: &Path) -> Cleaned {
    fn walk(dir: &Path, host: &str, depth: usize, in_backup: bool, out: &mut Cleaned) {
        let Ok(rd) = fs::read_dir(long_path(dir)) else { return };
        for e in rd.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let path = dir.join(&name);
            if kind.is_dir() {
                if depth == 0 && name == BACKUP_DIR {
                    walk(&path, host, 1, true, out);
                } else if depth < 4 && !in_backup {
                    walk(&path, host, depth + 1, false, out);
                }
                continue;
            }
            let Some(temp) = temp_of(&name).filter(|_| kind.is_file()) else { continue };
            let here = if temp.prefix == DB_TEMP { in_backup && depth == 1 } else { !in_backup };
            if !here {
                continue;
            }
            // Another computer's: not ours to judge, never looked at again.
            if temp.host != host {
                continue;
            }
            if pid_alive(temp.pid) {
                out.live += 1;
                continue;
            }
            match fs::remove_file(long_path(&path)) {
                Ok(()) => out.removed += 1,
                Err(err) => log::warn!("masters: the temporary {} stays: {err}", path.display()),
            }
        }
    }
    let mut out = Cleaned::default();
    if let Some(host) = host_tag() {
        walk(root, host, 0, false, &mut out);
    }
    out
}

/// Is `name` a database copy's own name, `library-YYYY-MM-DD.sqlite`?
pub fn is_copy_name(name: &str) -> bool {
    let Some(day) = name.strip_prefix("library-").and_then(|n| n.strip_suffix(".sqlite")) else { return false };
    let b = day.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// `YYYY-MM-DD` of now (UTC).
pub fn today() -> String {
    let (y, m, d) = fsutil::ymd_from_unix(fsutil::unix_secs());
    format!("{y:04}-{m:02}-{d:02}")
}

/// What [`copy_database`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbCopy {
    /// Written now.
    Made(PathBuf),
    /// The day has its copy already.
    Had,
    /// The masters folder is not there (a disk unplugged): nothing written.
    NoFolder,
}

/// `path` is a whole database file by its look: a file that starts with
/// SQLite's header (a copy is published whole, so a file there that is not
/// one is something else: an empty file, a folder, a user's own).
fn is_database(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    long_path(path).is_file()
        && File::open(long_path(path)).and_then(|mut f| f.read_exact(&mut head)).is_ok()
        && &head == b"SQLite format 3\0"
}

/// One copy of the library's database for the day `day` into
/// `<root>/.nikgob-backup/library-<day>.sqlite` (SQLite's online backup),
/// then the newest `keep` copies kept: only database files of exactly that
/// name go. A day that has its copy gets no other; something else at the
/// day's name is left as it is and the day gets no copy (an error); a
/// missing `root` gets nothing (the folder is never made). Blocking: never
/// on the player loop.
pub fn copy_database(lib: &Library, root: &Path, day: &str, keep: usize) -> Result<DbCopy> {
    if !long_path(root).is_dir() {
        return Ok(DbCopy::NoFolder);
    }
    let dir = root.join(BACKUP_DIR);
    let to = dir.join(format!("library-{day}.sqlite"));
    let had = |to: &Path| -> Result<DbCopy> {
        if is_database(to) {
            Ok(DbCopy::Had)
        } else {
            Err(ImportError::Other(format!("{} is not a database copy: left as it is", to.display())))
        }
    };
    if fs::symlink_metadata(long_path(&to)).is_ok() {
        return had(&to);
    }
    match fs::create_dir(long_path(&dir)) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(ImportError::io(&dir, e)),
    }
    let (tmp, file) = new_temp(&dir, DB_TEMP)?;
    drop(file);
    let made = (|| -> Result<bool> {
        lib.backup_to(&tmp).map_err(|e| ImportError::Other(format!("{}: {e}", to.display())))?;
        // Flushed through a handle that may write (Windows flushes no other).
        File::options().write(true).open(long_path(&tmp)).and_then(|f| f.sync_all()).map_err(fsutil::io_err(&tmp))?;
        link_new(&tmp, &to)
    })();
    match made {
        Ok(true) => {}
        // The day's name was taken meanwhile: what is there decides.
        Ok(false) => {
            let _ = fsutil::remove_file(&tmp);
            return had(&to);
        }
        Err(e) => {
            let _ = fsutil::remove_file(&tmp);
            return Err(e);
        }
    }
    rotate_copies(&dir, keep);
    Ok(DbCopy::Made(to))
}

/// Keeps the newest `keep` database copies in `dir` (by the day in their
/// names); only database files named as a copy ([`is_copy_name`],
/// [`is_database`]) count and ever go: anything else is left as it is.
fn rotate_copies(dir: &Path, keep: usize) {
    let mut copies: Vec<PathBuf> = fs::read_dir(long_path(dir))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_copy_name(&e.file_name().to_string_lossy()))
                .map(|e| dir.join(e.file_name()))
                .filter(|p| is_database(p))
                .collect()
        })
        .unwrap_or_default();
    copies.sort();
    let extra = copies.len().saturating_sub(keep);
    for old in copies.into_iter().take(extra) {
        if let Err(e) = fs::remove_file(long_path(&old)) {
            log::warn!("database copy: {} stays: {e}", old.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> LayoutInput {
        LayoutInput {
            album_artist: "Кино".into(),
            artist: "Кино".into(),
            album: Some("Группа крови".into()),
            title: "Группа крови".into(),
            year: Some(1988),
            disc_no: 1,
            disc_total: Some(1),
            track_no: 3,
            release_type: ReleaseType::Album,
            compilation: false,
        }
    }

    fn rel(p: &Path, root: &Path) -> String {
        p.strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn target(root: &Path, layout: &LayoutInput, tag: Option<&str>) -> PathBuf {
        master_target(root, layout, tag, "flac").expect("a name fits")
    }

    #[test]
    fn a_master_goes_where_its_album_does() {
        let root = Path::new("/m");
        assert_eq!(rel(&target(root, &layout(), None), root), "Кино/1988 - Группа крови/03 - Группа крови.flac");
        let two = LayoutInput { disc_no: 2, disc_total: Some(2), ..layout() };
        assert_eq!(rel(&target(root, &two, None), root), "Кино/1988 - Группа крови/2-03 - Группа крови.flac");
    }

    #[test]
    fn only_hi_res_has_its_format_in_the_folder() {
        assert_eq!(hi_res_tag(Some(16), Some(44_100)), None);
        assert_eq!(hi_res_tag(Some(16), Some(48_000)), None);
        assert_eq!(hi_res_tag(None, None), None);
        assert_eq!(hi_res_tag(Some(24), Some(96_000)).as_deref(), Some("[24-96]"));
        assert_eq!(hi_res_tag(Some(24), Some(44_100)).as_deref(), Some("[24-44.1]"));
        assert_eq!(hi_res_tag(Some(16), Some(88_200)).as_deref(), Some("[16-88.2]"));
        assert_eq!(hi_res_tag(Some(24), None).as_deref(), Some("[24]"));
        let root = Path::new("/m");
        let p = target(root, &layout(), hi_res_tag(Some(24), Some(48_000)).as_deref());
        assert_eq!(rel(&p, root), "Кино/1988 - Группа крови [24-48]/03 - Группа крови.flac");
    }

    #[test]
    fn a_compilation_names_the_artist_in_the_file() {
        let root = Path::new("/m");
        let va = LayoutInput {
            album_artist: "Various Artists".into(),
            artist: "Post Malone & Swae Lee".into(),
            album: Some("Spider-Man: Into the Spider-Verse".into()),
            title: "Sunflower".into(),
            year: Some(2018),
            track_no: 2,
            compilation: true,
            release_type: ReleaseType::Compilation,
            ..layout()
        };
        assert_eq!(
            rel(&target(root, &va, None), root),
            "Various Artists/2018 - Spider-Man - Into the Spider-Verse/02 - Post Malone & Swae Lee - Sunflower.flac"
        );
        // A compilation whose track names no artist of its own.
        let bare = LayoutInput { artist: "Various Artists".into(), ..va };
        assert_eq!(rel(&target(root, &bare, None), root), "Various Artists/2018 - Spider-Man - Into the Spider-Verse/02 - Sunflower.flac");
    }

    #[test]
    fn a_track_without_an_album_is_a_single() {
        let root = Path::new("/m");
        let single = LayoutInput { album: None, track_no: 0, year: Some(2021), title: "Песня".into(), ..layout() };
        assert_eq!(rel(&target(root, &single, None), root), "Кино/Singles/2021 - Песня/01 - Песня.flac");
        let nobody = LayoutInput { album_artist: String::new(), artist: " ".into(), album: None, year: None, title: String::new(), track_no: 0, ..layout() };
        assert_eq!(rel(&target(root, &nobody, None), root), "Unknown Artist/Singles/Untitled/01 - Untitled.flac");
    }

    #[test]
    fn names_are_safe_and_capped() {
        let root = Path::new("/m");
        let rough = LayoutInput {
            album_artist: "AC/DC".into(),
            album: Some("What? <Yes>: \"No\"...".into()),
            title: "CON".into(),
            year: None,
            ..layout()
        };
        assert_eq!(rel(&target(root, &rough, None), root), "AC-DC/What_ _Yes_ - 'No'/03 - CON_.flac");
        assert_eq!(cap_bytes("ab.  c", 4), "ab");
        assert_eq!(cap_bytes("日本", 4), "日");
    }

    /// Long names: every name within 255 bytes, and the whole path, with
    /// room for the longest numbered suffix, within the limit Explorer opens
    /// without help, a single's extra folder and a long masters folder
    /// included; the folders give way before the title. A masters folder too
    /// long for any name gets none.
    #[test]
    fn the_whole_path_fits_with_room_for_a_number() {
        let long = LayoutInput {
            album_artist: "あ".repeat(150),
            album: Some("い".repeat(150)),
            title: "う".repeat(150),
            ..layout()
        };
        let single = LayoutInput { album: None, release_type: ReleaseType::Single, ..long.clone() };
        let roots = [PathBuf::from(r"X:\masters\archive"), PathBuf::from(r"X:\").join("deep folder ".repeat(9))];
        for root in &roots {
            for (l, tag) in [(&long, Some("[24-96]")), (&single, None)] {
                let p = target(root, l, tag);
                for c in p.components() {
                    assert!(c.as_os_str().len() <= 255, "{c:?}");
                }
                let numbered = numbered_path(&p, MAX_NAMES);
                assert!(crate::sanitize::win_len(&numbered) < MAX_MASTER_PATH, "{}", numbered.display());
                assert!(p.file_name().unwrap().to_string_lossy().starts_with("03 - う") || p.file_name().unwrap().to_string_lossy().starts_with("01 - う"));
                assert!(p.file_stem().unwrap().to_string_lossy().chars().count() >= MIN_NAME + 5, "{}", p.display());
            }
        }
        if cfg!(windows) {
            let p = target(Path::new(r"X:\m"), &long, Some("[24-96]"));
            assert_eq!(p.parent().unwrap().file_name().unwrap().to_string_lossy().chars().last(), Some(']'), "the tag stays");
        }
        assert_eq!(master_target(&PathBuf::from(r"X:\").join("x".repeat(230)), &long, None, "flac"), None);
    }

    fn write(p: &Path, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn hash_of(bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// A player of this computer that is gone: its live lock file is there
    /// and no one holds it. Each test has a pid of its own; the file goes
    /// when the guard does.
    struct Gone(u32);

    impl Gone {
        fn new(pid: u32) -> Self {
            File::options().create(true).truncate(false).write(true).open(live_lock_path(pid)).unwrap();
            Self(pid)
        }

        /// A temporary name of this gone player's on this computer.
        fn temp(&self, prefix: &str, n: u32) -> String {
            format!("{prefix}{}-{}-{n}{TEMP_END}", host_part(), self.0)
        }
    }

    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = fs::remove_file(live_lock_path(self.0));
        }
    }

    fn is_ours(name: &str) -> bool {
        temp_of(name).is_some()
    }

    #[test]
    fn a_master_takes_a_free_name_and_never_writes_over_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in").join("song.flac");
        write(&src, b"the song");
        let target = dir.path().join("m").join("A").join("01 - Song.flac");
        let other = b"another song";
        write(&target, other);
        // Another file at the very name: the next free one.
        let placed = place_master(&src, &target, &hash_of(b"the song")).unwrap();
        assert_eq!(placed, Placed { path: target.with_file_name("01 - Song (2).flac"), written: true });
        assert_eq!(fs::read(&target).unwrap(), other, "the other file is as it was");
        assert_eq!(fs::read(&placed.path).unwrap(), b"the song");
        assert!(src.exists(), "the source is the caller's to let go");
        assert_eq!(files_in(target.parent().unwrap()), vec!["01 - Song (2).flac", "01 - Song.flac"], "no temporary left");

        // The same file again: found under its name, nothing written.
        let placed = place_master(&src, &target, &hash_of(b"the song")).unwrap();
        assert_eq!(placed, Placed { path: target.with_file_name("01 - Song (2).flac"), written: false });

        // A name taken in another letter case is taken.
        let upper = dir.path().join("m").join("B").join("01 - SONG.flac");
        write(&upper, other);
        let lower = upper.with_file_name("01 - song.flac");
        let placed = place_master(&src, &lower, &hash_of(b"the song")).unwrap();
        assert_eq!(placed.path, upper.with_file_name("01 - song (2).flac"));
        assert_eq!(fs::read(&upper).unwrap(), other);

        // A source that is not what its hash says: nothing is written.
        let wrong = place_master(&src, &dir.path().join("m").join("C").join("x.flac"), &hash_of(b"else"));
        assert!(wrong.is_err());
        assert!(files_in(&dir.path().join("m").join("C")).is_empty(), "no temporary, no file");
    }

    /// A crash before the master takes its name leaves its source and a
    /// temporary only; one after it leaves the master whole and its source
    /// too. Either way nothing is lost or written over, the next start
    /// removes the temporary, and the next try takes the master as it is.
    #[test]
    fn a_crash_on_the_way_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("m");
        let src = dir.path().join("in").join("song.flac");
        write(&src, b"the song");
        let hash = hash_of(b"the song");
        let album = root.join("A");
        fs::create_dir_all(&album).unwrap();
        let target = album.join("01 - Song.flac");
        let gone = Gone::new(3_999_999_999);
        // What a player that crashed left: its temporary under its pid.
        let crashed = |tmp: &Path| {
            let to = tmp.with_file_name(gone.temp(MASTER_TEMP, 1));
            fs::rename(tmp, &to).unwrap();
            to
        };

        // Before the name: the temporary is written, nothing else.
        let tmp = stage_temp(&src, &album, 8, Some(&hash)).unwrap();
        assert!(src.exists() && !target.exists());
        assert!(is_ours(&tmp.file_name().unwrap().to_string_lossy()));
        assert_eq!(clean_stale_temps(&root), Cleaned { removed: 0, live: 1 }, "this player's own work stays");
        crashed(&tmp);
        // The start after the crash: the temporary goes, nothing else.
        assert_eq!(clean_stale_temps(&root), Cleaned { removed: 1, live: 0 });
        assert!(files_in(&album).is_empty());
        assert!(src.exists());

        // After the name, before the row was written and the source went.
        let tmp = stage_temp(&src, &album, 8, Some(&hash)).unwrap();
        fs::hard_link(&tmp, &target).unwrap();
        crashed(&tmp);
        assert_eq!(fs::read(&target).unwrap(), b"the song");
        assert!(src.exists(), "the source is still there");
        // The next try: the very file at the name, no second copy.
        let placed = place_master(&src, &target, &hash).unwrap();
        assert_eq!(placed, Placed { path: target.clone(), written: false });
        assert_eq!(clean_stale_temps(&root), Cleaned { removed: 1, live: 0 }, "the temporary's second name goes");
        assert_eq!(files_in(&album), vec!["01 - Song.flac"]);
        assert_eq!(fs::read(&target).unwrap(), b"the song");

        // A name taken by another file between the look and the link.
        let other = album.join("02 - Other.flac");
        write(&other, b"other");
        let tmp = stage_temp(&target, &album, 8, Some(&hash)).unwrap();
        assert!(!link_new(&tmp, &other).unwrap());
        assert_eq!(fs::read(&other).unwrap(), b"other", "never written over");
        assert!(tmp.exists());
    }

    /// Where a disk has no links: the move the system refuses onto a taken
    /// name (Windows), or nothing at all (elsewhere); never a file written
    /// over, never an empty file left at the name, the temporary kept.
    #[test]
    fn without_links_a_name_is_never_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("t.part");
        let other = dir.path().join("taken.flac");
        write(&tmp, b"new");
        write(&other, b"other");
        let free = dir.path().join("free.flac");
        if cfg!(windows) {
            assert!(!move_new(&tmp, &other).unwrap());
            assert_eq!(fs::read(&other).unwrap(), b"other");
            assert!(tmp.exists());
            assert!(move_new(&tmp, &free).unwrap());
            assert_eq!(fs::read(&free).unwrap(), b"new");
            assert!(!tmp.exists());
        } else {
            assert!(move_new(&tmp, &free).is_err());
            assert!(tmp.exists() && !free.exists());
        }
    }

    /// Only exactly our names, each in its own place, and only a gone
    /// player's of this computer: this player's, another running one's, one
    /// whose lock this player cannot see and another computer's stay,
    /// however old.
    #[test]
    fn the_clean_up_touches_only_a_gone_player_s_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let gone = Gone::new(3_999_999_997);
        let ours = [
            root.join("A").join("B").join(gone.temp(MASTER_TEMP, 3)),
            root.join("A").join("Singles").join("2020 - X").join(gone.temp(MASTER_TEMP, 1)),
            // A flat master's or an image's, right in the folder.
            root.join(gone.temp(MASTER_TEMP, 0)),
            root.join(BACKUP_DIR).join(gone.temp(DB_TEMP, 0)),
        ];
        // A running player: it holds its live lock.
        let running = 3_999_999_996u32;
        let lock = File::options().create(true).truncate(false).write(true).open(live_lock_path(running)).unwrap();
        lock.lock().unwrap();
        let host = host_part();
        let elsewhere = if host == "0badf00d" { "0badf00e" } else { "0badf00d" };
        let theirs = [
            root.join("abc.flac"),
            root.join("abc.json"),
            root.join(".abc.flac.nikgob-part"),
            root.join("A").join(BACKUP_DIR).join(gone.temp(DB_TEMP, 0)),
            root.join(BACKUP_DIR).join(gone.temp(MASTER_TEMP, 0)),
            root.join("A").join("B").join("01 - Song.flac"),
            root.join("A").join("B").join(".nikgob-master-.part"),
            root.join("A").join("B").join(".nikgob-master-1.part"),
            root.join("A").join("B").join(".nikgob-master-1-2.part"),
            root.join("A").join("B").join(format!(".nikgob-master-{host}-1--2.part")),
            root.join("A").join("B").join(format!(".nikgob-master-{host}-1-2-3.part")),
            root.join("A").join("B").join(format!(".nikgob-master-{host}-x1-2.part")),
            root.join("A").join("B").join(format!(".nikgob-master-{}-1-2.part", host.to_uppercase().replace(char::is_numeric, "A"))),
            root.join("A").join("B").join(format!("nikgob-master-{host}-1-1.part")),
            root.join("A").join("B").join(format!(".nikgob-master-{host}-1-1.part.flac")),
            root.join("A").join("B").join(format!(".nikgob-master-{host}-{running}-0.part")),
            root.join("A").join("B").join(format!(".nikgob-master-{host}-{}-0.part", std::process::id())),
            // No lock file to look at: whether it runs is not known.
            root.join("A").join("B").join(format!(".nikgob-master-{host}-3999999995-0.part")),
            // Another computer's, over a shared disk.
            root.join("A").join("B").join(format!(".nikgob-master-{elsewhere}-{}-0.part", gone.0)),
            root.join(format!(".nikgob-master-{elsewhere}-{}-1.part", gone.0)),
            root.join(BACKUP_DIR).join("library-2026-11-14.sqlite"),
        ];
        for p in ours.iter().chain(&theirs) {
            write(p, b"x");
        }
        let cleaned = clean_stale_temps(root);
        assert_eq!(cleaned, Cleaned { removed: ours.len(), live: 3 });
        assert!(ours.iter().all(|p| !p.exists()));
        assert!(theirs.iter().all(|p| p.exists()), "{:?}", theirs.iter().filter(|p| !p.exists()).collect::<Vec<_>>());
        drop(lock);
        // Once that player is gone, its temporary goes too.
        assert_eq!(clean_stale_temps(root).removed, 1);
        let _ = fs::remove_file(live_lock_path(running));
    }

    #[test]
    fn a_cover_is_kept_once_per_album_folder() {
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("Music").join("cover.jpg");
        write(&cover, b"jpeg");
        let album = dir.path().join("m").join("A");
        fs::create_dir_all(&album).unwrap();
        keep_cover(&album, &cover);
        assert_eq!(fs::read(album.join(COVER_FILE)).unwrap(), b"jpeg");
        let newer = dir.path().join("cover2.jpg");
        write(&newer, b"newer");
        keep_cover(&album, &newer);
        assert_eq!(fs::read(album.join(COVER_FILE)).unwrap(), b"jpeg", "never written over");
        assert_eq!(files_in(&album), vec![COVER_FILE]);
    }

    #[test]
    fn an_older_master_is_found_by_its_hash_a_new_one_by_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ImportConfig::with_root(dir.path()).with_masters_dir(dir.path().join("m"));
        let root = cfg.masters_dir();
        let old = root.join("abc.flac");
        write(&old, b"old");
        write(&root.join("abc.json"), b"{}");
        write(&root.join("abc.cue"), b"FILE");
        let m = Master {
            track_id: TrackId::new(),
            codec: nikgob_core::model::Codec::Flac,
            file_hash: "abc".into(),
            size_bytes: 3,
            origin: None,
            file_path: None,
            replicas: Vec::new(),
        };
        assert_eq!(master_file(&cfg, &m), Some(old.clone()));
        let named = root.join("A").join("B").join("01 - C.flac");
        write(&named, b"new");
        let n = Master { file_hash: "def".into(), file_path: Some(named.to_string_lossy().into_owned()), ..m.clone() };
        assert_eq!(master_file(&cfg, &n), Some(named));
        let gone = Master { file_path: Some(root.join("gone.flac").to_string_lossy().into_owned()), ..m };
        assert_eq!(master_file(&cfg, &gone), None, "a flat file of the same hash is not this master's");
    }

    /// An image is known by its kept sheet or its sidecar, however many of
    /// its tracks the library has.
    #[test]
    fn a_disc_image_is_known_by_its_sheet() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(!is_image_master(root, "a"));
        write(&root.join("a.cue"), b"FILE");
        assert!(is_image_master(root, "a"));
        write(&root.join("b.json"), br#"{"track_id":"x","cue":"disc.cue"}"#);
        assert!(is_image_master(root, "b"));
        write(&root.join("c.json"), br#"{"track_id":"x"}"#);
        assert!(!is_image_master(root, "c"));
    }

    #[test]
    fn a_day_has_one_copy_and_seven_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path().join("library.sqlite")).unwrap();
        let root = dir.path().join("m");

        // The masters folder is not there: nothing, and it is not made.
        assert_eq!(copy_database(&lib, &root, "2026-11-14", BACKUP_KEEP).unwrap(), DbCopy::NoFolder);
        assert!(!root.exists());

        fs::create_dir_all(root.join(BACKUP_DIR)).unwrap();
        let mut db = b"SQLite format 3\0".to_vec();
        db.extend_from_slice(&[0; 84]);
        for day in 1..=8 {
            write(&root.join(BACKUP_DIR).join(format!("library-2026-11-{day:02}.sqlite")), &db);
        }
        // Not copies, whatever their names: never counted, never gone.
        let not_ours = [
            root.join(BACKUP_DIR).join("library-old.sqlite"),
            root.join(BACKUP_DIR).join("notes.txt"),
            root.join(BACKUP_DIR).join("library-2026-10-01.sqlite"),
        ];
        for p in &not_ours {
            write(p, b"");
        }
        let made = copy_database(&lib, &root, "2026-11-14", BACKUP_KEEP).unwrap();
        let to = root.join(BACKUP_DIR).join("library-2026-11-14.sqlite");
        assert_eq!(made, DbCopy::Made(to.clone()));
        assert!(Library::open(&to).is_ok(), "a whole database");
        let mut copies: Vec<String> = files_in(&root.join(BACKUP_DIR))
            .into_iter()
            .filter(|n| is_copy_name(n) && is_database(&root.join(BACKUP_DIR).join(n)))
            .collect();
        copies.sort();
        assert_eq!(copies.len(), BACKUP_KEEP);
        assert_eq!(copies.first().map(String::as_str), Some("library-2026-11-03.sqlite"));
        assert!(not_ours.iter().all(|p| p.exists()), "only our copies go");
        assert!(!files_in(&root.join(BACKUP_DIR)).iter().any(|n| is_ours(n)), "no temporary left");

        // The same day again: no second copy.
        assert_eq!(copy_database(&lib, &root, "2026-11-14", BACKUP_KEEP).unwrap(), DbCopy::Had);
        assert!(is_copy_name("library-2026-01-31.sqlite"));
        assert!(!is_copy_name("library-2026-1-31.sqlite"));
        assert!(!is_copy_name("library-2026-01-31.sqlite.bak"));

        // An empty file (or a folder) at the day's name is not the day's
        // copy: the day gets none, and what is there stays as it is.
        let empty = root.join(BACKUP_DIR).join("library-2026-11-15.sqlite");
        write(&empty, b"");
        assert!(copy_database(&lib, &root, "2026-11-15", BACKUP_KEEP).is_err());
        assert_eq!(fs::read(&empty).unwrap(), b"");
        fs::create_dir_all(root.join(BACKUP_DIR).join("library-2026-11-16.sqlite")).unwrap();
        assert!(copy_database(&lib, &root, "2026-11-16", BACKUP_KEEP).is_err());
        assert!(root.join(BACKUP_DIR).join("library-2026-11-16.sqlite").is_dir());
    }
}
