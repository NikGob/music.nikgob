//! `--console`: commands typed on stdin, to try the player by hand where no
//! server drives it yet (the duck and the output on the PC or the Pi).
//! `help` lists them.

use nikgob_runtime::Runtime;
use nikgob_view::Command;

const HELP: &str = "\
play | pause | toggle | next | prev
vol <0..1>
duck <gain 0..1> [fade_ms=120] [ttl_ms=10000]
unduck [fade_ms=450]
device <name or PulseAudio sink> | device default
liked                 play «Любимые» shuffled
wave                  play «Моя волна»
status | help";

/// What a line asks for.
#[derive(Debug, PartialEq)]
pub enum Line {
    Send(Command),
    Status,
    Help,
    Nothing,
}

/// The line typed, or why it is not a command.
pub fn parse(line: &str) -> Result<Line, String> {
    let line = line.trim();
    let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    let args: Vec<&str> = rest.split_whitespace().collect();
    let number = |i: usize, default: Option<f64>| -> Result<f64, String> {
        match args.get(i) {
            Some(a) => a.parse::<f64>().map_err(|_| format!("{a}: not a number")),
            None => default.ok_or_else(|| format!("{word} needs a number")),
        }
    };
    let fraction = |i: usize| -> Result<f32, String> {
        let a = args.get(i).ok_or_else(|| format!("{word} needs a number"))?;
        a.parse::<f32>().map_err(|_| format!("{a}: not a number"))
    };
    let ms = |i: usize, default: u32| -> Result<u32, String> {
        let n = number(i, Some(f64::from(default)))?;
        if n.is_finite() && n >= 0.0 { Ok(n.min(f64::from(u32::MAX)) as u32) } else { Err(format!("{n}: not a time")) }
    };
    Ok(match word {
        "" => Line::Nothing,
        "play" => Line::Send(Command::Play),
        "pause" => Line::Send(Command::Pause),
        "toggle" => Line::Send(Command::TogglePlay),
        "next" => Line::Send(Command::Next),
        "prev" => Line::Send(Command::Previous),
        "vol" => Line::Send(Command::SetVolume(fraction(0)?)),
        "duck" => Line::Send(Command::SetDuck {
            gain: fraction(0)?,
            fade_ms: ms(1, 120)?,
            ttl_ms: ms(2, 10_000)?,
        }),
        "unduck" => Line::Send(Command::ClearDuck { fade_ms: ms(0, 450)? }),
        "device" if rest.is_empty() => return Err("device needs a name, or default".into()),
        "device" if rest == "default" => Line::Send(Command::SetOutputDevice(None)),
        "device" => Line::Send(Command::SetOutputDevice(Some(rest.to_owned()))),
        "liked" => Line::Send(Command::PlayPlaylist { id: nikgob_view::LIKED_PLAYLIST, start: None, shuffle: true }),
        "wave" => Line::Send(Command::PlayWave { preset: None }),
        "status" => Line::Status,
        "help" | "?" => Line::Help,
        other => return Err(format!("{other}: no such command (help lists them)")),
    })
}

/// Reads stdin until it closes, then waits for good (the stop comes from a
/// signal, not from the end of stdin, which systemd closes at once). The
/// reads block on a thread of their own that nobody waits for, so a stop
/// while a read waits for input ends the process at once.
pub async fn run(runtime: &Runtime) {
    eprintln!("nikgobd console; help lists the commands");
    let (tx, mut lines) = tokio::sync::mpsc::channel::<String>(16);
    let reader = std::thread::Builder::new().name("nikgobd-console".into()).spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    });
    if let Err(e) = reader {
        eprintln!("no console: {e}");
    }
    while let Some(line) = lines.recv().await {
        match parse(&line) {
            Ok(Line::Send(c)) => runtime.send(c),
            Ok(Line::Status) => eprintln!("{}", status(runtime)),
            Ok(Line::Help) => eprintln!("{HELP}"),
            Ok(Line::Nothing) => {}
            Err(e) => eprintln!("{e}"),
        }
    }
    std::future::pending::<()>().await;
}

fn status(runtime: &Runtime) -> String {
    let s = runtime.state();
    let p = &s.player;
    let track = p.current.as_ref().map_or_else(|| "nothing".to_owned(), |t| format!("{} — {}", t.artist, t.title));
    let duck = p.duck.map_or_else(|| "none".to_owned(), |d| format!("{} until unix ms {}", d.gain, d.until_unix_ms));
    format!(
        "{:?}: {track} at {:.1} s\nvolume {:.2}{}, duck {duck}\noutput {}\noutputs here: {}",
        p.status,
        p.position.as_secs_f64(),
        p.volume,
        if p.muted { " (muted)" } else { "" },
        s.prefs.output_device.as_deref().unwrap_or("the system default"),
        s.prefs.output_devices.join(" | "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_duck_and_its_defaults() {
        assert_eq!(parse("duck 0.18").unwrap(), Line::Send(Command::SetDuck { gain: 0.18, fade_ms: 120, ttl_ms: 10_000 }));
        assert_eq!(parse("  duck 0.03 80 15000 ").unwrap(), Line::Send(Command::SetDuck { gain: 0.03, fade_ms: 80, ttl_ms: 15_000 }));
        assert_eq!(parse("unduck").unwrap(), Line::Send(Command::ClearDuck { fade_ms: 450 }));
        assert_eq!(parse("unduck 0").unwrap(), Line::Send(Command::ClearDuck { fade_ms: 0 }));
        assert!(parse("duck").is_err());
        assert!(parse("duck loud").is_err());
        assert!(parse("unduck -5").is_err());
    }

    #[test]
    fn a_device_by_any_name_or_the_default() {
        assert_eq!(
            parse("device alsa_output.usb-3142_fifine_Headset-00.analog-stereo").unwrap(),
            Line::Send(Command::SetOutputDevice(Some("alsa_output.usb-3142_fifine_Headset-00.analog-stereo".into())))
        );
        assert_eq!(
            parse("device Speakers (Realtek(R) Audio)").unwrap(),
            Line::Send(Command::SetOutputDevice(Some("Speakers (Realtek(R) Audio)".into())))
        );
        assert_eq!(parse("device default").unwrap(), Line::Send(Command::SetOutputDevice(None)));
        assert!(parse("device").is_err());
    }

    #[test]
    fn the_rest() {
        assert_eq!(parse("").unwrap(), Line::Nothing);
        assert_eq!(parse("vol 0.5").unwrap(), Line::Send(Command::SetVolume(0.5)));
        assert_eq!(parse("status").unwrap(), Line::Status);
        assert_eq!(parse("help").unwrap(), Line::Help);
        assert!(parse("dance").is_err());
    }
}
