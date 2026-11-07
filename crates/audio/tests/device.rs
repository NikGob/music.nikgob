//! Smoke tests that need a real sound device. Run with
//! `cargo test -p nikgob-audio --test device -- --ignored`.

mod common;

use common::*;
use nikgob_audio::{Event, PlayItem, Player, PlayerOptions};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[test]
#[ignore = "needs an audio output device"]
fn plays_two_files_gaplessly_on_the_default_device() {
    let dir = tmp();
    let a = path(&dir, "a.wav");
    let b = path(&dir, "b.wav");
    write_wav_f32(&a, 48_000, 2, &signal(48_000, 0, 48_000 * 2));
    write_wav_f32(&b, 48_000, 2, &signal(48_000, 96_000, 48_000 * 2));
    let (player, events) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(0.2);
    player.play(PlayItem::file(1, &a));
    player.set_next(PlayItem::file(2, &b));
    let seen = wait_event(&events, Duration::from_secs(10), |e| {
        *e == Event::QueueEmpty
    });
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::Started { id: 1, .. })),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{seen:?}"
    );
    assert!(seen.contains(&Event::QueueEmpty), "{seen:?}");
    assert_eq!(player.underruns(), 0);
}

#[test]
#[ignore = "needs an audio output device"]
fn pause_really_stops_the_callbacks_and_position_freezes() {
    let dir = tmp();
    let a = path(&dir, "a.wav");
    write_wav_f32(&a, 48_000, 2, &signal(48_000, 0, 48_000 * 5));
    let (player, _events) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(0.1);
    player.play(PlayItem::file(1, &a));
    std::thread::sleep(Duration::from_millis(800));
    let moving = player.position().unwrap();
    assert!(moving > Duration::from_millis(300), "{moving:?}");
    player.pause();
    std::thread::sleep(Duration::from_millis(400));
    let frozen = player.position().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(player.position().unwrap(), frozen);
    player.resume();
    std::thread::sleep(Duration::from_millis(500));
    assert!(player.position().unwrap() > frozen);
}

#[test]
#[ignore = "needs an audio output device"]
fn a_busy_cpu_does_not_make_the_device_callbacks_late() {
    let dir = tmp();
    let a = path(&dir, "a.wav");
    write_wav_f32(&a, 48_000, 2, &signal(48_000, 0, 48_000 * 7));
    let (player, _events) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(0.1);
    player.play(PlayItem::file(1, &a));
    std::thread::sleep(Duration::from_secs(1));
    // Every core and then some kept busy at normal priority for 5 s, the
    // way bliss analysis and torrent hashing load it.
    let stop = Arc::new(AtomicBool::new(false));
    let threads = std::thread::available_parallelism().map_or(8, |n| n.get()) + 4;
    let busy: Vec<_> = (0..threads.max(12))
        .map(|i| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut x = i as u64 + 1;
                while !stop.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    }
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(5));
    stop.store(true, Ordering::Relaxed);
    for t in busy {
        t.join().unwrap();
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(player.late_callbacks(), 0, "the device ran dry");
    assert_eq!(player.underruns(), 0, "the ring ran dry");
}
