//! Playing on another device, as the loop does it: a zone loaded from a
//! snapshot (playing, or paused where it says) answers what it held
//! before, and gives its own snapshot; the window driving another device's
//! zone sends the player's commands there with its session, mirrors its
//! state (its tracks as this library has them), says a lost link with
//! «Продолжить здесь», takes it back by itself, ignores what an ended
//! session reports, and a detach brings this zone back; target changes go
//! one at a time, and nothing starts by itself when a transfer fails.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track, TrackId};
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig, RuntimeHandle};
use nikgob_view::{
    ApiReply, Command, ConnectFail, ConnectJob, ConnectReport, MeshKind, MeshNodeView, MeshSnapshot, MeshSnapshotNode, Prefs, RemoteCommand,
    RemoteLink, Status, ZoneEntry, ZoneLoaded, ZoneSnapshot, ZoneState, ZoneStatus, ZoneTrack,
};

fn config(library: Library) -> RuntimeConfig {
    RuntimeConfig {
        library,
        streams: Vec::new(),
        library_dir: std::env::temp_dir(),
        prefs: Prefs::default(),
        prefs_path: None,
        settings_path: None,
        import: None,
    }
}

fn wait(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A library of `n` tracks, each with a file.
fn library(n: usize) -> (Library, Vec<TrackId>) {
    let lib = Library::open_in_memory().unwrap();
    let ids = (0..n)
        .map(|i| {
            let t = lib.upsert_track(&Track::new(format!("T{i}"), ArtistCredit::single("A", None))).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("music/{i}.flac"), Codec::Flac)).unwrap();
            t.id
        })
        .collect();
    (lib, ids)
}

type Slot = Arc<Mutex<Option<Result<serde_json::Value, String>>>>;

fn reply() -> (ApiReply, Slot) {
    let slot: Slot = Arc::new(Mutex::new(None));
    let s = slot.clone();
    (ApiReply::new(move |a| *s.lock().unwrap() = Some(a)), slot)
}

fn answer(slot: &Slot) -> serde_json::Value {
    wait("the answer", || slot.lock().unwrap().is_some());
    slot.lock().unwrap().take().unwrap().expect("answered")
}

type Jobs = Arc<Mutex<Vec<ConnectJob>>>;

/// The jobs the runtime hands the mesh service, kept.
fn service(h: &RuntimeHandle) -> Jobs {
    let jobs: Jobs = Arc::default();
    let j = jobs.clone();
    h.link_connect(move |job| {
        j.lock().unwrap().push(job);
        true
    });
    jobs
}

/// The generation of the last session that follows `node`.
fn session_of(jobs: &Jobs, node: &str) -> Option<u64> {
    jobs.lock().unwrap().iter().rev().find_map(|j| match j {
        ConnectJob::Session { gen, follow: Some(n) } if n == node => Some(*gen),
        _ => None,
    })
}

fn device(h: &RuntimeHandle, rt: &Runtime, id: &str, name: &str) {
    let pi = MeshNodeView { id: id.into(), name: name.into(), kind: MeshKind::Pi, online: true, ..MeshNodeView::default() };
    h.send(Command::MeshUpdate(Box::new(MeshSnapshot { running: true, paired: true, nodes: vec![MeshSnapshotNode { view: pi, cert: String::new() }], ..MeshSnapshot::default() })));
    wait("the device", || rt.state().mesh.node(id).is_some_and(|n| n.online));
}

#[test]
fn a_zone_takes_a_snapshot_and_answers_what_it_held() {
    let (lib, t) = library(3);
    let rt = Runtime::start(config(lib), Box::new(NullPlayback), Arc::new(|| {}));
    let snap = ZoneSnapshot { tracks: t.clone(), index: Some(1), position_ms: 30_000, ..ZoneSnapshot::default() };
    let (r, slot) = reply();
    rt.send(Command::ApiLoad { snapshot: Box::new(snap), from: None, reply: r });
    assert!(answer(&slot)["previous"].is_null(), "the queue was empty");
    wait("playing", || rt.state().player.status == Status::Playing);
    assert_eq!(rt.state().queue.current.as_ref().map(|c| c.id), Some(t[1]));
    assert!(rt.state().player.position_now() >= Duration::from_secs(30), "from its place");

    let (r, slot) = reply();
    rt.send(Command::ApiSnapshot { reply: r });
    let s: ZoneSnapshot = serde_json::from_value(answer(&slot)).unwrap();
    assert_eq!((s.tracks.clone(), s.index, s.paused), (t.clone(), Some(1), false));
    assert!(s.position_ms >= 30_000);

    // Paused where it says; the queue before is answered for an undo.
    let snap = ZoneSnapshot { tracks: vec![t[2], t[0]], index: Some(0), position_ms: 5_000, paused: true, ..ZoneSnapshot::default() };
    let (r, slot) = reply();
    rt.send(Command::ApiLoad { snapshot: Box::new(snap), from: None, reply: r });
    let previous: ZoneSnapshot = serde_json::from_value(answer(&slot)["previous"].clone()).unwrap();
    assert_eq!((previous.tracks, previous.index, previous.paused), (t.clone(), Some(1), false));
    wait("paused on the new track", || {
        let st = rt.state();
        st.player.status == Status::Paused && st.player.current.as_ref().map(|c| c.id) == Some(t[2])
    });
    assert_eq!(rt.state().player.position_now(), Duration::from_secs(5));
}

fn zone_track(id: TrackId, title: &str) -> ZoneTrack {
    ZoneTrack { id, title: title.into(), artist: "A".into(), album: None, duration_ms: Some(200_000), liked: false }
}

#[test]
fn a_remote_sends_its_commands_and_mirrors_the_target() {
    let (lib, t) = library(2);
    let rt = Runtime::start(config(lib), Box::new(NullPlayback), Arc::new(|| {}));
    let h = rt.handle();
    let jobs = service(&h);
    device(&h, &rt, "pi-node", "Raspberry Pi");

    rt.send(Command::OutputAttach { node_id: "pi-node".into() });
    wait("the target", || rt.state().target.is_some());
    wait("followed", || session_of(&jobs, "pi-node").is_some());
    let gen = session_of(&jobs, "pi-node").unwrap();
    assert!(rt.state().parked.is_some());

    let unknown = TrackId::new();
    let zone = ZoneState {
        status: ZoneStatus::Playing,
        position_ms: 10_000,
        rate: 1.0,
        speed: 1.0,
        volume: 0.4,
        current: Some(zone_track(t[0], "T0")),
        current_entry: Some(1),
        next: vec![ZoneEntry { entry: 2, track: zone_track(unknown, "Не здесь") }],
        ..ZoneState::default()
    };
    // A state of an ended session is not shown.
    h.send(Command::ConnectReport(Box::new(ConnectReport::State { gen: gen - 1, node: "pi-node".into(), state: Box::new(zone.clone()), at: Instant::now() })));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rt.state().player.status, Status::Buffering, "still waiting for the target's first state");
    h.send(Command::ConnectReport(Box::new(ConnectReport::State { gen, node: "pi-node".into(), state: Box::new(zone), at: Instant::now() })));
    wait("the mirror", || rt.state().player.status == Status::Playing);
    {
        let st = rt.state();
        assert_eq!(st.player.current.as_ref().map(|c| c.id), Some(t[0]));
        // This library's row (which knows no length), not the remote's.
        assert_eq!(st.player.current.as_ref().map(|c| c.duration), Some(None));
        assert_eq!(st.player.duration, Some(Duration::from_millis(200_000)), "the remote's length");
        assert_eq!(st.queue.next[0].track.title, "Не здесь", "a track the library lacks: as the remote said");
        assert!((st.player.volume - 0.4).abs() < 1e-6);
        let target = st.target.as_ref().unwrap();
        assert_eq!((target.link, target.kind, target.name.as_str()), (RemoteLink::Live, MeshKind::Pi, "Raspberry Pi"));
        assert!(st.player.position_now() >= Duration::from_secs(10));
    }

    // The window's commands go there, with the session.
    let sent = |c: RemoteCommand| jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Run { gen: g, node, commands } if *g == gen && node == "pi-node" && commands == &vec![c.clone()]));
    rt.send(Command::Pause);
    wait("the pause sent", || sent(RemoteCommand::Pause));
    rt.send(Command::SetVolume(0.7));
    wait("the volume shown at once", || (rt.state().player.volume - 0.7).abs() < 1e-6);
    assert!(sent(RemoteCommand::SetVolume { volume: 0.7 }));

    // The link lost: paused display and «Продолжить здесь»; back by itself.
    h.send(Command::ConnectReport(Box::new(ConnectReport::Link { gen, node: "pi-node".into(), link: RemoteLink::Lost })));
    wait("lost", || rt.state().target.as_ref().is_some_and(|t| t.link == RemoteLink::Lost));
    assert_eq!(rt.state().player.status, Status::Paused);
    let toast = |st: &nikgob_view::ViewState| st.toasts.iter().any(|t| matches!(&t.action, Some((_, Command::OutputContinueHere))));
    assert!(toast(&rt.state()));
    h.send(Command::ConnectReport(Box::new(ConnectReport::Link { gen, node: "pi-node".into(), link: RemoteLink::Live })));
    wait("live again", || rt.state().player.status == Status::Playing);
    assert!(!toast(&rt.state()), "the toast goes by itself");

    // Back to this device: nothing moved, the parked zone shows again.
    rt.send(Command::OutputDetach);
    wait("detached", || rt.state().target.is_none());
    assert!(jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Session { gen: g, follow: None } if *g > gen)));
    assert_eq!(rt.state().player.status, Status::Stopped);
    assert!(rt.state().parked.is_none());
}

fn move_of(jobs: &Jobs, to: &str) -> Option<u64> {
    jobs.lock().unwrap().iter().find_map(|j| match j {
        ConnectJob::Move { seq, to: Some(n), .. } if n == to => Some(*seq),
        _ => None,
    })
}

/// A transfer with nothing here makes the device the target as it is; a
/// transfer of a playing queue asks the service to move it, another one
/// waits for it, and its answer parks this zone; the undo goes for the
/// session it was raised for.
#[test]
fn a_transfer_moves_through_the_service_and_parks_this_zone() {
    let (lib, t) = library(2);
    let rt = Runtime::start(config(lib), Box::new(NullPlayback), Arc::new(|| {}));
    let h = rt.handle();
    let jobs = service(&h);
    device(&h, &rt, "pi-node", "Pi");
    rt.send(Command::PlayTracks { tracks: t.clone(), start: 0, context: "библиотеки".into() });
    wait("playing here", || rt.state().player.status == Status::Playing);

    rt.send(Command::OutputTransfer { node_id: Some("pi-node".into()) });
    wait("the move asked", || move_of(&jobs, "pi-node").is_some());
    let seq = move_of(&jobs, "pi-node").unwrap();
    assert!(rt.state().target.is_none(), "nothing changes before the answer");
    // Another change waits for this one.
    rt.send(Command::OutputDetach);
    rt.send(Command::OutputTransfer { node_id: None });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(jobs.lock().unwrap().iter().filter(|j| matches!(j, ConnectJob::Move { .. })).count(), 1, "one at a time");

    // The device was playing: its queue comes back with «Отменить».
    let previous = ZoneSnapshot { tracks: vec![t[1]], index: Some(0), paused: false, ..ZoneSnapshot::default() };
    let loaded = ZoneLoaded { previous: Some(previous), skipped: Vec::new(), loaded: 2 };
    h.send(Command::ConnectReport(Box::new(ConnectReport::Moved { seq, to: Some("pi-node".into()), result: Ok(loaded) })));
    // The device became the target, then the detach waiting ran after it:
    // the parked zone is back, paused.
    wait("the target and the waiting detach", || {
        let Some(g) = session_of(&jobs, "pi-node") else { return false };
        jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Session { gen, follow: None } if *gen > g))
    });
    wait("detached", || rt.state().target.is_none());
    assert_eq!(jobs.lock().unwrap().iter().filter(|j| matches!(j, ConnectJob::Move { .. })).count(), 1, "back here was here already");
    wait("shown paused", || rt.state().player.status == Status::Paused);
    assert_eq!(rt.state().queue.current.as_ref().map(|c| c.id), Some(t[0]));
    // The undo was for a target that is gone: nothing is sent.
    rt.send(Command::OutputUndoTransfer);
    std::thread::sleep(Duration::from_millis(100));
    assert!(!jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Undo { .. })));
}

/// A load that did not take: the source stays paused, the toast offers
/// «Продолжить здесь», and nothing plays until it is pressed.
#[test]
fn a_failed_transfer_leaves_the_source_paused() {
    let (lib, t) = library(2);
    let rt = Runtime::start(config(lib), Box::new(NullPlayback), Arc::new(|| {}));
    let h = rt.handle();
    let jobs = service(&h);
    device(&h, &rt, "pi-node", "Pi");
    rt.send(Command::PlayTracks { tracks: t.clone(), start: 0, context: "библиотеки".into() });
    wait("playing here", || rt.state().player.status == Status::Playing);
    rt.send(Command::OutputTransfer { node_id: Some("pi-node".into()) });
    wait("the move asked", || move_of(&jobs, "pi-node").is_some());
    let seq = move_of(&jobs, "pi-node").unwrap();
    // The service paused this zone (through the API) before the load.
    rt.send(Command::Pause);
    wait("paused", || rt.state().player.status == Status::Paused);
    h.send(Command::ConnectReport(Box::new(ConnectReport::Moved { seq, to: Some("pi-node".into()), result: Err(ConnectFail::NotLoaded("нет ответа".into())) })));
    let offer = |st: &nikgob_view::ViewState| st.toasts.iter().any(|t| t.text == "Не удалось перенести на Pi" && matches!(&t.action, Some((_, Command::OutputContinueHere))));
    wait("the toast", || offer(&rt.state()));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rt.state().player.status, Status::Paused, "never resumed by itself");
    assert!(rt.state().target.is_none());
    rt.send(Command::OutputContinueHere);
    wait("played on the owner's word", || rt.state().player.status == Status::Playing);
}

/// The device driven is forgotten: the window is this device's again, its
/// zone as it was, the session over.
#[test]
fn forgetting_the_target_brings_this_zone_back_paused() {
    let (lib, t) = library(1);
    let rt = Runtime::start(config(lib), Box::new(NullPlayback), Arc::new(|| {}));
    let h = rt.handle();
    let jobs = service(&h);
    device(&h, &rt, "pi-node", "Pi");
    rt.send(Command::PlayTracks { tracks: t.clone(), start: 0, context: "библиотеки".into() });
    wait("playing here", || rt.state().player.status == Status::Playing);
    rt.send(Command::OutputAttach { node_id: "pi-node".into() });
    wait("the target", || rt.state().target.is_some());
    let gen = session_of(&jobs, "pi-node").unwrap();
    rt.send(Command::MeshForget { node_id: "pi-node".into() });
    wait("back here", || rt.state().target.is_none());
    assert!(jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Session { gen: g, follow: None } if *g > gen)));
    assert_eq!(rt.state().player.status, Status::Paused, "parked, not resumed");
    assert_eq!(rt.state().player.current.as_ref().map(|c| c.id), Some(t[0]));
}
