//! How many calls the runtime makes to the services in the scenarios of the
//! stage 3 request audit ("Session cost"). Every call of a fake stands for
//! at least one request of the real service; the Spotify crate's own tests
//! count the HTTP requests behind them. Run with `--nocapture` to see the
//! numbers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::{
    FakeService, PlaylistSyncMode, RemoteId, Service, StreamService, StreamTrack,
};
use nikgob_library::{Library, PlaylistRemote};
use nikgob_runtime::{PlayRequest, Playback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs, StreamPage};

struct Quiet(Arc<Mutex<Vec<PlayRequest>>>);

impl Playback for Quiet {
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

fn wait_for(what: &str, secs: u64, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Short intervals, no jitter.
fn quick() -> StreamSchedule {
    StreamSchedule {
        likes_every: Duration::from_secs(3600),
        sync_every: Duration::from_secs(3600),
        sync_first: (Duration::from_secs(3600), Duration::from_secs(3600)),
        sync_stagger: Duration::ZERO,
        like_delay: Duration::from_millis(50),
        enrich_pause: (Duration::ZERO, Duration::ZERO),
        tick: Duration::from_millis(100),
        jitter: 0.0,
        // «Новое для тебя» stays out of these counts.
        discover_gap: (Duration::from_secs(3600), Duration::from_secs(3600)),
        ..StreamSchedule::default()
    }
}

struct Rig {
    rt: Runtime,
    asked: Arc<Mutex<Vec<PlayRequest>>>,
}

fn start(lib: &Library, streams: &[Arc<FakeService>], schedule: StreamSchedule) -> Rig {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams: streams.iter().map(|s| s.clone() as Arc<dyn StreamService>).collect(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Quiet(asked.clone())),
        Arc::new(|| {}),
        schedule,
    );
    Rig { rt, asked }
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

fn report(what: &str, fakes: &[&Arc<FakeService>]) {
    for f in fakes {
        eprintln!("[{what}] {}: {:?}", f.service(), f.all_calls());
    }
}

#[test]
fn an_app_start_asks_each_service_for_one_page_of_likes() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, &[sp.clone(), yt.clone()]);
    // The real schedule: the first playlist check is minutes away.
    let rig = start(&lib, &[sp.clone(), yt.clone()], StreamSchedule::default());
    wait_for("the like checks", 10, || sp.calls("liked_head") == 1 && yt.calls("liked_head") == 1);
    std::thread::sleep(Duration::from_millis(300));
    report("start", &[&sp, &yt]);
    // Before: liked() (every page) + my_playlists() per service at once.
    assert_eq!(sp.total_calls(), 1, "{:?}", sp.all_calls());
    assert_eq!(yt.total_calls(), 1, "{:?}", yt.all_calls());
    drop(rig);
}

#[test]
fn a_like_check_with_nothing_new_is_one_call() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    let schedule = StreamSchedule {
        likes_every: Duration::from_millis(300),
        ..quick()
    };
    // The first start reads the whole list once (an import)…
    let _rig = start(&lib, std::slice::from_ref(&sp), schedule);
    wait_for("the first pull", 10, || sp.calls("liked") == 1);
    // …then every check is the head alone.
    wait_for("three checks", 10, || sp.calls("liked_head") >= 3);
    report("likes, nothing new", &[&sp]);
    assert_eq!(sp.calls("liked"), 1, "no full read while nothing changed");
    // A like on the service side: the head brings it, still no full read.
    let before = lib.liked_count().unwrap();
    sp.set_liked_remotely("sp-homogenic-3", true);
    wait_for("the new like", 10, || lib.liked_count().unwrap() > before);
    assert_eq!(sp.calls("liked"), 1);
    // An unlike there: the counts differ, the whole list is read once.
    sp.set_liked_remotely("sp-homogenic-3", false);
    wait_for("the full read", 10, || sp.calls("liked") == 2);
    wait_for("the unlike", 10, || lib.liked_count().unwrap() == before);
}

#[test]
fn a_playlist_check_with_no_changes_reads_only_the_list() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify());
    pulled_before(&lib, std::slice::from_ref(&sp));
    let local = lib.create_playlist("Вечер").unwrap();
    let ids: Vec<String> = ["sp-gruppa-1", "sp-firstlove-3", "sp-homogenic-2", "sp-discovery-3"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let now = nikgob_library::now_ms();
    lib.put_playlist_remote(&PlaylistRemote {
        playlist_id: local.id,
        remote: RemoteId::new(Service::Spotify, "sp-pl-evening"),
        sync_mode: PlaylistSyncMode::Follow,
        remote_version: Some("1".into()),
        synced_at: Some(now),
        base_json: Some(serde_json::to_string(&ids).unwrap()),
    })
    .unwrap();
    // Someone else's playlist the account does not list: its version alone.
    let theirs = lib.create_playlist("Чужой").unwrap();
    lib.put_playlist_remote(&PlaylistRemote {
        playlist_id: theirs.id,
        remote: RemoteId::new(Service::Spotify, "sp-pl-gone-from-list"),
        sync_mode: PlaylistSyncMode::Follow,
        remote_version: Some("1".into()),
        synced_at: Some(now),
        base_json: Some("[]".into()),
    })
    .unwrap();
    let schedule = StreamSchedule {
        sync_first: (Duration::ZERO, Duration::ZERO),
        sync_every: Duration::from_millis(300),
        ..quick()
    };
    let _rig = start(&lib, std::slice::from_ref(&sp), schedule);
    wait_for("three checks", 10, || sp.calls("my_playlists") >= 3);
    std::thread::sleep(Duration::from_millis(100));
    report("sync, no changes", &[&sp]);
    assert_eq!(sp.calls("playlist"), 0, "nothing changed: no playlist is read");
    assert!(sp.calls("playlist_version") >= 2, "the unlisted one: its version alone");
}

#[test]
fn an_album_opened_twice_and_played_is_fetched_once() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, std::slice::from_ref(&yt));
    let rig = start(&lib, std::slice::from_ref(&yt), quick());
    let album = RemoteId::new(Service::Youtube, "yt-alb-gruppa");
    let page = StreamPage::Album(album.clone());
    for _ in 0..2 {
        rig.rt.send(Command::LoadStream(page.clone()));
        wait_for("the page", 10, || {
            rig.rt
                .state()
                .stream_page
                .as_ref()
                .is_some_and(|p| p.page == page && !p.loading && !p.tracks.is_empty())
        });
        rig.rt.send(Command::LoadStream(StreamPage::Artist(RemoteId::new(Service::Youtube, "yt-art-kino"))));
    }
    rig.rt.send(Command::PlayStreamAlbum {
        id: album,
        start: None,
        shuffle: false,
    });
    wait_for("playback", 10, || !rig.asked.lock().unwrap().is_empty());
    report("album twice + play", &[&yt]);
    assert_eq!(yt.calls("album"), 1, "was 3 (2 opens + play)");
    assert_eq!(yt.calls("artist"), 1, "the artist page is kept too");
}

#[test]
fn liking_a_twelve_track_album_is_one_call() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().with_album("big", "Big", 12));
    pulled_before(&lib, std::slice::from_ref(&sp));
    let rig = start(&lib, std::slice::from_ref(&sp), quick());
    let album = RemoteId::new(Service::Spotify, "sp-alb-big");
    rig.rt.send(Command::LoadStream(StreamPage::Album(album.clone())));
    wait_for("the page", 10, || rig.rt.state().stream_page.as_ref().is_some_and(|p| p.tracks.len() == 12));
    rig.rt.send(Command::LikeStreamAlbum { album: album.clone(), on: true });
    wait_for("the likes sent", 10, || sp.liked_ids().len() >= 15);
    std::thread::sleep(Duration::from_millis(300));
    report("like a 12-track album", &[&sp]);
    assert_eq!(sp.calls("set_liked_many"), 1, "was 12 requests");
    assert_eq!(sp.calls("set_liked"), 0);
    // Liked and unliked again within the delay: nothing is sent.
    rig.rt.send(Command::LikeStreamAlbum { album: album.clone(), on: false });
    rig.rt.send(Command::LikeStreamAlbum { album, on: true });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(sp.calls("set_liked_many"), 1, "a like and its unlike cancel out");
}

#[test]
fn twenty_spotify_tracks_are_looked_up_on_youtube_once() {
    let lib = Library::open_in_memory().unwrap();
    // Spotify without sound: its tracks play from YouTube Music.
    let sp = Arc::new(FakeService::spotify().with_album("solo", "Solo Thing", 5).without_audio());
    let yt = Arc::new(FakeService::youtube());
    pulled_before(&lib, &[sp.clone(), yt.clone()]);
    let tracks: Vec<StreamTrack> = sp.tracks().into_iter().take(20).collect();
    assert_eq!(tracks.len(), 20);
    let ids: Vec<TrackId> = tracks.iter().map(|t| lib.upsert_stream_track(t).unwrap()).collect();
    let settled = |lib: &Library, t: TrackId| {
        lib.sources_of_track(t).unwrap().iter().any(|s| s.kind == SourceKind::Youtube)
            || lib.cache_get(&format!("yt_miss.{t}")).unwrap().is_some()
    };
    {
        let rig = start(&lib, &[sp.clone(), yt.clone()], quick());
        wait_for("the like checks", 10, || yt.calls("liked_head") == 1);
        yt.reset_calls();
        for &t in &ids {
            rig.rt.send(Command::PlayTracks {
                tracks: vec![t],
                start: 0,
                context: "test".into(),
            });
            wait_for("the lookup", 10, || settled(&lib, t));
        }
        report("resolve 20", &[&yt]);
        let found = ids
            .iter()
            .filter(|t| lib.sources_of_track(**t).unwrap().iter().any(|s| s.kind == SourceKind::Youtube))
            .count();
        eprintln!("[resolve 20] {found} found, {} missed", 20 - found);
        assert_eq!(yt.calls("search"), 0, "the full five-category search is not used");
        // Before: find_isrc + 1–2 full searches (5 requests each) per track.
        assert!(yt.total_calls() <= 20 * 5, "{:?}", yt.all_calls());
        yt.reset_calls();
        // Played again: the misses are not asked again…
        for &t in &ids {
            rig.rt.send(Command::PlayTracks {
                tracks: vec![t],
                start: 0,
                context: "test".into(),
            });
        }
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(yt.total_calls(), 0, "replays: {:?}", yt.all_calls());
    }
    // …not even after a restart.
    let rig = start(&lib, &[sp.clone(), yt.clone()], quick());
    wait_for("the like checks", 10, || yt.calls("liked_head") == 1);
    yt.reset_calls();
    for &t in &ids {
        rig.rt.send(Command::PlayTracks {
            tracks: vec![t],
            start: 0,
            context: "test".into(),
        });
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(yt.total_calls(), 0, "after a restart: {:?}", yt.all_calls());
}

#[test]
fn completing_three_hundred_imported_tracks_is_six_calls() {
    let lib = Library::open_in_memory().unwrap();
    let sp = Arc::new(FakeService::spotify().with_album("big", "Big", 300));
    pulled_before(&lib, std::slice::from_ref(&sp));
    // As an export file makes them: id and title, no length.
    let bare: Vec<StreamTrack> = sp
        .tracks()
        .into_iter()
        .filter(|t| t.id.id.starts_with("sp-big-"))
        .map(|t| StreamTrack::new(t.id.clone(), t.title.clone()))
        .collect();
    assert_eq!(bare.len(), 300);
    lib.ensure_stream_tracks(&bare).unwrap();
    let _rig = start(&lib, std::slice::from_ref(&sp), quick());
    wait_for("the batches", 20, || sp.calls("tracks") >= 6);
    std::thread::sleep(Duration::from_millis(500));
    report("enrich 300", &[&sp]);
    assert_eq!(sp.calls("tracks"), 6, "was 300 single lookups");
    assert_eq!(sp.calls("track"), 0);
}
