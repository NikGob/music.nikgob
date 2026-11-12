//! «Скорость»: the version a track plays as is remembered with it, handed
//! to the engine with every request, changed while it plays, and switched
//! off as a whole from the settings without losing the choices.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track, TrackId};
use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, EQ_BANDS, Prefs, SpeedVariant, Status};

/// What the engine was asked.
#[derive(Clone, Debug, PartialEq)]
enum Asked {
    Play(u64, SpeedVariant, bool),
    Next(Option<(SpeedVariant, bool)>),
    Speed(u64, SpeedVariant, bool),
    Eq(bool, [f32; EQ_BANDS]),
}

struct Recorder(Arc<Mutex<Vec<Asked>>>);

impl Playback for Recorder {
    fn play(&mut self, item: PlayRequest) {
        self.0.lock().unwrap().push(Asked::Play(item.id, item.speed, item.keep_timbre));
    }
    fn set_next(&mut self, item: Option<PlayRequest>) {
        self.0.lock().unwrap().push(Asked::Next(item.map(|i| (i.speed, i.keep_timbre))));
    }
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
    fn set_equalizer(&mut self, on: bool, gains: [f32; EQ_BANDS]) {
        self.0.lock().unwrap().push(Asked::Eq(on, gains));
    }
    fn set_speed(&mut self, id: u64, speed: SpeedVariant, keep_timbre: bool) {
        self.0.lock().unwrap().push(Asked::Speed(id, speed, keep_timbre));
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
    asked: Arc<Mutex<Vec<Asked>>>,
    lib: Library,
    tracks: [TrackId; 2],
}

impl Rig {
    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }

    /// Reads the view. The state's lock is let go before this returns: a
    /// test that kept it while waiting for the runtime would wait for ever
    /// (the runtime needs it to publish).
    fn view<T>(&self, f: impl FnOnce(&nikgob_view::ViewState) -> T) -> T {
        f(&self.rt.state())
    }

    fn speed(&self) -> f32 {
        self.view(|s| s.player.speed)
    }

    fn rate(&self) -> f32 {
        self.view(|s| s.player.rate)
    }

    fn version_of(&self, track: TrackId) -> SpeedVariant {
        self.view(|s| s.speeds.of(track))
    }

    fn plays(&self) -> Vec<(u64, SpeedVariant, bool)> {
        self.asked()
            .into_iter()
            .filter_map(|a| match a {
                Asked::Play(id, speed, keep) => Some((id, speed, keep)),
                _ => None,
            })
            .collect()
    }

    fn speeds(&self) -> Vec<(u64, SpeedVariant, bool)> {
        self.asked()
            .into_iter()
            .filter_map(|a| match a {
                Asked::Speed(id, speed, keep) => Some((id, speed, keep)),
                _ => None,
            })
            .collect()
    }

    /// Plays both tracks from the first; returns the playing entry.
    fn play(&self) -> u64 {
        let before = self.plays().len();
        self.rt.send(Command::PlayTracks {
            tracks: self.tracks.to_vec(),
            start: 0,
            context: "test".into(),
        });
        wait_for("a play request", || self.plays().len() > before);
        let id = self.plays()[before].0;
        self.rt.playback_sender().send(PlaybackEvent::Started { id }).unwrap();
        wait_for("playing", || self.view(|s| s.player.status) == Status::Playing);
        id
    }
}

fn rig(prefs: Prefs, chosen: &[(usize, &str)]) -> Rig {
    let lib = Library::open_in_memory().unwrap();
    let artist = lib.upsert_artist(&Artist::new("A")).unwrap();
    let tracks = ["one", "two"].map(|title| {
        let track = lib
            .upsert_track(&Track::new(title, ArtistCredit::single("A", Some(artist.id))))
            .unwrap();
        lib.add_source(&Source::new(track.id, SourceKind::LocalFile, format!("{title}.opus"), Codec::Opus))
            .unwrap();
        track.id
    });
    for (i, key) in chosen {
        lib.set_track_fx(tracks[*i], Some(key)).unwrap();
    }
    let asked = Arc::new(Mutex::new(Vec::new()));
    // A game on the machine running the tests must not hold anything back.
    nikgob_runtime::set_background_quiet(false);
    let rt = Runtime::start(
        RuntimeConfig {
            library: lib.clone(),
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs,
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(Recorder(asked.clone())),
        Arc::new(|| {}),
    );
    let r = Rig { rt, asked, lib, tracks };
    // The runtime starts on its own thread: the equalizer is the last
    // thing it tells the engine before it reads the tracks' versions.
    wait_for("the runtime to start", || r.asked().iter().any(|a| matches!(a, Asked::Eq(..))));
    for (i, key) in chosen {
        let want = SpeedVariant::from_key(key).unwrap();
        wait_for("the versions to be read", || r.view(|s| s.speeds.chosen.get(&r.tracks[*i]).copied()) == Some(want));
    }
    r
}

#[test]
fn a_track_plays_as_the_version_remembered_with_it() {
    let r = rig(Prefs::default(), &[(0, "nightcore")]);
    assert_eq!(r.version_of(r.tracks[0]), SpeedVariant::Nightcore);
    let id = r.play();
    assert_eq!(r.plays(), vec![(id, SpeedVariant::Nightcore, false)]);
    wait_for("the view", || r.speed() == 1.3);
    assert_eq!(r.rate(), 1.3, "the position moves at the speed of the track");
    // The next track has no choice: preloaded as recorded.
    wait_for("the preload", || r.asked().contains(&Asked::Next(Some((SpeedVariant::Normal, false)))));
}

#[test]
fn a_new_choice_is_saved_and_heard_at_once_without_moving_the_position() {
    let r = rig(Prefs::default(), &[]);
    let id = r.play();
    let shown = || r.view(|s| s.player.position_now());
    r.rt.send(Command::Seek(Duration::from_secs(60)));
    wait_for("the seek", || shown() >= Duration::from_secs(59));
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::SlowEcho });
    wait_for("the engine to be told", || !r.speeds().is_empty());
    assert_eq!(r.speeds(), vec![(id, SpeedVariant::SlowEcho, false)]);
    wait_for("the view", || r.speed() == 0.85);
    assert_eq!(r.version_of(r.tracks[0]), SpeedVariant::SlowEcho);
    assert_eq!(r.rate(), 0.85);
    let at = shown();
    assert!(at >= Duration::from_secs(59) && at < Duration::from_secs(63), "{at:?}");
    assert_eq!(r.lib.track_fx().unwrap(), vec![(r.tracks[0], "slow_echo".to_owned())]);
    // The same choice again changes nothing for the engine.
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::SlowEcho });
    // Back to as recorded: the row is gone.
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::Normal });
    wait_for("as recorded again", || r.speed() == 1.0);
    assert_eq!(r.speeds().len(), 2);
    assert!(r.lib.track_fx().unwrap().is_empty());
}

#[test]
fn a_choice_for_the_next_track_reaches_it_in_the_engine_and_leaves_the_playing_one_alone() {
    let r = rig(Prefs::default(), &[]);
    let id = r.play();
    let nexts = |r: &Rig| r.asked().into_iter().filter(|a| matches!(a, Asked::Next(_))).count();
    // The preload asks twice (cleared while the tags are read, then the
    // item): wait for the item itself.
    wait_for("the preload", || r.asked().contains(&Asked::Next(Some((SpeedVariant::Normal, false)))));
    let before = nexts(&r);
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[1], variant: SpeedVariant::Fast });
    wait_for("the engine to be told", || !r.speeds().is_empty());
    let speeds = r.speeds();
    assert_eq!(speeds.len(), 1);
    assert_ne!(speeds[0].0, id, "the playing track was not touched");
    assert_eq!((speeds[0].1, speeds[0].2), (SpeedVariant::Fast, false));
    // The engine may be producing the next track already: it is told about
    // the item it has, not handed a second one.
    assert_eq!(nexts(&r), before);
    assert_eq!(r.speed(), 1.0);
    assert_eq!(r.version_of(r.tracks[1]), SpeedVariant::Fast);
}

#[test]
fn speed_switched_off_plays_everything_as_recorded_and_keeps_the_choices() {
    let prefs = Prefs { speed_enabled: false, ..Prefs::default() };
    let r = rig(prefs.clone(), &[(0, "fast"), (1, "slow")]);
    let id = r.play();
    assert_eq!(r.plays(), vec![(id, SpeedVariant::Normal, false)]);
    assert!(!r.view(|s| s.speeds.enabled));
    assert_eq!(r.version_of(r.tracks[0]), SpeedVariant::Normal);
    assert_eq!(r.view(|s| s.speeds.chosen.len()), 2, "the choices wait");
    assert_eq!(r.speed(), 1.0);
    wait_for("the preload", || r.asked().contains(&Asked::Next(Some((SpeedVariant::Normal, false)))));
    // Switched on: the playing track takes its version from where it is,
    // the next one its own.
    r.rt.send(Command::SetPrefs(Box::new(Prefs { speed_enabled: true, ..prefs.clone() })));
    wait_for("the engine to be told", || r.speeds().len() == 2);
    let speeds = r.speeds();
    assert_eq!(speeds[0], (id, SpeedVariant::Fast, false));
    assert_ne!(speeds[1].0, id);
    assert_eq!((speeds[1].1, speeds[1].2), (SpeedVariant::Slow, false));
    wait_for("the view", || r.version_of(r.tracks[0]) == SpeedVariant::Fast);
    // And off again: back to as recorded, the rows still there.
    r.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for("as recorded", || r.speeds().len() == 4);
    let speeds = r.speeds();
    assert_eq!(speeds[2], (id, SpeedVariant::Normal, false));
    assert_eq!((speeds[3].1, speeds[3].2), (SpeedVariant::Normal, false));
    assert_eq!(r.lib.track_fx().unwrap().len(), 2);
}

#[test]
fn keeping_the_timbre_goes_with_a_version_only() {
    let prefs = Prefs { keep_timbre: true, ..Prefs::default() };
    let r = rig(prefs, &[(1, "fast")]);
    let id = r.play();
    // A track played as recorded has no timbre to keep.
    assert_eq!(r.plays(), vec![(id, SpeedVariant::Normal, false)]);
    wait_for("the preload", || r.asked().contains(&Asked::Next(Some((SpeedVariant::Fast, true)))));
}

#[test]
fn a_live_track_reported_at_its_own_speed_moves_at_that_speed() {
    let r = rig(Prefs::default(), &[(0, "fast")]);
    let id = r.play();
    wait_for("the view", || r.speed() == 1.2);
    r.rt.playback_sender().send(PlaybackEvent::Speed { id, speed: 1.0 }).unwrap();
    wait_for("the view", || r.speed() == 1.0);
    assert_eq!(r.rate(), 1.0);
}

#[test]
fn the_equalizer_reaches_the_engine_at_the_start_and_when_it_changes() {
    let mut gains = [0.0; EQ_BANDS];
    gains[1] = 4.0;
    let prefs = Prefs { eq_on: true, eq_gains: gains, ..Prefs::default() };
    let r = rig(prefs.clone(), &[]);
    let eqs = |r: &Rig| -> Vec<Asked> { r.asked().into_iter().filter(|a| matches!(a, Asked::Eq(..))).collect() };
    assert_eq!(eqs(&r), vec![Asked::Eq(true, gains)]);
    r.rt.send(Command::SetPrefs(Box::new(Prefs { eq_on: false, ..prefs.clone() })));
    wait_for("the switch", || eqs(&r).len() == 2);
    assert_eq!(eqs(&r)[1], Asked::Eq(false, gains));
    // Another setting changing does not send it again. A command after it
    // tells when the runtime is through with both.
    r.rt.send(Command::SetPrefs(Box::new(Prefs { eq_on: false, normalization: false, ..prefs })));
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::Fast });
    wait_for("the runtime to be through", || r.version_of(r.tracks[0]) == SpeedVariant::Fast);
    assert_eq!(eqs(&r).len(), 2);
}

#[test]
fn a_speed_of_the_listeners_own_is_saved_by_its_number_and_heard() {
    let r = rig(Prefs::default(), &[]);
    let id = r.play();
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::Custom(115) });
    wait_for("the view", || r.speed() == 1.15);
    assert_eq!(r.speeds(), vec![(id, SpeedVariant::Custom(115), false)]);
    assert_eq!(r.rate(), 1.15);
    assert_eq!(r.lib.track_fx().unwrap(), vec![(r.tracks[0], "x115".to_owned())]);
    // The speed of a ready version is that version, under its own name.
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::Custom(120) });
    wait_for("the view", || r.speed() == 1.2);
    assert_eq!(r.version_of(r.tracks[0]), SpeedVariant::Fast);
    assert_eq!(r.lib.track_fx().unwrap(), vec![(r.tracks[0], "fast".to_owned())]);
}

#[test]
fn a_speed_of_the_listeners_own_is_read_back_at_the_start() {
    let r = rig(Prefs::default(), &[(0, "x70")]);
    assert_eq!(r.version_of(r.tracks[0]), SpeedVariant::Custom(70));
    let id = r.play();
    assert_eq!(r.plays(), vec![(id, SpeedVariant::Custom(70), false)]);
    wait_for("the view", || r.speed() == 0.7);
}

/// «Сбросить» on «Не нравится и скорость»: the track plays as recorded, its
/// name leaves the list, and the snackbar's «Отменить» brings the speed
/// (and the name) back.
#[test]
fn a_speed_reset_from_the_marks_can_be_undone() {
    let r = rig(Prefs::default(), &[(0, "nightcore")]);
    let track = r.tracks[0];
    wait_for("the name of the chosen track", || r.view(|s| s.speed_names.get(&track).map(|n| n.title.clone())) == Some("one".into()));
    r.rt.send(Command::ResetTrackSpeed(track));
    wait_for("as recorded", || r.version_of(track) == SpeedVariant::Normal);
    assert!(r.view(|s| s.speed_names.is_empty()));
    assert!(r.lib.track_fx().unwrap().is_empty());
    let undo = r.view(|s| s.toasts.iter().rev().find_map(|t| t.action.clone()));
    let Some((label, command)) = undo else { panic!("no «Отменить»") };
    assert_eq!(label, "Отменить");
    assert_eq!(command, Command::SetTrackSpeed { track, variant: SpeedVariant::Nightcore });
    r.rt.send(command);
    wait_for("the speed back", || r.version_of(track) == SpeedVariant::Nightcore);
    assert!(r.view(|s| s.speed_names.contains_key(&track)));
    // A track without a speed of its own: nothing to reset, no snackbar.
    r.rt.send(Command::ResetTrackSpeed(r.tracks[1]));
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[1], variant: SpeedVariant::Fast });
    wait_for("the second track's speed", || r.version_of(r.tracks[1]) == SpeedVariant::Fast);
    assert!(r.view(|s| !s.toasts.iter().any(|t| t.text.contains("two"))));
}
