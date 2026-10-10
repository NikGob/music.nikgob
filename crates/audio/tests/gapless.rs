//! Gapless playback: consecutive pieces of one recording must concatenate
//! sample-exactly through the whole engine (decoder, ring, callback).

mod common;

use common::*;
use nikgob_audio::{Event, PlayItem};
use std::time::Duration;

const RATE: u32 = 48_000;
/// Samples at the very start are shaped by the 8 ms de-click ramp.
const SKIP: usize = 512;

fn check_concatenation(a: &std::path::Path, b: &std::path::Path, original: &[f32], tol: f32) {
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, a));
    rig.player.set_next(PlayItem::file(2, b));
    let out = rig.device.drain(RATE as usize * 30);
    assert_eq!(out.len(), original.len(), "total length");
    let d = max_diff(&out[SKIP * 2..], &original[SKIP * 2..]);
    assert!(d <= tol, "max deviation {d}");

    let events = wait_event(&rig.events, Duration::from_secs(2), |e| {
        *e == Event::QueueEmpty
    });
    let kinds: Vec<_> = events
        .iter()
        .filter(|e| !matches!(e, Event::Position { .. }))
        .collect();
    assert!(
        matches!(kinds[0], Event::Started { id: 1, .. }),
        "{kinds:?}"
    );
    assert!(
        matches!(kinds[1], Event::TrackChanged { from: 1, to: 2, .. }),
        "{kinds:?}"
    );
    assert!(matches!(kinds[2], Event::Ended { id: 2 }), "{kinds:?}");
    assert!(matches!(kinds[3], Event::QueueEmpty), "{kinds:?}");
}

#[test]
fn wav_pair_concatenates_exactly() {
    let dir = tmp();
    let total = RATE as usize * 3 + 777;
    let cut = RATE as usize + 12_345;
    let original = signal(RATE, 0, total);
    let (a, b) = (path(&dir, "a.wav"), path(&dir, "b.wav"));
    write_wav_f32(&a, RATE, 2, &original[..cut * 2]);
    write_wav_f32(&b, RATE, 2, &original[cut * 2..]);
    check_concatenation(&a, &b, &original, 1e-6);
}

#[test]
fn flac_pair_concatenates_exactly() {
    let dir = tmp();
    let total = RATE as usize * 3 + 4321;
    let cut = RATE as usize * 2 + 1;
    let ints = to_i16(&signal(RATE, 0, total));
    let original = from_i16(&ints);
    let (a, b) = (path(&dir, "a.flac"), path(&dir, "b.flac"));
    write_flac16(&a, RATE, 2, &ints[..cut * 2]);
    write_flac16(&b, RATE, 2, &ints[cut * 2..]);
    check_concatenation(&a, &b, &original, 1e-6);
}

#[test]
fn flac_with_resampling_keeps_total_length_and_joins_smoothly() {
    // 44.1 kHz material on a 48 kHz device: the join must not click and the
    // length must match the rate conversion.
    let dir = tmp();
    let rate_in = 44_100;
    let total = rate_in as usize * 4;
    let cut = rate_in as usize * 2 + 100;
    let ints = to_i16(&signal(rate_in, 0, total));
    let (a, b) = (path(&dir, "a.flac"), path(&dir, "b.flac"));
    write_flac16(&a, rate_in, 2, &ints[..cut * 2]);
    write_flac16(&b, rate_in, 2, &ints[cut * 2..]);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &a));
    rig.player.set_next(PlayItem::file(2, &b));
    let out = rig.device.drain(RATE as usize * 30);
    let want_a = (cut * 48_000).div_ceil(44_100);
    // Rate conversion is continuous across the join: the total is the
    // converted length of the whole recording.
    assert_eq!(out.len() / 2, (total * 48_000).div_ceil(44_100));
    // A click at the join would show as a step much larger than the largest
    // step anywhere else in this (band-limited, noisy) signal.
    let join = want_a * 2;
    let step = |i: usize| (out[i + 2] - out[i]).abs();
    let around: f32 = (join - 40..join + 40)
        .step_by(2)
        .map(step)
        .fold(0.0, f32::max);
    let typical: f32 = (SKIP * 2..join - 200)
        .step_by(2)
        .map(step)
        .fold(0.0, f32::max);
    assert!(
        around <= typical * 1.2,
        "join step {around} vs typical {typical}"
    );
}

#[test]
fn opus_pair_concatenates_exactly_and_lengths_are_exact() {
    let dir = tmp();
    let total = RATE as usize * 3 + 555;
    let cut = RATE as usize + 7_000;
    let original = signal(RATE, 0, total);
    let (a, b) = (path(&dir, "a.opus"), path(&dir, "b.opus"));
    write_opus(&a, &original[..cut * 2], 0);
    write_opus(&b, &original[cut * 2..], 0);

    // Each file alone decodes to exactly its input length, and aligned.
    let mut single = rig(RATE);
    single.player.play(PlayItem::file(1, &a));
    let out_a = single.device.drain(RATE as usize * 30);
    assert_eq!(out_a.len() / 2, cut, "opus A length (pre-skip / end trim)");
    let snr = snr_db(&original[SKIP * 2..cut * 2], &out_a[SKIP * 2..]);
    assert!(snr > 15.0, "A is aligned with its source: SNR {snr:.1} dB");

    let mut single_b = rig(RATE);
    single_b.player.play(PlayItem::file(2, &b));
    let out_b = single_b.device.drain(RATE as usize * 30);
    assert_eq!(out_b.len() / 2, total - cut, "opus B length");

    // The pair through the engine equals the two decoded separately, joined.
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &a));
    rig.player.set_next(PlayItem::file(2, &b));
    let out = rig.device.drain(RATE as usize * 30);
    assert_eq!(out.len() / 2, total);
    let mut joined = out_a.clone();
    joined.extend_from_slice(&out_b);
    // (Run alone, B starts with the 8 ms de-click ramp; inside the pair it
    // does not.)
    assert!(max_diff(&out[SKIP * 2..cut * 2], &joined[SKIP * 2..cut * 2]) < 1e-6);
    assert!(max_diff(&out[(cut + SKIP) * 2..], &joined[(cut + SKIP) * 2..]) < 1e-6);
}
