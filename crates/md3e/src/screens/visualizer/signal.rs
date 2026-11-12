//! What the pictures read from the music, once per frame: the spectrum
//! scaled to the track's own loudness, the beat and its tempo, a slow
//! loudness for calm motion. The analyzer's levels sit high for any loud
//! track, so the beat is how far the bass jumps above its own recent
//! average, not how loud it is.
//!
//! Nothing here allocates after the first frame (the band buffers only
//! grow when the analyzer's band count changes).

/// Shortest time between two beats (about 210 BPM).
const MIN_BEAT_GAP: f64 = 0.28;
/// The tempo a track starts from before its beats say otherwise.
const DEFAULT_PERIOD: f32 = 0.5;

#[derive(Default)]
pub struct Signal {
    last: f64,
    /// The spectrum normalised between a slow floor and a falling peak
    /// (0..1, low frequencies first).
    pub bands: Vec<f32>,
    /// Per-band jumps above their own average (0..1, ~0.12 s decay).
    pub hits: Vec<f32>,
    band_avg: Vec<f32>,
    peak: f32,
    floor: f32,
    bass_avg: f32,
    /// The kick: 1 on a beat, decaying in ~0.18 s.
    pub kick: f32,
    /// A beat started this frame.
    pub beat: bool,
    /// Beats counted since the visualizer opened.
    pub beats: u64,
    beat_at: f64,
    /// The bass fell back near its average since the last beat: the next
    /// rise is a new beat, not the same one still going.
    armed: bool,
    /// Seconds between beats, averaged (0.3..1.2).
    pub period: f32,
    /// Overall loudness, smoothed over ~1 s (0..1).
    pub calm: f32,
    /// Overall loudness, quick (0..1).
    pub level: f32,
    /// Seconds since the previous frame (0..0.1).
    pub dt: f32,
}

impl Signal {
    /// One step at `now` (seconds) from the analyzer's `bands` and `bass`.
    /// `live` false (paused, nothing heard) lets everything fall to rest.
    pub fn update(&mut self, now: f64, bands: &[f32], bass: f32, live: bool) {
        let dt = if self.last == 0.0 { 1.0 / 60.0 } else { (now - self.last).clamp(0.0, 0.1) as f32 };
        self.last = now;
        self.dt = dt;
        if self.period == 0.0 {
            self.period = DEFAULT_PERIOD;
        }
        let ease = |tau: f32| 1.0 - (-dt / tau).exp();
        let n = bands.len();
        if self.bands.len() != n {
            self.bands.resize(n, 0.0);
            self.hits.resize(n, 0.0);
            self.band_avg.clear();
            self.band_avg.extend_from_slice(bands);
        }

        // Auto-gain: a floor following the mean, a peak that jumps up and
        // falls back over a few seconds.
        let (max, sum) = bands.iter().fold((0.0f32, 0.0f32), |(m, s), &b| (m.max(b), s + b));
        let mean = if n == 0 { 0.0 } else { sum / n as f32 };
        let (max, mean) = if live { (max, mean) } else { (0.0, 0.0) };
        self.peak = if max > self.peak { max } else { self.peak + (max - self.peak) * ease(2.5) };
        self.floor += (mean - self.floor) * ease(1.5);
        let lo = self.floor * 0.85;
        let span = (self.peak - lo).max(0.15);
        let quiet = max < 0.05;
        for i in 0..n {
            let b = if live { bands[i] } else { 0.0 };
            let target = if quiet { 0.0 } else { ((b - lo) / span).clamp(0.0, 1.0) };
            // The analyzer smooths already; a short follow only hides
            // frame-to-frame steps of the auto-gain.
            self.bands[i] += (target - self.bands[i]) * ease(0.03);
            self.band_avg[i] += (b - self.band_avg[i]) * ease(0.45);
            let jump = ((b - self.band_avg[i]) * 4.0).clamp(0.0, 1.0);
            self.hits[i] = jump.max(self.hits[i] * (-dt / 0.12).exp());
        }
        let level = if quiet { 0.0 } else { ((mean - lo) / span * 1.6).clamp(0.0, 1.0) };
        self.level += (level - self.level) * ease(0.08);
        self.calm += (level - self.calm) * ease(1.0);

        // The beat: the bass well above its average, not too soon after
        // the previous one.
        let bass = if live { bass } else { 0.0 };
        self.bass_avg += (bass - self.bass_avg) * ease(0.6);
        let rise = bass - self.bass_avg;
        self.beat = false;
        if rise < 0.03 {
            self.armed = true;
        }
        if live && self.armed && rise > 0.06 && bass > 0.15 && now - self.beat_at > MIN_BEAT_GAP {
            let gap = (now - self.beat_at) as f32;
            if (0.3..=1.2).contains(&gap) {
                self.period += (gap - self.period) * 0.25;
            } else if (1.2..=2.4).contains(&gap) {
                // A missed beat in between.
                self.period += (gap * 0.5 - self.period) * 0.1;
            }
            self.beat_at = now;
            self.armed = false;
            self.beat = true;
            self.beats += 1;
        }
        let hit = ((rise - 0.03) * 5.0).clamp(0.0, 1.0);
        self.kick = hit.max(self.kick * (-dt / 0.18).exp());
        if !live {
            self.kick *= (-dt / 0.1).exp();
        }
    }

    /// The level of direction `theta` (radians, clockwise from the top):
    /// low frequencies at the top, high at the bottom, mirrored left and
    /// right, interpolated between bands.
    pub fn around(values: &[f32], theta: f32) -> f32 {
        if values.is_empty() {
            return 0.0;
        }
        let fold = 1.0 - (2.0 * (theta / std::f32::consts::TAU).rem_euclid(1.0) - 1.0).abs();
        Self::at(values, fold)
    }

    /// The level at `x` (0 the lowest band .. 1 the highest), interpolated.
    pub fn at(values: &[f32], x: f32) -> f32 {
        if values.is_empty() {
            return 0.0;
        }
        let x = x.clamp(0.0, 1.0) * (values.len() - 1) as f32;
        let (i0, f) = (x.floor() as usize, x.fract());
        let i1 = (i0 + 1).min(values.len() - 1);
        values[i0] * (1.0 - f) + values[i1] * f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `seconds` at 60 fps of a kick every `period` seconds over a
    /// steady mid-range; returns the beats seen.
    fn run(sig: &mut Signal, period: f64, seconds: f64, live: bool) -> u64 {
        let mut bands = vec![0.4f32; 64];
        let start = sig.beats;
        let mut t = sig.last.max(1.0);
        let end = t + seconds;
        while t < end {
            t += 1.0 / 60.0;
            let phase = (t % period) / period;
            let bass = if period > 0.0 { 0.3 + 0.6 * (-(phase as f32) * 12.0).exp() } else { 0.5 };
            for (i, b) in bands.iter_mut().enumerate() {
                *b = if i < 8 { bass } else { 0.4 };
            }
            sig.update(t, &bands, bass, live);
        }
        sig.beats - start
    }

    #[test]
    fn a_steady_kick_is_counted_once_per_beat_and_gives_the_tempo() {
        let mut sig = Signal::default();
        let beats = run(&mut sig, 0.5, 10.0, true);
        assert!((18..=21).contains(&beats), "120 BPM over 10 s: {beats}");
        assert!((sig.period - 0.5).abs() < 0.05, "period {}", sig.period);
        let mut sig = Signal::default();
        run(&mut sig, 0.75, 10.0, true);
        assert!((sig.period - 0.75).abs() < 0.06, "period {}", sig.period);
    }

    #[test]
    fn a_steady_sound_has_no_beat() {
        let mut sig = Signal::default();
        let mut t = 1.0;
        let bands = vec![0.6f32; 64];
        let mut beats = 0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            sig.update(t, &bands, 0.6, true);
            beats += u64::from(sig.beat);
        }
        assert_eq!(beats, 0);
        assert!(sig.kick < 0.05);
    }

    #[test]
    fn a_pause_lets_everything_rest() {
        let mut sig = Signal::default();
        run(&mut sig, 0.5, 4.0, true);
        assert!(sig.calm > 0.1);
        assert_eq!(run(&mut sig, 0.5, 4.0, false), 0);
        assert!(sig.kick < 0.01 && sig.level < 0.01 && sig.bands.iter().all(|b| *b < 0.01));
    }

    #[test]
    fn the_bands_are_scaled_to_the_track() {
        // A quiet track still spans the range: its loudest band near the top.
        let mut sig = Signal::default();
        let mut bands = vec![0.2f32; 32];
        bands[5] = 0.45;
        let mut t = 1.0;
        for _ in 0..300 {
            t += 1.0 / 60.0;
            sig.update(t, &bands, 0.2, true);
        }
        assert!(sig.bands[5] > 0.8, "{}", sig.bands[5]);
        assert!(sig.bands[20] < 0.3, "{}", sig.bands[20]);
    }

    #[test]
    fn around_puts_the_lows_on_top_and_mirrors() {
        let v = [1.0, 0.5, 0.0];
        assert!((Signal::around(&v, 0.0) - 1.0).abs() < 1e-5);
        assert!(Signal::around(&v, std::f32::consts::PI).abs() < 1e-5);
        let l = Signal::around(&v, 1.0);
        let r = Signal::around(&v, std::f32::consts::TAU - 1.0);
        assert!((l - r).abs() < 1e-5);
    }
}
