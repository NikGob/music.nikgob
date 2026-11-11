//! `nikgob --quit`: the running player (or the developer profile, with
//! `--demo-profile`) quits as from the tray's «Выход», its session and
//! position saved, and this launch waits until it is gone, so a script can
//! replace the exe right after:
//!
//! ```text
//! nikgob --quit && copy /y new\nikgob.exe nikgob.exe
//! ```
//!
//! Exit codes: 0 when the player has quit or was not running, 1 when it
//! did not take the request or is still closing after [`WAIT`].

use std::io::ErrorKind;
use std::time::{Duration, Instant};

use fastframe_instance::Slot;

/// The flag.
pub const FLAG: &str = "--quit";

/// How long the launch waits for the running copy to end: its shutdown
/// saves the session, stops the torrents and lets Soloist leave Spotify
/// Connect.
const WAIT: Duration = Duration::from_secs(30);

/// How often it asks whether the running copy is still there.
const POLL: Duration = Duration::from_millis(100);

/// What the running copy made of the request.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    Taken,
    NotRunning,
    Refused(String),
}

/// Whether this launch is a `--quit`.
pub fn requested() -> bool {
    std::env::args().any(|a| a == FLAG)
}

/// Asks, waits, and ends the process.
pub fn run(slot: &Slot) -> ! {
    match ask(slot) {
        Asked::Taken => {}
        Asked::NotRunning => {
            eprintln!("music.nikgob is not running");
            std::process::exit(0);
        }
        Asked::Refused(e) => {
            eprintln!("music.nikgob did not take the request: {e}");
            std::process::exit(1);
        }
    }
    if wait_gone(slot, WAIT, POLL) {
        // The process is gone; its exe is free a moment later.
        std::thread::sleep(Duration::from_millis(200));
        eprintln!("music.nikgob has quit");
        std::process::exit(0);
    }
    eprintln!("music.nikgob is still closing after {} s", WAIT.as_secs());
    std::process::exit(1);
}

fn ask(slot: &Slot) -> Asked {
    match slot.send("quit") {
        Ok(_) => Asked::Taken,
        Err(e) if not_running(e.kind()) => Asked::NotRunning,
        Err(e) => Asked::Refused(e.to_string()),
    }
}

/// Waits up to `wait` for the slot's owner to stop answering, which it
/// does only when its process has ended.
fn wait_gone(slot: &Slot, wait: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if slot.send("ping").is_err_and(|e| not_running(e.kind())) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

fn not_running(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::NotFound | ErrorKind::ConnectionRefused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_running_is_gone_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Slot::at(dir.path(), "nikgob-quit-test");
        assert_eq!(ask(&slot), Asked::NotRunning);
        assert!(wait_gone(&slot, Duration::from_secs(5), Duration::from_millis(10)));
    }

    #[test]
    fn a_running_copy_takes_the_request_and_is_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let slot = Slot::at(dir.path(), "nikgob-quit-test");
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = std::sync::Arc::clone(&asked);
        let fastframe_instance::Claim::First(_guard) = slot.claim("show", move |r| match r {
            "quit" => {
                seen.store(true, std::sync::atomic::Ordering::Relaxed);
                Some("ok".to_owned())
            }
            "ping" => Some("ok".to_owned()),
            _ => None,
        }) else {
            panic!("the slot in a fresh directory is free");
        };
        assert_eq!(ask(&slot), Asked::Taken);
        assert!(asked.load(std::sync::atomic::Ordering::Relaxed));
        // Still answering: not gone within the wait.
        assert!(!wait_gone(&slot, Duration::from_millis(150), Duration::from_millis(20)));
    }
}
