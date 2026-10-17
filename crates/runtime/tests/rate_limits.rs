//! Rate limits are waited out, not shown as failures (docs/stage3.md, the
//! rate limits): a search section that a service rate-limited loads and is
//! asked again when the pause ends (for the current query only, at most
//! three times); like and playlist checks try again when the pause ends
//! instead of at their next interval. Both the pause a service reports
//! (`Retry-After`) and the one it only keeps itself (`paused_until`, as
//! YouTube Music does after a 429 without one) count.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{FakeService, PlaylistSyncMode, RemoteId, Service, StreamService, StreamTrack};
use nikgob_library::{Library, PlaylistRemote};
use nikgob_runtime::{PlayRequest, Playback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs, StreamSearchSection, ViewState};

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
            Service::Youtube => "yt",
        };
        linked(&lib, RemoteId::new(service, format!("{p}-pl-evening")));
        match service {
            Service::Spotify => svc.rate_limit_next("my_playlists", 1, Some(Duration::from_millis(200))),
            Service::Youtube => {
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
