//! Interleaved channel-layout conversion to the device's channel count.
//!
//! Layouts are assumed to follow the usual file order (L R C LFE Ls Rs Lb Rb);
//! this is a player, not a mastering tool, so the matrices are the common
//! ITU-style downmix and "put the left/right on the front pair" upmix.

const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Converts `src` (interleaved, `from` channels) and appends the result to
/// `dst` (interleaved, `to` channels).
pub(crate) fn convert(src: &[f32], from: usize, to: usize, dst: &mut Vec<f32>) {
    debug_assert!(from > 0 && to > 0 && src.len().is_multiple_of(from));
    if from == to {
        dst.extend_from_slice(src);
        return;
    }
    let frames = src.len() / from;
    dst.reserve(frames * to);
    match (from, to) {
        (1, _) => {
            for &s in src {
                if to > 2 {
                    // Mono to a surround device: front pair only.
                    dst.extend_from_slice(&[s, s]);
                    dst.extend(std::iter::repeat_n(0.0, to - 2));
                } else {
                    dst.extend(std::iter::repeat_n(s, to));
                }
            }
        }
        (_, 1) => {
            for frame in src.chunks_exact(from) {
                let (l, r) = mix_to_stereo(frame, from);
                dst.push((l + r) * 0.5);
            }
        }
        (_, 2) => {
            for frame in src.chunks_exact(from) {
                let (l, r) = mix_to_stereo(frame, from);
                dst.push(l);
                dst.push(r);
            }
        }
        _ => {
            // Anything else: copy what fits, silence for the rest.
            for frame in src.chunks_exact(from) {
                for i in 0..to {
                    dst.push(frame.get(i).copied().unwrap_or(0.0));
                }
            }
        }
    }
}

fn mix_to_stereo(f: &[f32], n: usize) -> (f32, f32) {
    match n {
        2 => (f[0], f[1]),
        // L R C
        3 => {
            let k = 1.0 / (1.0 + FRAC_1_SQRT_2);
            (
                (f[0] + FRAC_1_SQRT_2 * f[2]) * k,
                (f[1] + FRAC_1_SQRT_2 * f[2]) * k,
            )
        }
        // Quad: L R Ls Rs
        4 => {
            let k = 1.0 / (1.0 + FRAC_1_SQRT_2);
            (
                (f[0] + FRAC_1_SQRT_2 * f[2]) * k,
                (f[1] + FRAC_1_SQRT_2 * f[3]) * k,
            )
        }
        // 5.0 / 5.1: L R C [LFE] Ls Rs
        5 | 6 => {
            let (ls, rs) = if n == 5 { (f[3], f[4]) } else { (f[4], f[5]) };
            let k = 1.0 / (1.0 + 2.0 * FRAC_1_SQRT_2);
            (
                (f[0] + FRAC_1_SQRT_2 * (f[2] + ls)) * k,
                (f[1] + FRAC_1_SQRT_2 * (f[2] + rs)) * k,
            )
        }
        // 7.1: L R C LFE Ls Rs Lb Rb
        8 => {
            let k = 1.0 / (1.0 + 3.0 * FRAC_1_SQRT_2);
            (
                (f[0] + FRAC_1_SQRT_2 * (f[2] + f[4] + f[6])) * k,
                (f[1] + FRAC_1_SQRT_2 * (f[2] + f[5] + f[7])) * k,
            )
        }
        _ => {
            let (mut l, mut r, mut nl, mut nr) = (0.0, 0.0, 0.0f32, 0.0f32);
            for (i, s) in f.iter().enumerate() {
                if i % 2 == 0 {
                    l += s;
                    nl += 1.0;
                } else {
                    r += s;
                    nr += 1.0;
                }
            }
            (l / nl.max(1.0), r / nr.max(1.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_to_stereo_duplicates() {
        let mut out = Vec::new();
        convert(&[0.1, 0.2], 1, 2, &mut out);
        assert_eq!(out, [0.1, 0.1, 0.2, 0.2]);
    }

    #[test]
    fn stereo_to_mono_averages() {
        let mut out = Vec::new();
        convert(&[0.2, 0.6, 1.0, -1.0], 2, 1, &mut out);
        assert!((out[0] - 0.4).abs() < 1e-6 && out[1].abs() < 1e-6);
    }

    #[test]
    fn surround_downmix_stays_in_range() {
        let mut out = Vec::new();
        convert(&[1.0; 6], 6, 2, &mut out);
        assert!(out.iter().all(|s| (s - 1.0).abs() < 1e-5), "{out:?}");
    }

    #[test]
    fn stereo_to_surround_device_uses_front_pair() {
        let mut out = Vec::new();
        convert(&[0.3, 0.4], 2, 6, &mut out);
        assert_eq!(out, [0.3, 0.4, 0.0, 0.0, 0.0, 0.0]);
    }
}
