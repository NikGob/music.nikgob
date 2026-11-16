//! Playing on another device of the owner's, like Spotify Connect.
//!
//! Every instance has its own zone: its engine, queue and place in the
//! track. The window shows and drives one zone, its target: normally this
//! device's; when it is another device's, the window is a remote («пульт»)
//! and this device's own zone is parked (paused, queue kept). A transfer
//! moves what plays from one zone to another ([`ZoneSnapshot`]); an attach
//! only changes the target.
//!
//! Besides the view's part ([`RemoteTarget`], [`ParkedView`]) this holds
//! what goes between the devices (the snapshot, the zone's state as its
//! event stream says it, the commands a remote forwards) and between the
//! runtime and the mesh service that does the network part
//! ([`ConnectJob`], [`ConnectReport`]).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{
    ArtistId, ArtistLinks, Command, DuckView, ListKind, MeshKind, MixId, MixKey, PlaylistId, RadioSeed, ReleaseId, Repeat, Route,
    SleepChoice, Status, TrackId, TrackRow, ViewState,
};

/// The zone this window shows and drives when it is another device's.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTarget {
    pub node_id: String,
    pub name: String,
    pub kind: MeshKind,
    pub link: RemoteLink,
    /// The remote's volume as the remote reports it (0..1 perceptual) and its duck,
    /// mirrored into `ViewState.player` too; kept here for the popover's row.
    pub volume: f32,
    pub muted: bool,
    /// How the window came to drive it: what going back to this device
    /// does (a detach after an attach, the music brought back after a
    /// transfer).
    pub how: RemoteHow,
}

/// How a remote target was reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteHow {
    /// Made the target as it was (nothing moved): this device's own queue
    /// waits parked.
    #[default]
    Attached,
    /// What played here moved there (or an undo went back to it).
    Transferred,
}

/// How the remote's state reaches this window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemoteLink {
    #[default]
    Live,
    /// No state from the remote for ≥ 1.5 s; the player shows buffering, rate 0.
    Reconnecting,
    /// ≥ 3 s: the UI pauses its display and offers «Продолжить здесь»; the remote may still be playing.
    Lost,
}

/// This device's own zone while the target is remote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParkedView {
    pub current: Option<TrackRow>,
    pub position: Duration,
}

/// Repeat as the devices say it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneRepeat {
    #[default]
    Off,
    All,
    One,
}

impl From<Repeat> for ZoneRepeat {
    fn from(r: Repeat) -> Self {
        match r {
            Repeat::Off => Self::Off,
            Repeat::All => Self::All,
            Repeat::One => Self::One,
        }
    }
}

impl From<ZoneRepeat> for Repeat {
    fn from(r: ZoneRepeat) -> Self {
        match r {
            ZoneRepeat::Off => Self::Off,
            ZoneRepeat::All => Self::All,
            ZoneRepeat::One => Self::One,
        }
    }
}

/// What a zone plays, to be played by another (`GET /v1/player/snapshot`,
/// the body of `POST /v1/player/load`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneSnapshot {
    /// The queue's tracks in play order.
    pub tracks: Vec<TrackId>,
    /// The order to go back to when shuffle goes off, as places in `tracks`.
    pub original: Vec<usize>,
    /// The current track's place in `tracks`.
    pub index: Option<usize>,
    /// Where in the current track.
    pub position_ms: u64,
    pub paused: bool,
    /// The speed the current track plays at (each track's speed is the
    /// library's, which the devices share): shown, not applied.
    pub speed: f32,
    pub shuffle: bool,
    pub repeat: ZoneRepeat,
    /// What the queue was started from («альбома «…»»).
    pub context: Option<String>,
    /// The id of what it was started from (`release:<id>`, `playlist:<id>`,
    /// `artist:<id>`, a mix, a radio's seed).
    pub page: Option<String>,
    pub kind: Option<ListKind>,
    /// The radio, wave or endless mix the queue plays, as the player keeps
    /// it between runs: the receiving device goes on generating it.
    pub radio: Option<serde_json::Value>,
    /// The places (and tracks) of what «Автовоспроизведение» added.
    pub autoplay_at: Vec<(usize, TrackId)>,
    /// Tracks the device that made the snapshot plays from a file (it
    /// serves those of its library at `/v1/media/tracks/{id}/audio`).
    pub files: Vec<TrackId>,
}

impl ZoneSnapshot {
    /// The current track.
    pub fn current(&self) -> Option<TrackId> {
        self.index.and_then(|i| self.tracks.get(i)).copied()
    }

    /// At most `max` tracks around the current one (the rest of a very
    /// long queue is left out).
    pub fn window(self, max: usize) -> Self {
        let n = self.tracks.len();
        if n <= max || max == 0 {
            return self;
        }
        let at = self.index.unwrap_or(0).min(n - 1);
        // A quarter of the room for what played, the rest for what comes.
        let before = (max / 4).min(at);
        let start = at - before;
        let end = (start + max).min(n);
        let start = end.saturating_sub(max);
        self.keep_places(|i| (start..end).contains(&i)).0
    }

    /// Only the tracks `keep` says, the places renumbered; the ids left out
    /// (each once, in queue order). The current track left out: the next
    /// one kept is current from its start (else the one before it).
    pub fn keep(self, keep: impl Fn(TrackId) -> bool) -> (Self, Vec<TrackId>) {
        let tracks = self.tracks.clone();
        let (out, places) = self.keep_places(|i| keep(tracks[i]));
        let mut skipped: Vec<TrackId> = Vec::new();
        for (i, t) in tracks.iter().enumerate() {
            if !places.contains(&i) && !skipped.contains(t) {
                skipped.push(*t);
            }
        }
        (out, skipped)
    }

    /// [`Self::keep`] by places; also the places kept.
    fn keep_places(self, keep: impl Fn(usize) -> bool) -> (Self, Vec<usize>) {
        let kept: Vec<usize> = (0..self.tracks.len()).filter(|&i| keep(i)).collect();
        let mut map: Vec<Option<usize>> = vec![None; self.tracks.len()];
        for (new, &old) in kept.iter().enumerate() {
            map[old] = Some(new);
        }
        let at = |old: usize| map.get(old).copied().flatten();
        let (index, same) = match self.index {
            Some(i) => match at(i) {
                Some(j) => (Some(j), true),
                None => {
                    let after = (i + 1..self.tracks.len()).find_map(at);
                    let before = (0..i).rev().find_map(at);
                    (after.or(before), false)
                }
            },
            None => (None, true),
        };
        let original: Vec<usize> = self.original.iter().filter_map(|&o| at(o)).collect();
        let original = if original.len() == kept.len() { original } else { Vec::new() };
        let autoplay_at = self.autoplay_at.iter().filter_map(|&(p, t)| Some((at(p)?, t))).collect();
        let mut radio = self.radio.clone();
        if let Some(r) = radio.as_mut() {
            remap_radio_places(r, &at);
        }
        let tracks = kept.iter().map(|&i| self.tracks[i]).collect();
        let out = Self {
            tracks,
            original,
            index,
            position_ms: if same { self.position_ms } else { 0 },
            autoplay_at,
            radio,
            ..self
        };
        (out, kept)
    }
}

/// The field of a radio's save (the runtime's, as `session.json` keeps it)
/// with the queue places of the entries the radio added: `[[place, track],
/// …]`.
pub const RADIO_PLACES: &str = "added_at";

/// The places of a radio's save renumbered by `at` (old place → new); an
/// entry whose place was left out is the radio's no more.
fn remap_radio_places(radio: &mut serde_json::Value, at: impl Fn(usize) -> Option<usize>) {
    let Some(list) = radio.get_mut(RADIO_PLACES).and_then(serde_json::Value::as_array_mut) else { return };
    list.retain_mut(|pair| {
        let Some(old) = pair.get(0).and_then(serde_json::Value::as_u64).and_then(|p| usize::try_from(p).ok()) else { return false };
        match (at(old), pair.get_mut(0)) {
            (Some(new), Some(slot)) => {
                *slot = serde_json::Value::from(new);
                true
            }
            _ => false,
        }
    });
}

/// What `POST /v1/player/load` answers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneLoaded {
    /// What the zone held before (for an undo); none when its queue was
    /// empty.
    pub previous: Option<ZoneSnapshot>,
    /// Ids not loaded: unknown here, or with no file here nor at the
    /// device that sent them (a service's song).
    pub skipped: Vec<TrackId>,
    /// Tracks the queue has now.
    pub loaded: usize,
}

/// How a zone plays, as its event stream says it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
    Buffering,
}

impl ZoneStatus {
    pub fn of(s: Status) -> Self {
        match s {
            Status::Stopped => Self::Stopped,
            Status::Playing => Self::Playing,
            Status::Paused => Self::Paused,
            Status::Buffering => Self::Buffering,
        }
    }

    pub fn status(self) -> Status {
        match self {
            Self::Stopped => Status::Stopped,
            Self::Playing => Status::Playing,
            Self::Paused => Status::Paused,
            Self::Buffering => Status::Buffering,
        }
    }
}

/// A track of a zone's state: its id (a remote keeps the row of its own
/// library) and what to show when the remote's library lacks it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneTrack {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub liked: bool,
}

impl ZoneTrack {
    pub fn of(t: &TrackRow) -> Self {
        Self {
            id: t.id,
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_ms: t.duration.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            liked: t.liked,
        }
    }

    /// A row of what the remote said, for a track this library lacks.
    pub fn row(&self) -> TrackRow {
        TrackRow {
            id: self.id,
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            release_id: None,
            artists: ArtistLinks::single(self.artist.clone(), None),
            disc_no: 0,
            track_no: 0,
            duration: self.duration_ms.map(Duration::from_millis),
            quality: None,
            source: None,
            has_local: false,
            liked: self.liked,
            cover: None,
            clip: false,
            in_library: false,
        }
    }
}

/// One place of a zone's queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneEntry {
    pub entry: u64,
    pub track: ZoneTrack,
}

/// The radio a zone's queue plays.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneRadio {
    pub title: String,
    pub extending: bool,
    pub autoplay: bool,
}

/// A zone's player and queue (`GET /v1/events?zone=1`, `event: zone`): what
/// a remote mirrors. The position is `position_ms` at `server_time_ms` of
/// that device's clock, moving at `rate`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneState {
    pub status: ZoneStatus,
    pub position_ms: u64,
    pub server_time_ms: i64,
    /// 0 unless playing; else the track's speed or a little off it.
    pub rate: f32,
    pub speed: f32,
    pub duration_ms: Option<u64>,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: ZoneRepeat,
    pub context: Option<String>,
    /// The page of what the queue was started from (see [`page_key`]).
    pub page: Option<String>,
    pub current: Option<ZoneTrack>,
    pub current_entry: Option<u64>,
    pub prev: Vec<ZoneEntry>,
    pub next: Vec<ZoneEntry>,
    pub radio: Option<ZoneRadio>,
    pub duck: Option<DuckView>,
}

impl ZoneState {
    /// The zone of `st` at `now_ms` (unix ms of this device), the position
    /// moved on to then.
    pub fn of(st: &ViewState, now_ms: i64) -> Self {
        let p = &st.player;
        let q = &st.queue;
        let entry = |e: &crate::QueueEntry| ZoneEntry { entry: e.entry, track: ZoneTrack::of(&e.track) };
        let playing = p.status == Status::Playing;
        Self {
            status: ZoneStatus::of(p.status),
            position_ms: u64::try_from(p.position_now().as_millis()).unwrap_or(u64::MAX),
            server_time_ms: now_ms,
            rate: if playing { p.rate.max(0.0) } else { 0.0 },
            speed: p.speed,
            duration_ms: p.duration.or(p.current.as_ref().and_then(|c| c.duration)).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            volume: p.volume.clamp(0.0, 1.0),
            muted: p.muted,
            shuffle: p.shuffle,
            repeat: p.repeat.into(),
            context: p.context.clone(),
            page: p.context_page.as_ref().and_then(page_key),
            current: p.current.as_ref().map(ZoneTrack::of),
            current_entry: q.current_entry,
            prev: q.prev.iter().map(entry).collect(),
            next: q.next.iter().map(entry).collect(),
            radio: q.radio.as_ref().map(|r| ZoneRadio { title: r.title.clone(), extending: r.extending, autoplay: r.autoplay }),
            duck: p.duck,
        }
    }

    /// What tells one state from another for the event stream: everything
    /// but the clock.
    pub fn change_key(&self) -> String {
        let mut s = self.clone();
        s.position_ms = 0;
        s.server_time_ms = 0;
        serde_json::to_string(&s).unwrap_or_default()
    }

    /// What tells one queue from another (its tracks and places, the
    /// modes, what it was started from): a change of it is a new snapshot
    /// to keep.
    pub fn queue_key(&self) -> String {
        let entries: Vec<u64> = self.prev.iter().chain(&self.next).map(|e| e.entry).collect();
        let radio = self.radio.as_ref().map(|r| r.title.as_str());
        serde_json::to_string(&(self.track_ids(), entries, self.current_entry, self.shuffle, self.repeat, &self.context, &self.page, radio)).unwrap_or_default()
    }

    /// Every track id of the state: the current one, what played, what comes.
    pub fn track_ids(&self) -> Vec<TrackId> {
        self.current.iter().map(|c| c.id).chain(self.prev.iter().map(|e| e.track.id)).chain(self.next.iter().map(|e| e.track.id)).collect()
    }

    /// The state as a snapshot to play elsewhere from `position_ms` of its
    /// current track, playing (what was played and what comes, as far as
    /// the state lists them).
    pub fn snapshot(&self, position_ms: u64) -> ZoneSnapshot {
        let mut tracks: Vec<TrackId> = self.prev.iter().map(|e| e.track.id).collect();
        let index = self.current.as_ref().map(|c| {
            tracks.push(c.id);
            tracks.len() - 1
        });
        tracks.extend(self.next.iter().map(|e| e.track.id));
        ZoneSnapshot {
            tracks,
            index,
            position_ms,
            paused: false,
            speed: self.speed,
            shuffle: self.shuffle,
            repeat: self.repeat,
            context: self.context.clone(),
            page: self.page.clone(),
            ..ZoneSnapshot::default()
        }
    }
}

/// The id of a queue's page (`release:<id>`, `playlist:<id>`, `artist:<id>`).
pub fn page_key(r: &Route) -> Option<String> {
    match r {
        Route::Release(id) => Some(format!("release:{id}")),
        Route::Playlist(id) => Some(format!("playlist:{id}")),
        Route::Artist(id) => Some(format!("artist:{id}")),
        _ => None,
    }
}

/// The page of a queue's page id (see [`page_key`]); none for a mix, a
/// radio, a search.
pub fn page_route(key: &str) -> Option<Route> {
    let (kind, id) = key.split_once(':')?;
    match kind {
        "release" => id.parse().ok().map(Route::Release),
        "playlist" => id.parse().ok().map(Route::Playlist),
        "artist" => id.parse().ok().map(Route::Artist),
        _ => None,
    }
}

/// What a radio grows from, of the library (a service's seed stays with
/// the device that has the service).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum RemoteSeed {
    Track(TrackId),
    Artist(ArtistId),
    Album(ReleaseId),
    Playlist(PlaylistId),
}

/// «Таймер сна» as a remote sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "minutes", rename_all = "snake_case")]
pub enum RemoteSleep {
    Minutes(u16),
    TrackEnd,
    ContextEnd,
    Off,
}

/// A player or queue command a remote forwards to its target
/// (`POST /v1/player/remote`): what the window's buttons, keys and pages
/// send, of the library. Library edits (likes, playlists) stay where they
/// are made: the devices' sync carries them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum RemoteCommand {
    TogglePlay,
    Play,
    Pause,
    Next,
    Previous,
    Seek { ms: u64 },
    SeekBy { ms: i64 },
    SetVolume { volume: f32 },
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    Sleep { sleep: RemoteSleep },
    /// The next list started plays shuffled or in order as this says.
    Shuffle { on: bool },
    PlayRelease { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    PlayReleaseWhole { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    PlayReleaseMine { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    PlayPlaylist { id: PlaylistId, start: Option<TrackId>, shuffle: bool },
    PlayArtist { id: ArtistId, shuffle: bool },
    PlayTracks { tracks: Vec<TrackId>, start: usize, context: String },
    PlayAll { tracks: Vec<TrackId>, context: String },
    PlayNext { tracks: Vec<TrackId> },
    Enqueue { tracks: Vec<TrackId> },
    PlayReleaseNext { id: ReleaseId },
    EnqueueRelease { id: ReleaseId },
    PlayPlaylistNext { id: PlaylistId },
    EnqueuePlaylist { id: PlaylistId },
    PlayArtistNext { id: ArtistId },
    EnqueueArtist { id: ArtistId },
    PlayQueueEntry { entry: u64 },
    RemoveFromQueue { entry: u64 },
    MoveInQueue { entry: u64, anchor: u64, after: bool },
    ClearQueue,
    UndoClearQueue,
    PlayMix { id: MixId },
    PlayMixNext { id: MixId },
    EnqueueMix { id: MixId },
    StartRadio { seed: RemoteSeed },
    PlayWave { preset: Option<MixKey> },
    PlayBlend { profiles: Vec<u32> },
    OtherVibe,
}

impl RemoteCommand {
    /// The forwarded form of `c`, when it is one a remote sends to its
    /// target.
    pub fn of(c: &Command) -> Option<Self> {
        let ms = |d: &Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        Some(match c {
            Command::TogglePlay => Self::TogglePlay,
            Command::Play => Self::Play,
            Command::Pause => Self::Pause,
            Command::Next => Self::Next,
            Command::Previous => Self::Previous,
            Command::Seek(d) => Self::Seek { ms: ms(d) },
            Command::SeekBy(by) => Self::SeekBy { ms: *by },
            Command::SetVolume(v) => Self::SetVolume { volume: *v },
            Command::ToggleMute => Self::ToggleMute,
            Command::ToggleShuffle => Self::ToggleShuffle,
            Command::CycleRepeat => Self::CycleRepeat,
            Command::SleepTimer(choice) => Self::Sleep {
                sleep: match *choice {
                    SleepChoice::Minutes(m) => RemoteSleep::Minutes(m),
                    SleepChoice::TrackEnd => RemoteSleep::TrackEnd,
                    SleepChoice::ContextEnd => RemoteSleep::ContextEnd,
                    SleepChoice::Off => RemoteSleep::Off,
                },
            },
            Command::ApiShuffle(on) => Self::Shuffle { on: *on },
            Command::PlayRelease { id, start, shuffle } => Self::PlayRelease { id: *id, start: *start, shuffle: *shuffle },
            Command::PlayReleaseWhole { id, start, shuffle } => Self::PlayReleaseWhole { id: *id, start: *start, shuffle: *shuffle },
            Command::PlayReleaseMine { id, start, shuffle } => Self::PlayReleaseMine { id: *id, start: *start, shuffle: *shuffle },
            Command::PlayPlaylist { id, start, shuffle } => Self::PlayPlaylist { id: *id, start: *start, shuffle: *shuffle },
            Command::PlayArtist { id, shuffle } => Self::PlayArtist { id: *id, shuffle: *shuffle },
            Command::PlayTracks { tracks, start, context } => Self::PlayTracks { tracks: tracks.clone(), start: *start, context: context.clone() },
            Command::PlayAll { tracks, context } => Self::PlayAll { tracks: tracks.clone(), context: context.clone() },
            Command::PlayNext(tracks) => Self::PlayNext { tracks: tracks.clone() },
            Command::Enqueue(tracks) => Self::Enqueue { tracks: tracks.clone() },
            Command::PlayReleaseNext(id) => Self::PlayReleaseNext { id: *id },
            Command::EnqueueRelease(id) => Self::EnqueueRelease { id: *id },
            Command::PlayPlaylistNext(id) => Self::PlayPlaylistNext { id: *id },
            Command::EnqueuePlaylist(id) => Self::EnqueuePlaylist { id: *id },
            Command::PlayArtistNext(id) => Self::PlayArtistNext { id: *id },
            Command::EnqueueArtist(id) => Self::EnqueueArtist { id: *id },
            Command::PlayQueueEntry(entry) => Self::PlayQueueEntry { entry: *entry },
            Command::RemoveFromQueue(entry) => Self::RemoveFromQueue { entry: *entry },
            Command::MoveInQueue { entry, anchor, after } => Self::MoveInQueue { entry: *entry, anchor: *anchor, after: *after },
            Command::ClearQueue => Self::ClearQueue,
            Command::UndoClearQueue => Self::UndoClearQueue,
            Command::PlayMix { id } => Self::PlayMix { id: *id },
            Command::PlayMixNext { id } => Self::PlayMixNext { id: *id },
            Command::EnqueueMix { id } => Self::EnqueueMix { id: *id },
            Command::StartRadio { seed } => Self::StartRadio {
                seed: match seed {
                    RadioSeed::Track(t) => RemoteSeed::Track(*t),
                    RadioSeed::Artist(a) => RemoteSeed::Artist(*a),
                    RadioSeed::Album(r) => RemoteSeed::Album(*r),
                    RadioSeed::Playlist(p) => RemoteSeed::Playlist(*p),
                    RadioSeed::Stream(_) | RadioSeed::StreamAlbum(_) | RadioSeed::StreamPlaylist(_) => return None,
                },
            },
            Command::PlayWave { preset } => Self::PlayWave { preset: *preset },
            Command::PlayBlend { profiles } => Self::PlayBlend { profiles: profiles.clone() },
            Command::OtherVibe => Self::OtherVibe,
            _ => return None,
        })
    }

    /// The command the receiving device runs.
    pub fn command(self) -> Command {
        match self {
            Self::TogglePlay => Command::TogglePlay,
            Self::Play => Command::Play,
            Self::Pause => Command::Pause,
            Self::Next => Command::Next,
            Self::Previous => Command::Previous,
            Self::Seek { ms } => Command::Seek(Duration::from_millis(ms)),
            Self::SeekBy { ms } => Command::SeekBy(ms),
            Self::SetVolume { volume } => Command::SetVolume(if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 0.0 }),
            Self::ToggleMute => Command::ToggleMute,
            Self::ToggleShuffle => Command::ToggleShuffle,
            Self::CycleRepeat => Command::CycleRepeat,
            Self::Sleep { sleep } => Command::SleepTimer(match sleep {
                RemoteSleep::Minutes(m) => SleepChoice::Minutes(m),
                RemoteSleep::TrackEnd => SleepChoice::TrackEnd,
                RemoteSleep::ContextEnd => SleepChoice::ContextEnd,
                RemoteSleep::Off => SleepChoice::Off,
            }),
            Self::Shuffle { on } => Command::ApiShuffle(on),
            Self::PlayRelease { id, start, shuffle } => Command::PlayRelease { id, start, shuffle },
            Self::PlayReleaseWhole { id, start, shuffle } => Command::PlayReleaseWhole { id, start, shuffle },
            Self::PlayReleaseMine { id, start, shuffle } => Command::PlayReleaseMine { id, start, shuffle },
            Self::PlayPlaylist { id, start, shuffle } => Command::PlayPlaylist { id, start, shuffle },
            Self::PlayArtist { id, shuffle } => Command::PlayArtist { id, shuffle },
            Self::PlayTracks { tracks, start, context } => Command::PlayTracks { tracks, start, context },
            Self::PlayAll { tracks, context } => Command::PlayAll { tracks, context },
            Self::PlayNext { tracks } => Command::PlayNext(tracks),
            Self::Enqueue { tracks } => Command::Enqueue(tracks),
            Self::PlayReleaseNext { id } => Command::PlayReleaseNext(id),
            Self::EnqueueRelease { id } => Command::EnqueueRelease(id),
            Self::PlayPlaylistNext { id } => Command::PlayPlaylistNext(id),
            Self::EnqueuePlaylist { id } => Command::EnqueuePlaylist(id),
            Self::PlayArtistNext { id } => Command::PlayArtistNext(id),
            Self::EnqueueArtist { id } => Command::EnqueueArtist(id),
            Self::PlayQueueEntry { entry } => Command::PlayQueueEntry(entry),
            Self::RemoveFromQueue { entry } => Command::RemoveFromQueue(entry),
            Self::MoveInQueue { entry, anchor, after } => Command::MoveInQueue { entry, anchor, after },
            Self::ClearQueue => Command::ClearQueue,
            Self::UndoClearQueue => Command::UndoClearQueue,
            Self::PlayMix { id } => Command::PlayMix { id },
            Self::PlayMixNext { id } => Command::PlayMixNext { id },
            Self::EnqueueMix { id } => Command::EnqueueMix { id },
            Self::StartRadio { seed } => Command::StartRadio {
                seed: match seed {
                    RemoteSeed::Track(t) => RadioSeed::Track(t),
                    RemoteSeed::Artist(a) => RadioSeed::Artist(a),
                    RemoteSeed::Album(r) => RadioSeed::Album(r),
                    RemoteSeed::Playlist(p) => RadioSeed::Playlist(p),
                },
            },
            Self::PlayWave { preset } => Command::PlayWave { preset },
            Self::PlayBlend { profiles } => Command::PlayBlend { profiles },
            Self::OtherVibe => Command::OtherVibe,
        }
    }
}

/// A command that starts a service's music (it plays only where the
/// service is: a remote does not forward it).
pub fn starts_service_music(c: &Command) -> bool {
    matches!(
        c,
        Command::PlayStreamTracks { .. }
            | Command::PlaySearch { .. }
            | Command::PlayStreamAlbum { .. }
            | Command::PlayStreamPlaylist { .. }
            | Command::PlayStreamNext(_)
            | Command::EnqueueStream(_)
            | Command::PlayStreamAlbumNext(_)
            | Command::EnqueueStreamAlbum(_)
            | Command::PlayStreamPlaylistNext(_)
            | Command::EnqueueStreamPlaylist(_)
            | Command::PlayStreamArtist(_)
            | Command::PlayStreamArtistNext(_)
            | Command::EnqueueStreamArtist(_)
            | Command::PlayFromYoutube
            | Command::StartRadio { seed: RadioSeed::Stream(_) | RadioSeed::StreamAlbum(_) | RadioSeed::StreamPlaylist(_) }
    )
}

/// Work the runtime gives the mesh service (the network part of playing
/// elsewhere); it answers with [`ConnectReport`]s. Everything but `Session`
/// and `Pull` runs one at a time, in order; work of a session that has
/// ended is dropped before it touches the network.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectJob {
    /// The target session `sess` begins: follow `node`'s zone (its event
    /// stream and its snapshot), or none (the target is this device). Every
    /// earlier session has ended.
    Session { sess: u64, follow: Option<String> },
    /// Run these on `node`'s zone, in order, while session `sess` lasts.
    Run { sess: u64, node: String, commands: Vec<RemoteCommand> },
    /// Pause `node`'s zone if it can be done while session `sess` lasts and
    /// before `until` (best effort).
    Pause { sess: u64, node: String, until: Instant },
    /// Move a zone's music to another: from `from`'s zone (None: this
    /// device's) to `to`'s (None: here). The source is paused first (its
    /// answer awaited: none, nothing moves); a load not known to be taken
    /// leaves the source paused.
    Move { seq: u64, sess: u64, from: Option<String>, to: Option<String> },
    /// The undo of a transfer: `snapshot` back into `node`'s zone.
    Undo { seq: u64, sess: u64, node: String, snapshot: Box<ZoneSnapshot> },
    /// «Продолжить здесь»: `snapshot` into this device's zone, only what
    /// this device can play.
    Here { seq: u64, sess: u64, snapshot: Box<ZoneSnapshot> },
    /// `track`'s file from device `from` into this device's cache (or the
    /// copy kept there), for the zone's load `load`; `keep` are cached
    /// files in use (playing, next) that the cache must not drop.
    Pull { track: TrackId, from: String, load: u64, keep: Vec<PathBuf> },
}

/// Why a transfer, an undo or a «Продолжить здесь» did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectFail {
    /// Not done: its session had ended before it ran.
    Stale,
    /// Nothing changed (the source did not pause, a snapshot did not come).
    NothingDone(String),
    /// The source is paused, and the destination does not play what was
    /// sent (or did not answer whether it does).
    NotLoaded(String),
}

/// What the mesh service reports of a [`ConnectJob`]
/// ([`Command::ConnectReport`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectReport {
    /// `node`'s zone as its last event says it, in session `sess`; `at` is
    /// when that was (the event's arrival less half the round trip).
    State { sess: u64, node: String, state: Box<ZoneState>, at: Instant },
    /// How `node` answers (session `sess`): silent a while, or again.
    Link { sess: u64, node: String, link: RemoteLink },
    /// `node`'s whole transferable state, fetched when its queue changed.
    Snapshot { sess: u64, node: String, snapshot: Box<ZoneSnapshot> },
    Moved { seq: u64, to: Option<String>, result: Result<ZoneLoaded, ConnectFail> },
    Undone { seq: u64, node: String, result: Result<ZoneLoaded, ConnectFail> },
    Here { seq: u64, result: Result<ZoneLoaded, ConnectFail> },
    Pulled { track: TrackId, from: String, load: u64, result: Result<PathBuf, String> },
    /// Commands for `node` (session `sess`) did not get there.
    RunFailed { sess: u64, node: String, why: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The radio's own entries keep their places when tracks before them
    /// are left out, and lose them with their track.
    #[test]
    fn a_radios_places_follow_the_kept_tracks() {
        let t: Vec<TrackId> = (0..5).map(|_| TrackId::new()).collect();
        let radio = serde_json::json!({ "title": "Радио", RADIO_PLACES: [[1, t[1]], [3, t[3]], [4, t[4]]] });
        let s = ZoneSnapshot { tracks: t.clone(), index: Some(0), radio: Some(radio), ..ZoneSnapshot::default() };
        let (kept, skipped) = s.keep(|id| id != t[1] && id != t[2]);
        assert_eq!(skipped, vec![t[1], t[2]]);
        let places = kept.radio.as_ref().unwrap()[RADIO_PLACES].clone();
        assert_eq!(places, serde_json::json!([[1, t[3]], [2, t[4]]]));
        assert_eq!(kept.radio.as_ref().unwrap()["title"], "Радио");
    }

    fn ids(n: usize) -> Vec<TrackId> {
        (0..n).map(|_| TrackId::new()).collect()
    }

    fn snap(tracks: &[TrackId], index: usize) -> ZoneSnapshot {
        ZoneSnapshot {
            tracks: tracks.to_vec(),
            original: (0..tracks.len()).rev().collect(),
            index: Some(index),
            position_ms: 42_000,
            autoplay_at: vec![(3, tracks[3])],
            ..ZoneSnapshot::default()
        }
    }

    /// Tracks left out are listed once, the places renumbered; the current
    /// one kept keeps its place in the track.
    #[test]
    fn keeping_some_tracks_renumbers_the_places() {
        let t = ids(5);
        let gone = [t[1], t[4]];
        let (s, skipped) = snap(&t, 2).keep(|x| !gone.contains(&x));
        assert_eq!(s.tracks, vec![t[0], t[2], t[3]]);
        assert_eq!(skipped, vec![t[1], t[4]]);
        assert_eq!((s.index, s.position_ms), (Some(1), 42_000));
        assert_eq!(s.original, vec![2, 1, 0], "the shuffle's way back, renumbered");
        assert_eq!(s.autoplay_at, vec![(2, t[3])]);
        assert_eq!(s.current(), Some(t[2]));
    }

    /// The current track left out: the next one kept starts from its top,
    /// else the one before it.
    #[test]
    fn a_current_track_left_out_gives_its_place_to_the_next() {
        let t = ids(5);
        let (s, _) = snap(&t, 2).keep(|x| x != t[2]);
        assert_eq!((s.current(), s.position_ms), (Some(t[3]), 0));
        let (s, _) = snap(&t, 4).keep(|x| x != t[4] && x != t[3]);
        assert_eq!(s.current(), Some(t[2]));
        let (s, skipped) = snap(&t, 0).keep(|_| false);
        assert_eq!((s.tracks.len(), s.index), (0, None));
        assert_eq!(skipped.len(), 5);
    }

    #[test]
    fn a_long_queue_keeps_a_window_around_the_current_track() {
        let t = ids(100);
        let s = snap(&t, 50).window(20);
        assert_eq!(s.tracks.len(), 20);
        assert_eq!(s.current(), Some(t[50]));
        assert_eq!(s.tracks[0], t[45], "a quarter before");
        let s = snap(&t, 98).window(20);
        assert_eq!((s.tracks.len(), s.current()), (20, Some(t[98])));
        assert_eq!(snap(&t[..10], 3).window(20).tracks.len(), 10);
    }

    #[test]
    fn a_snapshot_survives_json() {
        let t = ids(4);
        let s = ZoneSnapshot { repeat: ZoneRepeat::All, radio: Some(serde_json::json!({"title": "Радио"})), ..snap(&t, 1) };
        let back: ZoneSnapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let partial: ZoneSnapshot = serde_json::from_str(&format!(r#"{{"tracks":["{}"],"index":0}}"#, t[0])).unwrap();
        assert_eq!((partial.current(), partial.paused, partial.repeat), (Some(t[0]), false, ZoneRepeat::Off));
    }

    /// Every forwarded command goes over JSON and comes back the same; a
    /// service's music and library edits are not forwarded.
    #[test]
    fn forwarded_commands_round_trip() {
        let t = ids(2);
        let commands = vec![
            Command::TogglePlay,
            Command::Seek(Duration::from_millis(61_500)),
            Command::SeekBy(-10_000),
            Command::SetVolume(0.25),
            Command::SleepTimer(SleepChoice::Minutes(30)),
            Command::SleepTimer(SleepChoice::TrackEnd),
            Command::ApiShuffle(true),
            Command::PlayRelease { id: ReleaseId::new(), start: Some(t[0]), shuffle: false },
            Command::PlayTracks { tracks: t.clone(), start: 1, context: "библиотеки".into() },
            Command::Enqueue(t.clone()),
            Command::MoveInQueue { entry: 4, anchor: 9, after: false },
            Command::PlayMix { id: MixId::Daily },
            Command::PlayWave { preset: None },
            Command::StartRadio { seed: RadioSeed::Track(t[1]) },
            Command::ClearQueue,
        ];
        for c in commands {
            let r = RemoteCommand::of(&c).unwrap_or_else(|| panic!("{c:?} is forwarded"));
            let back: RemoteCommand = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
            assert_eq!(back.command(), c);
        }
        assert_eq!(RemoteCommand::of(&Command::Like(crate::LikeTarget::Track(t[0]), true)), None);
        let stream = crate::RemoteId::new(crate::Service::Youtube, "x");
        assert_eq!(RemoteCommand::of(&Command::StartRadio { seed: RadioSeed::Stream(stream.clone()) }), None);
        assert!(starts_service_music(&Command::PlayStreamNext(vec![stream])));
        assert!(!starts_service_music(&Command::PlayNext(t)));
        let v: serde_json::Value = serde_json::to_value(RemoteCommand::Seek { ms: 5 }).unwrap();
        assert_eq!(v, serde_json::json!({"do": "seek", "ms": 5}));
    }

    #[test]
    fn page_ids_round_trip() {
        let r = Route::Release(ReleaseId::new());
        assert_eq!(page_route(&page_key(&r).unwrap()), Some(r));
        assert_eq!(page_key(&Route::Home), None);
        assert_eq!(page_route("mix:daily"), None);
    }

    /// The state a remote mirrors: the queue's tracks by id, the clock
    /// apart from what tells a change.
    #[test]
    fn a_zone_state_has_its_queue_and_a_key_without_the_clock() {
        let mut st = ViewState::default();
        let row = ZoneTrack { id: TrackId::new(), title: "T".into(), artist: "A".into(), album: None, duration_ms: Some(1000), liked: false }.row();
        st.player.status = Status::Playing;
        st.player.current = Some(row.clone());
        st.player.rate = 1.0;
        st.queue.current_entry = Some(3);
        st.queue.next = vec![crate::QueueEntry { entry: 4, track: row.clone(), why: None, guest: false }];
        let a = ZoneState::of(&st, 1_000);
        let b = ZoneState::of(&st, 5_000);
        assert_eq!(a.change_key(), b.change_key());
        assert_eq!(a.track_ids(), vec![row.id, row.id]);
        assert_eq!(a.status, ZoneStatus::Playing);
        let back: ZoneState = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        let s = a.snapshot(500);
        assert_eq!((s.index, s.tracks.len(), s.position_ms, s.paused), (Some(0), 2, 500, false));
    }
}
