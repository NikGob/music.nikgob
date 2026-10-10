//! Streaming rational polyphase resampler (Kaiser-windowed sinc).
//!
//! Opus encodes at 8/12/16/24/48 kHz, so 44.1 kHz (and everything else) is
//! converted to 48 kHz first. The ratio is reduced to `L/M` by the gcd; the
//! filter has ~32 zero crossings per side of the narrower Nyquist and a Kaiser
//! window with beta 9, which keeps passband ripple and alias rejection far
//! below what a 160 kbps Opus stream can show.
//!
//! The output is delay-compensated: the first output frame lines up with the
//! first input frame, and [`Resampler::finish`] flushes the tail so that the
//! total is `floor(frames_in * L / M)`.

const ZEROS_PER_SIDE: usize = 16;
const KAISER_BETA: f64 = 9.0;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / f64::from(k * k);
        sum += term;
        if term < 1e-14 * sum {
            break;
        }
    }
    sum
}

pub struct Resampler {
    channels: usize,
    up: usize,
    down: usize,
    taps_per_phase: usize,
    /// `table[phase * taps_per_phase + k]`
    table: Vec<f32>,
    /// Group delay of the filter in upsampled samples.
    center: usize,
    /// Interleaved input not yet released.
    buf: Vec<f32>,
    /// Absolute index of the first frame in `buf`.
    base: u64,
    /// Total input frames received.
    received: u64,
    /// Index of the next output frame.
    next_out: u64,
    /// Output frames emitted so far.
    emitted: u64,
    bypass: bool,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let g = gcd(u64::from(in_rate), u64::from(out_rate)).max(1);
        let up = (u64::from(out_rate) / g) as usize;
        let down = (u64::from(in_rate) / g) as usize;
        let ratio = (down as f64 / up as f64).max(1.0);
        let taps_per_phase = (2.0 * ZEROS_PER_SIDE as f64 * ratio).ceil() as usize;
        let len = taps_per_phase * up;
        let center = (len - 1) / 2;
        // Cutoff in cycles per upsampled sample, a little below the narrower Nyquist.
        let cutoff = 0.5 / up.max(down) as f64 * 0.97;
        let i0_beta = bessel_i0(KAISER_BETA);
        let mut h = vec![0f32; len];
        for (i, tap) in h.iter_mut().enumerate() {
            let x = i as f64 - (len as f64 - 1.0) / 2.0;
            let r = 2.0 * x / (len as f64 - 1.0);
            let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
            let arg = 2.0 * std::f64::consts::PI * cutoff * x;
            let sinc = if x == 0.0 { 1.0 } else { arg.sin() / arg };
            *tap = (2.0 * cutoff * sinc * window * up as f64) as f32;
        }
        // Polyphase layout: phase p uses h[p + k*up].
        let mut table = vec![0f32; len];
        for p in 0..up {
            for k in 0..taps_per_phase {
                table[p * taps_per_phase + k] = h[p + k * up];
            }
        }
        Self {
            channels,
            up,
            down,
            taps_per_phase,
            table,
            center,
            buf: Vec::new(),
            base: 0,
            received: 0,
            next_out: 0,
            emitted: 0,
            bypass: in_rate == out_rate,
        }
    }

    /// Feeds interleaved frames, appends resampled interleaved frames to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.bypass {
            out.extend_from_slice(input);
            self.emitted += (input.len() / self.channels) as u64;
            return;
        }
        self.buf.extend_from_slice(input);
        self.received += (input.len() / self.channels) as u64;
        self.produce(out, self.received);
        self.trim();
    }

    /// Flushes the filter tail. Call once after the last `process`.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.bypass {
            return;
        }
        let expected = self.received * self.up as u64 / self.down as u64;
        // Zero padding lets the filter see "silence" after the end.
        let pad = vec![0f32; (self.taps_per_phase + 1) * self.channels];
        self.buf.extend_from_slice(&pad);
        let available = self.received + self.taps_per_phase as u64 + 1;
        let before = out.len();
        self.produce(out, available);
        // Drop what lies beyond the true length.
        let produced_total = self.emitted;
        if produced_total > expected {
            let extra = (produced_total - expected) as usize * self.channels;
            out.truncate(out.len().saturating_sub(extra).max(before));
            self.emitted = expected;
        }
    }

    fn produce(&mut self, out: &mut Vec<f32>, available: u64) {
        let ch = self.channels;
        let tpp = self.taps_per_phase;
        loop {
            let t = self.next_out * self.down as u64 + self.center as u64;
            let j_max = t / self.up as u64;
            if j_max >= available {
                break;
            }
            let phase = (t % self.up as u64) as usize;
            let taps = &self.table[phase * tpp..(phase + 1) * tpp];
            for c in 0..ch {
                let mut acc = 0f32;
                for (k, &w) in taps.iter().enumerate() {
                    let Some(j) = j_max.checked_sub(k as u64) else {
                        break;
                    };
                    if j < self.base {
                        break;
                    }
                    let idx = (j - self.base) as usize * ch + c;
                    // Frames beyond what has been received count as silence.
                    if let Some(&x) = self.buf.get(idx) {
                        acc += x * w;
                    }
                }
                out.push(acc);
            }
            self.next_out += 1;
            self.emitted += 1;
        }
    }

    /// Releases input frames the filter can no longer reach.
    fn trim(&mut self) {
        let t = self.next_out * self.down as u64 + self.center as u64;
        let j_max = t / self.up as u64;
        let keep_from = j_max.saturating_sub(self.taps_per_phase as u64);
        if keep_from > self.base {
            let drop = (keep_from - self.base) as usize;
            self.buf.drain(..drop * self.channels);
            self.base = keep_from;
        }
    }

    /// Output frames produced so far.
    pub fn frames_out(&self) -> u64 {
        self.emitted
    }
}

/// Resamples a whole interleaved buffer (tests, small inputs).
pub fn resample_all(input: &[f32], channels: usize, in_rate: u32, out_rate: u32) -> Vec<f32> {
    let mut r = Resampler::new(in_rate, out_rate, channels);
    let mut out = Vec::new();
    r.process(input, &mut out);
    r.finish(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, secs: f64, amp: f32) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize)
            .map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    /// Frequency estimate from zero crossings and RMS of the middle part.
    fn measure(x: &[f32], rate: u32) -> (f64, f64) {
        let mid = &x[x.len() / 4..x.len() * 3 / 4];
        let rms = (mid.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / mid.len() as f64).sqrt();
        let crossings = mid.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        (crossings as f64 * f64::from(rate) / mid.len() as f64, rms)
    }

    #[test]
    fn upsampling_44100_to_48000_keeps_pitch_level_and_length() {
        let input = sine(1000.0, 44_100, 2.0, 0.5);
        let out = resample_all(&input, 1, 44_100, 48_000);
        assert_eq!(out.len(), 96_000);
        let (f, rms) = measure(&out, 48_000);
        assert!((f - 1000.0).abs() < 5.0, "freq {f}");
        assert!((rms - 0.5 / 2f64.sqrt()).abs() < 0.005, "rms {rms}");
    }

    #[test]
    fn downsampling_96000_to_48000_and_alias_rejection() {
        // 30 kHz is above the 24 kHz Nyquist of the output and must vanish.
        let mut input = sine(1000.0, 96_000, 1.0, 0.4);
        for (a, b) in input.iter_mut().zip(sine(30_000.0, 96_000, 1.0, 0.4)) {
            *a += b;
        }
        let out = resample_all(&input, 1, 96_000, 48_000);
        assert_eq!(out.len(), 48_000);
        let (f, rms) = measure(&out, 48_000);
        assert!((f - 1000.0).abs() < 5.0, "freq {f}");
        assert!((rms - 0.4 / 2f64.sqrt()).abs() < 0.01, "alias leaked: rms {rms}");
    }

    #[test]
    fn chunked_processing_equals_one_shot() {
        let input = sine(440.0, 44_100, 1.0, 0.3);
        let whole = resample_all(&input, 1, 44_100, 48_000);
        let mut r = Resampler::new(44_100, 48_000, 1);
        let mut out = Vec::new();
        for chunk in input.chunks(777) {
            r.process(chunk, &mut out);
        }
        r.finish(&mut out);
        assert_eq!(out.len(), whole.len());
        let max_diff = out
            .iter()
            .zip(&whole)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(max_diff < 1e-5, "{max_diff}");
    }

    #[test]
    fn stereo_channels_stay_separate_and_equal_rate_is_a_passthrough() {
        let l = sine(500.0, 44_100, 0.5, 0.5);
        let mut inter = Vec::new();
        for s in &l {
            inter.push(*s);
            inter.push(0.0);
        }
        let out = resample_all(&inter, 2, 44_100, 48_000);
        let right_max = out.iter().skip(1).step_by(2).fold(0f32, |m, v| m.max(v.abs()));
        let left_max = out.iter().step_by(2).fold(0f32, |m, v| m.max(v.abs()));
        assert!(right_max < 1e-6);
        assert!(left_max > 0.4);
        assert_eq!(resample_all(&inter, 2, 48_000, 48_000), inter);
    }

    #[test]
    fn start_is_aligned() {
        // An impulse at frame 100 must come out near frame 100 * 48/44.1.
        let mut input = vec![0f32; 2000];
        input[100] = 1.0;
        let out = resample_all(&input, 1, 44_100, 48_000);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        let expected = 100.0 * 48_000.0 / 44_100.0;
        assert!((peak as f64 - expected).abs() <= 1.0, "{peak} vs {expected}");
    }
}
