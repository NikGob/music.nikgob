//! Smoke tests that need a real sound device. Run with
//! `cargo test -p nikgob-audio --test device -- --ignored`.

mod common;

use common::*;
use nikgob_audio::{Event, PlayItem, Player, PlayerOptions};
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
