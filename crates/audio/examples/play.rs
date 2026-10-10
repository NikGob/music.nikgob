//! `cargo run -p nikgob-audio --example play -- <file> [next_file]`
//!
//! Plays the files through the default output device and prints events.
//! Keys on stdin (then Enter): `p` pause/resume, `s` stop, `+`/`-` volume,
//! `>` seek +10 s, `<` seek -10 s, `x` toggle a 3 s crossfade, `q` quit.

use std::io::BufRead;
use std::time::Duration;

use nikgob_audio::{Event, PlayItem, Player, PlayerOptions};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        eprintln!("usage: play <file> [next_file]");
        std::process::exit(2);
    };
    let next = args.next();

    let (player, events) = Player::spawn(PlayerOptions::default()).expect("start the engine");
    player.play(PlayItem::file(1, &first));
    if let Some(next) = next {
        player.set_next(PlayItem::file(2, next));
    }

    // Keyboard on a side thread; the main thread prints events.
    let keys = player.clone();
    let quit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let quit_flag = quit.clone();
    std::thread::spawn(move || {
        let mut crossfade = false;
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match line.trim() {
                "p" => keys.toggle(),
                "s" => keys.stop(),
                "+" => keys.set_volume((keys.volume() + 0.1).min(1.0)),
                "-" => keys.set_volume((keys.volume() - 0.1).max(0.0)),
                ">" => seek_by(&keys, 10),
                "<" => seek_by(&keys, -10),
                "x" => {
                    crossfade = !crossfade;
                    keys.set_crossfade(crossfade.then_some(Duration::from_secs(3)));
                    println!("crossfade {}", if crossfade { "on" } else { "off" });
                }
                "q" => {
                    quit_flag.store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
                _ => {}
            }
        }
        quit_flag.store(true, std::sync::atomic::Ordering::Release);
    });

    let idle_limit = Duration::from_secs(3);
    let mut last_activity = std::time::Instant::now();
    while !quit.load(std::sync::atomic::Ordering::Acquire) {
        match events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                last_activity = std::time::Instant::now();
                match &event {
                    Event::Position { id, pos } => println!("  [{id}] {:.1}s", pos.as_secs_f64()),
                    other => println!("{other:?}"),
                }
                if event == Event::QueueEmpty {
                    break;
                }
                if matches!(event, Event::Error { id: None, .. }) {
                    // No device: nothing will ever play.
                    break;
                }
            }
            Err(_) => {
                if last_activity.elapsed() > idle_limit && player.position().is_none() {
                    break;
                }
            }
        }
    }
    println!("underruns: {}", player.underruns());
}

fn seek_by(player: &Player, secs: i64) {
    if let Some(pos) = player.position() {
        let target = (pos.as_secs_f64() + secs as f64).max(0.0);
        player.seek(Duration::from_secs_f64(target));
    }
}
