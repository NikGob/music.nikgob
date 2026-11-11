//! «Миксы для тебя» (recs-v2 §6, profiles-v1 §6): the five moments of
//! [`nikgob_recs::mixes`] — the profiles' own mixes, «Всё вперемешку», the
//! calm and the lively side of everything loved, «Фон» for whoever
//! listens so, the blends the user made today first — and «На повторе».
//! What an epoch of the home picked stays the epoch's (`recs.mixes` in the
//! library's cache: the lineup in its order, each mix's songs in theirs,
//! the epoch before's, the lineups shown before, the salts of «Другой
//! состав» and the blends of the day): a start of the app or the next
//! part of the day draws them again ([`crate::home_v2::Epoch`]).
//!
//! Before the moments stand «Микс дня», the genre mixes («Хип-хоп микс»,
//! built of the songs of one family of genres by the tags the graph of
//! similar music keeps for their artists), the mix of the part of the day
//! («На вечер»: [`Builder::pinned`], the epoch's as the moments are) and a
//! named mix or two («Микс: ZUTOMAYO»: [`nikgob_recs::mix_names`]). «Микс
//! дня», the genre mixes and the named ones ([`Builder::daily`]) are the
//! day's: seeded by the day and kept for it in `recs.day_mixes` (their
//! lineup, each one's songs, yesterday's to draw other ones), whatever is
//! played meanwhile and however often the app starts; a new day, a history
//! that appeared and «Только скачанное» switched draw them again.
//!
//! What of all that the row shows, in which order and how many, is
//! [`nikgob_recs::mixes::row`]'s: ten cards at most, no two alike. «Любимые»
//! is the playlist's own tile before the row (the page's).

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventType, TrackId};
use nikgob_core::streaming::StreamTrack;
use nikgob_recs::mixes::{Builder, Kept, MIN_SONGS, Mix, MixKey, Settings};
use nikgob_recs::mix_new::{self, NewCandidate};
use nikgob_recs::profiles::NameStyle;
use nikgob_recs::tiles::franchise;
use nikgob_recs::{Live, Session, TilesInput};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{BuildInput, CoverRef, Snapshot, cache_read, cache_write};

const MIXES_KEY: &str = "recs.mixes";
const DAY_MIXES_KEY: &str = "recs.day_mixes";
/// The day's mixes as kept: 2 is «Микс дня», the genre mixes and the named
/// ones (1 had no genre mixes, before it six numbered ones).
const DAY_MIXES_V: u8 = 2;
/// A song shorter than this is an interlude: no mix picks it.
const INTERLUDE_MS: u32 = 75_000;
/// About two hours of «На повторе».
const MIX_MS: u64 = 2 * 60 * 60 * 1000;
/// A song of unknown length counts so.
const GUESS_MS: u32 = 210_000;
/// A song longer than this is left out (they are skipped early a third of
/// the time, recs-v2 §2).
pub(super) const LONG_MS: u32 = nikgob_recs::mixes::LONG_MS;
/// «На повторе» looks this far back (30 once the history is longer).
const REPEAT_DAYS_SHORT: i64 = 14;
const REPEAT_DAYS_LONG: i64 = 30;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// One mix of «Миксы для тебя».
pub(crate) struct LaneHome {
    pub key: v::MixKey,
    pub title: String,
    /// Shorter forms of the title for a narrow card.
    pub short: Vec<String>,
    pub tracks: Vec<TrackId>,
    /// Familiar and loved: a new song of «С новым» stands between two.
    pub anchors: Vec<bool>,
    /// Its artists, most played first.
    pub artists: Vec<String>,
    pub covers: Vec<CoverRef>,
    /// «С новым»: new songs of the services that fit it.
    pub new: Vec<StreamTrack>,
    /// The profile «Смешать с…» starts from.
    pub blend_base: Option<u32>,
    /// Why it is offered ([`nikgob_recs::explain`]).
    pub why: Option<String>,
}

impl LaneHome {
    /// What it plays: its songs, with «С новым» its new ones each between
    /// two loved familiar songs (recs-v2 §5.4), never in the first two
    /// places nor two running, spread as evenly as such places allow.
    pub fn picks(&self, with_new: bool) -> Vec<nikgob_recs::Pick> {
        use nikgob_recs::{Item, Pick, Reason};
        let n = if with_new { self.new.len() } else { 0 };
        let len = self.tracks.len();
        // The anchors of every song (a mix kept from before may lack some).
        let anchors: Vec<bool> = (0..len).map(|k| self.anchors.get(k) == Some(&true)).collect();
        let after: HashSet<usize> = nikgob_recs::mix_new::places(&anchors, n).into_iter().collect();
        let mut out = Vec::with_capacity(len + after.len());
        let mut new = self.new.iter();
        for (k, t) in self.tracks.iter().enumerate() {
            out.push(Pick { item: Item::Track(*t), reason: Reason::Favourite });
            if after.contains(&k)
                && let Some(s) = new.next()
            {
                out.push(Pick { item: Item::Stream(s.id.clone()), reason: Reason::New });
            }
        }
        out
    }
}

/// What the cache keeps: the day and the epoch it is for, the settings it
/// was chosen under and the epoch's mixes.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MixesCache {
    day: String,
    #[serde(default)]
    epoch: String,
    settings: u64,
    kept: Kept,
    /// The mixes left out when it was chosen: one back takes its place in
    /// the lineup again.
    #[serde(default)]
    hidden: Vec<MixKey>,
}

/// The engine's key of a view's key («На повторе» and a world's mixes are the
/// runtime's).
pub(crate) fn engine_key(k: v::MixKey) -> Option<MixKey> {
    Some(match k {
        v::MixKey::Profile(p) => MixKey::Profile(p),
        v::MixKey::Everything => MixKey::Everything,
        v::MixKey::Calm => MixKey::Calm,
        v::MixKey::Lively => MixKey::Lively,
        v::MixKey::Background { profile, card, cards } => MixKey::Background { profile, card, cards },
        v::MixKey::Blend(p) => MixKey::Blend(p),
        v::MixKey::Kind(c) => MixKey::Kind(c),
        v::MixKey::Liked => MixKey::Liked,
        v::MixKey::Part(p) => MixKey::Part(p),
        v::MixKey::Daily(n) => MixKey::Daily(n),
        v::MixKey::Genre(f) => MixKey::Genre(f),
        v::MixKey::OnRepeat | v::MixKey::World(_) => return None,
    })
}

/// A moment of the epoch's lottery (or a blend): what «Миксы для тебя»
/// shows [`nikgob_recs::mixes::MOMENTS`] of; the mix of the part of the
/// day, «На повторе» and the mixes of the day are beside them.
pub(crate) fn is_moment(k: v::MixKey) -> bool {
    !matches!(k, v::MixKey::Liked | v::MixKey::Part(_) | v::MixKey::Daily(_) | v::MixKey::Genre(_) | v::MixKey::OnRepeat | v::MixKey::World(_))
}

/// Where a blend made now stands among `lanes`: after the mixes of the day
/// and of its part, the first of the moments.
pub(crate) fn blend_place(lanes: &[LaneHome]) -> usize {
    lanes.iter().position(|l| is_moment(l.key) || l.key == v::MixKey::OnRepeat).unwrap_or(lanes.len())
}

/// What the cache keeps of the mixes of the day: the day they are for,
/// what they were drawn under and the day's mixes.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DayMixesCache {
    /// What the day's mixes are ([`DAY_MIXES_V`]): another kind is drawn
    /// again.
    #[serde(default)]
    v: u8,
    day: String,
    /// Something was played, liked or kept when they were drawn.
    history: bool,
    local_only: bool,
    kept: Kept,
    /// «Преимущественно новые песни» they were drawn under
    /// ([`MixPrefs::unheard_code`]).
    #[serde(default)]
    unheard: u8,
}

fn view_key(k: MixKey) -> v::MixKey {
    match k {
        MixKey::Profile(p) => v::MixKey::Profile(p),
        MixKey::Everything => v::MixKey::Everything,
        MixKey::Calm => v::MixKey::Calm,
        MixKey::Lively => v::MixKey::Lively,
        MixKey::Background { profile, card, cards } => v::MixKey::Background { profile, card, cards },
        MixKey::Blend(p) => v::MixKey::Blend(p),
        MixKey::Kind(c) => v::MixKey::Kind(c),
        MixKey::Liked => v::MixKey::Liked,
        MixKey::Part(p) => v::MixKey::Part(p),
        MixKey::Daily(n) => v::MixKey::Daily(n),
        MixKey::Genre(f) => v::MixKey::Genre(f),
    }
}

/// The moments the prefs ask for.
pub(crate) fn settings(prefs: &MixPrefs) -> Settings {
    Settings {
        profiles: prefs.profiles,
        everything: prefs.everything,
        moods: prefs.moods,
        background_cards: usize::from(prefs.background_cards),
        names: match prefs.names {
            v::ProfileNames::Kind => NameStyle::Kind,
            v::ProfileNames::Artists => NameStyle::Artists,
        },
        hidden: prefs.hidden.clone(),
        unheard: unheard_share(prefs.unheard),
    }
}

/// The share of songs never played a mix takes at least under
/// «Преимущественно новые песни» (the radio's setting: the same words mean
/// the same in the mixes; less of it, each such song standing between two
/// loved ones).
fn unheard_share(u: v::RadioUnheard) -> f32 {
    match u {
        v::RadioUnheard::Usual => 0.0,
        v::RadioUnheard::More => nikgob_recs::mixes::UNHEARD_MORE,
        v::RadioUnheard::Only => nikgob_recs::mixes::UNHEARD_MOST,
    }
}

/// The prefs of «Миксы для тебя» a build reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MixPrefs {
    pub profiles: bool,
    pub everything: bool,
    pub moods: bool,
    pub on_repeat: bool,
    pub background_cards: u8,
    pub names: v::ProfileNames,
    /// «Не предлагать этот микс».
    pub hidden: Vec<MixKey>,
    /// «Преимущественно новые песни».
    pub unheard: v::RadioUnheard,
}

impl MixPrefs {
    pub fn of(p: &v::Prefs) -> Self {
        Self {
            profiles: p.mixes_profiles,
            everything: p.mixes_everything,
            moods: p.mixes_moods,
            on_repeat: p.mixes_on_repeat,
            background_cards: p.background_cards,
            names: p.profile_names,
            hidden: p.home_hidden_mixes.iter().filter_map(|h| engine_key(h.key)).collect(),
            unheard: p.radio_unheard,
        }
    }

    /// What tells a lineup chosen under other settings (a mix left out
    /// leaves the rest of the day's lineup as it is).
    fn fingerprint(&self) -> u64 {
        u64::from(self.profiles)
            | u64::from(self.everything) << 1
            | u64::from(self.moods) << 2
            | u64::from(self.background_cards) << 3
            | u64::from(self.names == v::ProfileNames::Artists) << 6
            | u64::from(self.unheard_code()) << UNHEARD_BITS
    }

    /// «Преимущественно новые песни» as a number (the cache keeps it: the
    /// songs chosen under another are chosen again).
    fn unheard_code(&self) -> u8 {
        match self.unheard {
            v::RadioUnheard::Usual => 0,
            v::RadioUnheard::More => 1,
            v::RadioUnheard::Only => 2,
        }
    }
}

/// Where [`MixPrefs::unheard_code`] stands in the fingerprint.
const UNHEARD_BITS: u32 = 7;

/// The tags other listeners gave an artist, as the graph of similar music
/// keeps them: what a genre mix is built by and a named one called by.
type ArtistTags = HashMap<ArtistId, Vec<(String, f32)>>;

fn artist_tags(lib: &nikgob_library::Library) -> ArtistTags {
    let mut tags = ArtistTags::new();
    for t in lib.graph_tags().unwrap_or_default() {
        if t.subject_kind == "artist"
            && let Ok(a) = t.subject_id.parse::<ArtistId>()
        {
            tags.entry(a).or_default().push((t.tag, t.weight));
        }
    }
    tags
}

/// The engine over `snap` for `session` at `now`, in the epoch of `seed`;
/// with `tags` it knows the genres ([`Builder::with_genres`]).
#[allow(clippy::too_many_arguments)]
fn with_builder<T>(
    snap: &Snapshot,
    prefs: &MixPrefs,
    fit: &[bool],
    ok: &dyn Fn(usize) -> bool,
    tags: Option<&ArtistTags>,
    session: &Session,
    now: nikgob_core::model::UnixMs,
    utc_offset_min: i32,
    seed: u64,
    f: impl FnOnce(&Builder<'_>) -> T,
) -> T {
    let recs = snap.recs();
    let live: Live = snap.model.live(session);
    let suits = |i: usize| fit.get(i).copied().unwrap_or(false);
    let (part, _, _) = super::daylist_key(now, utc_offset_min);
    let mut b = Builder::new(&recs, &snap.model, ok, &suits, seed, part, live, settings(prefs));
    if let Some(tags) = tags {
        b = b.with_genres(&|a| tags.get(&a).cloned().unwrap_or_default());
    }
    f(&b)
}

/// The cache for the epoch `epoch` of `day`: another epoch's moves on.
fn cache_for(lib: &nikgob_library::Library, prefs: &MixPrefs, day: &str, epoch: &str) -> MixesCache {
    let mut cache: MixesCache = cache_read(lib, MIXES_KEY).unwrap_or_default();
    if cache.epoch != epoch || cache.day != day {
        let same_day = cache.day == day;
        let kept = std::mem::take(&mut cache.kept).next_epoch(same_day);
        cache = MixesCache { day: day.to_owned(), epoch: epoch.to_owned(), settings: prefs.fingerprint(), kept, hidden: Vec::new() };
    }
    cache
}

/// Today's «Миксы для тебя» and the profiles «Смешать с…» offers.
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> (Vec<LaneHome>, Vec<v::BlendProfile>) {
    let lib = &input.lib;
    let prefs = &input.mix_prefs;
    let mut cache = cache_for(lib, prefs, &input.day, &input.epoch.key);
    if cache.settings != prefs.fingerprint() || cache.hidden.iter().any(|k| !prefs.hidden.contains(k)) {
        // Songs chosen under another «Преимущественно новые песни» are
        // chosen again (the epoch's mixes keep the lineup's places).
        if cache.settings >> UNHEARD_BITS != prefs.fingerprint() >> UNHEARD_BITS {
            cache.kept.songs.clear();
        }
        cache.settings = prefs.fingerprint();
        cache.kept.lineup.clear();
    }
    cache.hidden.clone_from(&prefs.hidden);
    if let Some(key) = input.refresh_mix.and_then(engine_key) {
        let salt = fastrand::u64(1..);
        match cache.kept.salts.iter_mut().find(|s| s.0 == key) {
            Some(s) => s.1 = salt,
            None => cache.kept.salts.push((key, salt)),
        }
    }
    for b in input.blends.iter().filter_map(|b| engine_key(*b)) {
        if !cache.kept.blends.contains(&b) {
            cache.kept.blends.push(b);
        }
    }
    // A mix takes what the tiles know (their length), what is not junk.
    let ok = |i: usize| {
        let t = &snap.catalog.tracks()[i];
        tiles.facts.contains_key(&t.id) && (t.local || !snap.local_only)
    };
    let names = |a: ArtistId| tiles.artist_names.get(&a).cloned();
    // The genres: a genre mix is built by them, and a moment that is a
    // genre's songs again is told by them.
    let tags = artist_tags(lib);
    let (mixes, lineup, blendable, pinned) = with_builder(snap, prefs, &snap.background, &ok, Some(&tags), &input.session, input.now, input.utc_offset_min, input.epoch.seed, |b| {
        let pinned = b.pinned(&cache.kept);
        let (mut mixes, lineup) = b.lineup(&cache.kept);
        b.named(&mut mixes, &names);
        (mixes, lineup, blend_profiles(snap, b, &names), pinned)
    });
    cache.kept.lineup = lineup;
    cache.kept.keep(&mixes, &snap.model);
    cache.kept.keep(&pinned, &snap.model);
    cache_write(lib, MIXES_KEY, input.now, &cache);
    // The named mixes take no name another card of the row has.
    let mut taken: Vec<String> = pinned.iter().chain(&mixes).map(|m| m.title.clone()).collect();
    taken.extend(["Любимые".to_owned(), "На повторе".to_owned()]);
    let daily = day_mixes(snap, input, tiles, &tags, &taken, &ok);
    // The row: «Микс дня», the genre mixes, the part of the day's, a
    // named mix or two, the moments, no two alike. A new song of «С
    // новым» goes to the first that fits it.
    let mut mixes: Vec<Mix> = nikgob_recs::mixes::row_upto(daily, pinned.into_iter().next(), mixes, input.row_more);
    log_lineup(snap, tiles, &mixes);

    let seed = input.epoch.seed;
    let log = super::novelty::keyed(input.new_log.clone(), snap);
    let mut used_new: HashSet<String> = HashSet::new();
    let cands = if snap.local_only { Vec::new() } else { candidates(snap, &input.new_pool, &input.new_sources, &input.new_log, seed) };
    let mut out: Vec<LaneHome> = mixes
        .drain(..)
        .enumerate()
        .map(|(k, m)| {
            let mut lane = lane_of(snap, tiles, &m);
            // «Фон» and «Любимые» are what they are: no new songs.
            if !snap.local_only && !matches!(m.key, MixKey::Background { .. } | MixKey::Liked) {
                lane.new = new_for(snap, input, &log, &cands, &m, seed.wrapping_add(k as u64), &mut used_new);
            }
            lane
        })
        .collect();
    if prefs.on_repeat
        && snap.model.taste.has_history()
        && let Some(tracks) = on_repeat(snap, tiles)
    {
        let anchors = vec![true; tracks.len()];
        let week = snap.model.taste.now() - 7 * DAY_MS;
        let starts = tiles.events.iter().filter(|p| p.ts >= week && p.kind == EventType::Start && tracks.contains(&p.track)).count();
        out.push(LaneHome {
            key: v::MixKey::OnRepeat,
            title: "На повторе".into(),
            short: Vec::new(),
            artists: artists_of(snap, tiles, &tracks),
            covers: covers(snap, &tracks),
            anchors,
            tracks,
            new: Vec::new(),
            blend_base: None,
            why: nikgob_recs::explain::on_repeat(starts as u32),
        });
    }
    (out, blendable)
}

/// «Миксы дня»: the day's as they were drawn, else drawn now and kept for
/// the day (see the module docs). «Другой состав» of one draws it again.
fn day_mixes(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput, tags: &ArtistTags, taken: &[String], ok: &dyn Fn(usize) -> bool) -> Vec<Mix> {
    let lib = &input.lib;
    let history = snap.model.taste.has_history();
    let mut cache: DayMixesCache = cache_read(lib, DAY_MIXES_KEY).unwrap_or_default();
    let unheard = input.mix_prefs.unheard_code();
    if cache.v != DAY_MIXES_V || cache.day != input.day || (history && !cache.history) || cache.local_only != snap.local_only || cache.unheard != unheard {
        // Another day's songs are the songs before: today's are others.
        let kept = std::mem::take(&mut cache.kept).next_epoch(false);
        cache = DayMixesCache { v: DAY_MIXES_V, day: input.day.clone(), history, local_only: snap.local_only, kept, unheard };
    }
    if let Some(key @ (MixKey::Daily(_) | MixKey::Genre(_))) = input.refresh_mix.and_then(engine_key) {
        let salt = fastrand::u64(1..);
        match cache.kept.salts.iter_mut().find(|s| s.0 == key) {
            Some(s) => s.1 = salt,
            None => cache.kept.salts.push((key, salt)),
        }
    }
    // The day's seed, not the epoch's: a start of the app changes nothing.
    let seed = super::day_seed(&input.day);
    // What names a mix: the tags other listeners gave its artists (a
    // genre mix is built by them), the artists' names, the years.
    let fact = |i: usize| tiles.facts.get(&snap.model.id(i));
    let decade = common_decade(tiles.facts.values().filter_map(|f| f.year));
    let (mixes, lineup) = with_builder(snap, &input.mix_prefs, &snap.background, ok, Some(tags), &Session::default(), input.now, input.utc_offset_min, seed, |b| {
        let (mut mixes, lineup) = b.daily(&cache.kept);
        let made = b.makeups(
            &mixes,
            &|a| tags.get(&a).cloned().unwrap_or_default(),
            &|a| tiles.artist_names.get(&a).cloned(),
            &|i| fact(i).and_then(|f| f.year),
            &|i| fact(i).map(|f| f.title.clone()).filter(|t| !t.trim().is_empty()),
        );
        // The lineup and the songs are kept as drawn; a mix nothing can
        // name, or one that is a genre's mix again, is only not shown.
        let drawn = mixes.clone();
        b.name_daily(&mut mixes, &made, taken, decade);
        (mixes, (drawn, lineup))
    });
    let (drawn, lineup) = lineup;
    cache.kept.lineup = lineup;
    cache.kept.keep(&drawn, &snap.model);
    cache_write(lib, DAY_MIXES_KEY, input.now, &cache);
    mixes
}

/// The decade over half of the dated songs of `years` are of: it tells no
/// mix apart.
fn common_decade(years: impl Iterator<Item = i32>) -> Option<i32> {
    let mut count: HashMap<i32, usize> = HashMap::new();
    let mut all = 0usize;
    for y in years.filter(|y| (1900..2100).contains(y)) {
        *count.entry(y / 10 * 10).or_default() += 1;
        all += 1;
    }
    count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0))).filter(|c| c.1 * 2 > all).map(|c| c.0)
}

/// A blend of `profiles` (the card's first), now, from the snapshot: it
/// plays at once and stands first until the day's end.
pub(crate) fn blend_now(snap: &Snapshot, prefs: &MixPrefs, profiles: &[u32], names: &dyn Fn(ArtistId) -> Option<String>, session: &Session, seed: u64) -> Option<LaneHome> {
    let key = MixKey::blend(profiles)?;
    let (now, utc_offset_min) = (super::recs_now(), super::local_day().1);
    let ok = |i: usize| snap.catalog.tracks()[i].local || !snap.local_only;
    let mix = with_builder(snap, prefs, &[], &ok, None, session, now, utc_offset_min, seed, |b| {
        let mut m = vec![b.mix(key, &Kept::default())?];
        b.named(&mut m, names);
        m.pop()
    })?;
    let artists: Vec<String> = mix.artists.iter().filter_map(|a| names(*a)).collect();
    let tracks: Vec<TrackId> = mix.tracks.iter().map(|&i| snap.model.id(i)).collect();
    Some(LaneHome {
        key: view_key(mix.key),
        title: mix.title,
        short: mix.short,
        covers: covers(snap, &tracks),
        anchors: mix.anchors,
        tracks,
        artists: dedupe(artists),
        new: Vec::new(),
        why: None,
        blend_base: None,
    })
}

/// Keeps the blend of `key` among the day's mixes at once (a restart
/// before the next build keeps it too).
pub(crate) fn remember_blend(lib: &nikgob_library::Library, prefs: &MixPrefs, day: &str, epoch: &str, now: nikgob_core::model::UnixMs, key: v::MixKey) {
    let Some(key) = engine_key(key) else { return };
    let mut cache = cache_for(lib, prefs, day, epoch);
    cache.kept.blends.retain(|b| *b != key);
    cache.kept.blends.push(key);
    cache_write(lib, MIXES_KEY, now, &cache);
}

/// The visible profiles with songs enough, by their names (the largest
/// share first).
fn blend_profiles(snap: &Snapshot, b: &Builder<'_>, names: &dyn Fn(ArtistId) -> Option<String>) -> Vec<v::BlendProfile> {
    let model = &snap.model;
    let mut order: Vec<usize> = (0..model.profiles.len()).filter(|&j| model.profiles.list[j].visible).collect();
    order.sort_by(|a, b| model.profile_share[*b].total_cmp(&model.profile_share[*a]).then(a.cmp(b)));
    let mut out = Vec::new();
    for j in order {
        let key = model.profiles.list[j].key;
        let Some(mut m) = b.mix(MixKey::Profile(key), &Kept::default()) else { continue };
        if m.tracks.len() < MIN_SONGS {
            continue;
        }
        b.named(std::slice::from_mut(&mut m), names);
        out.push(v::BlendProfile { key, title: m.title });
    }
    out
}

fn dedupe(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names.into_iter().filter(|n| seen.insert(n.to_lowercase())).take(12).collect()
}

/// The home's card of a mix.
fn lane_of(snap: &Snapshot, tiles: &TilesInput, m: &Mix) -> LaneHome {
    let tracks: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
    let artists = dedupe(m.artists.iter().filter_map(|a| tiles.artist_names.get(a).cloned()).collect());
    LaneHome {
        key: view_key(m.key),
        title: m.title.clone(),
        short: m.short.clone(),
        covers: covers(snap, &tracks),
        anchors: m.anchors.clone(),
        tracks,
        artists,
        new: Vec::new(),
        blend_base: m.key.blend_base(),
        // A blend is the user's own, «Любимые» says what it is.
        why: if matches!(m.key, MixKey::Blend(_) | MixKey::Liked) { None } else { snap.recs().why_moment(&m.tracks, &m.title) },
    }
}

/// The row as it is shown, in the log: each card, its first songs.
fn log_lineup(snap: &Snapshot, tiles: &TilesInput, mixes: &[Mix]) {
    let line: Vec<String> = mixes.iter().map(|m| format!("«{}» {}", m.title, m.tracks.len())).collect();
    log::info!("recs: mixes for you: {}", line.join(", "));
    for m in mixes {
        let first: Vec<String> = m
            .tracks
            .iter()
            .take(5)
            .filter_map(|&i| tiles.facts.get(&snap.model.id(i)).map(|f| format!("{} — {}", f.artist, f.title)))
            .collect();
        log::debug!("recs: mix «{}»: {}", m.title, first.join(" · "));
    }
}

/// The taste profile most of `tracks` are of.
fn profile_of(snap: &Snapshot, tracks: &[TrackId]) -> Option<usize> {
    let mut n: HashMap<usize, usize> = HashMap::new();
    for t in tracks {
        if let Some(&j) = snap.profiles.of.get(t) {
            *n.entry(j).or_default() += 1;
        }
    }
    n.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|x| x.0)
}

/// «На повторе»: the songs started at least twice over the last two weeks
/// (a month once the history is that long), what plays on repeat now first
/// (the hot affinity, then the count).
fn on_repeat(snap: &Snapshot, tiles: &TilesInput) -> Option<Vec<TrackId>> {
    let now = snap.model.taste.now();
    let first = tiles.events.iter().map(|p| p.ts).min()?;
    let days = if now - first >= 30 * DAY_MS { REPEAT_DAYS_LONG } else { REPEAT_DAYS_SHORT };
    let mut n: HashMap<TrackId, u32> = HashMap::new();
    for p in tiles.events.iter().filter(|p| p.ts >= now - days * DAY_MS) {
        match p.kind {
            EventType::Start => *n.entry(p.track).or_default() += 1,
            EventType::Complete => *n.entry(p.track).or_default() += 1,
            _ => {}
        }
    }
    let mut top: Vec<(f32, u32, TrackId)> = n
        .into_iter()
        .filter(|(t, c)| {
            *c >= 2
                && snap.catalog.get(*t).is_some_and(|i| !snap.model.taste.blocks(i) && (i.local || !snap.local_only))
                && tiles.facts.get(t).is_none_or(|f| f.duration_ms.is_none_or(|d| d >= INTERLUDE_MS))
        })
        .map(|(t, c)| (snap.catalog.position(t).map_or(0.0, |i| snap.model.taste.hot_at(i)), c, t))
        .collect();
    top.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut total = 0u64;
    let mut out = Vec::new();
    for (_, _, t) in top {
        if total >= MIX_MS {
            break;
        }
        total += u64::from(tiles.facts.get(&t).and_then(|f| f.duration_ms).unwrap_or(GUESS_MS));
        out.push(t);
    }
    (out.len() >= MIN_SONGS).then_some(out)
}

/// The main artists of `tracks`, by how many of them they have (then by
/// their first place), as the library writes them.
fn artists_of(snap: &Snapshot, tiles: &TilesInput, tracks: &[TrackId]) -> Vec<String> {
    let mut count: HashMap<ArtistId, (usize, usize)> = HashMap::new();
    for (k, t) in tracks.iter().enumerate() {
        if let Some(a) = snap.catalog.get(*t).and_then(|i| i.artists.first()) {
            let e = count.entry(*a).or_insert((0, k));
            e.0 += 1;
        }
    }
    let mut ranked: Vec<_> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    dedupe(ranked.into_iter().filter_map(|(a, _)| tiles.artist_names.get(&a).cloned()).collect())
}

/// The albums of `tracks` (a mix's songs in their order) for its cover,
/// each once: the leading artist's first (the one with most songs, the
/// earlier on a tie), their album with most of the songs before the
/// others; then the rest the same way.
fn cover_order(songs: &[(Option<ArtistId>, Option<nikgob_core::model::ReleaseId>)]) -> Vec<nikgob_core::model::ReleaseId> {
    let mut by_artist: HashMap<Option<ArtistId>, (usize, usize)> = HashMap::new();
    let mut by_album: HashMap<nikgob_core::model::ReleaseId, (usize, usize, Option<ArtistId>)> = HashMap::new();
    for (k, (artist, album)) in songs.iter().enumerate() {
        by_artist.entry(*artist).or_insert((0, k)).0 += 1;
        if let Some(r) = album {
            by_album.entry(*r).or_insert((0, k, *artist)).0 += 1;
        }
    }
    let mut albums: Vec<_> = by_album.into_iter().collect();
    albums.sort_by(|a, b| {
        let (x, y) = (by_artist[&a.1.2], by_artist[&b.1.2]);
        y.0.cmp(&x.0).then(x.1.cmp(&y.1)).then(b.1.0.cmp(&a.1.0)).then(a.1.1.cmp(&b.1.1))
    });
    albums.into_iter().map(|a| a.0).collect()
}

/// Covers for a mix's card: its leading artist's album first
/// ([`cover_order`]).
fn covers(snap: &Snapshot, tracks: &[TrackId]) -> Vec<CoverRef> {
    let songs: Vec<_> = tracks.iter().filter_map(|t| snap.catalog.get(*t)).map(|i| (i.artists.first().copied(), i.release)).collect();
    let mut out: Vec<CoverRef> = Vec::new();
    for r in cover_order(&songs) {
        let c = CoverRef::Release(r);
        if !out.contains(&c) {
            out.push(c);
            if out.len() == super::COVER_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// What a song the graph found beside the library's weighs against a
/// discovery of the same rank (the pass ranked those with the user model).
const GRAPH_WEIGHT: f32 = 0.5;

/// The new songs the cards of the row may take ([`NewCandidate`]): every
/// discovery the pass kept, then the songs the graph of similar music found
/// beside the library's (new indeed and no junk, each once), each weighed
/// by its rank and by how new music of its source is received (one draw of
/// [`nikgob_recs::NewLog::source_shares`] for the epoch; the graph's songs
/// are YouTube Music's radios, times the source's trust); its kind of song
/// told by its preview where it was analysed, the library's tracks the
/// graph links it to.
pub(super) fn candidates(snap: &Snapshot, pool: &[StreamTrack], sources: &HashMap<String, nikgob_recs::Source>, log: &nikgob_recs::NewLog, seed: u64) -> Vec<NewCandidate> {
    use nikgob_recs::{GraphSource, Source, stream_song_key};
    let shares = log.source_shares(&mut nikgob_recs::rng::Rng::new(seed ^ 0x736f_7572_6365));
    let share = |s: Option<Source>| s.and_then(|s| shares.get(&s).copied()).unwrap_or(1.0 / nikgob_recs::novelty::SOURCES.len() as f32);
    let graph: HashMap<String, &super::graph::GraphFind> = snap.graph_new.iter().map(|f| (stream_song_key(&f.track), f)).collect();
    let near = |key: &str| -> Vec<usize> { graph.get(key).map(|f| f.near.iter().filter_map(|(t, _)| snap.model.position(*t)).collect()).unwrap_or_default() };
    let regions = &snap.model.regions;
    let candidate = |t: &StreamTrack, near: Vec<usize>, weight: f32| {
        let vector = snap.new_track(t).vector;
        // Its voice by its preview, else by the songs whose radios hold it.
        let voice = match vector.as_ref() {
            Some(v) => mix_new::voice_near(&snap.model, &snap.index, v),
            None => mix_new::voice_of(&snap.model, &near),
        };
        NewCandidate {
            track: t.clone(),
            lane: mix_new::lane_of_new(&snap.types, voice, t),
            soundtrack: t.album.as_ref().is_some_and(|a| franchise(&a.title).is_some()),
            profile: vector.map(|v| snap.model.embed(&v)).and_then(|e| regions.of(&e, None)),
            near,
            weight,
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let found: Vec<&StreamTrack> = pool.iter().filter(|t| snap.is_new(t)).collect();
    let len = found.len().max(1) as f32;
    for (k, t) in found.into_iter().enumerate() {
        let key = stream_song_key(t);
        if !seen.insert(key.clone()) {
            continue;
        }
        // The best found first.
        let weight = (1.0 - 0.5 * k as f32 / len) * share(sources.get(&key).copied());
        out.push(candidate(t, near(&key), weight));
    }
    let trust = share(Some(Source::YoutubeRadio)) * GraphSource::YoutubeRadio.trust();
    let rest: Vec<&super::graph::GraphFind> = snap.graph_new.iter().filter(|f| !seen.contains(&stream_song_key(&f.track))).collect();
    let len = rest.len().max(1) as f32;
    for (k, f) in rest.into_iter().enumerate() {
        let weight = GRAPH_WEIGHT * (1.0 - 0.5 * k as f32 / len) * trust;
        out.push(candidate(&f.track, f.near.iter().filter_map(|(t, _)| snap.model.position(*t)).collect(), weight));
    }
    out
}

/// «С новым»: the new songs of `cands` that fit the mix
/// ([`mix_new::choose`]), each in one mix only, as many as the appetite of
/// the mix's taste profile asks and the user's share of new music at least
/// ([`mix_new::want`]). «Фон» takes none.
fn new_for(snap: &Snapshot, input: &BuildInput, log: &nikgob_recs::NewLog, cands: &[NewCandidate], m: &Mix, seed: u64, used: &mut HashSet<String>) -> Vec<StreamTrack> {
    let tracks: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
    let profile = profile_of(snap, &tracks);
    let appetite = (!log.events.is_empty() && !snap.model.profiles.is_empty()).then(|| {
        let key = profile.and_then(|j| snap.model.profiles.list.get(j)).map(|p| p.key);
        let background = profile.is_some_and(|j| snap.model.is_background_profile(j));
        log.share(key, background, &mut nikgob_recs::rng::Rng::new(seed))
    });
    let want = mix_new::want(tracks.len(), appetite, input.new_share);
    mix_new::choose(&snap.model, m, cands, want, used).into_iter().map(|c| c.track.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The liked songs' mix, the mix of a part of the day, a mix of the
    /// day and a genre mix keep their keys both ways and are no moments of
    /// the lottery, and a genre mix is kept with the day's.
    #[test]
    fn the_mixes_beside_the_moments_keep_their_keys() {
        for k in [v::MixKey::Liked, v::MixKey::Part(2), v::MixKey::Daily(0), v::MixKey::Daily(3), v::MixKey::Genre(7), v::MixKey::Calm, v::MixKey::Profile(7)] {
            assert_eq!(engine_key(k).map(view_key), Some(k));
        }
        assert!(is_moment(v::MixKey::Calm) && is_moment(v::MixKey::Blend([1, 2, 0])) && is_moment(v::MixKey::Profile(1)));
        for k in [v::MixKey::Liked, v::MixKey::Part(0), v::MixKey::Daily(1), v::MixKey::Genre(7), v::MixKey::OnRepeat] {
            assert!(!is_moment(k), "{k:?}");
        }
        assert!(v::MixKey::Daily(2).is_daily() && v::MixKey::Genre(7).is_daily() && !v::MixKey::Liked.is_daily());
        let json = serde_json::to_string(&Kept { lineup: vec![MixKey::Daily(0), MixKey::Genre(7)], ..Kept::default() }).unwrap();
        assert_eq!(serde_json::from_str::<Kept>(&json).unwrap().lineup, [MixKey::Daily(0), MixKey::Genre(7)]);
    }

    /// A library where one franchise is the most loved (as MADE IN ABYSS
    /// among soundtracks): no mix plays it more than twice running nor
    /// over a quarter, and none takes a song over nine minutes.
    #[test]
    fn a_lane_has_no_run_of_one_world() {
        use nikgob_core::model::{ArtistId, ReleaseId};
        use nikgob_recs::{Catalog, DiscoveryFilter, ModelInput, SongTypes, TasteInput, TrackFacts, TrackInfo, UserModel, VectorIndex};
        let ids: Vec<TrackId> = (0..80).map(|_| TrackId::new()).collect();
        let albums: Vec<ReleaseId> = (0..12).map(|_| ReleaseId::new()).collect();
        let tracks: Vec<TrackInfo> = ids
            .iter()
            .enumerate()
            .map(|(k, id)| {
                let (world, album) = if k < 40 { ("made in abyss".to_owned(), albums[k % 4]) } else { (format!("world {}", k % 8), albums[4 + k % 8]) };
                let ms = if k == 0 { 10 * 60_000 } else { 180_000 };
                TrackInfo::new(*id, vec![ArtistId::new()], Some(album)).with_world(&world).with_duration(Some(ms))
            })
            .collect();
        let catalog = Catalog::new(tracks);
        let mut index = VectorIndex::new();
        for (k, id) in ids.iter().enumerate() {
            let mut v = [0.0f32; nikgob_recs::DIM];
            v[0] = (k % 40) as f32;
            v[1] = (k / 40) as f32;
            index.insert(*id, v);
        }
        let input = TasteInput { likes: ids[..40].to_vec(), ..TasteInput::default() };
        let model = UserModel::build(&catalog, &input, ModelInput { index: &index, types: None, weights: None, graph: None }, 1_000 * DAY_MS);
        let snap = Snapshot {
            model,
            types: SongTypes::default(),
            catalog,
            index,
            filter: DiscoveryFilter::default(),
            artist_keys: HashMap::new(),
            known_albums: HashSet::new(),
            played_songs: HashSet::new(),
            suggested_new: Vec::new(),
            local_only: false,
            worlds: HashMap::new(),
            new_vectors: HashMap::new(),
            artist_affinity: HashMap::new(),
            profiles: Default::default(),
            background: Vec::new(),
            graph_new: Vec::new(),
        };
        let mut tiles = TilesInput::default();
        for (k, id) in ids.iter().enumerate() {
            let ms = if k == 0 { 10 * 60_000 } else { 180_000 };
            tiles.facts.insert(*id, TrackFacts { duration_ms: Some(ms), ..TrackFacts::default() });
        }
        let prefs = MixPrefs::of(&v::Prefs::default());
        let ok = |i: usize| tiles.facts.contains_key(&snap.catalog.tracks()[i].id);
        let mixes = with_builder(&snap, &prefs, &[], &ok, None, &Session::default(), 1_000 * DAY_MS, 0, 1, |b| b.lineup(&Kept::default()).0);
        assert!(!mixes.is_empty());
        for m in &mixes {
            let picked: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
            assert!(picked.len() >= MIN_SONGS, "«{}»: {}", m.title, picked.len());
            assert!(!picked.contains(&ids[0]), "ten minutes is too long");
            let mia: Vec<bool> = picked.iter().map(|t| ids[..40].contains(t)).collect();
            assert!(mia.iter().filter(|m| **m).count() * 4 <= picked.len() + 3, "«{}», a quarter: {mia:?}", m.title);
            let worlds: Vec<Option<u64>> = picked.iter().map(|t| snap.catalog.get(*t).and_then(|i| i.world)).collect();
            for w in worlds.windows(3) {
                assert!(!(w[0] == w[1] && w[1] == w[2]), "«{}», a world thrice running: {worlds:?}", m.title);
            }
        }
    }

    /// «Миксы для тебя» of a real library (`NIKGOB_REAL_LIBRARY`, a copy of
    /// it is read): the lineup of each part of the day with its first
    /// songs, the rules of diversity each mix keeps, its energy by quarters,
    /// and what of it the next day keeps.
    #[test]
    #[ignore = "needs NIKGOB_REAL_LIBRARY"]
    fn the_mixes_of_a_real_library() {
        use nikgob_recs::DayPart;
        let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else { return };
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite");
        std::fs::copy(&src, &db).unwrap();
        for ext in ["-wal", "-shm"] {
            let mut side = src.clone();
            side.push(ext);
            if std::path::Path::new(&side).exists() {
                let mut to = db.clone().into_os_string();
                to.push(ext);
                std::fs::copy(&side, to).unwrap();
            }
        }
        let lib = nikgob_library::Library::open(&db).unwrap();
        let index = crate::analysis::SoundIndex::default();
        crate::analysis::load_index(&lib, &index);
        let now = super::super::recs_now();
        let (snap, tiles, _) = super::super::read_snapshot(&lib, &index, now, 180, &[], false).unwrap();
        let recs = snap.recs();
        let suits = |i: usize| snap.background.get(i).copied().unwrap_or(false);
        let ok = |i: usize| tiles.facts.contains_key(&snap.catalog.tracks()[i].id);
        let names = |a: ArtistId| tiles.artist_names.get(&a).cloned();
        let epoch = crate::home_v2::Epoch::of(7, now, 180).seed;
        let song = |i: usize| tiles.facts.get(&snap.model.id(i)).map_or_else(String::new, |f| format!("{} — {}", f.artist, f.title));
        let shares: Vec<String> = snap.model.profiles.list.iter().zip(&snap.model.profile_share).map(|(p, s)| format!("{:?} {:.2}", p.lane, s)).collect();
        println!("background listener: {}; profiles: {}", snap.model.listening.has_background, shares.join(", "));
        let mut broken = 0;
        let mut kept_day = Kept::default();
        for part in DayPart::ALL {
            let b = Builder::new(&recs, &snap.model, &ok, &suits, epoch, part, snap.model.live(&Session::default()), Settings::default());
            let (mut mixes, lineup) = b.lineup(&Kept::default());
            b.named(&mut mixes, &names);
            println!("{part:?}: {}", mixes.iter().map(|m| format!("«{}»", m.title)).collect::<Vec<_>>().join(", "));
            for m in &mixes {
                let info = |i: usize| &snap.catalog.tracks()[i];
                let world_runs = m.tracks.windows(3).filter(|w| info(w[0]).world.is_some() && info(w[0]).world == info(w[1]).world && info(w[1]).world == info(w[2]).world).count();
                let artist_near = (0..m.tracks.len())
                    .filter(|&k| {
                        let a = info(m.tracks[k]).artists.first();
                        (k.saturating_sub(2)..k).any(|j| info(m.tracks[j]).artists.first() == a)
                    })
                    .count();
                for k in 0..m.tracks.len() {
                    let a = info(m.tracks[k]).artists.first();
                    if let Some(j) = (k.saturating_sub(2)..k).find(|&j| info(m.tracks[j]).artists.first() == a) {
                        println!("    «{}» {j}: {} / {k}: {}", m.title, song(m.tracks[j]), song(m.tracks[k]));
                    }
                }
                let mut worlds: HashMap<u64, usize> = HashMap::new();
                for &i in &m.tracks {
                    if let Some(w) = info(i).world {
                        *worlds.entry(w).or_default() += 1;
                    }
                }
                let top = worlds.values().copied().max().unwrap_or(0) as f32 / m.tracks.len() as f32;
                let e: Vec<f32> = m.tracks.iter().filter_map(|&i| snap.model.energy_at(i)).collect();
                let mut sorted = e.clone();
                sorted.sort_by(f32::total_cmp);
                let pct = |x: f32| sorted.iter().filter(|y| **y < x).count() as f32 / sorted.len().max(1) as f32;
                let quarters: Vec<String> = e.chunks(e.len().div_ceil(4).max(1)).map(|c| format!("{:.2}", c.iter().map(|x| pct(*x)).sum::<f32>() / c.len() as f32)).collect();
                broken += world_runs + artist_near + usize::from(top > 0.25 + 1.0 / m.tracks.len() as f32);
                if part == DayPart::Day {
                    let artists: Vec<String> = m.artists.iter().take(4).filter_map(|a| names(*a)).collect();
                    println!("  «{}» · {} · {} songs · worlds {} (top {:.0}%) · world thrice {world_runs} · artist within 3 {artist_near} · energy {}", m.title, artists.join(", "), m.tracks.len(), worlds.len(), top * 100.0, quarters.join(" "));
                    for &i in m.tracks.iter().take(10) {
                        println!("      {}", song(i));
                    }
                } else {
                    println!("  «{}» energy {} · world thrice {world_runs} · artist within 3 {artist_near}", m.title, quarters.join(" "));
                }
            }
            if part == DayPart::Day {
                kept_day.lineup = lineup;
                kept_day.keep(&mixes, &snap.model);
            }
        }
        // The next epoch: what of each mix stays.
        let next = kept_day.clone().next_epoch(true);
        let b = Builder::new(&recs, &snap.model, &ok, &suits, epoch ^ 1, DayPart::Day, Live::default(), Settings::default());
        let (mut mixes, _) = b.lineup(&next);
        b.named(&mut mixes, &names);
        for m in &mixes {
            let before: HashSet<TrackId> = kept_day.songs.iter().find(|t| t.0 == m.key).map(|t| t.2.iter().copied().collect()).unwrap_or_default();
            let same = m.tracks.iter().filter(|&&i| before.contains(&snap.model.id(i))).count();
            println!("next epoch «{}»: {same} of {} kept", m.title, m.tracks.len());
        }
        assert_eq!(broken, 0, "rules broken");
    }

    /// A mix's cover is its leading artist's album with most of its songs;
    /// the decade most of a library is of names no mix.
    #[test]
    fn the_cover_is_the_leading_artists_album() {
        use nikgob_core::model::ReleaseId;
        let (a, b) = (ArtistId::new(), ArtistId::new());
        let (a1, a2, b1) = (ReleaseId::new(), ReleaseId::new(), ReleaseId::new());
        let songs = [(Some(b), Some(b1)), (Some(a), Some(a1)), (Some(a), Some(a2)), (Some(a), Some(a2)), (Some(b), None), (None, None)];
        assert_eq!(cover_order(&songs), [a2, a1, b1]);
        assert!(cover_order(&[]).is_empty());
        assert_eq!(common_decade([2021, 2022, 2015, 1999, 3].into_iter()), None);
        assert_eq!(common_decade([2021, 2022, 2023, 2015, 1999].into_iter()), Some(2020));
        assert_eq!(common_decade(std::iter::empty()), None);
    }

    /// «С новым» takes as many new songs as its share asks, each between
    /// two loved familiar songs, never in the first two places nor two
    /// running.
    #[test]
    fn new_songs_stand_between_loved_ones() {
        use nikgob_core::streaming::{RemoteId, Service};
        use nikgob_recs::Item;
        use nikgob_recs::mix_new::new_count;
        assert_eq!(new_count(36, 0.1), 4);
        assert_eq!(new_count(36, 0.2), 9);
        assert_eq!(new_count(3, 0.05), 1);
        let new: Vec<StreamTrack> = (0..4)
            .map(|k| StreamTrack::new(RemoteId { service: Service::Youtube, id: format!("v{k}") }, format!("New {k}")))
            .collect();
        let anchors: Vec<bool> = (0..28).map(|k| k % 5 != 3).collect();
        let lane = LaneHome {
            key: v::MixKey::Calm,
            title: "Спокойное".into(),
            short: Vec::new(),
            tracks: (0..28).map(|_| TrackId::new()).collect(),
            anchors: anchors.clone(),
            artists: Vec::new(),
            covers: Vec::new(),
            new,
            blend_base: None,
            why: None,
        };
        let picks = lane.picks(true);
        let at: Vec<usize> = picks.iter().enumerate().filter(|(_, p)| matches!(p.item, Item::Stream(_))).map(|(i, _)| i).collect();
        assert_eq!(at.len(), 4, "{at:?}");
        for (k, &i) in at.iter().enumerate() {
            assert!(i >= 2, "{at:?}");
            // The songs around it: before it k new ones stand.
            let before = i - 1 - k;
            assert!(anchors[before] && anchors[before + 1], "{at:?}");
        }
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= 4, "spread: {at:?}");
        }
        assert_eq!(lane.picks(false).len(), 28);
    }
}
