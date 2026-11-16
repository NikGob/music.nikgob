//! Playing on another device of the owner's, like Spotify Connect.
//!
//! Every instance has its own zone: its engine, queue and place in the
//! track. The window shows and drives one zone, its target: normally this
//! device's; when it is another device's, the window is a remote («пульт»)
//! and this device's own zone is parked (paused, queue kept).

use std::time::Duration;

use crate::{MeshKind, TrackRow};

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
