//! Liked YouTube clips become their songs: a new liked clip is stored as its song with the clip's id kept
//! as the like; the clips already in the library are turned into songs in
//! the background, one at a time, waiting out a rate limit and never asking
//! twice; a track with a song and its clip plays the song; an unlike goes
//! to the clip that was liked.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{LikeTarget, TrackId};
use nikgob_core::streaming::{FakeService, Service, StreamService, StreamTrack};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, PlaySource, Playback, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, Prefs};

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

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Fast ticks, short gaps between clip lookups, no jitter; a like check
/// at the start and then only when a test asks ([`check_likes`]).
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

fn start(lib: &Library, yt: &Arc<FakeService>, schedule: StreamSchedule) -> (Runtime, Arc<Mutex<Vec<PlayRequest>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams: vec![yt.clone() as Arc<dyn StreamService>],
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
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

/// The window gets the focus back: a like check runs.
fn check_likes(rt: &Runtime) {
    rt.send(Command::WindowState { visible: true, focused: false });
    rt.send(Command::WindowState { visible: true, focused: true });
}

fn find(service: &FakeService, id: &str) -> StreamTrack {
    service.tracks().into_iter().find(|t| t.id.id == id).unwrap()
}

/// YouTube likes only `liked` (most recent first), and the library pulled
/// exactly these before.
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
}

/// A clip of the fake catalogue without a song.
fn with_lonely_clip() -> FakeService {
    FakeService::youtube().with_video("live", "Кино - Последний герой (Live)", "Кино", 300, None)
}

fn track_of(lib: &Library, yt: &FakeService, id: &str) -> Option<TrackId> {
    lib.track_by_remote(&find(yt, id).id).unwrap()
}

#[test]
fn a_new_liked_clip_is_stored_as_its_song_and_its_unlike_goes_to_the_clip() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    liked_before(&lib, &yt, &["yt-discovery-1"]);
    let (rt, _) = start(&lib, &yt, quick());
    wait_until("the first check", || yt.calls("liked_head") + yt.calls("liked") > 0);
    // Liked on YouTube as the clip.
    yt.set_liked_remotely("yt-video-kino", true);
    check_likes(&rt);
    wait_until("the new like", || track_of(&lib, &yt, "yt-video-kino").is_some_and(|t| lib.is_liked(t).unwrap()));
    let track = track_of(&lib, &yt, "yt-video-kino").unwrap();
    assert_eq!(track_of(&lib, &yt, "yt-gruppa-1"), Some(track), "the clip and its song are one track");
    let row = lib.get_track_row(track).unwrap().unwrap();
    assert_eq!(row.title, "Группа крови");
    assert_eq!(row.album.as_deref(), Some("Группа крови"));
    assert!(!row.is_clip());
    let sources = lib.sources_of_track(track).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].locator, "yt-gruppa-1", "the song's audio");
    let likes = lib.remote_likes_of(track).unwrap();
    assert_eq!(likes.len(), 1);
    assert_eq!(likes[0].remote.id, "yt-video-kino", "the like is the clip's");
    assert_eq!(yt.calls("song_for_video"), 1);
    // The wishlist item is the song's (made right after the like).
    wait_until("the wishlist item", || !lib.wishlist_for_track(track).unwrap().is_empty());
    let item = lib.wishlist_for_track(track).unwrap().pop().unwrap();
    assert_eq!(item.title.as_deref(), Some("Группа крови"));
    assert_eq!(item.youtube_id.as_deref(), Some("yt-gruppa-1"));

    // Later checks know it: nothing imported again, nothing dropped.
    let checks = yt.calls("liked_head") + yt.calls("liked");
    check_likes(&rt);
    wait_until("another check", || yt.calls("liked_head") + yt.calls("liked") > checks);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(yt.calls("song_for_video"), 1, "not asked again");
    assert!(lib.is_liked(track).unwrap());
    assert_eq!(lib.liked_tracks(0, 10).unwrap().len(), 2);

    // An unlike here unlikes the clip on YouTube.
    rt.send(Command::Like(LikeTarget::Track(track), false));
    wait_until("the unlike", || !yt.liked_ids().contains(&"yt-video-kino".to_owned()));
    assert!(!yt.liked_ids().contains(&"yt-gruppa-1".to_owned()));
    assert!(lib.remote_likes_of(track).unwrap().is_empty());
}

#[test]
fn the_repair_turns_old_clips_into_songs_waits_out_a_pause_and_asks_once() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(with_lonely_clip());
    // Imported before clips had songs: two clips and a song.
    liked_before(&lib, &yt, &["yt-video-kino", "yt-video-live", "yt-discovery-1"]);
    let kino = track_of(&lib, &yt, "yt-video-kino").unwrap();
    let live = track_of(&lib, &yt, "yt-video-live").unwrap();
    assert!(lib.get_track_row(kino).unwrap().unwrap().is_clip());
    // The first lookup meets a rate limit: the service pauses 300 ms.
    yt.rate_limit_next_paused("song_for_video", 1, Duration::from_millis(300));
    let started = Instant::now();
    let (rt, _) = start(&lib, &yt, quick());
    wait_until("the clip to become its song", || {
        lib.get_track_row(kino).unwrap().is_some_and(|r| r.title == "Группа крови")
    });
    assert!(started.elapsed() >= Duration::from_millis(300), "the pause was waited out");
    wait_until("the lonely clip marked", || yt.calls("song_for_video") >= 3);
    wait_until("the mark", || lib.get_track_row(live).unwrap().unwrap().is_clip());
    let row = lib.get_track_row(kino).unwrap().unwrap();
    assert_eq!(row.album.as_deref(), Some("Группа крови"));
    assert_eq!(lib.remote_likes_of(kino).unwrap()[0].remote.id, "yt-video-kino");
    assert_eq!(yt.calls("song_for_video"), 3, "limited once, then one each");
    // The view says which is a clip.
    wait_until("the rows", || {
        let s = rt.state();
        let rows = &s.library.tracks;
        rows.iter().any(|t| t.id == live && t.clip) && rows.iter().any(|t| t.id == kino && !t.clip)
    });
    let cover = {
        let s = rt.state();
        let lonely = s.library.tracks.iter().find(|t| t.id == live).unwrap();
        lonely.cover.as_ref().and_then(|c| c.web_url()).map(str::to_owned)
    };
    assert_eq!(
        cover.as_deref(),
        Some("https://i.ytimg.com/vi/yt-video-live/hqdefault.jpg"),
        "a clip shows its video's picture"
    );
    drop(rt);

    // A restart asks nothing again.
    std::thread::sleep(Duration::from_millis(50));
    let (_rt, _) = start(&lib, &yt, quick());
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(yt.calls("song_for_video"), 3);
}

#[test]
fn a_track_with_a_song_and_its_clip_plays_the_song() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    liked_before(&lib, &yt, &["yt-video-kino"]);
    let track = track_of(&lib, &yt, "yt-video-kino").unwrap();
    // The song attached next to the clip (as an older build could leave it).
    lib.attach_stream_source(track, &find(&yt, "yt-gruppa-1")).unwrap();
    // Keep the repair away: this test is about playback.
    lib.cache_put("yt_clip.yt-video-kino", nikgob_library::now_ms(), "clip").unwrap();
    lib.cache_put("yt_clip_retry.v2", nikgob_library::now_ms(), "").unwrap();
    let (rt, asked) = start(&lib, &yt, quick());
    rt.send(Command::PlayTracks {
        tracks: vec![track],
        start: 0,
        context: "test".into(),
    });
    wait_until("a play request", || !asked.lock().unwrap().is_empty());
    assert_eq!(
        asked.lock().unwrap()[0].source,
        PlaySource::Stream {
            service: Service::Youtube,
            id: "yt-gruppa-1".into()
        }
    );
    assert_eq!(yt.calls("song_for_video"), 0);
}

#[test]
fn a_clip_without_its_song_plays_as_today() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(with_lonely_clip());
    liked_before(&lib, &yt, &["yt-video-live"]);
    let track = track_of(&lib, &yt, "yt-video-live").unwrap();
    let (rt, asked) = start(&lib, &yt, quick());
    wait_until("the lookup", || yt.calls("song_for_video") == 1);
    rt.send(Command::PlayTracks {
        tracks: vec![track],
        start: 0,
        context: "test".into(),
    });
    wait_until("a play request", || !asked.lock().unwrap().is_empty());
    assert_eq!(
        asked.lock().unwrap()[0].source,
        PlaySource::Stream {
            service: Service::Youtube,
            id: "yt-video-live".into()
        }
    );
    wait_until("now playing says clip", || rt.state().player.current.as_ref().is_some_and(|t| t.clip));
}

#[test]
fn the_misses_of_the_old_title_cleaner_are_asked_once_more() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(with_lonely_clip());
    liked_before(&lib, &yt, &["yt-video-live"]);
    let now = nikgob_library::now_ms();
    lib.mark_clip(&find(&yt, "yt-video-live").id).unwrap();
    // Found without a song yesterday, before the better cleaner.
    lib.cache_put("yt_clip.yt-video-live", now - 86_400_000, "clip").unwrap();
    let (rt, _) = start(&lib, &yt, quick());
    wait_until("the lookup again", || yt.calls("song_for_video") == 1);
    wait_until("its new verdict", || lib.cache_get("yt_clip.yt-video-live").unwrap().is_some_and(|(at, _)| at >= now));
    drop(rt);
    // Only once: the new miss stays a miss.
    let (_rt, _) = start(&lib, &yt, quick());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(yt.calls("song_for_video"), 1);
}

/// A music video a radio plays (YouTube Music's radios and the wave bring
/// clips: «Starboy (Official Video)»), the user never kept it.
fn radio_clip(lib: &Library, yt: &FakeService, id: &str) -> TrackId {
    let track = lib.upsert_stream_track(&find(yt, id)).unwrap();
    assert_eq!(lib.track_in_library(track).unwrap(), Some(false));
    track
}

fn played(asked: &Arc<Mutex<Vec<PlayRequest>>>) -> PlaySource {
    wait_until("a play request", || !asked.lock().unwrap().is_empty());
    asked.lock().unwrap()[0].source.clone()
}

fn youtube(id: &str) -> PlaySource {
    PlaySource::Stream {
        service: Service::Youtube,
        id: id.into(),
    }
}

#[test]
fn a_clip_a_radio_plays_is_its_song_before_it_plays() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(FakeService::youtube());
    let clip = radio_clip(&lib, &yt, "yt-video-kino");
    let (rt, asked) = start(&lib, &yt, quick());
    rt.send(Command::StartRadio { seed: nikgob_view::RadioSeed::Track(clip) });
    assert_eq!(played(&asked), youtube("yt-gruppa-1"), "the song, not the clip");
    assert_eq!(yt.calls("song_for_video"), 1);
    let song = lib.track_by_remote(&find(&yt, "yt-gruppa-1").id).unwrap();
    assert_eq!(rt.state().player.current.as_ref().map(|t| t.id), song);
    assert!(!rt.state().player.current.as_ref().unwrap().clip);
    assert_eq!(lib.cache_get("yt_clip.yt-video-kino").unwrap().map(|(_, v)| v).as_deref(), Some("song"));
    // Asked once: the next play of the clip's id is the song at once.
    drop(rt);
    asked.lock().unwrap().clear();
    let (rt, asked) = start(&lib, &yt, quick());
    let clip = lib.track_by_remote(&find(&yt, "yt-video-kino").id).unwrap().unwrap();
    rt.send(Command::StartRadio { seed: nikgob_view::RadioSeed::Track(clip) });
    assert_eq!(played(&asked), youtube("yt-gruppa-1"));
    assert_eq!(yt.calls("song_for_video"), 1, "nothing asked twice");
}

#[test]
fn a_clip_a_radio_plays_without_a_song_plays_as_the_clip() {
    let lib = Library::open_in_memory().unwrap();
    let yt = Arc::new(with_lonely_clip());
    let clip = radio_clip(&lib, &yt, "yt-video-live");
    let (rt, asked) = start(&lib, &yt, quick());
    rt.send(Command::StartRadio { seed: nikgob_view::RadioSeed::Track(clip) });
    assert_eq!(played(&asked), youtube("yt-video-live"), "no song: the clip, not someone else's");
    assert_eq!(yt.calls("song_for_video"), 1);
    assert_eq!(rt.state().player.current.as_ref().map(|t| t.id), Some(clip));
    wait_until("now playing says clip", || rt.state().player.current.as_ref().is_some_and(|t| t.clip));
    assert_eq!(lib.cache_get("yt_clip.yt-video-live").unwrap().map(|(_, v)| v).as_deref(), Some("clip"));
}
