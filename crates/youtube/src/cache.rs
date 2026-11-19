//! Recently played YouTube audio on disk, so going back or repeating a
//! track does not download it again.
//!
//! One file per video and format (`{id}.{itag}.webm` / `.m4a`, the bytes as
//! YouTube sent them) with a small `{id}.json` beside it (itag, length,
//! duration, loudness). A file is written under a `.part` name and renamed
//! when complete, and the note is written last: a partial download is
//! never served as complete. Least recently used files go first once the
//! folder is over its limit (a hit refreshes the file's modification time).
//! Saving a track for the library ([`crate::Youtube::save_audio`]) reuses a
//! cached file.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use nikgob_core::streaming::{AudioStream, format};
use serde::{Deserialize, Serialize};

use crate::range::Resolved;

/// The default size limit.
pub const DEFAULT_LIMIT: u64 = 500 * 1024 * 1024;

/// What a cached file is (the note next to it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Note {
    itag: u32,
    len: u64,
    format: String,
    duration_ms: Option<u64>,
    gain_db: Option<f32>,
    bitrate: u32,
}

pub struct AudioCache {
    dir: Mutex<Option<PathBuf>>,
    limit: AtomicU64,
}

/// A video id fit for a file name (YouTube ids are `[A-Za-z0-9_-]{11}`).
fn safe(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(id)
}

fn ext(format: &str) -> &'static str {
    if format == format::M4A { "m4a" } else { "webm" }
}

impl AudioCache {
    /// Off until [`Self::set_dir`].
    pub fn new() -> Self {
        Self {
            dir: Mutex::new(None),
            limit: AtomicU64::new(DEFAULT_LIMIT),
        }
    }

    pub fn set_dir(&self, dir: PathBuf) {
        *self.dir.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir);
    }

    /// 0 turns caching off (and empties the folder).
    pub fn set_limit(&self, bytes: u64) {
        self.limit.store(bytes, Ordering::Relaxed);
        self.evict();
    }

    fn dir(&self) -> Option<PathBuf> {
        self.dir.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn enabled(&self) -> Option<PathBuf> {
        self.dir().filter(|_| self.limit.load(Ordering::Relaxed) > 0)
    }

    /// Whether `id` has a complete cached file: its note is written last,
    /// so its presence says so. One look at the folder, nothing opened (and
    /// the file's place in the eviction order stays).
    pub(crate) fn has(&self, id: &str) -> bool {
        let (Some(dir), Some(id)) = (self.enabled(), safe(id)) else { return false };
        dir.join(format!("{id}.json")).is_file()
    }

    /// The complete cached file of `id`, if there is one.
    pub(crate) fn open(&self, id: &str) -> Option<CachedStream> {
        let dir = self.enabled()?;
        let id = safe(id)?;
        let note: Note = serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json"))).ok()?).ok()?;
        let path = dir.join(format!("{id}.{}.{}", note.itag, ext(&note.format)));
        let file = File::options().read(true).write(true).open(&path).ok()?;
        if file.metadata().ok()?.len() != note.len {
            return None;
        }
        // Recently used: evicted last.
        let _ = file.set_modified(SystemTime::now());
        log::debug!("youtube: {id}: cache hit (itag {}, {} bytes)", note.itag, note.len);
        Some(CachedStream {
            file: BufReader::new(file),
            note,
            path,
        })
    }

    /// Keeps a completely downloaded stream (`bytes` reads it from the
    /// start); one shorter or longer than it should be is not kept.
    pub(crate) fn store(&self, id: &str, r: &Resolved, bytes: &mut dyn Read) {
        let Some(dir) = self.enabled() else { return };
        let Some(id) = safe(id) else { return };
        if r.len == 0 || r.len > self.limit.load(Ordering::Relaxed) {
            return;
        }
        let mut write = || -> io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let name = format!("{id}.{}.{}", r.itag, ext(r.format));
            let part = dir.join(format!("{name}.part"));
            let copied = {
                let mut out = io::BufWriter::new(File::create(&part)?);
                // One byte past the length tells a longer one.
                let n = io::copy(&mut bytes.take(r.len + 1), &mut out)?;
                out.flush()?;
                n
            };
            if copied != r.len {
                let _ = std::fs::remove_file(&part);
                return Err(io::Error::other(format!("{copied} bytes, not {}", r.len)));
            }
            std::fs::rename(&part, dir.join(&name))?;
            let note = Note {
                itag: r.itag,
                len: r.len,
                format: r.format.to_owned(),
                duration_ms: r.duration.map(|d| d.as_millis() as u64),
                gain_db: r.gain_db,
                bitrate: r.bitrate,
            };
            std::fs::write(dir.join(format!("{id}.json")), serde_json::to_vec(&note)?)
        };
        match write() {
            Ok(()) => log::debug!("youtube: {id}: cached {} bytes", r.len),
            Err(e) => log::info!("youtube: {id}: cannot cache: {e}"),
        }
        self.evict();
    }

    /// Audio files in the folder, oldest use first: (path, size, used).
    fn files(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let ext = path.extension()?.to_str()?;
                if !matches!(ext, "webm" | "m4a" | "part") {
                    return None;
                }
                let m = e.metadata().ok()?;
                Some((path, m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
            })
            .collect();
        out.sort_by_key(|f| f.2);
        out
    }

    /// Bytes on disk.
    pub fn size(&self) -> u64 {
        self.dir().map_or(0, |d| Self::files(&d).iter().map(|f| f.1).sum())
    }

    /// Drops the least recently used files until the folder fits.
    fn evict(&self) {
        let Some(dir) = self.dir() else { return };
        let limit = self.limit.load(Ordering::Relaxed);
        let files = Self::files(&dir);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let stale_part = |f: &(PathBuf, u64, SystemTime)| {
            f.0.extension().is_some_and(|e| e == "part")
                && f.2.elapsed().unwrap_or_default() > Duration::from_secs(3600)
        };
        for f in &files {
            if total <= limit && !stale_part(f) {
                continue;
            }
            if std::fs::remove_file(&f.0).is_ok() {
                total = total.saturating_sub(f.1);
                if let Some(id) = f.0.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('.').next()) {
                    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
                }
                log::debug!("youtube: cache dropped {}", f.0.display());
            }
        }
    }

    /// Empties the folder.
    pub fn clear(&self) {
        let Some(dir) = self.dir() else { return };
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let _ = std::fs::remove_file(e.path());
        }
        log::info!("youtube: audio cache cleared");
    }
}

/// A cached file as the engine's stream.
pub(crate) struct CachedStream {
    file: BufReader<File>,
    note: Note,
    path: PathBuf,
}

impl CachedStream {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn is_opus(&self) -> bool {
        self.note.format != format::M4A
    }
}

impl Read for CachedStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for CachedStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl AudioStream for CachedStream {
    fn byte_len(&self) -> Option<u64> {
        Some(self.note.len)
    }

    fn format_hint(&self) -> &'static str {
        if self.note.format == format::M4A {
            format::M4A
        } else {
            format::WEBM
        }
    }

    fn gain_db(&self) -> Option<f32> {
        self.note.gain_db
    }

    fn duration(&self) -> Option<Duration> {
        self.note.duration_ms.map(Duration::from_millis)
    }

    fn quality(&self) -> Option<nikgob_core::streaming::StreamQuality> {
        Some(crate::range::quality_of(self.note.itag, self.is_opus(), self.note.bitrate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(len: u64) -> Resolved {
        Resolved {
            url: String::new(),
            len,
            format: format::WEBM,
            duration: Some(Duration::from_secs(3)),
            gain_db: Some(-2.0),
            itag: 251,
            bitrate: 160_000,
            route: None,
        }
    }

    #[test]
    fn complete_files_are_served_and_the_oldest_go_first() {
        let dir = std::env::temp_dir().join(format!("nikgob-yt-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let c = AudioCache::new();
        assert!(c.open("abc").is_none(), "off without a folder");
        c.set_dir(dir.clone());
        c.set_limit(25);
        // A short read is not complete: nothing is kept.
        c.store("abc", &resolved(10), &mut &[1; 9][..]);
        assert!(c.open("abc").is_none());
        c.store("abc", &resolved(10), &mut &[1; 10][..]);
        let mut s = c.open("abc").expect("cached");
        let mut all = Vec::new();
        s.read_to_end(&mut all).unwrap();
        assert_eq!((all.len(), s.byte_len(), s.gain_db()), (10, Some(10), Some(-2.0)));
        assert_eq!(
            s.quality().map(|q| q.words_in(nikgob_core::i18n::Locale::Ru)).as_deref(),
            Some("Opus ~160 кбит/с")
        );
        drop(s);
        std::thread::sleep(Duration::from_millis(30));
        c.store("def", &resolved(10), &mut &[2; 10][..]);
        std::thread::sleep(Duration::from_millis(30));
        // A hit makes "abc" the most recent; "def" goes when "ghi" comes.
        assert!(c.open("abc").is_some());
        std::thread::sleep(Duration::from_millis(30));
        c.store("ghi", &resolved(10), &mut &[3; 10][..]);
        assert!(c.open("def").is_none(), "least recently used went");
        assert!(c.open("abc").is_some() && c.open("ghi").is_some());
        assert_eq!(c.size(), 20);
        assert!(c.open("../x").is_none());
        c.clear();
        assert_eq!(c.size(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
