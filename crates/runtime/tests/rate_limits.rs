//! Rate limits are waited out, not shown as failures (the
//! rate limits): a search section that a service rate-limited loads and is
//! asked again when the pause ends (for the current query only, at most
//! three times); like and playlist checks try again when the pause ends
//! instead of at their next interval. Both the pause a service reports
//! (`Retry-After`) and the one it only keeps itself (`paused_until`, as
//! YouTube Music does after a 429 without one) count. A pause (YouTube's
//! bot check) does not run the play queue down: what plays now comes first
//! and the waiting tracks stay queued, or the player waits and goes on by
//! itself; nothing of it is a skip.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Codec, EventType, Source, SourceKind, TrackId};
use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service, StreamService, StreamTrack};
use nikgob_library::{Library, PlaylistRemote};
use nikgob_runtime::{PlayRequest, PlaySource, Playback, PlaybackEvent, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs, Status, StreamSearchSection, ViewState};

struct Quiet;

impl Playback for Quiet {
    fn play(&mut self, _: PlayRequest) {}
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

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// No periodic work unless a test asks for it, no jitter.
fn quick() -> StreamSchedule {
    StreamSchedule {
        likes_every: Duration::from_secs(3600),
        sync_every: Duration::from_secs(3600),
        sync_first: (Duration::from_secs(3600), Duration::from_secs(3600)),
        sync_stagger: Duration::ZERO,
        like_delay: Duration::from_millis(50),
        enrich_pause: (Duration::ZERO, Duration::ZERO),
        tick: Duration::from_millis(50),
        jitter: 0.0,
        ..StreamSchedule::default()
    }
}

fn start(lib: &Library, streams: &[Arc<FakeService>], schedule: StreamSchedule) -> Runtime {
    Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams: streams.iter().map(|s| s.clone() as Arc<dyn StreamService>).collect(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Quiet),
        Arc::new(|| {}),
        schedule,
    )
}

/// A library whose likes were pulled before (not the first start).
fn pulled_before(lib: &Library, services: &[Arc<FakeService>]) {
    let now = nikgob_library::now_ms();
    for s in services {
        let liked: Vec<StreamTrack> = s.tracks().into_iter().filter(|t| s.liked_ids().contains(&t.id.id)).collect();
        lib.take_remote_likes(&liked, &[], now).unwrap();
        lib.set_likes_pulled(s.service(), now).unwrap();
        lib.cache_put(&format!("likes_full.{}", s.service()), now, "").unwrap();
    }
}

fn section(s: &ViewState, service: Service) -> Option<StreamSearchSection> {
    s.search.streams.iter().find(|x| x.service == service).cloned()
}

fn filled(x: &StreamSearchSection) -> bool {
    !x.searching && x.error.is_none() && x.note.is_none() && !(x.tracks.is_empty() && x.albums.is_empty())
}

/// Searches `query` and polls the view until `done`; returns every
/// distinct Spotify section seen on the way.
fn search_until(
    rt: &Runtime,
    query: &str,
    service: Service,
    done: impl Fn(&ViewState) -> bool,
) -> Vec<StreamSearchSection> {
    rt.send(Command::Search(query.into()));
    let seen = Mutex::new(Vec::<StreamSearchSection>::new());
    wait_for("the search", 10, || {
        let s = rt.state();
        if s.search.query.trim() != query {
            return false;
        }
        if let Some(x) = section(&s, service) {
            let mut seen = seen.lock().unwrap();
            if seen.last() != Some(&x) {
                seen.push(x);
            }
        }
        done(&s)
    });
    seen.into_inner().unwrap()
}

#[test]
fn a_rate_limited_search_waits_and_fills_its_section_by_itself() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, &[sp.clone(), yt.clone()]);
    let rt = start(&lib, &[sp.clone(), yt.clone()], quick());
    // Two 429s with Retry-After 0.3 s, then an answer.
    sp.rate_limit_next("search", 2, Some(Duration::from_millis(300)));
    let yt_first = Mutex::new(false);
    let seen = search_until(&rt, "daft", Service::Spotify, |s| {
        let sp_x = section(s, Service::Spotify);
        let yt_x = section(s, Service::Youtube);
        if sp_x.as_ref().is_some_and(|x| x.searching) && yt_x.as_ref().is_some_and(|x| !x.searching) {
            *yt_first.lock().unwrap() = true;
        }
        sp_x.is_some_and(|x| filled(&x))
    });
    assert_eq!(sp.calls("search"), 3, "asked twice more, by itself");
    let waiting: Vec<_> = seen.iter().filter(|x| x.note.is_some()).collect();
    assert!(!waiting.is_empty(), "the wait was said: {seen:?}");
    for x in &waiting {
        assert!(x.searching, "the section keeps loading while it waits");
        assert!(x.error.is_none(), "a rate limit is not an error");
        assert_eq!(x.note.as_deref(), Some("Spotify отвечает с задержкой — результаты появятся сами"));
    }
    assert!(seen.iter().all(|x| x.error.is_none()), "never an error: {seen:?}");
    assert!(*yt_first.lock().unwrap(), "YouTube Music does not wait for Spotify");
}

#[test]
fn a_query_replaced_while_it_waits_is_not_asked_again() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    pulled_before(&lib, std::slice::from_ref(&sp));
    let rt = start(&lib, std::slice::from_ref(&sp), quick());
    sp.rate_limit_next("search", 1, Some(Duration::from_millis(400)));
    search_until(&rt, "daft", Service::Spotify, |s| {
        section(s, Service::Spotify).is_some_and(|x| x.note.is_some())
    });
    assert_eq!(sp.calls("search"), 1);
    // A new query while «daft» waits: only the new one is asked.
    search_until(&rt, "кино", Service::Spotify, |s| section(s, Service::Spotify).is_some_and(|x| filled(&x)));
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(sp.calls("search"), 2, "«daft» was not asked again");
    assert_eq!(rt.state().search.query, "кино");
}

#[test]
fn a_search_is_asked_again_at_most_three_times() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, &[sp.clone(), yt.clone()]);
    let rt = start(&lib, &[sp.clone(), yt.clone()], quick());
    sp.rate_limit_next("search", 100, Some(Duration::from_millis(100)));
    search_until(&rt, "daft", Service::Spotify, |s| {
        section(s, Service::Spotify).is_some_and(|x| !x.searching)
    });
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(sp.calls("search"), 4, "the first ask and three more");
    let s = rt.state();
    let x = section(&s, Service::Spotify).unwrap();
    assert!(x.error.is_none(), "still not an error");
    assert_eq!(x.note.as_deref(), Some("Spotify сейчас перегружен — ищу в YouTube Music"));
    assert!(section(&s, Service::Youtube).is_some_and(|y| !y.searching));
}

#[test]
fn a_long_pause_says_the_service_is_overloaded_and_the_other_answers() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, &[sp.clone(), yt.clone()]);
    let rt = start(&lib, &[sp.clone(), yt.clone()], quick());
    yt.rate_limit_next("search", 1, Some(Duration::from_secs(600)));
    search_until(&rt, "кино", Service::Youtube, |s| {
        section(s, Service::Youtube).is_some_and(|x| x.note.is_some())
            && section(s, Service::Spotify).is_some_and(|x| filled(&x))
    });
    let x = section(&rt.state(), Service::Youtube).unwrap();
    assert!(!x.searching && x.error.is_none());
    let note = x.note.unwrap();
    assert!(
        note.starts_with("YouTube Music сейчас перегружен до ") && note.ends_with(" — ищу в Spotify"),
        "{note}"
    );
}

#[test]
fn a_youtube_pause_without_retry_after_is_waited_out_too() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, std::slice::from_ref(&yt));
    let rt = start(&lib, std::slice::from_ref(&yt), quick());
    // A 429 without Retry-After; the service keeps the pause itself.
    yt.rate_limit_next_paused("search", 1, Duration::from_millis(300));
    let seen = search_until(&rt, "кино", Service::Youtube, |s| section(s, Service::Youtube).is_some_and(|x| filled(&x)));
    assert_eq!(yt.calls("search"), 2);
    assert!(
        seen.iter().any(|x| x.searching
            && x.note.as_deref() == Some("YouTube Music отвечает с задержкой — результаты появятся сами")),
        "{seen:?}"
    );
}

#[test]
fn the_first_likes_after_a_login_come_after_the_pause_not_the_next_interval() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    // The very first pull (an import): three 429s in a row (29 s, 1 s, 28 s
    // in the real log), shortened.
    sp.rate_limit_next("liked", 3, Some(Duration::from_millis(150)));
    let t = Instant::now();
    let _rt = start(&lib, std::slice::from_ref(&sp), quick());
    wait_for("the likes", 10, || lib.liked_count().unwrap() > 0);
    // 1×, 2×, 4× the pause: 150 + 300 + 600 ms.
    assert!(t.elapsed() >= Duration::from_millis(1050), "never before the pause: {:?}", t.elapsed());
    assert_eq!(sp.calls("liked"), 4);
    assert_eq!(lib.liked_count().unwrap(), sp.liked_ids().len() as u32);
}

#[test]
fn youtube_likes_wait_for_its_own_pause() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, std::slice::from_ref(&yt));
    yt.set_liked_remotely("yt-homogenic-3", true);
    yt.rate_limit_next_paused("liked_head", 1, Duration::from_millis(300));
    let before = lib.liked_count().unwrap();
    let t = Instant::now();
    let _rt = start(&lib, std::slice::from_ref(&yt), quick());
    wait_for("the new like", 10, || lib.liked_count().unwrap() > before);
    assert!(t.elapsed() >= Duration::from_millis(300), "{:?}", t.elapsed());
    assert_eq!(yt.calls("liked_head"), 2);
}

#[test]
fn a_like_check_that_stays_limited_goes_back_to_its_interval() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    pulled_before(&lib, std::slice::from_ref(&sp));
    sp.rate_limit_next("liked_head", 100, Some(Duration::from_millis(50)));
    let _rt = start(&lib, std::slice::from_ref(&sp), quick());
    wait_for("the retries", 10, || sp.calls("liked_head") == 4);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(sp.calls("liked_head"), 4, "three retries, then the hourly check");
}

fn linked(lib: &Library, remote: RemoteId) {
    let local = lib.create_playlist("Вечер").unwrap();
    lib.put_playlist_remote(&PlaylistRemote {
        playlist_id: local.id,
        remote,
        sync_mode: PlaylistSyncMode::Follow,
        remote_version: Some("0".into()),
        synced_at: Some(nikgob_library::now_ms()),
        base_json: Some("[]".into()),
    })
    .unwrap();
}

#[test]
fn a_rate_limited_playlist_check_runs_again_when_the_pause_ends() {
    for fake in [FakeService::spotify(), FakeService::youtube()] {
        let lib = Library::open_in_memory().unwrap();
        let svc = Arc::new(fake);
        let service = svc.service();
        pulled_before(&lib, std::slice::from_ref(&svc));
        let p = match service {
            Service::Spotify => "sp",
            Service::Youtube | Service::Deezer | Service::Soundcloud => "yt",
        };
        linked(&lib, RemoteId::new(service, format!("{p}-pl-evening")));
        match service {
            Service::Spotify => svc.rate_limit_next("my_playlists", 1, Some(Duration::from_millis(200))),
            Service::Youtube | Service::Deezer | Service::Soundcloud => {
                svc.rate_limit_next_paused("my_playlists", 1, Duration::from_millis(200));
            }
        }
        let schedule = StreamSchedule {
            sync_first: (Duration::ZERO, Duration::ZERO),
            ..quick()
        };
        let t = Instant::now();
        let _rt = start(&lib, std::slice::from_ref(&svc), schedule);
        wait_for("the second check", 10, || svc.calls("my_playlists") == 2);
        assert!(t.elapsed() >= Duration::from_millis(200), "{service}: {:?}", t.elapsed());
        // The version changed ("0" → "1"): the playlist is read once.
        wait_for("the fetch", 10, || svc.calls("playlist") == 1);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(svc.calls("my_playlists"), 2, "{service}: then the normal interval");
    }
}

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
    fn seek(&mut self, at: Duration) {
        self.1.lock().unwrap().push(at);
    }
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
}

/// A runtime on `yt` alone that records the engine's requests (no
/// autoplay: nothing asks for a radio).
fn start_playing(lib: &Library, yt: &Arc<FakeService>) -> (Runtime, Arc<Mutex<Vec<PlayRequest>>>) {
    let (rt, asked, _) = start_on(lib, vec![yt.clone() as Arc<dyn StreamService>]);
    (rt, asked)
}

/// [`start_playing`] on `streams`, with the engine's seeks.
#[allow(clippy::type_complexity)]
fn start_on(lib: &Library, streams: Vec<Arc<dyn StreamService>>) -> (Runtime, Arc<Mutex<Vec<PlayRequest>>>, Arc<Mutex<Vec<Duration>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seeks = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs: Prefs { autoplay: false, ..Prefs::default() },
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(asked.clone(), seeks.clone())),
        Arc::new(|| {}),
        quick(),
    );
    (rt, asked, seeks)
}

/// YouTube's bot check as the service keeps it: every call fails for
/// `pause` from now on.
fn bot_check(yt: &FakeService, pause: Duration) {
    yt.rate_limit_next_paused("open_audio", 1, pause);
    assert!(yt.open_audio("yt-gruppa-1").is_err());
    assert!(yt.paused_until().is_some());
}

/// A library track of the fake's `id` (a stream only).
fn stream_track(lib: &Library, yt: &FakeService, id: &str) -> TrackId {
    let t = yt.tracks().into_iter().find(|t| t.id.id == id).unwrap();
    lib.upsert_stream_track(&t).unwrap()
}

/// A library track with a file.
fn file_track(lib: &Library, yt: &FakeService, id: &str, file: &str) -> TrackId {
    let t = stream_track(lib, yt, id);
    lib.add_source(&Source::new(t, SourceKind::LocalFile, file, Codec::Opus)).unwrap();
    t
}

fn asked_at(asked: &Mutex<Vec<PlayRequest>>, n: usize) -> PlayRequest {
    wait_for("the engine's request", 10, || asked.lock().unwrap().len() > n);
    asked.lock().unwrap()[n].clone()
}

fn is_file(r: &PlayRequest, name: &str) -> bool {
    matches!(&r.source, PlaySource::File(p) if p.ends_with(name))
}

fn is_stream(r: &PlayRequest, id: &str) -> bool {
    matches!(&r.source, PlaySource::Stream { service: Service::Youtube, id: x } if x == id)
}

fn upcoming(rt: &Runtime) -> Vec<TrackId> {
    rt.state().queue.next.iter().map(|e| e.track.id).collect()
}

/// The pause toasts seen so far (by id).
fn note_toasts(rt: &Runtime, seen: &mut HashSet<u64>) {
    for t in &rt.state().toasts {
        if t.text.contains("просит подождать") {
            seen.insert(t.id);
        }
    }
}

fn skips(lib: &Library, t: TrackId) -> usize {
    lib.events_of_track(t, 50).unwrap().iter().filter(|e| e.event_type == EventType::Skip).count()
}

#[test]
fn a_bot_check_does_not_run_the_queue_down_a_file_plays_and_the_rest_waits() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let first = file_track(&lib, &yt, "yt-gruppa-2", "Music/first.opus");
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    let b = stream_track(&lib, &yt, "yt-firstlove-1");
    let file = file_track(&lib, &yt, "yt-firstlove-2", "Music/later.opus");
    let (rt, asked) = start_playing(&lib, &yt);
    rt.send(Command::PlayTracks { tracks: vec![first, a, b, file], start: 0, context: "test".into() });
    let r0 = asked_at(&asked, 0);
    assert!(is_file(&r0, "first.opus"), "{:?}", r0.source);
    let pause = Duration::from_millis(1500);
    bot_check(&yt, pause);
    let paused_at = Instant::now();
    let tx = rt.playback_sender();
    let mut toasts = HashSet::new();

    // The file ends: the two YouTube songs cannot play now, the file after
    // them plays instead, and they stay queued right after it.
    tx.send(PlaybackEvent::Ended { id: r0.id }).unwrap();
    let r1 = asked_at(&asked, 1);
    assert!(is_file(&r1, "later.opus"), "the playable one, not a run down the queue: {:?}", r1.source);
    wait_for("the queue", 10, || upcoming(&rt) == vec![a, b]);
    note_toasts(&rt, &mut toasts);

    // It ends too: nothing near plays now, the player waits on the first
    // of them, paused, and asks nothing meanwhile.
    tx.send(PlaybackEvent::Ended { id: r1.id }).unwrap();
    wait_for("the wait", 10, || {
        let s = rt.state();
        s.player.status == Status::Paused && s.player.current.as_ref().is_some_and(|c| c.id == a)
    });
    assert_eq!(asked.lock().unwrap().len(), 2, "nothing handed over while paused");
    assert_eq!(upcoming(&rt), vec![b]);
    note_toasts(&rt, &mut toasts);

    // The pause lifts: it starts by itself.
    let r2 = asked_at(&asked, 2);
    assert!(paused_at.elapsed() >= pause.mul_f64(0.9), "not before the pause ends: {:?}", paused_at.elapsed());
    assert!(is_stream(&r2, "yt-gruppa-3"), "{:?}", r2.source);
    assert_eq!(rt.state().player.status, Status::Buffering);
    note_toasts(&rt, &mut toasts);
    assert_eq!(toasts.len(), 1, "one toast for the pause");

    // Neither waiting song was started or skipped while it waited.
    assert!(lib.events_of_track(b, 50).unwrap().is_empty(), "{:?}", lib.events_of_track(b, 50));
    let starts = lib.events_of_track(a, 50).unwrap().iter().filter(|e| e.event_type == EventType::Start).count();
    assert_eq!(starts, 1, "started once, after the pause");
    assert_eq!(skips(&lib, a), 0);
}

#[test]
fn a_stream_refused_by_a_bot_check_as_it_opens_is_no_skip_and_keeps_its_place() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    let b = stream_track(&lib, &yt, "yt-firstlove-1");
    let file = file_track(&lib, &yt, "yt-firstlove-2", "Music/later.opus");
    let (rt, asked) = start_playing(&lib, &yt);
    rt.send(Command::PlayTracks { tracks: vec![a, b, file], start: 0, context: "test".into() });
    let r0 = asked_at(&asked, 0);
    assert!(is_stream(&r0, "yt-gruppa-3"), "{:?}", r0.source);
    // The stream's own player request is the one bot-checked.
    bot_check(&yt, Duration::from_secs(600));
    rt.playback_sender()
        .send(PlaybackEvent::Error { id: r0.id, message: "stream: rate limited".into() })
        .unwrap();
    let r1 = asked_at(&asked, 1);
    assert!(is_file(&r1, "later.opus"), "{:?}", r1.source);
    wait_for("the queue", 10, || upcoming(&rt) == vec![a, b]);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(asked.lock().unwrap().len(), 2, "no run down the queue");
    for t in [a, b] {
        assert_eq!(skips(&lib, t), 0, "a rate limit is never a skip");
    }
    let toasts: Vec<String> = rt.state().toasts.iter().map(|t| t.text.clone()).filter(|t| t.contains("просит подождать")).collect();
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(toasts[0].starts_with("YouTube Music просит подождать до "), "{toasts:?}");
}

#[test]
fn the_users_own_pick_waits_for_the_pause_instead_of_being_passed_over() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    let file = file_track(&lib, &yt, "yt-firstlove-2", "Music/later.opus");
    let (rt, asked) = start_playing(&lib, &yt);
    bot_check(&yt, Duration::from_millis(800));
    rt.send(Command::PlayTracks { tracks: vec![a, file], start: 0, context: "test".into() });
    wait_for("the wait", 10, || {
        let s = rt.state();
        s.player.status == Status::Paused && s.player.current.as_ref().is_some_and(|c| c.id == a)
    });
    assert!(asked.lock().unwrap().is_empty());
    let r0 = asked_at(&asked, 0);
    assert!(is_stream(&r0, "yt-gruppa-3"), "{:?}", r0.source);
    assert_eq!(upcoming(&rt), vec![file]);
}

/// The waiting entry's place is kept: the player stands paused on it,
/// shown where it stopped.
fn waits_on(rt: &Runtime, t: TrackId) -> bool {
    let s = rt.state();
    s.player.status == Status::Paused && s.player.current.as_ref().is_some_and(|c| c.id == t)
}

#[test]
fn pause_pressed_while_waiting_for_the_service_keeps_it_paused() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    let (rt, asked) = start_playing(&lib, &yt);
    bot_check(&yt, Duration::from_millis(600));
    rt.send(Command::PlayTracks { tracks: vec![a], start: 0, context: "test".into() });
    wait_for("the wait", 10, || waits_on(&rt, a));
    // A media key's pause: the wake no longer starts it.
    rt.send(Command::Pause);
    std::thread::sleep(Duration::from_millis(1600));
    assert!(asked.lock().unwrap().is_empty(), "started against the pause");
    assert!(waits_on(&rt, a));
    // Play starts it at once now the pause is over.
    rt.send(Command::Play);
    assert!(is_stream(&asked_at(&asked, 0), "yt-gruppa-3"));
}

#[test]
fn a_stream_cut_midway_goes_on_from_there_after_a_track_in_between() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    let file = file_track(&lib, &yt, "yt-firstlove-2", "Music/later.opus");
    let (rt, asked, seeks) = start_on(&lib, vec![yt.clone() as Arc<dyn StreamService>]);
    rt.send(Command::PlayTracks { tracks: vec![a, file], start: 0, context: "test".into() });
    let r0 = asked_at(&asked, 0);
    let tx = rt.playback_sender();
    // (Its first report while it buffers is taken as it is.)
    tx.send(PlaybackEvent::Position { id: r0.id, position: Duration::from_secs(90) }).unwrap();
    wait_for("the position", 10, || rt.state().player.position >= Duration::from_secs(90));
    bot_check(&yt, Duration::from_millis(800));
    tx.send(PlaybackEvent::Error { id: r0.id, message: "stream: rate limited".into() }).unwrap();
    let r1 = asked_at(&asked, 1);
    assert!(is_file(&r1, "later.opus"), "{:?}", r1.source);
    std::thread::sleep(Duration::from_millis(900));
    // The file ends after the pause: the song goes on at 1:30.
    tx.send(PlaybackEvent::Ended { id: r1.id }).unwrap();
    let r2 = asked_at(&asked, 2);
    assert!(is_stream(&r2, "yt-gruppa-3"), "{:?}", r2.source);
    wait_for("the seek", 10, || seeks.lock().unwrap().contains(&Duration::from_secs(90)));
}

#[test]
fn play_pressed_while_a_cut_stream_waits_keeps_its_place() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    let (rt, asked, seeks) = start_on(&lib, vec![yt.clone() as Arc<dyn StreamService>]);
    rt.send(Command::PlayTracks { tracks: vec![a], start: 0, context: "test".into() });
    let r0 = asked_at(&asked, 0);
    let tx = rt.playback_sender();
    // (Its first report while it buffers is taken as it is.)
    tx.send(PlaybackEvent::Position { id: r0.id, position: Duration::from_secs(90) }).unwrap();
    wait_for("the position", 10, || rt.state().player.position >= Duration::from_secs(90));
    bot_check(&yt, Duration::from_millis(1200));
    tx.send(PlaybackEvent::Error { id: r0.id, message: "stream: rate limited".into() }).unwrap();
    wait_for("the wait", 10, || waits_on(&rt, a));
    assert_eq!(rt.state().player.position, Duration::from_secs(90), "shown where it stopped");
    // Play while the service still waits: it waits again, place kept.
    rt.send(Command::Play);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(waits_on(&rt, a));
    assert_eq!(rt.state().player.position, Duration::from_secs(90));
    // The pause lifts: from 1:30.
    let r1 = asked_at(&asked, 1);
    assert!(is_stream(&r1, "yt-gruppa-3"), "{:?}", r1.source);
    wait_for("the seek", 10, || seeks.lock().unwrap().contains(&Duration::from_secs(90)));
    assert_eq!(skips(&lib, a), 0);
}

#[test]
fn another_copy_of_the_track_goes_on_when_its_service_pauses() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let yt = Arc::new(FakeService::youtube());
    // One song on both services (the same ISRC).
    let t = stream_track(&lib, &yt, "yt-discovery-1");
    assert_eq!(stream_track(&lib, &sp, "sp-discovery-1"), t);
    let (rt, asked, seeks) = start_on(&lib, vec![sp.clone() as Arc<dyn StreamService>, yt.clone()]);
    rt.send(Command::PlayTracks { tracks: vec![t], start: 0, context: "test".into() });
    let r0 = asked_at(&asked, 0);
    assert!(matches!(&r0.source, PlaySource::Stream { service: Service::Spotify, .. }), "{:?}", r0.source);
    let tx = rt.playback_sender();
    // (Its first report while it buffers is taken as it is.)
    tx.send(PlaybackEvent::Position { id: r0.id, position: Duration::from_secs(30) }).unwrap();
    wait_for("the position", 10, || rt.state().player.position >= Duration::from_secs(30));
    sp.rate_limit_next_paused("open_audio", 1, Duration::from_secs(600));
    assert!(sp.open_audio("sp-discovery-1").is_err());
    tx.send(PlaybackEvent::Error { id: r0.id, message: "stream: rate limited".into() }).unwrap();
    let r1 = asked_at(&asked, 1);
    assert_eq!(r1.id, r0.id, "the same entry");
    assert!(is_stream(&r1, "yt-discovery-1"), "{:?}", r1.source);
    wait_for("the seek", 10, || seeks.lock().unwrap().contains(&Duration::from_secs(30)));
    assert_eq!(skips(&lib, t), 0);
}

#[test]
fn a_song_on_disk_plays_during_the_pause() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let a = stream_track(&lib, &yt, "yt-gruppa-3");
    // It played before: its whole audio is on disk.
    drop(yt.open_audio("yt-gruppa-3").unwrap());
    let (rt, asked) = start_playing(&lib, &yt);
    bot_check(&yt, Duration::from_secs(600));
    rt.send(Command::PlayTracks { tracks: vec![a], start: 0, context: "test".into() });
    assert!(is_stream(&asked_at(&asked, 0), "yt-gruppa-3"));
}
