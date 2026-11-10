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
        wait_for("playing", || self.rt.state().player.status == Status::Playing);
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
    Rig { rt, asked, lib, tracks }
}

#[test]
fn a_track_plays_as_the_version_remembered_with_it() {
    let r = rig(Prefs::default(), &[(0, "nightcore")]);
    assert_eq!(r.rt.state().speeds.of(r.tracks[0]), SpeedVariant::Nightcore);
    let id = r.play();
    assert_eq!(r.plays(), vec![(id, SpeedVariant::Nightcore, false)]);
    let s = r.rt.state();
    assert_eq!(s.player.speed, 1.3);
    assert_eq!(s.player.rate, 1.3, "the position moves at the speed of the track");
    // The next track has no choice: preloaded as recorded.
    assert!(r.asked().contains(&Asked::Next(Some((SpeedVariant::Normal, false)))));
}

#[test]
fn a_new_choice_is_saved_and_heard_at_once_without_moving_the_position() {
    let r = rig(Prefs::default(), &[]);
    let id = r.play();
    r.rt.send(Command::Seek(Duration::from_secs(60)));
    wait_for("the seek", || r.rt.state().player.position_now() >= Duration::from_secs(59));
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::SlowEcho });
    wait_for("the engine to be told", || !r.speeds().is_empty());
    assert_eq!(r.speeds(), vec![(id, SpeedVariant::SlowEcho, false)]);
    wait_for("the view", || r.rt.state().player.speed == 0.85);
    let s = r.rt.state();
    assert_eq!(s.speeds.of(r.tracks[0]), SpeedVariant::SlowEcho);
    assert_eq!(s.player.rate, 0.85);
    let shown = s.player.position_now();
    assert!(shown >= Duration::from_secs(59) && shown < Duration::from_secs(63), "{shown:?}");
    assert_eq!(r.lib.track_fx().unwrap(), vec![(r.tracks[0], "slow_echo".to_owned())]);
    // The same choice again changes nothing for the engine.
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::SlowEcho });
    // Back to as recorded: the row is gone.
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[0], variant: SpeedVariant::Normal });
    wait_for("as recorded again", || r.rt.state().player.speed == 1.0);
    assert_eq!(r.speeds().len(), 2);
    assert!(r.lib.track_fx().unwrap().is_empty());
}

#[test]
fn a_choice_for_the_next_track_prepares_it_anew_and_leaves_the_playing_one_alone() {
    let r = rig(Prefs::default(), &[]);
    r.play();
    let nexts = |r: &Rig| r.asked().into_iter().filter(|a| matches!(a, Asked::Next(_))).count();
    wait_for("the preload", || nexts(&r) >= 1);
    let before = nexts(&r);
    r.rt.send(Command::SetTrackSpeed { track: r.tracks[1], variant: SpeedVariant::Fast });
    wait_for("the next track prepared again", || nexts(&r) > before);
    assert_eq!(r.asked().last(), Some(&Asked::Next(Some((SpeedVariant::Fast, false)))));
    assert!(r.speeds().is_empty(), "the playing track was not touched");
    assert_eq!(r.rt.state().player.speed, 1.0);
}

#[test]
fn speed_switched_off_plays_everything_as_recorded_and_keeps_the_choices() {
    let prefs = Prefs { speed_enabled: false, ..Prefs::default() };
    let r = rig(prefs.clone(), &[(0, "fast"), (1, "slow")]);
    let id = r.play();
    assert_eq!(r.plays(), vec![(id, SpeedVariant::Normal, false)]);
    let s = r.rt.state();
    assert!(!s.speeds.enabled);
    assert_eq!(s.speeds.of(r.tracks[0]), SpeedVariant::Normal);
    assert_eq!(s.speeds.chosen.len(), 2, "the choices wait");
    assert_eq!(s.player.speed, 1.0);
    // Switched on: the playing track takes its version from where it is,
    // the next one is prepared with its own.
    r.rt.send(Command::SetPrefs(Box::new(Prefs { speed_enabled: true, ..prefs.clone() })));
    wait_for("the engine to be told", || !r.speeds().is_empty());
    assert_eq!(r.speeds(), vec![(id, SpeedVariant::Fast, false)]);
    wait_for("the next track prepared again", || {
        r.asked().last() == Some(&Asked::Next(Some((SpeedVariant::Slow, false))))
    });
    wait_for("the view", || r.rt.state().speeds.of(r.tracks[0]) == SpeedVariant::Fast);
    // And off again: back to as recorded, the rows still there.
    r.rt.send(Command::SetPrefs(Box::new(prefs)));
    wait_for("as recorded", || r.speeds().len() == 2);
    assert_eq!(r.speeds()[1], (id, SpeedVariant::Normal, false));
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
    assert_eq!(r.rt.state().player.speed, 1.2);
    r.rt.playback_sender().send(PlaybackEvent::Speed { id, speed: 1.0 }).unwrap();
    wait_for("the view", || r.rt.state().player.speed == 1.0);
    assert_eq!(r.rt.state().player.rate, 1.0);
}

#[test]
fn the_equalizer_reaches_the_engine_at_the_start_and_when_it_changes() {
    let mut gains = [0.0; EQ_BANDS];
    gains[1] = 4.0;
    let prefs = Prefs { eq_on: true, eq_gains: gains, ..Prefs::default() };
    let r = rig(prefs.clone(), &[]);
    assert_eq!(r.asked().first(), Some(&Asked::Eq(true, gains)));
    r.rt.send(Command::SetPrefs(Box::new(Prefs { eq_on: false, ..prefs.clone() })));
    wait_for("the switch", || r.asked().contains(&Asked::Eq(false, gains)));
    // Another setting changing does not send it again.
    let count = |r: &Rig| r.asked().into_iter().filter(|a| matches!(a, Asked::Eq(..))).count();
    r.rt.send(Command::SetPrefs(Box::new(Prefs { eq_on: false, normalization: false, ..prefs })));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(count(&r), 2);
}
