//! Tracks with stream sources: the engine is asked for the best registered
//! source (local > Spotify > YouTube), the player reads Buffering until the
//! engine starts it, a source that fails to open falls back to the next,
//! and one that fails mid-track hands over where it stopped.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Codec, Source, SourceKind, TrackId};
use nikgob_core::streaming::{FakeService, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, Status};

/// Remembers what the engine was asked to play, and the seeks.
struct Recorder(Arc<Mutex<Vec<PlayRequest>>>, Arc<Mutex<Vec<Duration>>>);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.0.lock().unwrap().push(item);
    }
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, position: Duration) {
        self.1.lock().unwrap().push(position);
    }
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
    seeks: Arc<Mutex<Vec<Duration>>>,
}

impl Rig {
    fn start(lib: Library, streams: Vec<Arc<dyn StreamService>>) -> Self {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seeks = Arc::new(Mutex::new(Vec::new()));
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
            Box::new(Recorder(asked.clone(), seeks.clone())),
            Arc::new(|| {}),
        );
        Self { rt, asked, seeks }
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

    fn position(&self) -> Duration {
        self.rt.state().player.position
    }

    /// The engine started `id` and played it up to `at`.
    fn played_to(&self, id: u64, at: Duration) {
        let tx = self.rt.playback_sender();
        tx.send(PlaybackEvent::Started { id }).unwrap();
        wait_for("playing", || self.status() == Status::Playing);
        // A report right after a start is taken for a stale one.
        std::thread::sleep(Duration::from_millis(600));
        tx.send(PlaybackEvent::Position { id, position: at }).unwrap();
        wait_for("the position", || self.position() >= at);
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

#[test]
fn a_stream_cut_mid_track_goes_on_from_there_on_the_next_source() {
    let (lib, track) = both_services();
    let rig = Rig::start(lib, fakes());
    rig.play(track);
    let first = rig.asked(0);
    assert_eq!(first.source, stream(Service::Spotify, "sp-discovery-1"));
    let at = Duration::from_secs(17);
    rig.played_to(first.id, at);

    // The stream breaks off 17 s in: YouTube takes over there, not at 0.
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Error {
        id: first.id,
        message: "i/o error: stream URL keeps being refused (HTTP 403)".into(),
    })
    .unwrap();
    let second = rig.asked(1);
    assert_eq!(second.id, first.id);
    assert_eq!(second.source, stream(Service::Youtube, "yt-discovery-1"));
    assert_eq!(*rig.seeks.lock().unwrap(), vec![at]);
    wait_for("buffering again", || rig.status() == Status::Buffering);
    assert!(rig.position() >= at, "{:?}", rig.position());

    // Its start keeps the position.
    tx.send(PlaybackEvent::Started { id: second.id }).unwrap();
    wait_for("playing again", || rig.status() == Status::Playing);
    let p = rig.position();
    assert!(p >= at && p < at + Duration::from_secs(2), "{p:?}");

    // A new track starts at the top.
    rig.play(track);
    let third = rig.asked(2);
    assert_ne!(third.id, first.id);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.seeks.lock().unwrap().len(), 1, "no seek for a fresh start");
}

#[test]
fn a_copy_cut_mid_track_goes_on_from_there_on_the_copy_found_next() {
    let lib = Library::open_in_memory().unwrap();
    // Spotify without sound: the track plays from a YouTube copy it finds,
    // and YouTube Music has a second one.
    let track = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-gruppa-1")).unwrap();
    let youtube = FakeService::youtube()
        .logged_out()
        .with_song("gruppa-copy", "Группа крови", "Кино", "Группа крови", 285);
    let streams: Vec<Arc<dyn StreamService>> =
        vec![Arc::new(FakeService::spotify().logged_out()), Arc::new(youtube)];
    let rig = Rig::start(lib, streams);
    rig.play(track);
    let first = rig.asked(0);
    assert!(matches!(&first.source, PlaySource::Stream { service: Service::Youtube, .. }), "{:?}", first.source);
    let at = Duration::from_secs(17);
    rig.played_to(first.id, at);

    // That copy breaks off (the video is gone; a refusal of every client
    // of the player would be YouTube's and fail the next copy the same
    // way): the next copy found goes on at 17 s.
    rig.rt
        .playback_sender()
        .send(PlaybackEvent::Error {
            id: first.id,
            message: "i/o error: not available".into(),
        })
        .unwrap();
    let second = rig.asked(1);
    assert_eq!(second.id, first.id);
    assert_eq!(second.source, stream(Service::Youtube, "yt-x-gruppa-copy"));
    assert_eq!(*rig.seeks.lock().unwrap(), vec![at]);
    assert!(rig.position() >= at, "{:?}", rig.position());
}

/// The only copy of a song was set aside once (YouTube Music had no
/// details of it): ▶ tries it again rather than saying there is no file,
/// and once it plays it counts again.
#[test]
fn a_copy_set_aside_is_tried_again_and_counts_once_it_plays() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-discovery-1"))
        .unwrap();
    let source = lib.sources_of_track(track).unwrap()[0].id;
    lib.set_source_available(source, false).unwrap();
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::youtube())]);
    rig.play(track);
    let first = rig.asked(0);
    assert_eq!(first.source, stream(Service::Youtube, "yt-discovery-1"));
    assert!(!rig.rt.state().toasts.iter().any(|t| t.text.contains("Нет файла")));
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: first.id }).unwrap();
    tx.send(PlaybackEvent::Quality { id: first.id, label: "Opus".into() }).unwrap();
    wait_for("the copy to count again", || lib.get_source(source).unwrap().unwrap().available);
}

/// ▶ on an album the library knows one song of plays that song at once;
/// the rest of the album joins the queue when the service tells its
/// tracklist.
#[test]
fn an_album_known_in_part_plays_at_once_and_its_tracklist_joins_the_queue() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-discovery-1"))
        .unwrap();
    let release = lib.get_track(track).unwrap().unwrap().release_id.unwrap();
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::youtube())]);
    rig.rt.send(Command::PlayRelease { id: release, start: None, shuffle: false });
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-discovery-1"));
    wait_for("the album's other songs in the queue", || rig.rt.state().queue.next.len() == 3);
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "the playing song goes on");
}

/// YouTube refuses a stream's URLs some 15 s in (every client asked): the
/// same video is opened once more and goes on from there; refused again,
/// it gives up.
#[test]
fn a_youtube_stream_refused_midway_opens_again_once_where_it_stopped() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-discovery-1"))
        .unwrap();
    let rig = Rig::start(lib, vec![Arc::new(FakeService::youtube())]);
    rig.play(track);
    let first = rig.asked(0);
    assert_eq!(first.source, stream(Service::Youtube, "yt-discovery-1"));
    let at = Duration::from_secs(15);
    rig.played_to(first.id, at);
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Error {
        id: first.id,
        message: "i/o error: stream URL keeps being refused (HTTP 403)".into(),
    })
    .unwrap();
    let second = rig.asked(1);
    assert_eq!(second.id, first.id);
    assert_eq!(second.source, stream(Service::Youtube, "yt-discovery-1"));
    assert_eq!(*rig.seeks.lock().unwrap(), vec![at]);

    // Refused again: not a loop.
    rig.played_to(second.id, at + Duration::from_secs(5));
    tx.send(PlaybackEvent::Error {
        id: second.id,
        message: "i/o error: stream URL keeps being refused (HTTP 403)".into(),
    })
    .unwrap();
    wait_for("stopped", || rig.status() == Status::Stopped);
    assert_eq!(rig.asked.lock().unwrap().len(), 2);
}
