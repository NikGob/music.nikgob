//! File-system helpers: moving across volumes, hashing, scanning, dates.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{ImportError, Result};
use crate::sanitize::{long_path, numbered_path};

/// Extensions the importer understands as audio.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "mp4", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff",
];

pub fn is_audio_file(path: &Path) -> bool {
    has_extension(path, AUDIO_EXTENSIONS)
}

pub fn is_torrent_file(path: &Path) -> bool {
    has_extension(path, &["torrent"])
}

fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

pub fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ImportError + '_ {
    move |e| ImportError::io(path, e)
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(long_path(path)).map_err(io_err(path))
}

/// Moves `from` to `to`: a rename when possible, copy + delete across volumes.
pub fn move_file(from: &Path, to: &Path) -> Result<()> {
    let (f, t) = (long_path(from), long_path(to));
    if fs::rename(&f, &t).is_ok() {
        return Ok(());
    }
    if let Err(e) = fs::copy(&f, &t) {
        // A half-written copy must not be left behind as if it were the file.
        let _ = fs::remove_file(&t);
        return Err(ImportError::io(to, e));
    }
    fs::remove_file(&f).map_err(|e| {
        // The move failed as a whole (source locked): do not leave two files.
        let _ = fs::remove_file(&t);
        ImportError::io(from, e)
    })
}

/// Like [`crate::sanitize::unique_path`], but the name is *claimed*: an empty
/// placeholder is created atomically, so two threads (or two imports) can never
/// pick the same free name. The caller moves its file over the placeholder (a
/// rename replaces it) and removes it if that fails.
pub fn reserve_unique(path: &Path) -> Result<PathBuf> {
    let mut n = 1u32;
    loop {
        let candidate = if n == 1 {
            path.to_path_buf()
        } else {
            numbered_path(path, n)
        };
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(long_path(&candidate))
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(ImportError::io(&candidate, e)),
        }
    }
}

pub fn copy_file(from: &Path, to: &Path) -> Result<u64> {
    fs::copy(long_path(from), long_path(to)).map_err(io_err(to))
}

/// Copies `from` to `to` via a temporary `.nikgob-part` sibling, flushes it to
/// disk (`sync_all`), verifies that size and BLAKE3 hash match `expected_hash`
/// (or the source file's hash), and atomically renames into `to`.
pub fn copy_file_verified(from: &Path, to: &Path, expected_hash: Option<&str>) -> Result<String> {
    let expected = match expected_hash {
        Some(h) if !h.is_empty() => h.to_ascii_lowercase(),
        _ => hash_file(from)?,
    };
    let expected_size = file_size(from)?;
    if let Some(parent) = to.parent() {
        create_dir_all(parent)?;
    }
    if long_path(to).exists() {
        if file_size(to).ok() == Some(expected_size)
            && hash_file(to).ok().as_deref() == Some(expected.as_str())
        {
            return Ok(expected);
        }
        remove_file(to)?;
    }
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_owned());
    let tmp = to.with_file_name(format!(".{name}.nikgob-part"));
    let _ = remove_file(&tmp);
    {
        let mut reader = File::open(long_path(from)).map_err(io_err(from))?;
        let mut writer = File::create(long_path(&tmp)).map_err(io_err(&tmp))?;
        std::io::copy(&mut reader, &mut writer).map_err(io_err(&tmp))?;
        writer.sync_all().map_err(io_err(&tmp))?;
    }
    let actual_size = file_size(&tmp)?;
    let actual_hash = hash_file(&tmp)?;
    if actual_size != expected_size || actual_hash != expected {
        let _ = remove_file(&tmp);
        return Err(ImportError::Other(format!(
            "проверка хеша не прошла для {}: ожидалось {expected} ({expected_size} B), получено {actual_hash} ({actual_size} B)",
            to.display()
        )));
    }
    fs::rename(long_path(&tmp), long_path(to)).map_err(|e| {
        let _ = remove_file(&tmp);
        ImportError::io(to, e)
    })?;
    Ok(expected)
}

/// Transfers `from` to `to`, verifies the destination file's size and BLAKE3
/// hash on disk, and deletes `from` **only** after verification succeeds.
pub fn move_file_verified(from: &Path, to: &Path, expected_hash: Option<&str>) -> Result<String> {
    if from == to {
        let expected = match expected_hash {
            Some(h) if !h.is_empty() => h.to_ascii_lowercase(),
            _ => hash_file(from)?,
        };
        let actual = hash_file(to)?;
        if actual != expected {
            return Err(ImportError::Other(format!(
                "проверка хеша не прошла для {}: ожидалось {expected}, получено {actual}",
                to.display()
            )));
        }
        return Ok(expected);
    }
    let hash = copy_file_verified(from, to, expected_hash)?;
    fs::remove_file(long_path(from)).map_err(|e| {
        let _ = remove_file(to);
        ImportError::io(from, e)
    })?;
    Ok(hash)
}

pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(long_path(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ImportError::io(path, e)),
    }
}

/// blake3 of the file content, lowercase hex.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(long_path(path)).map_err(io_err(path))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn file_size(path: &Path) -> Result<u64> {
    fs::metadata(long_path(path))
        .map(|m| m.len())
        .map_err(io_err(path))
}

/// Audio and `.torrent` files below `roots` (a file is taken as is), sorted.
pub fn scan(roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let found = scan_import(roots);
    (found.audio, found.torrents)
}

/// What [`scan_import`] found.
#[derive(Debug, Default)]
pub struct Scanned {
    pub audio: Vec<PathBuf>,
    pub torrents: Vec<PathBuf>,
    /// Cue sheets (`.cue`): the index cards of single-file rips.
    pub cues: Vec<PathBuf>,
    /// APE and WavPack files: imported only as the images of a sheet.
    pub images: Vec<PathBuf>,
}

/// [`scan`] that also finds cue sheets.
pub fn scan_import(roots: &[PathBuf]) -> Scanned {
    let mut found = Scanned::default();
    for root in roots {
        walk(root, &mut found, 0);
    }
    for list in [&mut found.audio, &mut found.torrents, &mut found.cues, &mut found.images] {
        list.sort();
        list.dedup();
    }
    found
}

pub fn is_cue_file(path: &Path) -> bool {
    has_extension(path, &["cue"])
}

/// APE and WavPack: lossless images the pipeline reads only through a sheet.
pub fn is_sheet_image_file(path: &Path) -> bool {
    has_extension(path, &["ape", "wv"])
}

fn walk(path: &Path, found: &mut Scanned, depth: usize) {
    let p = long_path(path);
    let Ok(meta) = fs::metadata(&p) else {
        return;
    };
    if meta.is_file() {
        if is_audio_file(path) {
            found.audio.push(path.to_path_buf());
        } else if is_torrent_file(path) {
            found.torrents.push(path.to_path_buf());
        } else if is_cue_file(path) {
            found.cues.push(path.to_path_buf());
        } else if is_sheet_image_file(path) {
            found.images.push(path.to_path_buf());
        }
        return;
    }
    if !meta.is_dir() || depth > 12 {
        return;
    }
    let Ok(rd) = fs::read_dir(&p) else {
        return;
    };
    for entry in rd.flatten() {
        let name = entry.file_name();
        // Hidden files include our own temp files and the .musiclib directory.
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        walk(&path.join(&name), found, depth + 1);
    }
}

pub fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `(year, month, day)` in UTC for a unix timestamp (civil-from-days algorithm).
pub fn ymd_from_unix(secs: i64) -> (i32, u32, u32) {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// Removes `dir` and its parents (up to but excluding `stop`) when they hold no
/// audio files any more; leftover cover art and lyrics files go with them.
pub fn prune_empty_dirs(dir: &Path, stop: &Path) {
    let mut cur = dir.to_path_buf();
    while cur != stop && cur.starts_with(stop) {
        let p = long_path(&cur);
        let Ok(rd) = fs::read_dir(&p) else {
            break;
        };
        let entries: Vec<_> = rd.flatten().collect();
        let has_audio = entries.iter().any(|e| {
            let path = e.path();
            path.is_dir() || is_audio_file(&path)
        });
        if has_audio {
            break;
        }
        for e in &entries {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let leftover = matches!(
                name.as_str(),
                "cover.jpg" | "ambient.png" | "desktop.ini" | "thumbs.db"
            ) || name.ends_with(".lrc");
            if !leftover {
                return;
            }
        }
        for e in entries {
            let _ = fs::remove_file(e.path());
        }
        if fs::remove_dir(&p).is_err() {
            break;
        }
        match cur.parent() {
            Some(parent) => cur = parent.to_path_buf(),
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(ymd_from_unix(0), (1970, 1, 1));
        assert_eq!(ymd_from_unix(951_782_400), (2000, 2, 29));
        assert_eq!(ymd_from_unix(1_791_504_000), (2026, 10, 9));
        assert_eq!(ymd_from_unix(-86_400), (1969, 12, 31));
    }

    #[test]
    fn hashing_and_moving() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        std::fs::write(&a, b"hello").unwrap();
        let h = hash_file(&a).unwrap();
        assert_eq!(h, blake3::hash(b"hello").to_hex().to_string());
        let b = dir.path().join("sub");
        create_dir_all(&b).unwrap();
        let moved = b.join("b.bin");
        move_file(&a, &moved).unwrap();
        assert!(!a.exists() && moved.exists());
        assert_eq!(hash_file(&moved).unwrap(), h);
        remove_file(&a).unwrap();
    }

    #[test]
    fn scan_finds_audio_and_torrents_but_skips_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let album = dir.path().join("Album");
        std::fs::create_dir_all(album.join("CD2")).unwrap();
        std::fs::create_dir_all(dir.path().join(".musiclib")).unwrap();
        for f in [
            "Album/01.flac",
            "Album/CD2/02.MP3",
            "Album/cover.jpg",
            "x.torrent",
            ".musiclib/hidden.flac",
            "Album/.tmp.opus",
        ] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let (audio, torrents) = scan(&[dir.path().to_path_buf()]);
        assert_eq!(audio.len(), 2, "{audio:?}");
        assert_eq!(torrents.len(), 1);
        // A single file path is taken as is.
        let (one, _) = scan(&[album.join("01.flac")]);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn prune_removes_only_audio_free_dirs() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("Artist").join("Album");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("cover.jpg"), b"x").unwrap();
        std::fs::write(a.join("ambient.png"), b"x").unwrap();
        prune_empty_dirs(&a, root.path());
        assert!(!root.path().join("Artist").exists());
        // With audio present nothing is touched.
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("01 - T.opus"), b"x").unwrap();
        std::fs::write(a.join("cover.jpg"), b"x").unwrap();
        prune_empty_dirs(&a, root.path());
        assert!(a.join("cover.jpg").exists());
        // Unknown files protect the folder.
        std::fs::remove_file(a.join("01 - T.opus")).unwrap();
        std::fs::write(a.join("notes.txt"), b"x").unwrap();
        prune_empty_dirs(&a, root.path());
        assert!(a.join("notes.txt").exists());
    }
}
