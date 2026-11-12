//! «Таймер сна»: the end of a track or of the album pauses with the next
//! entry waiting, the fade is the engine's own (the user's volume stays),
//! and turning the timer off gives the preload back.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, SleepChoice, Status};

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Play(u64),
    Next(Option<u64>),
    Fade(f32, Duration),
    Volume(f32),
    Pause,
    Resume,
    Stop,
}

struct Recorder(Arc<Mutex<Vec<Call>>>);

impl Recorder {
    fn push(&self, c: Call) {
        self.0.lock().unwrap().push(c);
    }
}

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.push(Call::Play(item.id));
    }
    fn set_next(&mut self, item: Option<PlayRequest>) {
        self.push(Call::Next(item.map(|i| i.id)));
    }
    fn pause(&mut self) {
        self.push(Call::Pause);
    }
    fn resume(&mut self) {
        self.push(Call::Resume);
    }
    fn stop(&mut self) {
        self.push(Call::Stop);
    }
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, v: f32) {
        self.push(Call::Volume(v));
    }
    fn position(&self) -> Duration {
        Duration::ZERO
    }
    fn set_sleep_fade(&mut self, level: f32, over: Duration) {
        self.push(Call::Fade(level, over));
    }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Rig {
    rt: Runtime,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl Rig {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn played(&self) -> Vec<u64> {
        self.calls().into_iter().filter_map(|c| if let Call::Play(id) = c { Some(id) } else { None }).collect()
    }

    fn last_next(&self) -> Option<Option<u64>> {
        self.calls().into_iter().rev().find_map(|c| if let Call::Next(n) = c { Some(n) } else { None })
    }

    fn title(&self) -> Option<String> {
        self.rt.state().player.current.as_ref().map(|t| t.title.clone())
    }
}

/// Plays an album of `n` tracks of 200 s; returns the first entry's id.
fn playing_album(n: usize) -> (Rig, u64) {
    playing_lengths(&vec![Some(200_000); n])
}

/// Plays tracks of these lengths (None: unknown, as a stream's).
fn playing_lengths(lengths: &[Option<u32>]) -> (Rig, u64) {
    let n = lengths.len();
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let tracks: Vec<_> = (0..n)
        .map(|i| {
            let mut t = Track::new(format!("t{i}"), ArtistCredit::single("A", Some(artist.id)));
            t.duration_ms = lengths[i];
            let t = lib.upsert_track(&t).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("t{i}.opus"), Codec::Opus)).unwrap();
            t.id
        })
        .collect();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib,
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(calls.clone())),
        Arc::new(|| {}),
    );
    let rig = Rig { rt, calls };
    rig.rt.send(Command::PlayTracks { tracks, start: 0, context: "test".into() });
    wait_for("the engine to be asked to play", || !rig.played().is_empty());
    let id = rig.played()[0];
    rig.rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    wait_for("playing", || rig.rt.state().player.status == Status::Playing);
    if n > 1 {
        wait_for("the next one preloaded", || matches!(rig.last_next(), Some(Some(_))));
    }
    // Far-off position reports right after a start are taken for stale ones.
    std::thread::sleep(Duration::from_millis(600));
    (rig, id)
}

#[test]
fn until_the_track_ends_nothing_follows_and_the_next_waits_paused() {
    let (rig, id) = playing_album(3);
    let volume = rig.rt.state().player.volume;
    let set_at = rig.calls().len();
    rig.rt.send(Command::SleepTimer(SleepChoice::TrackEnd));
    wait_for("the timer to show", || rig.rt.state().player.sleep.is_some());
    assert_eq!(rig.rt.state().player.sleep.as_ref().unwrap().choice, SleepChoice::TrackEnd);
    wait_for("the preload to go", || rig.last_next() == Some(None));
    // Ten seconds before the end: the engine fades, the volume stays.
    rig.rt.playback_sender().send(PlaybackEvent::Position { id, position: Duration::from_secs(190) }).unwrap();
    wait_for("a fade", || rig.calls().iter().any(|c| matches!(c, Call::Fade(l, _) if *l < 0.6)));
    assert_eq!(rig.rt.state().player.volume, volume);
    assert!(!rig.calls()[set_at..].iter().any(|c| matches!(c, Call::Volume(_))), "{:?}", rig.calls());
    rig.rt.playback_sender().send(PlaybackEvent::Ended { id }).unwrap();
    wait_for("paused on the next track", || {
        let s = rig.rt.state();
        s.player.status == Status::Paused && s.player.sleep.is_none()
    });
    assert_eq!(rig.title().as_deref(), Some("t1"));
    assert_eq!(rig.played().len(), 1, "nothing else was started");
    assert_eq!(rig.rt.state().player.position_now(), Duration::ZERO);
    // Play again: full sound at once, then the next track.
    rig.rt.send(Command::TogglePlay);
    wait_for("the next track to play", || rig.played().len() == 2);
    let calls = rig.calls();
    let play_at = calls.iter().rposition(|c| matches!(c, Call::Play(_))).unwrap();
    let back = calls[..play_at].iter().rposition(|c| matches!(c, Call::Fade(..))).unwrap();
    assert_eq!(calls[back], Call::Fade(1.0, Duration::ZERO));
    assert_eq!(rig.rt.state().player.volume, volume);
}

#[test]
fn until_the_album_ends_the_last_entry_stops_it() {
    let (rig, first) = playing_album(2);
    rig.rt.send(Command::SleepTimer(SleepChoice::ContextEnd));
    wait_for("the timer to show", || rig.rt.state().player.sleep.is_some());
    // The first track ends as usual: the second takes over.
    let Some(Some(second)) = rig.last_next() else { panic!("no preload") };
    rig.rt.playback_sender().send(PlaybackEvent::TrackChanged { to: second }).unwrap();
    wait_for("the second track", || rig.title().as_deref() == Some("t1"));
    assert_ne!(first, second);
    wait_for("no preload after the last", || rig.last_next() == Some(None));
    assert!(rig.rt.state().player.sleep.is_some());
    rig.rt.playback_sender().send(PlaybackEvent::Ended { id: second }).unwrap();
    wait_for("the timer to end", || rig.rt.state().player.sleep.is_none());
    assert_ne!(rig.rt.state().player.status, Status::Playing);
}

#[test]
fn time_left_counts_the_rest_of_the_album() {
    let (rig, _) = playing_album(3);
    rig.rt.send(Command::SleepTimer(SleepChoice::ContextEnd));
    // Counted on the next second's poll.
    wait_for("what follows to be counted", || {
        rig.rt.state().player.sleep.as_ref().is_some_and(|s| s.after_current == Some(Duration::from_secs(400)))
    });
    let s = rig.rt.state();
    let left = s.player.sleep.as_ref().unwrap().left(&s.player, Instant::now()).unwrap();
    assert!(left > Duration::from_secs(590) && left <= Duration::from_secs(600), "{left:?}");
}

#[test]
fn turning_the_timer_off_gives_the_preload_back() {
    let (rig, _) = playing_album(3);
    rig.rt.send(Command::SleepTimer(SleepChoice::TrackEnd));
    wait_for("the preload to go", || rig.last_next() == Some(None));
    rig.rt.send(Command::SleepTimer(SleepChoice::Off));
    wait_for("the timer to go", || rig.rt.state().player.sleep.is_none());
    wait_for("the preload back", || matches!(rig.last_next(), Some(Some(_))));
}

#[test]
fn minutes_show_their_deadline_and_a_pause_keeps_counting() {
    let (rig, _) = playing_album(1);
    let before = Instant::now();
    rig.rt.send(Command::SleepTimer(SleepChoice::Minutes(15)));
    wait_for("the timer to show", || rig.rt.state().player.sleep.is_some());
    let deadline = rig.rt.state().player.sleep.as_ref().unwrap().deadline.unwrap();
    assert!(deadline >= before + Duration::from_secs(15 * 60) && deadline <= Instant::now() + Duration::from_secs(15 * 60));
    rig.rt.send(Command::Pause);
    wait_for("paused", || rig.rt.state().player.status == Status::Paused);
    std::thread::sleep(Duration::from_millis(1200));
    // The clock goes on: the deadline does not move.
    assert_eq!(rig.rt.state().player.sleep.as_ref().unwrap().deadline, Some(deadline));
    // A longer one replaces it.
    rig.rt.send(Command::SleepTimer(SleepChoice::Minutes(30)));
    wait_for("the new deadline", || {
        rig.rt.state().player.sleep.as_ref().and_then(|s| s.deadline).is_some_and(|d| d > deadline)
    });
    rig.rt.send(Command::SleepTimer(SleepChoice::Off));
    wait_for("the timer to go", || rig.rt.state().player.sleep.is_none());
    assert!(!rig.calls().iter().any(|c| matches!(c, Call::Fade(..))), "no fade far from the end");
}

fn fades(rig: &Rig) -> Vec<f32> {
    rig.calls().into_iter().filter_map(|c| if let Call::Fade(l, _) = c { Some(l) } else { None }).collect()
}

/// Review 1: repeat-one never reaches the album's last entry; the album
/// timer ends with the playing track then.
#[test]
fn under_repeat_one_the_album_timer_ends_with_the_track() {
    let (rig, first) = playing_album(3);
    rig.rt.send(Command::SleepTimer(SleepChoice::ContextEnd));
    rig.rt.send(Command::CycleRepeat); // off -> all
    rig.rt.send(Command::CycleRepeat); // all -> one
    wait_for("repeat one", || rig.rt.state().player.repeat == nikgob_view::Repeat::One);
    wait_for("no preload after the looping track", || rig.last_next() == Some(None));
    rig.rt.playback_sender().send(PlaybackEvent::Ended { id: first }).unwrap();
    wait_for("the timer to end", || rig.rt.state().player.sleep.is_none());
    assert_eq!(rig.rt.state().player.status, Status::Paused);
    assert_eq!(rig.played().len(), 1, "nothing else was started");
}

/// Review 2: the engine had already gone on to the next track (a
/// crossfade or a preload taken before the timer cleared it): that is the
/// boundary, the next track is paused at its start and silent.
#[test]
fn a_transition_the_engine_made_anyway_is_the_boundary() {
    let (rig, _) = playing_album(3);
    let Some(Some(second)) = rig.last_next() else { panic!("no preload") };
    rig.rt.send(Command::SleepTimer(SleepChoice::TrackEnd));
    wait_for("the timer to show", || rig.rt.state().player.sleep.is_some());
    rig.rt.playback_sender().send(PlaybackEvent::TrackChanged { to: second }).unwrap();
    wait_for("paused on the next track", || {
        let s = rig.rt.state();
        s.player.status == Status::Paused && s.player.sleep.is_none()
    });
    assert_eq!(rig.title().as_deref(), Some("t1"));
    let calls = rig.calls();
    let pause = calls.iter().rposition(|c| *c == Call::Pause).expect("paused in the engine");
    assert!(calls[..pause].contains(&Call::Fade(0.0, Duration::ZERO)), "{calls:?}");
    // Played again: full sound.
    rig.rt.send(Command::TogglePlay);
    wait_for("full sound", || fades(&rig).last() == Some(&1.0));
}

/// Review 3: a skip in the fade to a track whose length is unknown gets the
/// full sound back, not the old track's faded level.
#[test]
fn a_skip_in_the_fade_to_a_track_of_unknown_length_sounds_full() {
    let (rig, first) = playing_lengths(&[Some(200_000), None, Some(200_000)]);
    rig.rt.send(Command::SleepTimer(SleepChoice::TrackEnd));
    rig.rt.playback_sender().send(PlaybackEvent::Position { id: first, position: Duration::from_secs(195) }).unwrap();
    wait_for("a deep fade", || fades(&rig).iter().any(|l| *l < 0.3));
    rig.rt.send(Command::Next);
    wait_for("the next track", || rig.played().len() == 2);
    let calls = rig.calls();
    let play = calls.iter().rposition(|c| matches!(c, Call::Play(_))).unwrap();
    let before = calls[..play].iter().rev().find_map(|c| if let Call::Fade(l, _) = c { Some(*l) } else { None });
    assert_eq!(before, Some(1.0), "{calls:?}");
    let id = rig.played()[1];
    rig.rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(fades(&rig).last(), Some(&1.0), "{:?}", rig.calls());
    let s = rig.rt.state();
    assert_eq!(s.player.sleep.as_ref().unwrap().left(&s.player, Instant::now()), None);
}

/// Review 4: a move in the queue counts the album's rest again.
#[test]
fn a_move_in_the_queue_counts_the_rest_again() {
    let (rig, _) = playing_album(3);
    rig.rt.send(Command::SleepTimer(SleepChoice::ContextEnd));
    wait_for("counted", || {
        rig.rt.state().player.sleep.as_ref().is_some_and(|s| s.after_current == Some(Duration::from_secs(400)))
    });
    let (second, third) = {
        let s = rig.rt.state();
        (s.queue.next[0].entry, s.queue.next[1].entry)
    };
    // The second goes after the album's last: one track to its end.
    rig.rt.send(Command::MoveInQueue { entry: second, anchor: third, after: true });
    wait_for("counted again", || {
        rig.rt.state().player.sleep.as_ref().is_some_and(|s| s.after_current == Some(Duration::from_secs(200)))
    });
}

/// Review 5: a track without a length before the end: how far the end is
/// is unknown, so nothing fades yet.
#[test]
fn an_unknown_length_on_the_way_fades_nothing_yet() {
    let (rig, first) = playing_lengths(&[Some(200_000), None, Some(200_000)]);
    rig.rt.send(Command::SleepTimer(SleepChoice::ContextEnd));
    wait_for("the timer to show", || rig.rt.state().player.sleep.is_some());
    rig.rt.playback_sender().send(PlaybackEvent::Position { id: first, position: Duration::from_secs(195) }).unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    assert!(fades(&rig).iter().all(|l| *l >= 1.0), "{:?}", rig.calls());
    assert_eq!(rig.rt.state().player.sleep.as_ref().unwrap().after_current, None);
}
