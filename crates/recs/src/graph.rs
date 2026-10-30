//! The graph of similar music (docs/recs-graph.md, TODO 17): what stands in
//! for collaborative filtering in a player of one user. Two graphs, kept
//! apart:
//!
//! * **The collective graph** ([`SimGraph`]): what other listeners know —
//!   YouTube Music's radio of a track, the similar artists of YouTube Music,
//!   Deezer, ListenBrainz and Last.fm, ListenBrainz's similar recordings and
//!   the tags of MusicBrainz — fetched by the runtime, cached and resolved
//!   to the catalog ([`GraphInput`]). An edge weighs by its place in its
//!   list ([`rank_weight`]) times the trust of its source; the sources join
//!   as a noisy or, an edge back counts [`BACK`] of one forth. Track → track
//!   keeps the best [`TRACK_TOP`] of each track, artist → artist the best
//!   [`ARTIST_TOP`] of each artist, with the likeness of the artists' tags
//!   ([`TAG_WEIGHT`] of their IDF cosine) joining the lists.
//! * **The prior** of each track ([`SimGraph::prior`]): how much of the
//!   graph of the rest of the library meets on it and where it stands in
//!   its artist's popularity, ranked within the catalog to 0…1 — the
//!   library is a taste too: a track the collection gathers around, and a
//!   well known song of its artist, stand higher.
//! * **The personal graph** ([`personal`]): the user's own knowledge that
//!   the session pairs of the app's history do not hold — the sessions of
//!   an imported history ([`IMPORT_GAP_MS`], `0.5^(k−1)` for the k-th start
//!   after, k ≤ 3) and the neighbours in the order of the user's playlists
//!   (1, 0.5, 0.25 for one, two, three apart), both ways.
//!
//! No network, no database here; an empty input is an empty graph and
//! every lookup is 0.

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, EventOrigin, EventType, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::taste::{Catalog, Play};

/// Neighbours kept per track…
pub const TRACK_TOP: usize = 50;
/// …and per artist.
pub const ARTIST_TOP: usize = 30;
/// An edge back counts this much of one forth.
pub const BACK: f32 = 0.8;
/// The likeness of two artists' tags joins their edge at this weight.
pub const TAG_WEIGHT: f32 = 0.6;
/// Tags alike below this cosine are no edge.
const TAG_MIN: f32 = 0.2;
/// A tag held by more artists than this is not indexed (it says nothing).
const TAG_POSTINGS: usize = 2_000;
/// The prior: how much an artist's centrality and a track's popularity
/// weigh beside the track's own centrality.
const PRIOR_ARTIST: f32 = 0.5;
const PRIOR_POPULAR: f32 = 0.7;
/// A pause this long ends a session of an imported history.
pub const IMPORT_GAP_MS: i64 = 30 * 60_000;

/// Where an edge comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GraphSource {
    /// YouTube Music's radio of a track.
    YoutubeRadio,
    /// YouTube Music's similar artists.
    YoutubeArtists,
    /// Deezer's related artists.
    DeezerArtists,
    /// ListenBrainz's similar artists and recordings (its listeners'
    /// sessions).
    ListenBrainz,
    /// Last.fm's similar tracks and artists (scrobbles).
    LastFm,
}

impl GraphSource {
    pub const ALL: [Self; 5] = [Self::YoutubeRadio, Self::YoutubeArtists, Self::DeezerArtists, Self::ListenBrainz, Self::LastFm];

    /// How much an edge of the source is believed.
    pub const fn trust(self) -> f32 {
        match self {
            Self::YoutubeRadio => 1.0,
            Self::ListenBrainz | Self::LastFm => 0.9,
            Self::YoutubeArtists | Self::DeezerArtists => 0.8,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YoutubeRadio => "youtube_radio",
            Self::YoutubeArtists => "youtube_artists",
            Self::DeezerArtists => "deezer_artists",
            Self::ListenBrainz => "listenbrainz",
            Self::LastFm => "lastfm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.as_str() == s)
    }
}

/// The weight of the neighbour at `rank` (0 first) of a list of `of`.
pub fn rank_weight(rank: u32, of: u32) -> f32 {
    let n = of.max(rank + 1) as f32;
    (1.0 - rank as f32 / (n + 1.0)).max(0.0).powf(0.7)
}

/// One edge of a source's list, resolved to the catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge<T> {
    pub from: T,
    pub to: T,
    /// Its place in the list (0 first) of `of`.
    pub rank: u32,
    pub of: u32,
    pub source: GraphSource,
}

/// The collective graph as the runtime resolved it.
#[derive(Clone, Debug, Default)]
pub struct GraphInput {
    pub tracks: Vec<Edge<TrackId>>,
    pub artists: Vec<Edge<ArtistId>>,
    /// An artist's tags and their weights (MusicBrainz's counts).
    pub tags: Vec<(ArtistId, String, f32)>,
    /// A track's place in its artist's popularity (0 the best known) of
    /// how many listed.
    pub popular: Vec<(TrackId, u32, u32)>,
}

impl GraphInput {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.artists.is_empty() && self.tags.is_empty() && self.popular.is_empty()
    }
}

/// The collective graph over a catalog (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct SimGraph {
    /// By catalog position: the neighbours, the strongest first.
    track: Vec<Vec<(u32, f32)>>,
    artist: HashMap<ArtistId, Vec<(ArtistId, f32)>>,
    prior: Vec<f32>,
}

/// Noisy or of `w` into `slot`.
fn join(slot: &mut f32, w: f32) {
    *slot = 1.0 - (1.0 - *slot) * (1.0 - w.clamp(0.0, 1.0));
}

/// Rows of `forth` with the edges back at [`BACK`], the best `top` of each.
fn symmetric<K: Copy + Eq + std::hash::Hash + Ord>(forth: HashMap<K, HashMap<K, f32>>, top: usize) -> HashMap<K, Vec<(K, f32)>> {
    let mut both: HashMap<K, HashMap<K, f32>> = HashMap::new();
    for (a, row) in &forth {
        for (b, w) in row {
            let e = both.entry(*a).or_default().entry(*b).or_insert(0.0);
            *e = e.max(*w);
            let e = both.entry(*b).or_default().entry(*a).or_insert(0.0);
            *e = e.max(BACK * *w);
        }
    }
    both.into_iter()
        .map(|(a, row)| {
            let mut row: Vec<(K, f32)> = row.into_iter().filter(|(_, w)| *w > 0.0).collect();
            row.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            row.truncate(top);
            (a, row)
        })
        .collect()
}

impl SimGraph {
    /// The graph of `input` over `catalog` (edges of tracks or artists it
    /// does not hold are left out).
    pub fn build(catalog: &Catalog, input: &GraphInput) -> Self {
        let n = catalog.len();
        // Track → track.
        let mut forth: HashMap<u32, HashMap<u32, f32>> = HashMap::new();
        for e in &input.tracks {
            let (Some(a), Some(b)) = (catalog.position(e.from), catalog.position(e.to)) else { continue };
            if a == b {
                continue;
            }
            join(forth.entry(a as u32).or_default().entry(b as u32).or_insert(0.0), rank_weight(e.rank, e.of) * e.source.trust());
        }
        let rows = symmetric(forth, TRACK_TOP);
        let mut track = vec![Vec::new(); n];
        for (a, row) in rows {
            track[a as usize] = row;
        }
        // Artist → artist: the lists, then the tags.
        let held = |a: ArtistId| !catalog.of_artist(a).is_empty();
        let mut forth: HashMap<ArtistId, HashMap<ArtistId, f32>> = HashMap::new();
        for e in &input.artists {
            if e.from == e.to || !held(e.from) || !held(e.to) {
                continue;
            }
            join(forth.entry(e.from).or_default().entry(e.to).or_insert(0.0), rank_weight(e.rank, e.of) * e.source.trust());
        }
        for (a, b, cos) in tag_likeness(&input.tags, &held) {
            join(forth.entry(a).or_default().entry(b).or_insert(0.0), TAG_WEIGHT * cos);
        }
        let artist = symmetric(forth, ARTIST_TOP);
        let mut g = Self { track, artist, prior: Vec::new() };
        g.prior = g.priors(catalog, input);
        g
    }

    /// Nothing known.
    pub fn is_empty(&self) -> bool {
        self.track.iter().all(Vec::is_empty) && self.artist.is_empty()
    }

    /// Edges of tracks and of artists.
    pub fn len(&self) -> (usize, usize) {
        (self.track.iter().map(Vec::len).sum(), self.artist.values().map(Vec::len).sum())
    }

    /// The neighbours of the track at `i`, the strongest first.
    pub fn near(&self, i: usize) -> &[(u32, f32)] {
        self.track.get(i).map_or(&[], Vec::as_slice)
    }

    /// The edge of the tracks at `a` and `b`.
    pub fn track(&self, a: usize, b: usize) -> f32 {
        self.near(a).iter().find(|(c, _)| *c as usize == b).map_or(0.0, |e| e.1)
    }

    /// The artists like `a`, the most first.
    pub fn similar_artists(&self, a: ArtistId) -> &[(ArtistId, f32)] {
        self.artist.get(&a).map_or(&[], Vec::as_slice)
    }

    /// The edge of artists `a` and `b`.
    pub fn artist(&self, a: ArtistId, b: ArtistId) -> f32 {
        self.similar_artists(a).iter().find(|(c, _)| *c == b).map_or(0.0, |e| e.1)
    }

    /// The prior of the track at `i`, 0…1 (0 with no graph around it).
    pub fn prior(&self, i: usize) -> f32 {
        self.prior.get(i).copied().unwrap_or(0.0)
    }

    /// How much of the graph meets on each track, its artist's and its
    /// popularity, ranked to 0…1 among the tracks with any.
    fn priors(&self, catalog: &Catalog, input: &GraphInput) -> Vec<f32> {
        let n = catalog.len();
        let mut central = vec![0.0f32; n];
        for row in &self.track {
            for &(b, w) in row {
                central[b as usize] += w;
            }
        }
        let mut artist_central: HashMap<ArtistId, f32> = HashMap::new();
        for row in self.artist.values() {
            for &(b, w) in row {
                *artist_central.entry(b).or_default() += w;
            }
        }
        let mut popular = vec![0.0f32; n];
        for (t, rank, of) in &input.popular {
            if let Some(i) = catalog.position(*t) {
                popular[i] = popular[i].max(rank_weight(*rank, *of));
            }
        }
        let raw: Vec<f32> = (0..n)
            .map(|i| {
                let a = catalog.tracks()[i].main_artist().and_then(|a| artist_central.get(&a)).copied().unwrap_or(0.0);
                central[i].ln_1p() + PRIOR_ARTIST * a.ln_1p() + PRIOR_POPULAR * popular[i]
            })
            .collect();
        let mut order: Vec<usize> = (0..n).filter(|&i| raw[i] > 0.0).collect();
        order.sort_by(|a, b| raw[*a].total_cmp(&raw[*b]).then(a.cmp(b)));
        let mut prior = vec![0.0f32; n];
        let m = order.len() as f32;
        for (k, &i) in order.iter().enumerate() {
            prior[i] = (k as f32 + 1.0) / m;
        }
        prior
    }
}

/// Pairs of artists `held` alike by their tags: the IDF cosine of the
/// tags' weights, from [`TAG_MIN`].
fn tag_likeness(tags: &[(ArtistId, String, f32)], held: &dyn Fn(ArtistId) -> bool) -> Vec<(ArtistId, ArtistId, f32)> {
    let mut by_artist: HashMap<ArtistId, HashMap<&str, f32>> = HashMap::new();
    for (a, t, w) in tags {
        if *w > 0.0 && held(*a) {
            let e = by_artist.entry(*a).or_default().entry(t.as_str()).or_insert(0.0);
            *e = e.max(*w);
        }
    }
    let n = by_artist.len() as f32;
    if n < 2.0 {
        return Vec::new();
    }
    let mut df: HashMap<&str, f32> = HashMap::new();
    for row in by_artist.values() {
        for t in row.keys() {
            *df.entry(t).or_default() += 1.0;
        }
    }
    // Each artist's vector: its tag's share of its strongest × IDF, unit
    // length.
    let mut vecs: Vec<(ArtistId, Vec<(&str, f32)>)> = by_artist
        .iter()
        .map(|(a, row)| {
            let top = row.values().copied().fold(0.0f32, f32::max).max(1e-6);
            let mut v: Vec<(&str, f32)> = row.iter().map(|(t, w)| (*t, (w / top) * (n / df[t]).ln())).filter(|(_, w)| *w > 0.0).collect();
            let len = v.iter().map(|(_, w)| w * w).sum::<f32>().sqrt().max(1e-6);
            for x in &mut v {
                x.1 /= len;
            }
            (*a, v)
        })
        .collect();
    vecs.sort_by_key(|(a, _)| *a);
    let mut postings: HashMap<&str, Vec<(usize, f32)>> = HashMap::new();
    for (k, (_, v)) in vecs.iter().enumerate() {
        for (t, w) in v {
            postings.entry(t).or_default().push((k, *w));
        }
    }
    let mut out = Vec::new();
    for (k, (a, v)) in vecs.iter().enumerate() {
        let mut dot: HashMap<usize, f32> = HashMap::new();
        for (t, w) in v {
            let Some(p) = postings.get(t).filter(|p| p.len() <= TAG_POSTINGS) else { continue };
            for &(o, x) in p {
                if o > k {
                    *dot.entry(o).or_default() += w * x;
                }
            }
        }
        for (o, c) in dot {
            if c >= TAG_MIN {
                out.push((*a, vecs[o].0, c.min(1.0)));
            }
        }
    }
    out.sort_by_key(|x| (x.0, x.1));
    out
}

/// The personal graph (see the module docs) over `catalog`: by catalog
/// position, the best [`TRACK_TOP`] of each track.
pub fn personal(catalog: &Catalog, events: &[Play], playlists: &[Vec<TrackId>]) -> Vec<Vec<(u32, f32)>> {
    let n = catalog.len();
    let mut forth: HashMap<u32, HashMap<u32, f32>> = HashMap::new();
    let mut add = |a: usize, b: usize, w: f32| {
        if a != b {
            *forth.entry(a as u32).or_default().entry(b as u32).or_insert(0.0) += w;
        }
    };
    // The sessions of an imported history.
    let mut starts: Vec<(UnixMs, usize)> = events
        .iter()
        .filter(|p| p.origin == Some(EventOrigin::Import) && p.kind == EventType::Start)
        .filter_map(|p| Some((p.ts, catalog.position(p.track)?)))
        .collect();
    starts.sort_unstable();
    let mut session: Vec<(UnixMs, usize)> = Vec::new();
    for (ts, i) in starts {
        if session.last().is_some_and(|(l, _)| ts - l >= IMPORT_GAP_MS) {
            session.clear();
        }
        for (k, &(_, s)) in session.iter().rev().take(3).enumerate() {
            add(s, i, 0.5f32.powi(k as i32));
        }
        session.push((ts, i));
    }
    // The order of the playlists.
    for list in playlists {
        let at: Vec<usize> = list.iter().filter_map(|t| catalog.position(*t)).collect();
        for (k, &a) in at.iter().enumerate() {
            for (d, &b) in at.iter().skip(k + 1).take(3).enumerate() {
                add(a, b, 0.5f32.powi(d as i32));
            }
        }
    }
    let rows = symmetric(forth, TRACK_TOP);
    let mut out = vec![Vec::new(); n];
    for (a, row) in rows {
        out[a as usize] = row;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;

    fn edge(a: u128, b: u128, rank: u32, source: GraphSource) -> Edge<TrackId> {
        Edge { from: tid(a), to: tid(b), rank, of: 25, source }
    }

    #[test]
    fn a_list_weighs_by_its_order_and_sources_add_up() {
        assert!(rank_weight(0, 25) > rank_weight(5, 25));
        assert!(rank_weight(24, 25) > 0.0);
        let cat = Catalog::new((0..6u128).map(|i| info(i, &[i as u32], None)));
        let input = GraphInput {
            tracks: vec![
                edge(0, 1, 0, GraphSource::YoutubeRadio),
                edge(0, 2, 10, GraphSource::YoutubeRadio),
                edge(0, 1, 2, GraphSource::LastFm),
                // Not in the catalog: left out.
                edge(0, 99, 0, GraphSource::YoutubeRadio),
            ],
            ..GraphInput::default()
        };
        let g = SimGraph::build(&cat, &input);
        let one = g.track(0, 1);
        assert!(one > g.track(0, 2), "{one} vs {}", g.track(0, 2));
        assert!(one > rank_weight(0, 25) * GraphSource::YoutubeRadio.trust() - 1e-6 && one <= 1.0);
        // Both ways, the edge back a little less.
        assert!((g.track(1, 0) - BACK * one).abs() < 1e-6);
        assert_eq!(g.near(0)[0].0, 1);
        assert_eq!(g.track(0, 3), 0.0);
        // The neighbours of the others meet on 0, 1 and 2.
        assert!(g.prior(0) > 0.0 && g.prior(3) == 0.0);
    }

    #[test]
    fn tags_link_artists_the_lists_do_not() {
        let cat = Catalog::new((0..8u128).map(|i| info(i, &[(i / 2) as u32], None)));
        let tag = |a: u32, t: &str, w: f32| (aid(a), t.to_owned(), w);
        let input = GraphInput {
            tags: vec![
                tag(0, "russian hip hop", 10.0),
                tag(0, "rap", 5.0),
                tag(1, "russian hip hop", 8.0),
                tag(1, "rap", 4.0),
                tag(2, "video game music", 9.0),
                tag(3, "video game music", 3.0),
                tag(3, "ambient", 3.0),
            ],
            artists: vec![Edge { from: aid(2), to: aid(3), rank: 0, of: 20, source: GraphSource::DeezerArtists }],
            ..GraphInput::default()
        };
        let g = SimGraph::build(&cat, &input);
        assert!(g.artist(aid(0), aid(1)) > 0.3, "{}", g.artist(aid(0), aid(1)));
        assert_eq!(g.artist(aid(0), aid(2)), 0.0);
        // The list and the tags together beat the list alone.
        assert!(g.artist(aid(2), aid(3)) > rank_weight(0, 20) * GraphSource::DeezerArtists.trust());
        assert_eq!(g.similar_artists(aid(1))[0].0, aid(0));
    }

    #[test]
    fn the_personal_graph_reads_imports_and_playlists_only() {
        let cat = Catalog::new((0..10u128).map(|i| info(i, &[1], None)));
        let start = |i: u128, ts: i64, origin: Option<EventOrigin>| Play { origin, ..Play::new(tid(i), ts, EventType::Start, Some(0)) };
        let events = vec![
            start(0, 0, Some(EventOrigin::Import)),
            start(1, 200_000, Some(EventOrigin::Import)),
            // A pause: another session.
            start(2, 200_000 + IMPORT_GAP_MS, Some(EventOrigin::Import)),
            // The app's own history is the model's pairs, not this graph.
            start(5, 300_000 + IMPORT_GAP_MS, Some(EventOrigin::Manual)),
            start(6, 400_000 + IMPORT_GAP_MS, Some(EventOrigin::Manual)),
        ];
        let p = personal(&cat, &events, &[vec![tid(7), tid(8), tid(9)]]);
        let w = |a: usize, b: usize| p[a].iter().find(|x| x.0 as usize == b).map_or(0.0, |x| x.1);
        assert!(w(0, 1) > 0.9 && w(1, 0) > 0.7);
        assert_eq!(w(1, 2), 0.0);
        assert_eq!(w(5, 6), 0.0);
        assert!(w(7, 8) > w(7, 9) && w(7, 9) > 0.0);
    }
}
