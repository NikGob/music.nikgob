//! The log goes to `nikgob.log` in the app's data dir (the demo's own dir
//! in demo mode): a windowed exe has no stderr, and a log file is what the
//! user can send when something is slow or broken.
//!
//! Default level `info`; the streaming code (Spotify, YouTube Music, the
//! runtime's streaming part, the app's stream bridge) logs at `debug`:
//! every request with its status and time, search stages, stream opening,
//! cache hits. `RUST_LOG` overrides the filter. At [`MAX_BYTES`] the file is
//! renamed to `nikgob.log.1` (one old file is kept) and a new one starts.
//! Secrets never reach the log: tokens, cookies and keys are not logged.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const FILE: &str = "nikgob.log";
/// Size at which the log starts over (the old one becomes `nikgob.log.1`).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
// RustyPipe's own records are its tracing spans at level ERROR (noise that
// reads like failures); the youtube crate logs every request and failure.
const FILTER: &str = "info,nikgob_spotify=debug,nikgob_youtube=debug,nikgob_runtime::streams=debug,nikgob::streams=debug,nikgob::audio=debug,rustypipe=off";

/// Where the log lives: next to the library (the demo's dir in demo mode).
pub fn dir() -> PathBuf {
    if std::env::var_os("NIKGOB_DEMO").is_some() {
        return std::env::temp_dir().join("nikgob-demo");
    }
    directories::ProjectDirs::from("dev", "nikgob", "music.nikgob")
        .map_or_else(|| PathBuf::from("."), |d| d.data_dir().to_owned())
}

/// Installs the logger. Without a writable file the log goes to stderr.
pub fn init() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(FILTER));
    builder.format_timestamp_millis();
    if let Some(w) = Rotating::open(&dir()) {
        builder.target(env_logger::Target::Pipe(Box::new(w)));
    }
    builder.init();
    log::info!(
        "music.nikgob {} started, log at {}",
        env!("CARGO_PKG_VERSION"),
        dir().join(FILE).display()
    );
}

/// A log file that starts over at [`MAX_BYTES`], keeping one old file.
struct Rotating {
    path: PathBuf,
    file: File,
    written: u64,
}

impl Rotating {
    fn open(dir: &Path) -> Option<Self> {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join(FILE);
        let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;
        let written = file.metadata().map_or(0, |m| m.len());
        let mut me = Self { path, file, written };
        if me.written >= MAX_BYTES {
            me.rotate();
        }
        Some(me)
    }

    fn rotate(&mut self) {
        let old = self.path.with_file_name(format!("{FILE}.1"));
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&self.path, &old);
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            self.file = f;
            self.written = 0;
        }
    }
}

impl Write for Rotating {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written >= MAX_BYTES {
            self.rotate();
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_over_at_the_limit_and_keeps_one_old_file() {
        let dir = std::env::temp_dir().join(format!("nikgob-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = Rotating::open(&dir).unwrap();
        let line = vec![b'x'; 1024 * 1024];
        for _ in 0..6 {
            w.write_all(&line).unwrap();
        }
        w.flush().unwrap();
        let old = std::fs::metadata(dir.join(format!("{FILE}.1"))).unwrap().len();
        let new = std::fs::metadata(dir.join(FILE)).unwrap().len();
        assert_eq!(old, MAX_BYTES);
        assert_eq!(new, 1024 * 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
