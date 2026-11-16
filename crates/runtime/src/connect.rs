//! Playing on another device (like Spotify Connect), as the loop keeps it.
//!
//! The window's target is this device's zone or another device's. While it
//! is another's, the player's commands go there ([`Worker::user_command`]),
//! the view's player and queue mirror that zone (its event stream, through
//! the mesh service: [`ConnectReport::State`]), and this zone is parked:
//! paused, its queue kept, shown as [`nikgob_view::ParkedView`]. The link's
//! state comes from the service too: buffering after 1.5 s without a word,
//! «Нет связи» after 3 s, and back by itself.
//!
//! Each target is a session with a generation of its own: what is sent for
//! a session (forwarded commands, a pause) carries it, and the service
//! drops work of a session that has ended; reports of an ended session are
//! ignored here. Transfers, attaches, detaches, undos and «Продолжить
//! здесь» go one at a time: one asked while another runs waits for it.
//!
//! The one rule: nothing here starts sound by itself to make up for
//! something. When it is not known what happened, things stay paused and
//! the owner acts.
//!
//! A transfer is done by the service with the API's snapshot and load
//! ([`ConnectJob::Move`]); the loop answers those two requests itself
//! ([`Worker::zone_snapshot`], [`Worker::zone_load`]). A zone given tracks
//! it has no file of fetches each from the device that sent them into a
//! cache of its own ([`ConnectJob::Pull`]), waiting for it as for a stream.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{Codec, EventContext, EventOrigin, Source, SourceId, SourceKind, TrackId};
use nikgob_view::{
    ApiReply, Command, ConnectFail, ConnectJob, ConnectReport, MeshKind, ParkedView, RemoteCommand, RemoteHow, RemoteLink, RemoteTarget, Repeat, Status,
    Toast, ToastKind, ZoneEntry, ZoneLoaded, ZoneRepeat, ZoneSnapshot, ZoneState, ZoneStatus, ZoneTrack,
};

use crate::Worker;
use crate::queue::{self, Entry};

/// How long «Отменить» of a transfer onto a playing device works (its
/// snackbar stays 8 s).
const UNDO_FOR: Duration = Duration::from_secs(9);
/// «Продолжить здесь» after a lost link starts this much before where the
/// remote was.
const CONTINUE_BACK: Duration = Duration::from_millis(1500);
/// The best-effort pause of a remote left by «Продолжить здесь» is dropped
/// when it has not got there in this long.
const PAUSE_FOR: Duration = Duration::from_secs(5);
/// After a volume set here, the remote's reports of its volume (perhaps
/// sent before it heard of the set) are not shown for this long.
const VOLUME_HOLD: Duration = Duration::from_millis(800);
/// Target changes waiting for the one that runs.
const WAITING_MAX: usize = 16;
/// Files of the next tracks fetched ahead.
const PULL_AHEAD: usize = 2;

/// Where the loop hands the network part to the mesh service; false when
/// the job could not be taken.
pub(crate) type ConnectLink = Arc<dyn Fn(ConnectJob) -> bool + Send + Sync>;

struct Target {
    node: String,
    name: String,
    kind: MeshKind,
    link: RemoteLink,
    how: RemoteHow,
}

/// The target's last state and when it was (this device's clock).
struct Mirror {
    state: ZoneState,
    at: Instant,
}

/// What runs now of the target changes.
enum Doing {
    /// A transfer from `from` (None: this device) to `to` (None: here).
    Move { from: Option<String> },
    /// The undo of a transfer; the target before it (None: this device).
    Undo { before: Option<String> },
    /// «Продолжить здесь».
    Here,
}

/// A target change sent to the service, waiting for its report.
struct Busy {
    seq: u64,
    /// The session it was asked in.
    sess: u64,
    doing: Doing,
    /// The API's request waiting for its end.
    reply: Option<ApiReply>,
}

/// «Отменить» of a transfer onto a device that was playing.
struct Undo {
    /// The session it was raised for: another target since, no undo.
    sess: u64,
    node: String,
    previous: ZoneSnapshot,
    until: Instant,
    /// The target before the transfer (None: this device).
    before: Option<String>,
}

/// What the loop keeps of playing elsewhere.
#[derive(Default)]
pub(crate) struct ConnectKeep {
    pub(crate) link: Option<ConnectLink>,
    /// The target session's generation.
    sess: u64,
    target: Option<Target>,
    mirror: Option<Mirror>,
    /// The target's whole transferable state, as last fetched.
    full: Option<ZoneSnapshot>,
    /// The rows of the mirrored tracks (this library's), and the ids they
    /// were made for.
    rows: HashMap<TrackId, nikgob_view::TrackRow>,
    row_ids: Vec<TrackId>,
    /// The track whose lyrics and info the mirror shows.
    shown: Option<TrackId>,
    /// Where the display stopped while the link is not live.
    frozen: Option<Duration>,
    seq: u64,
    busy: Option<Busy>,
    /// Target changes asked while one runs, in order.
    waiting: VecDeque<Command>,
    undo: Option<Undo>,
    lost_toast: Option<u64>,
    volume_hold: Option<Instant>,
    /// The device files are fetched from: the last one that loaded or drove
    /// this zone.
    pull_from: Option<String>,
    /// The zone's loads, counted: a file asked for an earlier one is not
    /// this one's.
    load_gen: u64,
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

/// The answer of the API's transfer: done, or why not.
fn answer(reply: Option<ApiReply>, result: Result<(), String>) {
    let Some(r) = reply else { return };
    r.send(Ok(match result {
        Ok(()) => serde_json::json!({ "done": true }),
        Err(message) => serde_json::json!({ "done": false, "message": message }),
    }));
}

impl Worker {
    /// Hands `job` to the mesh service; false when there is none (or it
    /// could not take it).
    fn connect_send(&self, job: ConnectJob) -> bool {
        match self.connect.link.clone() {
            Some(f) => f(job),
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
            let sess = self.connect.sess;
            if !self.connect_send(ConnectJob::Run { sess, node, commands: vec![rc] }) {
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
            c @ (Command::OutputTransfer { .. }
            | Command::OutputAttach { .. }
            | Command::OutputDetach
            | Command::OutputContinueHere
            | Command::OutputUndoTransfer
            | Command::ApiTransfer { .. }) => {
                if self.connect.busy.is_none() {
                    self.connect_op(c);
                } else if self.connect.waiting.len() < WAITING_MAX {
                    self.connect.waiting.push_back(c);
                } else {
                    log::warn!("connect: too many target changes waiting, one dropped");
                    if let Command::ApiTransfer { reply, .. } = c {
                        answer(Some(reply), Err("Слишком много переносов подряд".to_owned()));
                    }
                }
            }
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

    /// A target change, with none running.
    fn connect_op(&mut self, c: Command) {
        match c {
            Command::OutputTransfer { node_id } => self.connect_transfer(node_id, None),
            Command::OutputAttach { node_id } => {
                let _ = self.connect_attach(node_id);
            }
            Command::OutputDetach => self.connect_detach(),
            Command::OutputContinueHere => self.connect_continue_here(),
            Command::OutputUndoTransfer => self.connect_undo(),
            Command::ApiTransfer { node_id, attach: false, reply } => self.connect_transfer(node_id, Some(reply)),
            Command::ApiTransfer { node_id: Some(node), attach: true, reply } => answer(Some(reply), self.connect_attach(node)),
            Command::ApiTransfer { node_id: None, attach: true, reply } => {
                self.connect_detach();
                answer(Some(reply), Ok(()));
            }
            other => log::warn!("connect: not a target change {other:?}"),
        }
    }

    /// The next target change waiting, now that none runs.
    fn connect_next(&mut self) {
        while self.connect.busy.is_none() {
            let Some(c) = self.connect.waiting.pop_front() else { break };
            self.connect_op(c);
        }
    }

    /// A paired device's name, kind and whether it is online.
    fn node_view(&self, id: &str) -> Option<(String, MeshKind, bool)> {
        let st = self.state.read();
        st.mesh.node(id).map(|n| (n.name.clone(), n.kind, n.online))
    }

    /// A new target session: what was sent for the one before is dropped.
    fn connect_session(&mut self, follow: Option<String>) {
        self.connect.sess += 1;
        let sess = self.connect.sess;
        if !self.connect_send(ConnectJob::Session { sess, follow }) {
            log::warn!("connect: no mesh service for the target's session");
        }
    }

    fn connect_transfer(&mut self, to: Option<String>, reply: Option<ApiReply>) {
        let from = self.connect.target.as_ref().map(|t| t.node.clone());
        match to {
            Some(node) => {
                if from.as_deref() == Some(node.as_str()) {
                    answer(reply, Ok(()));
                    return;
                }
                let Some((name, _, online)) = self.node_view(&node) else {
                    answer(reply, Err("Такого устройства нет".to_owned()));
                    return;
                };
                if !online {
                    let m = format!("{name} не в сети");
                    self.toast(m.clone(), None);
                    answer(reply, Err(m));
                    return;
                }
                // Nothing here to move: that device is the target as it is.
                if from.is_none() && self.queue.current().is_none() {
                    answer(reply, self.connect_attach(node));
                    return;
                }
                self.connect_move(from, Some(node), reply);
            }
            // Here already.
            None if from.is_none() => answer(reply, Ok(())),
            None => self.connect_move(from, None, reply),
        }
    }

    fn connect_move(&mut self, from: Option<String>, to: Option<String>, reply: Option<ApiReply>) {
        self.connect.seq += 1;
        let (seq, sess) = (self.connect.seq, self.connect.sess);
        if !self.connect_send(ConnectJob::Move { seq, sess, from: from.clone(), to }) {
            self.toast("Межсеть не запущена", None);
            answer(reply, Err("Межсеть не запущена".to_owned()));
            return;
        }
        self.connect.busy = Some(Busy { seq, sess, doing: Doing::Move { from }, reply });
    }

    /// `node` becomes the target as it is (nothing moves); why not.
    fn connect_attach(&mut self, node: String) -> Result<(), String> {
        if self.connect.target.as_ref().is_some_and(|t| t.node == node) {
            return Ok(());
        }
        let Some((name, kind, online)) = self.node_view(&node) else { return Err("Такого устройства нет".to_owned()) };
        if !online {
            let m = format!("{name} не в сети");
            self.toast(m.clone(), None);
            return Err(m);
        }
        self.connect_target(node, name, kind, RemoteHow::Attached);
        Ok(())
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

    /// `node` becomes the target (a new session): this zone parks
    /// (paused), the window follows that device's zone.
    fn connect_target(&mut self, node: String, name: String, kind: MeshKind, how: RemoteHow) {
        let first = self.connect.target.is_none();
        if first && matches!(self.status, Status::Playing | Status::Buffering) {
            self.pause();
        }
        let parked = first.then(|| self.parked_now());
        self.connect.target = Some(Target { node: node.clone(), name: name.clone(), kind, link: RemoteLink::Live, how });
        self.connect.mirror = None;
        self.connect.full = None;
        self.connect.rows.clear();
        self.connect.row_ids.clear();
        self.connect.shown = None;
        self.connect.frozen = None;
        self.connect.undo = None;
        self.connect_drop_lost_toast();
        let follow = node.clone();
        self.publish(move |s| {
            let (volume, muted) = (s.player.volume, s.player.muted);
            s.target = Some(RemoteTarget { node_id: node, name, kind, link: RemoteLink::Live, volume, muted, how });
            if let Some(p) = parked {
                s.parked = Some(p);
            }
            // Until its first state: buffering where it is.
            s.player.position = s.player.position_now();
            s.player.at = Instant::now();
            s.player.status = Status::Buffering;
            s.player.rate = 0.0;
        });
        self.connect_session(Some(follow));
    }

    fn connect_detach(&mut self) {
        if self.connect.target.is_none() {
            return;
        }
        self.connect_leave();
        self.connect_show_local();
    }

    /// The target is this device again (a new session; the mirror
    /// forgotten; the view's player is put back by
    /// [`Self::connect_show_local`] or a load).
    fn connect_leave(&mut self) {
        self.connect.target = None;
        self.connect.mirror = None;
        self.connect.full = None;
        self.connect.rows.clear();
        self.connect.row_ids.clear();
        self.connect.shown = None;
        self.connect.frozen = None;
        self.connect.volume_hold = None;
        self.connect.undo = None;
        self.connect_session(None);
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

    /// The device driven is no longer one of the owner's (forgotten,
    /// revoked, gone from the ring): the window is this device's again,
    /// its zone as it was (paused).
    pub(crate) fn connect_check_target(&mut self) {
        let Some(node) = self.connect.target.as_ref().map(|t| t.node.clone()) else { return };
        if self.state.read().mesh.node(&node).is_some() {
            return;
        }
        log::info!("connect: the device driven is no longer paired: back to this one");
        self.connect_leave();
        self.connect_show_local();
    }

    /// «Продолжить здесь»: the target's music plays here, from the state
    /// last known of it (a little before, after a lost link), only what
    /// this device can play. The target's session ends first (what was
    /// sent there and not done is dropped); the remote gets a pause if it
    /// still hears within a few seconds. With this device the target, its
    /// own paused zone goes on.
    fn connect_continue_here(&mut self) {
        let Some((node, live)) = self.connect.target.as_ref().map(|t| (t.node.clone(), t.link == RemoteLink::Live)) else {
            if self.status == Status::Paused {
                self.resume();
            }
            return;
        };
        let shown = self.state.read().player.position_now();
        let at = ms(if live { shown } else { shown.saturating_sub(CONTINUE_BACK) });
        let mirrored = self.connect.mirror.as_ref().map(|m| m.state.current.as_ref().map(|c| c.id));
        let snapshot = match (&self.connect.full, &self.connect.mirror) {
            (Some(f), Some(_)) if f.current().is_some() && Some(f.current()) == mirrored => Some(f.clone()),
            (_, Some(m)) => Some(m.state.snapshot(at)),
            (Some(f), None) => Some(f.clone()),
            (None, None) => None,
        }
        .map(|s| ZoneSnapshot {
            position_ms: at,
            paused: false,
            // The remote may not answer: only files of this library.
            files: Vec::new(),
            ..s
        });
        self.connect_leave();
        self.connect_show_local();
        let sess = self.connect.sess;
        if let Some(snapshot) = snapshot.filter(|s| s.index.is_some()) {
            self.connect.seq += 1;
            let seq = self.connect.seq;
            if self.connect_send(ConnectJob::Here { seq, sess, snapshot: Box::new(snapshot) }) {
                self.connect.busy = Some(Busy { seq, sess, doing: Doing::Here, reply: None });
            }
        }
        self.connect_send(ConnectJob::Pause { sess, node, until: Instant::now() + PAUSE_FOR });
    }

    /// «Отменить» of a transfer: the device's music before it comes back
    /// (it was playing: that is the point), and the target before it; only
    /// while the target is still the one the transfer made.
    fn connect_undo(&mut self) {
        let Some(u) = self.connect.undo.take() else { return };
        if Instant::now() > u.until || u.sess != self.connect.sess {
            log::info!("connect: the undo came too late (or the target changed)");
            return;
        }
        self.connect.seq += 1;
        let seq = self.connect.seq;
        if self.connect_send(ConnectJob::Undo { seq, sess: u.sess, node: u.node, snapshot: Box::new(u.previous) }) {
            self.connect.busy = Some(Busy { seq, sess: u.sess, doing: Doing::Undo { before: u.before }, reply: None });
        }
    }

    fn connect_report(&mut self, r: ConnectReport) {
        match r {
            ConnectReport::State { sess, node, state, at } => {
                if sess == self.connect.sess {
                    self.connect_state(&node, *state, at);
                }
            }
            ConnectReport::Link { sess, node, link } => {
                if sess == self.connect.sess {
                    self.connect_link(&node, link);
                }
            }
            ConnectReport::Snapshot { sess, node, snapshot } => {
                if sess == self.connect.sess && self.connect.target.as_ref().is_some_and(|t| t.node == node) {
                    self.connect.full = Some(*snapshot);
                }
            }
            ConnectReport::Moved { seq, to, result } => self.connect_moved(seq, to, result),
            ConnectReport::Undone { seq, result, .. } => self.connect_undone(seq, result),
            ConnectReport::Here { seq, result } => self.connect_here_done(seq, result),
            ConnectReport::Pulled { track, from, load, result } => self.connect_pulled(track, &from, load, result),
            ConnectReport::RunFailed { node, why, .. } => {
                let short: String = node.chars().take(10).collect();
                log::info!("connect: commands for {short}… did not get there: {why}");
            }
        }
    }

    /// The busy change `seq`, when it is that one.
    fn connect_done(&mut self, seq: u64) -> Option<Busy> {
        self.connect.busy.take_if(|b| b.seq == seq)
    }

    fn connect_moved(&mut self, seq: u64, to: Option<String>, result: Result<ZoneLoaded, ConnectFail>) {
        let Some(b) = self.connect_done(seq) else { return };
        let from = match b.doing {
            Doing::Move { from } => from,
            _ => None,
        };
        let same = b.sess == self.connect.sess;
        let name = to.as_deref().and_then(|n| self.node_view(n)).map(|v| v.0).unwrap_or_default();
        let failed = match &to {
            Some(_) => format!("Не удалось перенести на {name}"),
            None => "Не удалось вернуть музыку сюда".to_owned(),
        };
        let outcome = match result {
            Ok(_) if !same => {
                log::info!("connect: a transfer ended after the target changed: left as it is");
                Ok(())
            }
            Ok(loaded) => {
                match to {
                    Some(node) => self.connect_moved_to(node, from, loaded),
                    // The load here made this device the target already.
                    None => {
                        if self.connect.target.is_some() {
                            self.connect_leave();
                            self.connect_show_local();
                        }
                    }
                }
                Ok(())
            }
            Err(ConnectFail::Stale) => Err("Устройство сменилось".to_owned()),
            Err(ConnectFail::NothingDone(why)) => {
                log::warn!("connect: {failed}: {why}");
                self.toast(failed.clone(), None);
                Err(failed)
            }
            Err(ConnectFail::NotLoaded(why)) => {
                // The source is paused and stays so: the owner chooses.
                log::warn!("connect: {failed}: {why}");
                self.connect_toast_long(failed.clone(), Some(("Продолжить здесь".to_owned(), Command::OutputContinueHere)));
                Err(failed)
            }
        };
        answer(b.reply, outcome);
        self.connect_next();
    }

    /// `node` took the music: it is the target; onto a device that was
    /// playing, its music can come back for a while.
    fn connect_moved_to(&mut self, node: String, from: Option<String>, loaded: ZoneLoaded) {
        let Some((name, kind, _)) = self.node_view(&node) else { return };
        // What moved stops here (the service paused it; a play since too).
        if from.is_none() && matches!(self.status, Status::Playing | Status::Buffering) {
            self.pause();
        }
        self.connect_target(node.clone(), name.clone(), kind, RemoteHow::Transferred);
        if let Some(previous) = loaded.previous.filter(|prev| !prev.paused && prev.index.is_some()) {
            self.connect.undo = Some(Undo { sess: self.connect.sess, node, previous, until: Instant::now() + UNDO_FOR, before: from });
            self.connect_toast_long(format!("Очередь перенесена на {name}"), Some(("Отменить".to_owned(), Command::OutputUndoTransfer)));
        }
    }

    fn connect_undone(&mut self, seq: u64, result: Result<ZoneLoaded, ConnectFail>) {
        let Some(b) = self.connect_done(seq) else { return };
        let before = match b.doing {
            Doing::Undo { before } => before,
            _ => None,
        };
        if b.sess == self.connect.sess {
            match result {
                Ok(_) => match before.and_then(|n| self.node_view(&n).map(|(name, kind, _)| (n, name, kind))) {
                    // The target before, as it is (paused by the transfer).
                    Some((n, name, kind)) => self.connect_target(n, name, kind, RemoteHow::Transferred),
                    None => self.connect_detach(),
                },
                Err(ConnectFail::Stale) => {}
                Err(ConnectFail::NothingDone(why) | ConnectFail::NotLoaded(why)) => {
                    log::warn!("connect: the undo failed: {why}");
                    self.toast("Не удалось отменить", None);
                }
            }
        }
        self.connect_next();
    }

    fn connect_here_done(&mut self, seq: u64, result: Result<ZoneLoaded, ConnectFail>) {
        if self.connect_done(seq).is_none() {
            return;
        }
        match result {
            Ok(loaded) => {
                if !loaded.skipped.is_empty() {
                    let ids: Vec<String> = loaded.skipped.iter().map(ToString::to_string).collect();
                    log::info!("connect: not here, left out: {}", ids.join(", "));
                }
            }
            Err(ConnectFail::Stale) => {}
            Err(ConnectFail::NothingDone(why) | ConnectFail::NotLoaded(why)) => {
                log::warn!("connect: «Продолжить здесь» failed: {why}");
                self.toast(format!("Не удалось продолжить здесь: {why}"), None);
            }
        }
        self.connect_next();
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
        let duration = st
            .duration_ms
            .or_else(|| st.current.as_ref().and_then(|z| z.duration_ms))
            .map(Duration::from_millis)
            .or_else(|| current.as_ref().and_then(|c| c.duration));
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
        let target = RemoteTarget { node_id: t.node.clone(), name: t.name.clone(), kind: t.kind, link, volume, muted, how: t.how };
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
        // Files asked for the queue before are not this one's.
        self.connect.load_gen += 1;
        self.connect.pulling.clear();
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
        let (load, keep) = (self.connect.load_gen, self.connect_keep());
        if !self.connect_send(ConnectJob::Pull { track, from, load, keep }) {
            return false;
        }
        self.connect.pulling.insert(track);
        true
    }

    /// The cached files in use: the playing track's, the next ones'.
    fn connect_keep(&self) -> Vec<PathBuf> {
        let mut tracks: Vec<TrackId> = self.playing.iter().map(|e| e.track).collect();
        tracks.extend(self.queue.peek_auto_next().map(|e| e.track));
        tracks.extend(self.queue.ahead(PULL_AHEAD).into_iter().map(|e| e.track));
        let mut keep: Vec<PathBuf> = Vec::new();
        for t in tracks {
            if let Some(p) = self.connect.files.get(&t)
                && !keep.contains(p)
            {
                keep.push(p.clone());
            }
        }
        keep
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

    /// A file came (or did not) from `from`, asked for load `load`.
    fn connect_pulled(&mut self, track: TrackId, from: &str, load: u64, result: Result<PathBuf, String>) {
        // Asked for a queue before this one: not this one's.
        if load != self.connect.load_gen {
            return;
        }
        self.connect.pulling.remove(&track);
        let current_source = self.connect.pull_from.as_deref() == Some(from);
        match result {
            Ok(path) => {
                self.connect.files.insert(track, path);
            }
            // A device that is no longer the one files come from: asked
            // again of the one that is (below), never marked.
            Err(why) if !current_source => log::info!("connect: no file of {track} from a device left: {why}"),
            Err(why) => {
                log::warn!("connect: no file of {track} from the other device: {why}");
                self.connect.unpullable.insert(track);
            }
        }
        let have = self.connect.files.contains_key(&track);
        let waiting = self.playing.filter(|e| e.track == track && self.playing_source.is_none());
        if let Some(e) = waiting {
            if !have && !current_source && self.connect_pull(track) {
                return;
            }
            let paused = self.status == Status::Paused;
            if paused {
                // Kept for an explicit resume (it starts it from the
                // queue's current entry): nothing starts while paused.
                self.connect_keep_paused(e, have);
            } else {
                match have.then(|| self.request_skipping(e, &[])).flatten() {
                    // (A place it is to go on from: `start_with` seeks there.)
                    Some((req, source, kind)) => self.start_with(e, req, source, kind),
                    None => match self.queue.skip_forward() {
                        Some(n) if n.id != e.id => self.start(n),
                        _ => self.stop(),
                    },
                }
            }
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track) {
            self.preload_next();
        }
        self.connect_prefetch();
    }

    /// The waiting entry `e`, paused meanwhile: it (with its file) or the
    /// next one (without) waits paused for a resume, as a restored queue
    /// does, nothing opened.
    fn connect_keep_paused(&mut self, e: Entry, have: bool) {
        let at = if have {
            self.position
        } else {
            match self.queue.skip_forward() {
                Some(n) if n.id != e.id => Duration::ZERO,
                _ => {
                    self.stop();
                    return;
                }
            }
        };
        self.playback.stop();
        self.playing = None;
        self.playing_source = None;
        self.resume_at = Some(at);
        self.position = at;
        self.position_at = Instant::now();
        self.status = Status::Paused;
        self.publish_now_playing();
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
