//! The library's sync with the owner's other devices: every few seconds,
//! each paired owner device that is online and not turned off is asked
//! how far its changes go (`/v1/sync/heads`) and, when it has new ones,
//! they are taken table by table in pages (`/v1/sync/changes`) and merged
//! here ([`nikgob_library::Library::sync_apply`], on the blocking pool, one
//! short transaction a page). Pushing is the other side pulling from here.
//! A device whose database is another one (its epoch) or went back is
//! taken from the beginning again: merged, never wiped. A page from a
//! clock more than five minutes ahead is not taken («часы устройства
//! спешат»). Errors back off (5 s doubling to 5 min). Every request to the
//! devices, heads and pictures too, is paced to [`PACE`] (under the API's
//! rate limit, with room for the rest).
//!
//! Each turn the rows kept from before (that did not go through, deletes
//! held back) are tried again; their count is on the device's row
//! ([`Command::MeshSyncReport`]) with how a pull goes.
//!
//! Pictures: the covers and artist photos this device has are hashed
//! (BLAKE3) a few at a time; those the synced rows name and this device
//! lacks are fetched by hash from a device that has them, checked, and
//! kept in the library's own cache (`.musiclib/covers/sync/<hash>.<ext>`):
//! never into an album folder, never over a file.
//!
//! This device's id in the library becomes its node id here (once).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use nikgob_library::{Library, SYNC_TABLES, SyncHeads, SyncPage};
use nikgob_view::{Command, MeshScope, MeshSyncView};
use tokio::time::{Instant, MissedTickBehavior};

use super::Shared;

/// How often the devices are asked.
const EVERY: Duration = Duration::from_secs(5);
/// The longest wait after errors.
const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// A running pull reports at most this often.
const REPORT_EVERY: Duration = Duration::from_millis(500);
/// The least time between two requests to the devices (about 14 a second;
/// the API lets a caller 20).
const PACE: Duration = Duration::from_millis(70);
/// The largest answer taken.
const MAX_BODY: usize = 16 << 20;
/// The largest picture hashed or taken.
const MAX_PICTURE: u64 = 20 << 20;
/// Pictures of this device hashed a turn.
const INDEX_BATCH: u32 = 100;
/// Pictures fetched a turn.
const FETCH_BATCH: u32 = 20;
/// A picture no device gave is asked again after this.
const FETCH_AGAIN: Duration = Duration::from_secs(600);
/// Pictures remembered as not given, at most.
const MISSES_MAX: usize = 2_000;

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

/// Takes what device `id` has new.
async fn pull(sh: &Arc<Shared>, pace: &mut Pacer, lib: &Library, id: &str, p: &mut Peer) -> Result<(), Failure> {
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
        return Ok(());
    }
    p.view.running = true;
    p.view.done = 0;
    p.view.total = todo.iter().map(|(_, from, to)| (to - from) as u64).sum();
    report(sh, id, p);
    let (mut taken, mut kept) = (0u64, 0u64);
    for (table, from, head) in todo {
        let mut since = from;
        while since < head {
            let uri = format!("/v1/sync/changes?table={table}&since={since}&limit={}", nikgob_library::sync::PAGE_ROWS);
            let page: SyncPage = get_json(sh, pace, id, &uri).await?;
            if page.table != table {
                return Err(Failure::Other(format!("asked for {table}, got {}", page.table)));
            }
            if nikgob_library::page_ahead(&page, nikgob_library::now_ms()) {
                log::warn!("sync: {id}'s clock runs more than five minutes ahead; nothing of it taken");
                return Err(Failure::Ahead);
            }
            let (next, more) = (page.next, page.more);
            let done = {
                let (l, peer) = (lib.clone(), id.to_owned());
                blocking(move || l.sync_apply(&page, &peer, true)).await?
            };
            taken += u64::from(done.taken);
            kept += u64::from(done.held + done.failed);
            p.view.done += (next.min(head) - since).max(0) as u64;
            if next <= since {
                break;
            }
            since = next;
            if p.last_report.elapsed() >= REPORT_EVERY {
                report(sh, id, p);
            }
            if !more {
                break;
            }
        }
    }
    let now = nikgob_library::now_ms();
    {
        let (l, peer) = (lib.clone(), id.to_owned());
        blocking(move || l.sync_mark_ok(&peer, now)).await?;
    }
    if taken > 0 {
        p.view.last_ms = Some(now);
    }
    if taken > 0 || kept > 0 {
        log::info!("sync: from {id}: {taken} rows taken, {kept} kept to try again");
    }
    Ok(())
}

/// The kept rows of `id` on its row; rows that could not be written are a
/// blocker to show.
async fn note_conflicts(lib: &Library, id: &str, p: &mut Peer) {
    let (l, peer) = (lib.clone(), id.to_owned());
    match blocking(move || l.sync_conflicts(&peer)).await {
        Ok(c) => {
            p.view.conflicts = c.failed;
            p.view.held = c.held;
            if c.failed > 0 && p.view.error.is_none() {
                p.view.error = Some(format!("Не все изменения приняты: {}", c.failed));
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

/// The BLAKE3 hash of the picture file at `path` (none: not one, too big,
/// not readable).
fn hash_picture(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PICTURE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    picture_ext(&bytes)?;
    Some(blake3::hash(&bytes).to_hex().to_string())
}

/// Hashes a few covers and photos of this device.
async fn index_pictures(lib: &Library, library: PathBuf) {
    let l = lib.clone();
    let done = blocking(move || {
        for (kind, id, reference) in l.pictures_to_index(INDEX_BATCH)? {
            let path = picture_at(&reference, &library);
            let hash = hash_picture(&path);
            l.put_picture(&kind, &id, &reference, hash.as_deref().map(|h| (h, path.as_path())))?;
        }
        Ok(())
    })
    .await;
    if let Err(e) = done {
        log::warn!("sync: pictures not hashed: {e}");
    }
}

/// Keeps picture `bytes` of `hash` in `dir` (never over a file) and notes
/// it. Whether it is kept.
fn keep_picture(lib: &Library, dir: &Path, hash: &str, bytes: &[u8]) -> nikgob_library::Result<bool> {
    let Some(ext) = picture_ext(bytes) else { return Ok(false) };
    if blake3::hash(bytes).to_hex().as_str() != hash {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{hash}.{ext}"));
    if !path.exists() {
        let tmp = dir.join(format!("{hash}.part"));
        std::fs::write(&tmp, bytes)?;
        if path.exists() {
            let _ = std::fs::remove_file(&tmp);
        } else {
            std::fs::rename(&tmp, &path)?;
        }
    }
    lib.put_picture_file(hash, &path)?;
    Ok(true)
}

/// Fetches a few pictures this device lacks from `from` (devices synced
/// with now).
async fn fetch_pictures(sh: &Arc<Shared>, pace: &mut Pacer, lib: &Library, library: &Path, from: &[String], misses: &mut HashMap<String, Instant>) {
    if from.is_empty() {
        return;
    }
    let wanted = {
        let l = lib.clone();
        match blocking(move || l.pictures_missing(FETCH_BATCH * 4)).await {
            Ok(w) => w,
            Err(e) => {
                log::warn!("sync: the pictures missing not read: {e}");
                return;
            }
        }
    };
    let now = Instant::now();
    misses.retain(|_, at| now.duration_since(*at) < FETCH_AGAIN);
    let dir = library.join(".musiclib").join("covers").join("sync");
    for hash in wanted.into_iter().filter(|h| !misses.contains_key(h)).take(FETCH_BATCH as usize) {
        let mut got = false;
        for id in from {
            let uri = format!("/v1/sync/blob?hash={hash}");
            let Ok((status, bytes)) = get(sh, pace, id, &uri, MAX_PICTURE as usize).await else { continue };
            if !status.is_success() {
                continue;
            }
            let (l, d, h) = (lib.clone(), dir.clone(), hash.clone());
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
            if misses.len() >= MISSES_MAX {
                misses.clear();
            }
            misses.insert(hash, Instant::now());
        }
    }
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
    let mut misses: HashMap<String, Instant> = HashMap::new();
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
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
        let library = sh.rt.state().prefs.library_dir.clone();
        index_pictures(&lib, library.clone()).await;
        // What blocked a kept row may have changed here since.
        {
            let l = lib.clone();
            if let Err(e) = blocking(move || l.sync_retry()).await {
                log::warn!("sync: the kept rows not tried again: {e}");
            }
        }
        let off = {
            let l = lib.clone();
            blocking(move || l.sync_off_nodes()).await.unwrap_or_default()
        };
        peers.retain(|id, _| members.contains(id));
        let mut synced = Vec::new();
        for id in members {
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
            match pull(&sh, &mut pace, &lib, &id, p).await {
                Ok(()) => {
                    p.fails = 0;
                    p.next = Instant::now();
                    p.view.error = None;
                    synced.push(id.clone());
                }
                Err(e) => {
                    p.fails += 1;
                    p.next = Instant::now() + backoff(p.fails);
                    log::info!("sync: with {id} failed ({} in a row): {e:?}", p.fails);
                    p.view.error = Some(e.shown().to_owned());
                }
            }
            p.view.running = false;
            note_conflicts(&lib, &id, p).await;
            report(&sh, &id, p);
        }
        fetch_pictures(&sh, &mut pace, &lib, &library, &synced, &mut misses).await;
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
    /// only when it is the picture asked for, and never over a file.
    #[test]
    fn a_picture_is_kept_by_its_hash_in_the_cache() {
        let lib = Library::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bytes = [0xFF, 0xD8, 0xFF, 1, 2, 3];
        let hash = blake3::hash(&bytes).to_hex().to_string();
        assert!(!keep_picture(&lib, dir.path(), &hash, b"\xFF\xD8\xFFother").unwrap(), "not the one asked for");
        assert!(!keep_picture(&lib, dir.path(), &blake3::hash(b"text").to_hex(), b"text").unwrap(), "not a picture");
        assert!(keep_picture(&lib, dir.path(), &hash, &bytes).unwrap());
        let path = dir.path().join(format!("{hash}.jpg"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(lib.picture_file(&hash).unwrap(), Some(path.clone()));
        assert!(keep_picture(&lib, dir.path(), &hash, &bytes).unwrap(), "again: the file stays");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
