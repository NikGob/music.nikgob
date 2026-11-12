//! «Загрузки → Активные»: every download that runs, of every source, in one
//! list. Torrents come from the torrent client ([`crate::torrents`]); the
//! downloads of the other sources (Soulseek, Bandcamp, Internet Archive,
//! khinsider, Qobuz, Tidal…) and the saves from YouTube of «Хочу скачать»
//! items are [`Run`]s here; the imports and the cold-start jobs follow.
//!
//! One [`v::Transfer`] per running download is the only place its progress
//! lives: its row in «Активные» and its item's line in «Хочу скачать» are
//! both made of it, at the same poll, so they always say the same.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use nikgob_core::model::WishlistId;
use nikgob_view::{self as v, Transfer, TransferPhase};

use crate::Worker;
use crate::import::IMPORT_ID;
use crate::soulseek::SlskPhase;
use crate::streams::STREAM_JOB;

/// Bit of the ids of the downloads of sources other than torrents (inside
/// the import id space, so the torrent list leaves them alone).
pub(crate) const DIRECT_JOB: u64 = 1 << 44;

/// Whose a job of «Активные» is, in the list's order: the downloads first,
/// then what takes them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum JobKind {
    Torrent,
    Direct,
    Import,
    Cold,
}

pub(crate) fn job_kind(id: u64) -> JobKind {
    if id & IMPORT_ID == 0 {
        JobKind::Torrent
    } else if id & STREAM_JOB != 0 {
        JobKind::Cold
    } else if id & DIRECT_JOB != 0 {
        JobKind::Direct
    } else {
        JobKind::Import
    }
}

/// The jobs of `kind` in `list` become `jobs`; the list stays in
/// [`JobKind`] order, each kind in its own.
pub(crate) fn put_jobs(list: &mut Vec<v::Job>, kind: JobKind, jobs: Vec<v::Job>) {
    list.retain(|j| job_kind(j.id) != kind);
    list.extend(jobs);
    list.sort_by_key(|j| job_kind(j.id));
}

/// A download of a source other than torrents running for a «Хочу
/// скачать» item, or its save from YouTube.
pub(crate) struct Run {
    /// Its row's number (under [`DIRECT_JOB`]).
    job: u64,
    /// This run of it (a new start of the item is another): what it
    /// brings back is taken only while it is the item's run.
    token: u64,
    /// The item, «Artist — Title».
    title: String,
    /// Who gives it, as its rows say («mugilaw», «Bandcamp»).
    from: String,
    /// The source's name when `from` is someone of it («Soulseek»).
    source: Option<String>,
    /// The folder it writes into: a Soulseek peer's queue is told by it.
    dest: Option<PathBuf>,
    /// Its task: stopped by ✕ and «Не качать», then waited for before its
    /// folder goes.
    task: Option<tokio::task::JoinHandle<()>>,
    /// The bytes at the last poll and when, for the speed.
    sample: Option<(Instant, u64)>,
    bps: u64,
}

/// The [`Run`]s and what «Активные» last showed of them.
#[derive(Default)]
pub(crate) struct Runs {
    runs: HashMap<WishlistId, Run>,
    next: u64,
    tokens: u64,
    /// The rows last published, and their shape (what changes without
    /// the page shown).
    shown: Option<Vec<v::Job>>,
    shape: Option<Vec<(u64, TransferPhase)>>,
}

impl Runs {
    /// A new run's token, for its task to bring back with what it got.
    pub(crate) fn reserve(&mut self) -> u64 {
        self.tokens += 1;
        self.tokens
    }

    /// The run of `token` is still `item`'s (not stopped, not replaced).
    pub(crate) fn is_current(&self, item: WishlistId, token: u64) -> bool {
        self.runs.get(&item).is_some_and(|r| r.token == token)
    }

    /// `item` has a download running.
    pub(crate) fn runs(&self, item: WishlistId) -> bool {
        self.runs.contains_key(&item)
    }

    /// `item`'s download of `token` starts (one in place of the last).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(
        &mut self,
        item: WishlistId,
        token: u64,
        title: String,
        from: String,
        source: Option<String>,
        dest: Option<PathBuf>,
        task: Option<tokio::task::JoinHandle<()>>,
    ) {
        let job = match self.runs.remove(&item) {
            Some(old) => {
                if let Some(t) = old.task {
                    t.abort();
                }
                old.job
            }
            None => {
                self.next += 1;
                self.next
            }
        };
        // Who gives it, else the source; the source apart only when it is
        // not that already.
        let (from, source) = match source {
            Some(s) if from.is_empty() || s == from => (s, None),
            s => (from, s),
        };
        self.runs.insert(item, Run { job, token, title, from, source, dest, task, sample: None, bps: 0 });
    }

    /// `item`'s download ended (its files came, it failed).
    pub(crate) fn end(&mut self, item: WishlistId) {
        self.runs.remove(&item);
    }

    /// `item`'s download stops now (the item was turned down, ✕): its
    /// token and its task, aborted, to wait for before its files go.
    pub(crate) fn stop(&mut self, item: WishlistId) -> Option<(u64, Option<tokio::task::JoinHandle<()>>)> {
        let r = self.runs.remove(&item)?;
        if let Some(t) = &r.task {
            t.abort();
        }
        Some((r.token, r.task))
    }

    #[cfg(test)]
    fn get(&self, item: WishlistId) -> Option<&Run> {
        self.runs.get(&item)
    }

    /// The item of the row `id` of «Активные».
    pub(crate) fn item_of(&self, id: u64) -> Option<WishlistId> {
        let n = id & !(IMPORT_ID | DIRECT_JOB);
        self.runs.iter().find(|(_, r)| r.job == n).map(|(i, _)| *i)
    }
}

/// Where a download of a source other than torrents is: the bytes its task
/// wrote, a Soulseek peer's state, what the import does with it.
pub(crate) fn direct_transfer(bytes: Option<(u64, Option<u64>)>, slsk: Option<SlskPhase>, bps: u64) -> Transfer {
    let (done, total) = bytes.unwrap_or((0, None));
    let phase = match slsk {
        Some(SlskPhase::Queued(place)) => TransferPhase::Queued(place),
        Some(SlskPhase::Connecting) => TransferPhase::Connecting,
        Some(SlskPhase::Transferring { .. }) => TransferPhase::Going,
        None if done > 0 => TransferPhase::Going,
        None => TransferPhase::Starting,
    };
    let bps = match slsk {
        Some(SlskPhase::Transferring { bps }) if bps > 0 => bps,
        _ => bps,
    };
    Transfer { phase, done, total, bps: if phase == TransferPhase::Going { bps } else { 0 } }
}

/// Bytes per second between two polls, eased so a burst does not jump.
fn speed(before: Option<(Instant, u64)>, now: Instant, done: u64, last: u64) -> u64 {
    let Some((at, was)) = before else { return 0 };
    let dt = now.duration_since(at).as_secs_f64();
    if dt < 0.2 {
        return last;
    }
    let fresh = (done.saturating_sub(was) as f64 / dt) as u64;
    if last == 0 { fresh } else { (fresh + last) / 2 }
}

/// The rows' shape: what is there and in which state, without the numbers.
fn shape(runs: &[(u64, Transfer)]) -> Vec<(u64, TransferPhase)> {
    runs.iter().map(|(id, t)| (*id, t.phase)).collect()
}

/// The wished items' transfers with the speeds left out (what changes
/// every second while the page is not shown).
fn still(m: &HashMap<WishlistId, Transfer>) -> HashMap<WishlistId, Transfer> {
    m.iter().map(|(k, t)| (*k, Transfer { bps: 0, ..*t })).collect()
}

impl Worker {
    /// Once a second, after the torrents' poll: every running download's
    /// [`Transfer`], into «Активные» (the rows of the sources other than
    /// torrents) and «Хочу скачать» (the items' lines), from the same
    /// numbers.
    pub(crate) fn publish_running(&mut self) {
        let now = Instant::now();
        let bytes = self.streaming.wish.direct_bytes();
        let importing: HashSet<WishlistId> = self.importing_wishes();
        let mut rows: Vec<(u64, String, Option<String>, String, Transfer)> = Vec::new();
        let mut transfers: HashMap<WishlistId, Transfer> = self.streaming.wish.torrent_transfers.clone();
        let phases = self.soulseek.phases();
        for (item, run) in &mut self.streaming.wish.runs.runs {
            let got = bytes.get(item).copied();
            let done = got.map_or(0, |(d, _)| d);
            run.bps = speed(run.sample, now, done, run.bps);
            run.sample = Some((now, done));
            let slsk = run.dest.as_ref().and_then(|d| phases.get(d).copied());
            let t = direct_transfer(got, slsk, run.bps);
            rows.push((run.job, run.title.clone(), run.source.clone(), run.from.clone(), t));
            transfers.insert(*item, t);
        }
        // Its files came: the import takes them (its own row says how).
        for item in importing {
            transfers.entry(item).or_insert(Transfer { phase: TransferPhase::Importing, ..Transfer::default() });
        }
        rows.sort_by_key(|(id, ..)| *id);

        let live = self.torrents.shown && self.window_visible();
        let w = &mut self.streaming.wish;
        let changed = if live { transfers != w.transfers } else { still(&transfers) != still(&w.transfers) };
        if changed {
            w.transfers = transfers.clone();
            self.publish(|s| {
                for r in &mut s.downloads.wishlist.items {
                    r.transfer = transfers.get(&r.id).copied().filter(|_| r.status == v::WishlistStatus::Downloading);
                }
            });
        }

        let jobs: Vec<v::Job> = rows
            .iter()
            .map(|(id, title, via, from, t)| t.job(IMPORT_ID | DIRECT_JOB | id, title.clone(), via.as_deref(), Some(from), true, false))
            .collect();
        let shape_now = shape(&rows.iter().map(|(id, .., t)| (*id, *t)).collect::<Vec<_>>());
        let runs = &mut self.streaming.wish.runs;
        let moved = runs.shown.as_ref() != Some(&jobs);
        if moved && (live || runs.shape.as_ref() != Some(&shape_now)) {
            runs.shown = Some(jobs.clone());
            runs.shape = Some(shape_now);
            self.publish(|s| put_jobs(&mut s.downloads.active, JobKind::Direct, jobs));
        }
    }

    /// ✕ on a row of a source other than torrents: its item is turned
    /// down, which stops the download ([`Self::wish_dismiss`]).
    pub(crate) fn cancel_run(&mut self, id: u64) {
        if let Some(item) = self.streaming.wish.runs.item_of(id) {
            self.wish_dismiss(&[item]);
            self.publish_running();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: u64) -> v::Job {
        v::Job {
            id,
            title: id.to_string(),
            subtitle: String::new(),
            progress: None,
            rate: None,
            eta: None,
            paused: false,
            queued: false,
            controllable: true,
            pausable: false,
            error: None,
            at: 0,
        }
    }

    fn ids(list: &[v::Job]) -> Vec<u64> {
        list.iter().map(|j| j.id).collect()
    }

    /// Every source's downloads in one list: torrents, then the others,
    /// then the imports and the cold-start jobs; a part replaced leaves the
    /// rest as it was.
    #[test]
    fn one_list_of_every_download() {
        let direct = |n: u64| IMPORT_ID | DIRECT_JOB | n;
        let import = |n: u64| IMPORT_ID | n;
        let cold = |n: u64| IMPORT_ID | STREAM_JOB | n;
        assert_eq!(job_kind(7), JobKind::Torrent);
        assert_eq!(job_kind(direct(1)), JobKind::Direct);
        assert_eq!(job_kind(import(1)), JobKind::Import);
        assert_eq!(job_kind(cold(1)), JobKind::Cold);

        let mut list = Vec::new();
        put_jobs(&mut list, JobKind::Import, vec![job(import(5))]);
        put_jobs(&mut list, JobKind::Cold, vec![job(cold(1))]);
        put_jobs(&mut list, JobKind::Direct, vec![job(direct(1)), job(direct(2))]);
        put_jobs(&mut list, JobKind::Torrent, vec![job(3), job(1)]);
        assert_eq!(ids(&list), vec![3, 1, direct(1), direct(2), import(5), cold(1)]);
        // A Soulseek download ended: the others stay.
        put_jobs(&mut list, JobKind::Direct, vec![job(direct(2))]);
        assert_eq!(ids(&list), vec![3, 1, direct(2), import(5), cold(1)]);
        put_jobs(&mut list, JobKind::Torrent, Vec::new());
        assert_eq!(ids(&list), vec![direct(2), import(5), cold(1)]);
    }

    /// A peer's queue, its connection, the bytes: what the row says.
    #[test]
    fn a_soulseek_download_as_it_goes() {
        let t = direct_transfer(Some((0, Some(100))), Some(SlskPhase::Queued(Some(4))), 0);
        assert_eq!(t.phase, TransferPhase::Queued(Some(4)));
        assert_eq!(t.percent(), None);
        let t = direct_transfer(Some((0, Some(100))), Some(SlskPhase::Connecting), 0);
        assert_eq!(t.phase, TransferPhase::Connecting);
        let t = direct_transfer(Some((37, Some(100))), Some(SlskPhase::Transferring { bps: 2048 }), 10);
        assert_eq!((t.phase, t.percent(), t.bps), (TransferPhase::Going, Some(37), 2048));
        // Another source: the bytes say it goes, the polls its speed.
        let t = direct_transfer(Some((10, None)), None, 512);
        assert_eq!((t.phase, t.bps), (TransferPhase::Going, 512));
        assert_eq!(direct_transfer(None, None, 0).phase, TransferPhase::Starting);
    }

    #[test]
    fn the_speed_between_polls() {
        let t0 = Instant::now();
        assert_eq!(speed(None, t0, 100, 0), 0);
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert_eq!(speed(Some((t0, 0)), t1, 1000, 0), 1000);
        assert_eq!(speed(Some((t0, 0)), t1, 1000, 3000), 2000, "eased");
        assert_eq!(speed(Some((t0, 0)), t0, 1000, 7), 7, "too soon: as it was");
    }

    /// A row's number stays while its item downloads again (the next
    /// offer); ✕ finds the item by it.
    #[test]
    fn a_run_keeps_its_row() {
        let mut runs = Runs::default();
        let a = WishlistId::new();
        let b = WishlistId::new();
        let t1 = runs.reserve();
        runs.start(a, t1, "A".into(), "mugilaw".into(), Some("Soulseek".into()), None, None);
        let tb = runs.reserve();
        runs.start(b, tb, "B".into(), "Bandcamp".into(), Some("Bandcamp".into()), None, None);
        let ja = runs.get(a).unwrap().job;
        assert!(runs.get(b).unwrap().source.is_none(), "the source is not said twice");
        let t2 = runs.reserve();
        runs.start(a, t2, "A".into(), "rotted".into(), Some("Soulseek".into()), None, None);
        assert_eq!(runs.get(a).unwrap().job, ja);
        assert_eq!(runs.item_of(IMPORT_ID | DIRECT_JOB | ja), Some(a));
        runs.end(a);
        assert_eq!(runs.item_of(IMPORT_ID | DIRECT_JOB | ja), None);
    }

    /// What a run brings back counts only while it is its item's run: a
    /// save from YouTube that ends after ✕, or after the item started
    /// again, is dropped.
    #[test]
    fn a_stopped_or_replaced_run_brings_nothing_back() {
        let mut runs = Runs::default();
        let a = WishlistId::new();
        let first = runs.reserve();
        runs.start(a, first, "A".into(), "YouTube".into(), None, None, None);
        assert!(runs.is_current(a, first));
        // ✕: stopped.
        let (token, _) = runs.stop(a).unwrap();
        assert_eq!(token, first);
        assert!(!runs.is_current(a, first) && !runs.runs(a));
        assert!(runs.stop(a).is_none(), "nothing left to stop");
        // Started again: the first one's result is still not taken.
        let second = runs.reserve();
        runs.start(a, second, "A".into(), "YouTube".into(), None, None, None);
        assert!(runs.is_current(a, second) && !runs.is_current(a, first));
        assert!(runs.runs(a), "its folder stays for the new run");
    }
}
