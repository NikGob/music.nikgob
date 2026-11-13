//! «С новым»: the new songs of the services a mix of
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
//!   [`voice_near`]), else by the library's songs the graph links it to
//!   ([`voice_of`]), then by its artist's or title's language
//!   ([`lane_of_new`]), else a
//!   franchise's soundtrack for songs without words. Those the graph of
//!   similar music links to the mix's own songs first, then those of its
//!   profiles (by the graph or by the preview), then the rest; each group
//!   by the candidate's weight (its rank among the finds times how its
//!   source is received, [`NewCandidate::weight`]). One song goes to one
//!   mix of the row.
//! * **Where** ([`places`]): each between two loved familiar songs, never in
//!   the first two places nor two running, spread as evenly as such places
//!   allow; what finds no place is not played.
//!
//! The open world ([`NewMode`]): the
//! cap and the places between loved songs hold for the calm default
//! («Сбалансировано», [`NewMode::Calm`]). With «Больше нового»
//! ([`NewMode::Open`]) a mix takes as much as the share asks, up to
//! [`OPEN_MAX_SHARE`], spread evenly anywhere after its first song
//! ([`spread`]). A niche ([`NewMode::Niche`]: a mix of fewer than
//! [`crate::open_world::NICHE_TRACKS`] songs or
//! [`crate::open_world::NICHE_ARTISTS`] artists) takes [`NICHE_SHARE`] at
//! least, of the songs that fit the niche's sound
//! ([`crate::open_world::NicheSound`]), the same way ([`take`]). Whether a
//! mix is a niche is told by the whole pool it is drawn from
//! ([`Mix::pool_size`]), not by the card; the niches the row lacks (a few
//! calm songs, a small kind or genre: [`crate::open_world::niches`]) get a
//! card of their own, their songs as few as they are ([`with_niches`]).

use std::collections::HashSet;

use nikgob_core::model::TrackId;
use nikgob_core::streaming::StreamTrack;

use crate::analysis::Vector;
use crate::embedding::Compact;
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
/// The open modes' share at most…
pub const OPEN_MAX_SHARE: f32 = 0.9;
/// …a niche's at least…
pub const NICHE_SHARE: f32 = 0.8;
/// …and the user's share from which the open world's rules hold
/// («Больше нового» is 0.4, «Немного» 0.2).
pub const OPEN_FROM: f32 = 0.35;
/// A niche's card asks for new music as if it had this many songs at
/// least (three calm songs still give a card worth playing).
pub const NICHE_CARD_LEN: usize = 7;
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
    /// Its analysed preview (raw), if any.
    pub vector: Option<Vector>,
    /// The neural sound model's vector of its preview, if the model heard it.
    pub embedding: Option<Compact>,
    /// The library's songs of its main artist (catalog positions; empty
    /// for a new artist).
    pub artist_tracks: Vec<usize>,
}

/// How a mix takes new music (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewMode {
    /// Between loved songs, [`CARD_MAX_SHARE`] at most.
    #[default]
    Calm,
    /// As much as asked, anywhere after the first song.
    Open,
    /// A niche: [`NICHE_SHARE`] at least, of its sound.
    Niche,
}

impl NewMode {
    /// The mode of a mix of `tracks` songs by `artists` artists at the
    /// user's share `user`.
    pub fn of(tracks: usize, artists: usize, user: f32) -> Self {
        if crate::open_world::is_niche(tracks, artists) {
            Self::Niche
        } else if user >= OPEN_FROM {
            Self::Open
        } else {
            Self::Calm
        }
    }

    /// The new songs stand anywhere after the first.
    pub const fn open(self) -> bool {
        !matches!(self, Self::Calm)
    }
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

/// How many new songs a mix of `songs` asks for in `mode` ([`want`] for the
/// calm one).
pub fn want_in(songs: usize, appetite: Option<f32>, user: f32, mode: NewMode) -> usize {
    let base = appetite.unwrap_or(1.0 / NEW_EVERY as f32);
    match mode {
        NewMode::Calm => want(songs, appetite, user),
        NewMode::Open => new_count(songs, base.max(user).min(OPEN_MAX_SHARE)),
        NewMode::Niche => new_count(songs.max(NICHE_CARD_LEN), base.max(user).max(NICHE_SHARE).min(OPEN_MAX_SHARE)),
    }
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

/// The voice of a new song by its analysed preview (`v`, raw; `embedding`,
/// the neural model's vector of it, when the model heard it): what most of
/// its nearest songs of the library are, when they agree.
pub fn voice_near(model: &UserModel, index: &VectorIndex<TrackId>, v: &Vector, embedding: Option<&Compact>) -> Option<Voice> {
    let near: Vec<usize> = index.nearest_with(v, embedding, VOICE_NEAR, |_| true).into_iter().filter_map(|(id, _)| model.position(id)).collect();
    voice_of(model, &near)
}

/// What most of the library's songs `near` (catalog positions) are, sung or
/// without words, when they agree: the voice of a new song without a
/// preview that the graph links to them (YouTube Music's radio of a
/// soundtrack plays soundtracks).
pub fn voice_of(model: &UserModel, near: &[usize]) -> Option<Voice> {
    let (mut sung, mut inst) = (0usize, 0usize);
    for &i in near.iter().filter(|&&i| i < model.len()) {
        match model.song_type_at(i).voice {
            Voice::Sung => sung += 1,
            Voice::Instrumental => inst += 1,
            Voice::Unknown => {}
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
    choose_where(model, m, cands, want, used, &|_| true)
}

/// [`choose`] of the candidates `keep` lets through.
pub fn choose_where<'c>(model: &UserModel, m: &Mix, cands: &'c [NewCandidate], want: usize, used: &mut HashSet<String>, keep: &dyn Fn(&NewCandidate) -> bool) -> Vec<&'c NewCandidate> {
    choose_by(model, m, cands, want, used, keep, true)
}

/// [`choose_where`], the mix's kinds of song checked or not (a niche
/// checks its own, [`crate::open_world::NicheSound`]).
fn choose_by<'c>(model: &UserModel, m: &Mix, cands: &'c [NewCandidate], want: usize, used: &mut HashSet<String>, keep: &dyn Fn(&NewCandidate) -> bool, kind: bool) -> Vec<&'c NewCandidate> {
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
    let mut order: Vec<(u8, f32, usize)> =
        cands.iter().enumerate().filter(|(_, c)| (!kind || fits(c, &langs, inst)) && keep(c)).map(|(k, c)| (group(c), c.weight, k)).collect();
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

/// The new songs `m` takes of `cands` (none another mix took, `used`) as
/// the open world has it: its mode by its size and the user's share, as
/// many as the mode asks, a niche's only of its sound (see the module
/// docs). `appetite` is the share the mix's profile draws. Without new
/// music asked for (`user` 0) a mix is no niche. The size that makes a
/// niche is the whole pool's ([`Mix::pool_size`]; a card made without one
/// counts its own songs), its sound the pool's; a calm or lively card's is
/// a niche of a sound (strict), heard by the library's `index` with the
/// neural model's vectors when it is given.
pub fn take<'c>(
    model: &UserModel,
    index: Option<&VectorIndex<TrackId>>,
    m: &Mix,
    cands: &'c [NewCandidate],
    appetite: Option<f32>,
    user: f32,
    used: &mut HashSet<String>,
) -> (Vec<&'c NewCandidate>, NewMode) {
    let (tracks, artists) = if m.pool_size.0 > 0 { m.pool_size } else { (m.tracks.len(), m.artists.len()) };
    let mode = if user > 0.0 { NewMode::of(tracks, artists, user) } else { NewMode::Calm };
    let want = want_in(m.tracks.len(), appetite, user, mode);
    let chosen = if mode == NewMode::Niche {
        let seeds = if m.pool.is_empty() { &m.tracks } else { &m.pool };
        let strict = matches!(m.key, MixKey::Calm | MixKey::Lively | MixKey::Background { .. });
        let sound = crate::open_world::NicheSound::heard(model, seeds, strict, index);
        choose_by(
            model,
            m,
            cands,
            want,
            used,
            &|c| {
                let energies: Vec<f32> = c.artist_tracks.iter().filter_map(|&i| model.energy_at(i)).collect();
                sound.fits_heard(model, index, c.vector.as_ref(), c.embedding.as_ref(), c.lane, &c.near, &energies)
            },
            false,
        )
    } else {
        choose(model, m, cands, want, used)
    };
    (chosen, mode)
}

/// The niche cards of [`crate::open_world::niches`] in the row `mixes`,
/// each with its title and short titles: a card of that key already
/// drawn from a niche's pool stays; the calm side of everything (a third
/// of the library, whatever it sounds like) gives way to the calm niche's
/// songs; a niche the row lacks is a card of its own after the others,
/// its songs as few as they are.
pub fn with_niches(model: &UserModel, mixes: &mut Vec<Mix>, cards: Vec<(crate::open_world::NicheCard, String, Vec<String>)>) {
    for (card, title, short) in cards {
        let artists: Vec<nikgob_core::model::ArtistId> = {
            let mut seen = HashSet::new();
            card.seeds.iter().filter_map(|&i| model.main_artist_at(i)).filter(|a| seen.insert(*a)).take(12).collect()
        };
        let mut profiles: Vec<usize> = card.seeds.iter().filter_map(|&i| model.profile_at(i)).collect();
        profiles.sort_unstable();
        profiles.dedup();
        let size = (card.seeds.len(), artists.len());
        match mixes.iter_mut().find(|m| m.key == card.key) {
            Some(m) if crate::open_world::is_niche(m.pool_size.0, m.pool_size.1) => {}
            Some(m) if card.key == MixKey::Calm => {
                m.anchors = vec![true; card.seeds.len()];
                m.tracks.clone_from(&card.seeds);
                m.artists = artists;
                m.profiles = profiles;
                m.pool_size = size;
                m.pool = card.seeds;
            }
            Some(_) => {}
            None => {
                let genre = if let MixKey::Genre(f) = card.key { Some(f) } else { None };
                mixes.push(Mix {
                    key: card.key,
                    title,
                    short,
                    anchors: vec![true; card.seeds.len()],
                    tracks: card.seeds.clone(),
                    artists,
                    profiles,
                    score: 0.0,
                    kind: None,
                    genre,
                    pool_size: size,
                    pool: card.seeds,
                });
            }
        }
    }
}

/// After which places of a mix `n` new songs stand in `mode`: between
/// loved ones ([`places`]) in the calm one, else evenly ([`spread`]).
pub fn placements(mode: NewMode, anchors: &[bool], n: usize) -> Vec<usize> {
    if mode.open() { spread(anchors.len(), n) } else { places(anchors, n) }
}

/// After which places of a mix of `len` songs `n` new songs stand in the
/// open modes, in order: evenly from after the first song on, as many
/// after one place as the share needs (a place repeated).
pub fn spread(len: usize, n: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    (0..n).map(|j| ((j * len) / n.max(1)).min(len - 1)).collect()
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
        NewCandidate { track: StreamTrack::new(RemoteId::new(Service::Youtube, id), id), lane, soundtrack: false, profile: None, near, weight, vector: None, embedding: None, artist_tracks: Vec::new() }
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

    /// The modes: a small mix is a niche, «Больше нового» opens the rest;
    /// the open modes take more and place them evenly, runs where needed.
    #[test]
    fn the_open_modes_take_more_anywhere() {
        assert_eq!(NewMode::of(28, 10, 0.2), NewMode::Calm);
        assert_eq!(NewMode::of(28, 10, 0.4), NewMode::Open);
        assert_eq!(NewMode::of(14, 10, 0.2), NewMode::Niche);
        assert_eq!(NewMode::of(28, 3, 0.0), NewMode::Niche);
        assert_eq!(want_in(28, None, 0.4, NewMode::Calm), want(28, None, 0.4));
        assert_eq!(want_in(28, None, 0.4, NewMode::Open), new_count(28, 0.4));
        assert!(want_in(28, None, 0.4, NewMode::Open) > want(28, None, 0.4), "over the calm cap");
        assert_eq!(want_in(12, None, 0.2, NewMode::Niche), new_count(12, NICHE_SHARE));
        let at = spread(12, 48);
        assert_eq!(at.len(), 48);
        assert!(at.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(at.iter().filter(|&&k| k == 0).count(), 4, "four after each song: {at:?}");
        assert_eq!(*at.last().unwrap(), 11);
        assert_eq!(spread(28, 4), vec![0, 7, 14, 21]);
        assert!(spread(0, 3).is_empty());
        assert_eq!(placements(NewMode::Calm, &[true; 10], 2), places(&[true; 10], 2));
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
