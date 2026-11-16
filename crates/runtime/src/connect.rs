//! Playing on another device (like Spotify Connect), as the loop keeps it.
//!
//! The window's target is this device's zone or another device's. While it
//! is another's, the player's commands go there ([`Worker::user_command`]),
//! the view's player and queue mirror that zone (its event stream, through
//! the mesh service: [`ConnectReport::State`]), and this zone is parked:
//! paused, its queue kept, shown as [`nikgob_view::ParkedView`]. The link's
//! state comes from the service too: buffering after 1.5 s without a word,
//! «Нет связи» after 3 s, and back by itself; sound never starts here by
//! itself.
//!
//! A transfer is done by the service with the API's snapshot and load
//! ([`ConnectJob::Move`]); the loop answers those two requests itself
//! ([`Worker::zone_snapshot`], [`Worker::zone_load`]). A zone given tracks
//! it has no file of fetches each from the device that sent them into a
//! cache of its own ([`ConnectJob::Pull`]), waiting for it as for a stream.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{Codec, EventContext, EventOrigin, Source, SourceId, SourceKind, TrackId};
use nikgob_view::{
    Command, ConnectJob, ConnectReport, MeshKind, ParkedView, RemoteCommand, RemoteLink, RemoteTarget, Repeat, Status, Toast, ToastKind,
    ZoneEntry, ZoneLoaded, ZoneRepeat, ZoneSnapshot, ZoneState, ZoneStatus, ZoneTrack,
};

use crate::Worker;
use crate::queue::{self, Entry};

/// How long «Отменить» of a transfer onto a playing device works (its
/// snackbar stays 8 s).
const UNDO_FOR: Duration = Duration::from_secs(9);
/// «Продолжить здесь» starts this much before where the remote was.
const CONTINUE_BACK: Duration = Duration::from_millis(1500);
/// After a volume set here, the remote's reports of its volume (perhaps
/// sent before it heard of the set) are not shown for this long.
const VOLUME_HOLD: Duration = Duration::from_millis(800);
/// A transfer not answered in this long no longer holds the next one.
const PENDING_FOR: Duration = Duration::from_secs(20);
/// Files of the next tracks fetched ahead.
const PULL_AHEAD: usize = 2;

/// Where the loop hands the network part to the mesh service.
pub(crate) type ConnectLink = Arc<dyn Fn(ConnectJob) + Send + Sync>;

struct Target {
    node: String,
    name: String,
    kind: MeshKind,
    link: RemoteLink,
}

/// The target's last state and when it was (this device's clock).
struct Mirror {
    state: ZoneState,
    at: Instant,
}

/// A transfer waiting for its answer.
struct Pending {
    seq: u64,
    from: Option<String>,
    at: Instant,
    /// What the music came from was playing (an undo plays it again).
    was_playing: bool,
}

/// «Отменить» of a transfer onto a device that was playing.
struct Undo {
    node: String,
    previous: ZoneSnapshot,
    until: Instant,
    /// The target before the transfer (None: this device).
    before: Option<String>,
    before_playing: bool,
}

/// An undo sent, waiting for its answer.
struct Undoing {
    seq: u64,
    before: Option<String>,
    before_playing: bool,
}

/// What the loop keeps of playing elsewhere.
#[derive(Default)]
pub(crate) struct ConnectKeep {
    pub(crate) link: Option<ConnectLink>,
    target: Option<Target>,
    mirror: Option<Mirror>,
    /// The rows of the mirrored tracks (this library's), and the ids they
    /// were made for.
    rows: HashMap<TrackId, nikgob_view::TrackRow>,
    row_ids: Vec<TrackId>,
    /// The track whose lyrics and info the mirror shows.
    shown: Option<TrackId>,
    /// Where the display stopped while the link is not live.
    frozen: Option<Duration>,
    seq: u64,
    pending: Option<Pending>,
    undo: Option<Undo>,
    undoing: Option<Undoing>,
    lost_toast: Option<u64>,
    volume_hold: Option<Instant>,
    /// The device files are fetched from: the last one that loaded or drove
    /// this zone.
    pull_from: Option<String>,
    /// Files another device sent for tracks this library has no file of.
    files: HashMap<TrackId, PathBuf>,
    pulling: HashSet<TrackId>,
    /// Tracks whose file could not be had (until the next load).
    unpullable: HashSet<TrackId>,
}

impl ConnectKeep {
    /// The window shows another device's zone.
    pub(crate) fn remote(&self) -> bool {
        self.target.is_some()
    }
}

/// The places of `s` as the queue keeps them.
fn queue_snapshot(s: &ZoneSnapshot) -> queue::Snapshot {
    queue::Snapshot {
        items: s.tracks.clone(),
        original: s.original.clone(),
        pos: s.index.filter(|&i| i < s.tracks.len()),
        shuffle: s.shuffle,
        repeat: match s.repeat {
            ZoneRepeat::Off => 0,
            ZoneRepeat::All => 1,
            ZoneRepeat::One => 2,
        },
        context: s.context.clone(),
        kind: s.kind,
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Worker {
    /// Hands `job` to the mesh service; false when there is none.
    fn connect_send(&self, job: ConnectJob) -> bool {
        match self.connect.link.clone() {
            Some(f) => {
                f(job);
                true
            }
            None => false,
        }
    }

    /// A command of the window or the API: while the target is another
    /// device, the player's commands go there.
    pub(crate) fn user_command(&mut self, c: Command) {
        if let Some(c) = self.connect_intercept(c) {
            self.command(c);
        }
    }

    /// `c` to run here, or None when it went to the target (or cannot be
    /// done there).
    fn connect_intercept(&mut self, c: Command) -> Option<Command> {
        let Some((node, name)) = self.connect.target.as_ref().map(|t| (t.node.clone(), t.name.clone())) else {
            return Some(c);
        };
        if let Some(rc) = RemoteCommand::of(&c) {
            self.connect_optimistic(&rc);
            if !self.connect_send(ConnectJob::Run { node, commands: vec![rc] }) {
                log::warn!("connect: a command for the target dropped: no mesh service");
            }
            return None;
        }
        if nikgob_view::starts_service_music(&c) {
            self.toast(format!("На {name} играют только песни из библиотеки"), None);
            return None;
        }
        Some(c)
    }

    /// The volume and the mute show at once (the wheel), before the target
    /// says them.
    fn connect_optimistic(&mut self, rc: &RemoteCommand) {
        let (now, muted) = {
            let st = self.state.read();
            (st.player.volume, st.player.muted)
        };
        let (volume, muted) = match rc {
            RemoteCommand::SetVolume { volume } => (volume.clamp(0.0, 1.0), false),
            RemoteCommand::ToggleMute => (now, !muted),
            _ => return,
        };
        self.connect.volume_hold = Some(Instant::now() + VOLUME_HOLD);
        self.publish(move |s| {
            s.player.volume = volume;
            s.player.muted = muted;
            if let Some(t) = s.target.as_mut() {
                t.volume = volume;
                t.muted = muted;
            }
        });
    }

    pub(crate) fn connect_command(&mut self, c: Command) {
        match c {
            Command::OutputTransfer { node_id } => self.connect_transfer(node_id),
            Command::OutputAttach { node_id } => self.connect_attach(node_id),
            Command::OutputDetach => self.connect_detach(),
            Command::OutputContinueHere => self.connect_continue_here(),
            Command::OutputUndoTransfer => self.connect_undo(),
            Command::ApiSnapshot { reply } => {
                let s = self.zone_snapshot();
                reply.send(serde_json::to_value(s).map_err(|e| e.to_string()));
            }
            Command::ApiLoad { snapshot, from, reply } => {
                let previous = self.zone_load(*snapshot, from);
                reply.send(Ok(serde_json::json!({ "previous": previous })));
            }
            Command::ConnectRun { from, commands } => {
                if from.is_some() {
                    self.connect.pull_from = from;
                }
                // Run here as they are, never passed on again.
                for c in commands {
                    self.command(c);
                }
            }
            Command::ConnectReport(r) => self.connect_report(*r),
            other => log::warn!("connect: not a command of playing elsewhere {other:?}"),
        }
    }

    /// A paired device's name, kind and whether it is online.
    fn node_view(&self, id: &str) -> Option<(String, MeshKind, bool)> {
        let st = self.state.read();
        st.mesh.node(id).map(|n| (n.name.clone(), n.kind, n.online))
    }

    /// The music the target plays (as last mirrored).
    fn mirror_playing(&self) -> bool {
        self.connect.mirror.as_ref().is_some_and(|m| m.state.status == ZoneStatus::Playing)
    }

    fn connect_transfer(&mut self, to: Option<String>) {
        if self.connect.pending.as_ref().is_some_and(|p| p.at.elapsed() < PENDING_FOR) {
            log::info!("connect: a transfer is on its way, another waits for it");
            return;
        }
        let from = self.connect.target.as_ref().map(|t| t.node.clone());
        match to {
            Some(node) => {
                if from.as_deref() == Some(node.as_str()) {
                    return;
                }
                let Some((name, _, online)) = self.node_view(&node) else { return };
                if !online {
                    self.toast(format!("{name} не в сети"), None);
                    return;
                }
                // Nothing here to move: that device is the target as it is.
                if from.is_none() && self.queue.current().is_none() {
                    self.connect_attach(node);
                    return;
                }
                let was_playing = match from {
                    None => matches!(self.status, Status::Playing | Status::Buffering),
                    Some(_) => self.mirror_playing(),
                };
                self.connect_move(from, Some(node), was_playing);
            }
            // Here already.
            None if from.is_none() => {}
            None => self.connect_move(from, None, false),
        }
    }

    fn connect_move(&mut self, from: Option<String>, to: Option<String>, was_playing: bool) {
        self.connect.seq += 1;
        let seq = self.connect.seq;
        if !self.connect_send(ConnectJob::Move { seq, from: from.clone(), to }) {
            self.toast("Межсеть не запущена", None);
            return;
        }
        self.connect.pending = Some(Pending { seq, from, at: Instant::now(), was_playing });
    }

    fn connect_attach(&mut self, node: String) {
        if self.connect.target.as_ref().is_some_and(|t| t.node == node) {
            return;
        }
        let Some((name, kind, online)) = self.node_view(&node) else { return };
        if !online {
            self.toast(format!("{name} не в сети"), None);
            return;
        }
        self.connect_target(node, name, kind);
    }

    /// What this zone holds now, for the parked view.
    fn parked_now(&mut self) -> ParkedView {
        let shown = self.playing.or_else(|| self.resume_at.and(self.queue.current()));
        let row = shown.and_then(|e| self.lib.get_track_row(e.track).ok().flatten());
        let current = row.map(|r| self.res.track(&r));
        ParkedView { current, position: self.position_now() }
    }

    /// While the window drives another device: this zone into the parked
    /// view (the player shows the remote's).
    pub(crate) fn publish_parked(&mut self) {
        let parked = self.parked_now();
        if self.state.read().parked.as_ref() == Some(&parked) {
            return;
        }
        self.publish(move |s| s.parked = Some(parked));
    }

    /// `node` becomes the target: this zone parks (paused), the window
    /// follows that device's zone.
    fn connect_target(&mut self, node: String, name: String, kind: MeshKind) {
        let first = self.connect.target.is_none();
        if first && matches!(self.status, Status::Playing | Status::Buffering) {
            self.pause();
        }
        let parked = first.then(|| self.parked_now());
        self.connect.target = Some(Target { node: node.clone(), name: name.clone(), kind, link: RemoteLink::Live });
        self.connect.mirror = None;
        self.connect.rows.clear();
        self.connect.row_ids.clear();
        self.connect.shown = None;
        self.connect.frozen = None;
        self.connect_drop_lost_toast();
        let follow = node.clone();
        self.publish(move |s| {
            let (volume, muted) = (s.player.volume, s.player.muted);
            s.target = Some(RemoteTarget { node_id: node, name, kind, link: RemoteLink::Live, volume, muted });
            if let Some(p) = parked {
                s.parked = Some(p);
            }
            // Until its first state: buffering where it is.
            s.player.position = s.player.position_now();
            s.player.at = Instant::now();
            s.player.status = Status::Buffering;
            s.player.rate = 0.0;
        });
        if !self.connect_send(ConnectJob::Follow { node: follow }) {
            log::warn!("connect: no mesh service to follow the target");
        }
    }

    fn connect_detach(&mut self) {
        if self.connect.target.is_none() {
            return;
        }
        self.connect_leave();
        self.connect_show_local();
    }

    /// The target is this device again (the mirror forgotten; the view's
    /// player is put back by [`Self::connect_show_local`] or a load).
    fn connect_leave(&mut self) {
        self.connect.target = None;
        self.connect.mirror = None;
        self.connect.rows.clear();
        self.connect.row_ids.clear();
        self.connect.shown = None;
        self.connect.frozen = None;
        self.connect.volume_hold = None;
        self.connect_send(ConnectJob::Unfollow);
        let lost = self.connect.lost_toast.take();
        self.publish(move |s| {
            s.target = None;
            s.parked = None;
            if let Some(id) = lost {
                s.toasts.retain(|t| t.id != id);
            }
        });
    }

    /// This zone back in the view: its track (lyrics and info), queue,
    /// volume, duck and timer.
    fn connect_show_local(&mut self) {
        self.publish_now_playing();
        self.publish_volume();
        let duck = self.duck.view();
        self.publish(move |s| s.player.duck = duck);
        self.sleep_publish();
    }

    fn connect_continue_here(&mut self) {
        let Some(node) = self.connect.target.as_ref().map(|t| t.node.clone()) else { return };
        let snapshot = self.connect.mirror.as_ref().map(|m| {
            let at = self.state.read().player.position_now();
            m.state.snapshot(ms(at.saturating_sub(CONTINUE_BACK)))
        });
        // The remote stops if it still hears (no double sound once it is back).
        self.connect_send(ConnectJob::Run { node: node.clone(), commands: vec![RemoteCommand::Pause] });
        self.connect_leave();
        match snapshot {
            Some(s) if s.index.is_some() => {
                self.zone_load(s, Some(node));
            }
            _ => self.connect_show_local(),
        }
    }

    fn connect_undo(&mut self) {
        let Some(u) = self.connect.undo.take() else { return };
        if Instant::now() > u.until {
            return;
        }
        self.connect.seq += 1;
        let seq = self.connect.seq;
        if self.connect_send(ConnectJob::Load { seq, node: u.node, snapshot: Box::new(u.previous) }) {
            self.connect.undoing = Some(Undoing { seq, before: u.before, before_playing: u.before_playing });
        }
    }

    fn connect_report(&mut self, r: ConnectReport) {
        match r {
            ConnectReport::State { node, state, at } => self.connect_state(&node, *state, at),
            ConnectReport::Link { node, link } => self.connect_link(&node, link),
            ConnectReport::Moved { seq, to, result } => self.connect_moved(seq, to, result),
            ConnectReport::Loaded { seq, result, .. } => self.connect_undone(seq, result),
            ConnectReport::Pulled { track, result } => self.connect_pulled(track, result),
            ConnectReport::RunFailed { node, why } => {
                let short: String = node.chars().take(10).collect();
                log::info!("connect: commands for {short}… did not get there: {why}");
            }
        }
    }

    fn connect_moved(&mut self, seq: u64, to: Option<String>, result: Result<ZoneLoaded, String>) {
        let Some(p) = self.connect.pending.take_if(|p| p.seq == seq) else { return };
        match (result, to) {
            (Err(why), Some(node)) => {
                let name = self.node_view(&node).map(|n| n.0).unwrap_or_default();
                log::warn!("connect: the transfer to {name} failed: {why}");
                self.toast(format!("Не удалось перенести на {name}: {why}"), None);
            }
            (Err(why), None) => {
                log::warn!("connect: the music did not come back here: {why}");
                self.toast(format!("Не удалось вернуть музыку сюда: {why}"), None);
            }
            (Ok(answer), Some(node)) => {
                let Some((name, kind, _)) = self.node_view(&node) else { return };
                // What moved stops here (the service paused a remote it left).
                if p.from.is_none() && matches!(self.status, Status::Playing | Status::Buffering) {
                    self.pause();
                }
                self.connect_target(node.clone(), name.clone(), kind);
                // Onto a device that was playing: its music can come back.
                if let Some(previous) = answer.previous.filter(|prev| !prev.paused && prev.index.is_some()) {
                    self.connect.undo = Some(Undo { node, previous, until: Instant::now() + UNDO_FOR, before: p.from, before_playing: p.was_playing });
                    self.connect_toast_long(format!("Очередь перенесена на {name}"), Some(("Отменить".to_owned(), Command::OutputUndoTransfer)));
                }
            }
            (Ok(_), None) => {
                // The load here made this device the target already.
                if self.connect.target.is_some() {
                    self.connect_leave();
                    self.connect_show_local();
                }
            }
        }
    }

    fn connect_undone(&mut self, seq: u64, result: Result<ZoneLoaded, String>) {
        let Some(u) = self.connect.undoing.take_if(|u| u.seq == seq) else { return };
        if let Err(why) = result {
            log::warn!("connect: the undo failed: {why}");
            self.toast(format!("Не удалось отменить: {why}"), None);
            return;
        }
        match u.before {
            None => {
                self.connect_detach();
                if u.before_playing {
                    self.resume();
                }
            }
            Some(before) => {
                let Some((name, kind, _)) = self.node_view(&before) else { return };
                self.connect_target(before.clone(), name, kind);
                if u.before_playing {
                    self.connect_send(ConnectJob::Run { node: before, commands: vec![RemoteCommand::Play] });
                }
            }
        }
    }

    /// A toast that stays 8 s; its id.
    fn connect_toast_long(&mut self, text: String, action: Option<(String, Command)>) -> u64 {
        self.toast_id += 1;
        let id = self.toast_id;
        let t = Toast { id, kind: ToastKind::guess(&text), text, action, open: None, at: Instant::now(), long: true, again: 0 };
        self.publish(move |s| s.toasts.push(t));
        id
    }

    fn connect_drop_lost_toast(&mut self) {
        if let Some(id) = self.connect.lost_toast.take() {
            self.publish(move |s| s.toasts.retain(|t| t.id != id));
        }
    }

    /// The target's state: into the view's player and queue (its tracks as
    /// this library has them).
    fn connect_state(&mut self, node: &str, state: ZoneState, at: Instant) {
        let Some(t) = self.connect.target.as_mut().filter(|t| t.node == node) else { return };
        let was = t.link;
        t.link = RemoteLink::Live;
        self.connect.frozen = None;
        if was != RemoteLink::Live {
            self.connect_drop_lost_toast();
        }
        let ids = state.track_ids();
        if ids != self.connect.row_ids {
            let found = self.lib.get_track_rows(&ids).unwrap_or_default();
            let mut rows = HashMap::new();
            for id in &ids {
                if let Some(r) = found.get(id)
                    && !rows.contains_key(id)
                {
                    rows.insert(*id, self.res.track(r));
                }
            }
            self.connect.rows = rows;
            self.connect.row_ids = ids;
        }
        self.connect.mirror = Some(Mirror { state, at });
        self.connect_publish_mirror();
    }

    fn connect_link(&mut self, node: &str, link: RemoteLink) {
        let Some(t) = self.connect.target.as_mut().filter(|t| t.node == node) else { return };
        if t.link == link {
            return;
        }
        t.link = link;
        let name = t.name.clone();
        match link {
            RemoteLink::Live => {
                self.connect.frozen = None;
                self.connect_drop_lost_toast();
            }
            RemoteLink::Reconnecting => {}
            RemoteLink::Lost => {
                if self.connect.lost_toast.is_none() {
                    let id = self.connect_toast_long(format!("Нет связи с {name}"), Some(("Продолжить здесь".to_owned(), Command::OutputContinueHere)));
                    self.connect.lost_toast = Some(id);
                }
            }
        }
        if self.connect.mirror.is_some() {
            self.connect_publish_mirror();
        } else {
            self.publish(move |s| {
                if let Some(tv) = s.target.as_mut() {
                    tv.link = link;
                }
            });
        }
    }

    /// The mirror into the view: live, it runs from the state's clock;
    /// otherwise it stands where it got to (buffering, then paused).
    fn connect_publish_mirror(&mut self) {
        let (Some(m), Some(t)) = (self.connect.mirror.as_ref(), self.connect.target.as_ref()) else { return };
        let now = Instant::now();
        let st = &m.state;
        let rows = &self.connect.rows;
        let row_of = |z: &ZoneTrack| rows.get(&z.id).cloned().unwrap_or_else(|| z.row());
        let entry = |e: &ZoneEntry| nikgob_view::QueueEntry { entry: e.entry, track: row_of(&e.track), why: None, guest: false };
        let current = st.current.as_ref().map(|z| row_of(z));
        let prev: Vec<nikgob_view::QueueEntry> = st.prev.iter().map(entry).collect();
        let next: Vec<nikgob_view::QueueEntry> = st.next.iter().map(entry).collect();
        let duration = st.duration_ms.map(Duration::from_millis).or_else(|| current.as_ref().and_then(|c| c.duration));
        let link = t.link;
        let anchor = Duration::from_millis(st.position_ms);
        let (status, position, at, rate) = if link == RemoteLink::Live {
            (st.status.status(), anchor, m.at, st.rate.max(0.0))
        } else {
            let ran = if st.status == ZoneStatus::Playing { now.saturating_duration_since(m.at).mul_f32(st.rate.max(0.0)) } else { Duration::ZERO };
            let stood = self.connect.frozen.unwrap_or_else(|| duration.map_or(anchor + ran, |d| (anchor + ran).min(d)));
            let status = if link == RemoteLink::Lost { Status::Paused } else { Status::Buffering };
            (status, stood, now, 0.0)
        };
        let speed = if st.speed > 0.0 { st.speed } else { 1.0 };
        let (volume, muted) = (st.volume.clamp(0.0, 1.0), st.muted);
        let (shuffle, repeat): (bool, Repeat) = (st.shuffle, st.repeat.into());
        let context = st.context.clone();
        let page = st.page.as_deref().and_then(nikgob_view::page_route);
        let duck = st.duck;
        let radio = st.radio.as_ref().map(|r| nikgob_view::RadioView {
            title: r.title.clone(),
            extending: r.extending,
            autoplay: r.autoplay,
            ..Default::default()
        });
        let current_entry = st.current_entry;
        let current_id = current.as_ref().map(|c| c.id);
        let target = RemoteTarget { node_id: t.node.clone(), name: t.name.clone(), kind: t.kind, link, volume, muted };
        if link != RemoteLink::Live {
            self.connect.frozen = Some(position);
        }
        let hold = self.connect.volume_hold.is_some_and(|h| now < h);
        self.publish(move |s| {
            let p = &mut s.player;
            p.status = status;
            p.current = current.clone();
            p.position = position;
            p.at = at;
            p.rate = rate;
            p.speed = speed;
            p.duration = duration;
            if !hold {
                p.volume = volume;
                p.muted = muted;
            }
            p.shuffle = shuffle;
            p.repeat = repeat;
            p.context = context;
            p.context_page = page;
            p.audio_service = None;
            p.audio_quality = None;
            p.soloist = false;
            p.sleep = None;
            p.duck = duck;
            let mut target = target;
            if hold {
                target.volume = p.volume;
                target.muted = p.muted;
            }
            s.target = Some(target);
            let q = &mut s.queue;
            q.current = current;
            q.current_entry = current_entry;
            q.current_why = None;
            q.current_guest = false;
            q.prev = prev;
            q.next = next;
            q.radio = radio;
        });
        if current_id != self.connect.shown {
            self.connect.shown = current_id;
            self.connect_show_track(current_id);
        }
    }

    /// The lyrics and the info of the target's track, when this library has
    /// it (none otherwise).
    fn connect_show_track(&mut self, track: Option<TrackId>) {
        let row = track.and_then(|t| self.lib.get_track_row(t).ok().flatten());
        match (track, row) {
            (Some(t), Some(row)) => {
                let src = self.lib.active_source(t).ok().flatten();
                let info = self.res.info(Some(&row), src.as_ref());
                self.publish(move |s| s.info = info);
                self.track_details(t);
            }
            _ => self.publish(|s| {
                s.info = nikgob_view::TrackInfo::default();
                s.lyrics = nikgob_view::LyricsView::default();
            }),
        }
    }

    // ------------------------------------------------------------ this zone

    /// This zone as a snapshot to play elsewhere (`GET /v1/player/snapshot`;
    /// the API adds the tracks with a file of the library).
    pub(crate) fn zone_snapshot(&mut self) -> ZoneSnapshot {
        let q = self.queue.snapshot();
        let position = if self.queue.current().is_some() { self.position_now() } else { Duration::ZERO };
        let radio = self.radio_save().and_then(|r| serde_json::to_value(r).ok());
        let page = self.context_page_id().map(str::to_owned);
        let mut files: Vec<TrackId> = Vec::new();
        for t in &q.items {
            if self.connect.files.contains_key(t) && !files.contains(t) {
                files.push(*t);
            }
        }
        ZoneSnapshot {
            tracks: q.items,
            original: q.original,
            index: q.pos,
            position_ms: ms(position),
            paused: !matches!(self.status, Status::Playing | Status::Buffering),
            speed: self.speed as f32,
            shuffle: q.shuffle,
            repeat: match q.repeat {
                1 => ZoneRepeat::All,
                2 => ZoneRepeat::One,
                _ => ZoneRepeat::Off,
            },
            context: q.context,
            page,
            kind: q.kind,
            radio,
            autoplay_at: self.autoplay_save(),
            files,
        }
    }

    /// `POST /v1/player/load`: this zone's queue becomes `snap` (its tracks
    /// checked by the API), playing from its place unless it says paused;
    /// `from` is the device that sent it (files missing here come from
    /// it). What the zone held before, for an undo.
    pub(crate) fn zone_load(&mut self, snap: ZoneSnapshot, from: Option<String>) -> Option<ZoneSnapshot> {
        // The music plays here now: the window shows this device.
        if self.connect.target.is_some() {
            self.connect_leave();
        }
        let previous = (!self.queue.is_empty()).then(|| self.zone_snapshot());
        if from.is_some() {
            self.connect.pull_from = from;
        }
        self.connect.unpullable.clear();
        self.user_switch(EventOrigin::Manual);
        self.playback.stop();
        self.playing = None;
        self.playing_source = None;
        self.failover = None;
        self.resume_at = None;
        self.queue.restore(&queue_snapshot(&snap));
        self.play_context = EventContext::Other;
        self.play_context_id = snap.page.clone();
        self.restored_page = None;
        self.autoplay_restore(&snap.autoplay_at);
        if let Some(radio) = snap.radio.clone().and_then(|v| serde_json::from_value::<crate::recs::RadioSave>(v).ok()) {
            self.radio_restore(radio);
        }
        if self.queue.current().is_some() {
            let at = Duration::from_millis(snap.position_ms);
            self.resume_at = Some(at);
            self.position = at;
            self.position_at = Instant::now();
            self.status = Status::Paused;
            if snap.paused {
                self.publish_now_playing();
            } else {
                self.start_restored();
            }
        } else {
            self.status = Status::Stopped;
            self.position = Duration::ZERO;
            self.publish_now_playing();
        }
        self.connect_prefetch();
        log::info!("connect: a queue of {} tracks loaded here{}", snap.tracks.len(), if snap.paused { ", paused" } else { "" });
        previous
    }

    // ------------------------------------------------------------ files

    /// A file another device sent for `track` as a source to play; its id
    /// is the track's, so one that failed is skipped once tried. A file
    /// gone from the cache is forgotten (and fetched again when it plays).
    pub(crate) fn connect_source(&mut self, track: TrackId, skip: &[SourceId]) -> Option<Source> {
        let path = self.connect.files.get(&track)?.clone();
        let id = SourceId::from_uuid(*track.as_uuid());
        if skip.contains(&id) {
            return None;
        }
        if !path.is_file() {
            self.connect.files.remove(&track);
            return None;
        }
        let mut s = Source::new(track, SourceKind::LocalFile, path.to_string_lossy().into_owned(), Codec::Other(String::new()));
        s.id = id;
        Some(s)
    }

    /// Whether `e` waits for its file from another device (asked for now if
    /// it was not): the player shows it buffering meanwhile, as a stream
    /// being looked up.
    pub(crate) fn wait_for_pull(&mut self, e: Entry) -> bool {
        if !self.connect_pull(e.track) {
            return false;
        }
        self.playback.stop();
        self.playing = Some(e);
        self.playing_source = None;
        self.restarted = None;
        self.status = Status::Buffering;
        self.position = self.failover.filter(|(id, _)| *id == e.id).map_or(Duration::ZERO, |(_, at)| at);
        self.position_at = Instant::now();
        self.rate = 1.0;
        self.publish_now_playing();
        true
    }

    /// Asks for `track`'s file from the device that drives this zone,
    /// unless one is kept, on its way, or could not be had. Whether it is
    /// on its way.
    fn connect_pull(&mut self, track: TrackId) -> bool {
        if self.connect.files.contains_key(&track) || self.connect.unpullable.contains(&track) {
            return false;
        }
        if self.connect.pulling.contains(&track) {
            return true;
        }
        let Some(from) = self.connect.pull_from.clone() else { return false };
        if !self.connect_send(ConnectJob::Pull { track, from }) {
            return false;
        }
        self.connect.pulling.insert(track);
        true
    }

    /// The files of the next tracks fetched ahead (while this zone plays
    /// what another device sent).
    pub(crate) fn connect_prefetch(&mut self) {
        if self.connect.pull_from.is_none() {
            return;
        }
        for e in self.queue.ahead(PULL_AHEAD) {
            let t = e.track;
            if self.connect.files.contains_key(&t) || self.connect.pulling.contains(&t) || self.connect.unpullable.contains(&t) {
                continue;
            }
            let local = self
                .lib
                .sources_of_track(t)
                .unwrap_or_default()
                .iter()
                .any(|s| s.kind == SourceKind::LocalFile && s.available && s.trashed_at.is_none());
            if !local {
                self.connect_pull(t);
            }
        }
    }

    fn connect_pulled(&mut self, track: TrackId, result: Result<PathBuf, String>) {
        self.connect.pulling.remove(&track);
        match result {
            Ok(path) => {
                self.connect.files.insert(track, path);
            }
            Err(why) => {
                log::warn!("connect: no file of {track} from the other device: {why}");
                self.connect.unpullable.insert(track);
            }
        }
        let have = self.connect.files.contains_key(&track);
        let waiting = self.playing.filter(|e| e.track == track && self.playing_source.is_none());
        if let Some(e) = waiting {
            let paused = self.status == Status::Paused;
            match have.then(|| self.request_skipping(e, &[])).flatten() {
                Some((req, source, kind)) => {
                    // (A place it is to go on from: `start_with` seeks there.)
                    self.start_with(e, req, source, kind);
                    if paused {
                        self.playback.pause();
                        self.status = Status::Paused;
                        self.publish_player();
                    }
                }
                None => match self.queue.skip_forward() {
                    Some(n) if n.id != e.id && paused => self.start_kept_paused(n),
                    Some(n) if n.id != e.id => self.start(n),
                    _ => self.stop(),
                },
            }
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track) {
            self.preload_next();
        }
        self.connect_prefetch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_becomes_the_queues_own() {
        let t: Vec<TrackId> = (0..3).map(|_| TrackId::new()).collect();
        let s = ZoneSnapshot { tracks: t.clone(), original: vec![2, 1, 0], index: Some(5), repeat: ZoneRepeat::One, ..ZoneSnapshot::default() };
        let q = queue_snapshot(&s);
        assert_eq!(q.items, t);
        assert_eq!(q.pos, None, "a place past the end is none");
        assert_eq!(q.repeat, 2);
        let mut back = crate::queue::Queue::default();
        back.restore(&queue_snapshot(&ZoneSnapshot { index: Some(1), ..s }));
        assert_eq!(back.current().map(|e| e.track), Some(t[1]));
        assert_eq!(back.repeat, Repeat::One);
    }
}
