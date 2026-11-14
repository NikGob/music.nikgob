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
//! BLAKE3), then takes its name without ever replacing a file (a hard link,
//! or a rename into a name seen free where the disk has no links), and only
//! then is its path recorded in the library. A crash leaves at worst a
//! temporary file, removed at a later start ([`clean_stale_temps`]). A name
//! that holds another file is never written over: the next free one
//! (`name (2).flac`, …) is taken.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use nikgob_core::model::{Master, ReleaseType, TrackId};
use nikgob_library::Library;

use crate::config::ImportConfig;
use crate::error::{ImportError, Result};
use crate::fsutil;
use crate::layout::{LayoutInput, is_various_artists};
use crate::sanitize::{fit_stem, long_path, numbered_path};

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
/// Windows and Linux, capped in letters and bytes, and the whole path fits
/// [`MAX_MASTER_PATH`].
pub fn master_target(root: &Path, layout: &LayoutInput, tag: Option<&str>, ext: &str) -> PathBuf {
    let album = layout.album_dir(Path::new(""));
    let parts: Vec<String> = album.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let mut dir = root.to_path_buf();
    let last = parts.len().saturating_sub(1);
    for (i, part) in parts.iter().enumerate() {
        let mut name = cap_bytes(part, MAX_DIR_BYTES);
        if i == last
            && let Some(tag) = tag
        {
            name = format!("{name} {tag}");
        }
        dir.push(name);
    }
    let mut named = layout.clone();
    let artist = layout.artist.trim();
    if layout.is_compilation() && !artist.is_empty() && !is_various_artists(artist) {
        let title = if layout.title.trim().is_empty() { "Untitled" } else { layout.title.trim() };
        named.title = format!("{artist} - {title}");
    }
    let (stem, prefix) = named.file_stem();
    let stem = cap_bytes(&stem, MAX_STEM_BYTES);
    let stem = fit_stem(&dir, &stem, ext, MAX_MASTER_PATH, prefix);
    dir.join(format!("{stem}.{ext}"))
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
fn same_content(path: &Path, size: u64, hash: &str) -> bool {
    fsutil::file_size(path).ok() == Some(size) && fsutil::hash_file(path).ok().is_some_and(|h| h.eq_ignore_ascii_case(hash))
}

/// `target`, then `target (2)`, … up to [`MAX_NAMES`].
fn candidate(target: &Path, n: u32) -> PathBuf {
    if n == 1 { target.to_path_buf() } else { numbered_path(target, n) }
}

/// Flushes the folder's entries to the disk where that is a thing one can
/// ask for (not on Windows, where a file's own flush is what there is).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// A fresh temporary file of ours in `dir` (`<prefix><pid>-<n>.part`),
/// made with `create_new`: never a file that is there.
fn new_temp(dir: &Path, prefix: &str) -> Result<(PathBuf, File)> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    for _ in 0..1000 {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let p = dir.join(format!("{prefix}{}-{n}{TEMP_END}", std::process::id()));
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
/// already: a hard link (refused by the system when the name is taken),
/// then the temporary name goes. On a disk without links (FAT, exFAT) a
/// rename into the name, checked free just before (one writer of the
/// masters folder at a time, [`crate::master_writes`]). False when the name
/// is taken; `tmp` stays for the caller then.
fn link_new(tmp: &Path, to: &Path) -> Result<bool> {
    match fs::hard_link(long_path(tmp), long_path(to)) {
        Ok(()) => {
            if let Err(e) = fs::remove_file(long_path(tmp)) {
                // A second name of the same file: a later start removes it.
                log::warn!("masters: the temporary {} stays: {e}", tmp.display());
            }
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(_) => {
            if long_path(to).exists() {
                return Ok(false);
            }
            fs::rename(long_path(tmp), long_path(to)).map_err(fsutil::io_err(to))?;
        }
    }
    if let Some(dir) = to.parent() {
        sync_dir(dir);
    }
    Ok(true)
}

/// What [`place_master`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Where the master is now.
    pub path: PathBuf,
    /// It was written now (false: the very file was there already).
    pub written: bool,
}

/// Puts the file `src` (BLAKE3 `hash`) at `target` or the first free
/// numbered name after it, see the module docs. The same file under one of
/// those names already is taken as it is. With `take`, `src` goes once its
/// copy is in place (a file from the Inbox); a `src` that cannot go stays
/// (logged), the master is kept.
pub fn place_master(src: &Path, target: &Path, hash: &str, take: bool) -> Result<Placed> {
    let dir = target.parent().ok_or_else(|| ImportError::Other(format!("{} has no folder", target.display())))?;
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
    let placed = placed.ok_or_else(|| ImportError::Other(format!("{}: no free name", target.display())))?;
    if take
        && long_path(src).exists()
        && let Err(e) = fsutil::remove_file(src)
    {
        log::warn!("masters: {} is kept at {}, {} stays: {e}", hash, placed.path.display(), src.display());
    }
    Ok(placed)
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
/// kept by name, else `{file_hash}.<ext>` right in the folder (not the
/// `.json` beside it, not the `.cue` kept with an image).
pub fn master_file(cfg: &ImportConfig, m: &Master) -> Option<PathBuf> {
    if let Some(p) = m.file_path.as_deref().map(PathBuf::from)
        && long_path(&p).is_file()
    {
        return Some(p);
    }
    flat_master(&cfg.masters_dir(), &m.file_hash)
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

/// A torrent's payload file `from` that is the master of the one track of
/// `m`, kept by name in the masters folder: copied there (the payload is
/// left to the caller), its row pointed at the copy. A copy an earlier try
/// made is taken as it is. Returns where it is kept; None when the track is
/// not there to name it (the caller keeps it the older way).
pub fn keep_torrent_master(lib: &Library, cfg: &ImportConfig, from: &Path, m: &Master) -> Result<Option<PathBuf>> {
    let size = fsutil::file_size(from)?;
    if let Some(p) = m.file_path.as_deref().map(PathBuf::from)
        && long_path(&p).is_file()
        && same_content(&p, size, &m.file_hash)
    {
        return Ok(Some(p));
    }
    let Some(layout) = layout_of_track(lib, m.track_id) else { return Ok(None) };
    let info = crate::tags::probe_info(from).ok();
    let tag = info.as_ref().and_then(|i| hi_res_tag(i.bit_depth, i.sample_rate));
    let ext = from.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "flac".to_owned());
    let root = cfg.masters_dir();
    let target = master_target(&root, &layout, tag.as_deref(), &ext);
    let placed = place_master(from, &target, &m.file_hash, false)?;
    lib.set_master_file(m.track_id, &placed.path.to_string_lossy())?;
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

/// Is `name` one of our temporary files (`.nikgob-master-<pid>-<n>.part`,
/// `.nikgob-db-<pid>-<n>.part`)?
fn is_temp_name(name: &str) -> bool {
    [MASTER_TEMP, DB_TEMP].iter().any(|prefix| {
        name.strip_prefix(prefix).and_then(|n| n.strip_suffix(TEMP_END)).is_some_and(|mid| {
            !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit() || b == b'-') && mid.bytes().any(|b| b.is_ascii_digit())
        })
    })
}

/// Removes our temporary files ([`is_temp_name`]) older than `min_age` in
/// `root` and its folders (four levels down, as deep as an album folder of
/// a single goes): what a crash left on the way. Nothing else is touched.
/// Returns how many went.
pub fn clean_stale_temps(root: &Path, min_age: Duration) -> usize {
    fn walk(dir: &Path, depth: usize, min_age: Duration, now: SystemTime, n: &mut usize) {
        let Ok(rd) = fs::read_dir(long_path(dir)) else { return };
        for e in rd.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            let path = dir.join(e.file_name());
            if kind.is_dir() {
                if depth < 4 {
                    walk(&path, depth + 1, min_age, now, n);
                }
            } else if kind.is_file() && is_temp_name(&e.file_name().to_string_lossy()) {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .map(|t| now.duration_since(t).unwrap_or_default())
                    .is_some_and(|age| age >= min_age);
                if old {
                    match fs::remove_file(long_path(&path)) {
                        Ok(()) => *n += 1,
                        Err(err) => log::warn!("masters: the temporary {} stays: {err}", path.display()),
                    }
                }
            }
        }
    }
    let mut n = 0;
    walk(root, 0, min_age, SystemTime::now(), &mut n);
    n
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

/// One copy of the library's database for the day `day` into
/// `<root>/.nikgob-backup/library-<day>.sqlite` (SQLite's online backup),
/// then the newest `keep` copies kept: only files of exactly that name go.
/// A day that has its copy gets no other; a missing `root` gets nothing
/// (the folder is never made). Blocking: never on the player loop.
pub fn copy_database(lib: &Library, root: &Path, day: &str, keep: usize) -> Result<DbCopy> {
    if !long_path(root).is_dir() {
        return Ok(DbCopy::NoFolder);
    }
    let dir = root.join(BACKUP_DIR);
    let to = dir.join(format!("library-{day}.sqlite"));
    if long_path(&to).exists() {
        return Ok(DbCopy::Had);
    }
    match fs::create_dir(long_path(&dir)) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(ImportError::io(&dir, e)),
    }
    let (tmp, file) = new_temp(&dir, DB_TEMP)?;
    drop(file);
    let made = (|| -> Result<()> {
        lib.backup_to(&tmp).map_err(|e| ImportError::Other(format!("{}: {e}", to.display())))?;
        // Flushed through a handle that may write (Windows flushes no other).
        File::options().write(true).open(long_path(&tmp)).and_then(|f| f.sync_all()).map_err(fsutil::io_err(&tmp))?;
        if !link_new(&tmp, &to)? {
            let _ = fsutil::remove_file(&tmp);
        }
        Ok(())
    })();
    if let Err(e) = made {
        let _ = fsutil::remove_file(&tmp);
        return Err(e);
    }
    rotate_copies(&dir, keep);
    Ok(DbCopy::Made(to))
}

/// Keeps the newest `keep` database copies in `dir` (by the day in their
/// names); only files named as a copy ([`is_copy_name`]) ever go.
fn rotate_copies(dir: &Path, keep: usize) {
    let mut copies: Vec<PathBuf> = fs::read_dir(long_path(dir))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_copy_name(&e.file_name().to_string_lossy()))
                .map(|e| dir.join(e.file_name()))
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

    #[test]
    fn a_master_goes_where_its_album_does() {
        let root = Path::new("/m");
        assert_eq!(rel(&master_target(root, &layout(), None, "flac"), root), "Кино/1988 - Группа крови/03 - Группа крови.flac");
        let two = LayoutInput { disc_no: 2, disc_total: Some(2), ..layout() };
        assert_eq!(rel(&master_target(root, &two, None, "flac"), root), "Кино/1988 - Группа крови/2-03 - Группа крови.flac");
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
        let p = master_target(root, &layout(), hi_res_tag(Some(24), Some(48_000)).as_deref(), "flac");
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
            rel(&master_target(root, &va, None, "flac"), root),
            "Various Artists/2018 - Spider-Man - Into the Spider-Verse/02 - Post Malone & Swae Lee - Sunflower.flac"
        );
        // A compilation whose track names no artist of its own.
        let bare = LayoutInput { artist: "Various Artists".into(), ..va };
        assert_eq!(rel(&master_target(root, &bare, None, "flac"), root), "Various Artists/2018 - Spider-Man - Into the Spider-Verse/02 - Sunflower.flac");
    }

    #[test]
    fn a_track_without_an_album_is_a_single() {
        let root = Path::new("/m");
        let single = LayoutInput { album: None, track_no: 0, year: Some(2021), title: "Песня".into(), ..layout() };
        assert_eq!(rel(&master_target(root, &single, None, "flac"), root), "Кино/Singles/2021 - Песня/01 - Песня.flac");
        let nobody = LayoutInput { album_artist: String::new(), artist: " ".into(), album: None, year: None, title: String::new(), track_no: 0, ..layout() };
        assert_eq!(rel(&master_target(root, &nobody, None, "flac"), root), "Unknown Artist/Singles/Untitled/01 - Untitled.flac");
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
        assert_eq!(rel(&master_target(root, &rough, None, "flac"), root), "AC-DC/What_ _Yes_ - 'No'/03 - CON_.flac");
        // Long Japanese names: every name within 255 bytes, the path within
        // the limit Explorer opens without help.
        let long = LayoutInput {
            album_artist: "あ".repeat(150),
            album: Some("い".repeat(150)),
            title: "う".repeat(150),
            ..layout()
        };
        let p = master_target(Path::new(r"X:\masters\archive"), &long, Some("[24-96]"), "flac");
        for c in p.components().skip(1) {
            assert!(c.as_os_str().len() <= 255, "{c:?}");
        }
        assert!(crate::sanitize::win_len(&p) < MAX_MASTER_PATH, "{}", p.display());
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("03 - う"));
        assert_eq!(cap_bytes("ab.  c", 4), "ab");
        assert_eq!(cap_bytes("日本", 4), "日");
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

    #[test]
    fn a_master_takes_a_free_name_and_never_writes_over_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in").join("song.flac");
        write(&src, b"the song");
        let target = dir.path().join("m").join("A").join("01 - Song.flac");
        let other = b"another song";
        write(&target, other);
        // Another file at the very name: the next free one.
        let placed = place_master(&src, &target, &hash_of(b"the song"), true).unwrap();
        assert_eq!(placed, Placed { path: target.with_file_name("01 - Song (2).flac"), written: true });
        assert_eq!(fs::read(&target).unwrap(), other, "the other file is as it was");
        assert_eq!(fs::read(&placed.path).unwrap(), b"the song");
        assert!(!src.exists(), "taken from the Inbox once in place");
        assert_eq!(files_in(target.parent().unwrap()), vec!["01 - Song (2).flac", "01 - Song.flac"], "no temporary left");

        // The same file again: found under its name, nothing written.
        let again = dir.path().join("in").join("again.flac");
        write(&again, b"the song");
        let placed = place_master(&again, &target, &hash_of(b"the song"), false).unwrap();
        assert_eq!(placed, Placed { path: target.with_file_name("01 - Song (2).flac"), written: false });
        assert!(again.exists(), "a file not taken stays");

        // A name taken in another letter case is taken.
        let upper = dir.path().join("m").join("B").join("01 - SONG.flac");
        write(&upper, other);
        let lower = upper.with_file_name("01 - song.flac");
        let placed = place_master(&again, &lower, &hash_of(b"the song"), false).unwrap();
        assert_eq!(placed.path, upper.with_file_name("01 - song (2).flac"));
        assert_eq!(fs::read(&upper).unwrap(), other);

        // A source that is not what its hash says: nothing is written.
        let wrong = place_master(&again, &dir.path().join("m").join("C").join("x.flac"), &hash_of(b"else"), false);
        assert!(wrong.is_err());
        assert!(files_in(&dir.path().join("m").join("C")).is_empty(), "no temporary, no file");
    }

    /// A crash before the master takes its name leaves its source and a
    /// temporary only; one after it leaves the master whole and its source
    /// too. Either way nothing is lost or written over, and the next try
    /// takes the master as it is.
    #[test]
    fn a_crash_on_the_way_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in").join("song.flac");
        write(&src, b"the song");
        let hash = hash_of(b"the song");
        let album = dir.path().join("m").join("A");
        fs::create_dir_all(&album).unwrap();
        let target = album.join("01 - Song.flac");

        // Before the name: the temporary is written, nothing else.
        let tmp = stage_temp(&src, &album, 8, Some(&hash)).unwrap();
        assert!(src.exists() && !target.exists());
        assert!(is_temp_name(&tmp.file_name().unwrap().to_string_lossy()));
        // The start after the crash: the temporary goes, nothing else.
        assert_eq!(clean_stale_temps(&dir.path().join("m"), Duration::ZERO), 1);
        assert!(files_in(&album).is_empty());
        assert!(src.exists());

        // After the name, before the source went and the row was written.
        let tmp = stage_temp(&src, &album, 8, Some(&hash)).unwrap();
        fs::hard_link(&tmp, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"the song");
        assert!(src.exists(), "the source is still there");
        // The next try: the very file at the name, no second copy.
        let placed = place_master(&src, &target, &hash, true).unwrap();
        assert_eq!(placed, Placed { path: target.clone(), written: false });
        assert!(!src.exists());
        assert_eq!(clean_stale_temps(&dir.path().join("m"), Duration::ZERO), 1, "the temporary's second name goes");
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

    #[test]
    fn the_clean_up_touches_only_our_temporaries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ours = [
            root.join(".nikgob-master-12-0.part"),
            root.join("A").join("B").join(".nikgob-master-7-3.part"),
            root.join("A").join("Singles").join("2020 - X").join(".nikgob-master-1-1.part"),
            root.join(BACKUP_DIR).join(".nikgob-db-5-0.part"),
        ];
        let theirs = [
            root.join("abc.flac"),
            root.join("abc.json"),
            root.join(".abc.flac.nikgob-part"),
            root.join("A").join("B").join("01 - Song.flac"),
            root.join("A").join("B").join(".nikgob-master-.part"),
            root.join("A").join("B").join(".nikgob-master-x1.part"),
            root.join("A").join("B").join("nikgob-master-1-1.part"),
            root.join("A").join("B").join(".nikgob-master-1-1.part.flac"),
            root.join(BACKUP_DIR).join("library-2026-11-14.sqlite"),
        ];
        for p in ours.iter().chain(&theirs) {
            write(p, b"x");
        }
        // Young ones may be on their way right now: they stay.
        assert_eq!(clean_stale_temps(root, Duration::from_secs(3600)), 0);
        assert_eq!(clean_stale_temps(root, Duration::ZERO), ours.len());
        assert!(ours.iter().all(|p| !p.exists()));
        assert!(theirs.iter().all(|p| p.exists()));
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
        assert_eq!(master_file(&cfg, &gone), Some(old), "a path that is gone falls back to the hash");
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
        for day in 1..=8 {
            write(&root.join(BACKUP_DIR).join(format!("library-2026-11-{day:02}.sqlite")), b"old");
        }
        let not_ours = [root.join(BACKUP_DIR).join("library-old.sqlite"), root.join(BACKUP_DIR).join("notes.txt")];
        for p in &not_ours {
            write(p, b"keep");
        }
        let made = copy_database(&lib, &root, "2026-11-14", BACKUP_KEEP).unwrap();
        let to = root.join(BACKUP_DIR).join("library-2026-11-14.sqlite");
        assert_eq!(made, DbCopy::Made(to.clone()));
        assert!(Library::open(&to).is_ok(), "a whole database");
        let mut copies: Vec<String> = files_in(&root.join(BACKUP_DIR)).into_iter().filter(|n| is_copy_name(n)).collect();
        copies.sort();
        assert_eq!(copies.len(), BACKUP_KEEP);
        assert_eq!(copies.first().map(String::as_str), Some("library-2026-11-03.sqlite"));
        assert!(not_ours.iter().all(|p| p.exists()), "only our copies go");
        assert!(!files_in(&root.join(BACKUP_DIR)).iter().any(|n| is_temp_name(n)), "no temporary left");

        // The same day again: no second copy.
        assert_eq!(copy_database(&lib, &root, "2026-11-14", BACKUP_KEEP).unwrap(), DbCopy::Had);
        assert!(is_copy_name("library-2026-01-31.sqlite"));
        assert!(!is_copy_name("library-2026-1-31.sqlite"));
        assert!(!is_copy_name("library-2026-01-31.sqlite.bak"));
    }
}
