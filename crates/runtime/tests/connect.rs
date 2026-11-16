//! Playing on another device, as the loop does it: a zone loaded from a
//! snapshot (playing, or paused where it says) answers what it held
//! before, and gives its own snapshot; the window driving another device's
//! zone sends the player's commands there, mirrors its state (its tracks
//! as this library has them), says a lost link with «Продолжить здесь»,
//! takes it back by itself, and a detach brings this zone back.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track, TrackId};
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{
    ApiReply, Command, ConnectJob, ConnectReport, MeshKind, MeshNodeView, MeshSnapshot, MeshSnapshotNode, Prefs, RemoteCommand, RemoteLink, Status,
    ZoneEntry, ZoneSnapshot, ZoneState, ZoneStatus, ZoneTrack,
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
    let jobs: Arc<Mutex<Vec<ConnectJob>>> = Arc::default();
    {
        let j = jobs.clone();
        h.link_connect(move |job| j.lock().unwrap().push(job));
    }
    let has = |job: &ConnectJob| jobs.lock().unwrap().contains(job);
    let pi = MeshNodeView { id: "pi-node".into(), name: "Raspberry Pi".into(), kind: MeshKind::Pi, online: true, ..MeshNodeView::default() };
    h.send(Command::MeshUpdate(Box::new(MeshSnapshot {
        running: true,
        paired: true,
        nodes: vec![MeshSnapshotNode { view: pi, cert: String::new() }],
        ..MeshSnapshot::default()
    })));
    wait("the device", || rt.state().mesh.node("pi-node").is_some_and(|n| n.online));

    rt.send(Command::OutputAttach { node_id: "pi-node".into() });
    wait("the target", || rt.state().target.is_some());
    wait("followed", || has(&ConnectJob::Follow { node: "pi-node".into() }));
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
    h.send(Command::ConnectReport(Box::new(ConnectReport::State { node: "pi-node".into(), state: Box::new(zone), at: Instant::now() })));
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

    // The window's commands go there, not to this zone.
    rt.send(Command::Pause);
    wait("the pause sent", || has(&ConnectJob::Run { node: "pi-node".into(), commands: vec![RemoteCommand::Pause] }));
    rt.send(Command::SetVolume(0.7));
    wait("the volume shown at once", || (rt.state().player.volume - 0.7).abs() < 1e-6);
    assert!(has(&ConnectJob::Run { node: "pi-node".into(), commands: vec![RemoteCommand::SetVolume { volume: 0.7 }] }));

    // The link lost: paused display and «Продолжить здесь»; back by itself.
    h.send(Command::ConnectReport(Box::new(ConnectReport::Link { node: "pi-node".into(), link: RemoteLink::Lost })));
    wait("lost", || rt.state().target.as_ref().is_some_and(|t| t.link == RemoteLink::Lost));
    assert_eq!(rt.state().player.status, Status::Paused);
    let toast = |st: &nikgob_view::ViewState| st.toasts.iter().any(|t| matches!(&t.action, Some((_, Command::OutputContinueHere))));
    assert!(toast(&rt.state()));
    h.send(Command::ConnectReport(Box::new(ConnectReport::Link { node: "pi-node".into(), link: RemoteLink::Live })));
    wait("live again", || rt.state().player.status == Status::Playing);
    assert!(!toast(&rt.state()), "the toast goes by itself");

    // Back to this device: nothing moved, the parked zone shows again.
    rt.send(Command::OutputDetach);
    wait("detached", || rt.state().target.is_none());
    assert!(has(&ConnectJob::Unfollow));
    assert_eq!(rt.state().player.status, Status::Stopped);
    assert!(rt.state().parked.is_none());
}

/// A transfer with nothing here makes the device the target as it is; a
/// transfer of a playing queue asks the service to move it, and its answer
/// parks this zone.
#[test]
fn a_transfer_moves_through_the_service_and_parks_this_zone() {
    let (lib, t) = library(2);
    let rt = Runtime::start(config(lib), Box::new(NullPlayback), Arc::new(|| {}));
    let h = rt.handle();
    let jobs: Arc<Mutex<Vec<ConnectJob>>> = Arc::default();
    {
        let j = jobs.clone();
        h.link_connect(move |job| j.lock().unwrap().push(job));
    }
    let pi = MeshNodeView { id: "pi-node".into(), name: "Pi".into(), kind: MeshKind::Pi, online: true, ..MeshNodeView::default() };
    h.send(Command::MeshUpdate(Box::new(MeshSnapshot { running: true, nodes: vec![MeshSnapshotNode { view: pi, cert: String::new() }], ..MeshSnapshot::default() })));
    wait("the device", || rt.state().mesh.node("pi-node").is_some());
    rt.send(Command::PlayTracks { tracks: t.clone(), start: 0, context: "библиотеки".into() });
    wait("playing here", || rt.state().player.status == Status::Playing);

    rt.send(Command::OutputTransfer { node_id: Some("pi-node".into()) });
    wait("the move asked", || jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Move { from: None, to: Some(n), .. } if n == "pi-node")));
    let seq = jobs.lock().unwrap().iter().find_map(|j| if let ConnectJob::Move { seq, .. } = j { Some(*seq) } else { None }).unwrap();
    assert!(rt.state().target.is_none(), "nothing changes before the answer");
    // The device was playing: its queue comes back with «Отменить».
    let previous = ZoneSnapshot { tracks: vec![t[1]], index: Some(0), paused: false, ..ZoneSnapshot::default() };
    let loaded = nikgob_view::ZoneLoaded { previous: Some(previous), skipped: Vec::new(), loaded: 2 };
    h.send(Command::ConnectReport(Box::new(ConnectReport::Moved { seq, to: Some("pi-node".into()), result: Ok(loaded) })));
    wait("the target", || rt.state().target.is_some());
    let st = rt.state();
    assert_eq!(st.parked.as_ref().and_then(|p| p.current.as_ref()).map(|c| c.id), Some(t[0]), "this zone parked");
    assert!(st.toasts.iter().any(|t| t.long && matches!(&t.action, Some((_, Command::OutputUndoTransfer)))));
    drop(st);
    rt.send(Command::OutputUndoTransfer);
    wait("the undo sent", || jobs.lock().unwrap().iter().any(|j| matches!(j, ConnectJob::Load { node, snapshot, .. } if node == "pi-node" && snapshot.tracks == vec![t[1]])));
}
