//! «Что играет» as the API says it (`GET /v1/state`, the event stream):
//! the view's player, its next track, the output and the duck, with the
//! position as of a moment of the server's clock.

use serde::Serialize;
use utoipa::ToSchema;

use nikgob_view::{Repeat, Status, TrackRow, ViewState};

use crate::outputs;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlayStatus {
    Playing,
    Paused,
    Buffering,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn of(r: Repeat) -> Self {
        match r {
            Repeat::Off => Self::Off,
            Repeat::All => Self::All,
            Repeat::One => Self::One,
        }
    }

    /// Its place in the button's cycle (off → all → one).
    pub fn step(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::All => 1,
            Self::One => 2,
        }
    }
}

/// The track playing.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct NowTrack {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    /// The track's own length (not its speed's).
    pub duration_ms: Option<i64>,
    pub liked: bool,
}

/// What plays next.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct NextTrack {
    pub title: String,
    pub artists: Vec<String>,
}

/// Where the sound goes.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct OutputRef {
    /// `local:default`, `local:<id>`, `node:<id>` (`GET /v1/outputs`).
    pub id: String,
    pub name: String,
}

/// The sound lowered for a while (`PUT /v1/player/duck`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, ToSchema)]
pub struct DuckState {
    /// A factor on the volume, 0..1.
    pub gain: f32,
    /// When it ends by itself (server clock, unix ms).
    pub until_ms: i64,
    pub remaining_ms: i64,
}

/// `GET /v1/state`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PlayerState {
    pub status: PlayStatus,
    /// `null` when stopped.
    pub track: Option<NowTrack>,
    /// `null` when stopped or nothing follows.
    pub next: Option<NextTrack>,
    /// Inside the track, as of `at_ms`.
    pub position_ms: i64,
    /// The server's clock (unix ms) `position_ms` is of.
    pub at_ms: i64,
    /// The server's clock when the answer was made.
    pub server_time_ms: i64,
    /// How fast the position moves: 0 paused, buffering or stopped; else the
    /// track's speed (1 as recorded).
    pub rate: f32,
    /// 0..1, the player's slider.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub output: OutputRef,
    pub duck: Option<DuckState>,
}

/// Every credited name of a track (its display line when it has none).
pub fn artists_of(t: &TrackRow) -> Vec<String> {
    let names: Vec<String> = t.artists.0.iter().map(|n| n.name.clone()).filter(|n| !n.trim().is_empty()).collect();
    if names.is_empty() && !t.artist.trim().is_empty() { vec![t.artist.clone()] } else { names }
}

fn ms(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// The duck as of `now_ms` (`None` once it ended).
pub fn duck_of(st: &ViewState, now_ms: i64) -> Option<DuckState> {
    let d = st.player.duck?;
    (d.until_unix_ms > now_ms).then(|| DuckState { gain: d.gain, until_ms: d.until_unix_ms, remaining_ms: d.until_unix_ms - now_ms })
}

impl PlayerState {
    /// The state of `st` at `now_ms` of the server's clock (the position
    /// moved on to then).
    pub fn of(st: &ViewState, now_ms: i64) -> Self {
        let p = &st.player;
        let status = match p.status {
            Status::Playing => PlayStatus::Playing,
            Status::Paused => PlayStatus::Paused,
            Status::Buffering => PlayStatus::Buffering,
            Status::Stopped => PlayStatus::Stopped,
        };
        let stopped = status == PlayStatus::Stopped;
        let track = if stopped {
            None
        } else {
            p.current.as_ref().map(|t| NowTrack {
                id: t.id.to_string(),
                title: t.title.clone(),
                artists: artists_of(t),
                album: t.album.clone(),
                duration_ms: p.duration.or(t.duration).map(ms),
                liked: t.liked,
            })
        };
        let next = if stopped {
            None
        } else {
            st.queue.next.first().map(|e| NextTrack { title: e.track.title.clone(), artists: artists_of(&e.track) })
        };
        let output = match (&st.target, st.prefs.output_device.as_deref()) {
            // The window drives another device of the home: the state is its.
            (Some(t), _) => OutputRef { id: format!("{}{}", outputs::NODE_PREFIX, t.node_id), name: t.name.clone() },
            (None, Some(name)) => OutputRef { id: outputs::local_id(name), name: name.to_owned() },
            (None, None) => OutputRef { id: outputs::DEFAULT_ID.to_owned(), name: outputs::DEFAULT_NAME.to_owned() },
        };
        Self {
            status,
            track,
            next,
            position_ms: if stopped { 0 } else { ms(p.position_now()) },
            at_ms: now_ms,
            server_time_ms: now_ms,
            rate: if status == PlayStatus::Playing { p.rate.max(0.0) } else { 0.0 },
            volume: p.volume.clamp(0.0, 1.0),
            muted: p.muted,
            shuffle: p.shuffle,
            repeat: RepeatMode::of(p.repeat),
            output,
            duck: duck_of(st, now_ms),
        }
    }

    /// What tells one state from another for the event stream: everything
    /// but the clock (the position moving on, the server's time, the
    /// duck's time left).
    pub fn change_key(&self) -> String {
        let mut s = self.clone();
        s.position_ms = 0;
        s.at_ms = 0;
        s.server_time_ms = 0;
        if let Some(d) = &mut s.duck {
            d.remaining_ms = 0;
        }
        serde_json::to_string(&s).unwrap_or_default()
    }
}

/// `{"state": …}`, the answer of the commands.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StateAnswer {
    pub state: PlayerState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn row(title: &str, artist: &str) -> TrackRow {
        TrackRow {
            id: nikgob_view::TrackId::new(),
            title: title.into(),
            artist: artist.into(),
            album: Some("In Rainbows".into()),
            release_id: None,
            artists: nikgob_view::ArtistLinks::single(artist, None),
            disc_no: 1,
            track_no: 4,
            duration: Some(Duration::from_millis(318_000)),
            quality: None,
            source: None,
            has_local: true,
            liked: true,
            cover: None,
            clip: false,
            in_library: true,
            temporary: false,
        }
    }

    fn fixture() -> ViewState {
        let mut st = ViewState::default();
        st.player.status = Status::Playing;
        st.player.current = Some(row("Weird Fishes/Arpeggi", "Radiohead"));
        st.player.position = Duration::from_millis(41_250);
        st.player.at = Instant::now();
        st.player.speed = 1.0;
        st.player.rate = 1.0;
        st.player.duration = Some(Duration::from_millis(318_000));
        st.player.volume = 0.62;
        st.player.duck = Some(nikgob_view::DuckView { gain: 0.18, until_unix_ms: 1_763_300_008_000 });
        st.queue.next = vec![nikgob_view::QueueEntry { entry: 7, track: row("All I Need", "Radiohead"), why: None, guest: false }];
        st
    }

    /// The field names (and their kinds) are the voice contract's, exactly.
    #[test]
    fn the_state_has_the_contract_shape() {
        let now = 1_763_300_000_420;
        // Read back from the wire text (an f32 goes out as its shortest form).
        let text = serde_json::to_string(&PlayerState::of(&fixture(), now)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let keys: std::collections::BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let want: std::collections::BTreeSet<&str> = [
            "status", "track", "next", "position_ms", "at_ms", "server_time_ms", "rate", "volume", "muted", "shuffle", "repeat", "output",
            "duck",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, want);
        let track: std::collections::BTreeSet<&str> = v["track"].as_object().unwrap().keys().map(String::as_str).collect();
        let want_track: std::collections::BTreeSet<&str> = ["id", "title", "artists", "album", "duration_ms", "liked"].into_iter().collect();
        assert_eq!(track, want_track);
        assert_eq!(v["status"], "playing");
        assert_eq!(v["track"]["title"], "Weird Fishes/Arpeggi");
        assert_eq!(v["track"]["artists"], serde_json::json!(["Radiohead"]));
        assert_eq!(v["track"]["duration_ms"], 318_000);
        assert_eq!(v["next"], serde_json::json!({"title": "All I Need", "artists": ["Radiohead"]}));
        assert_eq!(v["repeat"], "off");
        assert_eq!(v["output"], serde_json::json!({"id": "local:default", "name": "Default"}));
        assert_eq!(v["duck"], serde_json::json!({"gain": 0.18, "until_ms": 1_763_300_008_000_i64, "remaining_ms": 7580}));
        assert_eq!(v["volume"], serde_json::json!(0.62));
        assert_eq!(v["rate"], serde_json::json!(1.0));
        assert_eq!(v["at_ms"], now);
        assert!(v["position_ms"].as_i64().unwrap() >= 41_250);
    }

    #[test]
    fn stopped_has_no_track_and_no_rate() {
        let mut st = fixture();
        st.player.status = Status::Stopped;
        st.player.duck = None;
        let s = PlayerState::of(&st, 5);
        assert_eq!((s.track.clone(), s.next.clone(), s.rate, s.position_ms), (None, None, 0.0, 0));
        assert_eq!(s.duck, None);
        st.player.status = Status::Paused;
        assert_eq!(PlayerState::of(&st, 5).rate, 0.0);
        st.player.status = Status::Buffering;
        let b = PlayerState::of(&st, 5);
        assert_eq!((b.status, b.rate), (PlayStatus::Buffering, 0.0));
        // An ended duck is none.
        st.player.duck = Some(nikgob_view::DuckView { gain: 0.1, until_unix_ms: 4 });
        assert_eq!(PlayerState::of(&st, 5).duck, None);
    }

    /// The rate is the clock's, not the speed asked for: a stall while
    /// playing stands still.
    #[test]
    fn the_rate_is_the_clocks() {
        let mut st = fixture();
        st.player.speed = 1.2;
        st.player.rate = 1.19;
        assert!((PlayerState::of(&st, 5).rate - 1.19).abs() < 1e-6);
        st.player.rate = 0.0;
        assert_eq!(PlayerState::of(&st, 5).rate, 0.0, "a stall");
        st.player.rate = -1.0;
        assert_eq!(PlayerState::of(&st, 5).rate, 0.0);
    }

    #[test]
    fn the_key_ignores_the_clock() {
        let st = fixture();
        let a = PlayerState::of(&st, 1_763_300_000_000);
        let b = PlayerState::of(&st, 1_763_300_003_000);
        assert_eq!(a.change_key(), b.change_key());
        let mut paused = st.clone();
        paused.player.status = Status::Paused;
        assert_ne!(a.change_key(), PlayerState::of(&paused, 1_763_300_000_000).change_key());
    }
}
