//! Speed, tone, reverb and the equalizer through the whole engine on the
//! offline backend: lengths, positions, seeks, joins and the absence of
//! clicks when anything changes.

mod common;

use common::*;
use nikgob_audio::{EQ_BANDS, Event, ItemFx, PlayItem, Reverb, eq_preamp_db};
use std::f64::consts::TAU;
use std::path::PathBuf;
use std::time::Duration;

const RATE: u32 = 48_000;
/// Samples at the very start are shaped by the 8 ms de-click ramp.
const SKIP: usize = 512;

fn speed(speed: f32) -> ItemFx {
    ItemFx {
        speed,
        ..ItemFx::default()
    }
}

fn echo() -> ItemFx {
    ItemFx {
        speed: 1.0,
        low_shelf_db: 4.0,
        high_shelf_db: -3.0,
        preamp_db: -3.0,
        reverb: Some(Reverb {
            wet: 0.4,
            decay_s: 2.8,
            damping: 0.45,
            pre_delay_ms: 25.0,
        }),
        keep_formants: false,
    }
}

/// A stereo WAV of one sine.
fn sine_wav(dir: &tempfile::TempDir, name: &str, rate: u32, freq: f64, amp: f64, frames: usize) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .flat_map(|i| {
            let s = (amp * (TAU * freq * i as f64 / f64::from(rate)).sin()) as f32;
            [s, s]
        })
        .collect();
    let p = path(dir, name);
    write_wav_f32(&p, rate, 2, &data);
    p
}

/// A stereo WAV whose pitch tells the time: second `n` of it is a sine of
/// `400 + 100 n` Hz.
fn stairs_wav(dir: &tempfile::TempDir, name: &str, seconds: usize) -> PathBuf {
    let mut phase = 0.0f64;
    let mut data = Vec::with_capacity(seconds * RATE as usize * 2);
    for i in 0..seconds * RATE as usize {
        let freq = 400.0 + 100.0 * (i / RATE as usize) as f64;
        phase += TAU * freq / f64::from(RATE);
        let s = (0.4 * phase.sin()) as f32;
        data.extend_from_slice(&[s, s]);
    }
    let p = path(dir, name);
    write_wav_f32(&p, RATE, 2, &data);
    p
}

fn left(interleaved: &[f32]) -> Vec<f32> {
    interleaved.iter().step_by(2).copied().collect()
}

/// Frequency by counting rising zero crossings, with sub-sample interpolation.
fn measure_freq(samples: &[f32], rate: u32) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for i in 1..samples.len() {
        if samples[i - 1] < 0.0 && samples[i] >= 0.0 {
            let frac = f64::from(-samples[i - 1]) / f64::from(samples[i] - samples[i - 1]);
            let t = (i - 1) as f64 + frac;
            if first.is_none() {
                first = Some(t);
            } else {
                count += 1;
            }
            last = t;
        }
    }
    let first = first.expect("no zero crossings");
    count as f64 * f64::from(rate) / (last - first)
}

/// The largest step between two consecutive samples of the left channel.
fn largest_step(interleaved: &[f32]) -> f32 {
    interleaved
        .windows(3)
        .step_by(2)
        .map(|w| (w[2] - w[0]).abs())
        .fold(0.0, f32::max)
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn secs(d: Option<Duration>) -> f64 {
    d.expect("a position").as_secs_f64()
}

// ----- speed ------------------------------------------------------------

#[test]
fn a_faster_item_is_shorter_and_higher_and_its_position_runs_faster() {
    let dir = tmp();
    let frames = RATE as usize * 4;
    let p = sine_wav(&dir, "a.wav", RATE, 1000.0, 0.5, frames);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_fx(speed(1.25)));
    let first = rig.device.render(RATE as usize);
    // A second of sound is a second and a quarter of the item.
    let pos = secs(rig.player.position());
    assert!((pos - 1.25).abs() < 0.003, "position {pos}");
    let rest = rig.device.drain(RATE as usize * 10);
    let total = (first.len() + rest.len()) / 2;
    assert_eq!(total, frames * 4 / 5, "four seconds at 1.25 are 3.2 s of sound");
    let f = measure_freq(&left(&first)[4800..], RATE);
    assert!((f - 1250.0).abs() < 1.0, "pitch follows the speed: {f}");
    // The length reported is the item's own.
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Started { .. })
    });
    match ev.iter().find(|e| matches!(e, Event::Started { .. })) {
        Some(Event::Started {
            duration: Some(d), ..
        }) => assert!((d.as_secs_f64() - 4.0).abs() < 1e-6, "{d:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_slower_item_from_another_rate_is_longer_and_lower() {
    let dir = tmp();
    let rate_in = 44_100u32;
    let frames = rate_in as usize * 3 + 123;
    let p = sine_wav(&dir, "a.wav", rate_in, 1000.0, 0.5, frames);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_fx(speed(0.85)));
    let out = rig.device.drain(RATE as usize * 10);
    // 44100 * 17/20 in, 48000 out: 2499 to 3200.
    assert_eq!(out.len() / 2, (frames * 3200).div_ceil(2499));
    let l = left(&out);
    let f = measure_freq(&l[4800..l.len() - 4800], RATE);
    assert!((f - 850.0).abs() < 1.0, "{f}");
    // No level lost to the conversion in the passband.
    let level = peak(&l[9600..l.len() - 9600]);
    assert!((level - 0.5).abs() < 0.01, "{level}");
}

#[test]
fn speeding_up_does_not_fold_the_highs_back() {
    // 17 kHz at 1.35 would be 22.95 kHz: still under the device's Nyquist.
    // 19 kHz would be 25.65 kHz, over it: it has to go, not alias down to
    // 22.35 kHz.
    let dir = tmp();
    let rate_in = 44_100u32;
    let frames = rate_in as usize * 2;
    for (freq, kept) in [(17_000.0, true), (19_000.0, false)] {
        let p = sine_wav(&dir, "hi.wav", rate_in, freq, 0.5, frames);
        let mut rig = rig(RATE);
        rig.player.play(PlayItem::file(1, &p).with_fx(speed(1.35)));
        let out = rig.device.drain(RATE as usize * 10);
        let level = rms(&out[9600..out.len() - 9600]);
        if kept {
            assert!(level > 0.3, "{freq} Hz was lost: {level}");
        } else {
            assert!(level < 0.5 * 10f32.powf(-60.0 / 20.0), "{freq} Hz aliased: {level}");
        }
    }
}

#[test]
fn slowing_down_keeps_the_highs() {
    // 20 kHz of a 44.1 kHz file lands at 17 kHz: nothing is muffled.
    let dir = tmp();
    let rate_in = 44_100u32;
    let p = sine_wav(&dir, "hi.wav", rate_in, 20_000.0, 0.5, rate_in as usize * 2);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_fx(speed(0.85)));
    let out = rig.device.drain(RATE as usize * 10);
    let level = rms(&out[9600..out.len() - 9600]);
    assert!((level - 0.5 / 2f32.sqrt()).abs() < 0.02, "{level}");
    let l = left(&out);
    let f = measure_freq(&l[4800..l.len() - 4800], RATE);
    assert!((f - 17_000.0).abs() < 5.0, "{f}");
}

#[test]
fn a_seek_at_speed_lands_on_the_items_own_time() {
    let dir = tmp();
    let p = stairs_wav(&dir, "stairs.wav", 8);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_fx(speed(1.25)));
    rig.device.render(5000);
    let target = Duration::from_millis(3_250);
    rig.seek_to(target);
    let out = rig.device.render(RATE as usize / 4);
    // Second 3 of the file is 700 Hz, heard a quarter higher.
    let f = measure_freq(&left(&out)[2400..], RATE);
    assert!((f - 875.0).abs() < 2.0, "{f}");
    // A quarter second of sound later the item is 0.3125 s further (the
    // wait for the seek has already played out the old sound's fade).
    let pos = secs(rig.player.position());
    assert!((pos - 3.5625).abs() < 0.02, "{pos}");
    // The item's last second is 1100 Hz; it ends after 0.4 / 1.25 s more.
    rig.seek_to(Duration::from_millis(7_600));
    let tail = rig.device.drain(RATE as usize * 5);
    // 0.4 s of the item at 1.25 are 15360 frames, less the one frame the
    // wait for the seek had already taken of them.
    let frames = tail.len() / 2;
    assert!(
        (15_358..=15_360 + 400).contains(&frames),
        "{frames} frames"
    );
    let f = measure_freq(&left(&tail)[2400..], RATE);
    assert!((f - 1375.0).abs() < 3.0, "{f}");
}

#[test]
fn two_items_at_one_speed_join_without_a_seam() {
    let dir = tmp();
    let rate_in = 44_100;
    let total = rate_in as usize * 4;
    let cut = rate_in as usize * 2 + 100;
    let ints = to_i16(&signal(rate_in, 0, total));
    let (a, b) = (path(&dir, "a.flac"), path(&dir, "b.flac"));
    write_flac16(&a, rate_in, 2, &ints[..cut * 2]);
    write_flac16(&b, rate_in, 2, &ints[cut * 2..]);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &a).with_fx(speed(1.2)));
    rig.player.set_next(PlayItem::file(2, &b).with_fx(speed(1.2)));
    let out = rig.device.drain(RATE as usize * 30);
    // 44100 * 6/5 in, 48000 out: 441 to 400. One resampler runs through
    // both, so the total is the converted length of the whole recording.
    assert_eq!(out.len() / 2, (total * 400).div_ceil(441));
    let join = (cut * 400).div_ceil(441) * 2;
    let step = |i: usize| (out[i + 2] - out[i]).abs();
    let around: f32 = (join - 40..join + 40).step_by(2).map(step).fold(0.0, f32::max);
    let typical: f32 = (SKIP * 2..join - 200).step_by(2).map(step).fold(0.0, f32::max);
    assert!(around <= typical * 1.2, "join step {around} vs typical {typical}");
    let events = wait_event(&rig.events, Duration::from_secs(2), |e| *e == Event::QueueEmpty);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{events:?}"
    );
}

#[test]
fn items_at_different_speeds_follow_each_other_without_silence() {
    let dir = tmp();
    let rate_in = 44_100;
    let (len_a, len_b) = (rate_in as usize * 2 + 100, rate_in as usize * 2 - 100);
    let a = sine_wav(&dir, "a.wav", rate_in, 500.0, 0.4, len_a);
    let b = sine_wav(&dir, "b.wav", rate_in, 500.0, 0.4, len_b);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &a).with_fx(speed(1.2)));
    rig.player.set_next(PlayItem::file(2, &b).with_fx(speed(0.85)));
    let out = rig.device.drain(RATE as usize * 30);
    // Each item has exactly its own converted length: nothing in between.
    let out_a = (len_a * 400).div_ceil(441);
    let out_b = (len_b * 3200).div_ceil(2499);
    assert_eq!(out.len() / 2, out_a + out_b);
    let l = left(&out);
    let fa = measure_freq(&l[4800..out_a - 2400], RATE);
    let fb = measure_freq(&l[out_a + 2400..l.len() - 2400], RATE);
    assert!((fa - 600.0).abs() < 1.0 && (fb - 425.0).abs() < 1.0, "{fa} then {fb}");
    // No hole around the join: every 2.5 ms there carries the tone.
    for w in l[out_a - 1200..out_a + 1200].chunks(120) {
        let level = rms(w);
        assert!(level > 0.1, "a gap at the join: {level}");
    }
    // The position starts over with the second item at its own speed.
    let mut again = common::rig(RATE);
    again.player.play(PlayItem::file(1, &a).with_fx(speed(1.2)));
    again.player.set_next(PlayItem::file(2, &b).with_fx(speed(0.85)));
    again.device.render(out_a + RATE as usize);
    assert_eq!(again.player.current_id(), Some(2));
    let pos = secs(again.player.position());
    assert!((pos - 0.85).abs() < 0.01, "{pos}");
}

#[test]
fn a_crossfade_starts_where_the_faster_item_really_ends() {
    let dir = tmp();
    // Long enough that the fade starts well past the second the decoder
    // runs ahead: the next item, which opens on its own thread, is there by
    // then (a fade due inside that first second starts when it is, a few
    // milliseconds late and that much shorter, with or without a speed).
    let a = sine_wav(&dir, "a.wav", RATE, 440.0, 0.3, RATE as usize * 6);
    let b = sine_wav(&dir, "b.wav", RATE, 880.0, 0.3, RATE as usize * 3);
    let mut rig = rig(RATE);
    rig.player.set_crossfade(Some(Duration::from_secs(1)));
    rig.player.play(PlayItem::file(1, &a).with_fx(speed(1.5)));
    rig.player.set_next(PlayItem::file(2, &b));
    let out = rig.device.drain(RATE as usize * 30);
    // Six seconds at 1.5 are four of sound; the last of them overlaps the
    // first second of the next item, which plays its own three seconds.
    assert_eq!(out.len() / 2, RATE as usize * 6);
    let l = left(&out);
    let sec = RATE as usize;
    // Before the fade only the first item (660 Hz: 440 at 1.5) ...
    let f = measure_freq(&l[4800..sec * 3 - 2400], RATE);
    assert!((f - 660.0).abs() < 1.0, "{f}");
    // ... in the middle of it both, each at 0.3 * cos(45 deg) ...
    let mid = sec * 3 + sec / 2;
    let level = rms(&l[mid - 2400..mid + 2400]);
    assert!((level - 0.212).abs() < 0.02, "{level}");
    // ... and after it only the second.
    let f = measure_freq(&l[sec * 4 + 2400..sec * 5], RATE);
    assert!((f - 880.0).abs() < 1.0, "{f}");
}

// ----- changing things while it plays -------------------------------------

#[test]
fn changing_the_speed_while_playing_goes_on_from_the_same_place_without_a_click() {
    let dir = tmp();
    let p = sine_wav(&dir, "a.wav", RATE, 1000.0, 0.5, RATE as usize * 6);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(7, &p));
    let mut heard = rig.device.render(RATE as usize / 2);
    let before = secs(rig.player.position());
    rig.player.set_fx(7, speed(1.25));
    // Reported at once, and it does not jump.
    let asked = secs(rig.player.position());
    assert!((asked - before).abs() < 0.001, "{before} -> {asked}");
    heard.extend(rig.device.render(RATE as usize / 2));
    let after = secs(rig.player.position());
    // Half a second of sound at 1.25 (less the few milliseconds the switch
    // itself takes).
    assert!(
        after > before + 0.55 && after < before + 0.64,
        "{before} -> {after}"
    );
    let l = left(&heard);
    let f_old = measure_freq(&l[4800..RATE as usize / 2 - 480], RATE);
    let f_new = measure_freq(&l[RATE as usize / 2 + 4800..], RATE);
    assert!((f_old - 1000.0).abs() < 1.0 && (f_new - 1250.0).abs() < 1.5, "{f_old} then {f_new}");
    // A 1250 Hz sine at 0.5 moves 0.082 per sample at most.
    let step = largest_step(&heard[SKIP * 2..]);
    assert!(step < 0.09, "a click: {step}");
    // Back to normal, and to the echo preset: still no click.
    rig.player.set_fx(7, ItemFx::default());
    heard.extend(rig.device.render(RATE as usize / 2));
    rig.player.set_fx(7, echo());
    heard.extend(rig.device.render(RATE as usize / 2));
    let step = largest_step(&heard[SKIP * 2..]);
    assert!(step < 0.09, "a click: {step}");
    assert_eq!(rig.player.current_id(), Some(7));
}

#[test]
fn a_speed_change_right_after_a_seek_keeps_the_seek() {
    let dir = tmp();
    let p = stairs_wav(&dir, "stairs.wav", 8);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(RATE as usize / 4);
    rig.player.seek(Duration::from_millis(5_200));
    rig.player.set_fx(1, speed(0.85));
    let out = rig.device.render(RATE as usize / 4);
    // Second 5 is 900 Hz, at 0.85.
    let f = measure_freq(&left(&out)[2400..], RATE);
    assert!((f - 765.0).abs() < 2.0, "{f}");
    let pos = secs(rig.player.position());
    assert!((pos - (5.2 + 0.25 * 0.85)).abs() < 0.02, "{pos}");
}

#[test]
fn the_next_item_takes_a_new_speed_before_it_starts() {
    let dir = tmp();
    let a = sine_wav(&dir, "a.wav", RATE, 500.0, 0.4, RATE as usize);
    let b = sine_wav(&dir, "b.wav", RATE, 500.0, 0.4, RATE as usize * 2);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &a));
    rig.player.set_next(PlayItem::file(2, &b));
    rig.device.render(RATE as usize / 4);
    rig.player.set_fx(2, speed(1.25));
    let rest = rig.device.drain(RATE as usize * 10);
    // The first item plays to its own end (the decoder was already past
    // it: nothing buffered of it may be dropped), then the second at its
    // new speed from its first frame.
    let total = RATE as usize / 4 + rest.len() / 2;
    assert_eq!(total, RATE as usize + RATE as usize * 2 * 4 / 5);
    let l = left(&rest);
    let rest_of_first = RATE as usize * 3 / 4;
    let f = measure_freq(&l[2400..rest_of_first - 2400], RATE);
    assert!((f - 500.0).abs() < 1.0, "the end of the first item: {f}");
    let f = measure_freq(&l[rest_of_first + 2400..l.len() - 2400], RATE);
    assert!((f - 625.0).abs() < 1.0, "{f}");
    for w in l[rest_of_first - 1200..rest_of_first + 1200].chunks(120) {
        assert!(rms(w) > 0.1, "a gap at the join");
    }
    let events = wait_event(&rig.events, Duration::from_secs(2), |e| *e == Event::QueueEmpty);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{events:?}"
    );
}

#[test]
fn a_new_speed_for_an_item_fading_in_waits_until_it_is_heard() {
    // With a crossfade the next item's start is mixed into the end of the
    // playing one a second before the listener gets there: the change must
    // not cut the playing item short, and still has to come.
    let dir = tmp();
    let a = sine_wav(&dir, "a.wav", RATE, 500.0, 0.4, RATE as usize * 3);
    let b = sine_wav(&dir, "b.wav", RATE, 500.0, 0.4, RATE as usize * 3);
    let mut rig = rig(RATE);
    rig.player.set_crossfade(Some(Duration::from_secs(1)));
    rig.player.play(PlayItem::file(1, &a));
    rig.player.set_next(PlayItem::file(2, &b));
    // The listener is at 1.5 s, the decoder about a second on: in the fade.
    let first = rig.device.render(RATE as usize * 3 / 2);
    assert_eq!(rig.player.current_id(), Some(1));
    rig.player.set_fx(2, speed(1.25));
    let rest = rig.device.drain(RATE as usize * 10);
    let total = (first.len() + rest.len()) / 2;
    // Untouched it would be 5 s; changed the moment the second item is
    // heard (at 2 s), 2 s + 3 s / 1.25.
    let sec = RATE as usize;
    assert!(total > sec * 43 / 10 && total < sec * 49 / 10, "{total} frames");
    let l = left(&rest);
    // The playing item went on undisturbed up to the fade ...
    let f = measure_freq(&l[2400..sec / 2 - 2400], RATE);
    assert!((f - 500.0).abs() < 1.0, "{f}");
    // ... and the second ends at its new speed.
    let f = measure_freq(&l[l.len() - sec / 2..l.len() - 2400], RATE);
    assert!((f - 625.0).abs() < 1.0, "{f}");
}


// ----- timbre ---------------------------------------------------------------

#[test]
fn keeping_the_timbre_changes_neither_length_nor_pitch_nor_the_join() {
    let dir = tmp();
    let rate_in = 44_100;
    let total = rate_in as usize * 4;
    let cut = rate_in as usize * 2 + 100;
    let ints = to_i16(&signal(rate_in, 0, total));
    let (a, b) = (path(&dir, "a.flac"), path(&dir, "b.flac"));
    write_flac16(&a, rate_in, 2, &ints[..cut * 2]);
    write_flac16(&b, rate_in, 2, &ints[cut * 2..]);
    let fx = ItemFx {
        keep_formants: true,
        ..speed(1.2)
    };
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &a).with_fx(fx));
    rig.player.set_next(PlayItem::file(2, &b).with_fx(fx));
    let out = rig.device.drain(RATE as usize * 30);
    assert_eq!(out.len() / 2, (total * 400).div_ceil(441));
    let join = (cut * 400).div_ceil(441) * 2;
    let step = |i: usize| (out[i + 2] - out[i]).abs();
    let around: f32 = (join - 40..join + 40).step_by(2).map(step).fold(0.0, f32::max);
    let typical: f32 = (SKIP * 2..join - 200).step_by(2).map(step).fold(0.0, f32::max);
    assert!(around <= typical * 1.2, "join step {around} vs typical {typical}");

    let p = sine_wav(&dir, "s.wav", RATE, 1000.0, 0.5, RATE as usize * 2);
    let mut one = common::rig(RATE);
    one.player.play(PlayItem::file(3, &p).with_fx(ItemFx {
        keep_formants: true,
        ..speed(1.25)
    }));
    let out = one.device.drain(RATE as usize * 10);
    assert_eq!(out.len() / 2, RATE as usize * 2 * 4 / 5);
    let l = left(&out);
    let f = measure_freq(&l[4800..l.len() - 4800], RATE);
    assert!((f - 1250.0).abs() < 1.0, "{f}");
}

// ----- reverb ---------------------------------------------------------------

#[test]
fn the_echo_rings_out_after_the_last_item_and_ends_in_silence() {
    let dir = tmp();
    let frames = RATE as usize;
    let data = signal(RATE, 0, frames);
    let p = path(&dir, "a.wav");
    write_wav_f32(&p, RATE, 2, &data);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_fx(echo()));
    let out = rig.device.drain(RATE as usize * 20);
    let tail = &out[frames * 2..];
    assert!(tail.len() / 2 > RATE as usize * 2, "no tail: {} frames", tail.len() / 2);
    assert!(rms(&tail[..RATE as usize]) > 1e-3, "the tail is silent");
    assert!(peak(&tail[tail.len() - 200..]) < 1e-3, "cut off while still sounding");
    // The tail only falls.
    let early = rms(&tail[..RATE as usize / 2]);
    let late = rms(&tail[tail.len() - RATE as usize / 2..]);
    assert!(late < early * 0.05, "{early} -> {late}");
}

#[test]
fn a_gapless_join_under_the_echo_is_the_uncut_recording() {
    // The item that ended rings on under the next one; added to the next
    // item's own fresh reverb that is exactly what one uncut item gives.
    let dir = tmp();
    let total = RATE as usize * 2;
    let cut = RATE as usize + 321;
    let original = signal(RATE, 0, total);
    let (whole, a, b) = (path(&dir, "w.wav"), path(&dir, "a.wav"), path(&dir, "b.wav"));
    write_wav_f32(&whole, RATE, 2, &original);
    write_wav_f32(&a, RATE, 2, &original[..cut * 2]);
    write_wav_f32(&b, RATE, 2, &original[cut * 2..]);

    let mut one = rig(RATE);
    one.player.play(PlayItem::file(1, &whole).with_fx(echo()));
    let uncut = one.device.drain(RATE as usize * 20);

    let mut two = rig(RATE);
    two.player.play(PlayItem::file(1, &a).with_fx(echo()));
    two.player.set_next(PlayItem::file(2, &b).with_fx(echo()));
    let joined = two.device.drain(RATE as usize * 20);

    assert_eq!(uncut.len(), joined.len());
    let d = max_diff(&joined[SKIP * 2..total * 2], &uncut[SKIP * 2..total * 2]);
    assert!(d < 1e-3, "the join is heard: {d}");
}

#[test]
fn the_echo_preset_does_not_sit_in_the_limiter() {
    let dir = tmp();
    // As hot as a mix gets: full-scale noise-like signal.
    let frames = RATE as usize * 3;
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let data: Vec<f32> = (0..frames * 2)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        })
        .collect();
    let p = path(&dir, "hot.wav");
    write_wav_f32(&p, RATE, 2, &data);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_fx(echo()));
    let out = rig.device.render(frames - 1000);
    assert!(out.iter().all(|s| s.abs() <= 1.0));
    let body = &out[RATE as usize * 2..];
    let over = body.iter().filter(|s| s.abs() > 0.89).count();
    assert!(over * 20 < body.len(), "{over} of {} samples in the limiter's knee", body.len());
}

// ----- equalizer --------------------------------------------------------------

fn only(band: usize, db: f32) -> [f32; EQ_BANDS] {
    let mut g = [0.0; EQ_BANDS];
    g[band] = db;
    g
}

#[test]
fn the_equalizer_is_out_of_the_path_when_off_or_flat() {
    let dir = tmp();
    let total = RATE as usize;
    let original = signal(RATE, 0, total);
    let p = path(&dir, "a.wav");
    write_wav_f32(&p, RATE, 2, &original);
    for (on, gains) in [(false, only(1, 9.0)), (true, [0.0; EQ_BANDS])] {
        let mut rig = rig(RATE);
        rig.player.set_eq(on, gains);
        rig.player.play(PlayItem::file(1, &p));
        let out = rig.device.drain(RATE as usize * 5);
        assert_eq!(out.len(), original.len());
        assert_eq!(&out[SKIP * 2..], &original[SKIP * 2..], "on {on}: not bit-exact");
    }
}

#[test]
fn a_band_lifts_its_tone_and_the_preamp_makes_room_for_it() {
    let dir = tmp();
    let p = sine_wav(&dir, "a.wav", RATE, 1000.0, 0.25, RATE as usize * 2);
    let mut rig = rig(RATE);
    rig.player.set_eq(true, only(5, 6.0));
    rig.player.play(PlayItem::file(1, &p));
    let out = rig.device.render(RATE as usize * 3 / 2);
    // +6 dB at 1 kHz, behind a preamp of -6 dB: the tone keeps its level,
    // everything else drops.
    let level = peak(&out[RATE as usize * 2..]);
    assert!((level - 0.25).abs() < 0.01, "{level}");
    assert!((eq_preamp_db(&only(5, 6.0)) + 6.0).abs() < 0.1);

    let low = sine_wav(&dir, "low.wav", RATE, 64.0, 0.25, RATE as usize * 2);
    let mut rig = common::rig(RATE);
    rig.player.set_eq(true, only(5, 6.0));
    rig.player.play(PlayItem::file(1, &low));
    let out = rig.device.render(RATE as usize * 3 / 2);
    let level = peak(&out[RATE as usize * 2..]);
    assert!((level - 0.25 * 0.5).abs() < 0.01, "{level}");
}

#[test]
fn the_strongest_preset_never_clips_a_full_scale_tone() {
    let dir = tmp();
    let bass = [6.0, 6.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let all_up = [12.0; EQ_BANDS];
    for gains in [bass, all_up] {
        for freq in [45.0, 64.0, 90.0, 180.0, 1000.0] {
            let p = sine_wav(&dir, "full.wav", RATE, freq, 1.0, RATE as usize * 2);
            let mut rig = rig(RATE);
            rig.player.set_eq(true, gains);
            rig.player.play(PlayItem::file(1, &p));
            let out = rig.device.render(RATE as usize * 3 / 2);
            assert!(out.iter().all(|s| s.abs() <= 1.0), "{gains:?} at {freq} Hz");
            // Never louder than the same tone without the equalizer (a
            // full-scale sine comes out of the limiter at 0.974).
            let level = peak(&out[RATE as usize * 2..]);
            assert!(level > 0.05 && level <= 0.98, "{gains:?} at {freq} Hz: {level}");
        }
    }
}

#[test]
fn switching_presets_and_the_equalizer_itself_makes_no_click() {
    let dir = tmp();
    // A 100 Hz sine at 0.5 moves 0.0065 per sample at most.
    let p = sine_wav(&dir, "a.wav", RATE, 100.0, 0.5, RATE as usize * 6);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    let mut heard = rig.device.render(RATE as usize / 2);
    let bass = [6.0, 6.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let bright = [-6.0, -6.0, -3.0, 0.0, 0.0, 0.0, 3.0, 6.0, 9.0, 12.0];
    for (on, gains) in [(true, bass), (true, bright), (false, bright), (true, bass)] {
        rig.player.set_eq(on, gains);
        heard.extend(rig.device.render(RATE as usize / 2));
    }
    let step = largest_step(&heard[SKIP * 2..]);
    assert!(step < 0.012, "a click: {step}");
}

#[test]
fn pausing_with_the_equalizer_on_makes_no_click() {
    let dir = tmp();
    let p = sine_wav(&dir, "a.wav", RATE, 50.0, 0.4, RATE as usize * 6);
    let mut rig = rig(RATE);
    rig.player.set_eq(true, only(0, 12.0));
    rig.player.play(PlayItem::file(1, &p));
    let mut heard = rig.device.render(RATE as usize / 2);
    rig.player.pause();
    wait_event(&rig.events, Duration::from_secs(2), |e| *e == Event::Paused);
    // What the device still pulls before it is halted.
    for _ in 0..80 {
        heard.extend(rig.device.render(256));
        std::thread::sleep(Duration::from_millis(2));
    }
    rig.player.resume();
    wait_event(&rig.events, Duration::from_secs(2), |e| *e == Event::Resumed);
    std::thread::sleep(Duration::from_millis(50));
    heard.extend(rig.device.render(RATE as usize / 2));
    // 50 Hz at 0.4 behind a 12 dB preamp and a boost that does not reach it
    // fully: far under 0.01 a sample.
    let step = largest_step(&heard[SKIP * 2..]);
    assert!(step < 0.01, "a click: {step}");
}

