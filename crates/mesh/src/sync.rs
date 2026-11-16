//! The library's sync with the owner's other devices: every few seconds,
//! each paired owner device that is online and not turned off is asked
//! how far its changes go (`/v1/sync/heads`) and, when it has new ones,
//! they are taken table by table in pages (`/v1/sync/changes`) and merged
//! here ([`nikgob_library::Library::sync_apply`], on the blocking pool, one
//! short transaction a page). Pushing is the other side pulling from here.
//! A device whose database is another one (its epoch) or went back is
//! taken from the beginning again: merged, never wiped. Errors back off
//! (5 s doubling to 5 min). How it goes is reported to the runtime
//! ([`Command::MeshSyncReport`]) for the device's row.
//!
//! This device's id in the library becomes its node id here (once).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
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
/// A breath between two pages (the other side's rate limit, the writer).
const PAGE_GAP: Duration = Duration::from_millis(20);
/// The largest answer taken.
const MAX_BODY: usize = 16 << 20;

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

/// A GET of the device's API, its JSON answer.
async fn get_json<T: serde::de::DeserializeOwned>(sh: &Arc<Shared>, id: &str, uri: &str) -> Result<T, String> {
    let req = http::Request::get(uri).body(Body::empty()).map_err(|e| e.to_string())?;
    let resp = sh.request(id, req).await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), MAX_BODY).await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).into_owned();
        return Err(format!("{status}: {text}"));
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

/// Takes what device `id` has new. `me` is this device's node id.
async fn pull(sh: &Arc<Shared>, lib: &Library, me: &str, id: &str, p: &mut Peer) -> Result<(), String> {
    let heads: SyncHeads = get_json(sh, id, &format!("/v1/sync/heads?peer={me}")).await?;
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
    let (mut taken, mut skipped) = (0u64, 0u64);
    for (table, from, head) in todo {
        let mut since = from;
        while since < head {
            let uri = format!("/v1/sync/changes?table={table}&since={since}&limit={}&peer={me}", nikgob_library::sync::PAGE_ROWS);
            let page: SyncPage = get_json(sh, id, &uri).await?;
            if page.table != table {
                return Err(format!("asked for {table}, got {}", page.table));
            }
            let (next, more) = (page.next, page.more);
            let done = {
                let (l, peer) = (lib.clone(), id.to_owned());
                blocking(move || l.sync_apply(&page, &peer, true)).await?
            };
            taken += u64::from(done.taken);
            skipped += u64::from(done.skipped);
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
            tokio::time::sleep(PAGE_GAP).await;
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
    if taken > 0 || skipped > 0 {
        log::info!("sync: from {id}: {taken} rows taken, {skipped} left out");
    }
    Ok(())
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
        if !owner {
            continue;
        }
        let off = {
            let l = lib.clone();
            blocking(move || l.sync_off_nodes()).await.unwrap_or_default()
        };
        peers.retain(|id, _| members.contains(id));
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
            match pull(&sh, &lib, &me, &id, p).await {
                Ok(()) => {
                    p.fails = 0;
                    p.next = Instant::now();
                    p.view.error = None;
                }
                Err(e) => {
                    p.fails += 1;
                    p.next = Instant::now() + backoff(p.fails);
                    log::info!("sync: with {id} failed ({} in a row): {e}", p.fails);
                    p.view.error = Some(if e.starts_with("403") { "Устройство не принимает синхронизацию".to_owned() } else { "Синхронизация не удалась".to_owned() });
                }
            }
            p.view.running = false;
            report(&sh, &id, p);
        }
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
}
