//! Soloist's local WebSocket API, as documented by Spotify
//! (developer.spotify.com/documentation/soloist/reference/websocket-api):
//! JSON text frames, commands `{"type":"command","command":…}`, events
//! with a `type`. On connect the server sends `auth_state`, and
//! `playback_state` when someone is signed in. A control command answers
//! `command_result` (dispatched, not done: the change itself arrives as
//! `playback_changed`, `track_changed`, `position_sync`, …); a refused one
//! answers `error` to this client only. There is no track-ended event:
//! the end is read from the state (see [`crate::track`]).

use serde_json::{Value, json};

/// Where a playback position was, and how fast it moves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Anchor {
    pub position_ms: u64,
    /// Server wall clock, unix ms.
    pub timestamp_ms: u64,
    /// 0 while not advancing (paused, buffering).
    pub speed: f64,
}

impl Anchor {
    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            position_ms: v.get("position_ms")?.as_u64()?,
            timestamp_ms: v.get("timestamp_ms").and_then(Value::as_u64).unwrap_or(0),
            speed: v.get("speed").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 4.0),
        })
    }

    /// The position at wall-clock `now_ms`.
    pub fn at(&self, now_ms: u64) -> u64 {
        if self.timestamp_ms == 0 || self.speed <= 0.0 {
            return self.position_ms;
        }
        let elapsed = now_ms.saturating_sub(self.timestamp_ms) as f64;
        self.position_ms + (elapsed * self.speed) as u64
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Playing,
    Paused,
    Buffering,
}

impl Status {
    /// A status this player does not know is no status: the last one stands.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "playing" => Some(Self::Playing),
            "paused" => Some(Self::Paused),
            "buffering" => Some(Self::Buffering),
            "idle" | "stopped" => Some(Self::Idle),
            _ => None,
        }
    }
}

/// The item playing: its URI and length.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Item {
    /// `spotify:track:…`; empty when unknown.
    pub uri: String,
    pub duration_ms: Option<u64>,
}

impl Item {
    fn parse(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        Some(Self {
            uri: v.get("uri").and_then(Value::as_str).unwrap_or_default().to_owned(),
            duration_ms: v
                .pointer("/decorations/playback/duration_ms")
                .and_then(Value::as_u64)
                .filter(|d| *d > 0),
        })
    }

    /// The `item` field of an event: `None` when it is missing (the item
    /// before stands), `Some(None)` when it is null (nothing plays).
    fn field(event: &Value) -> Option<Option<Self>> {
        event.get("item").map(Self::parse)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Auth {
        logged_in: bool,
        active: bool,
        device_name: String,
    },
    State {
        /// `None`: missing or unknown, the status before stands.
        status: Option<Status>,
        /// `None`: missing, the item before stands.
        item: Option<Option<Item>>,
        position: Option<Anchor>,
        volume: Option<u8>,
        active: Option<bool>,
    },
    /// `None`: the item is missing, the one before stands.
    TrackChanged(Option<Option<Item>>),
    PlaybackChanged(Status),
    Volume(u8),
    Device {
        active: bool,
    },
    Position(Anchor),
    /// A control command was accepted (by name).
    Accepted(String),
    /// A command was refused; the text is Soloist's.
    Error(String),
    /// Any other event (queue, options, context).
    Other(String),
}

fn volume(v: &Value) -> Option<u8> {
    v.as_u64().map(|v| v.min(100) as u8)
}

impl Event {
    pub fn parse(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        let kind = v.get("type")?.as_str()?;
        let flag = |k: &str| v.get(k).and_then(Value::as_bool);
        Some(match kind {
            "auth_state" => Self::Auth {
                logged_in: flag("logged_in").unwrap_or(false),
                active: flag("is_active").unwrap_or(false),
                device_name: v.get("device_name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            },
            "playback_state" => Self::State {
                status: v.get("status").and_then(Value::as_str).and_then(Status::parse),
                item: Item::field(&v),
                position: v.get("position").and_then(Anchor::parse),
                volume: v.get("volume").and_then(volume),
                active: flag("is_active"),
            },
            "track_changed" => Self::TrackChanged(Item::field(&v)),
            "playback_changed" => match v.get("status").and_then(Value::as_str).and_then(Status::parse) {
                Some(status) => Self::PlaybackChanged(status),
                None => Self::Other(kind.to_owned()),
            },
            "volume_changed" => Self::Volume(v.get("volume").and_then(volume)?),
            "device_changed" => Self::Device {
                active: flag("is_active").unwrap_or(false),
            },
            "position_sync" => Self::Position(v.get("position").and_then(Anchor::parse)?),
            "command_result" => Self::Accepted(v.get("command").and_then(Value::as_str).unwrap_or_default().to_owned()),
            "error" => Self::Error(v.get("message").and_then(Value::as_str).unwrap_or_default().to_owned()),
            other => Self::Other(other.to_owned()),
        })
    }
}

/// The commands the player uses.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    GetAuthState,
    GetState,
    /// Plays a URI, or resumes with `None`.
    Play(Option<String>),
    Pause,
    Seek(u64),
    /// 0–100.
    SetVolume(u8),
    SetRepeatTrack(bool),
    SetRepeatContext(bool),
    SetShuffle(bool),
    Activate,
    /// Soloist stops being the account's active Connect device, so the
    /// official apps no longer hand playback to it by themselves.
    Deactivate,
}

impl Command {
    /// Soloist answers it with `command_result` or `error` (the two
    /// requests for state answer with the state itself).
    pub fn answered(&self) -> bool {
        !matches!(self, Self::GetAuthState | Self::GetState)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::GetAuthState => "get_auth_state",
            Self::GetState => "get_state",
            Self::Play(_) => "play",
            Self::Pause => "pause",
            Self::Seek(_) => "seek",
            Self::SetVolume(_) => "set_volume",
            Self::SetRepeatTrack(_) => "set_repeat_track",
            Self::SetRepeatContext(_) => "set_repeat_context",
            Self::SetShuffle(_) => "set_shuffle",
            Self::Activate => "activate",
            Self::Deactivate => "deactivate",
        }
    }

    pub fn to_json(&self) -> String {
        let mut v = json!({"type": "command", "command": self.name()});
        match self {
            Self::Play(Some(uri)) => v["uri"] = json!(uri),
            Self::Seek(ms) => v["position_ms"] = json!(ms),
            Self::SetVolume(n) => v["volume"] = json!((*n).min(100)),
            Self::SetRepeatTrack(on) | Self::SetRepeatContext(on) | Self::SetShuffle(on) => v["enabled"] = json!(on),
            _ => {}
        }
        v.to_string()
    }
}

/// `spotify:track:<id>` of a track id (base62).
pub fn track_uri(id: &str) -> String {
    if id.starts_with("spotify:") {
        id.to_owned()
    } else {
        format!("spotify:track:{id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_events_parse() {
        assert_eq!(
            Event::parse(r#"{"type":"auth_state","logged_in":true,"is_active":false,"device_name":"PC"}"#),
            Some(Event::Auth { logged_in: true, active: false, device_name: "PC".into() })
        );
        let state = r#"{"type":"playback_state","status":"playing","volume":52,"is_active":true,
            "position":{"position_ms":2300,"timestamp_ms":1000,"speed":1.0},
            "item":{"uri":"spotify:track:abc","entity_type":"track","decorations":{"playback":{"duration_ms":123000}}},
            "options":{"shuffle":false,"repeat":"off"}}"#;
        let Some(Event::State { status, item, position, volume, active }) = Event::parse(state) else { panic!() };
        assert_eq!(status, Some(Status::Playing));
        assert_eq!(item, Some(Some(Item { uri: "spotify:track:abc".into(), duration_ms: Some(123_000) })));
        assert_eq!(position.unwrap().at(1500), 2800);
        assert_eq!((volume, active), (Some(52), Some(true)));
        assert_eq!(
            Event::parse(r#"{"type":"position_sync","position":{"position_ms":45000,"timestamp_ms":1747654321000,"speed":0.0}}"#),
            Some(Event::Position(Anchor { position_ms: 45_000, timestamp_ms: 1_747_654_321_000, speed: 0.0 }))
        );
        assert_eq!(Event::parse(r#"{"type":"command_result","command":"pause"}"#), Some(Event::Accepted("pause".into())));
        assert_eq!(
            Event::parse(r#"{"type":"error","message":"command requires authentication"}"#),
            Some(Event::Error("command requires authentication".into()))
        );
        assert_eq!(Event::parse(r#"{"type":"queue_changed","previous":[],"upcoming":[]}"#), Some(Event::Other("queue_changed".into())));
        assert_eq!(Event::parse("not json"), None);
    }

    #[test]
    fn missing_fields_and_unknown_statuses_change_nothing() {
        let Some(Event::State { status, item, .. }) = Event::parse(r#"{"type":"playback_state","status":"seeking"}"#) else {
            panic!()
        };
        assert_eq!((status, item), (None, None), "unknown status, no item field");
        let Some(Event::State { status, item, .. }) = Event::parse(r#"{"type":"playback_state","status":"idle","item":null}"#)
        else {
            panic!()
        };
        assert_eq!((status, item), (Some(Status::Idle), Some(None)), "a null item is cleared");
        assert_eq!(Event::parse(r#"{"type":"track_changed"}"#), Some(Event::TrackChanged(None)));
        assert_eq!(
            Event::parse(r#"{"type":"playback_changed","status":"weird"}"#),
            Some(Event::Other("playback_changed".into()))
        );
        assert_eq!(
            Event::parse(r#"{"type":"playback_changed","status":"paused"}"#),
            Some(Event::PlaybackChanged(Status::Paused))
        );
        assert!(Command::Play(None).answered() && !Command::GetState.answered());
    }

    #[test]
    fn commands_are_documented_frames() {
        let v = |c: Command| serde_json::from_str::<Value>(&c.to_json()).unwrap();
        assert_eq!(v(Command::Play(Some("spotify:track:x".into()))), json!({"type":"command","command":"play","uri":"spotify:track:x"}));
        assert_eq!(v(Command::Play(None)), json!({"type":"command","command":"play"}));
        assert_eq!(v(Command::Seek(1500)), json!({"type":"command","command":"seek","position_ms":1500}));
        assert_eq!(v(Command::SetVolume(140)), json!({"type":"command","command":"set_volume","volume":100}));
        assert_eq!(v(Command::SetRepeatContext(false)), json!({"type":"command","command":"set_repeat_context","enabled":false}));
        assert_eq!(track_uri("4uLU6hMCjMI75M1A2tKUQC"), "spotify:track:4uLU6hMCjMI75M1A2tKUQC");
    }
}
