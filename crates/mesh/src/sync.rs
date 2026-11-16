//! The library's sync with the owner's other devices. Each turn, each
//! paired owner device that is online and not turned off is asked how far
//! its changes go (`/v1/sync/heads`) and, when it has new ones, they are
//! taken table by table in pages (`/v1/sync/changes`) and merged here
//! ([`nikgob_library::Library::sync_apply`], on the blocking pool, one short
//! transaction a page) — at most [`PAGES_PER_TURN`] pages a device a turn,
//! the devices in turn, so a long first sync of one holds up neither the
//! others nor the pictures; while any has more, the next turn comes soon.
//! Pushing is the other side pulling from here. A device whose database is
//! another one (its epoch) or went back is taken from the beginning again:
//! merged, never wiped. A page from a clock more than five minutes ahead is
//! not taken («часы устройства спешат»). Errors back off (5 s doubling to
//! 5 min). Every request to the devices, heads and pictures too, is paced
//! to [`PACE`] (under the API's rate limit, with room for the rest). When
//! syncing is off on this database (its epoch's copy could not be kept),
//! nothing is asked and each device's row says so.
//!
//! Each turn the kept rows that are due are tried again; their counts are
//! on the device's row ([`Command::MeshSyncReport`]) with how a pull goes:
//! artists, albums and streams this device has under another id are the
//! blocker shown («Не все изменения приняты: N»).
//!
//! Pictures: the covers and artist photos this device has are hashed
//! (BLAKE3) a few at a time; those the synced rows name and this device
//! lacks (no file, or its file gone) are fetched by hash from a device that
//! has them, checked, and kept in the library's own cache
//! (`.musiclib/covers/sync/<hash>.<ext>`, which has to resolve inside the
//! library folder): never into an album folder.
//!
//! This device's id in the library becomes its node id here (once).

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use nikgob_library::{Library, SYNC_TABLES, SyncHeads, SyncPage};
use nikgob_view::{Command, MeshScope, MeshSyncView};
use tokio::time::Instant;

use super::Shared;

/// How often the devices are asked when nothing is left to take.
const EVERY: Duration = Duration::from_secs(5);
/// The next turn while a device has more to take.
const SOON: Duration = Duration::from_millis(250);
/// The longest wait after errors.
const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// A running pull reports at most this often.
const REPORT_EVERY: Duration = Duration::from_millis(500);
/// The least time between two requests to the devices (about 14 a second;
/// the API lets a caller 20).
const PACE: Duration = Duration::from_millis(70);
/// Pages taken from one device in a turn, at most.
const PAGES_PER_TURN: u32 = 20;
/// The largest answer taken.
const MAX_BODY: usize = 16 << 20;
/// The largest picture hashed or taken.
const MAX_PICTURE: u64 = 20 << 20;
/// Pictures of this device hashed a turn.
const INDEX_BATCH: u32 = 100;
/// A picture file that could not be read is read again after this.
const INDEX_AGAIN: Duration = Duration::from_secs(3_600);
/// Pictures fetched a turn.
const FETCH_BATCH: usize = 20;
/// Picture rows looked through for those to fetch, a turn.
const SCAN_ROWS: u32 = 2_000;
/// A picture no device gave is asked again after this.
const FETCH_AGAIN: Duration = Duration::from_secs(600);
/// Pictures remembered as not given, at most.
const MISSES_MAX: usize = 2_000;
/// What a device's row says when syncing is off on this database.
const BLOCKED: &str = "Синхронизация остановлена: метка базы не записана";

/// What the loop keeps of a device.
struct Peer {
    next: Instant,
    fails: u32,
    view: MeshSyncView,
    sent: Option<MeshSyncView>,
    last_report: Instant,
}

impl Peer {
    fn new() -> Self {
        Self { next: Instant::now(), fails: 0, view: MeshSyncView::default(), sent: None, last_report: Instant::now() }
    }
}

/// Why a pull did not go through.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The device does not take sync (turned off there, not an owner).
    Refused,
    /// Its clock runs ahead: nothing of it is taken.
    Ahead,
    Other(String),
}

impl Failure {
    /// What the device's row shows.
    fn shown(&self) -> &'static str {
        match self {
            Self::Refused => "Устройство не принимает синхронизацию",
            Self::Ahead => "Часы устройства спешат",
            Self::Other(_) => "Синхронизация не удалась",
        }
    }
}

impl From<String> for Failure {
    fn from(e: String) -> Self {
        if e.starts_with("403") {
            Self::Refused
        } else if e.starts_with("409") {
            Self::Ahead
        } else {
            Self::Other(e)
        }
    }
}

/// Keeps the requests to the devices at least [`PACE`] apart.
struct Pacer {
    last: Option<Instant>,
}

impl Pacer {
    async fn wait(&mut self) {
        if let Some(last) = self.last {
            tokio::time::sleep_until(last + PACE).await;
        }
        self.last = Some(Instant::now());
    }
}

/// The wait after `fails` failures in a row.
pub(crate) fn backoff(fails: u32) -> Duration {
    EVERY.saturating_mul(1u32 << fails.min(6)).min(BACKOFF_MAX)
}

fn report(sh: &Shared, id: &str, p: &mut Peer) {
    if p.sent.as_ref() != Some(&p.view) {
        sh.rt.send(Command::MeshSyncReport { node_id: id.to_owned(), sync: p.view.clone() });
        p.sent = Some(p.view.clone());
    }
    p.last_report = Instant::now();
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> nikgob_library::Result<T> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// A GET of the device's API (paced), its answer: the status and the body.
async fn get(sh: &Arc<Shared>, pace: &mut Pacer, id: &str, uri: &str, max: usize) -> Result<(http::StatusCode, Bytes), String> {
    pace.wait().await;
    let req = http::Request::get(uri).body(Body::empty()).map_err(|e| e.to_string())?;
    let resp = sh.request(id, req).await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), max).await.map_err(|e| e.to_string())?;
    Ok((status, bytes))
}

/// A GET of the device's API, its JSON answer.
async fn get_json<T: serde::de::DeserializeOwned>(sh: &Arc<Shared>, pace: &mut Pacer, id: &str, uri: &str) -> Result<T, String> {
    let (status, bytes) = get(sh, pace, id, uri, MAX_BODY).await?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).into_owned();
        return Err(format!("{}: {text}", status.as_u16()));
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// What is left to take of each table: (table, from, up to).
pub(crate) fn pending(heads: &SyncHeads, cursors: &std::collections::BTreeMap<String, i64>) -> Vec<(&'static str, i64, i64)> {
    SYNC_TABLES
        .iter()
        .filter_map(|t| {
            let head = heads.tables.get(t.name).copied().unwrap_or(0);
            let at = cursors.get(t.name).copied().unwrap_or(0).min(head);
            (head > at).then_some((t.name, at, head))
        })
        .collect()
}

/// Takes what device `id` has new, at most `budget` pages. Whether it has
/// more (the next turn goes on from the cursors).
async fn pull(sh: &Arc<Shared>, pace: &mut Pacer, lib: &Library, id: &str, p: &mut Peer, budget: u32) -> Result<bool, Failure> {
    let heads: SyncHeads = get_json(sh, pace, id, "/v1/sync/heads").await?;
    let cursors = {
        let (l, peer) = (lib.clone(), id.to_owned());
        blocking(move || l.sync_cursors(&peer)).await?
    };
    let mut at = cursors.tables;
    if cursors.epoch.as_deref() != Some(heads.epoch.as_str()) {
        // Met for the first time, or its database is another one now.
        let (l, peer, epoch) = (lib.clone(), id.to_owned(), heads.epoch.clone());
        blocking(move || l.sync_start_over(&peer, &epoch, &[])).await?;
        at.clear();
    }
    let back: Vec<&'static str> =
        SYNC_TABLES.iter().filter(|t| at.get(t.name).copied().unwrap_or(0) > heads.tables.get(t.name).copied().unwrap_or(0)).map(|t| t.name).collect();
    if !back.is_empty() {
        log::info!("sync: {id} went back in {}; taken again from the start", back.join(", "));
        let (l, peer, epoch, tables) = (lib.clone(), id.to_owned(), heads.epoch.clone(), back.clone());
        blocking(move || l.sync_start_over(&peer, &epoch, &tables)).await?;
        for t in &back {
            at.remove(*t);
        }
    }
    let todo = pending(&heads, &at);
    if todo.is_empty() {
        return Ok(false);
    }
    let left: u64 = todo.iter().map(|(_, from, to)| (to - from) as u64).sum();
    if p.view.running {
        // Going on from the last turn: the bar keeps what it had.
        p.view.total = p.view.done + left;
    } else {
        p.view.running = true;
        p.view.done = 0;
        p.view.total = left;
    }
    report(sh, id, p);
    let (mut taken, mut kept, mut pages) = (0u64, 0u64, 0u32);
    let mut more = false;
    'tables: for (table, from, head) in todo {
        let mut since = from;
        while since < head {
            if pages >= budget {
                more = true;
                break 'tables;
            }
            pages += 1;
            let uri = format!("/v1/sync/changes?table={table}&since={since}&limit={}", nikgob_library::sync::PAGE_ROWS);
            let page: SyncPage = get_json(sh, pace, id, &uri).await?;
            if page.table != table {
                return Err(Failure::Other(format!("asked for {table}, got {}", page.table)));
            }
            if nikgob_library::page_ahead(&page, nikgob_library::now_ms()) {
                log::warn!("sync: {id}'s clock runs more than five minutes ahead; nothing of it taken");
                return Err(Failure::Ahead);
            }
            let (next, page_more) = (page.next, page.more);
            let done = {
                let (l, peer) = (lib.clone(), id.to_owned());
                blocking(move || l.sync_apply(&page, &peer, true)).await?
            };
            taken += u64::from(done.taken);
            kept += u64::from(done.held + done.waiting + done.identity);
            p.view.done += (next.min(head) - since).max(0) as u64;
            if next <= since {
                break;
            }
            since = next;
            if p.last_report.elapsed() >= REPORT_EVERY {
                report(sh, id, p);
            }
            if !page_more {
                break;
            }
        }
    }
    let now = nikgob_library::now_ms();
    if !more {
        let (l, peer) = (lib.clone(), id.to_owned());
        blocking(move || l.sync_mark_ok(&peer, now)).await?;
    }
    if taken > 0 {
        p.view.last_ms = Some(now);
    }
    if taken > 0 || kept > 0 {
        log::info!("sync: from {id}: {taken} rows taken, {kept} kept");
    }
    Ok(more)
}

/// The kept rows of `id` on its row; artists, albums and streams under
/// another id are the blocker shown.
async fn note_conflicts(lib: &Library, id: &str, p: &mut Peer) {
    let (l, peer) = (lib.clone(), id.to_owned());
    match blocking(move || l.sync_conflicts(&peer)).await {
        Ok(c) => {
            p.view.conflicts = c.identity;
            p.view.waiting = c.waiting;
            p.view.held = c.held;
            if c.identity > 0 && p.view.error.is_none() {
                p.view.error = Some(format!("Не все изменения приняты: {}", c.identity));
            }
        }
        Err(e) => log::warn!("sync: the kept rows of {id} not read: {e}"),
    }
}

// --------------------------------------------------------------- pictures

/// Where the picture a row names (`reference`) is: absolute, or in the
/// library folder.
pub(crate) fn picture_at(reference: &str, library: &Path) -> PathBuf {
    let p = Path::new(reference);
    if p.is_absolute() { p.to_owned() } else { library.join(p) }
}

/// The extension of a picture's bytes (none: not a picture).
pub(crate) fn picture_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else {
        None
    }
}

/// The BLAKE3 hash of the picture in `bytes` (none: not a picture).
fn picture_hash(bytes: &[u8]) -> Option<String> {
    picture_ext(bytes)?;
    Some(blake3::hash(bytes).to_hex().to_string())
}

/// The bytes of the file at `path` (none: not a file, too big, not
/// readable).
fn read_picture(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PICTURE {
        return None;
    }
    std::fs::read(path).ok()
}

/// Hashes a few covers and photos of this device (a reference is done only
/// once its file was read).
async fn index_pictures(lib: &Library, library: PathBuf) {
    let l = lib.clone();
    let again = nikgob_library::now_ms() - INDEX_AGAIN.as_millis() as i64;
    let done = blocking(move || {
        for (kind, id, reference) in l.pictures_to_index(INDEX_BATCH, again)? {
            let path = picture_at(&reference, &library);
            let hash = read_picture(&path).and_then(|b| picture_hash(&b));
            l.put_picture(&kind, &id, &reference, hash.as_deref().map(|h| (h, path.as_path())))?;
        }
        Ok(())
    })
    .await;
    if let Err(e) = done {
        log::warn!("sync: pictures not hashed: {e}");
    }
}

/// The cache folder for pictures taken from devices, made if need be: its
/// real path, only when it is inside the library folder (a link out of it
/// is refused).
fn cache_dir(library: &Path) -> std::io::Result<PathBuf> {
    let dir = library.join(".musiclib").join("covers").join("sync");
    std::fs::create_dir_all(&dir)?;
    let real = std::fs::canonicalize(&dir)?;
    let root = std::fs::canonicalize(library)?;
    if !real.starts_with(&root) {
        return Err(std::io::Error::other(format!("{} is outside the library folder", real.display())));
    }
    Ok(real)
}

/// Writes `bytes` to `path` through a new `.part` beside it (never one
/// left from before: it is removed first).
fn write_through_part(dir: &Path, hash: &str, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let part = dir.join(format!("{hash}.part"));
    let open = || std::fs::OpenOptions::new().write(true).create_new(true).open(&part);
    let mut file = match open() {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(&part)?;
            open()?
        }
        Err(e) => return Err(e),
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, path)
}

/// Keeps picture `bytes` of `hash` in the cache of `library` and notes it.
/// Whether it is kept. A file already there under that name is used only
/// when it is that picture; else it is replaced (it is the cache's own).
fn keep_picture(lib: &Library, library: &Path, hash: &str, bytes: &[u8]) -> nikgob_library::Result<bool> {
    let Some(ext) = picture_ext(bytes) else { return Ok(false) };
    if blake3::hash(bytes).to_hex().as_str() != hash {
        return Ok(false);
    }
    let dir = cache_dir(library)?;
    let path = dir.join(format!("{hash}.{ext}"));
    let there = read_picture(&path).is_some_and(|b| blake3::hash(&b).to_hex().as_str() == hash);
    if !there {
        write_through_part(&dir, hash, &path, bytes)?;
    }
    lib.put_picture_file(hash, &path)?;
    Ok(true)
}

/// Up to [`FETCH_BATCH`] pictures to fetch: named by the synced rows, with
/// no file here or a file gone, not given lately (`skip`), looked for from
/// `cursor` on (in hash order, wrapping round). And where to look next turn.
fn pictures_to_fetch(lib: &Library, cursor: &str, skip: &HashSet<String>) -> nikgob_library::Result<(Vec<String>, String)> {
    let mut out = Vec::new();
    let mut at = cursor.to_owned();
    let mut scanned = 0u32;
    while out.len() < FETCH_BATCH && scanned < SCAN_ROWS {
        let rows = lib.pictures_wanted(&at, 200)?;
        if rows.is_empty() {
            // The end: round again from the start next turn.
            return Ok((out, String::new()));
        }
        scanned += rows.len() as u32;
        for (hash, file) in rows {
            at.clone_from(&hash);
            let lacking = file.is_none_or(|f| !f.is_file());
            if lacking && !skip.contains(&hash) {
                out.push(hash);
                if out.len() >= FETCH_BATCH {
                    break;
                }
            }
        }
    }
    Ok((out, at))
}

/// What the picture fetching keeps between turns.
#[derive(Default)]
struct Fetching {
    /// Hashes no device gave, and when.
    misses: HashMap<String, Instant>,
    /// Where the next look for pictures to fetch starts.
    cursor: String,
}

/// Fetches a few pictures this device lacks from `from` (devices that
/// answered this turn).
async fn fetch_pictures(sh: &Arc<Shared>, pace: &mut Pacer, lib: &Library, library: &Path, from: &[String], f: &mut Fetching) {
    if from.is_empty() {
        return;
    }
    let now = Instant::now();
    f.misses.retain(|_, at| now.duration_since(*at) < FETCH_AGAIN);
    let skip: HashSet<String> = f.misses.keys().cloned().collect();
    let wanted = {
        let (l, cursor) = (lib.clone(), f.cursor.clone());
        match blocking(move || pictures_to_fetch(&l, &cursor, &skip)).await {
            Ok((w, next)) => {
                f.cursor = next;
                w
            }
            Err(e) => {
                log::warn!("sync: the pictures to fetch not read: {e}");
                return;
            }
        }
    };
    for hash in wanted {
        let mut got = false;
        for id in from {
            let uri = format!("/v1/sync/blob?hash={hash}");
            let Ok((status, bytes)) = get(sh, pace, id, &uri, MAX_PICTURE as usize).await else { continue };
            if !status.is_success() {
                continue;
            }
            let (l, d, h) = (lib.clone(), library.to_path_buf(), hash.clone());
            match blocking(move || keep_picture(&l, &d, &h, &bytes)).await {
                Ok(true) => {
                    got = true;
                    break;
                }
                Ok(false) => log::info!("sync: the picture {hash} from {id} is not what it should be"),
                Err(e) => log::warn!("sync: the picture {hash} not kept: {e}"),
            }
        }
        if !got {
            if f.misses.len() >= MISSES_MAX {
                f.misses.clear();
            }
            f.misses.insert(hash, Instant::now());
        }
    }
}

/// The devices in this turn's order: each turn starts one further, so none
/// is always last.
pub(crate) fn in_turn(mut members: Vec<String>, turn: usize) -> Vec<String> {
    members.sort();
    if !members.is_empty() {
        let n = turn % members.len();
        members.rotate_left(n);
    }
    members
}

/// The loop, until the mesh stops (it is aborted then).
pub(crate) async fn run(sh: Arc<Shared>) {
    let Some(lib) = sh.library.clone() else { return };
    let me = sh.key.public().to_string();
    {
        let (l, id) = (lib.clone(), me.clone());
        if let Err(e) = blocking(move || l.adopt_node_id(&id)).await {
            log::warn!("sync: this device's id not changed to its node id: {e}");
        }
    }
    let mut peers: HashMap<String, Peer> = HashMap::new();
    let mut pace = Pacer { last: None };
    let mut fetching = Fetching::default();
    let mut upkeep: Option<Instant> = None;
    let mut wait = EVERY;
    for turn in 0usize.. {
        tokio::time::sleep(wait).await;
        wait = EVERY;
        if sh.node.lock().is_none() {
            continue;
        }
        let (owner, members) = {
            let s = sh.state.lock();
            match s.ring.as_ref() {
                Some(r) => (
                    r.me.scope == MeshScope::Owner,
                    r.others().filter(|c| c.scope == MeshScope::Owner).map(|c| c.node.clone()).collect::<Vec<String>>(),
                ),
                None => (false, Vec::new()),
            }
        };
        if !owner || members.is_empty() {
            continue;
        }
        peers.retain(|id, _| members.contains(id));
        if lib.sync_blocked().is_some() {
            for id in members {
                let p = peers.entry(id.clone()).or_insert_with(Peer::new);
                p.view.running = false;
                p.view.error = Some(BLOCKED.to_owned());
                report(&sh, &id, p);
            }
            continue;
        }
        let library = sh.rt.state().prefs.library_dir.clone();
        // Hashing and the kept rows' retries at their own pace, not at every
        // quick turn of a long pull.
        if upkeep.is_none_or(|at| at.elapsed() >= EVERY) {
            upkeep = Some(Instant::now());
            index_pictures(&lib, library.clone()).await;
            let l = lib.clone();
            if let Err(e) = blocking(move || l.sync_retry()).await {
                log::warn!("sync: the kept rows not tried again: {e}");
            }
        }
        let off = {
            let l = lib.clone();
            blocking(move || l.sync_off_nodes()).await.unwrap_or_default()
        };
        let mut answered = Vec::new();
        for id in in_turn(members, turn) {
            let p = peers.entry(id.clone()).or_insert_with(Peer::new);
            if off.contains(&id) || !sh.online(&id) {
                if p.view.running {
                    p.view.running = false;
                    report(&sh, &id, p);
                }
                continue;
            }
            if Instant::now() < p.next {
                continue;
            }
            match pull(&sh, &mut pace, &lib, &id, p, PAGES_PER_TURN).await {
                Ok(more) => {
                    p.fails = 0;
                    p.next = Instant::now();
                    p.view.error = None;
                    p.view.running = more;
                    if more {
                        wait = SOON;
                    }
                    answered.push(id.clone());
                }
                Err(e) => {
                    p.fails += 1;
                    p.next = Instant::now() + backoff(p.fails);
                    log::info!("sync: with {id} failed ({} in a row): {e:?}", p.fails);
                    p.view.error = Some(e.shown().to_owned());
                    p.view.running = false;
                }
            }
            note_conflicts(&lib, &id, p).await;
            report(&sh, &id, p);
        }
        // The pictures' share of every turn.
        fetch_pictures(&sh, &mut pace, &lib, &library, &answered, &mut fetching).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn errors_back_off_up_to_five_minutes() {
        assert_eq!(backoff(1), Duration::from_secs(10));
        assert_eq!(backoff(2), Duration::from_secs(20));
        assert_eq!(backoff(5), Duration::from_secs(160));
        assert_eq!(backoff(6), BACKOFF_MAX);
        assert_eq!(backoff(40), BACKOFF_MAX);
    }

    #[test]
    fn only_tables_with_news_are_taken_in_their_order() {
        let mut heads = SyncHeads { epoch: "e".into(), tables: BTreeMap::new() };
        heads.tables.insert("track".into(), 50);
        heads.tables.insert("artist".into(), 40);
        heads.tables.insert("likes".into(), 7);
        let mut at = BTreeMap::new();
        at.insert("likes".to_owned(), 7);
        at.insert("artist".to_owned(), 10);
        let todo = pending(&heads, &at);
        assert_eq!(todo, vec![("artist", 10, 40), ("track", 0, 50)], "parents first, nothing for likes");
    }

    #[test]
    fn failures_show_what_went_wrong() {
        assert_eq!(Failure::from("403: off".to_owned()), Failure::Refused);
        assert_eq!(Failure::from("409: clock".to_owned()).shown(), "Часы устройства спешат");
        assert!(matches!(Failure::from("timed out".to_owned()), Failure::Other(_)));
    }

    #[test]
    fn the_devices_take_turns() {
        let m = || vec!["c".to_owned(), "a".to_owned(), "b".to_owned()];
        assert_eq!(in_turn(m(), 0), ["a", "b", "c"]);
        assert_eq!(in_turn(m(), 1), ["b", "c", "a"]);
        assert_eq!(in_turn(m(), 5), ["c", "a", "b"]);
        assert!(in_turn(Vec::new(), 3).is_empty());
    }

    /// Requests go at most about fourteen a second.
    #[tokio::test(start_paused = true)]
    async fn requests_are_paced() {
        let mut pace = Pacer { last: None };
        let start = Instant::now();
        for _ in 0..15 {
            pace.wait().await;
        }
        assert!(start.elapsed() >= PACE * 14);
        assert!(PACE * 15 >= Duration::from_secs(1));
    }

    #[test]
    fn pictures_are_known_by_their_bytes() {
        assert_eq!(picture_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(picture_ext(b"\x89PNG\r\n"), Some("png"));
        assert_eq!(picture_ext(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(picture_ext(b"fLaC"), None);
        assert_eq!(picture_at("Кино/cover.jpg", Path::new("/music")), Path::new("/music").join("Кино/cover.jpg"));
    }

    /// A picture taken from a device lands in the cache under its hash,
    /// only when it is the picture asked for; a wrong file already there
    /// under that name is replaced, a right one is used.
    #[test]
    fn a_picture_is_kept_by_its_hash_in_the_cache() {
        let lib = Library::open_in_memory().unwrap();
        let library = tempfile::tempdir().unwrap();
        let bytes = [0xFF, 0xD8, 0xFF, 1, 2, 3];
        let hash = blake3::hash(&bytes).to_hex().to_string();
        assert!(!keep_picture(&lib, library.path(), &hash, b"\xFF\xD8\xFFother").unwrap(), "not the one asked for");
        assert!(!keep_picture(&lib, library.path(), &blake3::hash(b"text").to_hex(), b"text").unwrap(), "not a picture");
        let dir = cache_dir(library.path()).unwrap();
        let path = dir.join(format!("{hash}.jpg"));
        std::fs::write(&path, b"\xFF\xD8\xFFstale").unwrap();
        std::fs::write(dir.join(format!("{hash}.part")), b"left over").unwrap();
        assert!(keep_picture(&lib, library.path(), &hash, &bytes).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "the wrong file replaced");
        assert_eq!(lib.picture_file(&hash).unwrap(), Some(path.clone()));
        assert!(keep_picture(&lib, library.path(), &hash, &bytes).unwrap(), "again: the file stays");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no part left");
    }

    /// The pictures to fetch: those whose file is not here, not those
    /// given up on lately.
    #[test]
    fn pictures_to_fetch_skip_what_is_here_and_what_was_not_given() {
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path().join("here.jpg");
        std::fs::write(&here, b"\xff\xd8\xff").unwrap();
        let gone = dir.path().join("gone.jpg");
        for (i, h) in ["h1", "h2", "h3", "h4"].into_iter().enumerate() {
            let id = lib.upsert_artist(&nikgob_core::model::Artist::new(format!("A {i}"))).unwrap().id.to_string();
            lib.put_picture("artist", &id, &format!("{h}.jpg"), Some((h, here.as_path()))).unwrap();
            if h != "h1" {
                lib.put_picture_file(h, &gone).unwrap();
            }
        }
        let skip: HashSet<String> = ["h3".to_owned()].into();
        let (out, next) = pictures_to_fetch(&lib, "", &skip).unwrap();
        assert_eq!(out, ["h2", "h4"]);
        assert_eq!(next, "", "the end: round again");
    }
}
