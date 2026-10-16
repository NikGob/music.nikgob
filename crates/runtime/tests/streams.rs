//! Tracks with stream sources: the engine is asked for the best registered
//! source (local > Spotify > YouTube), the player reads Buffering until the
//! engine starts it, and a source that fails to open falls back to the next.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Codec, Source, SourceKind, TrackId};
use nikgob_core::streaming::{FakeService, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, Status};

/// Remembers what the engine was asked to play.
struct Recorder(Arc<Mutex<Vec<PlayRequest>>>);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.0.lock().unwrap().push(item);
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
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service
        .tracks()
        .into_iter()
        .find(|t| t.id.id == id)
        .unwrap()
}

struct Rig {
    rt: Runtime,
    asked: Arc<Mutex<Vec<PlayRequest>>>,
}

impl Rig {
    fn start(lib: Library, streams: Vec<Arc<dyn StreamService>>) -> Self {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let rt = Runtime::start(
            RuntimeConfig {
                library: lib,
                streams,
                library_dir: std::env::temp_dir(),
                prefs: Prefs::default(),
                prefs_path: None,
                settings_path: None,
                import: None,
            },
            Box::new(Recorder(asked.clone())),
            Arc::new(|| {}),
        );
        Self { rt, asked }
    }

    fn play(&self, track: TrackId) {
        self.rt.send(Command::PlayTracks {
            tracks: vec![track],
            start: 0,
            context: "test".into(),
        });
    }

    /// The `n`-th request (waits for it).
    fn asked(&self, n: usize) -> PlayRequest {
        wait_for("a play request", || self.asked.lock().unwrap().len() > n);
        self.asked.lock().unwrap()[n].clone()
    }

    fn status(&self) -> Status {
        self.rt.state().player.status
    }
}

fn stream(service: Service, id: &str) -> PlaySource {
    PlaySource::Stream {
        service,
        id: id.into(),
    }
}

/// One track known to both services (same ISRC), liked.
fn both_services() -> (Library, TrackId) {
    let lib = Library::open_in_memory().unwrap();
    let id = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-discovery-1"))
        .unwrap();
    assert_eq!(
        lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1"))
            .unwrap(),
        id
    );
    lib.like(id).unwrap();
    (lib, id)
}

fn fakes() -> Vec<Arc<dyn StreamService>> {
    vec![
        Arc::new(FakeService::spotify()),
        Arc::new(FakeService::youtube()),
    ]
}

#[test]
fn a_stream_buffers_then_plays_and_falls_back_to_the_next_service() {
    let (lib, track) = both_services();
    let rig = Rig::start(lib.clone(), fakes());
    rig.play(track);
    let first = rig.asked(0);
    assert_eq!(first.source, stream(Service::Spotify, "sp-discovery-1"));
    assert_eq!(first.duration, Some(Duration::from_secs(320)));
    wait_for("buffering", || rig.status() == Status::Buffering);
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: first.id }).unwrap();
    wait_for("playing", || rig.status() == Status::Playing);

    // Spotify gives up (region, no login): YouTube is next, same entry.
    tx.send(PlaybackEvent::Error {
        id: first.id,
        message: "unavailable".into(),
    })
    .unwrap();
    let second = rig.asked(1);
    assert_eq!(second.id, first.id);
    assert_eq!(second.source, stream(Service::Youtube, "yt-discovery-1"));
    wait_for("buffering again", || rig.status() == Status::Buffering);

    // Nothing left: the queue moves on (here: stops).
    tx.send(PlaybackEvent::Error {
        id: first.id,
        message: "unavailable".into(),
    })
    .unwrap();
    wait_for("stopped", || rig.status() == Status::Stopped);
    assert_eq!(rig.asked.lock().unwrap().len(), 2);

    // The start was recorded as a stream play.
    let events = lib.events_of_track(track, 10).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.source_kind == Some(SourceKind::Spotify)),
        "{events:?}"
    );
}

#[test]
fn a_service_that_is_not_registered_is_skipped() {
    let (lib, track) = both_services();
    let rig = Rig::start(lib, vec![Arc::new(FakeService::youtube())]);
    rig.play(track);
    assert_eq!(
        rig.asked(0).source,
        stream(Service::Youtube, "yt-discovery-1")
    );
}

#[test]
fn a_local_file_comes_first_and_a_stream_backs_it_up() {
    let (lib, track) = both_services();
    lib.add_source(&Source::new(
        track,
        SourceKind::LocalFile,
        "Music/one-more-time.opus",
        Codec::Opus,
    ))
    .unwrap();
    let rig = Rig::start(lib, fakes());
    rig.play(track);
    let first = rig.asked(0);
    assert!(
        matches!(&first.source, PlaySource::File(p) if p.ends_with("one-more-time.opus")),
        "{:?}",
        first.source
    );
    wait_for("playing", || rig.status() == Status::Playing);
    // The file is gone: Spotify takes over.
    rig.rt
        .playback_sender()
        .send(PlaybackEvent::Error {
            id: first.id,
            message: "no such file".into(),
        })
        .unwrap();
    assert_eq!(
        rig.asked(1).source,
        stream(Service::Spotify, "sp-discovery-1")
    );
}

#[test]
fn without_services_a_stream_only_track_is_not_playable() {
    let (lib, track) = both_services();
    let rig = Rig::start(lib, Vec::new());
    rig.play(track);
    std::thread::sleep(Duration::from_millis(300));
    assert!(rig.asked.lock().unwrap().is_empty());
    assert_eq!(rig.status(), Status::Stopped);
}
