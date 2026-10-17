//! A stream track replaced by hand with a picked song (docs/stage3.md §7
//! «Найти песню…»): only here the like stays the clip's; «И в YouTube Music»
//! moves the like there and the like checks stay calm about it; both come
//! back with «Отменить»; the clip repair never touches a track replaced by
//! hand; the scope chosen last is the next default. Fakes only: no network.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{FakeService, RemoteId, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs};

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn quick() -> StreamSchedule {
    StreamSchedule {
        likes_every: Duration::from_secs(3600),
        likes_fresh: Duration::ZERO,
        sync_every: Duration::from_secs(3600),
        sync_first: (Duration::from_secs(3600), Duration::from_secs(3600)),
        sync_stagger: Duration::ZERO,
        like_delay: Duration::from_millis(30),
        enrich_pause: (Duration::ZERO, Duration::ZERO),
        clip_gap: Duration::from_millis(20),
        tick: Duration::from_millis(20),
        jitter: 0.0,
        ..StreamSchedule::default()
    }
}

fn start_with(lib: &Library, streams: Vec<Arc<dyn StreamService>>, schedule: StreamSchedule, prefs_path: Option<std::path::PathBuf>) -> Runtime {
    Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
        schedule,
    )
}

fn check_likes(rt: &Runtime) {
    rt.send(Command::WindowState { visible: true, focused: false });
    rt.send(Command::WindowState { visible: true, focused: true });
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service.tracks().into_iter().find(|t| t.id.id == id).unwrap()
}

fn lonely() -> FakeService {
    FakeService::youtube().with_video("live", "Кино - Последний герой (Live)", "Кино", 300, None)
}

/// YouTube likes only `liked`, and the library pulled exactly these;
/// the repair already found no song for the clip.
fn liked_before(lib: &Library, yt: &FakeService, liked: &[&str]) {
    for id in yt.liked_ids() {
        yt.set_liked_remotely(&id, false);
    }
    for id in liked.iter().rev() {
        yt.set_liked_remotely(id, true);
    }
    let tracks: Vec<StreamTrack> = liked.iter().map(|id| find(yt, id)).collect();
    let now = nikgob_library::now_ms();
    lib.take_remote_likes(&tracks, &[], now).unwrap();
    lib.set_likes_pulled(Service::Youtube, now).unwrap();
    lib.cache_put("likes_full.youtube", now, "").unwrap();
    lib.mark_clip(&RemoteId::new(Service::Youtube, "yt-video-live")).unwrap();
    lib.cache_put("yt_clip.yt-video-live", now, "clip").unwrap();
    lib.cache_put("yt_clip_retry.v2", now, "").unwrap();
}

fn clip_track(lib: &Library) -> TrackId {
    lib.track_by_remote(&RemoteId::new(Service::Youtube, "yt-video-live")).unwrap().unwrap()
}

/// Opens the dialog for `track` with `query` and waits for its results.
fn search(rt: &Runtime, track: TrackId, query: &str) {
    rt.send(Command::FindSong { track, query: Some(query.into()) });
    wait_until("the results", || {
        rt.state().find_song.as_ref().is_some_and(|f| f.query == query && !f.loading && !f.results.is_empty())
    });
}

/// The «Отменить» of the last toast.
fn undo_of_last_toast(rt: &Runtime) -> Command {
    wait_until("the undo toast", || {
        rt.state().toasts.iter().any(|t| matches!(t.action, Some((_, Command::UndoReplace(_)))))
    });
    let s = rt.state();
    let t = s.toasts.iter().rev().find(|t| matches!(t.action, Some((_, Command::UndoReplace(_))))).expect("an undo toast");
    t.action.clone().unwrap().1
}

fn remote_likes(lib: &Library, track: TrackId) -> Vec<String> {
    lib.remote_likes_of(track).unwrap().into_iter().map(|l| l.remote.id).collect()
}

#[test]
fn the_dialog_opens_with_the_cleaned_title_and_lists_songs_only() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    liked_before(&lib, &yt, &["yt-video-live"]);
    let track = clip_track(&lib);
    let rt = start_with(&lib, vec![yt.clone()], quick(), None);
    rt.send(Command::FindSong { track, query: None });
    wait_until("the dialog", || rt.state().find_song.is_some());
    {
        let s = rt.state();
        let f = s.find_song.as_ref().unwrap();
        assert!(f.clip);
        assert_eq!(f.title, "Последний герой (Live)");
        assert_eq!(f.query, "Последний герой (Live) Кино");
        assert_eq!(f.duration, Some(Duration::from_secs(300)));
        assert_eq!(f.youtube_off, None, "signed in, liked there");
    }
    search(&rt, track, "Кино");
    let s = rt.state();
    let f = s.find_song.as_ref().unwrap();
    assert!(f.results.iter().all(|r| r.kind == nikgob_core::streaming::TrackKind::Song), "no clips");
    assert!(f.results.iter().any(|r| r.remote.id == "yt-gruppa-2"));
    // The library row shows the clip by its song title, the info keeps it.
    let row = s.library.tracks.iter().find(|t| t.id == track).cloned();
    if let Some(row) = row {
        assert_eq!(row.title, "Последний герой (Live)");
        assert!(row.clip);
    }
}

#[test]
fn only_here_the_like_stays_the_clips_and_undo_brings_the_clip_back() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    liked_before(&lib, &yt, &["yt-video-live", "yt-discovery-1"]);
    let track = clip_track(&lib);
    let rt = start_with(&lib, vec![yt.clone()], quick(), None);
    search(&rt, track, "Кино");
    rt.send(Command::ReplaceTrack {
        track,
        with: RemoteId::new(Service::Youtube, "yt-gruppa-2"),
        youtube: false,
    });
    wait_until("the replacement", || lib.get_track_row(track).unwrap().unwrap().title == "Закрой за мной дверь, я ухожу");
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert!(!row.is_clip());
    assert!(row.liked);
    assert_eq!(row.album.as_deref(), Some("Группа крови"));
    assert_eq!(row.youtube_id.as_deref(), Some("yt-gruppa-2"), "the song's audio");
    assert_eq!(remote_likes(&lib, track), ["yt-video-live"], "the like is still the clip's");
    let _ = undo_of_last_toast(&rt);
    assert!(rt.state().find_song.is_none(), "the dialog closed");
    {
        let s = rt.state();
        let toast = s.toasts.last().unwrap();
        assert_eq!(toast.text, "Заменено на «Закрой за мной дверь, я ухожу»");
        assert_eq!(toast.action.as_ref().unwrap().0, "Отменить");
    }
    // A like check changes nothing; nothing went to YouTube.
    let checks = yt.calls("liked_head") + yt.calls("liked");
    check_likes(&rt);
    wait_until("a check", || yt.calls("liked_head") + yt.calls("liked") > checks);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(yt.calls("set_liked_many"), 0);
    assert!(yt.liked_ids().contains(&"yt-video-live".to_owned()));
    assert!(lib.is_liked(track).unwrap());
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 2);
    assert_eq!(remote_likes(&lib, track), ["yt-video-live"]);

    rt.send(undo_of_last_toast(&rt));
    wait_until("the clip back", || lib.get_track_row(track).unwrap().unwrap().is_clip());
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert_eq!(row.title, "Кино - Последний герой (Live)");
    assert_eq!(row.youtube_id.as_deref(), Some("yt-video-live"));
    assert_eq!(remote_likes(&lib, track), ["yt-video-live"]);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(yt.calls("set_liked_many"), 0, "nothing to take back there");
}

#[test]
fn in_youtube_music_the_like_moves_to_the_song_stays_put_and_comes_back() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    liked_before(&lib, &yt, &["yt-video-live", "yt-discovery-1"]);
    let track = clip_track(&lib);
    let dir = tempfile::tempdir().unwrap();
    let prefs = dir.path().join("prefs.json");
    // The like waits a while in the queue: a check meanwhile still sees
    // the clip liked there and the song not.
    let schedule = StreamSchedule { like_delay: Duration::from_millis(600), ..quick() };
    let rt = start_with(&lib, vec![yt.clone()], schedule, Some(prefs.clone()));
    assert!(!rt.state().prefs.replace_in_youtube, "«Только здесь» at first");
    search(&rt, track, "Кино");
    rt.send(Command::ReplaceTrack {
        track,
        with: RemoteId::new(Service::Youtube, "yt-gruppa-2"),
        youtube: true,
    });
    wait_until("the replacement", || !lib.get_track_row(track).unwrap().unwrap().is_clip());
    assert_eq!(remote_likes(&lib, track), ["yt-gruppa-2"], "the like is the song's now");
    // The choice is the next default, kept in the prefs.
    wait_until("the prefs", || rt.state().prefs.replace_in_youtube);
    let saved: Prefs = serde_json::from_slice(&std::fs::read(&prefs).unwrap()).unwrap();
    assert!(saved.replace_in_youtube);

    let checks = yt.calls("liked_head") + yt.calls("liked");
    check_likes(&rt);
    wait_until("a check before the flush", || yt.calls("liked_head") + yt.calls("liked") > checks);
    std::thread::sleep(Duration::from_millis(100));
    assert!(yt.liked_ids().contains(&"yt-video-live".to_owned()), "not sent yet");
    assert_eq!(remote_likes(&lib, track), ["yt-gruppa-2"], "the clip is no news, the song not gone");
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 2);

    wait_until("the like moved there", || {
        let liked = yt.liked_ids();
        liked.contains(&"yt-gruppa-2".to_owned()) && !liked.contains(&"yt-video-live".to_owned())
    });
    // The next checks are calm: nothing imported again, nothing dropped.
    for _ in 0..2 {
        let checks = yt.calls("liked_head") + yt.calls("liked");
        check_likes(&rt);
        wait_until("a check", || yt.calls("liked_head") + yt.calls("liked") > checks);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(lib.is_liked(track).unwrap());
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 2);
    assert_eq!(remote_likes(&lib, track), ["yt-gruppa-2"]);
    assert_eq!(yt.calls("song_for_video"), 0, "the clip was not taken back as news");
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert_eq!(row.youtube_id.as_deref(), Some("yt-gruppa-2"));

    // «Отменить»: the clip again, here and there.
    rt.send(undo_of_last_toast(&rt));
    wait_until("the like back there", || {
        let liked = yt.liked_ids();
        !liked.contains(&"yt-gruppa-2".to_owned()) && liked.contains(&"yt-video-live".to_owned())
    });
    assert!(lib.get_track_row(track).unwrap().unwrap().is_clip());
    assert_eq!(remote_likes(&lib, track), ["yt-video-live"]);
    let checks = yt.calls("liked_head") + yt.calls("liked");
    check_likes(&rt);
    wait_until("a check", || yt.calls("liked_head") + yt.calls("liked") > checks);
    std::thread::sleep(Duration::from_millis(100));
    assert!(lib.is_liked(track).unwrap());
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 2);
    assert_eq!(remote_likes(&lib, track), ["yt-video-live"]);
}

#[test]
fn an_undo_before_the_flush_sends_nothing() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    liked_before(&lib, &yt, &["yt-video-live"]);
    let track = clip_track(&lib);
    let schedule = StreamSchedule { like_delay: Duration::from_millis(400), ..quick() };
    let rt = start_with(&lib, vec![yt.clone()], schedule, None);
    search(&rt, track, "Кино");
    rt.send(Command::ReplaceTrack {
        track,
        with: RemoteId::new(Service::Youtube, "yt-gruppa-2"),
        youtube: true,
    });
    wait_until("the replacement", || !lib.get_track_row(track).unwrap().unwrap().is_clip());
    rt.send(undo_of_last_toast(&rt));
    wait_until("the clip back", || lib.get_track_row(track).unwrap().unwrap().is_clip());
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(yt.calls("set_liked_many"), 0, "the like and its undo cancel out");
    assert_eq!(yt.liked_ids(), ["yt-video-live"]);
}

#[test]
fn the_repair_never_touches_a_track_replaced_by_hand() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    liked_before(&lib, &yt, &["yt-video-live"]);
    let track = clip_track(&lib);
    // Misses expire at once and the repair refills fast: without the
    // mark the clip would be asked again and again.
    let schedule = StreamSchedule {
        miss_ttl: Duration::ZERO,
        clip_refill: Duration::from_millis(20),
        ..quick()
    };
    let rt = start_with(&lib, vec![yt.clone()], schedule, None);
    wait_until("the repair at work", || yt.calls("song_for_video") >= 2);
    search(&rt, track, "Кино");
    rt.send(Command::ReplaceTrack {
        track,
        with: RemoteId::new(Service::Youtube, "yt-gruppa-2"),
        youtube: false,
    });
    wait_until("the replacement", || !lib.get_track_row(track).unwrap().unwrap().is_clip());
    rt.send(undo_of_last_toast(&rt));
    wait_until("the clip back", || lib.get_track_row(track).unwrap().unwrap().is_clip());
    std::thread::sleep(Duration::from_millis(100));
    let asked = yt.calls("song_for_video");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(yt.calls("song_for_video"), asked, "a clip kept by hand is not asked again");
    assert!(lib.get_track_row(track).unwrap().unwrap().is_clip());
}

#[test]
fn a_deezer_pick_plays_from_its_youtube_song_and_likes_that() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    let dz = Arc::new(FakeService::deezer());
    liked_before(&lib, &yt, &["yt-video-live"]);
    let track = clip_track(&lib);
    let rt = start_with(&lib, vec![yt.clone(), dz.clone()], quick(), None);
    search(&rt, track, "Война");
    rt.send(Command::ReplaceTrack {
        track,
        with: RemoteId::new(Service::Deezer, "dz-gruppa-3"),
        youtube: true,
    });
    wait_until("the replacement", || !lib.get_track_row(track).unwrap().unwrap().is_clip());
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert_eq!(row.title, "Война");
    assert_eq!(row.youtube_id.as_deref(), Some("yt-gruppa-3"), "it plays from the YouTube song");
    wait_until("the like there", || yt.liked_ids() == ["yt-gruppa-3"]);
    assert_eq!(remote_likes(&lib, track), ["yt-gruppa-3"]);
}

#[test]
fn the_youtube_scope_says_why_it_is_off() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(lonely());
    liked_before(&lib, &yt, &["yt-video-live"]);
    // A track liked here only, not there: «Другая версия…».
    let song = find(&yt, "yt-gruppa-4");
    let other = lib.upsert_stream_track(&song).unwrap();
    lib.like(nikgob_core::model::LikeTarget::Track(other)).unwrap();
    let rt = start_with(&lib, vec![yt.clone()], quick(), None);
    rt.send(Command::FindSong { track: other, query: None });
    wait_until("the dialog", || rt.state().find_song.as_ref().is_some_and(|f| f.track == other));
    {
        let s = rt.state();
        let f = s.find_song.as_ref().unwrap();
        assert!(!f.clip);
        assert_eq!(f.youtube_off.as_deref(), Some("Трек не лайкнут в YouTube Music"));
    }
    drop(rt);
    let signed_out = Arc::new(lonely().logged_out());
    let rt = start_with(&lib, vec![signed_out], quick(), None);
    let track = clip_track(&lib);
    rt.send(Command::FindSong { track, query: None });
    wait_until("the dialog", || rt.state().find_song.as_ref().is_some_and(|f| f.track == track));
    assert_eq!(rt.state().find_song.as_ref().unwrap().youtube_off.as_deref(), Some("Нужен вход в YouTube Music"));
}
