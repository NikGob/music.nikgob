//! Tracks with stream sources: the engine is asked for the best registered
//! source (local > Spotify > YouTube), the player reads Buffering until the
//! engine starts it, a source that fails to open falls back to the next,
//! and one that fails mid-track hands over where it stopped.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Codec, Source, SourceKind, TrackId};
use nikgob_core::streaming::{FakeService, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent, QualityLabel, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs, Status};

/// Remembers what the engine was asked to play, the seeks, what it was
/// handed as the next item, and every play, pause, resume, stop and seek
/// in order.
struct Recorder(
    Arc<Mutex<Vec<PlayRequest>>>,
    Arc<Mutex<Vec<Duration>>>,
    Arc<Mutex<Vec<Option<PlayRequest>>>>,
    Arc<Mutex<Vec<String>>>,
);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.3.lock().unwrap().push("play".into());
        self.0.lock().unwrap().push(item);
    }
    fn set_next(&mut self, next: Option<PlayRequest>) {
        self.2.lock().unwrap().push(next);
    }
    fn pause(&mut self) {
        self.3.lock().unwrap().push("pause".into());
    }
    fn resume(&mut self) {
        self.3.lock().unwrap().push("resume".into());
    }
    fn stop(&mut self) {
        self.3.lock().unwrap().push("stop".into());
    }
    fn seek(&mut self, position: Duration) {
        self.3.lock().unwrap().push("seek".into());
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
    nexts: Arc<Mutex<Vec<Option<PlayRequest>>>>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    fn start(lib: Library, streams: Vec<Arc<dyn StreamService>>) -> Self {
        Self::start_with(lib, streams, StreamSchedule::default())
    }

    fn start_with(lib: Library, streams: Vec<Arc<dyn StreamService>>, schedule: StreamSchedule) -> Self {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seeks = Arc::new(Mutex::new(Vec::new()));
        let nexts = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Vec::new()));
        let rt = Runtime::start_scheduled(
            RuntimeConfig {
                library: lib,
                streams,
                library_dir: std::env::temp_dir(),
                prefs: Prefs::default(),
                prefs_path: None,
                settings_path: None,
                import: None,
            },
            Box::new(Recorder(asked.clone(), seeks.clone(), nexts.clone(), log.clone())),
            Arc::new(|| {}),
            schedule,
        );
        Self { rt, asked, seeks, nexts, log }
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
    let st = rig.rt.state();
    let no_file = nikgob_core::i18n::errors_in(st.prefs.resolved_locale()).no_file_for_track;
    assert!(!st.toasts.iter().any(|t| t.text == no_file || t.text.contains("Нет файла")));
    drop(st);
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: first.id }).unwrap();
    tx.send(PlaybackEvent::Quality { id: first.id, label: QualityLabel::Text("Opus".into()) }).unwrap();
    wait_for("the copy to count again", || lib.get_source(source).unwrap().unwrap().available);
}

/// «Слушать весь альбом» of an album the library knows one song of plays
/// that song at once; the rest of the album joins the queue when the
/// service tells its tracklist.
#[test]
fn an_album_known_in_part_plays_at_once_and_its_tracklist_joins_the_queue() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-discovery-1"))
        .unwrap();
    let release = lib.get_track(track).unwrap().unwrap().release_id.unwrap();
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::youtube())]);
    // One song of it is the user's: «Слушать весь альбом» asks for all of
    // it (a plain «Слушать» plays that song alone).
    rig.rt.send(Command::PlayReleaseWhole { id: release, start: None, shuffle: false });
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

/// YouTube Music's top search result («Всё») names a song's cover but no
/// album and no length.
fn top_result(t: &StreamTrack) -> StreamTrack {
    let mut top = t.clone();
    top.album = None;
    top.duration_ms = None;
    top.track_no = None;
    top
}

/// A song started from the top search result: its video's picture shows
/// at once, then its details bring the album with its cover and the length.
#[test]
fn a_top_search_result_gets_its_album_cover_and_length_while_it_plays() {
    let yt = FakeService::youtube().with_song("mozgi", "Мозги & деньги", "Комсомольск", "Комсомольск-1", 150);
    let full = find(&yt, "yt-x-mozgi");
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&top_result(&full)).unwrap();
    assert_eq!(lib.get_track(track).unwrap().unwrap().release_id, None);
    let rig = Rig::start(lib.clone(), vec![Arc::new(yt)]);
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-x-mozgi"));
    wait_for("the album", || {
        rig.rt.state().player.current.as_ref().is_some_and(|r| r.release_id.is_some())
    });
    let s = rig.rt.state();
    let row = s.player.current.as_ref().unwrap();
    assert_eq!(row.album.as_deref(), Some("Комсомольск-1"));
    assert_eq!(row.cover.as_ref().and_then(|c| c.web_url()), full.cover_url.as_deref());
    assert_eq!(s.player.duration, Some(Duration::from_secs(150)));
}

/// A stream its service has no details of: the video's picture is its
/// cover, and the length the engine found is its length.
#[test]
fn a_stream_without_details_takes_the_heard_length_and_its_video_picture() {
    let mut t = top_result(&find(&FakeService::youtube(), "yt-gruppa-1"));
    t.id = nikgob_core::streaming::RemoteId::new(Service::Youtube, "yt-not-in-the-catalog");
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&t).unwrap();
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::youtube())]);
    rig.play(track);
    let first = rig.asked(0);
    wait_for("buffering", || rig.status() == Status::Buffering);
    {
        let s = rig.rt.state();
        assert_eq!(s.player.duration, None);
        assert_eq!(
            s.player.current.as_ref().and_then(|r| r.cover.as_ref()).and_then(|c| c.web_url()),
            Some("https://i.ytimg.com/vi/yt-not-in-the-catalog/hqdefault.jpg")
        );
    }
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: first.id }).unwrap();
    tx.send(PlaybackEvent::Duration { id: first.id, duration: Duration::from_millis(149_521) }).unwrap();
    wait_for("the length", || rig.rt.state().player.duration.is_some());
    assert_eq!(rig.rt.state().player.duration, Some(Duration::from_millis(149_521)));
    assert_eq!(lib.get_track(track).unwrap().unwrap().duration_ms, Some(149_521));
}

/// A song known only from YouTube Music (a like, a radio, a search), with
/// its ISRC.
fn youtube_only(lib: &Library) -> TrackId {
    let mut yt = find(&FakeService::youtube(), "yt-homogenic-3");
    yt.isrc = Some("ZZFAK9700003".into());
    lib.upsert_stream_track(&yt).unwrap()
}

fn has_spotify_copy(lib: &Library, track: TrackId) -> bool {
    lib.sources_of_track(track)
        .unwrap()
        .iter()
        .any(|s| s.kind == SourceKind::Spotify && s.locator == "sp-homogenic-3")
}

#[test]
fn a_track_from_youtube_plays_from_a_connected_spotify_first() {
    // Spotify is asked for it (by its ISRC) before it starts: the first
    // play is Spotify's, nothing is opened on YouTube Music.
    let lib = Library::open_in_memory().unwrap();
    let track = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    let youtube = Arc::new(FakeService::youtube());
    let streams: Vec<Arc<dyn StreamService>> = vec![
        spotify.clone() as Arc<dyn StreamService>,
        youtube.clone() as Arc<dyn StreamService>,
    ];
    let rig = Rig::start(lib.clone(), streams);
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Spotify, "sp-homogenic-3"));
    assert!(has_spotify_copy(&lib, track), "kept for the next time");
    assert_eq!(spotify.calls("find_isrc"), 1, "{:?}", spotify.all_calls());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "nothing restarts it");
    // The next play asks nothing.
    rig.play(track);
    assert_eq!(rig.asked(1).source, stream(Service::Spotify, "sp-homogenic-3"));
    assert_eq!(spotify.calls("find_isrc"), 1);
}

#[test]
fn a_slow_spotify_leaves_the_first_play_to_youtube_and_has_the_next() {
    // Spotify does not answer within the wait: YouTube Music plays, the
    // answer still joins the track, the next play is Spotify's.
    let lib = Library::open_in_memory().unwrap();
    let track = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(1500));
    let streams: Vec<Arc<dyn StreamService>> = vec![
        spotify.clone() as Arc<dyn StreamService>,
        Arc::new(FakeService::youtube()),
    ];
    let schedule = StreamSchedule { prefer_wait: Duration::from_millis(300), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    let t = Instant::now();
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-homogenic-3"));
    assert!(t.elapsed() < Duration::from_millis(1400), "the wait is bounded: {:?}", t.elapsed());
    wait_for("the Spotify copy", || has_spotify_copy(&lib, track));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "the play goes on from YouTube Music");
    assert_eq!(spotify.calls("find_isrc"), 1, "asked once, not again when the play started");
    spotify.delay("find_isrc", Duration::ZERO);
    rig.play(track);
    assert_eq!(rig.asked(1).source, stream(Service::Spotify, "sp-homogenic-3"));
}

#[test]
fn without_spotify_a_track_from_youtube_plays_from_youtube_at_once() {
    let lib = Library::open_in_memory().unwrap();
    let track = youtube_only(&lib);
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::youtube())]);
    let t = Instant::now();
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-homogenic-3"));
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
}

#[test]
fn a_song_spotify_does_not_have_plays_from_youtube_and_is_not_asked_again() {
    let lib = Library::open_in_memory().unwrap();
    let mut yt = find(&FakeService::youtube(), "yt-homogenic-3");
    yt.isrc = None;
    yt.title = "Песня, которой нет в Spotify".into();
    let track = lib.upsert_stream_track(&yt).unwrap();
    let spotify = Arc::new(FakeService::spotify());
    let streams: Vec<Arc<dyn StreamService>> = vec![
        spotify.clone() as Arc<dyn StreamService>,
        Arc::new(FakeService::youtube()),
    ];
    let rig = Rig::start(lib.clone(), streams);
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-homogenic-3"));
    let asked = spotify.total_calls();
    assert!(asked >= 1);
    rig.play(track);
    assert_eq!(rig.asked(1).source, stream(Service::Youtube, "yt-homogenic-3"));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(spotify.total_calls(), asked, "the miss is kept");
}

#[test]
fn a_track_from_youtube_is_not_looked_up_on_a_spotify_without_sound() {
    let lib = Library::open_in_memory().unwrap();
    let track = lib
        .upsert_stream_track(&find(&FakeService::youtube(), "yt-homogenic-3"))
        .unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![
        Arc::new(FakeService::spotify().without_audio()),
        Arc::new(FakeService::youtube()),
    ];
    let rig = Rig::start(lib.clone(), streams);
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-homogenic-3"));
    // Spotify would find it by name: nothing joins the track.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !lib.sources_of_track(track).unwrap().iter().any(|s| s.kind == SourceKind::Spotify)
    );
}

/// The engine lets go of a stream it was reading when another track
/// takes over (a read that stalled is cancelled). Should an error of the
/// track let go still come, it is not the playing track's: no skip, no
/// failure, no toast, and its source is not set aside.
#[test]
fn an_error_of_a_track_no_longer_playing_changes_nothing() {
    let lib = Library::open_in_memory().unwrap();
    let yt = FakeService::youtube();
    let a = lib.upsert_stream_track(&find(&yt, "yt-discovery-1")).unwrap();
    let b = lib.upsert_stream_track(&find(&yt, "yt-discovery-2")).unwrap();
    let rig = Rig::start(lib.clone(), vec![Arc::new(yt)]);
    rig.rt.send(Command::PlayTracks { tracks: vec![a, b], start: 0, context: "test".into() });
    let first = rig.asked(0);
    rig.played_to(first.id, Duration::from_secs(5));

    rig.rt.send(Command::Next);
    let second = rig.asked(1);
    assert_ne!(second.id, first.id);
    assert_eq!(second.source, stream(Service::Youtube, "yt-discovery-2"));
    rig.played_to(second.id, Duration::from_secs(3));

    // The track left behind fails late, as a refused video would.
    let toasts = rig.rt.state().toasts.len();
    rig.rt
        .playback_sender()
        .send(PlaybackEvent::Error { id: first.id, message: "youtube: video not available".into() })
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(rig.asked.lock().unwrap().len(), 2, "nothing else was played");
    assert_eq!(rig.status(), Status::Playing);
    assert!(rig.position() >= Duration::from_secs(3), "{:?}", rig.position());
    assert_eq!(rig.rt.state().toasts.len(), toasts, "{:?}", rig.rt.state().toasts);
    let sources = lib.sources_of_track(a).unwrap();
    assert!(sources.iter().all(|s| s.available), "{sources:?}");
}

/// A song of Spotify's own (it plays from there at once) and the
/// YouTube-only song after it.
fn spotify_then_youtube(lib: &Library) -> (TrackId, TrackId) {
    let first = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    (first, youtube_only(lib))
}

fn play_both(rig: &Rig, tracks: [TrackId; 2]) {
    rig.rt.send(Command::PlayTracks { tracks: tracks.to_vec(), start: 0, context: "test".into() });
}

fn handed_youtube(rig: &Rig) -> bool {
    rig.nexts.lock().unwrap().iter().flatten().any(|n| n.source == stream(Service::Youtube, "yt-homogenic-3"))
}

#[test]
fn gapless_the_next_track_is_handed_over_with_its_spotify_copy_once_known() {
    // The next song is asked of Spotify while the first plays; the engine
    // gets it only with that answer (never its YouTube copy first), and
    // the join records the source the engine was handed.
    let lib = Library::open_in_memory().unwrap();
    let (first, next) = spotify_then_youtube(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(600));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let rig = Rig::start(lib.clone(), streams);
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    assert_eq!(playing.source, stream(Service::Spotify, "sp-discovery-1"));
    wait_for("the next one handed over", || {
        rig.nexts.lock().unwrap().iter().flatten().any(|n| n.source == stream(Service::Spotify, "sp-homogenic-3"))
    });
    assert!(!handed_youtube(&rig), "never its YouTube copy first: {:?}", rig.nexts.lock().unwrap());
    let handed = rig.nexts.lock().unwrap().iter().flatten().last().cloned().unwrap();
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: playing.id }).unwrap();
    tx.send(PlaybackEvent::TrackChanged { to: handed.id }).unwrap();
    wait_for("the next one plays", || rig.rt.state().player.current.as_ref().is_some_and(|t| t.id == next));
    wait_for("its start", || {
        lib.events_of_track(next, 10).unwrap().iter().any(|e| e.event_type == nikgob_core::model::EventType::Start)
    });
    let start = lib.events_of_track(next, 10).unwrap().into_iter().find(|e| e.event_type == nikgob_core::model::EventType::Start).unwrap();
    assert_eq!(start.source_kind, Some(SourceKind::Spotify));
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "a gapless join, no new play");
}

fn handed_spotify(rig: &Rig) -> bool {
    rig.nexts.lock().unwrap().iter().flatten().any(|n| n.source == stream(Service::Spotify, "sp-homogenic-3"))
}

/// The source the start of `track` was recorded with (waits for it).
fn started_from(lib: &Library, track: TrackId) -> Option<SourceKind> {
    let start = |e: &nikgob_core::model::Event| e.event_type == nikgob_core::model::EventType::Start;
    wait_for("its start", || lib.events_of_track(track, 10).unwrap().iter().any(start));
    lib.events_of_track(track, 10).unwrap().into_iter().find(start).unwrap().source_kind
}

#[test]
fn a_late_spotify_answer_leaves_the_next_track_its_youtube_copy_for_a_gapless_join() {
    // Spotify is slower than the next track may wait: its YouTube copy is
    // handed over in time for the join. The answer then joins the track for
    // the next time but swaps nothing, nor does a change of the queue, and
    // the join records what the engine was handed.
    let lib = Library::open_in_memory().unwrap();
    let (first, next) = spotify_then_youtube(&lib);
    let other = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-2")).unwrap();
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(1200));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_millis(300), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    let t = Instant::now();
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    wait_for("the YouTube copy handed over", || handed_youtube(&rig));
    assert!(t.elapsed() < Duration::from_millis(1100), "before Spotify answered: {:?}", t.elapsed());
    wait_for("the Spotify copy", || has_spotify_copy(&lib, next));
    rig.rt.send(Command::Enqueue(vec![other]));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!handed_spotify(&rig), "the handed request is never swapped: {:?}", rig.nexts.lock().unwrap());
    let last = rig.nexts.lock().unwrap().last().cloned().flatten();
    assert!(last.is_some_and(|n| n.source == stream(Service::Youtube, "yt-homogenic-3")), "never taken back");
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: playing.id }).unwrap();
    let handed = rig.nexts.lock().unwrap().iter().flatten().last().cloned().unwrap();
    tx.send(PlaybackEvent::TrackChanged { to: handed.id }).unwrap();
    assert_eq!(started_from(&lib, next), Some(SourceKind::Youtube), "what the engine plays");
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "a gapless join, no new play");
}

#[test]
fn the_next_track_is_handed_over_ten_seconds_before_the_end_at_the_latest() {
    // The playing song has 12 s: the next one waits 2 s for Spotify, not
    // the whole wait, and goes over with its YouTube copy. (A song of the
    // fake's own catalog would come back with its own length from the
    // likes, playlists or details read at the start: one of 12 s there.)
    let lib = Library::open_in_memory().unwrap();
    let spotify = Arc::new(FakeService::spotify().with_song("short", "Short", "Daft Punk", "Short", 12));
    let first = lib.upsert_stream_track(&find(&spotify, "sp-x-short")).unwrap();
    let next = youtube_only(&lib);
    spotify.delay("find_isrc", Duration::from_millis(4000));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_secs(6), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    let t = Instant::now();
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    rig.rt.playback_sender().send(PlaybackEvent::Started { id: playing.id }).unwrap();
    wait_for("the YouTube copy handed over", || handed_youtube(&rig));
    let at = t.elapsed();
    assert!(at >= Duration::from_millis(1500) && at < Duration::from_millis(3500), "about 2 s: {at:?}");
    assert!(!handed_spotify(&rig));
}

#[test]
fn a_file_that_comes_after_the_handover_leaves_the_engine_what_it_was_handed() {
    // The next song went over with its YouTube copy; then a file of it is
    // attached and the queue changes before the join is acknowledged. The
    // engine keeps the YouTube request (none withdrawn, no file instead),
    // and the join records YouTube.
    let lib = Library::open_in_memory().unwrap();
    let (first, next) = spotify_then_youtube(&lib);
    let other = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-2")).unwrap();
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(1200));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_millis(300), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    wait_for("the YouTube copy handed over", || handed_youtube(&rig));
    let handed_at = rig.nexts.lock().unwrap().len();
    lib.add_source(&Source::new(next, SourceKind::LocalFile, "Music/homogenic-3.opus", Codec::Opus)).unwrap();
    rig.rt.send(Command::Enqueue(vec![other]));
    std::thread::sleep(Duration::from_millis(400));
    let after: Vec<_> = rig.nexts.lock().unwrap()[handed_at..].to_vec();
    assert!(
        after.iter().all(|n| n.as_ref().is_some_and(|n| n.source == stream(Service::Youtube, "yt-homogenic-3"))),
        "neither withdrawn nor swapped for the file: {after:?}"
    );
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: playing.id }).unwrap();
    let handed = rig.nexts.lock().unwrap().iter().flatten().last().cloned().unwrap();
    tx.send(PlaybackEvent::TrackChanged { to: handed.id }).unwrap();
    assert_eq!(started_from(&lib, next), Some(SourceKind::Youtube), "what the engine plays");
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "a gapless join, no new play");
}

#[test]
fn the_next_track_goes_over_at_once_when_the_playing_ones_length_is_unknown() {
    // Its end could come any moment: the next song does not wait for
    // Spotify and goes over with its YouTube copy. The song is not in the
    // fake's catalog and was completed in vain lately: nothing at the
    // start gives it a length.
    let lib = Library::open_in_memory().unwrap();
    let mut unknown = StreamTrack::new(nikgob_core::streaming::RemoteId::new(Service::Spotify, "sp-x-nowhere"), "Nowhere");
    unknown.artists = vec![nikgob_core::streaming::ArtistRef::new(None, "Daft Punk")];
    let first = lib.upsert_stream_track(&unknown).unwrap();
    lib.cache_put("enrich.miss.spotify:sp-x-nowhere", nikgob_library::now_ms(), "-").unwrap();
    assert_eq!(lib.get_track(first).unwrap().unwrap().duration_ms, None);
    let next = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(3000));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_secs(6), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    let t = Instant::now();
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    rig.rt.playback_sender().send(PlaybackEvent::Started { id: playing.id }).unwrap();
    wait_for("the YouTube copy handed over", || handed_youtube(&rig));
    assert!(t.elapsed() < Duration::from_millis(1500), "at once: {:?}", t.elapsed());
    assert!(!handed_spotify(&rig));
}

#[test]
fn a_seek_near_the_end_hands_the_next_track_over_at_once() {
    // The next song waits for Spotify while a long song plays; a seek to
    // its last seconds leaves no time for the wait: it goes over with its
    // YouTube copy then, not when the wait would have ended.
    let lib = Library::open_in_memory().unwrap();
    let mut long = find(&FakeService::spotify(), "sp-discovery-1");
    long.duration_ms = Some(320_000);
    let first = lib.upsert_stream_track(&long).unwrap();
    let next = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(5000));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_secs(4), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    rig.rt.playback_sender().send(PlaybackEvent::Started { id: playing.id }).unwrap();
    wait_for("playing", || rig.status() == Status::Playing);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!handed_youtube(&rig), "still waiting for Spotify");
    let t = Instant::now();
    rig.rt.send(Command::Seek(Duration::from_secs(315)));
    wait_for("the YouTube copy handed over", || handed_youtube(&rig));
    assert!(t.elapsed() < Duration::from_millis(1500), "right after the seek: {:?}", t.elapsed());
    assert!(!handed_spotify(&rig));
}

#[test]
fn without_a_request_handed_over_the_next_track_waits_for_spotify_as_any_other() {
    // Gapless off: when the first song ends, the next one waits (bounded)
    // for its Spotify copy and plays from there.
    let lib = Library::open_in_memory().unwrap();
    let (first, next) = spotify_then_youtube(&lib);
    let spotify = Arc::new(FakeService::spotify());
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let rig = Rig::start(lib.clone(), streams);
    let prefs = Prefs { gapless: false, ..rig.rt.state().prefs.clone() };
    rig.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for("gapless off", || !rig.rt.state().prefs.gapless);
    // Asked ahead; delayed so the answer is not in when the first ends.
    spotify.delay("find_isrc", Duration::from_millis(600));
    play_both(&rig, [first, next]);
    let playing = rig.asked(0);
    let tx = rig.rt.playback_sender();
    tx.send(PlaybackEvent::Started { id: playing.id }).unwrap();
    tx.send(PlaybackEvent::Ended { id: playing.id }).unwrap();
    assert_eq!(rig.asked(1).source, stream(Service::Spotify, "sp-homogenic-3"));
    assert!(!handed_youtube(&rig));
}

#[test]
fn a_failed_spotify_lookup_is_no_miss_and_is_asked_again_later() {
    // No connection: YouTube Music plays, nothing is kept as «not on
    // Spotify», the next play asks nothing (held a moment) and nothing is
    // attached.
    let lib = Library::open_in_memory().unwrap();
    let track = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.fail_next("find_isrc", 1);
    spotify.fail_next("search_kind", 2);
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let rig = Rig::start(lib.clone(), streams);
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-homogenic-3"));
    std::thread::sleep(Duration::from_millis(200));
    assert!(lib.cache_get(&format!("sp_miss.{track}")).unwrap().is_none(), "a failure is not a miss");
    assert!(!has_spotify_copy(&lib, track));
    let asked = spotify.calls("find_isrc");
    rig.play(track);
    assert_eq!(rig.asked(1).source, stream(Service::Youtube, "yt-homogenic-3"));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(spotify.calls("find_isrc"), asked, "held back a moment, not asked at once");
}

#[test]
fn a_pause_during_the_wait_is_kept_through_the_youtube_lookup_that_follows() {
    // A Deezer track (nothing plays), paused while Spotify is asked:
    // Spotify does not answer in time, YouTube Music is looked up, and
    // the copy it finds is prepared paused, never played.
    let lib = Library::open_in_memory().unwrap();
    let mut listed = find(&FakeService::deezer(), "dz-discovery-2");
    listed.isrc = None;
    let track = lib.upsert_stream_track(&listed).unwrap();
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(1500));
    let youtube = Arc::new(FakeService::youtube().logged_out());
    youtube.delay("find_isrc", Duration::from_millis(400));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, youtube.clone() as Arc<dyn StreamService>, Arc::new(FakeService::deezer())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_millis(300), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    rig.play(track);
    wait_for("the wait", || rig.status() == Status::Buffering);
    rig.rt.send(Command::Pause);
    wait_for("paused", || rig.status() == Status::Paused);
    let req = rig.asked(0);
    assert_eq!(req.source, stream(Service::Youtube, "yt-discovery-2"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.status(), Status::Paused, "still paused once it is ready");
    assert!(paused_right_after_play(&rig), "{:?}", rig.log.lock().unwrap());
}

/// The engine was paused right after it was asked to play (nothing in
/// between), and never resumed.
fn paused_right_after_play(rig: &Rig) -> bool {
    let log = rig.log.lock().unwrap();
    let Some(at) = log.iter().rposition(|e| e == "play") else { return false };
    log.get(at + 1).is_some_and(|e| e == "pause") && !log.iter().any(|e| e == "resume")
}

#[test]
fn a_pause_during_the_wait_is_kept_when_nothing_is_found_and_the_next_track_starts() {
    // Paused while a track that plays from nothing waits for Spotify;
    // neither Spotify nor YouTube Music has it: the next track is prepared
    // paused, never played.
    let lib = Library::open_in_memory().unwrap();
    let mut listed = find(&FakeService::deezer(), "dz-discovery-2");
    listed.isrc = None;
    listed.id = nikgob_core::streaming::RemoteId::new(Service::Deezer, "dz-not-listed");
    listed.title = "Zzqx Nothing Here".into();
    let missing = lib.upsert_stream_track(&listed).unwrap();
    let after = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("search_kind", Duration::from_millis(800));
    let youtube = Arc::new(FakeService::youtube().logged_out());
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, youtube as Arc<dyn StreamService>, Arc::new(FakeService::deezer())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_millis(300), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    play_both(&rig, [missing, after]);
    wait_for("the wait", || rig.status() == Status::Buffering);
    rig.rt.send(Command::Pause);
    wait_for("paused", || rig.status() == Status::Paused);
    let req = rig.asked(0);
    assert_eq!(req.source, stream(Service::Spotify, "sp-discovery-1"), "the next track");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.status(), Status::Paused);
    assert!(rig.rt.state().player.current.as_ref().is_some_and(|t| t.id == after));
    assert!(paused_right_after_play(&rig), "{:?}", rig.log.lock().unwrap());
}

#[test]
fn a_seek_during_the_wait_is_where_the_track_starts() {
    // The bar moved to 1:30 while the track waited for its Spotify copy:
    // it starts there, the engine asked to seek once the track is open.
    let lib = Library::open_in_memory().unwrap();
    let track = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(600));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let rig = Rig::start(lib.clone(), streams);
    rig.play(track);
    wait_for("the wait", || rig.status() == Status::Buffering);
    let at = Duration::from_secs(90);
    rig.rt.send(Command::Seek(at));
    wait_for("the bar moved", || rig.position() >= at);
    let req = rig.asked(0);
    assert_eq!(req.source, stream(Service::Spotify, "sp-homogenic-3"));
    wait_for("the seek", || !rig.seeks.lock().unwrap().is_empty());
    assert_eq!(*rig.seeks.lock().unwrap(), vec![at], "once the track is open");
    {
        let log = rig.log.lock().unwrap();
        let play = log.iter().position(|e| e == "play").unwrap();
        assert!(log[play..].iter().any(|e| e == "seek"), "{log:?}");
    }
    rig.rt.playback_sender().send(PlaybackEvent::Started { id: req.id }).unwrap();
    wait_for("playing", || rig.status() == Status::Playing);
    assert!(rig.position() >= at, "{:?}", rig.position());
}

#[test]
fn a_spotify_outage_asks_nothing_track_after_track_until_its_time() {
    // Spotify fails: the next songs are not asked of it (and play from
    // YouTube Music at once) until the hold is over; then it is asked
    // again.
    let lib = Library::open_in_memory().unwrap();
    let yt = FakeService::youtube();
    let tracks = vec![
        youtube_only(&lib),
        lib.upsert_stream_track(&find(&yt, "yt-gruppa-3")).unwrap(),
        lib.upsert_stream_track(&find(&yt, "yt-firstlove-2")).unwrap(),
    ];
    let spotify = Arc::new(FakeService::spotify());
    spotify.fail_next("find_isrc", 1000);
    spotify.fail_next("search_kind", 1000);
    let asked = || spotify.calls("find_isrc") + spotify.calls("search_kind");
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_retry: Duration::from_millis(1500), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    rig.rt.send(Command::PlayTracks { tracks, start: 0, context: "test".into() });
    assert_eq!(rig.asked(0).source, stream(Service::Youtube, "yt-homogenic-3"));
    let failed_at = Instant::now();
    std::thread::sleep(Duration::from_millis(200));
    let before = asked();
    assert!(before >= 1);
    rig.rt.send(Command::Next);
    assert_eq!(rig.asked(1).source, stream(Service::Youtube, "yt-gruppa-3"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(asked(), before, "nothing asked of Spotify meanwhile");
    std::thread::sleep(Duration::from_millis(1600).saturating_sub(failed_at.elapsed()));
    rig.rt.send(Command::Next);
    wait_for("Spotify asked again", || asked() > before);
    assert_eq!(rig.asked(2).source, stream(Service::Youtube, "yt-firstlove-2"));
}

#[test]
fn a_spotify_copy_set_aside_is_tried_again_when_nothing_else_plays() {
    // Its only copy was set aside and the song was looked up just now:
    // the copy is tried again rather than nothing.
    let lib = Library::open_in_memory().unwrap();
    let track = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    let source = lib.sources_of_track(track).unwrap()[0].id;
    lib.set_source_aside(source).unwrap();
    lib.cache_put(&format!("sp_miss.{track}"), nikgob_library::now_ms(), "").unwrap();
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::spotify()) as Arc<dyn StreamService>]);
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Spotify, "sp-discovery-1"));
}

#[test]
fn a_preview_only_listing_is_never_played_for_the_song() {
    // A SoundCloud listing that gives a preview only, and nowhere to look
    // the song up (YouTube Music off): nothing plays rather than the
    // snippet. The same listing only set aside is tried again.
    let lib = Library::open_in_memory().unwrap();
    let mut preview = StreamTrack::new(nikgob_core::streaming::RemoteId::new(Service::Soundcloud, "sc-preview"), "Группа крови");
    preview.artists = vec![nikgob_core::streaming::ArtistRef::new(None, "Кино")];
    preview.duration_ms = Some(285_000);
    preview.playable = false;
    let track = lib.upsert_stream_track(&preview).unwrap();
    let source = lib.sources_of_track(track).unwrap()[0].id;
    assert!(!lib.get_source(source).unwrap().unwrap().available);
    let rig = Rig::start(lib.clone(), vec![Arc::new(FakeService::new(Service::Soundcloud)) as Arc<dyn StreamService>]);
    rig.play(track);
    std::thread::sleep(Duration::from_millis(500));
    assert!(rig.asked.lock().unwrap().is_empty(), "the preview is not played");
    lib.set_source_aside(source).unwrap();
    rig.play(track);
    assert_eq!(rig.asked(0).source, stream(Service::Soundcloud, "sc-preview"), "a copy set aside is");
}

#[test]
fn a_failed_spotify_lookup_of_a_track_nothing_plays_is_no_miss_either() {
    // YouTube Music off: the track is looked up on Spotify alone, and that
    // lookup fails. Nothing is kept as «not on Spotify», and it is not
    // asked again at once (a second ask would find it now).
    let lib = Library::open_in_memory().unwrap();
    let track = youtube_only(&lib);
    let spotify = Arc::new(FakeService::spotify());
    spotify.fail_next("find_isrc", 1);
    spotify.fail_next("search_kind", 2);
    let rig = Rig::start(lib.clone(), vec![spotify.clone() as Arc<dyn StreamService>]);
    rig.play(track);
    wait_for("the lookup", || spotify.calls("search_kind") >= 2);
    std::thread::sleep(Duration::from_millis(300));
    assert!(lib.cache_get(&format!("sp_miss.{track}")).unwrap().is_none(), "a failure is not a miss");
    assert!(!has_spotify_copy(&lib, track), "not asked again at once");
    assert!(rig.asked.lock().unwrap().is_empty());
}

#[test]
fn a_track_skipped_during_the_wait_does_not_start_when_spotify_answers() {
    // «Дальше» while the first song waits for its Spotify copy: the next
    // one plays, and neither the end of the wait nor the answer starts the
    // skipped song.
    let lib = Library::open_in_memory().unwrap();
    let waiting = youtube_only(&lib);
    let other = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    let spotify = Arc::new(FakeService::spotify());
    spotify.delay("find_isrc", Duration::from_millis(800));
    let streams: Vec<Arc<dyn StreamService>> = vec![spotify.clone() as Arc<dyn StreamService>, Arc::new(FakeService::youtube())];
    let schedule = StreamSchedule { prefer_wait: Duration::from_millis(300), ..StreamSchedule::default() };
    let rig = Rig::start_with(lib.clone(), streams, schedule);
    play_both(&rig, [waiting, other]);
    wait_for("the wait", || rig.status() == Status::Buffering);
    assert!(rig.asked.lock().unwrap().is_empty());
    rig.rt.send(Command::Next);
    assert_eq!(rig.asked(0).source, stream(Service::Spotify, "sp-discovery-1"));
    wait_for("the answer", || has_spotify_copy(&lib, waiting));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.asked.lock().unwrap().len(), 1, "nothing else starts");
    assert!(rig.rt.state().player.current.as_ref().is_some_and(|t| t.id == other));
}
