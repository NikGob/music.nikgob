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

/// Starts the playlist `id` and waits for its first track to be asked for
/// (a queue of `n` entries at least).
fn play(s: &Started, id: PlaylistId, n: usize) {
    let asked = s.asked.lock().unwrap().len();
    s.rt.send(Command::PlayPlaylist { id, start: None, shuffle: false });
    wait_until("the playlist plays", || s.asked.lock().unwrap().len() > asked);
    wait_for(&s.rt, "the playlist's queue", |st| st.queue.prev.is_empty() && st.queue.next.len() + 1 >= n);
}

/// The guests ahead.
fn guests_ahead(st: &ViewState) -> Vec<u64> {
    st.queue.next.iter().filter(|e| e.guest).map(|e| e.entry).collect()
}

/// The place of the first guest ahead: the guest after the third own track
/// (the one playing the first) is at 2.
fn guest_place(st: &ViewState) -> Option<usize> {
    st.queue.next.iter().position(|e| e.guest)
}

/// Each guest keeps being a guest after a restart and after «Очистить» →
/// «Отменить»: the chip off still takes it out, and no second comes.
#[test]
fn a_guest_stays_a_guest_after_a_restart_and_an_undo() {
    let (lib, id, tracks) = library();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prefs.json");
    // (Without autoplay: a cleared queue would go on with a radio.)
    let s = start(&lib, Prefs { plus_playlists: vec![id], autoplay: false, ..prefs() }, Some(&path));
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    play(&s, id, tracks.len());
    wait_for(&s.rt, "a guest", |st| guest_place(st) == Some(2));
    let guest = guests_ahead(&s.rt.state())[0];

    // «Очистить» → «Отменить»: the guest is marked again, alone.
    s.rt.send(Command::ClearQueue);
    wait_for(&s.rt, "the queue cleared", |st| st.queue.next.is_empty());
    s.rt.send(Command::UndoClearQueue);
    wait_for(&s.rt, "the queue back", |st| st.queue.next.len() == tracks.len());
    std::thread::sleep(Duration::from_millis(300));
    let st = s.rt.state();
    assert_eq!(guest_place(&st), Some(2), "{:?}", rows(&st.queue.next));
    assert_eq!(guests_ahead(&st).len(), 1, "{:?}", rows(&st.queue.next));
    assert_ne!(guests_ahead(&st)[0], guest, "a fresh entry");
    drop(st);
    drop(s);

    // A restart: the same.
    let s = start(&lib, Prefs { plus_playlists: vec![id], autoplay: false, ..prefs() }, Some(&path));
    wait_for(&s.rt, "the queue restored", |st| st.queue.next.len() == tracks.len());
    std::thread::sleep(Duration::from_millis(500));
    let st = s.rt.state();
    assert_eq!(guest_place(&st), Some(2), "{:?}", rows(&st.queue.next));
    assert_eq!(guests_ahead(&st).len(), 1, "{:?}", rows(&st.queue.next));
    drop(st);
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: false }));
    wait_for(&s.rt, "the guest out after the restart", |st| guests_ahead(st).is_empty() && st.queue.next.len() == tracks.len() - 1);
}

/// ♡ on the guest ahead keeps it where it is: no second guest comes beside
/// it, and the chip off leaves it.
#[test]
fn a_kept_guest_still_counts() {
    let (lib, id, tracks) = library();
    let s = start(&lib, Prefs { plus_playlists: vec![id], ..prefs() }, None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    play(&s, id, tracks.len());
    wait_for(&s.rt, "a guest", |st| guest_place(st) == Some(2));
    let track = s.rt.state().queue.next[2].track.id;
    s.rt.send(Command::Like(nikgob_view::LikeTarget::Track(track), true));
    std::thread::sleep(Duration::from_millis(300));
    let st = s.rt.state();
    assert_eq!(guests_ahead(&st).len(), 1, "{:?}", rows(&st.queue.next));
    assert_eq!(st.queue.next[2].track.id, track);
    drop(st);
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: false }));
    wait_for(&s.rt, "the chip off", |st| !st.prefs.plus_playlists.contains(&id));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(s.rt.state().queue.next[2].track.id, track, "kept");
}

/// Shuffled, the guest ahead goes back to its place (one ahead, never two);
/// dragged by the user, it stays where it was put and none other comes.
#[test]
fn shuffle_and_a_drag_keep_one_guest_ahead() {
    let (lib, id, tracks) = library();
    // The playlist from its first track: shuffle turned off later goes back
    // to it with the whole playlist ahead. (Started shuffled, it would go
    // back to wherever the random first track is in the playlist, and near
    // its end no place for a guest is ahead at all.)
    let in_order = nikgob_view::ListShuffles::all(nikgob_view::ListShuffle::InOrder);
    let s = start(&lib, Prefs { plus_playlists: vec![id], list_shuffle: in_order, ..prefs() }, None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    play(&s, id, tracks.len());
    wait_for(&s.rt, "a guest", |st| guest_place(st) == Some(2));
    for k in 0..4 {
        let shuffled = k % 2 == 0;
        s.rt.send(Command::ToggleShuffle);
        // The guest goes back to its place in the same step as the shuffle
        // (never two ahead, never none, whenever looked at).
        wait_for(&s.rt, "the shuffle", |st| {
            assert_eq!(guests_ahead(st).len(), 1, "{:?}", rows(&st.queue.next));
            st.player.shuffle == shuffled
        });
        let st = s.rt.state();
        assert_eq!(guests_ahead(&st).len(), 1, "{:?}", rows(&st.queue.next));
        assert_eq!(guest_place(&st), Some(2), "{:?}", rows(&st.queue.next));
    }
    let st = s.rt.state();
    let guest = guests_ahead(&st)[0];
    let last = st.queue.next.last().unwrap().entry;
    drop(st);
    s.rt.send(Command::MoveInQueue { entry: guest, anchor: last, after: true });
    wait_for(&s.rt, "the guest at the end", |st| st.queue.next.last().map(|e| e.entry) == Some(guest));
    std::thread::sleep(Duration::from_millis(300));
    let st = s.rt.state();
    assert_eq!(guests_ahead(&st), vec![guest], "{:?}", rows(&st.queue.next));
}

/// «Дальше» on a guest is a skip whatever its position: no more guests.
#[test]
fn next_late_in_a_guest_is_a_skip() {
    let (lib, id, tracks) = library();
    let s = start(&lib, Prefs { plus_playlists: vec![id], ..prefs() }, None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    play(&s, id, tracks.len());
    wait_for(&s.rt, "a guest", |st| guest_place(st) == Some(2));
    for _ in 0..3 {
        end_current(&s);
    }
    wait_for(&s.rt, "the guest plays", |st| st.queue.current_guest);
    let req = s.asked.lock().unwrap().last().unwrap().id;
    s.rt.playback_sender().send(PlaybackEvent::Started { id: req }).unwrap();
    s.rt.playback_sender().send(PlaybackEvent::Position { id: req, position: Duration::from_secs(10_000) }).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    s.rt.send(Command::Next);
    wait_for(&s.rt, "the next own track", |st| !st.queue.current_guest);
    for _ in 0..3 {
        end_current(&s);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(guests_ahead(&s.rt.state()).is_empty(), "{:?}", rows(&s.rt.state().queue.next));
}

/// Tracks queued by hand are not the playlist's even when they are its
/// songs: a playlist of two with three of them queued after it gets no
/// guest when the chip goes on.
#[test]
fn hand_queued_tracks_are_not_the_playlists() {
    let (lib, _, tracks) = library();
    let two = lib.create_playlist("Два").unwrap();
    lib.playlist_add_many(two.id, &tracks[..2]).unwrap();
    let s = start(&lib, prefs(), None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    play(&s, two.id, 2);
    s.rt.send(Command::Enqueue(tracks[..3].to_vec()));
    wait_for(&s.rt, "the hand-queued tracks", |st| st.queue.next.len() == 4);
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: two.id, on: true }));
    wait_for(&s.rt, "the chip on", |st| st.prefs.plus_playlists.contains(&two.id));
    std::thread::sleep(Duration::from_millis(500));
    assert!(guests_ahead(&s.rt.state()).is_empty(), "{:?}", rows(&s.rt.state().queue.next));
}

/// A place passed without a guest (none could come then) is owed: the
/// guest comes right after the track playing as soon as one can.
#[test]
fn a_missed_place_is_owed() {
    let (lib, id, tracks) = library();
    let s = start(&lib, Prefs { plus_playlists: vec![id], mixes_downloaded_only: true, ..prefs() }, None);
    wait_for(&s.rt, "the home", |st| st.home.hero.is_some());
    play(&s, id, tracks.len());
    for _ in 0..4 {
        end_current(&s);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(guests_ahead(&s.rt.state()).is_empty());
    s.rt.send(Command::SetPref(PrefPatch::MixesDownloadedOnly(false)));
    wait_for(&s.rt, "the owed guest", |st| guest_place(st) == Some(0));
    assert_eq!(guests_ahead(&s.rt.state()).len(), 1);
}

/// Repeat one plays no guest; the chip off takes out the guests that
/// played too (repeat all would bring them back).
#[test]
fn repeat_and_the_guests_that_played() {
    let (lib, id, tracks) = library();
    let s = start(&lib, Prefs { plus_playlists: vec![id], ..prefs() }, None);
    wait_for(&s.rt, "discoveries", |st| !st.home.discover.is_empty());
    play(&s, id, tracks.len());
    wait_for(&s.rt, "a guest", |st| guest_place(st) == Some(2));
    for _ in 0..4 {
        end_current(&s);
    }
    wait_for(&s.rt, "the guest played", |st| st.queue.prev.iter().any(|e| e.guest));
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: false }));
    wait_for(&s.rt, "the played guest out", |st| !st.queue.prev.iter().any(|e| e.guest) && guests_ahead(st).is_empty());
    assert_eq!(s.rt.state().queue.prev.len(), 3, "the own ones stay");

    // Repeat one: nothing joins.
    s.rt.send(Command::CycleRepeat);
    s.rt.send(Command::CycleRepeat);
    wait_for(&s.rt, "repeat one", |st| st.player.repeat == nikgob_view::Repeat::One);
    s.rt.send(Command::SetPref(PrefPatch::Plus { playlist: id, on: true }));
    play(&s, id, tracks.len());
    std::thread::sleep(Duration::from_millis(500));
    assert!(guests_ahead(&s.rt.state()).is_empty(), "{:?}", rows(&s.rt.state().queue.next));
}
