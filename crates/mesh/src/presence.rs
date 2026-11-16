//! What a device tells the others of itself while they are connected: its
//! name, kind and version, its library disk, and what it plays with the
//! clock of the position (a remote extrapolates the progress itself).

use nikgob_view::{MeshKind, MeshPlaying, MeshStorage, Status, ViewState};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub name: String,
    pub kind: MeshKind,
    pub version: String,
    pub storage: Option<MeshStorage>,
    pub playing: Option<MeshPlaying>,
}

/// A position that drifted further than this from where the last report
/// puts it is a change (a seek); closer, the clock says it.
pub const JUMP_MS: i64 = 1500;

/// This device's presence from its view at `now_ms`.
pub fn of_view(st: &ViewState, name: &str, kind: MeshKind, version: &str, now_ms: i64) -> Presence {
    let p = &st.player;
    let playing = match (p.status, p.current.as_ref()) {
        (Status::Stopped, _) | (_, None) => None,
        (status, Some(t)) => {
            let mut artists: Vec<String> = t.artists.0.iter().map(|n| n.name.clone()).filter(|n| !n.trim().is_empty()).collect();
            if artists.is_empty() && !t.artist.trim().is_empty() {
                artists.push(t.artist.clone());
            }
            let playing = status == Status::Playing;
            Some(MeshPlaying {
                title: t.title.clone(),
                artists,
                paused: !playing,
                position_ms: i64::try_from(p.position_now().as_millis()).unwrap_or(i64::MAX),
                at_ms: now_ms,
                rate: if playing { p.rate.max(0.0) } else { 0.0 },
                duration_ms: p.duration.or(t.duration).and_then(|d| i64::try_from(d.as_millis()).ok()),
            })
        }
    };
    let s = &st.downloads.storage;
    let storage = s.known.then(|| MeshStorage { used: s.music(), free: s.disks.first().map_or(0, |d| d.free) });
    Presence { name: name.to_owned(), kind, version: version.to_owned(), storage, playing }
}

/// Where `p` puts the position at `now_ms`.
pub fn position_at(p: &MeshPlaying, now_ms: i64) -> i64 {
    p.position_ms + ((now_ms - p.at_ms) as f64 * f64::from(p.rate)) as i64
}

/// Whether `new` says something `old` does not (the position moving on
/// with its clock is nothing new).
pub fn differs(old: &Presence, new: &Presence) -> bool {
    if old.name != new.name || old.kind != new.kind || old.version != new.version || old.storage != new.storage {
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
        });
        let p = of_view(&st, "PC", MeshKind::Pc, "0.0.5", 1).playing.unwrap();
        assert_eq!(p.title, "Song");
        assert_eq!(p.artists, vec!["Band".to_owned()]);
        assert!(p.paused);
        assert_eq!(p.rate, 0.0);
    }
}
