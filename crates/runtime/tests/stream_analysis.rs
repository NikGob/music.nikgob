//! The sound analysis of stream-only tracks on the fake
//! services: a played YouTube track is analysed from the disk cache without
//! a request; liked tracks are downloaded and analysed in the background one
//! at a time, a gap apart and within the daily cap (kept over a restart);
//! Spotify likes go through their YouTube Music songs and Spotify's sound is
//! never opened; a rate limit only waits; nothing is downloaded while
//! YouTube Music plays; the switch stops it at once; a
//! file that arrives later is analysed again from the file.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{AnalysisSource, Codec, Source, SourceKind, TrackId};
use nikgob_core::streaming::{FakeService, RemoteId, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs, ViewState};

const WITH: &str = nikgob_recs::ANALYZED_WITH;

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

fn schedule(gap_ms: u64, daily: u32) -> StreamSchedule {
    StreamSchedule {
        tick: Duration::from_millis(50),
        analysis_start: Duration::ZERO,
        analysis_idle: Duration::from_millis(200),
        analysis_gap: Duration::from_millis(gap_ms),
        analysis_daily: daily,
        ..StreamSchedule::default()
    }
}

fn start(
    lib: &Library,
    streams: Vec<Arc<dyn StreamService>>,
    prefs: Prefs,
    schedule: StreamSchedule,
) -> (Runtime, Arc<Mutex<Vec<PlayRequest>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs,
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(asked.clone())),
        Arc::new(|| {}),
        schedule,
    );
    (rt, asked)
}

/// Long enough for bliss to say something about the fake's sine.
fn youtube() -> FakeService {
    let yt = FakeService::youtube();
    yt.set_audio_secs(6.0);
    yt
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service.tracks().into_iter().find(|t| t.id.id == id).unwrap()
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}: {:?}", rt.state().analysis);
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn source_of(lib: &Library, track: TrackId) -> Option<AnalysisSource> {
    lib.get_analysis(track)
        .unwrap()
        .filter(|a| a.analyzed_with == WITH)
        .map(|a| a.source)
}

#[test]
fn a_played_track_is_analysed_from_the_cache_without_a_request() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube().logged_out());
    let track = lib.upsert_stream_track(&find(&yt, "yt-gruppa-1")).unwrap();
    let skipped = lib.upsert_stream_track(&find(&yt, "yt-firstlove-1")).unwrap();
    // It played before: its whole audio is "on disk". The other one is not.
    drop(yt.open_audio("yt-gruppa-1").unwrap());
    yt.reset_calls();
    let opened = yt.opened().len();
    // (Without autoplay, which would ask the service for a radio.)
    let (rt, asked) = start(&lib, vec![yt.clone()], Prefs { autoplay: false, ..Prefs::default() }, schedule(50, 300));

    rt.send(Command::PlayTracks {
        tracks: vec![track, skipped],
        start: 0,
        context: "test".into(),
    });
    wait_until("the play request", || !asked.lock().unwrap().is_empty());
    let first = asked.lock().unwrap()[0].id;
    rt.playback_sender().send(PlaybackEvent::Started { id: first }).unwrap();
    rt.playback_sender().send(PlaybackEvent::Ended { id: first }).unwrap();
    wait_until("the analysis", || source_of(&lib, track).is_some());
    assert_eq!(source_of(&lib, track), Some(AnalysisSource::Youtube));

    // The second one ends without its audio in the cache: nothing.
    wait_until("the next request", || asked.lock().unwrap().len() >= 2);
    let second = asked.lock().unwrap()[1].id;
    rt.playback_sender().send(PlaybackEvent::Ended { id: second }).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(source_of(&lib, skipped), None);
    assert_eq!(yt.total_calls(), 0, "{:?}", yt.all_calls());
    assert_eq!(yt.opened().len(), opened, "the cache only");
    drop(rt);
}

#[test]
fn liked_tracks_are_analysed_one_by_one_within_the_daily_cap() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube());
    let gap = Duration::from_millis(400);
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(gap.as_millis() as u64, 2));

    // The three likes come in and are counted before anything is analysed.
    wait_for(&rt, "the likes", |s| s.analysis.streams_total == 3);
    let mut at = Vec::new();
    for n in 1..=2 {
        wait_for(&rt, "the next analysis", |s| s.analysis.streams_done >= n);
        at.push(Instant::now());
        assert_eq!(rt.state().analysis.streams_done, n, "one at a time");
    }
    assert!(at[1] - at[0] >= gap.mul_f64(0.7), "{:?} apart", at[1] - at[0]);
    // The day's two downloads are made: the third waits for tomorrow.
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(rt.state().analysis.streams_done, 2);
    assert_eq!(yt.opened().len(), 2, "{:?}", yt.opened());
    drop(rt);

    // The count survives a restart.
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(50, 2));
    wait_for(&rt, "the progress", |s| s.analysis.streams_total == 3);
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(rt.state().analysis.streams_done, 2);
    assert_eq!(yt.opened().len(), 2);
    drop(rt);

    // Another day's budget: the last one goes, newest first was kept.
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(50, 3));
    wait_for(&rt, "the last one", |s| s.analysis.streams_done == 3);
    assert!(rt.state().analysis.complete());
    let vectors = lib.analysis_vectors(WITH).unwrap();
    assert_eq!(vectors.len(), 3);
    assert!(vectors.iter().all(|a| a.source == AnalysisSource::Youtube));
    drop(rt);
}

#[test]
fn spotify_likes_go_through_their_youtube_music_songs() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let yt = Arc::new(youtube().logged_out());
    let streams: Vec<Arc<dyn StreamService>> = vec![sp.clone(), yt.clone()];
    let (rt, _) = start(&lib, streams, Prefs::default(), schedule(50, 300));

    wait_for(&rt, "the likes", |s| s.analysis.streams_total > 0);
    wait_for(&rt, "the analysis", |s| s.analysis.streams_total > 0 && s.analysis.complete());
    let done = rt.state().analysis.streams_done;
    assert!(done >= 1, "{:?}", rt.state().analysis);
    // Analysed as the same library tracks, from YouTube; Spotify's own
    // sound is never opened.
    for row in lib.analysis_vectors(WITH).unwrap() {
        assert_eq!(row.source, AnalysisSource::Youtube);
        let sources = lib.sources_of_track(row.track_id).unwrap();
        assert!(sources.iter().any(|s| s.kind == SourceKind::Spotify), "a Spotify track");
        assert!(sources.iter().any(|s| s.kind == SourceKind::Youtube), "its song attached");
    }
    assert!(sp.opened().is_empty(), "{:?}", sp.opened());
    assert!(yt.opened().iter().all(|id| id.starts_with("yt-")));
    assert_eq!(yt.opened().len() as u32, done);
    drop(rt);
}

#[test]
fn a_rate_limit_waits_and_is_not_a_failure() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube());
    let pause = Duration::from_millis(900);
    yt.rate_limit_next_paused("open_audio", 1, pause);
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(50, 300));

    wait_until("the limited download", || yt.calls("open_audio") == 1);
    let limited = Instant::now();
    // Nothing marked: every like is still due.
    assert_eq!(lib.streams_needing_analysis(WITH, 0, 10).unwrap().len(), 3);
    wait_for(&rt, "the first analysis", |s| s.analysis.streams_done >= 1);
    assert!(limited.elapsed() >= pause.mul_f64(0.8), "waited {:?}", limited.elapsed());
    wait_for(&rt, "the rest", |s| s.analysis.streams_done == 3 && s.analysis.streams_total == 3);
    assert_eq!(yt.calls("open_audio"), 1, "only the limited one is counted");
    drop(rt);
}

#[test]
fn the_switch_stops_the_analysis_at_once() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube());
    let off = Prefs {
        stream_analysis: false,
        ..Prefs::default()
    };
    let (rt, _) = start(&lib, vec![yt.clone()], off.clone(), schedule(300, 300));
    wait_for(&rt, "the likes", |s| s.analysis.streams_total == 3);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(rt.state().analysis.streams_done, 0);
    assert!(yt.opened().is_empty(), "{:?}", yt.opened());

    rt.send(Command::SetPrefs(Box::default()));
    wait_for(&rt, "the first analysis", |s| s.analysis.streams_done >= 1);
    rt.send(Command::SetPrefs(Box::new(off)));
    std::thread::sleep(Duration::from_millis(100));
    let done = rt.state().analysis.streams_done;
    let opened = yt.opened().len();
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(rt.state().analysis.streams_done, done);
    assert_eq!(yt.opened().len(), opened);
    assert!(done < 3, "stopped before the end");
    drop(rt);
}

#[test]
fn tracks_that_cannot_be_looked_up_do_not_block_the_older_ones() {
    let lib = Library::open_in_memory().unwrap();
    // Logged out: its own likes are not pulled, only the library's count.
    let yt = Arc::new(youtube().logged_out());
    let older = lib.upsert_stream_track(&find(&yt, "yt-gruppa-1")).unwrap();
    lib.like(older).unwrap();
    // More newer likes than one read of the queue, each missed on YouTube
    // Music a moment ago: none of them can be looked up now.
    for n in 0..40 {
        let t = StreamTrack::new(RemoteId::new(Service::Spotify, format!("sp-missed-{n}")), format!("Missed {n}"));
        let id = lib.upsert_stream_track(&t).unwrap();
        lib.like(id).unwrap();
        lib.cache_put(&format!("yt_miss.{id}"), nikgob_library::now_ms(), "").unwrap();
    }
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(50, 300));

    wait_until("the older track's analysis", || source_of(&lib, older).is_some());
    assert_eq!(yt.opened(), vec!["yt-gruppa-1".to_owned()]);
    drop(rt);
}

fn write_wav(path: &Path, secs: f32) {
    let rate = 44_100;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut seed = 3u32;
    for i in 0..(secs * rate as f32) as u32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 16) as f32 / 65_536.0 - 0.5) * 0.1;
        let t = i as f32 / rate as f32;
        let env = (-5.0 * (t % 0.4)).exp();
        let tone = (t * 440.0 * std::f32::consts::TAU).sin() * 0.4 * env;
        let s = ((tone + noise) * 20_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn a_file_that_arrives_later_is_analysed_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube());
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(50, 300));
    wait_for(&rt, "the stream analysis", |s| s.analysis.streams_total == 3 && s.analysis.streams_done == 3);
    let track = lib.track_by_remote(&find(&yt, "yt-gruppa-1").id).unwrap().unwrap();
    let from_stream = lib.get_analysis(track).unwrap().unwrap();
    assert_eq!(from_stream.source, AnalysisSource::Youtube);
    drop(rt);

    // The file lands on the same track (as a download does).
    let wav = dir.path().join("gruppa.wav");
    write_wav(&wav, 6.0);
    lib.add_source(&Source::new(track, SourceKind::LocalFile, wav.to_string_lossy().into_owned(), Codec::Wav))
        .unwrap();
    let (rt, _) = start(&lib, vec![yt.clone()], Prefs::default(), schedule(50, 300));
    wait_until("the file's analysis", || source_of(&lib, track) == Some(AnalysisSource::File));
    assert_ne!(lib.get_analysis(track).unwrap().unwrap().bliss_vector, from_stream.bliss_vector);
    wait_for(&rt, "the progress", |s| {
        (s.analysis.files_done, s.analysis.files_total, s.analysis.streams_total) == (1, 1, 2)
    });
    drop(rt);
}

#[test]
fn nothing_is_downloaded_or_previewed_in_the_background_while_youtube_music_plays() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube());
    let track = lib.upsert_stream_track(&find(&yt, "yt-gruppa-1")).unwrap();
    let off = Prefs { autoplay: false, stream_analysis: false, ..Prefs::default() };
    let (rt, asked) = start(&lib, vec![yt.clone()], off, schedule(50, 300));
    rt.send(Command::PlayTracks {
        tracks: vec![track],
        start: 0,
        context: "test".into(),
    });
    wait_until("the play request", || !asked.lock().unwrap().is_empty());
    let id = asked.lock().unwrap()[0].id;
    rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    wait_for(&rt, "the playing state", |s| s.player.status == nikgob_view::Status::Playing);
    wait_for(&rt, "the likes", |s| s.analysis.streams_total >= 3);
    rt.send(Command::SetPrefs(Box::new(Prefs { autoplay: false, stream_analysis: true, ..Prefs::default() })));
    // The player's requests are the listener's: none in the background.
    std::thread::sleep(Duration::from_millis(800));
    assert!(yt.opened().is_empty(), "{:?}", yt.opened());
    assert_eq!(rt.state().analysis.streams_done, 0);

    // Paused by the user: the background goes on.
    rt.send(Command::Pause);
    wait_for(&rt, "the analysis", |s| s.analysis.streams_done >= 1);
    assert!(!yt.opened().is_empty());
    drop(rt);
}

#[test]
fn a_lookup_answered_after_youtube_music_began_to_play_downloads_nothing() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    // A song only YouTube Music has (Spotify's likes would win it otherwise).
    let yt = Arc::new(youtube().with_track("solo", "Solo", "Nobody", 200).logged_out());
    let playable = lib.upsert_stream_track(&find(&yt, "yt-x-solo")).unwrap();
    // A slow network: the analysis's lookup of a Spotify like is still on
    // its way when YouTube Music starts to play.
    yt.delay("search_kind", Duration::from_millis(700));
    yt.delay("find_isrc", Duration::from_millis(700));
    let streams: Vec<Arc<dyn StreamService>> = vec![sp.clone(), yt.clone()];
    let (rt, asked) = start(&lib, streams, Prefs { autoplay: false, ..Prefs::default() }, schedule(50, 300));
    wait_until("a lookup on its way", || yt.calls("search_kind") + yt.calls("find_isrc") >= 1);
    rt.send(Command::PlayTracks {
        tracks: vec![playable],
        start: 0,
        context: "test".into(),
    });
    wait_until("the play request", || !asked.lock().unwrap().is_empty());
    let first = asked.lock().unwrap()[0].clone();
    assert!(matches!(&first.source, PlaySource::Stream { service: Service::Youtube, .. }), "{:?}", first.source);
    rt.playback_sender().send(PlaybackEvent::Started { id: first.id }).unwrap();
    // The answer comes while it plays: nothing is downloaded.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(yt.opened().is_empty(), "{:?}", yt.opened());

    rt.send(Command::Pause);
    wait_for(&rt, "the analysis", |s| s.analysis.streams_done >= 1);
    drop(rt);
}


#[test]
fn nothing_is_downloaded_while_the_next_song_needs_youtube_music() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube());
    // A file plays; the next song only Spotify has, and Spotify is not
    // here: YouTube Music looks it up and plays it.
    let file = lib.upsert_track(&nikgob_core::model::Track::new("A file", nikgob_core::model::ArtistCredit::single("Someone", None))).unwrap();
    lib.add_source(&Source::new(file.id, SourceKind::LocalFile, "Music/a-file.opus", Codec::Opus)).unwrap();
    let next = lib.upsert_stream_track(&find(&FakeService::spotify(), "sp-discovery-1")).unwrap();
    let off = Prefs { autoplay: false, stream_analysis: false, ..Prefs::default() };
    let (rt, asked) = start(&lib, vec![yt.clone()], off, schedule(50, 300));
    rt.send(Command::PlayTracks {
        tracks: vec![file.id, next],
        start: 0,
        context: "test".into(),
    });
    wait_until("the play request", || !asked.lock().unwrap().is_empty());
    let id = asked.lock().unwrap()[0].id;
    rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    wait_for(&rt, "the playing state", |s| s.player.status == nikgob_view::Status::Playing);
    wait_for(&rt, "the likes", |s| s.analysis.streams_total >= 3);
    rt.send(Command::SetPrefs(Box::new(Prefs { autoplay: false, stream_analysis: true, ..Prefs::default() })));
    std::thread::sleep(Duration::from_millis(800));
    assert!(yt.opened().is_empty(), "{:?}", yt.opened());
    rt.send(Command::Pause);
    wait_for(&rt, "the analysis", |s| s.analysis.streams_done >= 1);
    drop(rt);
}

/// A stream analysed before sketches were (its bliss current, no sketch)
/// gets one: from the disk cache when it plays (no request), and in the
/// background by a download — not while YouTube Music plays, then after.
#[test]
fn a_stream_analysed_before_sketches_gets_one() {
    let current = |lib: &Library, t: TrackId| lib.sketch_of(t).unwrap().is_some_and(|s| s.sketched_with == nikgob_recs::SKETCHED_WITH);
    let analysed = |lib: &Library, t: TrackId| {
        lib.put_analysis(&nikgob_core::model::Analysis { track_id: t, bliss_vector: vec![0.1; nikgob_recs::DIM], analyzed_with: WITH.to_owned(), source: AnalysisSource::Youtube }).unwrap();
    };
    // Played from the cache.
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube().logged_out());
    let track = lib.upsert_stream_track(&find(&yt, "yt-gruppa-1")).unwrap();
    analysed(&lib, track);
    drop(yt.open_audio("yt-gruppa-1").unwrap());
    yt.reset_calls();
    let (rt, asked) = start(&lib, vec![yt.clone()], Prefs { autoplay: false, ..Prefs::default() }, schedule(50, 300));
    rt.send(Command::PlayTracks { tracks: vec![track], start: 0, context: "test".into() });
    wait_until("the play request", || !asked.lock().unwrap().is_empty());
    let id = asked.lock().unwrap()[0].id;
    rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    rt.playback_sender().send(PlaybackEvent::Ended { id }).unwrap();
    wait_until("the sketch", || current(&lib, track));
    assert_eq!(yt.total_calls(), 0, "{:?}", yt.all_calls());
    drop(rt);

    // In the background: a liked stream, analysed, without a sketch.
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(youtube().logged_out());
    let liked = lib.upsert_stream_track(&find(&yt, "yt-firstlove-1")).unwrap();
    lib.like(liked).unwrap();
    analysed(&lib, liked);
    let playing = lib.upsert_stream_track(&find(&yt, "yt-gruppa-1")).unwrap();
    let (rt, asked) = start(&lib, vec![yt.clone()], Prefs { autoplay: false, ..Prefs::default() }, schedule(50, 300));
    rt.send(Command::PlayTracks { tracks: vec![playing], start: 0, context: "test".into() });
    wait_until("the play request", || !asked.lock().unwrap().is_empty());
    let id = asked.lock().unwrap()[0].id;
    rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert!(!current(&lib, liked), "nothing while YouTube Music plays");
    assert!(yt.opened().iter().all(|v| v != "yt-firstlove-1"), "{:?}", yt.opened());
    rt.send(Command::Pause);
    wait_until("the background sketch", || current(&lib, liked));
    drop(rt);
}
