//! Covers of streaming services on disk (`data/cache/covers`), so a cover
//! seen once is not downloaded again at the next start.
//!
//! One file per url, named by a hash of the url: these urls never change
//! what they point to, so nothing expires. Files are written whole under a
//! temporary name and renamed into place; reading one touches its
//! modification time, and past the size limit the least recently used ones
//! go. A file that does not look like an image (cut short, overwritten) is
//! deleted and counts as a miss.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

/// Bytes the folder may take by default.
pub const DEFAULT_LIMIT: u64 = 300 << 20;

const EXT: &str = "img";
const TMP: &str = "tmp";

pub struct DiskCache {
    dir: PathBuf,
    limit: u64,
    /// Bytes of the cached files (as of the last scan plus what came since).
    size: Mutex<u64>,
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl DiskCache {
    /// Opens (creates) the cache in `dir`: leftovers of interrupted writes
    /// are removed and the size is counted.
    pub fn open(dir: impl Into<PathBuf>, limit: u64) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut size = 0;
        for e in fs::read_dir(&dir)?.flatten() {
            let path = e.path();
            match path.extension().and_then(|x| x.to_str()) {
                Some(EXT) => size += e.metadata().map_or(0, |m| m.len()),
                Some(TMP) => {
                    let _ = fs::remove_file(&path);
                }
                _ => {}
            }
        }
        let cache = Self { dir, limit, size: Mutex::new(size) };
        if size > limit {
            cache.evict();
        }
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Bytes the cached files take.
    pub fn size(&self) -> u64 {
        *self.size.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn path(&self, url: &str) -> PathBuf {
        let hash = blake3::hash(url.as_bytes()).to_hex();
        self.dir.join(format!("{}.{EXT}", &hash[..32]))
    }

    /// The cached image of `url`; marks it as just used.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        let path = self.path(url);
        let bytes = fs::read(&path).ok()?;
        if !looks_like_image(&bytes) {
            log::debug!("cover cache: dropping a broken file for {url}");
            if fs::remove_file(&path).is_ok() {
                self.sub(bytes.len() as u64);
            }
            return None;
        }
        // The access time is the modification time: Windows does not keep
        // access times reliably.
        if let Ok(f) = fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    /// Stores the image of `url` (written whole, then renamed into place).
    pub fn put(&self, url: &str, bytes: &[u8]) -> io::Result<()> {
        if !looks_like_image(bytes) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not an image"));
        }
        let path = self.path(url);
        let tmp = path.with_extension(format!(
            "{}-{}.{TMP}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp, bytes)?;
        let old = fs::metadata(&path).map_or(0, |m| m.len());
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let over = {
            let mut size = self.size.lock().unwrap_or_else(PoisonError::into_inner);
            *size = size.saturating_sub(old) + bytes.len() as u64;
            *size > self.limit
        };
        if over {
            self.evict();
        }
        Ok(())
    }

    fn sub(&self, n: u64) {
        let mut size = self.size.lock().unwrap_or_else(PoisonError::into_inner);
        *size = size.saturating_sub(n);
    }

    /// Deletes the least recently used files until the cache is down to 90 %
    /// of its limit (so it is not scanned again at the very next write).
    fn evict(&self) {
        let Ok(dir) = fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = dir
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some(EXT))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), e.path()))
            })
            .collect();
        files.sort_unstable();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let target = self.limit / 10 * 9;
        let mut removed = 0usize;
        for (_, len, path) in files {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                removed += 1;
            }
        }
        log::debug!("cover cache: {removed} old covers removed, {} MB left", total >> 20);
        *self.size.lock().unwrap_or_else(PoisonError::into_inner) = total;
    }
}

/// A JPEG or PNG whose header parses (what the image loader can decode).
fn looks_like_image(bytes: &[u8]) -> bool {
    let magic = bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(b"\x89PNG\r\n\x1a\n");
    magic
        && image::ImageReader::new(io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .is_some_and(|(w, h)| w > 0 && h > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(side: u32, shade: u8) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(side, side, image::Rgba([shade, 0, 0, 255]));
        let mut out = io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_stored_cover_comes_back_and_survives_a_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::open(dir.path(), DEFAULT_LIMIT).unwrap();
        let img = png(8, 1);
        assert_eq!(c.get("https://x/a"), None, "miss before it is stored");
        c.put("https://x/a", &img).unwrap();
        assert_eq!(c.get("https://x/a").as_deref(), Some(&img[..]));
        assert_eq!(c.get("https://x/b"), None);
        assert_eq!(c.size(), img.len() as u64);
        // Storing it again does not count it twice.
        c.put("https://x/a", &img).unwrap();
        assert_eq!(c.size(), img.len() as u64);
        drop(c);
        let c = DiskCache::open(dir.path(), DEFAULT_LIMIT).unwrap();
        assert_eq!(c.size(), img.len() as u64);
        assert_eq!(c.get("https://x/a").as_deref(), Some(&img[..]));
    }

    #[test]
    fn the_least_recently_used_covers_go_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let one = png(64, 0).len() as u64;
        // Room for three covers.
        let c = DiskCache::open(dir.path(), one * 3 + one / 2).unwrap();
        let set = |path: &Path, secs: u64| {
            let f = fs::File::options().write(true).open(path).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        };
        for (i, url) in ["a", "b", "c"].iter().enumerate() {
            c.put(url, &png(64, i as u8)).unwrap();
            set(&c.path(url), 1000 + i as u64);
        }
        // "a" is read, so "b" is now the least recently used.
        assert!(c.get("a").is_some());
        c.put("d", &png(64, 9)).unwrap();
        assert!(c.get("b").is_none(), "the oldest went");
        for url in ["a", "c", "d"] {
            assert!(c.get(url).is_some(), "{url} stays");
        }
        assert!(c.size() <= one * 3);
    }

    #[test]
    fn a_broken_file_is_a_miss_and_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let c = DiskCache::open(dir.path(), DEFAULT_LIMIT).unwrap();
        let img = png(16, 3);
        c.put("u", &img).unwrap();
        // Cut short and garbage.
        fs::write(c.path("u"), &img[..10]).unwrap();
        assert_eq!(c.get("u"), None);
        assert!(!c.path("u").exists());
        fs::write(c.path("v"), b"<html>not found</html>").unwrap();
        assert_eq!(c.get("v"), None);
        assert!(!c.path("v").exists());
        // An error page is not stored at all.
        assert!(c.put("w", b"<html>").is_err());
        assert!(!c.path("w").exists());
    }

    #[test]
    fn leftovers_of_interrupted_writes_are_cleaned_at_open() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("abc.1-2.tmp"), b"half").unwrap();
        let c = DiskCache::open(dir.path(), DEFAULT_LIMIT).unwrap();
        assert!(!dir.path().join("abc.1-2.tmp").exists());
        assert_eq!(c.size(), 0);
    }
}
