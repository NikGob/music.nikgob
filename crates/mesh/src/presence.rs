//! What a device tells the others of itself while they are connected: its
//! name, kind and version, its library disk, and what it plays with the
//! clock of the position (a remote extrapolates the progress itself); how
//! its copies of the library come along; the ring holder also what each
//! device is to store, and any device whose copies the owner paused there.

use nikgob_view::{MeshCopy, MeshKind, MeshPlaying, MeshProfile, MeshScope, MeshStorage, MeshTransfer, PlaylistId, Status, TrackRow, ViewState};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub name: String,
    pub kind: MeshKind,
    pub version: String,
    pub storage: Option<MeshStorage>,
    /// What this device's replica plan gives it and how much of that it
    /// holds now (none: it keeps no copies, or has no plan yet).
    #[serde(default)]
    pub copy: Option<MeshCopy>,
    pub playing: Option<MeshPlaying>,
    /// This device pulling the library's files (none: nothing to pull).
    #[serde(default)]
    pub transfer: Option<MeshTransfer>,
    /// This device holds the ring: what it gives the devices to store is
    /// what they store.
    #[serde(default)]
    pub holder: bool,
    /// What each owner's device stores (the ring holder's choice).
    #[serde(default)]
    pub assign: Vec<Assigned>,
    /// The devices whose pulls the owner paused here.
    #[serde(default)]
    pub pause: Vec<String>,
}

/// What a device stores, as the ring holder chose it: its profile and, for
/// «Избранное», what it keeps pinned.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assigned {
    pub node: String,
    pub profile: MeshProfile,
    #[serde(default)]
    pub liked: bool,
    #[serde(default)]
    pub playlists: Vec<PlaylistId>,
}

/// A transfer's bytes that moved this far are told at once (the heartbeat
/// carries smaller steps).
pub const TRANSFER_STEP: u64 = 16 << 20;

/// A position that drifted further than this from where the last report
/// puts it is a change (a seek); closer, the clock says it.
pub const JUMP_MS: i64 = 1500;

/// Every credited name of a track (its display line when it has none).
fn artists_of(t: &TrackRow) -> Vec<String> {
    let mut artists: Vec<String> = t.artists.0.iter().map(|n| n.name.clone()).filter(|n| !n.trim().is_empty()).collect();
    if artists.is_empty() && !t.artist.trim().is_empty() {
        artists.push(t.artist.clone());
    }
    artists
}

fn ms(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// This device's presence from its view at `now_ms`. While its window
/// drives another device, the view's player is that device's: this one
/// tells of its own zone, parked (paused).
pub fn of_view(st: &ViewState, name: &str, kind: MeshKind, version: &str, now_ms: i64) -> Presence {
    let p = &st.player;
    let playing = match (p.status, p.current.as_ref()) {
        _ if st.target.is_some() => st.parked.as_ref().and_then(|z| {
            z.current.as_ref().map(|t| MeshPlaying {
                title: t.title.clone(),
                artists: artists_of(t),
                paused: true,
                position_ms: ms(z.position),
                at_ms: now_ms,
                rate: 0.0,
                duration_ms: t.duration.map(ms),
            })
        }),
        (Status::Stopped, _) | (_, None) => None,
        (status, Some(t)) => {
            let playing = status == Status::Playing;
            Some(MeshPlaying {
                title: t.title.clone(),
                artists: artists_of(t),
                paused: !playing,
                position_ms: ms(p.position_now()),
                at_ms: now_ms,
                rate: if playing { p.rate.max(0.0) } else { 0.0 },
                duration_ms: p.duration.or(t.duration).map(ms),
            })
        }
    };
    let s = &st.downloads.storage;
    let storage = s.known.then(|| MeshStorage { used: s.music(), free: s.disks.first().map_or(0, |d| d.free) });
    let holder = st.mesh.ring_holder;
    let assign = if holder {
        st.mesh
            .nodes
            .iter()
            .filter(|n| n.scope == MeshScope::Owner)
            .map(|n| Assigned { node: n.id.clone(), profile: n.profile, liked: n.pins.liked, playlists: n.pins.playlists.clone() })
            .collect()
    } else {
        Vec::new()
    };
    Presence { name: name.to_owned(), kind, version: version.to_owned(), storage, copy: None, playing, transfer: None, holder, assign, pause: Vec::new() }
}

/// Whether a transfer changed enough to tell at once: begun or ended, its
/// total, its pause, a stop or a start, or bytes past [`TRANSFER_STEP`]
/// (or a fiftieth of the whole).
pub fn transfer_differs(old: &Option<MeshTransfer>, new: &Option<MeshTransfer>) -> bool {
    match (old, new) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            a.total != b.total
                || a.paused != b.paused
                || (a.speed == 0) != (b.speed == 0)
                || a.done.abs_diff(b.done) >= (b.total / 50).max(TRANSFER_STEP)
        }
        _ => true,
    }
}

/// Where `p` puts the position at `now_ms`.
pub fn position_at(p: &MeshPlaying, now_ms: i64) -> i64 {
    p.position_ms + ((now_ms - p.at_ms) as f64 * f64::from(p.rate)) as i64
}

/// Whether `new` says something `old` does not (the position moving on
/// with its clock is nothing new).
pub fn differs(old: &Presence, new: &Presence) -> bool {
    if old.name != new.name || old.kind != new.kind || old.version != new.version || old.storage != new.storage || old.copy != new.copy {
        return true;
    }
    if old.holder != new.holder || old.assign != new.assign || old.pause != new.pause || transfer_differs(&old.transfer, &new.transfer) {
        return true;
    }
    match (&old.playing, &new.playing) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            a.title != b.title
                || a.artists != b.artists
                || a.paused != b.paused
                || (a.rate - b.rate).abs() > 0.01
                || a.duration_ms != b.duration_ms
                || (position_at(a, b.at_ms) - b.position_ms).abs() > JUMP_MS
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(pos: i64, at: i64, rate: f32) -> Presence {
        Presence {
            name: "Pi".into(),
            playing: Some(MeshPlaying { title: "T".into(), artists: vec!["A".into()], paused: rate == 0.0, position_ms: pos, at_ms: at, rate, duration_ms: Some(200_000) }),
            ..Presence::default()
        }
    }

    #[test]
    fn the_clock_moving_on_is_no_change_but_a_seek_is() {
        let a = playing(10_000, 1_000_000, 1.0);
        assert!(!differs(&a, &playing(15_000, 1_005_000, 1.0)), "five seconds later, five seconds on");
        assert!(differs(&a, &playing(60_000, 1_005_000, 1.0)), "a seek");
        assert!(differs(&a, &playing(15_000, 1_005_000, 0.0)), "paused");
        assert!(differs(&a, &Presence { playing: None, ..a.clone() }), "stopped");
        assert!(differs(&a, &Presence { name: "Кухня".into(), ..a.clone() }));
        assert_eq!(position_at(a.playing.as_ref().unwrap(), 1_002_000), 12_000);
    }

    #[test]
    fn the_view_says_what_plays() {
        let mut st = ViewState::default();
        assert_eq!(of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1).playing, None);
        st.player.status = Status::Paused;
        st.player.current = Some(nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(),
            title: "Song".into(),
            artist: "Band".into(),
            album: None,
            release_id: None,
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
            temporary: false,
        });
        let p = of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1).playing.unwrap();
        assert_eq!(p.title, "Song");
        assert_eq!(p.artists, vec!["Band".to_owned()]);
        assert!(p.paused);
        assert_eq!(p.rate, 0.0);

        // Driving another device: the player is that one's; this device
        // tells of its own zone, parked.
        st.player.status = Status::Playing;
        st.player.rate = 1.0;
        st.target = Some(nikgob_view::RemoteTarget {
            node_id: "pi".into(),
            name: "Pi".into(),
            kind: MeshKind::Pi,
            link: nikgob_view::RemoteLink::Live,
            volume: 1.0,
            muted: false,
            how: nikgob_view::RemoteHow::Transferred,
        });
        assert_eq!(of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1).playing, None, "nothing parked");
        let mut own = st.player.current.clone().unwrap();
        own.title = "Own".into();
        st.parked = Some(nikgob_view::ParkedView { current: Some(own), position: std::time::Duration::from_secs(7) });
        let p = of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1).playing.unwrap();
        assert_eq!((p.title.as_str(), p.paused, p.position_ms, p.rate), ("Own", true, 7_000, 0.0));
    }

    /// The ring holder tells what each owner device stores (a guest
    /// nothing); another device tells nothing of that.
    #[test]
    fn the_holder_gives_the_devices_their_profiles() {
        let mut st = ViewState::default();
        let list = PlaylistId::new();
        st.mesh.nodes = vec![
            nikgob_view::MeshNodeView {
                id: "phone".into(),
                profile: MeshProfile::Favorites { cap_bytes: 8 << 30 },
                pins: nikgob_view::MeshPins { liked: true, playlists: vec![list] },
                ..nikgob_view::MeshNodeView::default()
            },
            nikgob_view::MeshNodeView { id: "tv".into(), scope: MeshScope::Guest, ..nikgob_view::MeshNodeView::default() },
        ];
        assert!(of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1).assign.is_empty(), "not the holder");
        st.mesh.ring_holder = true;
        let p = of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1);
        assert!(p.holder);
        assert_eq!(p.assign, vec![Assigned { node: "phone".into(), profile: MeshProfile::Favorites { cap_bytes: 8 << 30 }, liked: true, playlists: vec![list] }]);
        let back: Presence = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        let old: Presence = serde_json::from_str(r#"{"name":"Pi","kind":"pi","version":"0.0.4","storage":null,"playing":null}"#).unwrap();
        assert!(old.assign.is_empty() && old.transfer.is_none() && old.copy.is_none(), "a device of an older version");
    }

    /// What a device's plan gives it and how much of it it holds goes out
    /// with its presence; a presence without it (an older build) still parses.
    #[test]
    fn a_devices_plan_progress_survives_json() {
        let p = Presence { copy: Some(MeshCopy { done: 14 << 30, goal: 16 << 30 }), ..Presence::default() };
        let back: Presence = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back.copy, Some(MeshCopy { done: 14 << 30, goal: 16 << 30 }));
        let old: Presence = serde_json::from_str(r#"{"name":"Pi","kind":"pi","version":"0.0.4","storage":null,"playing":null}"#).unwrap();
        assert_eq!(old.copy, None, "a device of an older version");
        assert!(differs(&Presence::default(), &p), "the progress is news");
    }

    #[test]
    fn a_transfer_is_told_when_it_moved_enough() {
        let t = |done: u64, speed: u64| Some(MeshTransfer { done, total: 10 << 30, paused: false, speed });
        assert!(!transfer_differs(&t(0, 5), &t(100 << 20, 5)), "under a fiftieth of ten gigabytes");
        assert!(transfer_differs(&t(0, 5), &t(300 << 20, 5)));
        assert!(transfer_differs(&t(0, 5), &t(0, 0)), "it stopped");
        assert!(transfer_differs(&None, &t(0, 0)));
        assert!(transfer_differs(&t(0, 0), &None));
        let p = Presence { transfer: t(0, 5), ..Presence::default() };
        assert!(differs(&p, &Presence { pause: vec!["pi".into()], ..p.clone() }));
    }
}
