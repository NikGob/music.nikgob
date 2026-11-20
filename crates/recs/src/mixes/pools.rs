use std::collections::{HashMap, HashSet};

use nikgob_core::i18n::Locale;
use nikgob_core::model::{ArtistId, EventContext};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::mix_names::{Makeup, MixName};
use crate::model::{UNHEARD_LOVE, freshness};
use crate::model::{Live, Moment, UserModel};
use crate::pick::Recs;
use crate::profiles::{ACTIVE_SHARE, Kind, LaneKey, Loudness, background_cards, quota};
use crate::radio::MIX_REST;
use crate::rng::Rng;
use crate::session::Session;
use crate::taste::DayPart;

use super::naming::{lane_kind, side_lang};

use super::*;

impl<'a> Builder<'a> {
    /// The engine over `recs` (whose model is `model`): `ok` the tracks the
    /// runtime lets a mix take, `suits` those that suit the background,
    /// `epoch` the seed of the home's epoch and `part` the part of the day,
    /// `live` the session going on.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        recs: &'a Recs<'a>,
        model: &'a UserModel,
        ok: &dyn Fn(usize) -> bool,
        suits: &'a dyn Fn(usize) -> bool,
        epoch: u64,
        part: DayPart,
        live: Live,
        settings: Settings,
    ) -> Self {
        let now = model.now();
        let m0: Moment = model.moment(&Session::default(), now);
        let n = model.len().min(recs.catalog.len());
        let mut a = vec![0.0f32; n];
        let mut love = vec![0.0f32; n];
        let mut value = vec![None; n];
        // What was heard and when, against the latest of it (a user back
        // after a year has last year's weeks, not none): the model's.
        let latest = model.latest_heard();
        let mut fresh = vec![1.0f32; n];
        let mut stale = vec![false; n];
        for i in 0..n {
            let t = recs.info(i);
            if recs.blocked(i) || !ok(i) || t.duration_ms.is_some_and(|d| !(INTERLUDE_MS..=LONG_MS).contains(&d)) {
                continue;
            }
            let s = &model.tracks[i];
            love[i] = s.aff_long_whole(now).max(0.0).max((recs.taste.affinity_at(i).max(0.0) / 3.0).tanh()).max(model.prior_mass(i));
            // What only the library's prior stands for is no love to fade.
            let state = love[i] > model.prior_mass(i);
            match (latest, model.last_heard_at(i)) {
                (Some(l), Some(h)) => {
                    fresh[i] = freshness(l - h);
                    stale[i] = state && l - h >= STALE_MS;
                }
                (Some(_), None) if state => {
                    fresh[i] = UNHEARD_LOVE;
                    stale[i] = true;
                }
                _ => {}
            }
            a[i] = love[i] * fresh[i];
            let p = model.p_skip(&m0, i, EventContext::HomeSection).min(0.95);
            value[i] = Some(
                a[i] + W_HOT * s.aff_hot(now).max(0.0) - MIX_REST * recs.taste.lately_at(i) + W_SKIP * (1.0 - p).ln()
                    - recs.taste.dislike_near_at(i),
            );
        }
        let history = model.tracks.iter().any(|t| t.starts > 0);
        Self {
            recs,
            model,
            suits,
            epoch,
            part,
            live,
            settings,
            history,
            a,
            love,
            value,
            fresh,
            stale,
            daily: std::cell::OnceCell::new(),
            genres: Vec::new(),
            genre_pools: std::cell::OnceCell::new(),
        }
    }

    /// Sets the target locale for rendered mix titles.
    pub fn with_locale(mut self, loc: Locale) -> Self {
        self.settings.locale = loc;
        self
    }

    /// The engine knowing the genres: `tags` are those other listeners
    /// gave an artist (as [`Self::makeups`] takes them), asked once for
    /// each main artist. A song is of the family its main artist is of
    /// ([`crate::mix_names::leading`]); without this there are no genre
    /// mixes and no card is told for a genre's.
    pub fn with_genres(mut self, tags: &dyn Fn(ArtistId) -> Vec<(String, f32)>) -> Self {
        let mut of: HashMap<ArtistId, Option<u8>> = HashMap::new();
        let mut table: Vec<Option<u8>> = vec![None; self.value.len()];
        for (i, family) in table.iter_mut().enumerate() {
            if !self.pickable(i) {
                continue;
            }
            if let Some(a) = self.recs.info(i).artists.first().copied() {
                *family = *of.entry(a).or_insert_with(|| crate::mix_names::leading(&tags(a)));
            }
        }
        self.genres = table;
        self.genre_pools = std::cell::OnceCell::new();
        self
    }

    /// The family of genres of the track at `i` (its number), if known.
    pub fn genre_at(&self, i: usize) -> Option<u8> {
        self.genres.get(i).copied().flatten()
    }

    /// How much of the love of the track at `i` counts now (see the module
    /// docs): 1 while it is heard, down to [`STALE_FLOOR`].
    pub fn fresh_at(&self, i: usize) -> f32 {
        self.fresh.get(i).copied().unwrap_or(1.0)
    }

    /// A loved song not heard for [`STALE_MS`], or never.
    pub fn stale_at(&self, i: usize) -> bool {
        self.stale.get(i).copied().unwrap_or(false)
    }

    fn liked(&self, i: usize) -> bool {
        self.model.tracks[i].liked(self.model.now()) || self.recs.taste.liked_at(i)
    }

    fn pickable(&self, i: usize) -> bool {
        self.value.get(i).is_some_and(Option::is_some)
    }

    /// Never played and not loved otherwise (a like, a playlist): what a
    /// mix takes little of.
    pub(super) fn unheard(&self, i: usize) -> bool {
        self.history && self.model.tracks[i].starts == 0 && self.a[i] <= 0.0
    }

    pub(super) fn profile_index(&self, key: u32) -> Option<usize> {
        self.model.profiles.list.iter().position(|p| p.key == key)
    }

    fn profile_tracks(&self, j: usize) -> Vec<usize> {
        (0..self.value.len()).filter(|&i| self.pickable(i) && self.model.profile_at(i) == Some(j)).collect()
    }

    /// The profiles shown, active and with songs enough, the largest share
    /// first.
    fn active(&self) -> Vec<usize> {
        let p = &self.model.profiles;
        let mut out: Vec<usize> = (0..p.len())
            .filter(|&j| p.list[j].visible && self.model.profile_share.get(j).is_some_and(|s| *s >= ACTIVE_SHARE))
            .filter(|&j| self.profile_tracks(j).len() >= MIN_SONGS)
            .collect();
        out.sort_by(|a, b| self.model.profile_share[*b].total_cmp(&self.model.profile_share[*a]).then(a.cmp(b)));
        out
    }

    /// The «Фон» cards of the background profiles: the balance's own count
    /// of them (at most the user's), each a key.
    fn background_keys(&self) -> Vec<MixKey> {
        if self.settings.background_cards == 0 {
            return Vec::new();
        }
        let suits = |i: usize| self.pickable(i) && (self.suits)(i);
        let home = self.model.home_profiles(MOMENTS, self.settings.background_cards, suits);
        let mut out = Vec::new();
        for (j, cards) in home.background.iter().enumerate() {
            let (Some(p), n) = (self.model.profiles.list.get(j), cards.len()) else { continue };
            if !p.visible || n == 0 {
                continue;
            }
            for c in 0..n {
                out.push(MixKey::Background { profile: p.key, card: c as u8, cards: n as u8 });
            }
        }
        out
    }

    /// The epoch's moments: the lineup kept for it in its order, else those
    /// the lottery draws ([`Self::drawn`]) in the order of how they fit now;
    /// the blends made today first, in the places of the moments that fit
    /// now least. And the lineup to keep.
    pub fn lineup(&self, kept: &Kept) -> (Vec<Mix>, Vec<MixKey>) {
        let mut chosen: Vec<Mix> = Vec::new();
        // The epoch's lineup first, as it was; new moments only for what
        // of it is gone.
        let hidden = |k: &MixKey| self.settings.hidden.contains(k);
        let before: Vec<MixKey> = kept.lineup.iter().copied().filter(|k| !hidden(k)).collect();
        let fresh = self.drawn(kept).into_iter().filter(|k| !kept.lineup.contains(k) && !hidden(k));
        for (n, key) in before.iter().copied().chain(fresh).enumerate() {
            if chosen.len() >= MOMENTS {
                break;
            }
            let Some(mix) = self.mix(key, kept) else { continue };
            // «Всё вперемешку» takes the best of every profile: it is no
            // other moment's double, nor they its.
            let set: HashSet<usize> = mix.tracks.iter().copied().collect();
            let same = key != MixKey::Everything
                && chosen.iter().filter(|c| c.key != MixKey::Everything).any(|c| {
                    let shared = c.tracks.iter().filter(|i| set.contains(i)).count();
                    shared as f32 >= OVERLAP * set.len().min(c.tracks.len()) as f32
                });
            if n >= before.len() && same {
                continue;
            }
            chosen.push(mix);
        }
        if before.is_empty() {
            chosen.sort_by(|a, b| b.score.total_cmp(&a.score));
        }
        let keys: Vec<MixKey> = chosen.iter().map(|m| m.key).collect();
        // The blends stand in the places of the moments that fit now least
        // (the two latest; those stay in the day's lineup).
        let mut blends: Vec<Mix> = kept.blends.iter().rev().filter_map(|k| self.mix(*k, kept)).take(BLENDS_SHOWN).collect();
        chosen.truncate(MOMENTS.saturating_sub(blends.len()));
        blends.extend(chosen);
        (blends, keys)
    }

    /// Every moment there is, in the order the epoch's lottery draws them
    /// (Efraimidis–Spirakis): each by how well its songs fit now
    /// ([`Self::context`]), a profile's own by its share too (at least
    /// [`LOT_SHARE`] of the largest's chances), one shown in the epochs
    /// before far less ([`LOT_SHOWN`]); every moment has its chances.
    pub fn drawn(&self, kept: &Kept) -> Vec<MixKey> {
        let all = self.candidates();
        let top = self.model.profile_share.iter().copied().fold(0.0f32, f32::max).max(1e-6);
        let mut lots: Vec<(f32, MixKey)> = all
            .into_iter()
            .filter_map(|key| {
                let pool = self.pool(key)?;
                let profiles: Vec<usize> = {
                    let mut p: Vec<usize> = pool.tracks.iter().filter_map(|&i| self.model.profile_at(i)).collect();
                    p.sort_unstable();
                    p.dedup();
                    p
                };
                let probe = Mix { key, title: String::new(), short: Vec::new(), tracks: pool.tracks, anchors: Vec::new(), artists: Vec::new(), profiles, score: 0.0, kind: None, genre: None, pool_size: (0, 0), pool: Vec::new() };
                let mut w = (LOT_FIT * self.context(&probe).clamp(-2.0, 2.0)).exp();
                if let MixKey::Profile(k) = key
                    && let Some(j) = self.profile_index(k)
                {
                    w *= (self.model.profile_share.get(j).copied().unwrap_or(0.0) / top).sqrt().max(LOT_SHARE);
                }
                for (shown, f) in kept.shown.iter().zip(LOT_SHOWN) {
                    if shown.contains(&key) {
                        w *= f;
                    }
                }
                let u = Rng::derive(self.epoch ^ 0x6d69_7865, key.token()).f32().max(1e-6);
                Some((-u.ln() / w.max(1e-6), key))
            })
            .collect();
        lots.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        lots.into_iter().map(|(_, k)| k).collect()
    }

    /// The moments to choose from.
    fn candidates(&self) -> Vec<MixKey> {
        let s = &self.settings;
        let active = self.active();
        if active.is_empty() {
            // Nothing heard yet: the library's kinds.
            let mut kinds: HashMap<LaneKey, usize> = HashMap::new();
            for i in (0..self.value.len()).filter(|&i| self.pickable(i)) {
                *kinds.entry(LaneKey::of(self.model.song_type_at(i))).or_default() += 1;
            }
            let mut kinds: Vec<(LaneKey, usize)> = kinds.into_iter().filter(|k| k.1 >= MIN_SONGS).collect();
            kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut out: Vec<MixKey> = kinds.into_iter().map(|k| MixKey::Kind(lane_code(k.0))).collect();
            if s.moods {
                out.extend([MixKey::Lively, MixKey::Calm]);
            }
            return out;
        }
        let bg = self.background_keys();
        let with_bg: HashSet<u32> = bg.iter().filter_map(|k| k.blend_base()).collect();
        let mut out = bg;
        if s.everything && active.len() >= 2 {
            out.push(MixKey::Everything);
        }
        if s.profiles {
            let plain = active.iter().map(|&j| self.model.profiles.list[j].key).filter(|k| !with_bg.contains(k));
            out.extend(plain.map(MixKey::Profile));
        }
        if s.moods {
            out.extend([MixKey::Lively, MixKey::Calm]);
        }
        out
    }

    /// The mix of `key` in the epoch: its songs kept for it in their order,
    /// else drawn (`None` when it has too few).
    pub fn mix(&self, key: MixKey, kept: &Kept) -> Option<Mix> {
        // A family there is no more (a number of an older build).
        if let MixKey::Genre(f) = key
            && !crate::mix_names::has_family(f)
        {
            return None;
        }
        let salt = kept.salt(key);
        let steady = matches!(key, MixKey::Background { .. });
        // A mix of the day and of a part of it stays what it was drawn as,
        // whatever its clusters, its genre or its part's best became
        // since; the others keep the songs their pool still has.
        let whole = matches!(key, MixKey::Daily(_) | MixKey::Genre(_) | MixKey::Part(_));
        let pool = self.pool(key);
        // The whole pool's size, and the pool itself when it is a niche.
        let pool_of = |tracks: &[usize]| -> ((usize, usize), Vec<usize>) {
            let artists: HashSet<ArtistId> = tracks.iter().filter_map(|&i| self.recs.info(i).artists.first().copied()).collect();
            let size = (tracks.len(), artists.len());
            (size, if crate::open_world::is_niche(size.0, size.1) { tracks.to_vec() } else { Vec::new() })
        };
        let whole_pool = pool.as_ref().map(|p| pool_of(&p.tracks));
        let kept_songs = match kept.songs(key) {
            Some((s, ids)) if s == salt && (whole || pool.is_some()) => {
                let have: Option<HashSet<usize>> = if whole { None } else { pool.as_ref().map(|p| p.tracks.iter().copied().collect()) };
                let t: Vec<usize> = ids
                    .iter()
                    .filter_map(|id| self.model.position(*id))
                    .filter(|&i| self.pickable(i) && have.as_ref().is_none_or(|h| h.contains(&i)))
                    .collect();
                (t.len() >= MIN_SONGS).then_some(t)
            }
            _ => None,
        };
        let tracks = match kept_songs {
            Some(t) => t,
            None => {
                let pool = pool?;
                let before = kept.songs(key).map_or(&[][..], |t| t.1);
                // What never played and finds no place between loved ones
                // in the order of the day leaves whatever the part, and
                // others are drawn in its place (the second time no more
                // songs never played than found a place).
                let arc = if pool.arc == Arc::Focus { Arc::Focus } else { Arc::Day };
                let mut left = pool.clone();
                let mut order = Vec::new();
                for round in 0..REDRAWS {
                    let drawn = self.compose(key, &left, salt, kept, before);
                    order = self.order(&drawn, &left, arc, steady, true);
                    let placed: HashSet<usize> = order.iter().copied().collect();
                    let dropped: Vec<usize> = drawn.iter().copied().filter(|i| !placed.contains(i)).collect();
                    if dropped.is_empty() || order.len() >= pool.len {
                        break;
                    }
                    left.tracks.retain(|i| !dropped.contains(i));
                    if round > 0 {
                        let fit = order.iter().filter(|&&i| self.unheard(i)).count();
                        left.unheard = fit as f32 / pool.len.max(1) as f32;
                        left.floor = left.floor.min(left.unheard);
                        left.widen = false;
                    }
                }
                if order.len() < MIN_SONGS {
                    return None;
                }
                // On the part's own arc, unless that leaves a song never
                // played outside a gap between loved ones (more of them
                // when the user asks for new songs): then the order of the
                // day, where each found its gap.
                let on_arc = self.order(&order, &pool, pool.arc, steady, false);
                if self.sandwiched(&on_arc) { on_arc } else { order }
            }
        };
        if tracks.len() < MIN_SONGS {
            return None;
        }
        let mut profiles: Vec<usize> = tracks.iter().filter_map(|&i| self.model.profile_at(i)).collect();
        profiles.sort_unstable();
        profiles.dedup();
        let anchors = self.anchors(&tracks);
        let (kind, genre) = (self.kind_of(key, &tracks), self.genre_of(key, &tracks));
        let (pool_size, pool) = whole_pool.unwrap_or_else(|| pool_of(&tracks));
        let mut mix = Mix { key, title: String::new(), short: Vec::new(), artists: self.artists(&tracks), anchors, tracks, profiles, score: 0.0, kind, genre, pool_size, pool };
        mix.title = self.title(key, &mix.tracks);
        mix.short = self.short_titles(key);
        mix.score = self.context(&mix);
        Some(mix)
    }

    /// What `key` draws from and how.
    pub(super) fn pool(&self, key: MixKey) -> Option<Pool> {
        let floor = self.settings.unheard.clamp(0.0, 1.0);
        let plain = |tracks: Vec<usize>| Pool {
            tracks,
            len: MIX_LEN,
            max_ms: MIX_MS,
            unheard: UNHEARD_SHARE,
            floor,
            widen: true,
            quota: None,
            blocks: None,
            arc: Arc::of(self.part),
            cross: false,
            stale: STALE_SHARE,
        };
        let mut pool = match key {
            MixKey::Profile(k) => plain(self.profile_tracks(self.profile_index(k)?)),
            MixKey::Liked => {
                let liked: Vec<usize> = (0..self.value.len()).filter(|&i| self.pickable(i) && self.liked(i)).collect();
                Pool { len: LIKED_LEN, max_ms: LIKED_MS, cross: true, stale: LIKED_STALE_SHARE, floor: 0.0, ..plain(liked) }
            }
            MixKey::Part(p) => {
                let part = DayPart::ALL.get(usize::from(p)).copied()?;
                Pool { arc: Arc::of(part), cross: true, ..plain(self.part_tracks(part)) }
            }
            MixKey::Daily(0) => {
                // Everything the clusters of the day draw from; before
                // there is enough of that, whatever a mix may take.
                let mut tracks: Vec<usize> = self.daily_pools().iter().flatten().copied().collect();
                if tracks.len() < MIN_SONGS {
                    tracks = (0..self.value.len()).filter(|&i| self.pickable(i)).collect();
                }
                tracks.sort_unstable();
                tracks.dedup();
                // Each taste profile by what of it is listened to now, in
                // blocks, as «Всё вперемешку» takes them by their shares.
                let mut weight: HashMap<usize, f32> = HashMap::new();
                for &i in &tracks {
                    if let Some(j) = self.model.profile_at(i)
                        && !self.stale[i]
                    {
                        *weight.entry(j).or_default() += self.a[i].max(0.0);
                    }
                }
                let mut of: Vec<(usize, f32)> = weight.into_iter().filter(|w| w.1 > 0.0).collect();
                of.sort_by(|a, b| a.0.cmp(&b.0));
                let total: f32 = of.iter().map(|w| w.1).sum();
                let by_profile: Option<HashMap<usize, usize>> = (of.len() >= 2 && total > 0.0).then(|| {
                    let shares: Vec<f32> = of.iter().map(|w| w.1 / total).collect();
                    let counts = quota(&shares, MIX_LEN, &vec![false; of.len()], MIX_LEN);
                    of.iter().map(|w| w.0).zip(counts).collect()
                });
                let blocks = by_profile.as_ref().map(|_| 0);
                Pool { unheard: DAILY_UNHEARD, arc: Arc::Day, cross: true, quota: by_profile, blocks, ..plain(tracks) }
            }
            MixKey::Daily(n) => {
                let tracks = self.daily_pools().get(usize::from(n) - 1)?.clone();
                // The day's order, whatever the part.
                Pool { unheard: DAILY_UNHEARD, arc: Arc::Day, ..plain(tracks) }
            }
            MixKey::Genre(f) => {
                let tracks = self.genre_pools().iter().find(|g| g.0 == f)?.1.clone();
                // As a cluster's; a genre is sung in many languages.
                Pool { unheard: DAILY_UNHEARD, arc: Arc::Day, cross: true, ..plain(tracks) }
            }
            MixKey::Kind(c) => {
                let lane = lane_of_code(c);
                plain((0..self.value.len()).filter(|&i| self.pickable(i) && LaneKey::of(self.model.song_type_at(i)) == lane).collect())
            }
            MixKey::Everything => {
                let active = self.active();
                if active.len() < 2 {
                    return None;
                }
                let shares: Vec<f32> = active.iter().map(|&j| self.model.profile_share[j]).collect();
                let counts = quota(&shares, MIX_LEN, &vec![false; active.len()], MIX_LEN);
                let tracks = active.iter().flat_map(|&j| self.profile_tracks(j)).collect();
                Pool { quota: Some(active.iter().copied().zip(counts).collect()), blocks: Some(0), cross: true, ..plain(tracks) }
            }
            MixKey::Calm | MixKey::Lively => Pool { cross: true, ..plain(self.side(key == MixKey::Calm)) },
            MixKey::Background { profile, card, cards } => {
                let j = self.profile_index(profile)?;
                let mut suited: Vec<(usize, f32)> =
                    self.profile_tracks(j).into_iter().filter(|&i| (self.suits)(i)).map(|i| (i, self.model.energy_at(i).unwrap_or(0.0))).collect();
                // The steady middle of the profile's energy, split by the cards.
                let mut e: Vec<f32> = suited.iter().map(|x| x.1).collect();
                e.sort_by(f32::total_cmp);
                if e.len() >= 2 * MIN_SONGS * usize::from(cards) {
                    let band = if cards == 1 { BG_BAND } else { BG_CARDS_BAND };
                    let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
                    let (lo, hi) = (at(band.0), at(band.1));
                    suited.retain(|x| (lo..=hi).contains(&x.1));
                }
                let tracks: Vec<usize> = background_cards(&suited, usize::from(cards)).get(usize::from(card))?.clone();
                Pool { tracks, len: BG_LEN, max_ms: BG_MS, unheard: BG_UNHEARD_SHARE, floor: 0.0, widen: false, quota: None, blocks: None, arc: Arc::Focus, cross: false, stale: STALE_SHARE }
            }
            MixKey::Blend(keys) => {
                let sides: Vec<usize> = keys.iter().filter(|k| **k != 0).filter_map(|k| self.profile_index(*k)).collect();
                if sides.len() < 2 {
                    return None;
                }
                // Each side's third nearest the others' sound (the bridges).
                let mut tracks = Vec::new();
                for &j in &sides {
                    let others: Vec<&crate::analysis::Vector> =
                        sides.iter().filter(|&&o| o != j).map(|&o| &self.model.profiles.list[o].sound).collect();
                    let mut own = self.profile_tracks(j);
                    let d = |i: usize| self.model.embedded_at(i).map_or(f32::INFINITY, |e| others.iter().map(|o| euclid(e, *o)).sum::<f32>());
                    own.sort_by(|a, b| d(*a).total_cmp(&d(*b)).then(a.cmp(b)));
                    own.truncate((own.len() / 3).max(2 * MIN_SONGS));
                    tracks.extend(own);
                }
                let each = BLEND_LEN / sides.len();
                let quota = sides.iter().map(|&j| (j, each)).collect();
                Pool { tracks, len: each * sides.len(), quota: Some(quota), blocks: Some(2), cross: true, ..plain(Vec::new()) }
            }
        };
        // In a room a song never played is a skip by voice away: fewer of
        // them ([`crate::context::AMBIENT_MAX_UNHEARD`]).
        if self.model.device() == crate::context::PlayDevice::Ambient {
            pool.unheard = pool.unheard.min(crate::context::AMBIENT_MAX_UNHEARD);
            pool.floor = pool.floor.min(crate::context::AMBIENT_MAX_UNHEARD);
        }
        (pool.tracks.len() >= MIN_SONGS).then_some(pool)
    }

    /// The loved songs the part of the day `part` hears more than the
    /// others (this weekday's: the song's own part of the day, its
    /// profile's slot and the sound the user plays then,
    /// [`crate::context`]), the liked ones before the rest: the best
    /// [`PART_POOL`] times a mix's length.
    pub(super) fn part_tracks(&self, part: DayPart) -> Vec<usize> {
        let slot = self.model.slot().with_part(part);
        let mut scored: Vec<(f32, usize)> = (0..self.value.len())
            .filter(|&i| self.pickable(i) && self.a[i] > 0.0)
            .map(|i| {
                let more = self.model.time_bonus_at(i, slot) + self.model.sound_fit_at(i, slot);
                (self.a[i] * (1.0 + PART_BONUS * more) + if self.liked(i) { PART_LIKED } else { 0.0 }, i)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(PART_POOL * MIX_LEN);
        scored.into_iter().map(|x| x.1).collect()
    }

    /// The mix that is always there, before the lottery's: the mix of
    /// this part of the day once anything was played, unless it is left
    /// out («Не предлагать этот микс»). Its songs are the epoch's, as any
    /// moment's (`kept`). «Любимые» is the playlist's own tile: the mix of
    /// the liked songs ([`MixKey::Liked`]) is not shown beside it.
    pub fn pinned(&self, kept: &Kept) -> Vec<Mix> {
        let part = MixKey::Part(self.part.index() as u8);
        if !self.history || self.settings.hidden.contains(&part) {
            return Vec::new();
        }
        self.mix(part, kept).into_iter().collect()
    }

    /// The mixes of the day (see the module docs): the day's own as they
    /// were drawn (`kept`: its lineup, each mix's songs), else «Микс дня»,
    /// the genre mixes (the families most loved now whose songs make a
    /// mix, [`GENRE_MIXES`] at most, none of those left out) and one for
    /// each cluster of what the user listens to now, the heaviest first.
    /// A cluster's mix is called by its taste profile until
    /// [`Self::name_daily`] names it by what it is made of. And the lineup
    /// to keep.
    pub fn daily(&self, kept: &Kept) -> (Vec<Mix>, Vec<MixKey>) {
        let hidden = |k: &MixKey| self.settings.hidden.contains(k);
        let mixes: Vec<Mix> = if kept.lineup.is_empty() {
            let genres: Vec<Mix> =
                self.genre_pools().iter().map(|g| MixKey::Genre(g.0)).filter(|k| !hidden(k)).filter_map(|k| self.mix(k, kept)).take(GENRE_MIXES).collect();
            let clusters = (1..=self.daily_pools().len().min(DAILY_MIXES)).filter_map(|n| self.mix(MixKey::Daily(n as u8), kept));
            self.mix(MixKey::Daily(0), kept).into_iter().chain(genres).chain(clusters).collect()
        } else {
            kept.lineup.iter().copied().filter(|k| !hidden(k)).filter_map(|k| self.mix(k, kept)).collect()
        };
        let keys = mixes.iter().map(|m| m.key).collect();
        (mixes, keys)
    }

    /// What each of `mixes` is made of, for its name
    /// ([`crate::mix_names`]): `tags` of an artist, `artist` their name,
    /// `year` and `song` of the track at a catalog position.
    pub fn makeups(
        &self,
        mixes: &[Mix],
        tags: &dyn Fn(ArtistId) -> Vec<(String, f32)>,
        artist: &dyn Fn(ArtistId) -> Option<String>,
        year: &dyn Fn(usize) -> Option<i32>,
        song: &dyn Fn(usize) -> Option<String>,
    ) -> Vec<Makeup> {
        // The library's energies, to place a mix among them.
        let mut energies: Vec<f32> = (0..self.value.len()).filter(|&i| self.pickable(i)).filter_map(|i| self.model.energy_at(i)).collect();
        energies.sort_by(f32::total_cmp);
        mixes
            .iter()
            .map(|m| {
                let mut count: HashMap<ArtistId, (usize, usize)> = HashMap::new();
                let mut of_artist: HashMap<ArtistId, Vec<(String, f32)>> = HashMap::new();
                let mut song_tags: Vec<Vec<(String, f32)>> = Vec::with_capacity(m.tracks.len());
                for (k, &i) in m.tracks.iter().enumerate() {
                    let Some(a) = self.recs.info(i).artists.first().copied() else {
                        song_tags.push(Vec::new());
                        continue;
                    };
                    count.entry(a).or_insert((0, k)).0 += 1;
                    song_tags.push(of_artist.entry(a).or_insert_with(|| tags(a)).clone());
                }
                let mut ranked: Vec<(ArtistId, (usize, usize))> = count.into_iter().collect();
                ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
                let mut own: Vec<f32> = m.tracks.iter().filter_map(|&i| self.model.energy_at(i)).collect();
                own.sort_by(f32::total_cmp);
                let placed = !own.is_empty() && own.len() * 2 >= m.tracks.len() && energies.len() >= 4 * MIN_SONGS;
                let energy = placed.then(|| {
                    let median = own[own.len() / 2];
                    energies.partition_point(|e| *e < median) as f32 / energies.len() as f32
                });
                Makeup {
                    tags: song_tags,
                    artists: ranked.into_iter().filter_map(|(a, (n, _))| Some((artist(a)?, n))).collect(),
                    years: m.tracks.iter().map(|&i| year(i)).collect(),
                    energy,
                    kind: Some(self.cluster_kind(&m.tracks)),
                    songs: m.tracks.iter().take(3).filter_map(|&i| song(i)).collect(),
                }
            })
            .collect()
    }

    /// Names the named mixes of the day among `mixes` by what they are made
    /// of (`makeups` of [`Self::makeups`], one for each of `mixes`): no
    /// two alike, none of a name in `taken` (the other cards of the row);
    /// `common_decade` is the decade most of the library is of. A mix
    /// nothing can name is left out, and so is one whose own genre has its
    /// genre mix among `mixes`: it is that mix again.
    pub fn name_daily(&self, mixes: &mut Vec<Mix>, makeups: &[Makeup], taken: &[String], common_decade: Option<i32>) {
        self.name_daily_in(mixes, makeups, taken, common_decade, self.settings.locale);
    }

    /// [`Self::name_daily`] rendering names in `loc`.
    pub fn name_daily_in(&self, mixes: &mut Vec<Mix>, makeups: &[Makeup], taken: &[String], common_decade: Option<i32>, loc: Locale) {
        let genres: Vec<u8> = mixes
            .iter()
            .filter_map(|m| match m.key {
                MixKey::Genre(f) => Some(f),
                _ => None,
            })
            .collect();
        let double = |k: usize| makeups.get(k).and_then(|m| crate::mix_names::genre(m).first().copied()).is_some_and(|g| genres.contains(&g));
        let doubles: Vec<usize> = (0..mixes.len()).filter(|&k| mixes[k].key.is_cluster() && double(k)).collect();
        let at: Vec<usize> = (0..mixes.len()).filter(|&k| mixes[k].key.is_cluster() && !doubles.contains(&k)).collect();
        let of: Vec<Makeup> = at.iter().map(|&k| makeups.get(k).cloned().unwrap_or_default()).collect();
        let mut taken: Vec<String> = taken.to_vec();
        taken.extend(mixes.iter().filter(|m| !m.key.is_cluster()).map(|m| m.title.clone()));
        let names = crate::mix_names::assign_in(&of, &taken, common_decade, loc);
        let mut unnamed: Vec<usize> = doubles;
        for (&k, assigned) in at.iter().zip(names) {
            match assigned {
                Some((mix_name, title)) => {
                    mixes[k].title = title;
                    if let MixName::Kind { kind, .. } = mix_name {
                        mixes[k].kind = Some(kind);
                    }
                }
                None => unnamed.push(k),
            }
        }
        unnamed.sort_unstable();
        for k in unnamed.into_iter().rev() {
            mixes.remove(k);
        }
    }

    /// The kind (and optional loudness disambiguator) of `tracks`: its
    /// majority taste profile's kind, or the majority song kind.
    fn cluster_kind(&self, tracks: &[usize]) -> (Kind, Option<Loudness>) {
        let mut of: HashMap<usize, usize> = HashMap::new();
        for &i in tracks {
            if let Some(j) = self.model.profile_at(i) {
                *of.entry(j).or_default() += 1;
            }
        }
        if let Some((j, _)) = of.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))) {
            return (self.model.profiles.list[j].kind, self.model.profiles.kind_loudness(j));
        }
        let mut kinds: HashMap<LaneKey, usize> = HashMap::new();
        for &i in tracks {
            *kinds.entry(LaneKey::of(self.model.song_type_at(i))).or_default() += 1;
        }
        let k = kinds.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(Kind::Songs, |k| lane_kind(k.0));
        (k, None)
    }

    /// What a named mix of `tracks` is called in `loc` before its makeup is
    /// known: its taste profile's name, or its songs' kind.
    pub(super) fn cluster_title_in(&self, tracks: &[usize], loc: Locale) -> String {
        let (kind, loudness) = self.cluster_kind(tracks);
        match loudness {
            Some(loud) => format!("{}, {}", kind.title_in(loc), loud.word_in(loc)),
            None => kind.title_in(loc),
        }
    }

    /// The pools of the mixes of the day, the heaviest cluster first.
    fn daily_pools(&self) -> &[Vec<usize>] {
        self.daily.get_or_init(|| self.cluster_daily())
    }

    /// How much each track is listened to now (0: not at all, or not
    /// pickable): its love as it counts now, an old song's none; with too
    /// little of that (a library just brought in, a long silence)
    /// everything loved.
    fn now_weights(&self) -> Vec<f32> {
        let n = self.value.len();
        let weigh = |now: bool| -> Vec<f32> {
            (0..n).map(|i| if self.pickable(i) && !(now && self.stale[i]) { self.a[i].max(0.0) } else { 0.0 }).collect()
        };
        let mut w = weigh(true);
        if w.iter().filter(|x| **x > 0.0).count() < 2 * MIN_SONGS {
            w = weigh(false);
        }
        w
    }

    /// The families of genres that may have a mix, with the songs each
    /// draws from (every pickable song of the family): those with
    /// [`DAILY_CORES`] songs listened to now and [`MIN_SONGS`] songs, the
    /// most loved now first (the sum of [`Self::now_weights`]).
    fn genre_pools(&self) -> &[(u8, Vec<usize>)] {
        self.genre_pools.get_or_init(|| {
            if self.genres.is_empty() {
                return Vec::new();
            }
            let w = self.now_weights();
            // Each family's weight, songs listened to now and songs.
            let mut by: HashMap<u8, (f32, usize, Vec<usize>)> = HashMap::new();
            for i in (0..self.value.len()).filter(|&i| self.pickable(i)) {
                let Some(f) = self.genre_at(i) else { continue };
                let g = by.entry(f).or_default();
                g.2.push(i);
                if w[i] > 0.0 {
                    g.0 += w[i];
                    g.1 += 1;
                }
            }
            let mut ranked: Vec<(u8, (f32, usize, Vec<usize>))> = by.into_iter().filter(|(_, g)| g.1 >= DAILY_CORES && g.2.len() >= MIN_SONGS).collect();
            ranked.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.0.cmp(&b.0)));
            ranked.into_iter().map(|(f, g)| (f, g.2)).collect()
        })
    }

    /// The kind of song the mix of `key` with `tracks` is of
    /// ([`Mix::kind`]).
    fn kind_of(&self, key: MixKey, tracks: &[usize]) -> Option<Kind> {
        match key {
            MixKey::Profile(k) => Some(self.model.profiles.list[self.profile_index(k)?].kind),
            MixKey::Kind(c) => Some(lane_kind(lane_of_code(c))),
            MixKey::Calm | MixKey::Lively => {
                let lanes: Vec<LaneKey> = tracks.iter().map(|&i| LaneKey::of(self.model.song_type_at(i))).collect();
                side_lang(&lanes).map(Kind::Sung)
            }
            _ => None,
        }
    }

    /// The family of genres the mix of `key` with `tracks` is of
    /// ([`Mix::genre`]).
    fn genre_of(&self, key: MixKey, tracks: &[usize]) -> Option<u8> {
        if let MixKey::Genre(f) = key {
            return Some(f);
        }
        let mut count: HashMap<u8, usize> = HashMap::new();
        for f in tracks.iter().filter_map(|&i| self.genre_at(i)) {
            *count.entry(f).or_default() += 1;
        }
        count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).filter(|c| c.1 as f32 >= GENRE_SAME * tracks.len() as f32).map(|c| c.0)
    }

    /// What the user listens to now in clusters, and the songs each draws
    /// from (see the module docs).
    fn cluster_daily(&self) -> Vec<Vec<usize>> {
        let n = self.value.len();
        let w = self.now_weights();
        // By taste profile (one group before there are profiles): the
        // songs listened to now, all its songs, the weight.
        struct Group {
            profile: Option<usize>,
            cores: Vec<usize>,
            pool: Vec<usize>,
            weight: f32,
            mixes: usize,
        }
        let mut by_profile: HashMap<Option<usize>, Group> = HashMap::new();
        for i in (0..n).filter(|&i| self.pickable(i)) {
            let profile = self.model.profile_at(i);
            let g = by_profile.entry(profile).or_insert_with(|| Group { profile, cores: Vec::new(), pool: Vec::new(), weight: 0.0, mixes: 0 });
            g.pool.push(i);
            if w[i] > 0.0 {
                g.cores.push(i);
                g.weight += w[i];
            }
        }
        let mut groups: Vec<Group> = by_profile.into_values().filter(|g| g.cores.len() >= DAILY_CORES && g.pool.len() >= MIN_SONGS).collect();
        groups.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.profile.cmp(&b.profile)));
        groups.truncate(DAILY_MIXES);
        // One mix for each profile, then the rest to the heaviest by what
        // each has for a mix more (D'Hondt), while its songs are enough.
        let room = |g: &Group| (g.cores.len() / DAILY_CORES_EACH).min(g.pool.len() / MIX_LEN).max(1);
        for g in &mut groups {
            g.mixes = 1;
        }
        for _ in groups.len()..DAILY_MIXES {
            let share = |g: &Group| g.weight / (g.mixes + 1) as f32;
            let next = (0..groups.len())
                .filter(|&k| groups[k].mixes < room(&groups[k]))
                .max_by(|&x, &y| share(&groups[x]).total_cmp(&share(&groups[y])).then(y.cmp(&x)));
            let Some(k) = next else { break };
            groups[k].mixes += 1;
        }
        let mut clusters: Vec<(f32, Vec<usize>)> = groups.iter().flat_map(|g| self.split(&g.cores, &g.pool, &w, g.mixes)).collect();
        clusters.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.first().cmp(&b.1.first())));
        clusters.truncate(DAILY_MIXES);
        clusters.into_iter().map(|c| c.1).collect()
    }

    /// `pool` in `k` clusters by the sound of its `cores` (weighed by `w`),
    /// in fewer when one would have too few of them or too few songs: the
    /// weight of each cluster and its songs. A song without a sound goes
    /// where most of its artist is, else by its place.
    fn split(&self, cores: &[usize], pool: &[usize], w: &[f32], k: usize) -> Vec<(f32, Vec<usize>)> {
        let sounded: Vec<(&Vector, f32)> = cores.iter().filter_map(|&i| self.model.embedded_at(i).map(|e| (e, w[i]))).collect();
        for k in (2..=k).rev() {
            if sounded.len() < k * DAILY_CORES {
                continue;
            }
            let centres = kmeans(&sounded, k);
            if centres.len() < k {
                continue;
            }
            let nearest = |e: &Vector| (0..k).min_by(|&a, &b| euclid(e, &centres[a]).total_cmp(&euclid(e, &centres[b])).then(a.cmp(&b))).unwrap_or(0);
            // Each cluster's weight, songs and count of cores.
            let mut out: Vec<(f32, Vec<usize>, usize)> = vec![(0.0, Vec::new(), 0); k];
            let mut of_artist: HashMap<(ArtistId, usize), f32> = HashMap::new();
            let mut unsounded: Vec<usize> = Vec::new();
            for &i in pool {
                let Some(e) = self.model.embedded_at(i) else {
                    unsounded.push(i);
                    continue;
                };
                let c = nearest(e);
                out[c].1.push(i);
                if w[i] > 0.0 {
                    out[c].0 += w[i];
                    out[c].2 += 1;
                    if let Some(a) = self.model.main_artist_at(i) {
                        *of_artist.entry((a, c)).or_default() += w[i];
                    }
                }
            }
            for i in unsounded {
                let most = self.model.main_artist_at(i).and_then(|a| {
                    let weight = |c: usize| of_artist.get(&(a, c)).copied().unwrap_or(0.0);
                    (0..k).filter(|&c| weight(c) > 0.0).max_by(|&x, &y| weight(x).total_cmp(&weight(y)).then(y.cmp(&x)))
                });
                let c = most.unwrap_or(i % k);
                out[c].1.push(i);
                if w[i] > 0.0 {
                    out[c].0 += w[i];
                    out[c].2 += 1;
                }
            }
            if out.iter().all(|c| c.2 >= DAILY_CORES && c.1.len() >= MIN_SONGS) {
                return out
                    .into_iter()
                    .map(|(weight, mut songs, _)| {
                        songs.sort_unstable();
                        (weight, songs)
                    })
                    .collect();
            }
        }
        vec![(cores.iter().map(|&i| w[i]).sum::<f32>(), pool.to_vec())]
    }

    /// The calm (or the lively) side of what is loved, by the energy of
    /// the loved songs with a sound (all with a sound before anything is
    /// loved); a side of one language keeps to it.
    fn side(&self, calm: bool) -> Vec<usize> {
        let with_sound: Vec<usize> = (0..self.value.len()).filter(|&i| self.pickable(i) && self.model.embedded_at(i).is_some()).collect();
        let loved: Vec<usize> = with_sound.iter().copied().filter(|&i| self.a[i] > 0.0).collect();
        let base = if loved.len() >= 2 * MIN_SONGS { loved } else { with_sound };
        let mut e: Vec<f32> = base.iter().filter_map(|&i| self.model.energy_at(i)).collect();
        if e.len() < MIN_SONGS {
            return Vec::new();
        }
        e.sort_by(f32::total_cmp);
        let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
        let (lo, hi) = (at(SIDE), at(1.0 - SIDE));
        // A calm song never explodes ([`crate::songtype::SongType::calm_dynamics`]).
        let mut side: Vec<usize> = base
            .into_iter()
            .filter(|&i| self.model.energy_at(i).is_some_and(|x| if calm { x <= lo && self.model.song_type_at(i).calm_dynamics() } else { x >= hi }))
            .collect();
        let mut langs: HashMap<Lang, usize> = HashMap::new();
        for &i in &side {
            if let LaneKey::Sung(l) = LaneKey::of(self.model.song_type_at(i)) {
                *langs.entry(l).or_default() += 1;
            }
        }
        if let Some((&l, &n)) = langs.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            && n as f32 >= LANG_HOLD * side.len() as f32
        {
            side.retain(|&i| {
                let t = self.model.song_type_at(i);
                match LaneKey::of(t) {
                    LaneKey::Sung(x) => x == l,
                    LaneKey::Unknown => !t.ost,
                    LaneKey::Instrumental => false,
                }
            });
        }
        side
    }

}

/// Rounds of the k-means of the mixes of the day at most.
const KMEANS_ROUNDS: usize = 12;

/// `k` centres of `pts` (each with its weight) by a weighted k-means that
/// needs no chance: it starts from the heaviest point, then each time from
/// the point farthest (by the root of its weight) from the centres there
/// are. Fewer than `k` when the points are fewer.
fn kmeans(pts: &[(&Vector, f32)], k: usize) -> Vec<Vector> {
    let mut centres: Vec<Vector> = Vec::with_capacity(k);
    let Some(first) = (0..pts.len()).max_by(|&a, &b| pts[a].1.total_cmp(&pts[b].1).then(b.cmp(&a))) else {
        return centres;
    };
    centres.push(*pts[first].0);
    let mut far: Vec<f32> = pts.iter().map(|p| euclid(p.0, &centres[0])).collect();
    while centres.len() < k.min(pts.len()) {
        let reach = |i: usize| pts[i].1.max(0.0).sqrt() * far[i];
        let Some(next) = (0..pts.len()).max_by(|&a, &b| reach(a).total_cmp(&reach(b)).then(b.cmp(&a))) else {
            break;
        };
        let centre = *pts[next].0;
        for (d, p) in far.iter_mut().zip(pts) {
            *d = d.min(euclid(p.0, &centre));
        }
        centres.push(centre);
    }
    for _ in 0..KMEANS_ROUNDS {
        let mut sum = vec![[0.0f32; DIM]; centres.len()];
        let mut mass = vec![0.0f32; centres.len()];
        for &(e, weight) in pts {
            let c = (0..centres.len()).min_by(|&a, &b| euclid(e, &centres[a]).total_cmp(&euclid(e, &centres[b])).then(a.cmp(&b))).unwrap_or(0);
            for (s, x) in sum[c].iter_mut().zip(e.iter()) {
                *s += weight * x;
            }
            mass[c] += weight;
        }
        let mut moved = false;
        for c in 0..centres.len() {
            if mass[c] <= 0.0 {
                continue;
            }
            let next: Vector = std::array::from_fn(|d| sum[c][d] / mass[c]);
            moved |= euclid(&next, &centres[c]) > 1e-4;
            centres[c] = next;
        }
        if !moved {
            break;
        }
    }
    centres
}

