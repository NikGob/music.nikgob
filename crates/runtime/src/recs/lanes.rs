//! «Миксы для тебя»: the five moments of
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
//!
//! «С новым» goes by the open world ([`nikgob_recs::mix_new::take`]): between loved songs and a third at most
//! under «Сбалансировано»; as much as asked, anywhere after the first song,
//! under «Больше нового»; a niche (a mix of few songs or artists) takes
//! mostly new music of its sound, its songs the seeds of its top-up
//! ([`super::niche`]).

use std::collections::{HashMap, HashSet};

use nikgob_core::i18n::{Locale, TileStatic, nav_in};
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
/// the time).
pub(super) const LONG_MS: u32 = nikgob_recs::mixes::LONG_MS;
/// «На повторе» looks this far back (30 once the history is longer).
const REPEAT_DAYS_SHORT: i64 = 14;
const REPEAT_DAYS_LONG: i64 = 30;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub(crate) use nikgob_core::i18n::PerLocale;

/// Localized display text for a [`LaneHome`] card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LaneText {
    pub title: String,
    pub short: Vec<String>,
}

/// All output of [`build`]: today's lanes plus blend-profile metadata for live relocalization.
pub(crate) struct BuiltLanes {
    pub lanes: Vec<LaneHome>,
    pub blend_profiles: Vec<v::BlendProfile>,
    pub blend_titles: Vec<(u32, PerLocale<String>)>,
}

/// One mix of «Миксы для тебя».
pub(crate) struct LaneHome {
    pub key: v::MixKey,
    pub title: String,
    /// Shorter forms of the title for a narrow card.
    pub short: Vec<String>,
    /// Precomputed localized titles and short forms for all supported locales.
    pub labels: PerLocale<LaneText>,
    /// Composition flavor for `Calm` / `Lively` side mixes so titles and wave/shuffle
    /// origins remain typed and dynamically localizable across restarts.
    pub side_flavor: Option<nikgob_recs::mixes::SideFlavor>,
    pub tracks: Vec<TrackId>,
    /// Familiar and loved: a new song of «С новым» stands between two.
    pub anchors: Vec<bool>,
    /// Its artists, most played first.
    pub artists: Vec<String>,
    pub covers: Vec<CoverRef>,
    /// «С новым»: new songs of the services that fit it.
    pub new: Vec<StreamTrack>,
    /// They stand anywhere after the first song (the open world), not only
    /// between two loved ones.
    pub open: bool,
    /// A niche: its songs, the seeds of its top-up (empty for the others).
    pub niche: Vec<TrackId>,
    /// The profile «Смешать с…» starts from.
    pub blend_base: Option<u32>,
    /// Why it is offered ([`nikgob_recs::explain`]).
    pub why: Option<String>,
}

impl LaneHome {
    /// Invariant Russian title used as canonical analytics key (`mix:{title}`).
    pub fn ru_title(&self) -> &str {
        &self.labels.ru.title
    }

    /// Updates visible `title` and `short` synchronously for `loc`.
    pub fn relocalize(&mut self, loc: Locale) {
        let text = self.labels.get(loc);
        self.title.clone_from(&text.title);
        self.short.clone_from(&text.short);
    }

    /// What it plays: its songs, with «С новым» its new ones each between
    /// two loved familiar songs, never in the first two
    /// places nor two running, spread as evenly as such places allow; in
    /// the open world evenly anywhere after the first song, runs of them
    /// where the share asks ([`nikgob_recs::mix_new::spread`]).
    pub fn picks(&self, with_new: bool) -> Vec<nikgob_recs::Pick> {
        use nikgob_recs::{Item, Pick, Reason};
        let n = if with_new { self.new.len() } else { 0 };
        let len = self.tracks.len();
        // The anchors of every song (a mix kept from before may lack some).
        let anchors: Vec<bool> = (0..len).map(|k| self.anchors.get(k) == Some(&true)).collect();
        let mode = if self.open { mix_new::NewMode::Open } else { mix_new::NewMode::Calm };
        let mut after: HashMap<usize, usize> = HashMap::new();
        for k in mix_new::placements(mode, &anchors, n) {
            *after.entry(k).or_default() += 1;
        }
        let mut out = Vec::with_capacity(len + n);
        let mut new = self.new.iter();
        for (k, t) in self.tracks.iter().enumerate() {
            out.push(Pick { item: Item::Track(*t), reason: Reason::Favourite });
            for _ in 0..after.get(&k).copied().unwrap_or(0) {
                if let Some(s) = new.next() {
                    out.push(Pick { item: Item::Stream(s.id.clone()), reason: Reason::New });
                }
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
pub(crate) fn settings(prefs: &MixPrefs, loc: Locale) -> Settings {
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
        locale: loc,
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
    /// The cards play new music («С новым», not «Только скачанное»): the
    /// niches get cards of their own.
    pub with_new: bool,
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
            with_new: p.mixes_with_new && !p.mixes_downloaded_only,
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
    loc: Locale,
    f: impl FnOnce(&Builder<'_>) -> T,
) -> T {
    let recs = snap.recs();
    let live: Live = snap.model.live(session);
    let suits = |i: usize| fit.get(i).copied().unwrap_or(false);
    let (part, _, _) = super::daylist_key(now, utc_offset_min);
    let mut b = Builder::new(&recs, &snap.model, ok, &suits, seed, part, live, settings(prefs, loc)).with_locale(loc);
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

fn mixes_labels(b: &Builder<'_>, mixes: &[Mix], names: &dyn Fn(ArtistId) -> Option<String>, loc: Locale) -> HashMap<MixKey, LaneText> {
    let mut copy = mixes.to_vec();
    for m in &mut copy {
        if !matches!(m.key, MixKey::Profile(_) | MixKey::Blend(_)) {
            m.title = b.title_in(m.key, &m.tracks, loc);
        }
    }
    b.named_in(&mut copy, loc, names);
    copy.into_iter()
        .map(|m| (m.key, LaneText { short: b.short_titles_in(m.key, loc), title: m.title }))
        .collect()
}

/// Today's «Миксы для тебя» and the profiles «Смешать с…» offers.
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> BuiltLanes {
    let lib = &input.lib;
    let prefs = &input.mix_prefs;
    let loc = input.locale;
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
    let wants_niches = prefs.with_new && !snap.local_only && input.new_share > 0.0;
    let (mixes, lineup, (blendable, blend_titles), pinned, niches, mut labels_for) = with_builder(
        snap,
        prefs,
        &snap.background,
        &ok,
        Some(&tags),
        &input.session,
        input.now,
        input.utc_offset_min,
        input.epoch.seed,
        loc,
        |b| {
            let pinned = b.pinned(&cache.kept);
            let (mut mixes, lineup) = b.lineup(&cache.kept);
            b.named_in(&mut mixes, loc, &names);
            let mut labels_for = PerLocale::init(|l| {
                let mut map = mixes_labels(b, &mixes, &names, l);
                for m in &pinned {
                    map.insert(m.key, LaneText { title: b.title_in(m.key, &m.tracks, l), short: b.short_titles_in(m.key, l) });
                }
                map
            });
            // The niches the row may hold a card of, with their titles.
            let niches: Vec<(nikgob_recs::open_world::NicheCard, String, Vec<String>)> = if wants_niches {
                let genre = |i: usize| b.genre_at(i);
                let background = |i: usize| snap.background.get(i).copied().unwrap_or(false);
                let none = |_: &StreamTrack| Vec::new();
                let ctx = nikgob_recs::open_world::WaveContext { model: &snap.model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &none, index: Some(&snap.index), learned: None };
                nikgob_recs::open_world::niches(&ctx)
                    .into_iter()
                    .filter(|c| !prefs.hidden.contains(&c.key))
                    .map(|c| {
                        for l in Locale::ALL {
                            labels_for.get_mut(l).insert(
                                c.key,
                                LaneText { title: b.title_in(c.key, &c.seeds, l), short: b.short_titles_in(c.key, l) },
                            );
                        }
                        let title = b.title_in(c.key, &c.seeds, loc);
                        let short = b.short_titles_in(c.key, loc);
                        (c, title, short)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (mixes, lineup, blend_profiles(snap, b, &names, loc), pinned, niches, labels_for)
        },
    );
    cache.kept.lineup = lineup;
    cache.kept.keep(&mixes, &snap.model);
    cache.kept.keep(&pinned, &snap.model);
    cache_write(lib, MIXES_KEY, input.now, &cache);
    // The named mixes take no name another card of the row has.
    let taken_for = PerLocale::init(|l| {
        let mut taken: Vec<String> = pinned
            .iter()
            .chain(&mixes)
            .map(|m| labels_for.get(l).get(&m.key).map_or_else(|| m.title.clone(), |t| t.title.clone()))
            .collect();
        taken.extend([nav_in(l).liked.to_owned(), TileStatic::OnRepeat.title_in(l).to_owned()]);
        taken
    });
    let (daily, daily_labels) = day_mixes(snap, input, tiles, &tags, &taken_for, &ok);
    for l in Locale::ALL {
        labels_for.get_mut(l).extend(daily_labels.get(l).iter().map(|(k, v)| (*k, v.clone())));
    }
    // The row: «Микс дня», the genre mixes, the part of the day's, a
    // named mix or two, the moments, no two alike. A new song of «С
    // новым» goes to the first that fits it.
    let mut mixes: Vec<Mix> = nikgob_recs::mixes::row_upto(daily, pinned.into_iter().next(), mixes, input.row_more);
    // The niches: the calm songs in place of the calm side, a small kind or
    // genre a card of its own.
    mix_new::with_niches(&snap.model, &mut mixes, niches);
    log_lineup(snap, tiles, &mixes);

    let seed = input.epoch.seed;
    let log = super::novelty::keyed(input.new_log.clone(), snap);
    let mut used_new: HashSet<String> = HashSet::new();
    let cands = if snap.local_only { Vec::new() } else { candidates(snap, &input.new_pool, &input.new_sources, &input.new_log, seed) };
    let mut out: Vec<LaneHome> = mixes
        .drain(..)
        .enumerate()
        .map(|(k, m)| {
            let labels = PerLocale::init(|l| {
                labels_for
                    .get(l)
                    .get(&m.key)
                    .cloned()
                    .unwrap_or_else(|| LaneText { title: m.title.clone(), short: m.short.clone() })
            });
            let mut lane = lane_of(snap, tiles, &m, labels);
            // «Фон» and «Любимые» are what they are: no new songs.
            if !snap.local_only && !matches!(m.key, MixKey::Background { .. } | MixKey::Liked) {
                // A niche's top-up joins the candidates of its mix (the
                // top-up of these seeds, not of another draw of the key).
                let seeds: Vec<TrackId> = if m.pool.is_empty() { &m.tracks } else { &m.pool }.iter().map(|&i| snap.model.id(i)).collect();
                let topped = super::niche::cached(lib, lane.key, &seeds).map(|f| niche_candidates(snap, &f, &input.new_log)).unwrap_or_default();
                let all: Vec<NewCandidate> = if topped.is_empty() { Vec::new() } else { cands.iter().cloned().chain(topped).collect() };
                let pool = if all.is_empty() { &cands } else { &all };
                let (new, mode) = new_for(snap, input, &log, pool, &m, seed.wrapping_add(k as u64), &mut used_new);
                lane.new = new;
                lane.open = mode.open();
                if mode == mix_new::NewMode::Niche {
                    // The whole pool seeds the top-up, not the card alone.
                    lane.niche = seeds;
                }
            }
            lane
        })
        .collect();
    if prefs.on_repeat
        && snap.model.taste.has_history()
        && let Some(tracks) = on_repeat(snap, tiles)
    {
        let anchors = vec![true; tracks.len()];
        let labels = PerLocale::init(|l| LaneText { title: TileStatic::OnRepeat.title_in(l).to_owned(), short: Vec::new() });
        out.push(LaneHome {
            key: v::MixKey::OnRepeat,
            title: labels.get(loc).title.clone(),
            short: Vec::new(),
            labels,
            side_flavor: None,
            artists: artists_of(snap, tiles, &tracks),
            covers: covers(snap, &tracks),
            anchors,
            tracks,
            new: Vec::new(),
            open: false,
            niche: Vec::new(),
            blend_base: None,
            // A card's name is all it says.
            why: None,
        });
    }
    BuiltLanes { lanes: out, blend_profiles: blendable, blend_titles }
}

/// «Миксы дня»: the day's as they were drawn, else drawn now and kept for
/// the day (see the module docs). «Другой состав» of one draws it again.
fn day_mixes(
    snap: &Snapshot,
    input: &BuildInput,
    tiles: &TilesInput,
    tags: &ArtistTags,
    taken: &PerLocale<Vec<String>>,
    ok: &dyn Fn(usize) -> bool,
) -> (Vec<Mix>, PerLocale<HashMap<MixKey, LaneText>>) {
    let lib = &input.lib;
    let loc = input.locale;
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
    let (mixes, lineup) = with_builder(snap, &input.mix_prefs, &snap.background, ok, Some(tags), &Session::default(), input.now, input.utc_offset_min, seed, loc, |b| {
        let (drawn_base, lineup) = b.daily(&cache.kept);
        let made = b.makeups(
            &drawn_base,
            &|a| tags.get(&a).cloned().unwrap_or_default(),
            &|a| tiles.artist_names.get(&a).cloned(),
            &|i| fact(i).and_then(|f| f.year),
            &|i| fact(i).map(|f| f.title.clone()).filter(|t| !t.trim().is_empty()),
        );
        let daily_labels = PerLocale::init(|l| {
            let mut localized = drawn_base.clone();
            for m in &mut localized {
                if !m.key.is_cluster() {
                    m.title = b.title_in(m.key, &m.tracks, l);
                }
            }
            b.name_daily_in(&mut localized, &made, taken.get(l), decade, l);
            localized
                .into_iter()
                .map(|m| (m.key, LaneText { short: b.short_titles_in(m.key, l), title: m.title }))
                .collect::<HashMap<_, _>>()
        });
        let mut mixes = drawn_base.clone();
        // The lineup and the songs are kept as drawn; a mix nothing can
        // name, or one that is a genre's mix again, is only not shown.
        b.name_daily_in(&mut mixes, &made, taken.get(loc), decade, loc);
        (mixes, (drawn_base, lineup, daily_labels))
    });
    let (drawn, lineup, daily_labels) = lineup;
    cache.kept.lineup = lineup;
    cache.kept.keep(&drawn, &snap.model);
    cache_write(lib, DAY_MIXES_KEY, input.now, &cache);
    (mixes, daily_labels)
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
pub(crate) fn blend_now(
    snap: &Snapshot,
    prefs: &MixPrefs,
    profiles: &[u32],
    names: &dyn Fn(ArtistId) -> Option<String>,
    session: &Session,
    seed: u64,
    loc: Locale,
) -> Option<LaneHome> {
    let key = MixKey::blend(profiles)?;
    let (now, utc_offset_min) = (super::recs_now(), super::local_day().1);
    let ok = |i: usize| snap.catalog.tracks()[i].local || !snap.local_only;
    let (mix, labels) = with_builder(snap, prefs, &[], &ok, None, session, now, utc_offset_min, seed, loc, |b| {
        let mut m = vec![b.mix(key, &Kept::default())?];
        let labels = PerLocale::init(|l| {
            let mut one = m.clone();
            b.named_in(&mut one, l, names);
            LaneText {
                title: one.pop().map_or_else(String::new, |x| x.title),
                short: b.short_titles_in(key, l),
            }
        });
        b.named_in(&mut m, loc, names);
        m.pop().map(|m| (m, labels))
    })?;
    let artists: Vec<String> = mix.artists.iter().filter_map(|a| names(*a)).collect();
    let tracks: Vec<TrackId> = mix.tracks.iter().map(|&i| snap.model.id(i)).collect();
    Some(LaneHome {
        key: view_key(mix.key),
        title: mix.title,
        short: mix.short,
        labels,
        side_flavor: None,
        covers: covers(snap, &tracks),
        anchors: mix.anchors,
        tracks,
        artists: dedupe(artists),
        new: Vec::new(),
        open: false,
        niche: Vec::new(),
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
fn blend_profiles(snap: &Snapshot, b: &Builder<'_>, names: &dyn Fn(ArtistId) -> Option<String>, loc: Locale) -> (Vec<v::BlendProfile>, Vec<(u32, PerLocale<String>)>) {
    let model = &snap.model;
    let mut order: Vec<usize> = (0..model.profiles.len()).filter(|&j| model.profiles.list[j].visible).collect();
    order.sort_by(|a, b| model.profile_share[*b].total_cmp(&model.profile_share[*a]).then(a.cmp(b)));
    let mut out = Vec::new();
    let mut titles_out = Vec::new();
    for j in order {
        let key = model.profiles.list[j].key;
        let Some(m) = b.mix(MixKey::Profile(key), &Kept::default()) else { continue };
        if m.tracks.len() < MIN_SONGS {
            continue;
        }
        let titles = PerLocale::init(|l| {
            let mut one = vec![m.clone()];
            b.named_in(&mut one, l, names);
            one.pop().map_or_else(String::new, |x| x.title)
        });
        out.push(v::BlendProfile { key, title: titles.get(loc).clone() });
        titles_out.push((key, titles));
    }
    (out, titles_out)
}

fn dedupe(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names.into_iter().filter(|n| seen.insert(n.to_lowercase())).take(12).collect()
}

/// The home's card of a mix.
fn lane_of(snap: &Snapshot, tiles: &TilesInput, m: &Mix, labels: PerLocale<LaneText>) -> LaneHome {
    let tracks: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
    let artists = dedupe(m.artists.iter().filter_map(|a| tiles.artist_names.get(a).cloned()).collect());
    let side_flavor = matches!(m.key, MixKey::Calm | MixKey::Lively).then(|| {
        nikgob_recs::mixes::side_flavor_of_types(m.tracks.iter().map(|&i| *snap.model.song_type_at(i)))
    });
    LaneHome {
        key: view_key(m.key),
        title: m.title.clone(),
        short: m.short.clone(),
        labels,
        side_flavor,
        covers: covers(snap, &tracks),
        anchors: m.anchors.clone(),
        tracks,
        artists,
        new: Vec::new(),
        open: false,
        niche: Vec::new(),
        blend_base: m.key.blend_base(),
        // A card's name is all it says.
        why: None,
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
/// graph links it to. None of an artist kept out ([`banned`]).
pub(super) fn candidates(snap: &Snapshot, pool: &[StreamTrack], sources: &HashMap<String, nikgob_recs::Source>, log: &nikgob_recs::NewLog, seed: u64) -> Vec<NewCandidate> {
    use nikgob_recs::{GraphSource, Source, stream_song_key};
    let shares = log.source_shares(&mut nikgob_recs::rng::Rng::new(seed ^ 0x736f_7572_6365));
    let share = |s: Option<Source>| s.and_then(|s| shares.get(&s).copied()).unwrap_or(1.0 / nikgob_recs::novelty::SOURCES.len() as f32);
    let graph: HashMap<String, &super::graph::GraphFind> = snap.graph_new.iter().map(|f| (stream_song_key(&f.track), f)).collect();
    let near = |key: &str| -> Vec<usize> { graph.get(key).map(|f| f.near.iter().filter_map(|(t, _)| snap.model.position(*t)).collect()).unwrap_or_default() };
    let trusts = log.seed_trusts();
    let candidate = |t: &StreamTrack, near: Vec<usize>, weight: f32| {
        let trust = seeds_trust(snap, &trusts, &near);
        new_candidate(snap, t, near, weight * trust)
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let now = snap.model.now();
    let found: Vec<&StreamTrack> = pool.iter().filter(|t| snap.is_new(t) && !banned(log, t, now)).collect();
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
    let rest: Vec<&super::graph::GraphFind> = snap.graph_new.iter().filter(|f| !seen.contains(&stream_song_key(&f.track)) && !banned(log, &f.track, now)).collect();
    let len = rest.len().max(1) as f32;
    for (k, f) in rest.into_iter().enumerate() {
        let weight = GRAPH_WEIGHT * (1.0 - 0.5 * k as f32 / len) * trust;
        out.push(candidate(&f.track, f.near.iter().filter_map(|(t, _)| snap.model.position(*t)).collect(), weight));
    }
    out
}

/// Whether an artist of the new song `t` is kept out of new music at
/// `now` by how their songs were received ([`nikgob_recs::NewLog::artist_penalty`]:
/// two early skips within a month), its artists keyed as the log keys them
/// ([`nikgob_recs::credit_keys`]: a collaboration of an artist kept out is
/// kept out too). Every way a new song comes in minds it: the discoveries,
/// the graph's songs, the niches' top-ups.
pub(super) fn banned(log: &nikgob_recs::NewLog, t: &StreamTrack, now: nikgob_core::model::UnixMs) -> bool {
    let keys = nikgob_recs::credit_keys(t.artists.iter().map(|a| a.name.as_str()));
    log.credit_penalty(&keys, now).is_infinite()
}

/// A new song as a card may take it: its kind of song told by its preview
/// where it was analysed, else by the library's songs `near` it, its
/// profile, the library's songs of its artist.
fn new_candidate(snap: &Snapshot, t: &StreamTrack, near: Vec<usize>, weight: f32) -> NewCandidate {
    let vector = snap.new_track(t).vector;
    let embedding = snap.new_embedding(t);
    // Its voice by its preview, else by the songs whose radios hold it.
    let voice = match vector.as_ref() {
        Some(v) => mix_new::voice_near(&snap.model, &snap.index, v, embedding.as_ref()),
        None => mix_new::voice_of(&snap.model, &near),
    };
    NewCandidate {
        track: t.clone(),
        lane: mix_new::lane_of_new(&snap.types, voice, t),
        soundtrack: t.album.as_ref().is_some_and(|a| franchise(&a.title).is_some()),
        profile: vector.map(|v| snap.model.embed(&v)).and_then(|e| snap.model.regions.of(&e, None)),
        near,
        weight,
        vector,
        embedding,
        artist_tracks: artist_tracks(snap, t),
    }
}

/// The library's songs of the main artist of the new song `t` (catalog
/// positions).
pub(super) fn artist_tracks(snap: &Snapshot, t: &StreamTrack) -> Vec<usize> {
    t.artists
        .first()
        .and_then(|a| snap.artist_keys.get(&nikgob_recs::artist_key(&a.name)))
        .map(|a| snap.model.of_artist(*a).iter().map(|&i| i as usize).collect())
        .unwrap_or_default()
}

/// The songs a niche's top-up found as candidates of its mix: linked to
/// the niche's songs whose lists held them, the most seeds first; none of
/// an artist kept out ([`banned`]).
/// The trust of a new song's seeds lies within these bounds.
const SEED_TRUST: (f32, f32) = (0.4, 1.2);

/// How far the library's songs `near` a new song (catalog positions) are
/// to be trusted to find new music ([`nikgob_recs::NewLog::seed_trusts`],
/// relative to the prior): the mean of those the log knows, 1 when it
/// knows none of them.
fn seeds_trust(snap: &Snapshot, trusts: &HashMap<TrackId, f32>, near: &[usize]) -> f32 {
    let known: Vec<f32> = near.iter().filter(|&&i| i < snap.model.len()).filter_map(|&i| trusts.get(&snap.model.id(i)).copied()).collect();
    if known.is_empty() {
        return 1.0;
    }
    (known.iter().sum::<f32>() / known.len() as f32).clamp(SEED_TRUST.0, SEED_TRUST.1)
}

pub(super) fn niche_candidates(snap: &Snapshot, found: &super::niche::NicheFound, log: &nikgob_recs::NewLog) -> Vec<NewCandidate> {
    let len = found.tracks.len().max(1) as f32;
    let now = snap.model.now();
    let trusts = log.seed_trusts();
    found
        .tracks
        .iter()
        .filter(|(t, _)| snap.is_new(t) && !banned(log, t, now))
        .enumerate()
        .map(|(k, (t, seeds))| {
            let near: Vec<usize> = seeds.iter().filter_map(|s| snap.model.position(*s)).collect();
            let trust = seeds_trust(snap, &trusts, &near);
            new_candidate(snap, t, near, 0.8 * (1.0 - 0.5 * k as f32 / len) * trust)

        })
        .collect()
}

/// «С новым»: the new songs of `cands` that fit the mix as the open world
/// has it ([`mix_new::take`]), each in one mix only, as many as the
/// appetite of the mix's taste profile asks and the user's share of new
/// music at least; and the mode it took them in. «Фон» takes none.
fn new_for(snap: &Snapshot, input: &BuildInput, log: &nikgob_recs::NewLog, cands: &[NewCandidate], m: &Mix, seed: u64, used: &mut HashSet<String>) -> (Vec<StreamTrack>, mix_new::NewMode) {
    let tracks: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
    let profile = profile_of(snap, &tracks);
    let appetite = (!log.events.is_empty() && !snap.model.profiles.is_empty()).then(|| {
        let key = profile.and_then(|j| snap.model.profiles.list.get(j)).map(|p| p.key);
        let background = profile.is_some_and(|j| snap.model.is_background_profile(j));
        log.share(key, background, &mut nikgob_recs::rng::Rng::new(seed))
    });
    let (chosen, mode) = mix_new::take(&snap.model, Some(&snap.index), m, cands, appetite, input.new_share, used);
    (chosen.into_iter().map(|c| c.track.clone()).collect(), mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An artist two of whose new songs were skipped early within a month
    /// is kept out of new music whichever way a song of theirs comes in;
    /// one skip only lowers them, and another artist is untouched.
    #[test]
    fn an_artist_kept_out_is_banned_everywhere() {
        use nikgob_core::streaming::{ArtistRef, RemoteId, Service};
        use nikgob_recs::{Heard, NewEvent, NewLog};
        let day = 86_400_000i64;
        let ev = |song: &str, at: i64| NewEvent {
            song: song.into(),
            artist: nikgob_recs::artist_key("Somebody"),
            source: None,
            vector: None,
            track: None,
            at,
            heard: Heard::Early,
            profile: None,
            background: false,
            seeds: Vec::new(),
        };
        let song = |artist: &str| {
            let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, "x"), "Song");
            t.artists.push(ArtistRef::new(None, artist));
            t
        };
        let mut log = NewLog::default();
        log.push(ev("a", 10 * day));
        assert!(!banned(&log, &song("Somebody"), 11 * day), "one skip lowers only");
        log.push(ev("b", 12 * day));
        assert!(banned(&log, &song("Somebody"), 13 * day));
        assert!(!banned(&log, &song("Someone else"), 13 * day));
        assert!(!banned(&NewLog::default(), &song("Somebody"), 13 * day));
    }

    /// The log keeps the artist of a new song heard as the library credits
    /// it (its row's credit line) and a candidate is checked by the
    /// service's list of its artists: one key function, so the ban of the
    /// main artist of a collaboration skipped twice keeps out their own
    /// songs and the songs they are a guest on, while the guest stays in.
    #[test]
    fn a_ban_from_the_log_meets_every_candidate_of_that_artist() {
        use nikgob_core::streaming::{ArtistRef, RemoteId, Service};
        use nikgob_recs::{Heard, NewEvent, NewLog};
        let day = 86_400_000i64;
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let song = |id: &str, artists: &[&str]| {
            let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), format!("Song {id}"));
            for a in artists {
                t.artists.push(ArtistRef::new(None, *a));
            }
            t
        };
        let mut log = NewLog::default();
        // Two early skips of songs whose credits name «Somebody» first, as
        // the log keeps them (`novelty::new_song`).
        for (k, credit) in [["Somebody / Guest"].as_slice(), &["Somebody", "Another"]].into_iter().enumerate() {
            let id = lib.upsert_stream_tracks(&[song(&format!("h{k}"), credit)]).unwrap()[0];
            let row = lib.get_track_row(id).unwrap().unwrap();
            log.push(NewEvent {
                song: format!("s{k}"),
                artist: nikgob_recs::main_artist_key(&row.artist),
                source: None,
                vector: None,
                track: Some(id),
                at: (10 + k as i64) * day,
                heard: Heard::Early,
                profile: None,
                background: false,
                seeds: Vec::new(),
            });
        }
        let now = 12 * day;
        assert!(banned(&log, &song("1", &["Somebody"]), now));
        assert!(banned(&log, &song("2", &["Somebody feat. Third"]), now));
        assert!(banned(&log, &song("3", &["Third", "Somebody"]), now), "a guest kept out keeps the song out");
        assert!(banned(&log, &song("4", &["Third & Somebody, Fourth"]), now));
        assert!(!banned(&log, &song("5", &["Guest"]), now), "the guest of a skipped song stays in");
        assert!(!banned(&log, &song("6", &["Third"]), now));
    }

    /// A log kept before the artists were keyed by [`nikgob_recs::credit_keys`]
    /// keyed a credit whole («somebody guest» of «Somebody / Guest»): read
    /// now, its early skips count with the main artist's — old and new
    /// together, and two old ones alone — and keep out their own songs.
    #[test]
    fn an_old_logs_whole_credits_count_with_the_main_artist() {
        use nikgob_core::streaming::{ArtistRef, RemoteId, Service};
        use nikgob_recs::{Heard, NewEvent, NewLog};
        let day = 86_400_000i64;
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let song = |id: &str, artists: &[&str]| {
            let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), format!("Song {id}"));
            for a in artists {
                t.artists.push(ArtistRef::new(None, *a));
            }
            t
        };
        let heard = |id: &str, credit: &[&str], old: bool, at: i64| {
            let track = lib.upsert_stream_tracks(&[song(id, credit)]).unwrap()[0];
            let row = lib.get_track_row(track).unwrap().unwrap();
            NewEvent {
                song: id.into(),
                // The key of before, or of now.
                artist: if old { nikgob_recs::artist_key(&row.artist) } else { nikgob_recs::main_artist_key(&row.artist) },
                source: None,
                vector: None,
                track: Some(track),
                at,
                heard: Heard::Early,
                profile: None,
                background: false,
                seeds: Vec::new(),
            }
        };
        let now = 12 * day;
        let alone = song("x", &["Somebody"]);
        // One skip of before, one of now.
        let mut mixed = NewLog::default();
        mixed.push(heard("o1", &["Somebody / Guest"], true, 10 * day));
        mixed.push(heard("n1", &["Somebody"], false, 11 * day));
        assert_ne!(mixed.events[0].artist, mixed.events[1].artist, "the keys of before and now differ");
        assert!(!banned(&mixed, &alone, now), "as kept, neither key has two skips");
        assert!(banned(&super::super::novelty::normalised(&lib, mixed), &alone, now));
        // Two skips of before.
        let mut old = NewLog::default();
        old.push(heard("o2", &["Somebody / Guest"], true, 10 * day));
        old.push(heard("o3", &["Somebody / Other"], true, 11 * day));
        assert!(banned(&super::super::novelty::normalised(&lib, old), &alone, now));
        // A song gone from the library keeps its key, the main of it.
        let mut gone = NewLog::default();
        gone.push(NewEvent { track: None, ..heard("g1", &["Third"], false, 10 * day) });
        assert_eq!(super::super::novelty::normalised(&lib, gone).events[0].artist, "third");
    }

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
            new_embeddings: HashMap::new(),
            artist_affinity: HashMap::new(),
            profiles: Default::default(),
            background: Vec::new(),
            graph_new: Vec::new(),
            genres: HashMap::new(),
        };
        let mut tiles = TilesInput::default();
        for (k, id) in ids.iter().enumerate() {
            let ms = if k == 0 { 10 * 60_000 } else { 180_000 };
            tiles.facts.insert(*id, TrackFacts { duration_ms: Some(ms), ..TrackFacts::default() });
        }
        let prefs = MixPrefs::of(&v::Prefs::default());
        let ok = |i: usize| tiles.facts.contains_key(&snap.catalog.tracks()[i].id);
        let mixes = with_builder(&snap, &prefs, &[], &ok, None, &Session::default(), 1_000 * DAY_MS, 0, 1, Locale::Ru, |b| b.lineup(&Kept::default()).0);
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
        crate::analysis::load_offline(&lib, &index, &nikgob_view::Prefs::default());
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
            labels: PerLocale::default(),
            side_flavor: None,
            tracks: (0..28).map(|_| TrackId::new()).collect(),
            anchors: anchors.clone(),
            artists: Vec::new(),
            covers: Vec::new(),
            new,
            open: false,
            niche: Vec::new(),
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
        // The open world: as many as there are, evenly after the first.
        let many: Vec<StreamTrack> = (0..20).map(|k| StreamTrack::new(RemoteId { service: Service::Youtube, id: format!("w{k}") }, format!("Wide {k}"))).collect();
        let open = LaneHome { open: true, new: many, tracks: lane.tracks[..10].to_vec(), anchors: vec![false; 10], ..lane };
        let picks = open.picks(true);
        assert_eq!(picks.len(), 30);
        assert!(matches!(picks[0].item, Item::Track(_)));
        assert_eq!(picks.iter().filter(|p| matches!(p.item, Item::Stream(_))).count(), 20);
    }
}
