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
//!
//! The certificate checker's records ([`ONCE`]) are written once per run
//! each, as warnings: it logs an ERROR for every certificate it refuses,
//! without the host, and a torrent tracker with a broken certificate (such
//! as opentracker.xyz, whose certificate is for another name) is asked by
//! librqbit again every few minutes while its torrent seeds. librqbit
//! itself says which tracker failed, at `debug`.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

pub const FILE: &str = "nikgob.log";
/// Size at which the log starts over (the old one becomes `nikgob.log.1`).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
// RustyPipe's own records are its tracing spans at level ERROR (noise that
// reads like failures); the youtube crate logs every request and failure.
const FILTER: &str = "info,nikgob_spotify=debug,nikgob_youtube=debug,nikgob_runtime::streams=debug,nikgob::streams=debug,nikgob::audio=debug,rustypipe=off";
/// Targets whose records repeat without telling more: each message once.
const ONCE: &[&str] = &["rustls_platform_verifier"];

/// Where the log lives: next to the library (the demo's dir in demo mode,
/// the developer profile's in the profile).
pub fn dir() -> PathBuf {
    if crate::profile::requested() {
        return crate::profile::dir();
    }
    if let Some(dir) = std::env::var_os("NIKGOB_DATA") {
        return PathBuf::from(dir);
    }
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
    let logger = Quiet::new(builder.build());
    let max = logger.inner.filter();
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(max);
    }
    log::info!(
        "music.nikgob {} started, log at {}",
        env!("CARGO_PKG_VERSION"),
        dir().join(FILE).display()
    );
}

/// Warnings and errors kept for a settings sweep while it asks
/// ([`capture_start`]): each shot says what went wrong during it.
static CAPTURED: Mutex<Option<Vec<String>>> = Mutex::new(None);
/// At most this many are kept between two takes.
const CAPTURE_MAX: usize = 200;

/// Starts keeping warnings and errors.
pub fn capture_start() {
    *CAPTURED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
}

/// The warnings and errors since the last take (and goes on keeping them).
pub fn capture_take() -> Vec<String> {
    CAPTURED.lock().unwrap_or_else(PoisonError::into_inner).as_mut().map(std::mem::take).unwrap_or_default()
}

fn capture(record: &log::Record<'_>) {
    if record.level() > log::Level::Warn {
        return;
    }
    let mut kept = CAPTURED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(list) = kept.as_mut()
        && list.len() < CAPTURE_MAX
    {
        list.push(format!("{} {}: {}", record.level(), record.target(), record.args()));
    }
}

/// The logger, writing the records of [`ONCE`] once per message.
struct Quiet {
    inner: env_logger::Logger,
    seen: Mutex<HashSet<String>>,
}

impl Quiet {
    fn new(inner: env_logger::Logger) -> Self {
        Self { inner, seen: Mutex::new(HashSet::new()) }
    }
}

impl log::Log for Quiet {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.inner.matches(record) {
            capture(record);
        }
        if !ONCE.iter().any(|t| record.target().starts_with(t)) {
            self.inner.log(record);
            return;
        }
        if !self.inner.matches(record) {
            return;
        }
        let text = record.args().to_string();
        if !self.seen.lock().unwrap_or_else(PoisonError::into_inner).insert(text.clone()) {
            return;
        }
        self.inner.log(
            &log::Record::builder()
                .args(format_args!("{text} (once; repeats are not logged)"))
                .level(record.level().max(log::Level::Warn))
                .target(record.target())
                .module_path(record.module_path())
                .file(record.file())
                .line(record.line())
                .build(),
        );
    }

    fn flush(&self) {
        self.inner.flush();
    }
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

    /// A writer the test reads back.
    #[derive(Clone, Default)]
    struct Shared(std::sync::Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_refused_certificate_is_logged_once_as_a_warning() {
        use log::Log;
        let out = Shared::default();
        let logger = Quiet::new(
            env_logger::Builder::new()
                .parse_filters(FILTER)
                .target(env_logger::Target::Pipe(Box::new(out.clone())))
                .build(),
        );
        let say = |level: log::Level, target: &str, msg: &str| {
            logger.log(&log::Record::builder().args(format_args!("{msg}")).level(level).target(target).build());
        };
        let verifier = "rustls_platform_verifier::verification::windows";
        for _ in 0..3 {
            say(log::Level::Error, verifier, "failed to verify TLS certificate: invalid peer certificate: NotValidForName");
        }
        say(log::Level::Error, verifier, "failed to verify TLS certificate: invalid peer certificate: UnknownIssuer");
        for _ in 0..2 {
            say(log::Level::Warn, "nikgob_runtime::search", "prowlarr check: indexer is unreachable");
        }
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].contains("WARN") && lines[0].contains("NotValidForName (once; repeats are not logged)"), "{text}");
        assert!(lines[1].contains("WARN") && lines[1].contains("UnknownIssuer"), "{text}");
        assert!(!text.contains("ERROR"), "{text}");
        assert!(lines[2..].iter().all(|l| l.ends_with("indexer is unreachable")), "the app's own records stay as they are");
    }
}
