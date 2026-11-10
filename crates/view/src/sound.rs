//! The sound a listener chooses: the speed of a track (a version of the
//! song, remembered with it) and the equalizer (one for the player).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::model::TrackId;

/// How a track is played: as recorded, or as one of the versions people
/// make of songs by playing them faster or slower. The pitch moves with
/// the speed, as on a turntable; each version has the tone (and, for the
/// last, the reverb) that goes with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpeedVariant {
    #[default]
    Normal,
    Fast,
    Nightcore,
    Slow,
    SlowEcho,
}

/// A reverb of a [`SpeedVariant`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Echo {
    /// Level of the reverberation against the music.
    pub wet: f32,
    /// Seconds the tail takes to fall by 60 dB.
    pub decay_s: f32,
    /// 0 bright to 1 dark.
    pub damping: f32,
    pub pre_delay_ms: f32,
}

/// What a [`SpeedVariant`] does to the sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedSound {
    pub speed: f32,
    /// Shelf below 100 Hz, dB.
    pub low_shelf_db: f32,
    /// Shelf above 7 kHz, dB.
    pub high_shelf_db: f32,
    /// Level before the tone and the reverb, dB: the headroom they take.
    pub preamp_db: f32,
    pub echo: Option<Echo>,
}

impl SpeedVariant {
    pub const ALL: [Self; 5] = [Self::Normal, Self::Fast, Self::Nightcore, Self::Slow, Self::SlowEcho];

    /// The name in the «Скорость» menu.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "Обычно",
            Self::Fast => "Быстрее",
            Self::Nightcore => "Nightcore",
            Self::Slow => "Замедленно",
            Self::SlowEcho => "Замедленно с эхом",
        }
    }

    /// The quiet mark under the title while the track is not played as
    /// recorded.
    pub const fn chip(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Fast => Some("×1.2"),
            Self::Nightcore => Some("×1.3"),
            Self::Slow => Some("замедленно"),
            Self::SlowEcho => Some("замедленно · эхо"),
        }
    }

    /// The name it is stored under.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Fast => "fast",
            Self::Nightcore => "nightcore",
            Self::Slow => "slow",
            Self::SlowEcho => "slow_echo",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == key)
    }

    pub fn speed(self) -> f32 {
        self.sound().speed
    }

    /// The numbers of each version.
    ///
    /// * Faster versions get a little more top and a little less bottom (a
    ///   kick moved up a third sounds boxy otherwise); slowed ones more
    ///   bass and a softer top. Shelves, never a hard cut: a low-pass
    ///   swallows the words.
    /// * The preamp is the headroom of the boost: all of a bass shelf
    ///   (the bass is where a master peaks), half of a treble shelf, so the
    ///   limiter stays out of it.
    /// * The echo: the reverberation is 8 dB under the music, which leaves
    ///   the words in front; a tail of 2.8 s, darker than the song (highs
    ///   gone in about a second), starting 25 ms after the sound so the
    ///   voice keeps its outline.
    pub fn sound(self) -> SpeedSound {
        let plain = |speed, low_shelf_db, high_shelf_db, preamp_db| SpeedSound {
            speed,
            low_shelf_db,
            high_shelf_db,
            preamp_db,
            echo: None,
        };
        match self {
            Self::Normal => plain(1.0, 0.0, 0.0, 0.0),
            Self::Fast => plain(1.2, -1.0, 2.0, -1.0),
            Self::Nightcore => plain(1.3, -2.0, 3.0, -1.5),
            Self::Slow => plain(0.85, 3.0, -2.0, -3.0),
            Self::SlowEcho => SpeedSound {
                speed: 0.85,
                low_shelf_db: 4.0,
                high_shelf_db: -3.0,
                preamp_db: -4.0,
                echo: Some(Echo {
                    wet: 0.4,
                    decay_s: 2.8,
                    damping: 0.45,
                    pre_delay_ms: 25.0,
                }),
            },
        }
    }
}

/// The tracks that are not played as recorded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackSpeeds {
    /// «Скорость» is on in the settings: off, every track plays as
    /// recorded and the choices wait.
    pub enabled: bool,
    pub chosen: Arc<HashMap<TrackId, SpeedVariant>>,
}

impl TrackSpeeds {
    /// How `track` plays now.
    pub fn of(&self, track: TrackId) -> SpeedVariant {
        if !self.enabled {
            return SpeedVariant::Normal;
        }
        self.chosen.get(&track).copied().unwrap_or_default()
    }
}

/// A time of the track (a position, its length, a moment of its lyrics) as
/// the listener lives it at `speed`: the clock on screen shows seconds of
/// sound, the player counts in the track's own time.
pub fn heard_time(track_time: Duration, speed: f32) -> Duration {
    if speed.is_finite() && speed > 0.0 && speed != 1.0 {
        // In f64 and to whole microseconds: 1.2 as an f32 is not 1.2, and a
        // clock showing 3:19 for 3:20 of sound reads as a bug.
        let speed = (f64::from(speed) * 1e6).round() / 1e6;
        let us = (track_time.as_secs_f64() * 1e6 / speed).round();
        Duration::from_micros(us as u64)
    } else {
        track_time
    }
}

/// Number of equalizer bands.
pub const EQ_BANDS: usize = 10;
/// Centre of each band, Hz.
pub const EQ_FREQS: [f32; EQ_BANDS] = [32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0];
/// The farthest a band goes either way, dB.
pub const EQ_MAX_DB: f32 = 12.0;

/// The equalizer's ready curves, by name. The first is flat.
pub const EQ_PRESETS: [(&str, [f32; EQ_BANDS]); 5] = [
    ("Ровно", [0.0; EQ_BANDS]),
    ("Бас", [0.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Вокал", [0.0, 0.0, 0.0, -1.0, 0.0, 2.0, 2.0, 0.0, 0.0, 0.0]),
    ("Наушники", [0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0]),
    ("Тёплый", [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -2.0, -3.0, 0.0]),
];

/// The preset these gains are, if any.
pub fn eq_preset_of(gains: &[f32; EQ_BANDS]) -> Option<usize> {
    EQ_PRESETS
        .iter()
        .position(|(_, g)| g.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.05))
}

/// The equalizer's gain at `freq` (Hz) for these band gains, dB: the curve
/// drawn through the points. Each band is a bell an octave wide (the same
/// filter the engine runs), so neighbours add up.
pub fn eq_curve_db(gains: &[f32; EQ_BANDS], freq: f32) -> f32 {
    const RATE: f64 = 48_000.0;
    const Q: f64 = 1.41;
    let w = std::f64::consts::TAU * f64::from(freq) / RATE;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let mut db = 0.0f64;
    for (gain, centre) in gains.iter().zip(EQ_FREQS) {
        if *gain == 0.0 || !gain.is_finite() {
            continue;
        }
        let a = 10f64.powf(f64::from(gain.clamp(-EQ_MAX_DB, EQ_MAX_DB)) / 40.0);
        let w0 = std::f64::consts::TAU * f64::from(centre) / RATE;
        let (cos, alpha) = (w0.cos(), w0.sin() / (2.0 * Q));
        let power = |b0: f64, b2: f64| {
            let re = b0 - 2.0 * cos * c1 + b2 * c2;
            let im = 2.0 * cos * s1 - b2 * s2;
            re * re + im * im
        };
        let num = power(1.0 + alpha * a, 1.0 - alpha * a);
        let den = power(1.0 + alpha / a, 1.0 - alpha / a);
        db += 10.0 * (num / den).log10();
    }
    db as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_keep_their_keys() {
        for v in SpeedVariant::ALL {
            assert_eq!(SpeedVariant::from_key(v.key()), Some(v));
        }
        assert_eq!(SpeedVariant::from_key("twice"), None);
        assert_eq!(SpeedVariant::Normal.chip(), None);
        assert!(SpeedVariant::ALL[1..].iter().all(|v| v.chip().is_some()));
    }

    #[test]
    fn a_boost_comes_with_its_headroom() {
        for v in SpeedVariant::ALL {
            let s = v.sound();
            assert!(s.preamp_db <= 0.0);
            assert!(s.preamp_db <= -s.low_shelf_db.max(0.0), "{v:?}: the bass shelf's headroom");
            assert!((0.5..=2.0).contains(&s.speed));
        }
        assert_eq!(SpeedVariant::Normal.sound().speed, 1.0);
    }

    #[test]
    fn speed_off_plays_everything_as_recorded_and_keeps_the_choices() {
        let track = TrackId::new();
        let mut speeds = TrackSpeeds {
            enabled: true,
            chosen: Arc::new(HashMap::from([(track, SpeedVariant::Nightcore)])),
        };
        assert_eq!(speeds.of(track), SpeedVariant::Nightcore);
        assert_eq!(speeds.of(TrackId::new()), SpeedVariant::Normal);
        speeds.enabled = false;
        assert_eq!(speeds.of(track), SpeedVariant::Normal);
        assert_eq!(speeds.chosen.len(), 1);
    }

    #[test]
    fn the_clock_on_screen_counts_seconds_of_sound() {
        let four = Duration::from_secs(240);
        assert_eq!(heard_time(four, 1.0), four);
        assert_eq!(heard_time(four, 1.2), Duration::from_secs(200));
        assert!((heard_time(four, 0.85).as_secs_f32() - 282.35).abs() < 0.01);
        assert_eq!(heard_time(four, 0.0), four);
    }

    #[test]
    fn the_curve_goes_through_a_lone_point_and_presets_are_found() {
        let mut gains = [0.0; EQ_BANDS];
        assert_eq!(eq_curve_db(&gains, 1000.0), 0.0);
        gains[5] = 6.0;
        assert!((eq_curve_db(&gains, 1000.0) - 6.0).abs() < 0.01);
        assert!(eq_curve_db(&gains, 60.0).abs() < 0.1);
        assert_eq!(eq_preset_of(&[0.0; EQ_BANDS]), Some(0));
        assert_eq!(eq_preset_of(&EQ_PRESETS[2].1), Some(2));
        assert_eq!(eq_preset_of(&gains), None);
    }
}
