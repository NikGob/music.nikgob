//! The log goes to `nikgob.log` in the app's data dir (the demo's own dir
//! in demo mode): a windowed exe has no stderr, and a log file is what the
//! user can send when something is slow or broken.
//!
//! Default level `info` for the player's own crates and `warn` for every
//! other crate (symphonia's stream notes, librqbit's and librespot's
//! progress); lofty only at `error` (its warnings are about other people's
//! tags). The streaming code (Spotify, YouTube Music, the runtime's
//! streaming part, the app's stream bridge) logs at `debug`: every request
//! with its status and time, search stages, stream opening, cache hits. `RUST_LOG` overrides the filter. At [`MAX_BYTES`] the file is
//! renamed to `nikgob.log.1` (one old file is kept) and a new one starts.
//! Secrets never reach the log: tokens, cookies and keys are not logged.
//!
//! The log is made to be sent and posted, so nothing in it says whose it
//! is. Every record goes through [`scrub`]: the home dir becomes `~`, the
//! user's folder of any other path (`C:\Users\~\…`, `/home/~/…`) too, IP
//! addresses become `<ip>`, e-mail addresses `<email>`, and a long hex id
//! is cut to its first characters. Records of other crates that name the
//! account or the country are reworded or dropped ([`REWORDED`]), and the
//! player's own records never carry an account's name or id. `crash.log`
//! goes through the same [`scrub`], and the files an earlier version wrote
//! are scrubbed at the start.
//!
//! The certificate checker's records ([`ONCE`]) are written once per run
//! each, as warnings: it logs an ERROR for every certificate it refuses,
//! without the host, and a torrent tracker with a broken certificate (such
//! as opentracker.xyz, whose certificate is for another name) is asked by
//! librqbit again every few minutes while its torrent seeds. librqbit
//! itself says which tracker failed, at `debug`.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

pub const FILE: &str = "nikgob.log";
/// The panics' file, next to the log.
pub const CRASH: &str = "crash.log";
/// Size at which the log starts over (the old one becomes `nikgob.log.1`).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
// Other crates' INFO was most of the log (symphonia alone 13 %): only their
// warnings and errors are kept. `nikgob` covers every crate of the player
// (a directive is a target prefix). RustyPipe's own records are its tracing
// spans at level ERROR (noise that reads like failures); the youtube crate
// logs every request and failure.
const FILTER: &str = "warn,nikgob=info,nikgob_spotify=debug,nikgob_youtube=debug,nikgob_runtime::streams=debug,nikgob::streams=debug,nikgob::audio=debug,rustypipe=off,lofty=error";
/// Targets whose records repeat without telling more: each message once.
const ONCE: &[&str] = &["rustls_platform_verifier"];
/// Records that say whose account it is: target, how the text starts, and
/// the text logged instead (`None`: the record is dropped).
const REWORDED: &[(&str, &str, Option<&str>)] = &[
    ("librespot_core::session", "Authenticated as", Some("Authenticated")),
    ("librespot_core::session", "Country:", None),
    // What earlier versions wrote (the files of before are scrubbed too).
    ("nikgob_soulseek::client", "soulseek: logged in as", Some("soulseek: logged in")),
    ("nikgob_soundcloud", "soundcloud: signed in as", Some("soundcloud: signed in")),
    ("nikgob_soundcloud", "soundcloud: signed in again as", Some("soundcloud: signed in again")),
];
/// A run of hex digits this long is an id of the machine or the account.
const HEX_RUN: usize = 32;
/// How much of such an id stays, to tell two apart.
const HEX_KEPT: usize = 6;

/// Where the log lives: next to the library (the demo's dir in demo mode,
/// the developer profile's in the profile).
pub fn dir() -> PathBuf {
    #[cfg(feature = "dev")]
    {
        if crate::profile::requested() {
            return crate::profile::dir();
        }
        if let Some(dir) = std::env::var_os("NIKGOB_DATA") {
            return PathBuf::from(dir);
        }
        if std::env::var_os("NIKGOB_DEMO").is_some() {
            return std::env::temp_dir().join("nikgob-demo");
        }
    }
    directories::ProjectDirs::from("dev", "nikgob", "music.nikgob")
        .map_or_else(|| PathBuf::from("."), |d| d.data_dir().to_owned())
}

/// Installs the logger, after scrubbing the files already there. Without a
/// writable file the log goes to stderr.
pub fn init() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(FILTER));
    builder.format_timestamp_millis();
    scrub_files(&dir(), homes());
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
#[cfg(feature = "dev")]
pub fn capture_start() {
    *CAPTURED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
}

/// The warnings and errors since the last take (and goes on keeping them).
#[cfg(feature = "dev")]
pub fn capture_take() -> Vec<String> {
    CAPTURED.lock().unwrap_or_else(PoisonError::into_inner).as_mut().map(std::mem::take).unwrap_or_default()
}

fn capture(level: log::Level, target: &str, text: &str) {
    if level > log::Level::Warn {
        return;
    }
    let mut kept = CAPTURED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(list) = kept.as_mut()
        && list.len() < CAPTURE_MAX
    {
        list.push(format!("{level} {target}: {text}"));
    }
}

/// `text` without what says whose machine and network it is: the home dir
/// and any user's folder become `~`, IP addresses `<ip>`, e-mail addresses
/// `<email>`, and a hex id of [`HEX_RUN`] digits or more keeps its first
/// [`HEX_KEPT`]. Borrowed when nothing was found.
pub fn scrub(text: &str) -> Cow<'_, str> {
    scrub_with(text, homes())
}

/// The spellings of this user's home dir, taken once.
fn homes() -> &'static [String] {
    static HOMES: OnceLock<Vec<String>> = OnceLock::new();
    HOMES.get_or_init(|| {
        directories::BaseDirs::new().map(|d| spellings(&d.home_dir().to_string_lossy())).unwrap_or_default()
    })
}

/// A home dir as paths are printed: as it is, with every `\` doubled (a
/// path printed with `{:?}`), and with `/` for `\`. None for a dir that is
/// a root.
fn spellings(home: &str) -> Vec<String> {
    let home = home.trim_end_matches(['\\', '/']);
    if !home.contains(['\\', '/']) {
        return Vec::new();
    }
    let mut all = vec![home.to_owned()];
    if home.contains('\\') {
        all.push(home.replace('\\', "\\\\"));
        all.push(home.replace('\\', "/"));
    }
    all
}

/// [`scrub`] for the home dir of `homes` ([`spellings`]).
fn scrub_with<'a>(text: &'a str, homes: &[String]) -> Cow<'a, str> {
    // Most records have nothing to find: one look says which passes run.
    let (mut sep, mut dot, mut colon, mut at, mut long, mut run) = (false, false, false, false, false, 0);
    for c in text.bytes() {
        match c {
            b'\\' | b'/' => sep = true,
            b'.' => dot = true,
            b':' => colon = true,
            b'@' => at = true,
            _ => {}
        }
        run = if is_word(c) { run + 1 } else { 0 };
        long |= run >= HEX_RUN;
    }
    let mut out = Cow::Borrowed(text);
    if sep {
        out = rewrite(out, |t, from| next_home(t, from, homes));
        out = rewrite(out, next_user_dir);
    }
    if dot {
        out = rewrite(out, next_ipv4);
    }
    if colon {
        out = rewrite(out, next_ipv6);
    }
    if at && dot {
        out = rewrite(out, next_email);
    }
    if long {
        out = rewrite(out, next_hex);
    }
    out
}

/// A place in a text (start, end) and what goes there instead.
type Hit = (usize, usize, Cow<'static, str>);

/// `text` with every hit of `next` (asked from the end of the last one)
/// replaced; as it came when there is none.
fn rewrite<'a>(text: Cow<'a, str>, next: impl Fn(&str, usize) -> Option<Hit>) -> Cow<'a, str> {
    let mut out: Option<String> = None;
    let mut copied = 0;
    while let Some((start, end, with)) = next(&*text, copied) {
        let o = out.get_or_insert_with(|| String::with_capacity(text.len()));
        o.push_str(&text[copied..start]);
        o.push_str(&with);
        copied = end;
    }
    match out {
        Some(mut o) => {
            o.push_str(&text[copied..]);
            Cow::Owned(o)
        }
        None => text,
    }
}

/// A letter, a digit or `_`: what a name or an id goes on with.
fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn word_before(b: &[u8], at: usize) -> bool {
    at > 0 && is_word(b[at - 1])
}

fn word_at(b: &[u8], at: usize) -> bool {
    b.get(at).is_some_and(|c| is_word(*c))
}

/// The home dir in one of its spellings, where the name ends there.
fn next_home(text: &str, from: usize, homes: &[String]) -> Option<Hit> {
    homes
        .iter()
        .filter_map(|home| {
            let mut at = from;
            loop {
                let start = at + text[at..].find(home.as_str())?;
                let end = start + home.len();
                if !text[end..].starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '-') {
                    return Some((start, end, Cow::Borrowed("~")));
                }
                at = end;
            }
        })
        .min_by_key(|hit| hit.0)
}

/// The folder's name after `\Users\` (in any case, `\\` and `/` as well)
/// or `/home/`: any user's, as the paths of the machine a build was made
/// on are in a backtrace too.
fn next_user_dir(text: &str, from: usize) -> Option<Hit> {
    let b = text.as_bytes();
    let mut i = from;
    while i < b.len() {
        if !matches!(b[i], b'\\' | b'/') {
            i += 1;
            continue;
        }
        let rest = &b[i + 1..];
        let after = if rest.len() >= 5 && rest[..5].eq_ignore_ascii_case(b"users") {
            i + 6
        } else if rest.starts_with(b"home") {
            i + 5
        } else {
            i += 1;
            continue;
        };
        let start = match &b[after..] {
            [b'\\', b'\\', ..] => after + 2,
            [b'\\' | b'/', ..] => after + 1,
            _ => {
                i += 1;
                continue;
            }
        };
        let mut end = b[start..].iter().position(|c| ends_name(*c)).map_or(b.len(), |n| start + n);
        // «John Smith»: a name of a few words, when the path goes on right
        // after it.
        if b.get(end) == Some(&b' ') {
            let rest = &b[end..(end + SPACED_NAME).min(b.len())];
            if let Some(n) = rest.iter().position(|c| *c != b' ' && ends_name(*c))
                && matches!(rest[n], b'\\' | b'/')
                && rest[n - 1] != b' '
                && rest[..n].iter().filter(|c| **c == b' ').count() <= 2
            {
                end += n;
            }
        }
        if end > start && &b[start..end] != b"~" {
            return Some((start, end, Cow::Borrowed("~")));
        }
        i = after;
    }
    None
}

/// A user folder's name with spaces is looked for this far.
const SPACED_NAME: usize = 40;

/// What a folder's name in a logged path stops at.
fn ends_name(c: u8) -> bool {
    matches!(c, b'\\' | b'/' | b'"' | b'\'' | b':' | b'<' | b'>' | b'|' | b'*' | b'?')
        || c.is_ascii_whitespace()
        || c.is_ascii_control()
}

/// Four numbers up to 255 with dots between, alone: a version of three
/// parts, or five, is not an address.
fn next_ipv4(text: &str, from: usize) -> Option<Hit> {
    let b = text.as_bytes();
    let part = |c: u8| c.is_ascii_digit() || c == b'.';
    let mut i = from;
    while i < b.len() {
        if !part(b[i]) {
            i += 1;
            continue;
        }
        let run = b[i..].iter().position(|c| !part(*c)).map_or(b.len(), |n| i + n);
        // A dot that ends the sentence is not the address's.
        let mut end = run;
        while end > i && b[end - 1] == b'.' {
            end -= 1;
        }
        if is_ipv4(&b[i..end]) && !word_before(b, i) && !word_at(b, end) {
            return Some((i, end, Cow::Borrowed("<ip>")));
        }
        i = run;
    }
    None
}

fn is_ipv4(s: &[u8]) -> bool {
    let mut groups = 0;
    for g in s.split(|c| *c == b'.') {
        groups += 1;
        if g.is_empty() || g.len() > 3 || g.iter().fold(0u32, |n, d| n * 10 + u32::from(*d - b'0')) > 255 {
            return false;
        }
    }
    groups == 4
}

/// A run of hex digits and colons that reads as an IPv6 address
/// ([`is_ipv6`]), alone. A colon of the sentence around it is not its own.
fn next_ipv6(text: &str, from: usize) -> Option<Hit> {
    let b = text.as_bytes();
    let part = |c: u8| c.is_ascii_hexdigit() || c == b':';
    let mut i = from;
    while i < b.len() {
        if !part(b[i]) {
            i += 1;
            continue;
        }
        let run = b[i..].iter().position(|c| !part(*c)).map_or(b.len(), |n| i + n);
        let (mut start, mut end) = (i, run);
        if end - start >= 2 && b[start] == b':' && b[start + 1] != b':' {
            start += 1;
        }
        if end - start >= 2 && b[end - 1] == b':' && b[end - 2] != b':' {
            end -= 1;
        }
        if is_ipv6(&b[start..end]) && !word_before(b, start) && !word_at(b, end) {
            return Some((start, end, Cow::Borrowed("<ip>")));
        }
        i = run;
    }
    None
}

/// Groups of up to four hex digits, one of them with a digit, and either
/// four colons with three groups or `::` with two: a clock time, `[::]`
/// and a Rust path are none of that.
fn is_ipv6(s: &[u8]) -> bool {
    let (mut groups, mut digit) = (0, false);
    for g in s.split(|c| *c == b':') {
        if g.len() > 4 {
            return false;
        }
        if !g.is_empty() {
            groups += 1;
            digit |= g.iter().any(u8::is_ascii_digit);
        }
    }
    let colons = s.iter().filter(|c| **c == b':').count();
    let double = s.windows(2).any(|w| w == b"::");
    digit && ((colons >= 4 && groups >= 3) || (double && groups >= 2))
}

/// `name@host.tld`.
fn next_email(text: &str, from: usize) -> Option<Hit> {
    let b = text.as_bytes();
    let name = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'%' | b'+' | b'-');
    let host = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-');
    let mut at = from;
    while let Some(n) = b[at..].iter().position(|c| *c == b'@') {
        let i = at + n;
        let mut start = i;
        while start > from && name(b[start - 1]) {
            start -= 1;
        }
        let mut end = i + 1;
        while end < b.len() && host(b[end]) {
            end += 1;
        }
        while end > i + 1 && !b[end - 1].is_ascii_alphanumeric() {
            end -= 1;
        }
        let domain = &b[i + 1..end];
        let tld = domain.rsplit(|c| *c == b'.').next().unwrap_or_default();
        if start < i && tld.len() >= 2 && tld.len() < domain.len() && tld.iter().all(u8::is_ascii_alphabetic) {
            return Some((start, end, Cow::Borrowed("<email>")));
        }
        at = i + 1;
    }
    None
}

/// [`HEX_RUN`] hex digits or more, alone: cut to the first [`HEX_KEPT`].
fn next_hex(text: &str, from: usize) -> Option<Hit> {
    let b = text.as_bytes();
    let mut i = from;
    while i < b.len() {
        if !is_word(b[i]) {
            i += 1;
            continue;
        }
        let end = b[i..].iter().position(|c| !is_word(*c)).map_or(b.len(), |n| i + n);
        if end - i >= HEX_RUN && b[i..end].iter().all(u8::is_ascii_hexdigit) {
            return Some((i, end, Cow::Owned(format!("{}…", &text[i..i + HEX_KEPT]))));
        }
        i = end;
    }
    None
}

/// A record's text as it is logged ([`REWORDED`]); `None` drops the record.
fn reworded<'a>(target: &str, text: Cow<'a, str>) -> Option<Cow<'a, str>> {
    match REWORDED.iter().find(|(t, start, _)| target == *t && text.starts_with(*start)) {
        Some((_, _, to)) => to.map(Cow::Borrowed),
        None => Some(text),
    }
}

/// A line of a log file as [`reworded`] leaves its record.
fn reworded_line(line: &str) -> Option<Cow<'_, str>> {
    for (target, start, to) in REWORDED {
        let Some(at) = line.find(target) else {
            continue;
        };
        let Some(rest) = line[at + target.len()..].strip_prefix("] ") else {
            continue;
        };
        if rest.starts_with(start) {
            let head = &line[..line.len() - rest.len()];
            let end = if line.ends_with('\n') { "\n" } else { "" };
            return to.map(|to| Cow::Owned(format!("{head}{to}{end}")));
        }
    }
    Some(Cow::Borrowed(line))
}

/// Scrubs the log files and `crash.log` already in `dir`: an earlier
/// version wrote them as they came. A file that does not read or write is
/// left as it is.
fn scrub_files(dir: &Path, homes: &[String]) {
    for name in [FILE.to_owned(), format!("{FILE}.1"), CRASH.to_owned()] {
        let _ = scrub_file(&dir.join(name), homes);
    }
}

/// Scrubs one file in place (written beside it, then renamed over it);
/// true when it changed.
fn scrub_file(path: &Path, homes: &[String]) -> std::io::Result<bool> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let Some(line) = reworded_line(line) else {
            changed = true;
            continue;
        };
        let clean = scrub_with(&line, homes);
        changed |= matches!(&line, Cow::Owned(_)) || matches!(&clean, Cow::Owned(_));
        out.push_str(&clean);
    }
    if !changed {
        return Ok(false);
    }
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    let temp = path.with_file_name(name);
    let done = std::fs::write(&temp, out).and_then(|()| std::fs::rename(&temp, path));
    if done.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    done.map(|()| true)
}

/// The logger: every record scrubbed ([`scrub`], [`REWORDED`]), the
/// records of [`ONCE`] written once per message.
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
        if !self.inner.matches(record) {
            return;
        }
        let text = match record.args().as_str() {
            Some(s) => Cow::Borrowed(s),
            None => Cow::Owned(record.args().to_string()),
        };
        let Some(text) = reworded(record.target(), text) else {
            return;
        };
        let text = scrub(&text);
        capture(record.level(), record.target(), &text);
        let once = ONCE.iter().any(|t| record.target().starts_with(t));
        if once && !self.seen.lock().unwrap_or_else(PoisonError::into_inner).insert(text.to_string()) {
            return;
        }
        let (level, note) = if once {
            (record.level().max(log::Level::Warn), " (once; repeats are not logged)")
        } else {
            (record.level(), "")
        };
        self.inner.log(
            &log::Record::builder()
                .args(format_args!("{text}{note}"))
                .level(level)
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

    #[test]
    fn the_account_and_the_ids_of_other_crates_stay_out_of_the_log() {
        use log::Log;
        let out = Shared::default();
        // Their INFO is off by default; `RUST_LOG` can turn it on.
        let logger = Quiet::new(
            env_logger::Builder::new()
                .parse_filters(&format!("{FILTER},librespot_core=info,librqbit_dht=info"))
                .target(env_logger::Target::Pipe(Box::new(out.clone())))
                .build(),
        );
        let say = |target: &str, msg: &str| {
            logger.log(&log::Record::builder().args(format_args!("{msg}")).level(log::Level::Info).target(target).build());
        };
        say("librespot_core::session", "Authenticated as 'iri5cuexample'");
        say("librespot_core::session", "Country: \"IL\"");
        say("librespot_core::session", "Session closed");
        say("librqbit_dht::dht", &format!("starting up DHT with peer id 2d7251{}", "0a".repeat(17)));
        say("nikgob_runtime::streams", "Authenticated as 'someone' is another target's text");
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].ends_with("librespot_core::session] Authenticated"), "{text}");
        assert!(lines[1].ends_with("Session closed"), "{text}");
        assert!(lines[2].ends_with("peer id 2d7251…"), "{text}");
        assert!(lines[3].ends_with("another target's text"), "{text}");
        assert!(!text.contains("iri5cu") && !text.contains("Country"), "{text}");
    }

    #[test]
    fn other_crates_log_warnings_only_and_ours_keep_their_level() {
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
        say(log::Level::Info, "symphonia_bundle_mp3::demuxer", "using xing header");
        say(log::Level::Info, "librqbit::session", "added torrent");
        say(log::Level::Warn, "lofty::id3::v2::read", "unknown frame");
        say(log::Level::Warn, "librqbit::session", "a torrent warning");
        say(log::Level::Info, "nikgob_runtime::lyrics", "our info");
        say(log::Level::Info, "nikgob::player", "the app's info");
        say(log::Level::Debug, "nikgob_youtube", "youtube debug");
        say(log::Level::Debug, "nikgob_library", "library debug");
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].ends_with("a torrent warning"), "{text}");
        assert!(lines[1].ends_with("our info"), "{text}");
        assert!(lines[2].ends_with("the app's info"), "{text}");
        assert!(lines[3].ends_with("youtube debug"), "{text}");
    }

    fn home() -> Vec<String> {
        spellings(r"C:\Users\Rafael")
    }

    fn clean(text: &str) -> String {
        scrub_with(text, &home()).into_owned()
    }

    #[test]
    fn the_home_dir_becomes_a_tilde_in_every_spelling() {
        assert_eq!(
            clean(r"log at C:\Users\Rafael\AppData\Roaming\nikgob\nikgob.log"),
            r"log at ~\AppData\Roaming\nikgob\nikgob.log"
        );
        assert_eq!(
            clean(r#"opened filename="C:\\Users\\Rafael\\AppData\\Roaming\\nikgob\\session.json""#),
            r#"opened filename="~\\AppData\\Roaming\\nikgob\\session.json""#
        );
        assert_eq!(clean("C:/Users/Rafael/Music/a.flac"), "~/Music/a.flac");
        assert_eq!(clean(r"cannot open C:\Users\Rafael: denied"), "cannot open ~: denied");
        // Another user's folder that starts the same is not the home.
        assert_eq!(clean(r"C:\Users\Rafaela\x"), r"C:\Users\~\x");
        assert_eq!(scrub_with("/home/nik/.config/nikgob", &spellings("/home/nik")), "~/.config/nikgob");
        assert!(spellings("/").is_empty() && spellings(r"C:\").is_empty());
    }

    #[test]
    fn any_users_folder_loses_its_name() {
        assert_eq!(
            clean(r"   at C:\Users\PC\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tokio-1.40.0\src\runtime\task\mod.rs:12"),
            r"   at C:\Users\~\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tokio-1.40.0\src\runtime\task\mod.rs:12"
        );
        assert_eq!(
            clean(r#"librqbit: filename="C:\\Users\\PC\\AppData\\Roaming\\nikgob\\session.json""#),
            r#"librqbit: filename="C:\\Users\\~\\AppData\\Roaming\\nikgob\\session.json""#
        );
        assert_eq!(clean(r"c:\users\someone\x"), r"c:\users\~\x");
        assert_eq!(clean("/home/builder/.cargo/registry/src"), "/home/~/.cargo/registry/src");
        assert_eq!(clean("/Users/anna/Library/Caches"), "/Users/~/Library/Caches");
        assert_eq!(clean(r"C:\Users\~\AppData"), r"C:\Users\~\AppData");
        assert_eq!(clean(r"log at C:\Users\John Smith\AppData\Roaming\nikgob"), r"log at C:\Users\~\AppData\Roaming\nikgob");
        assert_eq!(clean("/home/bob and /home/al/x"), "/home/~ and /home/~/x");
        assert_eq!(clean(r"C:\Users\PC took 5 ms at D:\x"), r"C:\Users\~ took 5 ms at D:\x");
        for same in ["three users/guests were here", "the users of home pages", r"D:\Music\Users.flac", "/home"] {
            assert_eq!(clean(same), same);
        }
    }

    #[test]
    fn ip_addresses_are_hidden_and_versions_stay() {
        assert_eq!(clean("peer 203.0.113.7:51413 refused"), "peer <ip>:51413 refused");
        assert_eq!(clean("listening on 0.0.0.0:55122"), "listening on <ip>:55122");
        assert_eq!(clean("connected to 198.51.100.20."), "connected to <ip>.");
        for same in ["ONNX Runtime 1.24.4 loaded", "took 0.25 s", "build 1.2.3.4.5", "tag v1.2.3.4", "size 1000.2.3.4"] {
            assert_eq!(clean(same), same);
        }
    }

    #[test]
    fn ipv6_addresses_are_hidden_and_times_and_paths_stay() {
        assert_eq!(clean("peer [2a01:4f8:c17:6a3::2]:6881"), "peer [<ip>]:6881");
        assert_eq!(clean("from 2001:db8::1 now"), "from <ip> now");
        assert_eq!(clean("peer 2001:db8:0:0:0:0:0:1: refused"), "peer <ip>: refused");
        for same in [
            "bound to [::]:55122",
            "at 14:40:15 it stopped",
            "[2026-11-10T14:40:15.123Z INFO  nikgob_runtime::streams::resolve] a::b and face::cafe",
            "librespot_core::session] added: 3 of 12",
        ] {
            assert_eq!(clean(same), same);
        }
    }

    #[test]
    fn email_addresses_are_hidden() {
        assert_eq!(clean("account some.one+tag@mail.example.com is gone"), "account <email> is gone");
        for same in ["tokio@1.40.0", "someone@localhost", "48 kHz @ 24 bit", "see example.com"] {
            assert_eq!(clean(same), same);
        }
    }

    #[test]
    fn a_long_hex_id_is_cut_and_a_track_id_stays() {
        let id = format!("2d7251{}", "0a".repeat(17));
        assert_eq!(clean(&format!("starting up DHT with peer id {id}")), "starting up DHT with peer id 2d7251…");
        assert_eq!(clean(&"AB".repeat(16)), "ABABAB…");
        let short = "a".repeat(31);
        let named = format!("{}g", "ab".repeat(16));
        for same in ["track 3f2b8c1e-7a4d-4e9b-9c1a-5d6e7f8a9b0c", short.as_str(), named.as_str()] {
            assert_eq!(clean(same), same);
        }
    }

    #[test]
    fn a_plain_record_is_not_copied() {
        for same in ["prowlarr check: indexer is unreachable", "analysis: 12 tracks done in 3.5 s", ""] {
            assert!(matches!(scrub_with(same, &home()), Cow::Borrowed(_)), "{same}");
        }
        assert!(matches!(scrub_with("peer 203.0.113.7", &home()), Cow::Owned(_)));
    }

    #[test]
    fn the_files_of_an_earlier_version_are_scrubbed_in_place() {
        let dir = std::env::temp_dir().join(format!("nikgob-scrub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = concat!(
            "[2026-11-01T10:00:00.000Z INFO  nikgob] music.nikgob 0.0.9 started, log at C:\\Users\\Rafael\\AppData\\Roaming\\nikgob\\nikgob.log\n",
            "[2026-11-01T10:00:01.000Z INFO  librespot_core::session] Authenticated as 'iri5cuexample'\n",
            "[2026-11-01T10:00:01.001Z INFO  librespot_core::session] Country: \"IL\"\n",
            "[2026-11-01T10:00:02.000Z INFO  nikgob_soulseek::client] soulseek: listening on port 2234\n",
        );
        let plain = "[2026-10-30T09:00:00.000Z WARN  nikgob_runtime::search] prowlarr check: indexer is unreachable\n";
        let crash = "--- panic at unix 1 in thread 'main'\n   at C:\\Users\\PC\\.cargo\\registry\\src\\lib.rs:10\n";
        std::fs::write(dir.join(FILE), old).unwrap();
        std::fs::write(dir.join(format!("{FILE}.1")), plain).unwrap();
        std::fs::write(dir.join(CRASH), crash).unwrap();
        scrub_files(&dir, &home());
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE)).unwrap(),
            concat!(
                "[2026-11-01T10:00:00.000Z INFO  nikgob] music.nikgob 0.0.9 started, log at ~\\AppData\\Roaming\\nikgob\\nikgob.log\n",
                "[2026-11-01T10:00:01.000Z INFO  librespot_core::session] Authenticated\n",
                "[2026-11-01T10:00:02.000Z INFO  nikgob_soulseek::client] soulseek: listening on port 2234\n",
            )
        );
        assert_eq!(std::fs::read_to_string(dir.join(format!("{FILE}.1"))).unwrap(), plain);
        assert_eq!(
            std::fs::read_to_string(dir.join(CRASH)).unwrap(),
            "--- panic at unix 1 in thread 'main'\n   at C:\\Users\\~\\.cargo\\registry\\src\\lib.rs:10\n"
        );
        assert!(!scrub_file(&dir.join(FILE), &home()).unwrap(), "a scrubbed file is not written again");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3, "nothing is left beside the files");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
