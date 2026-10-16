//! Repeat-one: the progress must follow the track back to its start. The view
//! extrapolates from the last published position and stops at the end, so a
//! loop the runtime did not publish leaves the progress frozen there.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, Repeat, Status};

/// Remembers which entries were handed to the engine.
struct Recorder(Arc<Mutex<Vec<u64>>>);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.0.lock().unwrap().push(item.id);
    }
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A runtime repeating one playing entry; returns the entry's id.
fn playing_on_repeat_one() -> (Runtime, u64) {
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let track = lib
        .upsert_track(&Track::new("t", ArtistCredit::single("A", Some(artist.id))))
        .unwrap();
    lib.add_source(&Source::new(track.id, SourceKind::LocalFile, "t.opus", Codec::Opus))
        .unwrap();
    let played = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib,
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(played.clone())),
        Arc::new(|| {}),
    );
    rt.send(Command::CycleRepeat); // off -> all
    rt.send(Command::CycleRepeat); // all -> one
    rt.send(Command::PlayTracks {
        tracks: vec![track.id],
        start: 0,
        context: "test".into(),
    });
    wait_for("the engine to be asked to play", || !played.lock().unwrap().is_empty());
    let id = played.lock().unwrap()[0];
    rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    wait_for("playing on repeat one", || {
        let s = rt.state();
        s.player.status == Status::Playing && s.player.repeat == Repeat::One
    });
    // Far-off reports right after a start are taken for stale ones.
    std::thread::sleep(Duration::from_millis(600));
    (rt, id)
}

fn shown(rt: &Runtime) -> Duration {
    rt.state().player.position_now()
}

#[test]
fn the_engine_reporting_the_start_again_moves_the_progress_back() {
    let (rt, id) = playing_on_repeat_one();
    let tx = rt.playback_sender();
    tx.send(PlaybackEvent::Position { id, position: Duration::from_secs(100) }).unwrap();
    wait_for("the view to follow the engine forward", || shown(&rt) >= Duration::from_secs(99));
    // The loop was not announced (or its announcement was swallowed): the
    // next position report alone must pull the view back.
    tx.send(PlaybackEvent::Position { id, position: Duration::from_millis(500) }).unwrap();
    wait_for("the view to follow the engine back", || shown(&rt) < Duration::from_secs(3));
}

#[test]
fn a_report_from_before_a_seek_does_not_pull_the_progress_back() {
    let (rt, id) = playing_on_repeat_one();
    rt.send(Command::Seek(Duration::from_secs(60)));
    wait_for("the seek to show", || shown(&rt) >= Duration::from_secs(59));
    // Sent by the engine before it got the seek.
    rt.playback_sender().send(PlaybackEvent::Position { id, position: Duration::from_secs(1) }).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(shown(&rt) >= Duration::from_secs(59), "a stale report moved the progress back");
}

#[test]
fn a_gapless_repeat_one_loop_restarts_the_progress() {
    let (rt, id) = playing_on_repeat_one();
    let tx = rt.playback_sender();
    tx.send(PlaybackEvent::Position { id, position: Duration::from_secs(100) }).unwrap();
    wait_for("the view to follow the engine forward", || shown(&rt) >= Duration::from_secs(99));
    tx.send(PlaybackEvent::TrackChanged { to: id }).unwrap();
    wait_for("the loop to restart the progress", || shown(&rt) < Duration::from_secs(3));
}
