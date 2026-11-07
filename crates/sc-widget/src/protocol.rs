//! The lines between the player and its widget host (`nikgob --sc-widget
//! <profile dir>`): commands on the host's stdin, events on its stdout, one
//! per line, ASCII.
//!
//! Every load gets a number (`seq`) from the player; the commands for a
//! track carry it and the page ignores those for another one, and the
//! events carry the load they are about, so nothing of the track before is
//! taken for the one now.
//!
//! ```text
//! player → host                 host → player
//! LOAD <seq> <track id>         HOST                      the page and SC.Widget's script are up
//! PLAY <seq>                    <seq> READY <duration ms> the sound is loaded; it plays now
//! PAUSE <seq>                   <seq> PLAY | PAUSE | FINISH
//! SEEK <seq> <ms>               <seq> PROGRESS <ms>
//! VOLUME <0-100>                <seq> ERROR <text>
//!                               FAIL <text>               the host cannot play anything
//! ```

/// The flag of the host run.
pub const FLAG: &str = "--sc-widget";

/// What the player asks of the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Loads SoundCloud track `track` (its numeric id) and plays it.
    Load { seq: u64, track: u64 },
    Play { seq: u64 },
    Pause { seq: u64 },
    Seek { seq: u64, ms: u64 },
    /// The widget's own volume (kept at 100: the level is set elsewhere).
    Volume(u8),
}

impl Command {
    pub fn line(&self) -> String {
        match self {
            Self::Load { seq, track } => format!("LOAD {seq} {track}"),
            Self::Play { seq } => format!("PLAY {seq}"),
            Self::Pause { seq } => format!("PAUSE {seq}"),
            Self::Seek { seq, ms } => format!("SEEK {seq} {ms}"),
            Self::Volume(v) => format!("VOLUME {v}"),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let mut w = line.split_ascii_whitespace();
        let verb = w.next()?;
        let mut num = || w.next().and_then(|s| s.parse::<u64>().ok());
        let c = match verb {
            "LOAD" => Self::Load { seq: num()?, track: num()? },
            "PLAY" => Self::Play { seq: num()? },
            "PAUSE" => Self::Pause { seq: num()? },
            "SEEK" => Self::Seek { seq: num()?, ms: num()? },
            "VOLUME" => Self::Volume(u8::try_from(num()?).ok()?.min(100)),
            _ => return None,
        };
        Some(c)
    }

    /// The call into the page (`window.nk`, see `page.html`). Only numbers
    /// go in, so nothing needs escaping.
    pub fn js(&self) -> String {
        match self {
            Self::Load { seq, track } => format!("window.nk.load({seq},{track})"),
            Self::Play { seq } => format!("window.nk.play({seq})"),
            Self::Pause { seq } => format!("window.nk.pause({seq})"),
            Self::Seek { seq, ms } => format!("window.nk.seek({seq},{ms})"),
            Self::Volume(v) => format!("window.nk.volume({v})"),
        }
    }
}

/// What the host reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Host,
    Ready { seq: u64, duration_ms: Option<u64> },
    Play { seq: u64 },
    Pause { seq: u64 },
    Progress { seq: u64, ms: u64 },
    Finish { seq: u64 },
    Error { seq: u64, text: String },
    Fail(String),
}

/// Text from the page, kept short and on one line.
fn clean(text: &str) -> String {
    let t: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
    t.trim().to_owned()
}

impl Event {
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line == "HOST" {
            return Some(Self::Host);
        }
        if let Some(text) = line.strip_prefix("FAIL") {
            return Some(Self::Fail(clean(text)));
        }
        let (seq, rest) = line.split_once(' ')?;
        let seq: u64 = seq.parse().ok()?;
        let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        let num = || arg.trim().parse::<u64>().ok();
        Some(match verb {
            "READY" => Self::Ready { seq, duration_ms: num().filter(|d| *d > 0) },
            "PLAY" => Self::Play { seq },
            "PAUSE" => Self::Pause { seq },
            "PROGRESS" => Self::Progress { seq, ms: num()? },
            "FINISH" => Self::Finish { seq },
            "ERROR" => Self::Error { seq, text: clean(arg) },
            _ => return None,
        })
    }

    pub fn line(&self) -> String {
        match self {
            Self::Host => "HOST".into(),
            Self::Ready { seq, duration_ms } => format!("{seq} READY {}", duration_ms.unwrap_or(0)),
            Self::Play { seq } => format!("{seq} PLAY"),
            Self::Pause { seq } => format!("{seq} PAUSE"),
            Self::Progress { seq, ms } => format!("{seq} PROGRESS {ms}"),
            Self::Finish { seq } => format!("{seq} FINISH"),
            Self::Error { seq, text } => format!("{seq} ERROR {}", clean(text)),
            Self::Fail(text) => format!("FAIL {}", clean(text)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_and_events_go_both_ways() {
        for c in [
            Command::Load { seq: 3, track: 1_234_567 },
            Command::Play { seq: 3 },
            Command::Pause { seq: 3 },
            Command::Seek { seq: 3, ms: 61_000 },
            Command::Volume(100),
        ] {
            assert_eq!(Command::parse(&c.line()), Some(c));
        }
        for e in [
            Event::Host,
            Event::Ready { seq: 2, duration_ms: Some(215_500) },
            Event::Ready { seq: 2, duration_ms: None },
            Event::Play { seq: 2 },
            Event::Pause { seq: 2 },
            Event::Progress { seq: 2, ms: 260 },
            Event::Finish { seq: 2 },
            Event::Error { seq: 2, text: "widget".into() },
            Event::Fail("no WebView2".into()),
        ] {
            assert_eq!(Event::parse(&e.line()), Some(e));
        }
        assert_eq!(Command::parse("LOAD 1 x"), None);
        assert_eq!(Command::parse("LOAD 1 2;alert(1)"), None);
        assert_eq!(Event::parse("x PLAY"), None);
        assert_eq!(Command::parse("PLAY 4").unwrap().js(), "window.nk.play(4)");
    }
}
