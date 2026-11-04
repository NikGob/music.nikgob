//! «Загрузки → Раздача»: what the player gives away and what it costs.
//! Torrent numbers come from the poll the torrents get anyway (once a
//! second); Soulseek is asked on its own thread, once a second while the
//! page is in front of the user and once a minute otherwise (only to keep
//! the totals). How much was given away today and over all time is kept in
//! a small JSON file beside the torrent state, so it survives restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nikgob_soulseek::{DownloadState, Soulseek, Status, UploadInfo, UploadState};
use nikgob_torrent::TorrentStatus;
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use crate::Worker;
use crate::import::Internal;

const FILE_NAME: &str = "share-totals.json";
/// How often Soulseek is asked while «Раздача» is not looked at.
const SLSK_IDLE_POLL: Duration = Duration::from_secs(60);
/// How often changed totals are written.
const SAVE_EVERY: Duration = Duration::from_secs(60);
/// A look at Soulseek that has not come back by then is given up.
const SLSK_POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// What was given away, by kind, today and over all time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Totals {
    /// The local day `*_today` counts (`YYYY-MM-DD`).
    pub day: String,
    pub torrent_today: u64,
    pub slsk_today: u64,
    pub torrent_total: u64,
    pub slsk_total: u64,
    /// The uploads torrents had before these totals were kept were added
    /// once (to the overall total only).
    pub torrents_counted: bool,
}

impl Totals {
    /// Adds bytes given away on `day`; a new day starts its counts over.
    pub fn add(&mut self, day: &str, torrent: u64, slsk: u64) {
        if torrent == 0 && slsk == 0 {
            return;
        }
        self.roll(day);
        self.torrent_today += torrent;
        self.slsk_today += slsk;
        self.torrent_total += torrent;
        self.slsk_total += slsk;
    }

    /// Today's counts as of `day` (0 once the day the counts are for has
    /// passed).
    pub fn today(&self, day: &str) -> (u64, u64) {
        if self.day == day { (self.torrent_today, self.slsk_today) } else { (0, 0) }
    }

    fn roll(&mut self, day: &str) {
        if self.day != day {
            self.day = day.to_owned();
            self.torrent_today = 0;
            self.slsk_today = 0;
        }
    }

    fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

/// A byte counter read now and then that may start over from 0 (a client
/// restarted): how much it grew since the last look.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counter {
    last: Option<u64>,
}

impl Counter {
    /// A counter whose first reading counts from 0 (it started with us).
    pub fn from_zero() -> Self {
        Self { last: Some(0) }
    }

    pub fn delta(&mut self, now: u64) -> u64 {
        let d = match self.last {
            Some(l) if now >= l => now - l,
            Some(_) => now,
            None => 0,
        };
        self.last = Some(now);
        d
    }
}

/// Bytes the torrents uploaded since the last look, by info hash. A
/// torrent seen for the first time adds nothing: its lifetime counter was
/// counted by the runs before (or, with `first_run`, it is returned apart
/// as what came before the totals were kept).
pub(crate) fn torrent_delta(seen: &mut HashMap<String, u64>, list: &[TorrentStatus], first_run: bool) -> (u64, u64) {
    let mut delta = 0;
    let mut before = 0;
    let mut now = HashMap::with_capacity(list.len());
    for t in list {
        let last = seen.get(&t.info_hash).copied();
        match last {
            Some(last) => delta += t.uploaded.saturating_sub(last),
            None if first_run => before += t.uploaded,
            None => {}
        }
        now.insert(t.info_hash.clone(), t.uploaded.max(last.unwrap_or(0)));
    }
    *seen = now;
    (delta, before)
}

/// Torrent numbers from the last poll.
#[derive(Clone, Debug, Default, PartialEq)]
struct TorrentNow {
    up_bps: u64,
    down_bps: u64,
    peers: u32,
    disk: u64,
}

/// What a look at Soulseek brought back.
pub(crate) struct SlskPoll {
    generation: u64,
    remote: bool,
    status: Option<Status>,
    /// Asked only while the page is looked at.
    uploads: Option<Vec<UploadInfo>>,
    down_bps: Option<u64>,
}

/// The worker's side of «Раздача».
pub(crate) struct Sharing {
    path: Option<PathBuf>,
    totals: Totals,
    dirty: bool,
    saved_at: Instant,
    seen: HashMap<String, u64>,
    torrent: TorrentNow,
    /// The Soulseek client the counter belongs to.
    slsk_gen: u64,
    slsk_counter: Counter,
    slsk_sample: Option<(Instant, u64)>,
    slsk_up_bps: u64,
    slsk_down_bps: u64,
    slsk_status: Option<Status>,
    slsk_uploads: Vec<v::UploadRow>,
    /// The client a look is out to.
    slsk_polling: Option<u64>,
    slsk_polled: Option<Instant>,
    /// What settings.toml says about Soulseek.
    slsk_cfg: v::SlskSharingView,
    last: Option<v::SharingView>,
}

impl Sharing {
    /// Totals kept in `dir` (None: not kept).
    pub(crate) fn new(dir: Option<&Path>) -> Self {
        let path = dir.map(|d| d.join(FILE_NAME));
        let totals = path.as_deref().map(Totals::load).unwrap_or_default();
        Self {
            path,
            totals,
            dirty: false,
            saved_at: Instant::now(),
            seen: HashMap::new(),
            torrent: TorrentNow::default(),
            slsk_gen: 0,
            slsk_counter: Counter::default(),
            slsk_sample: None,
            slsk_up_bps: 0,
            slsk_down_bps: 0,
            slsk_status: None,
            slsk_uploads: Vec::new(),
            slsk_polling: None,
            slsk_polled: None,
            slsk_cfg: v::SlskSharingView::default(),
            last: None,
        }
    }

    fn add(&mut self, torrent: u64, slsk: u64) {
        if torrent > 0 || slsk > 0 {
            self.totals.add(&today(), torrent, slsk);
            self.dirty = true;
        }
    }

    /// Writes the totals when they changed (`force`: now, else at most
    /// once a minute).
    pub(crate) fn save(&mut self, force: bool) {
        if !self.dirty || (!force && self.saved_at.elapsed() < SAVE_EVERY) {
            return;
        }
        self.saved_at = Instant::now();
        self.dirty = false;
        if let Some(p) = &self.path
            && let Err(e) = self.totals.save(p)
        {
            log::warn!("cannot save the share totals: {e}");
        }
    }
}

/// The local calendar day.
fn today() -> String {
    jiff::Zoned::now().date().to_string()
}

/// A Soulseek upload as «Раздача» lists it.
pub(crate) fn upload_row(u: &UploadInfo) -> v::UploadRow {
    v::UploadRow {
        id: u.id,
        user: u.user.clone(),
        file: u.file_name().to_owned(),
        size: u.size,
        sent: u.sent,
        bytes_per_sec: u.bytes_per_sec,
        state: match u.state {
            UploadState::Queued => v::UploadRowState::Queued,
            UploadState::Sending => v::UploadRowState::Sending,
            UploadState::Done => v::UploadRowState::Done,
            UploadState::Failed { .. } => v::UploadRowState::Failed,
        },
        ended_at: (u.ended_at as i64).saturating_mul(1000),
    }
}

/// Upload speed from two readings of a byte counter.
pub(crate) fn rate(bytes: u64, over: Duration) -> u64 {
    let secs = over.as_secs_f64();
    if secs < 0.2 { 0 } else { (bytes as f64 / secs) as u64 }
}

impl Worker {
    /// From [`Worker::poll_torrents`]: what the torrents gave away since the
    /// last poll, their speeds, peers and disk.
    pub(crate) fn share_torrents(&mut self, list: &[TorrentStatus]) {
        let first_run = !self.sharing.totals.torrents_counted;
        let (delta, before) = torrent_delta(&mut self.sharing.seen, list, first_run);
        if first_run {
            self.sharing.totals.torrents_counted = true;
            self.sharing.totals.torrent_total += before;
            self.sharing.dirty = true;
        }
        self.sharing.add(delta, 0);
        self.sharing.torrent = TorrentNow {
            up_bps: list.iter().map(|t| t.up_bps).sum(),
            down_bps: list.iter().map(|t| t.down_bps).sum(),
            peers: list.iter().map(|t| t.peers).sum(),
            disk: list.iter().filter(|t| t.completed_at.is_some()).map(|t| t.total).sum(),
        };
    }

    /// No torrent client runs: nothing moves.
    pub(crate) fn share_no_torrents(&mut self) {
        self.sharing.torrent = TorrentNow::default();
    }

    /// Once a second: asks Soulseek when it is time, and refreshes
    /// «Раздача» while it is looked at.
    pub(crate) fn poll_share(&mut self) {
        let live = self.torrents.shown && self.window_visible();
        self.ask_soulseek(live);
        if live {
            self.publish_share();
        }
        self.sharing.save(false);
    }

    fn ask_soulseek(&mut self, live: bool) {
        let Some(slsk) = self.soulseek.source() else {
            let sh = &mut self.sharing;
            sh.slsk_polling = None;
            sh.slsk_status = None;
            sh.slsk_uploads.clear();
            sh.slsk_up_bps = 0;
            sh.slsk_down_bps = 0;
            sh.slsk_sample = None;
            return;
        };
        let generation = self.soulseek.generation();
        let since = self.sharing.slsk_polled.map(|t| t.elapsed());
        // One look at a time, unless it was for a client gone since or hangs.
        let out = self.sharing.slsk_polling == Some(generation) && since.is_some_and(|d| d < SLSK_POLL_TIMEOUT);
        let due = live || since.is_none_or(|d| d >= SLSK_IDLE_POLL);
        if out || !due {
            return;
        }
        self.sharing.slsk_polling = Some(generation);
        self.sharing.slsk_polled = Some(Instant::now());
        let remote = matches!(slsk, Soulseek::Remote(_));
        let tx = self.internal_tx.clone();
        let sent = self.soulseek.spawn(async move {
            let status = slsk.status().await.ok();
            let (uploads, down_bps) = if live {
                let uploads = slsk.uploads().await.ok();
                let down = slsk.downloads().await.ok().map(|d| {
                    d.iter()
                        .map(|d| match d.state {
                            DownloadState::Transferring { bytes_per_sec, .. } => bytes_per_sec,
                            _ => 0,
                        })
                        .sum()
                });
                (uploads, down)
            } else {
                (None, None)
            };
            let _ = tx.send(Internal::Sharing(SlskPoll { generation, remote, status, uploads, down_bps }));
        });
        if !sent {
            self.sharing.slsk_polling = None;
        }
    }

    /// A look at Soulseek came back.
    pub(crate) fn share_polled(&mut self, p: SlskPoll) {
        if p.generation != self.soulseek.generation() {
            return;
        }
        let sh = &mut self.sharing;
        if sh.slsk_polling == Some(p.generation) {
            sh.slsk_polling = None;
        }
        if p.generation != sh.slsk_gen {
            // Another client: the embedded one counts from its start, a
            // service has counted before we looked.
            sh.slsk_gen = p.generation;
            sh.slsk_counter = if p.remote { Counter::default() } else { Counter::from_zero() };
            sh.slsk_sample = None;
        }
        if let Some(st) = &p.status {
            let d = sh.slsk_counter.delta(st.uploaded_bytes);
            let now = Instant::now();
            sh.slsk_up_bps = match sh.slsk_sample {
                Some((at, _)) => rate(d, now - at),
                None => 0,
            };
            sh.slsk_sample = Some((now, st.uploaded_bytes));
            self.sharing.add(0, d);
        }
        let sh = &mut self.sharing;
        sh.slsk_status = p.status;
        if let Some(u) = p.uploads {
            sh.slsk_uploads = u.iter().map(upload_row).collect();
        }
        if let Some(d) = p.down_bps {
            sh.slsk_down_bps = d;
        }
    }

    /// Soulseek's settings for «Раздача» (from [`Worker::soulseek_apply`]).
    pub(crate) fn share_settings(&mut self, s: &nikgob_core::settings::SoulseekSettings, configured: bool) {
        self.sharing.slsk_cfg = v::SlskSharingView {
            configured,
            remote: s.mode == nikgob_core::settings::SoulseekMode::Remote,
            share: s.share,
            upload_limit_kbps: s.upload_limit_kbps,
            upload_slots: s.upload_slots.max(1),
            ..Default::default()
        };
        self.publish_share();
    }

    fn publish_share(&mut self) {
        let sh = &self.sharing;
        let (torrent_today, slsk_today) = sh.totals.today(&today());
        let running = self.soulseek.source().is_some();
        let status = sh.slsk_status.as_ref().filter(|_| running);
        let slsk = v::SlskSharingView {
            running,
            files: status.map_or(0, |s| s.shared_files),
            folders: status.map_or(0, |s| s.shared_dirs),
            uploads: if running { sh.slsk_uploads.clone() } else { Vec::new() },
            ..sh.slsk_cfg.clone()
        };
        let view = v::SharingView {
            torrent_up_bps: sh.torrent.up_bps,
            torrent_down_bps: sh.torrent.down_bps,
            slsk_up_bps: if running { sh.slsk_up_bps } else { 0 },
            slsk_down_bps: if running { sh.slsk_down_bps } else { 0 },
            torrent_today,
            slsk_today,
            torrent_total: sh.totals.torrent_total,
            slsk_total: sh.totals.slsk_total,
            seeding_disk_bytes: sh.torrent.disk,
            torrent_peers: sh.torrent.peers,
            slsk,
        };
        if self.sharing.last.as_ref() == Some(&view) {
            return;
        }
        self.sharing.last = Some(view.clone());
        self.publish(|s| s.downloads.sharing = view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torrent(hash: &str, uploaded: u64) -> TorrentStatus {
        TorrentStatus {
            id: 1,
            info_hash: hash.into(),
            name: hash.into(),
            state: nikgob_torrent::TorrentState::Seeding,
            progress: 1.0,
            down_bps: 0,
            up_bps: 0,
            downloaded: 0,
            uploaded,
            total: 0,
            eta: None,
            completed_at: None,
            selected_files: Vec::new(),
            peers: 0,
        }
    }

    #[test]
    fn totals_start_a_new_day_over() {
        let mut t = Totals::default();
        t.add("2026-11-04", 10, 5);
        t.add("2026-11-04", 1, 0);
        assert_eq!(t.today("2026-11-04"), (11, 5));
        assert_eq!(t.today("2026-11-05"), (0, 0));
        t.add("2026-11-05", 0, 2);
        assert_eq!(t.today("2026-11-05"), (0, 2));
        assert_eq!((t.torrent_total, t.slsk_total), (11, 7));
        // Nothing given does not move the day.
        t.add("2026-11-06", 0, 0);
        assert_eq!(t.day, "2026-11-05");
    }

    #[test]
    fn totals_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut t = Totals::default();
        t.add("2026-11-04", 3, 4);
        t.torrents_counted = true;
        t.save(&path).unwrap();
        assert_eq!(Totals::load(&path), t);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(Totals::load(&path), Totals::default());
    }

    #[test]
    fn counters_survive_a_restart_of_the_client() {
        let mut c = Counter::from_zero();
        assert_eq!(c.delta(100), 100);
        assert_eq!(c.delta(150), 50);
        // The client started over.
        assert_eq!(c.delta(20), 20);
        let mut service = Counter::default();
        assert_eq!(service.delta(5_000), 0, "a service counted before we looked");
        assert_eq!(service.delta(5_010), 10);
    }

    #[test]
    fn torrents_count_only_what_they_uploaded_while_seen() {
        let mut seen = HashMap::new();
        let (d, before) = torrent_delta(&mut seen, &[torrent("a", 100)], true);
        assert_eq!((d, before), (0, 100));
        let (d, before) = torrent_delta(&mut seen, &[torrent("a", 130), torrent("b", 40)], false);
        assert_eq!((d, before), (30, 0), "b was counted by the run before");
        // `a` stopped seeding (gone), `b` went on.
        let (d, _) = torrent_delta(&mut seen, &[torrent("b", 45)], false);
        assert_eq!(d, 5);
        assert!(!seen.contains_key("a"));
        // A counter that went back adds nothing.
        let (d, _) = torrent_delta(&mut seen, &[torrent("b", 10)], false);
        assert_eq!(d, 0);
        assert_eq!(seen["b"], 45);
    }

    #[test]
    fn speed_from_two_readings() {
        assert_eq!(rate(1000, Duration::from_secs(2)), 500);
        assert_eq!(rate(1000, Duration::from_millis(10)), 0);
    }

    #[test]
    fn uploads_become_rows() {
        let u = UploadInfo {
            id: 7,
            user: "peer".into(),
            path: "@@music\\A\\B\\01 x.flac".into(),
            size: 10,
            sent: 5,
            bytes_per_sec: 3,
            state: UploadState::Failed { reason: "Cancelled".into() },
            ended_at: 2,
        };
        let r = upload_row(&u);
        assert_eq!(r.file, "01 x.flac");
        assert_eq!(r.state, v::UploadRowState::Failed);
        assert_eq!(r.ended_at, 2000);
    }
}
