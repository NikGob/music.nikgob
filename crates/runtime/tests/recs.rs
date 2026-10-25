//! Recommendations in the runtime (docs/stage4.md §3–§6) over a library
//! with a listening history and sound vectors, and the fake services: the
//! home's hero, moods and style mixes, the daily mix kept for the day,
//! mixes and radios that play, extend themselves, learn from skips and
//! survive a restart, «Не рекомендовать» with its undo, and the discovery
//! pass (budget, rate limits, known songs, persistence, new releases), and
//! the services' own suggestions (cadence, budget, cache, the snapshot and
//! the discoveries they change, signed-out and switched-off services).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{
    Analysis, AnalysisSource, Artist, ArtistCredit, ArtistId, Codec, Event, EventContext, EventType, Release, Source,
    SourceKind, Track, TrackId,
};
use nikgob_core::streaming::{FakeService, Service, StreamService};
use nikgob_library::{Library, now_ms};
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, MixId, Mood, Prefs, RadioSeed, RecTarget, TileKind, ViewState};

const DAY: i64 = 24 * 3600 * 1000;

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

fn schedule() -> StreamSchedule {
    StreamSchedule {
        tick: Duration::from_millis(50),
        analysis_start: Duration::ZERO,
        analysis_idle: Duration::from_secs(60),
        recs_debounce: Duration::from_millis(20),
        discover_gap: (Duration::from_millis(1), Duration::from_millis(5)),
        ..StreamSchedule::default()
    }
}

struct Started {
    rt: Runtime,
    asked: Arc<Mutex<Vec<PlayRequest>>>,
}

fn start(lib: &Library, streams: Vec<Arc<dyn StreamService>>, prefs: Prefs, prefs_path: Option<&Path>) -> Started {
    start_with(lib, streams, prefs, prefs_path, schedule())
}

fn start_with(
    lib: &Library,
    streams: Vec<Arc<dyn StreamService>>,
    prefs: Prefs,
    prefs_path: Option<&Path>,
    schedule: StreamSchedule,
) -> Started {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams,
            library_dir: std::env::temp_dir(),
            prefs,
            prefs_path: prefs_path.map(Path::to_path_buf),
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(asked.clone())),
        Arc::new(|| {}),
        schedule,
    );
    Started { rt, asked }
}

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = rt.state();
    panic!(
        "timed out waiting for {what}: hero {:?}, moods {}, styles {}, discover {}, queue {:?}",
        s.home.hero.as_ref().map(|h| &h.subtitle),
        s.home.moods.len(),
        s.home.style_mixes.len(),
        s.home.discover.len(),
        s.queue.radio
    );
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f32) / (1u64 << 31) as f32
    }
}

struct Lib {
    lib: Library,
    tracks: Vec<TrackId>,
    artists: Vec<ArtistId>,
}

/// `styles` groups of artists, each with its own sound, `per` tracks per
/// artist over `artists` artists a group; every track played in the last
/// weeks (the first track of each artist also liked). Titles are
/// `{artist} {n}`, unless `named` gives real ones.
fn library(styles: usize, artists: usize, per: usize, named: &[(&str, &[&str])]) -> Lib {
    let lib = Library::open_in_memory().unwrap();
    let mut rng = Lcg(7);
    let now = now_ms();
    let mut tracks = Vec::new();
    let mut artist_ids = Vec::new();
    let mut add = |name: &str, titles: Vec<String>, style: usize, lib: &Library, rng: &mut Lcg| {
        let a = lib.upsert_artist(&Artist::new(name)).unwrap();
        artist_ids.push(a.id);
        let credit = ArtistCredit::single(name, Some(a.id));
        let mut r = Release::new(format!("{name} album"), credit.clone());
        r.cover_ref = Some(format!("https://fake.invalid/{}.jpg", artist_ids.len()));
        let r = lib.upsert_release(&r).unwrap();
        for (n, title) in titles.iter().enumerate() {
            let mut t = Track::new(title.as_str(), credit.clone());
            t.release_id = Some(r.id);
            t.track_no = n as u32 + 1;
            t.duration_ms = Some(200_000);
            let t = lib.upsert_track(&t).unwrap();
            let s = Source::new(t.id, SourceKind::LocalFile, format!("missing/{name}/{n}.flac"), Codec::Flac);
            lib.add_source(&s).unwrap();
            // The style's sound: its own centre, a little spread.
            let v: Vec<f32> = (0..nikgob_recs::DIM)
                .map(|i| ((style * 7 + i * 3) % 11) as f32 * 0.8 + rng.f32() * 0.3)
                .collect();
            lib.put_analysis(&Analysis {
                track_id: t.id,
                bliss_vector: v,
                analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(),
                source: AnalysisSource::File,
            })
            .unwrap();
            for k in 0..(1 + n % 3) {
                let ts = now - (5 + (n * 3 + k * 7) as i64 % 20) * DAY - (k as i64) * 3_600_000;
                for kind in [EventType::Start, EventType::Complete] {
                    let mut e = Event::new(t.id, kind, ts + i64::from(kind == EventType::Complete) * 200_000, lib.device_id());
                    e.context = EventContext::Album;
                    lib.record_event_raw(&e).unwrap();
                }
            }
            if n == 0 {
                lib.like(t.id).unwrap();
            }
            tracks.push(t.id);
        }
    };
    for (style, (name, titles)) in named.iter().enumerate() {
        add(name, titles.iter().map(|t| (*t).to_owned()).collect(), style, &lib, &mut rng);
    }
    for style in 0..styles {
        for a in 0..artists {
            let name = format!("Artist {style}-{a}");
            let titles = (0..per).map(|n| format!("{name} song {n}")).collect();
            add(&name, titles, style, &lib, &mut rng);
        }
    }
    Lib {
        lib,
        tracks,
        artists: artist_ids,
    }
}

fn hero_mix(s: &ViewState) -> Vec<TrackId> {
    s.home.mix.iter().map(|t| t.id).collect()
}

fn home_ready(s: &ViewState) -> bool {
    s.home.hero.is_some() && s.home.moods.len() == 6 && s.home.style_mixes.len() >= 2
}

#[test]
fn the_home_gets_a_hero_moods_and_style_mixes() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the home's mixes", home_ready);
    let st = s.rt.state();
    let hero = st.home.hero.as_ref().unwrap();
    assert_eq!(hero.id, MixId::Daily);
    assert_eq!(hero.title, "Твой микс");
    assert_eq!((hero.kicker.as_str(), hero.subtitle.as_str()), ("НА СЕГОДНЯ", "30 треков · любимое и похожее"));
    assert!(!hero.covers.is_empty() && hero.covers.len() <= 4);
    assert_eq!(st.home.mix.len(), 30);
    let moods: Vec<Mood> = st
        .home
        .moods
        .iter()
        .map(|m| match m.id {
            MixId::Mood(m) => m,
            other => panic!("{other:?}"),
        })
        .collect();
    let mut sorted = moods.clone();
    sorted.sort();
    assert_eq!(sorted, Mood::ALL.to_vec(), "six moods, each once");
    assert!(st.home.moods.iter().all(|m| !m.covers.is_empty() && !m.subtitle.is_empty()));
    for (i, m) in st.home.style_mixes.iter().enumerate() {
        assert_eq!(m.id, MixId::Style(i as u8));
        assert!(m.title.contains(" и другие"), "{}", m.title);
        // What binds the artists: the sound in words, or that it is alike.
        assert!(m.subtitle.starts_with("по звуку: ") || m.subtitle.ends_with(" похожего звука"), "{}", m.subtitle);
        assert!(!m.covers.is_empty());
    }
}

#[test]
fn the_daily_mix_stays_the_same_all_day_until_refreshed() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the hero", |s| s.home.mix.len() == 30);
    let first = hero_mix(&s.rt.state());
    // Another build (a like changes the library) and a restart keep it.
    s.rt.send(Command::Like(nikgob_core::model::LikeTarget::Track(l.tracks[5]), true));
    std::thread::sleep(Duration::from_millis(400));
    s.rt.send(Command::LoadHome);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(hero_mix(&s.rt.state()), first);
    drop(s);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the hero again", |s| s.home.mix.len() == 30);
    assert_eq!(hero_mix(&s.rt.state()), first, "kept for the day");
    // «Обновить» picks again.
    s.rt.send(Command::RefreshMix { id: MixId::Daily });
    wait_for(&s.rt, "a new daily mix", |st| st.home.mix.len() == 30 && hero_mix(st) != first);
}

/// The start event of the track playing now: the queue is published before
/// the event is written, so it is waited for.
fn current_start(lib: &Library, rt: &Runtime) -> Event {
    let playing = || rt.state().player.current.as_ref().map(|t| t.id);
    let latest = || {
        lib.recent_events(50)
            .unwrap()
            .into_iter()
            .find(|e| e.event_type == EventType::Start && e.ts > now_ms() - 60_000)
    };
    wait_until("the start of the track playing", || {
        latest().is_some_and(|e| Some(e.track_id) == playing())
    });
    latest().expect("a start just now")
}

#[test]
fn play_mix_plays_and_records_the_home_context() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the home's mixes", home_ready);

    s.rt.send(Command::PlayMix { id: MixId::Daily });
    wait_until("the daily mix plays", || !s.asked.lock().unwrap().is_empty());
    wait_for(&s.rt, "its queue", |st| st.queue.next.len() == 29);
    assert_eq!(current_start(&l.lib, &s.rt).context, EventContext::HomeSection);
    assert!(s.rt.state().queue.radio.is_none(), "the daily mix is not endless");
    let first = hero_mix(&s.rt.state())[0];
    assert_eq!(s.rt.state().player.current.as_ref().map(|t| t.id), Some(first));

    s.rt.send(Command::PlayMix { id: MixId::Mood(Mood::Calm) });
    wait_for(&s.rt, "the mood's radio", |st| {
        st.queue.radio.as_ref().is_some_and(|r| r.title == "Спокойное") && st.queue.next.len() >= 9
    });
    assert_eq!(current_start(&l.lib, &s.rt).context, EventContext::HomeSection);
    assert_eq!(s.rt.state().player.context_line(), "Играет из микса «Спокойное»");

    s.rt.send(Command::PlayMix { id: MixId::Style(0) });
    wait_for(&s.rt, "the style's radio", |st| {
        st.queue.radio.as_ref().is_some_and(|r| r.title.contains(" и другие")) && st.queue.next.len() >= 9
    });

    // Playing anything else ends the radio (a lone track goes on by
    // autoplay, with a radio of its own).
    s.rt.send(Command::PlayTracks {
        tracks: vec![l.tracks[0]],
        start: 0,
        context: "test".into(),
    });
    wait_for(&s.rt, "the radio to end", |st| st.queue.radio.as_ref().is_none_or(|r| r.autoplay));
}

#[test]
fn autoplay_goes_on_after_the_last_track_with_nothing_the_queue_had() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the home", home_ready);
    let album = vec![l.tracks[0], l.tracks[1], l.tracks[2]];
    s.rt.send(Command::PlayTracks { tracks: album.clone(), start: 0, context: "Альбом".into() });
    wait_until("the first plays", || !s.asked.lock().unwrap().is_empty());
    wait_for(&s.rt, "the queue", |st| st.queue.next.len() == 2);
    assert!(s.rt.state().queue.radio.is_none(), "no radio before the last track");
    let context = s.rt.state().player.context_line();
    end_current(&s);
    end_current(&s);
    // The last track plays: similar ones join the queue behind it.
    wait_for(&s.rt, "autoplay", |st| {
        st.queue.radio.as_ref().is_some_and(|r| r.autoplay && !r.extending) && st.queue.next.len() == 10
    });
    let st = s.rt.state();
    assert_eq!(st.queue.radio.as_ref().unwrap().title, format!("Похожее на «{}»", st.player.current.as_ref().unwrap().title));
    assert_eq!(st.player.current.as_ref().map(|t| t.id), Some(album[2]));
    assert!(st.queue.next.iter().all(|e| !album.contains(&e.track.id)), "nothing the queue had");
    assert_eq!(st.player.context_line(), context, "the queue keeps its name");
    drop(st);

    // Off: a queue simply ends, and the radio it had stops picking.
    let prefs = Prefs { autoplay: false, ..s.rt.state().prefs.clone() };
    s.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&s.rt, "autoplay off", |st| st.queue.radio.is_none());
    s.rt.send(Command::PlayTracks { tracks: vec![l.tracks[4]], start: 0, context: "test".into() });
    wait_for(&s.rt, "the lone track", |st| st.player.current.as_ref().is_some_and(|t| t.id == l.tracks[4]));
    std::thread::sleep(Duration::from_millis(300));
    let st = s.rt.state();
    assert!(st.queue.radio.is_none() && st.queue.next.is_empty(), "{:?}", st.queue.radio);
}

#[test]
fn a_radio_switched_to_downloaded_only_drops_what_streams_and_picks_again() {
    let l = library(3, 6, 5, &[]);
    let streams = add_streams(&l);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the home", home_ready);
    s.rt.send(Command::StartRadio { seed: RadioSeed::Track(streams[1]) });
    wait_for(&s.rt, "the stream's radio", |st| st.queue.radio.is_some() && st.queue.next.len() >= 8);
    let streamed = |st: &ViewState| st.queue.next.iter().filter(|e| streams.contains(&e.track.id)).count();
    assert!(streamed(&s.rt.state()) > 0, "the radio of a streamed song plays its neighbours");

    let prefs = Prefs { mixes_downloaded_only: true, ..s.rt.state().prefs.clone() };
    s.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&s.rt, "only files ahead", |st| streamed(st) == 0 && st.queue.next.len() >= 4);
    assert!(s.rt.state().queue.next.iter().all(|e| e.track.has_local));
}

/// Ends the playing item; returns once the next one was asked for.
fn end_current(s: &Started) {
    let (id, n) = {
        let asked = s.asked.lock().unwrap();
        (asked.last().unwrap().id, asked.len())
    };
    s.rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    s.rt.playback_sender().send(PlaybackEvent::Ended { id }).unwrap();
    wait_until("the next track", || s.asked.lock().unwrap().len() > n);
}

#[test]
fn a_radio_extends_itself_and_survives_a_restart() {
    let l = library(3, 6, 5, &[]);
    let dir = tempfile::tempdir().unwrap();
    let prefs_path = dir.path().join("prefs.json");
    let s = start(&l.lib, Vec::new(), Prefs::default(), Some(&prefs_path));
    wait_for(&s.rt, "the home", home_ready);
    let seed = l.tracks[3];
    s.rt.send(Command::StartRadio { seed: RadioSeed::Track(seed) });
    wait_until("the seed plays", || !s.asked.lock().unwrap().is_empty());
    assert_eq!(s.rt.state().player.current.as_ref().map(|t| t.id), Some(seed));
    wait_for(&s.rt, "ten more", |st| st.queue.next.len() == 10);
    let title = s.rt.state().queue.radio.clone().unwrap().title;
    assert!(title.starts_with("Радио · "), "{title}");
    assert_eq!(current_start(&l.lib, &s.rt).context, EventContext::Radio);

    // Down to three left: ten more come by themselves.
    for _ in 0..7 {
        end_current(&s);
    }
    wait_for(&s.rt, "the queue to extend", |st| st.queue.next.len() == 13);
    let upcoming: Vec<TrackId> = s.rt.state().queue.next.iter().map(|e| e.track.id).collect();
    let played: Vec<TrackId> = s.rt.state().queue.prev.iter().map(|e| e.track.id).collect();
    assert!(upcoming.iter().all(|t| !played.contains(t)), "nothing served twice");
    drop(s);

    // The radio goes on after a restart.
    let s = start(&l.lib, Vec::new(), Prefs::default(), Some(&prefs_path));
    wait_for(&s.rt, "the radio back", |st| st.queue.radio.as_ref().is_some_and(|r| r.title == title));
    let left = s.rt.state().queue.next.len();
    assert_eq!(left, 13);
    s.rt.send(Command::TogglePlay);
    wait_until("the restored track plays", || !s.asked.lock().unwrap().is_empty());
    for _ in 0..10 {
        end_current(&s);
    }
    wait_for(&s.rt, "the restored radio to extend", |st| st.queue.next.len() == 13);
}

#[test]
fn an_early_skip_teaches_the_radio() {
    let l = library(3, 6, 5, &[]);
    let dir = tempfile::tempdir().unwrap();
    let prefs_path = dir.path().join("prefs.json");
    let s = start(&l.lib, Vec::new(), Prefs::default(), Some(&prefs_path));
    wait_for(&s.rt, "the home", home_ready);
    s.rt.send(Command::StartRadio { seed: RadioSeed::Artist(l.artists[0]) });
    wait_for(&s.rt, "the artist's radio", |st| st.queue.next.len() >= 9 && st.player.current.is_some());
    let skipped = s.rt.state().player.current.as_ref().unwrap().id;
    s.rt.send(Command::Next);
    wait_for(&s.rt, "the next one", |st| st.player.current.as_ref().is_some_and(|t| t.id != skipped));
    drop(s);
    let session: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("session.json")).unwrap()).unwrap();
    let lessons = &session["radio"]["state"]["skipped"];
    assert_eq!(lessons[0]["Track"], serde_json::json!(skipped.to_string()), "{lessons}");
}

fn daylist_picks(lib: &Library) -> serde_json::Value {
    let (_, json) = lib.cache_get("recs.daylist").unwrap().expect("the daylist is kept");
    serde_json::from_str::<serde_json::Value>(&json).unwrap()["picks"].clone()
}

#[test]
fn the_bento_gets_a_daylist_and_tiles_that_play() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the daylist and the tiles", |st| st.home.daylist.is_some() && !st.home.tiles.is_empty());
    let st = s.rt.state().home.clone();
    let daylist = st.daylist.clone().unwrap();
    assert_eq!(daylist.id, MixId::Daylist);
    assert_eq!(daylist.title.split(' ').count(), 3, "«Тягучий вечер четверга»: {}", daylist.title);
    assert!(daylist.subtitle.starts_with("что ты слушаешь ") && daylist.subtitle.contains(" · до "), "{}", daylist.subtitle);
    assert!(!daylist.covers.is_empty());
    // «На повторе» first, then four kinds of the day, each once, each with
    // its kicker.
    let tiles: Vec<MixId> = st.tiles.iter().map(|t| t.id).collect();
    assert_eq!(tiles.len(), 5, "{tiles:?}");
    assert_eq!(tiles[0], MixId::Tile(TileKind::OnRepeat));
    assert_eq!(tiles.iter().collect::<std::collections::HashSet<_>>().len(), 5);
    assert_eq!((st.tiles[0].kicker.as_str(), st.tiles[0].title.as_str()), ("Чаще всего", "На повторе"));
    assert_eq!(st.tiles[0].subtitle, "30 треков · что крутил весь месяц");
    let lottery = |t: &&nikgob_view::MixCard| t.id == MixId::Tile(TileKind::Lottery);
    assert!(st.tiles.iter().all(|t| !t.kicker.is_empty() && !t.subtitle.is_empty() && (lottery(&t) || !t.covers.is_empty())));
    assert!(st.tiles.iter().filter(lottery).all(|t| t.covers.is_empty()), "the lottery shows nothing of what it plays");
    assert!(daylist.kicker.starts_with("НА "), "{}", daylist.kicker);
    // The board is kept for the day: a restart shows the same tiles.
    drop(s);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the tiles again", |st| !st.home.tiles.is_empty());
    assert_eq!(s.rt.state().home.tiles.iter().map(|t| t.id).collect::<Vec<_>>(), tiles);

    // A tile plays as it is.
    s.rt.send(Command::PlayMix { id: MixId::Tile(TileKind::OnRepeat) });
    wait_until("on repeat plays", || !s.asked.lock().unwrap().is_empty());
    wait_for(&s.rt, "its queue", |st| st.queue.next.len() == 29);
    assert!(s.rt.state().queue.radio.is_none(), "a tile is not endless");
    assert_eq!(s.rt.state().player.context_line(), "Играет из микса «На повторе»");
    assert_eq!(current_start(&l.lib, &s.rt).context, EventContext::HomeSection);

    // The daylist plays the picks shown, then goes on as a radio.
    s.rt.send(Command::PlayMix { id: MixId::Daylist });
    wait_for(&s.rt, "the daylist", |st| {
        st.queue.radio.as_ref().is_some_and(|r| r.title == daylist.title) && st.queue.next.len() == 29
    });
    assert_eq!(current_start(&l.lib, &s.rt).context, EventContext::HomeSection);
    for _ in 0..27 {
        end_current(&s);
    }
    wait_for(&s.rt, "the daylist to extend", |st| st.queue.next.len() == 12);
    let upcoming: Vec<TrackId> = s.rt.state().queue.next.iter().map(|e| e.track.id).collect();
    let played: Vec<TrackId> = s.rt.state().queue.prev.iter().map(|e| e.track.id).collect();
    assert!(upcoming.iter().all(|t| !played.contains(t)), "nothing served twice");

    // «Обновить» picks another selection.
    let before = daylist_picks(&l.lib);
    s.rt.send(Command::RefreshMix { id: MixId::Daylist });
    wait_until("another daylist", || daylist_picks(&l.lib) != before);
}

/// Ten liked songs by «Stream Band» that only stream (no file), played as
/// much as the rest, in the sound of the first style.
fn add_streams(l: &Lib) -> Vec<TrackId> {
    let lib = &l.lib;
    let a = lib.upsert_artist(&Artist::new("Stream Band")).unwrap();
    let credit = ArtistCredit::single("Stream Band", Some(a.id));
    let now = now_ms();
    let mut rng = Lcg(11);
    (0..10)
        .map(|n| {
            let t = lib.upsert_track(&Track::new(format!("Stream song {n}").as_str(), credit.clone())).unwrap();
            let s = Source::new(t.id, SourceKind::Youtube, format!("yt-{n}"), Codec::Opus);
            lib.add_source(&s).unwrap();
            lib.like(t.id).unwrap();
            let v: Vec<f32> = (0..nikgob_recs::DIM).map(|i| ((i * 3) % 11) as f32 * 0.8 + rng.f32() * 0.3).collect();
            lib.put_analysis(&Analysis {
                track_id: t.id,
                bliss_vector: v,
                analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(),
                source: AnalysisSource::Youtube,
            })
            .unwrap();
            for k in 0..3i64 {
                let ts = now - (2 + k * 5) * DAY;
                for kind in [EventType::Start, EventType::Complete] {
                    let e = Event::new(t.id, kind, ts + i64::from(kind == EventType::Complete) * 200_000, lib.device_id());
                    lib.record_event_raw(&e).unwrap();
                }
            }
            t.id
        })
        .collect()
}

/// The ids of what plays now and next.
fn queued(st: &ViewState) -> Vec<TrackId> {
    st.queue.current.iter().chain(st.queue.next.iter().map(|e| &e.track)).map(|t| t.id).collect()
}

#[test]
fn downloaded_only_keeps_every_mix_to_files_at_once() {
    let l = library(3, 6, 5, &[]);
    let streams = add_streams(&l);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the home's mixes", home_ready);
    let streamed = |ids: &[TrackId]| ids.iter().filter(|t| streams.contains(t)).count();
    // A favourite of the day's mix streams, for now.
    assert!(streamed(&hero_mix(&s.rt.state())) > 0, "the fixture's streams are favourites too");

    let prefs = Prefs { mixes_downloaded_only: true, ..s.rt.state().prefs.clone() };
    s.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&s.rt, "the home built again", |st| home_ready(st) && streamed(&hero_mix(st)) == 0 && st.home.mix.len() == 30);
    let st = s.rt.state().home.clone();
    assert!(st.mix.iter().all(|t| t.has_local));
    assert!(!st.tiles.iter().any(|t| t.id == MixId::Tile(TileKind::NotDownloaded)));
    // Nothing else on the home plays without a file either.
    assert!(st.discover.is_empty() && st.new_releases.is_empty());
    assert!(st.continue_listening.iter().all(|c| matches!(c, nikgob_view::ContinueCard::Release(r) if r.local_tracks > 0)));
    assert!(st.long_unplayed.iter().chain(&st.recently_added).all(|r| r.local_tracks > 0));

    // Moods, style mixes, an artist's radio, the daylist and the lottery.
    let mixes = [
        MixId::Mood(Mood::Calm),
        MixId::Style(0),
        MixId::Artist(l.artists[0]),
        MixId::Daylist,
        MixId::Tile(TileKind::Lottery),
    ];
    for id in mixes {
        let before = queued(&s.rt.state());
        s.rt.send(Command::PlayMix { id });
        wait_for(&s.rt, &format!("{id:?} plays"), |st| queued(st).len() >= 8 && queued(st) != before);
        let q = queued(&s.rt.state());
        assert_eq!(streamed(&q), 0, "{id:?} plays a stream");
    }
    // The radio of a streamed song: the song (it cannot play here without
    // a service), then only files.
    s.rt.send(Command::StartRadio { seed: RadioSeed::Track(streams[0]) });
    let radio = |st: &ViewState| st.queue.radio.as_ref().is_some_and(|r| r.title == "Радио · Stream song 0");
    wait_for(&s.rt, "the stream's radio", |st| radio(st) && queued(st).len() >= 8);
    let q: Vec<TrackId> = queued(&s.rt.state()).into_iter().filter(|t| *t != streams[0]).collect();
    assert_eq!(streamed(&q), 0);
}

#[test]
fn the_lottery_draws_again_at_every_click_and_shows_nothing() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the home's mixes", home_ready);
    let mut draws: Vec<Vec<TrackId>> = Vec::new();
    for _ in 0..4 {
        s.rt.send(Command::PlayMix { id: MixId::Tile(TileKind::Lottery) });
        let before = draws.last().cloned();
        wait_for(&s.rt, "a draw", |st| queued(st).len() == 30 && Some(queued(st)) != before);
        assert!(s.rt.state().queue.radio.is_none(), "the lottery is not endless");
        draws.push(queued(&s.rt.state()));
    }
    let firsts: std::collections::HashSet<TrackId> = draws.iter().map(|d| d[0]).collect();
    assert!(firsts.len() >= 3, "the first song is known before: {firsts:?}");
}

#[test]
fn a_dislike_leaves_the_home_and_its_undo_brings_it_back() {
    let l = library(3, 6, 5, &[]);
    let s = start(&l.lib, Vec::new(), Prefs::default(), None);
    wait_for(&s.rt, "the hero", |s| s.home.mix.len() == 30);
    let t = hero_mix(&s.rt.state())[4];
    s.rt.send(Command::RecFeedback {
        target: RecTarget::Track(t),
        dislike: true,
    });
    wait_for(&s.rt, "the track to leave", |st| !hero_mix(st).contains(&t) && !st.home.mix.is_empty());
    let undo = s
        .rt
        .state()
        .toasts
        .iter()
        .find_map(|toast| toast.action.clone().filter(|(label, _)| label == "Отменить"))
        .expect("an undo toast");
    assert_eq!(
        undo.1,
        Command::RecFeedback {
            target: RecTarget::Track(t),
            dislike: false
        }
    );
    assert!(l.lib.is_rec_disliked("track", &t.to_string()).unwrap());
    s.rt.send(undo.1);
    wait_for(&s.rt, "the track back", |st| hero_mix(st).contains(&t));
    assert!(!l.lib.is_rec_disliked("track", &t.to_string()).unwrap());
}

/// Songs the fake services know, as library tracks: seeds of a discovery.
const KNOWN: &[(&str, &[&str])] = &[
    ("Кино", &["Группа крови", "Война", "Спокойная ночь", "Закрой за мной дверь, я ухожу"]),
    ("Daft Punk", &["One More Time", "Aerodynamic", "Digital Love"]),
];

fn fakes() -> (Arc<FakeService>, Arc<FakeService>) {
    let yt = Arc::new(FakeService::youtube().logged_out());
    let dz = Arc::new(FakeService::deezer().with_new_release("daft", "voyager2", "Voyager 2", 10));
    (yt, dz)
}

const DISCOVERY_CALLS: &[&str] = &["related", "radio", "similar_artists", "artist_top", "artist_radio", "search_kind", "artist"];

fn discovery_calls(f: &FakeService) -> usize {
    DISCOVERY_CALLS.iter().map(|c| f.calls(c)).sum()
}

#[test]
fn discovery_finds_new_songs_within_its_budget_and_keeps_them() {
    let l = library(0, 0, 0, KNOWN);
    let (yt, dz) = fakes();
    let s = start(&l.lib, vec![yt.clone(), dz.clone()], Prefs::default(), None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty() && !st.home.new_releases.is_empty());
    let st = s.rt.state();
    let artists: Vec<&str> = st.home.discover.iter().map(|r| r.artist.as_str()).collect();
    assert!(
        artists.iter().any(|a| ["Аквариум", "Наутилус Помпилиус", "Justice"].contains(a)),
        "{artists:?}"
    );
    for r in &st.home.discover {
        for (_, titles) in KNOWN {
            assert!(!titles.contains(&r.title.as_str()), "a library song came back: {}", r.title);
        }
    }
    assert!(st.home.new_releases.iter().any(|a| a.title == "Voyager 2"), "{:?}", st.home.new_releases);
    let used = discovery_calls(&yt) + discovery_calls(&dz);
    assert!(used <= 100, "{used}: {:?} {:?}", yt.all_calls(), dz.all_calls());
    let shown = st.home.discover.len();
    drop(st);

    // «Слушать всё»: the discoveries play as a radio.
    s.rt.send(Command::PlayMix { id: MixId::Discover });
    wait_for(&s.rt, "the discoveries' radio", |st| {
        st.queue.radio.as_ref().is_some_and(|r| r.title == "Новое для тебя") && st.player.current.is_some()
    });

    // A dislike of a discovery takes it off the home at once.
    let first = s.rt.state().home.discover[0].remote.clone();
    s.rt.send(Command::RecFeedback {
        target: RecTarget::StreamTrack(first.clone()),
        dislike: true,
    });
    wait_for(&s.rt, "the discovery to leave", |st| st.home.discover.iter().all(|r| r.remote != first));
    s.rt.send(Command::RecFeedback {
        target: RecTarget::StreamTrack(first.clone()),
        dislike: false,
    });
    wait_for(&s.rt, "the discovery back", |st| st.home.discover.iter().any(|r| r.remote == first));
    drop(s);

    // Kept for the day: a restart shows them without asking again.
    yt.reset_calls();
    dz.reset_calls();
    let s = start(&l.lib, vec![yt.clone(), dz.clone()], Prefs::default(), None);
    wait_for(&s.rt, "the kept discoveries", |st| st.home.discover.len() == shown);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(discovery_calls(&yt) + discovery_calls(&dz), 0, "{:?} {:?}", yt.all_calls(), dz.all_calls());
}

#[test]
fn downloaded_only_hides_new_music_on_the_home_and_plays_none() {
    let l = library(0, 0, 0, KNOWN);
    let (yt, dz) = fakes();
    let s = start(&l.lib, vec![yt.clone(), dz.clone()], Prefs::default(), None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty() && !st.home.new_releases.is_empty());
    let prefs = Prefs { mixes_downloaded_only: true, ..s.rt.state().prefs.clone() };
    s.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&s.rt, "the new rows gone", |st| st.home.discover.is_empty() && st.home.new_releases.is_empty());
    // «Слушать всё» of the discoveries plays nothing new.
    s.rt.send(Command::PlayMix { id: MixId::Discover });
    std::thread::sleep(Duration::from_millis(300));
    let st = s.rt.state();
    assert!(st.queue.radio.is_none() && st.player.current.is_none(), "{:?}", st.queue.radio);
    assert!(st.toasts.iter().any(|t| t.text.contains("Только скачанное")), "{:?}", st.toasts);
    drop(st);
    // Off again: they are back at once.
    let prefs = Prefs { mixes_downloaded_only: false, ..s.rt.state().prefs.clone() };
    s.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for(&s.rt, "the new rows back", |st| !st.home.discover.is_empty());
}

#[test]
fn discovery_waits_out_a_rate_limit() {
    let l = library(0, 0, 0, KNOWN);
    let (yt, dz) = fakes();
    yt.rate_limit_next_paused("radio", 1, Duration::from_millis(1500));
    let s = start(&l.lib, vec![yt.clone(), dz.clone()], Prefs::default(), None);
    wait_until("the limited radio call", || yt.calls("radio") >= 1);
    let at = (yt.total_calls(), dz.total_calls());
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!((yt.total_calls(), dz.total_calls()), at, "nothing while paused");
    wait_for(&s.rt, "discoveries after the pause", |st| !st.home.discover.is_empty());
    assert!(yt.calls("radio") >= 2, "the same request again: {:?}", yt.all_calls());
}

// ---- The services' own suggestions (docs/stage4.md §12.3) ----------------

/// Suggestions read at once, the discovery quick.
fn suggest_schedule() -> StreamSchedule {
    StreamSchedule {
        suggest_first: Duration::ZERO,
        ..schedule()
    }
}

fn line(st: &ViewState, service: Service) -> Option<String> {
    st.account(service).and_then(|a| a.suggestions.clone())
}

fn read(st: &ViewState, service: Service) -> bool {
    line(st, service).is_some_and(|l| l.starts_with("Подсказки: ") && l.contains("обновлено"))
}

#[test]
fn suggestions_are_read_within_their_budget_kept_and_change_the_recommendations() {
    let l = library(0, 0, 0, KNOWN);
    let yt = Arc::new(FakeService::youtube());
    let sp = Arc::new(FakeService::spotify());
    let dz = Arc::new(FakeService::deezer());
    let streams: Vec<Arc<dyn StreamService>> = vec![yt.clone(), sp.clone(), dz.clone()];
    let s = start_with(&l.lib, streams.clone(), Prefs::default(), None, suggest_schedule());
    wait_for(&s.rt, "both services' suggestions", |st| read(st, Service::Youtube) && read(st, Service::Spotify));
    let st = s.rt.state();
    let yt_line = line(&st, Service::Youtube).unwrap();
    assert!(yt_line.starts_with("Подсказки: 7 треков, 2 исполнителя · обновлено в "), "{yt_line}");
    assert!(line(&st, Service::Spotify).unwrap().contains("4 исполнителя"), "{:?}", line(&st, Service::Spotify));
    assert_eq!(st.account(Service::Spotify).unwrap().suggestions_note, None);
    assert_eq!(line(&st, Service::Deezer), None, "Deezer has none");
    drop(st);
    assert_eq!(yt.calls("suggestions"), 2, "{:?}", yt.all_calls());
    assert_eq!(sp.calls("suggestions"), 5, "{:?}", sp.all_calls());
    assert!(l.lib.cache_get("suggest.youtube").unwrap().is_some());
    assert!(l.lib.cache_get("suggest.spotify").unwrap().is_some());

    // The songs only the services suggest (Sigur Rós on YouTube Music,
    // 椎名林檎 on Spotify: the library's seeds never lead to them) become
    // discoveries once the recommendations are built again.
    wait_for(&s.rt, "the suggested songs among the discoveries", |st| {
        let titles: Vec<&str> = st.home.discover.iter().map(|r| r.title.as_str()).collect();
        titles.contains(&"Svefn-g-englar") && titles.contains(&"丸の内サディスティック")
    });
    drop(s);

    // The snapshot matches the kept suggestions against the library.
    let preview = nikgob_runtime::preview_home(&l.lib, &std::env::temp_dir()).unwrap();
    assert!(preview.suggested_library >= 4, "{}", preview.suggested_library);
    assert!(preview.suggested_new >= 3, "{}", preview.suggested_new);

    // Kept for six hours: a restart asks nothing.
    yt.reset_calls();
    sp.reset_calls();
    let s = start_with(&l.lib, streams, Prefs::default(), None, suggest_schedule());
    wait_for(&s.rt, "the kept suggestions", |st| read(st, Service::Youtube) && read(st, Service::Spotify));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(yt.calls("suggestions") + sp.calls("suggestions"), 0, "{:?} {:?}", yt.all_calls(), sp.all_calls());
}

#[test]
fn signed_out_or_switched_off_services_are_asked_for_no_suggestions() {
    let l = library(0, 0, 0, KNOWN);
    let yt = Arc::new(FakeService::youtube().logged_out());
    let sp = Arc::new(FakeService::spotify());
    let prefs = Prefs {
        spotify_enabled: false,
        deezer_enabled: false,
        ..Prefs::default()
    };
    let s = start_with(&l.lib, vec![yt.clone(), sp.clone()], prefs, None, suggest_schedule());
    wait_for(&s.rt, "the quiet line of a signed-out YouTube Music", |st| {
        line(st, Service::Youtube).as_deref() == Some("Подсказки — после входа в аккаунт")
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(yt.calls("suggestions") + sp.calls("suggestions"), 0, "{:?} {:?}", yt.all_calls(), sp.all_calls());
    assert_eq!(line(&s.rt.state(), Service::Spotify), None, "switched off");
}

#[test]
fn a_rate_limited_feed_is_asked_again_after_the_pause() {
    let l = library(0, 0, 0, KNOWN);
    let yt = Arc::new(FakeService::youtube());
    yt.rate_limit_next_paused("suggestions", 1, Duration::from_millis(1500));
    let prefs = Prefs {
        spotify_enabled: false,
        deezer_enabled: false,
        ..Prefs::default()
    };
    let s = start_with(&l.lib, vec![yt.clone()], prefs, None, suggest_schedule());
    wait_until("the limited call", || yt.calls("suggestions") >= 1);
    let at = yt.calls("suggestions");
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(yt.calls("suggestions"), at, "nothing while paused");
    wait_for(&s.rt, "the suggestions after the pause", |st| read(st, Service::Youtube));
    assert_eq!(yt.calls("suggestions"), 3, "the same feed again, then the other: {:?}", yt.all_calls());
}

#[test]
fn an_old_spotify_sign_in_asks_quietly_for_a_new_one() {
    let l = library(0, 0, 0, KNOWN);
    let sp = Arc::new(FakeService::spotify().without_top_scope());
    let prefs = Prefs {
        youtube_enabled: false,
        deezer_enabled: false,
        ..Prefs::default()
    };
    let s = start_with(&l.lib, vec![sp.clone()], prefs, None, suggest_schedule());
    wait_for(&s.rt, "the reconnect note", |st| {
        st.account(Service::Spotify).is_some_and(|a| a.suggestions_note.is_some())
    });
    let st = s.rt.state();
    let a = st.account(Service::Spotify).unwrap();
    assert_eq!(a.suggestions_note.as_deref(), Some("Переподключи Spotify, чтобы брать твой топ"));
    assert!(a.suggestions.as_deref().unwrap().starts_with("Подсказки: 3 трека"), "{:?}", a.suggestions);
    assert!(
        !st.toasts.iter().any(|t| t.kind == nikgob_view::ToastKind::Error || t.text.contains("топ")),
        "no error toast: {:?}",
        st.toasts
    );
}

#[test]
fn nothing_goes_to_the_network_without_a_usable_service() {
    let l = library(0, 0, 0, KNOWN);
    let (yt, dz) = fakes();
    let prefs = Prefs {
        youtube_enabled: false,
        deezer_enabled: false,
        ..Prefs::default()
    };
    let sp = Arc::new(FakeService::spotify().logged_out());
    let s = start(&l.lib, vec![yt.clone(), dz.clone(), sp.clone()], prefs, None);
    wait_for(&s.rt, "the hero", |st| st.home.hero.is_some());
    s.rt.send(Command::StartRadio { seed: RadioSeed::Track(l.tracks[0]) });
    wait_for(&s.rt, "the radio", |st| st.queue.next.len() >= 3);
    s.rt.send(Command::PlayMix { id: MixId::Discover });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(yt.total_calls() + dz.total_calls() + sp.total_calls(), 0);
    assert!(s.rt.state().home.discover.is_empty());
}
