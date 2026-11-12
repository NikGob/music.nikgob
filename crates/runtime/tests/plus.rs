//! «✦ С дополнением» in the runtime (docs/recommendations.md §4): a
//! playlist with its chip on plays as it is, shuffled or not, with a new
//! song after every three of its own, queued only a few ahead; a skip of a
//! guest or the chip turned off takes the guests out of the queue, never
//! the playlist's own tracks nor the playlist; the chip is remembered.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{
    Analysis, AnalysisSource, Artist, ArtistCredit, Codec, Event, EventContext, EventType, PlaylistId, Release, Source, SourceKind,
    Track, TrackId,
};
use nikgob_core::streaming::{FakeService, StreamService};
use nikgob_library::{Library, now_ms};
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig, StreamSchedule};
use nikgob_view::{Command, PrefPatch, Prefs, QueueEntry, ViewState};

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

struct Started {
    rt: Runtime,
    asked: Arc<Mutex<Vec<PlayRequest>>>,
}

fn start(lib: &Library, prefs: Prefs, prefs_path: Option<&Path>) -> Started {
    let yt: Arc<dyn StreamService> = Arc::new(FakeService::youtube().logged_out());
    let dz: Arc<dyn StreamService> = Arc::new(FakeService::deezer());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let schedule = StreamSchedule {
        tick: Duration::from_millis(50),
        analysis_start: Duration::ZERO,
        analysis_idle: Duration::from_secs(60),
        recs_debounce: Duration::from_millis(20),
        discover_gap: (Duration::from_millis(1), Duration::from_millis(5)),
        ..StreamSchedule::default()
    };
    let rt = Runtime::start_scheduled(
        RuntimeConfig {
            library: lib.clone(),
            streams: vec![yt, dz],
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
    panic!("timed out waiting for {what}: discover {}, next {:?}", s.home.discover.len(), rows(&s.queue.next));
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn rows(list: &[QueueEntry]) -> Vec<String> {
    list.iter().map(|e| format!("{}{}", if e.guest { "✦ " } else { "" }, e.track.title)).collect()
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

const KNOWN: &[(&str, &[&str])] = &[
    ("Кино", &["Группа крови", "Война", "Спокойная ночь", "Закрой за мной дверь, я ухожу", "Кукушка", "Звезда по имени Солнце"]),
    ("Daft Punk", &["One More Time", "Aerodynamic", "Digital Love", "Harder, Better, Faster, Stronger"]),
];

/// The known songs with files, sound, plays and a like each, and a
/// playlist of all of them.
fn library() -> (Library, PlaylistId, Vec<TrackId>) {
    let lib = Library::open_in_memory().unwrap();
    let now = now_ms();
    let mut tracks = Vec::new();
    for (style, (name, titles)) in KNOWN.iter().enumerate() {
        let a = lib.upsert_artist(&Artist::new(*name)).unwrap();
        let credit = ArtistCredit::single(*name, Some(a.id));
        let r = lib.upsert_release(&Release::new(format!("{name} album"), credit.clone())).unwrap();
        for (n, title) in titles.iter().enumerate() {
            let mut t = Track::new(*title, credit.clone());
            t.release_id = Some(r.id);
            t.track_no = n as u32 + 1;
            t.duration_ms = Some(200_000);
            let t = lib.upsert_track(&t).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("missing/{name}/{n}.flac"), Codec::Flac)).unwrap();
            let v: Vec<f32> = (0..nikgob_recs::DIM).map(|i| ((style * 7 + i * 3) % 11) as f32 * 0.8 + n as f32 * 0.05).collect();
            lib.put_analysis(&Analysis { track_id: t.id, bliss_vector: v, analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(), source: AnalysisSource::File })
                .unwrap();
            for k in 0..2 {
                let ts = now - (3 + (n * 3 + k * 5) as i64 % 20) * DAY;
                for kind in [EventType::Start, EventType::Complete] {
                    let mut e = Event::new(t.id, kind, ts + i64::from(kind == EventType::Complete) * 200_000, lib.device_id());
                    e.context = EventContext::Album;
                    lib.record_event_raw(&e).unwrap();
                }
            }
            lib.like(t.id).unwrap();
            tracks.push(t.id);
        }
    }
    let p = lib.create_playlist("Всё").unwrap();
    lib.playlist_add_many(p.id, &tracks).unwrap();
    (lib, p.id, tracks)
}

fn prefs() -> Prefs {
    // The graph's pass asks the same services for itself (tests/graph.rs).
    Prefs { graph_services: false, graph_open_data: false, ..Prefs::default() }
}

fn playlist_of(lib: &Library, id: PlaylistId) -> Vec<TrackId> {
    lib.playlist_entries(id, 0, u32::MAX).unwrap().into_iter().map(|e| e.track.id).collect()
}

/// The queue's own tracks (not guests) in play order, the current one in
/// its place.
fn own_order(st: &ViewState) -> Vec<TrackId> {
    let q = &st.queue;
    let mut out: Vec<TrackId> = q.prev.iter().filter(|e| !e.guest).map(|e| e.track.id).collect();
    if !q.current_guest {
        out.extend(q.current.as_ref().map(|t| t.id));
    }
    out.extend(q.next.iter().filter(|e| !e.guest).map(|e| e.track.id));
    out
}

/// The guest comes after the third own track, only a few ahead; the play
/// order of the playlist is the same as without it, the playlist the same;
/// a skip of the guest takes the guests away for this queue.
#[test]
fn a_guest_after_every_three_and_a_skip_sends_them_away() {
    let (lib, id, tracks) = library();
    let s = start(&lib, Prefs { plus_playlists: vec![id], ..prefs() }, None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    let before = playlist_of(&lib, id);
    for shuffle in [false, true] {
        let asked = s.asked.lock().unwrap().len();
        s.rt.send(Command::PlayPlaylist { id, start: None, shuffle });
        wait_until("the playlist plays", || s.asked.lock().unwrap().len() > asked);
        wait_for(&s.rt, "a guest", |st| st.queue.prev.is_empty() && st.queue.next.iter().any(|e| e.guest));
        let st = s.rt.state();
        // (A plain play shuffles as the setting of own playlists says.)
        assert!(!shuffle || st.player.shuffle);
        // Playing the first: two own, then the guest, then the rest.
        let next = &st.queue.next;
        assert!(!st.queue.current_guest && !next[0].guest && !next[1].guest && next[2].guest, "{:?}", rows(next));
        assert_eq!(next.iter().filter(|e| e.guest).count(), 1, "one ahead at a time: {:?}", rows(next));
        assert!(!tracks.contains(&next[2].track.id), "a new song");
        // Every own track is in the queue once, as the playlist has them.
        let mut own = own_order(&st);
        own.sort();
        let mut all = tracks.clone();
        all.sort();
        assert_eq!(own, all);
    }
    let order = own_order(&s.rt.state());
    // The three own tracks play out, the guest plays.
    for _ in 0..3 {
        end_current(&s);
    }
    wait_for(&s.rt, "the guest plays", |st| st.queue.current_guest);
    // Skipped: no guest comes again while this queue plays.
    s.rt.send(Command::Next);
    wait_for(&s.rt, "the next own track", |st| !st.queue.current_guest && st.queue.prev.len() == 4);
    for _ in 0..4 {
        end_current(&s);
    }
    let st = s.rt.state();
    assert!(st.queue.next.iter().chain(&st.queue.prev).filter(|e| e.guest).count() <= 1, "{:?}", rows(&st.queue.next));
    assert!(!st.queue.next.iter().any(|e| e.guest), "{:?}", rows(&st.queue.next));
    assert_eq!(own_order(&st), order, "the playlist plays as it would");
    assert_eq!(playlist_of(&lib, id), before, "the playlist itself untouched");
}

/// The chip turned off takes the guest ahead out at once and leaves every
/// own track; on again, a guest comes back.
#[test]
fn the_chip_off_takes_the_guests_out() {
    let (lib, id, tracks) = library();
    let s = start(&lib, Prefs { plus_playlists: vec![id], ..prefs() }, None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    s.rt.send(Command::PlayPlaylist { id, start: None, shuffle: false });
    wait_for(&s.rt, "a guest", |st| st.queue.next.iter().any(|e| e.guest));
    let order = own_order(&s.rt.state());
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: false }));
    wait_for(&s.rt, "the guest out", |st| !st.queue.next.iter().any(|e| e.guest) && !st.prefs.plus_playlists.contains(&id));
    let st = s.rt.state();
    assert_eq!(own_order(&st), order);
    assert_eq!(st.queue.next.len(), tracks.len() - 1);
    drop(st);
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: true }));
    wait_for(&s.rt, "a guest again", |st| st.queue.next.iter().any(|e| e.guest));
    // Another playlist plays: nothing joins it.
    let other = lib.create_playlist("Другой").unwrap();
    lib.playlist_add_many(other.id, &tracks[..5]).unwrap();
    s.rt.send(Command::PlayPlaylist { id: other.id, start: None, shuffle: false });
    wait_for(&s.rt, "the other playlist", |st| st.queue.next.len() == 4);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!s.rt.state().queue.next.iter().any(|e| e.guest));
}

/// The chip of each playlist is kept with the prefs and comes back after a
/// restart; «Только скачанное» queues no guest.
#[test]
fn the_chip_is_remembered() {
    let (lib, id, _) = library();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prefs.json");
    let s = start(&lib, prefs(), Some(&path));
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: true }));
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: nikgob_view::LIKED_PLAYLIST, on: true }));
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: nikgob_view::LIKED_PLAYLIST, on: false }));
    wait_for(&s.rt, "the chip on", |st| st.prefs.plus_playlists == vec![id]);
    drop(s);
    let saved: Prefs = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved.plus_playlists, vec![id]);

    let s = start(&lib, Prefs { mixes_downloaded_only: true, ..saved }, None);
    wait_for(&s.rt, "the home", |st| st.home.hero.is_some());
    s.rt.send(Command::PlayPlaylist { id, start: None, shuffle: false });
    wait_for(&s.rt, "the playlist", |st| st.queue.next.len() >= 3);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!s.rt.state().queue.next.iter().any(|e| e.guest));
}
