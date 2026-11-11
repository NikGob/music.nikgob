//! «С новым» (recs-v2 §5.4): the new songs of the services a mix of
//! «Миксы для тебя» plays between its own.
//!
//! * **How many**: the share the appetite of the mix's taste profile draws
//!   ([`crate::NewLog::share`], about every fifth song before anything new
//!   was heard), at least the user's own share of new music («Новое в радио
//!   и миксах»), at most [`CARD_MAX_SHARE`] ([`want`]).
//! * **Which**: of the kinds of song the mix holds ([`kinds`]: the sung
//!   languages of its profiles, songs without words), a candidate's kind
//!   told by its preview where it was analysed (its nearest songs of the
//!   library: an instrumental track with a Latin title is no English song,
//!   [`lane_of_new`]), else by its artist's or title's language, else a
//!   franchise's soundtrack for songs without words. Those the graph of
//!   similar music links to the mix's own songs first, then those of its
//!   profiles (by the graph or by the preview), then the rest; each group
//!   by the candidate's weight (its rank among the finds times how its
//!   source is received, [`NewCandidate::weight`]). One song goes to one
//!   mix of the row.
//! * **Where** ([`places`]): each between two loved familiar songs, never in
//!   the first two places nor two running, spread as evenly as such places
//!   allow; what finds no place is not played.

use std::collections::HashSet;

use nikgob_core::model::TrackId;
use nikgob_core::streaming::StreamTrack;

use crate::analysis::Vector;
use crate::index::VectorIndex;
use crate::mixes::{Mix, MixKey};
use crate::model::UserModel;
use crate::profiles::LaneKey;
use crate::songtype::{Lang, SongTypes, Voice};

/// About every fifth song of a mix is new before anything new was heard.
pub const NEW_EVERY: usize = 5;
/// The user's share of new music a mix takes at most (more finds no places
/// between loved songs anyway).
pub const CARD_MAX_SHARE: f32 = 0.3;
/// A preview's kind of song is told by this many of its nearest songs of
/// the library…
const VOICE_NEAR: usize = 7;
/// …this share of those with a known voice agreeing.
const VOICE_AGREE: f32 = 0.7;

/// A new song a mix may take.
#[derive(Clone, Debug)]
pub struct NewCandidate {
    pub track: StreamTrack,
    /// Its kind of song, if told ([`lane_of_new`]).
    pub lane: Option<LaneKey>,
    /// On a franchise's album (a soundtrack).
    pub soundtrack: bool,
    /// The taste profile its preview falls in.
    pub profile: Option<usize>,
    /// The library's songs (catalog positions) the graph of similar music
    /// links it to.
    pub near: Vec<usize>,
    /// How much it is believed: its rank among the finds times the share
    /// its source draws ([`crate::NewLog::source_shares`]) or the source's
    /// trust ([`crate::GraphSource::trust`]).
    pub weight: f32,
}

/// How many new songs a mix of `songs` gets at `share` of the whole (one
/// at least).
pub fn new_count(songs: usize, share: f32) -> usize {
    let share = share.clamp(0.0, 0.9);
    ((songs as f32 * share / (1.0 - share)).round() as usize).max(1)
}

/// How many new songs a mix of `songs` asks for: the profile's appetite
/// (`None`: nothing new was heard yet, every fifth song), at least the
/// user's `share` of new music, at most [`CARD_MAX_SHARE`] by the user's.
pub fn want(songs: usize, appetite: Option<f32>, user: f32) -> usize {
    let base = appetite.unwrap_or(1.0 / NEW_EVERY as f32);
    new_count(songs, base.max(user.min(CARD_MAX_SHARE)))
}

/// The kinds of song of a mix: the sung languages of its profiles (of its
/// songs for a mix of a kind of the library) and whether songs without
/// words are among them.
pub fn kinds(model: &UserModel, m: &Mix) -> (HashSet<Lang>, bool) {
    let mut langs = HashSet::new();
    let mut inst = false;
    let lanes: Vec<LaneKey> = match m.key {
        MixKey::Kind(_) => m.tracks.iter().map(|&i| LaneKey::of(model.song_type_at(i))).collect(),
        _ => m.profiles.iter().filter_map(|&j| model.profiles.list.get(j)).map(|p| p.lane).collect(),
    };
    for l in lanes {
        match l {
            LaneKey::Sung(x) => {
                langs.insert(x);
            }
            LaneKey::Instrumental => inst = true,
            LaneKey::Unknown => {}
        }
    }
    (langs, inst)
}

/// The voice of a new song by its analysed preview (`v`, raw): what most of
/// its nearest songs of the library are, when they agree.
pub fn voice_near(model: &UserModel, index: &VectorIndex<TrackId>, v: &Vector) -> Option<Voice> {
    let near = index.nearest(v, VOICE_NEAR, |_| true);
    let (mut sung, mut inst) = (0usize, 0usize);
    for (id, _) in near {
        match model.position(id).map(|i| model.song_type_at(i).voice) {
            Some(Voice::Sung) => sung += 1,
            Some(Voice::Instrumental) => inst += 1,
            _ => {}
        }
    }
    let known = sung + inst;
    if known == 0 {
        return None;
    }
    if inst as f32 >= VOICE_AGREE * known as f32 {
        Some(Voice::Instrumental)
    } else if sung as f32 >= VOICE_AGREE * known as f32 {
        Some(Voice::Sung)
    } else {
        None
    }
}

/// The kind of song of a new song: without words when its preview sounds
/// so, else the language its artist or title tells (`types`).
pub fn lane_of_new(types: &SongTypes, voice: Option<Voice>, t: &StreamTrack) -> Option<LaneKey> {
    if voice == Some(Voice::Instrumental) {
        return Some(LaneKey::Instrumental);
    }
    let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
    types.lang_of_new(&t.title, &artists).map(LaneKey::Sung)
}

/// A candidate fits a mix of `langs` (and of songs without words, `inst`).
pub fn fits(c: &NewCandidate, langs: &HashSet<Lang>, inst: bool) -> bool {
    match c.lane {
        Some(LaneKey::Sung(l)) => langs.contains(&l),
        Some(LaneKey::Instrumental) => inst,
        _ => inst && c.soundtrack,
    }
}

/// The new songs `m` takes of `cands`, `want` at most, none another mix
/// took (`used`, by their remote ids): see the module docs.
pub fn choose<'c>(model: &UserModel, m: &Mix, cands: &'c [NewCandidate], want: usize, used: &mut HashSet<String>) -> Vec<&'c NewCandidate> {
    let (langs, inst) = kinds(model, m);
    let own: HashSet<usize> = m.tracks.iter().copied().collect();
    let of_mix = |j: usize| m.profiles.contains(&j);
    let group = |c: &NewCandidate| -> u8 {
        if c.near.iter().any(|i| own.contains(i)) {
            2
        } else if c.profile.is_some_and(of_mix) || c.near.iter().any(|&i| model.profile_at(i).is_some_and(of_mix)) {
            1
        } else {
            0
        }
    };
    let mut order: Vec<(u8, f32, usize)> = cands.iter().enumerate().filter(|(_, c)| fits(c, &langs, inst)).map(|(k, c)| (group(c), c.weight, k)).collect();
    order.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut out = Vec::new();
    for (_, _, k) in order {
        if out.len() >= want {
            break;
        }
        if used.insert(cands[k].track.id.to_string()) {
            out.push(&cands[k]);
        }
    }
    out
}

/// After which places of a mix of `anchors` (familiar and loved) `n` new
/// songs stand, in order: each between two anchors, never in the first two
/// places nor two running, spread as evenly as such places allow.
pub fn places(anchors: &[bool], n: usize) -> Vec<usize> {
    let len = anchors.len();
    // The gaps after place k (k ≥ 1): between two anchors.
    let gaps: Vec<usize> = (1..len.saturating_sub(1)).filter(|&k| anchors[k] && anchors[k + 1]).collect();
    let mut after: Vec<usize> = Vec::new();
    for j in 0..n.min(gaps.len()) {
        // The gap nearest the even spot of the j-th new song, free.
        let spot = (j + 1) * len / (n + 1);
        if let Some(&g) = gaps.iter().filter(|g| !after.contains(g) && !after.contains(&(*g - 1)) && !after.contains(&(*g + 1))).min_by_key(|g| g.abs_diff(spot)) {
            after.push(g);
        }
    }
    after.sort_unstable();
    after
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{RemoteId, Service};

    fn cand(id: &str, lane: Option<LaneKey>, near: Vec<usize>, weight: f32) -> NewCandidate {
        NewCandidate { track: StreamTrack::new(RemoteId::new(Service::Youtube, id), id), lane, soundtrack: false, profile: None, near, weight }
    }

    /// The share of a mix: the appetite, at least the user's, at most
    /// [`CARD_MAX_SHARE`]; one at least.
    #[test]
    fn the_count_follows_the_appetite_and_the_user() {
        assert_eq!(new_count(36, 0.1), 4);
        assert_eq!(new_count(36, 0.2), 9);
        assert_eq!(new_count(3, 0.05), 1);
        // Nothing heard yet: every fifth.
        assert_eq!(want(28, None, 0.0), 7);
        assert_eq!(want(28, Some(0.1), 0.0), new_count(28, 0.1));
        // «Побольше»: the user's share over a small appetite, capped.
        assert_eq!(want(28, Some(0.1), 0.4), new_count(28, CARD_MAX_SHARE));
        assert_eq!(want(28, Some(0.2), 0.2), new_count(28, 0.2));
    }

    /// A candidate of the mix's kind only; an instrumental preview fits a
    /// mix without words whatever its title; one the graph links to the
    /// mix's songs goes first, then by weight; each to one mix.
    #[test]
    fn candidates_fit_and_rank() {
        let langs: HashSet<Lang> = [Lang::Russian].into_iter().collect();
        assert!(fits(&cand("a", Some(LaneKey::Sung(Lang::Russian)), vec![], 1.0), &langs, false));
        assert!(!fits(&cand("b", Some(LaneKey::Sung(Lang::English)), vec![], 1.0), &langs, true));
        assert!(fits(&cand("c", Some(LaneKey::Instrumental), vec![], 1.0), &HashSet::new(), true));
        assert!(!fits(&cand("d", None, vec![], 1.0), &HashSet::new(), true), "no kind, no soundtrack");
        let ost = NewCandidate { soundtrack: true, ..cand("e", None, vec![], 1.0) };
        assert!(fits(&ost, &HashSet::new(), true));
        let types = SongTypes::default();
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, "x"), "survive and get the blade, boy");
        t.artists.push(nikgob_core::streaming::ArtistRef::new(None, "Yuki Kajiura"));
        assert_eq!(lane_of_new(&types, Some(Voice::Instrumental), &t), Some(LaneKey::Instrumental));
        assert_ne!(lane_of_new(&types, None, &t), Some(LaneKey::Instrumental));
    }

    /// New songs stand between two anchors, never in the first two places
    /// nor two running, spread out.
    #[test]
    fn places_are_between_anchors() {
        let anchors: Vec<bool> = (0..28).map(|k| k % 5 != 3).collect();
        let at = places(&anchors, 4);
        assert_eq!(at.len(), 4, "{at:?}");
        for &k in &at {
            assert!(k >= 1 && anchors[k] && anchors[k + 1], "{at:?}");
        }
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= 3, "spread: {at:?}");
        }
        assert!(places(&anchors, 0).is_empty());
        assert!(places(&[true, true], 3).is_empty());
    }
}
