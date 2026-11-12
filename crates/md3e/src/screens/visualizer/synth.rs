//! A made-up song's levels for screenshot runs (`NIKGOB_VIZ_SYNTH=1`):
//! 120 BPM with a kick on every beat, a snare on two and four, hi-hats on
//! the eighths and a melody walking over the mids, in the analyzer's
//! scale (0..1, 64 bands, low first). The demo's tones are one pulsing
//! chord, which shows little of what a picture does with real music.

use std::f32::consts::TAU;

pub const BANDS: usize = 64;

/// Fills `bands` with the levels at `t` seconds; returns the bass (0..1).
pub fn levels(t: f64, bands: &mut Vec<f32>) -> f32 {
    bands.resize(BANDS, 0.0);
    let beat = 0.5f64;
    let n = (t / beat).floor() as i64;
    let phase = ((t % beat) / beat) as f32;
    let since = phase * beat as f32;
    let kick = (-since * 14.0).exp();
    let snare = if n % 2 == 1 { (-since * 10.0).exp() } else { 0.0 };
    let eighth = ((t % (beat / 2.0)) as f32 * 30.0).min(8.0);
    let hat = (-eighth).exp();
    // A melody note per beat from a fixed walk, two partials apart.
    let walk = [18.0, 22.0, 25.0, 22.0, 30.0, 27.0, 25.0, 20.0];
    let note = walk[n.rem_euclid(8) as usize];
    let ft = t as f32;
    for (i, b) in bands.iter_mut().enumerate() {
        let x = i as f32;
        // A soft slope: music is louder low than high.
        let mut v = 0.62 - 0.22 * x / BANDS as f32 + 0.03 * (ft * 0.7 + x * 0.4).sin();
        if i < 10 {
            v += 0.32 * kick * (1.0 - x / 12.0);
        }
        if (20..42).contains(&i) {
            v += 0.18 * snare * (0.6 + 0.4 * (x * 1.7 + ft * 9.0).sin().abs());
        }
        if i >= 44 {
            v += 0.2 * hat * (0.5 + 0.5 * (x * 2.3).sin().abs());
        }
        for (k, amp) in [(note, 0.22f32), (note + 12.0, 0.12)] {
            let d = (x - k) / 1.6;
            v += amp * (-d * d).exp() * (0.8 + 0.2 * (TAU * 5.0 * ft).sin());
        }
        *b = v.clamp(0.0, 1.0);
    }
    (0.35 + 0.6 * kick).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_made_up_song_beats_in_the_signal() {
        let mut sig = super::super::signal::Signal::default();
        let mut bands = Vec::new();
        let mut t = 1.0;
        for _ in 0..600 {
            t += 1.0 / 60.0;
            let bass = levels(t, &mut bands);
            assert!(bands.iter().all(|b| (0.0..=1.0).contains(b)));
            sig.update(t, &bands, bass, true);
        }
        assert!((17..=21).contains(&sig.beats), "{}", sig.beats);
        assert!((sig.period - 0.5).abs() < 0.05);
    }
}
