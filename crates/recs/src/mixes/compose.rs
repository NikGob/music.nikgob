use std::collections::{HashMap, HashSet};

use crate::index::euclid;
use crate::rng::Rng;

use super::*;

impl<'a> Builder<'a> {
    /// The songs of `key` from `pool` (see the module docs): the core drawn
    /// by value, the rest by the epoch's chance; `before` the songs it had
    /// before «Другой состав» (left out of the rotation, as the epoch
    /// before's).
    pub(super) fn compose(&self, key: MixKey, pool: &Pool, salt: u64, kept: &Kept, before: &[TrackId]) -> Vec<usize> {
        let seed = self.epoch.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ key.token() ^ salt.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let chance = |i: usize| Rng::derive(seed, i as u64).f32();
        let mut away: HashSet<usize> = kept.before(key).iter().filter_map(|t| self.model.position(*t)).collect();
        if salt != 0 {
            away.extend(before.iter().filter_map(|t| self.model.position(*t)));
        }
        // What is never played but near the loved: the loved of now, or of
        // any time when nothing of the pool is heard now.
        let now_loved = pool.tracks.iter().any(|&i| self.love[i] >= 0.5 && !self.stale[i]);
        let is_loved = |i: usize| self.love[i] >= 0.5 && !(now_loved && self.stale[i]);
        let loved: Vec<&crate::analysis::Vector> = pool.tracks.iter().filter(|&&i| is_loved(i)).filter_map(|&i| self.model.embedded_at(i)).collect();
        let scale = self.model.scale().max(1e-3);
        // …or beside the loved in the graphs.
        let mut linked: HashMap<usize, f32> = HashMap::new();
        for &l in pool.tracks.iter().filter(|&&i| is_loved(i)) {
            for &(c, w) in self.model.graph.near(l).iter().chain(self.model.personal_near(l)) {
                let e = linked.entry(c as usize).or_default();
                *e = e.max(w);
            }
        }
        let value = |i: usize| {
            let mut v = self.value[i].unwrap_or(f32::NEG_INFINITY);
            if self.unheard(i) {
                let sound = self.model.embedded_at(i).map_or(0.0, |e| {
                    let near = loved.iter().map(|l| euclid(e, *l)).fold(f32::INFINITY, f32::min);
                    (1.0 - near / scale).max(0.0)
                });
                let near = sound.max(linked.get(&i).copied().unwrap_or(0.0));
                v += W_EXPLORE * near;
                // A mix of the day brings its share of what fits and was
                // never played.
                if near > 0.0 && matches!(key, MixKey::Daily(_) | MixKey::Genre(_)) {
                    v += W_DAILY_NEW;
                }
                // A file come lately leads the never played.
                if crate::open_world::part_on(crate::open_world::Part::Arrivals) {
                    v += W_ARRIVAL * self.model.arrival_at(i);
                }
            }
            v
        };
        let n = pool.len;
        // «Преимущественно новые песни»: this many never played at least.
        let floor = if self.history && pool.floor > 0.0 { ((n as f32 * pool.floor).ceil() as usize).min(n) } else { 0 };
        let liked = key == MixKey::Liked;
        let (unheard, stale) = self.unheard_and_stale(pool, n, floor, liked);
        let caps = Caps {
            artist: n.div_ceil(3).max(1),
            // Two in ten would allow six of 28, too tight a weave for an order.
            album: (n / 7).max(2),
            // One world (a franchise) is a quarter at most, also of a pool
            // of it alone: that makes no mix.
            world: ((n as f32 * WORLD_SHARE).ceil() as usize).max(2),
            unheard,
            stale,
        };
        let mut pick = Picking::new(self, caps, pool.quota.clone(), pool.max_ms);
        // The floor first: the best never played by value (the nearest the
        // loved), the epoch before's after the others; the sandwich of the
        // order still decides which of them stay.
        if floor > 0 {
            let mut fresh: Vec<(f32, usize)> = pool.tracks.iter().filter(|&&i| self.unheard(i)).map(|&i| (value(i), i)).collect();
            fresh.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            for strict in [true, false] {
                for &(_, i) in &fresh {
                    if pick.unheard >= floor || pick.full(n) {
                        break;
                    }
                    if strict && away.contains(&i) {
                        continue;
                    }
                    pick.take(i);
                }
            }
        }
        // Half-persistent daily refresh: on automatic day rollover (`salt == 0`)
        // daily & genre mixes keep ~42% of yesterday's familiar non-worn core
        // so the mix feels recognizable yet genuinely fresh (~58% rotated).
        let daily_carry = matches!(key, MixKey::Daily(_) | MixKey::Genre(_)) && salt == 0 && !away.is_empty();
        if daily_carry {
            let carry_target = ((n as f32 * DAILY_CARRY_SHARE).round() as usize).min(n.saturating_sub(floor));
            let mut prev_core: Vec<(f32, usize)> = pool
                .tracks
                .iter()
                .copied()
                .filter(|&i| away.contains(&i) && !self.unheard(i) && self.model.worn(i) == 0.0)
                .map(|i| (value(i) + 0.18 * chance(i), i))
                .collect();
            prev_core.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            for &(_, i) in &prev_core {
                if pick.len() >= floor + carry_target || pick.full(n) {
                    break;
                }
                pick.take(i);
            }
        }
        // The core: drawn from the best by value (Gumbel keys over a third
        // of their spread), the epoch before's less likely; then the rest of
        // the pool by value, for what the rules held back.
        let half = n.div_ceil(2);
        let mut core: Vec<(f32, usize)> = pool.tracks.iter().map(|&i| (value(i), i)).collect();
        core.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let k = (CORE_POOL * half).min(core.len());
        let mut tau = 1.0;
        if k > 0 {
            tau = ((core[0].0 - core[k - 1].0) / CORE_SPREAD).max(0.05);
            let gumbel = |i: usize| {
                let u = Rng::derive(seed ^ 0x636f_7265, i as u64).f64().clamp(1e-12, 1.0 - 1e-12);
                -(-u.ln()).ln() as f32
            };
            let mut top: Vec<(f32, usize)> =
                core[..k].iter().map(|&(v, i)| (v / tau + gumbel(i) - if away.contains(&i) { CORE_BEFORE } else { 0.0 }, i)).collect();
            top.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            core.splice(..k, top);
        }
        // A mix of one kind keeps to the scene its first songs are of: what
        // the graph links to the songs taken stands higher; without a graph nothing changes.
        let cohere = (matches!(key, MixKey::Profile(_) | MixKey::Kind(_) | MixKey::Genre(_)) || key.is_cluster()) && !self.model.graph.is_empty();
        if cohere {
            let mut top: Vec<(f32, usize)> = core[..k].to_vec();
            self.take_cohering(&mut pick, &mut top, 1.0 / tau, &|p: &Picking<'_, '_>| p.len() >= half || p.full(n), &|i| daily_carry && away.contains(&i));
        }
        for &(_, i) in &core[if cohere { k } else { 0 }..] {
            if pick.len() >= half || pick.full(n) {
                break;
            }
            if daily_carry && away.contains(&i) {
                continue;
            }
            pick.take(i);
        }
        // The rest by the epoch's chance, the epoch before's left out while
        // others last.
        let mut rest: Vec<(f32, usize)> = pool.tracks.iter().map(|&i| (value(i) + ROTATE * chance(i), i)).collect();
        rest.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        if cohere {
            let mut left = rest.clone();
            self.take_cohering(&mut pick, &mut left, 1.0, &|p: &Picking<'_, '_>| p.full(n), &|i| away.contains(&i));
        }
        for strict in [true, false] {
            for &(_, i) in &rest {
                if pick.full(n) {
                    break;
                }
                if strict && away.contains(&i) {
                    continue;
                }
                pick.take(i);
            }
        }
        // Too few by the quotas (a small profile): the others fill in.
        if pick.quota.is_some() && !pick.full(n) {
            pick.quota = None;
            for &(_, i) in &rest {
                if pick.full(n) {
                    break;
                }
                pick.take(i);
            }
        }
        // Still short (the rules held back what is heard now, or nothing
        // else is there): old songs fill in up to a mix's least length
        // rather than leave no mix, past their share no further — an
        // imported history or a library liked at once is no listening of
        // now. «Любимые» to its length.
        let least = if liked { n } else { MIN_SONGS.min(n) };
        if pick.len() < least && !pick.full(n) && pick.caps.stale != usize::MAX {
            pick.caps.stale = usize::MAX;
            for &(_, i) in &rest {
                if pick.len() >= least || pick.full(n) {
                    break;
                }
                pick.take(i);
            }
        }
        // Short of its length (the rules held it back), a world and an
        // artist keep their share of what is there: the least worth of
        // whichever is over goes, until none is.
        let mut out = pick.out;
        loop {
            let world_cap = ((out.len() as f32 * WORLD_SHARE).ceil() as usize).max(2);
            let artist_cap = out.len().div_ceil(3).max(1);
            let mut per_world: HashMap<u64, usize> = HashMap::new();
            let mut per_artist: HashMap<ArtistId, usize> = HashMap::new();
            for &i in &out {
                let t = self.recs.info(i);
                if let Some(w) = t.world {
                    *per_world.entry(w).or_default() += 1;
                }
                if let Some(a) = t.artists.first() {
                    *per_artist.entry(*a).or_default() += 1;
                }
            }
            let over = |i: usize| {
                let t = self.recs.info(i);
                t.world.is_some_and(|w| per_world[&w] > world_cap) || t.artists.first().is_some_and(|a| per_artist[a] > artist_cap)
            };
            let Some(k) = (0..out.len()).filter(|&k| over(out[k])).min_by(|a, b| value(out[*a]).total_cmp(&value(out[*b])).then(a.cmp(b))) else {
                break;
            };
            out.remove(k);
        }
        out
    }

    /// Takes from `list` (keys, the best first) while `done` says no, each
    /// time the best by its key plus [`W_COHERE`] (× `unit`) of how much the
    /// graph links it to what was taken, `skip` left for later.
    fn take_cohering(
        &self,
        pick: &mut Picking<'_, '_>,
        list: &mut Vec<(f32, usize)>,
        unit: f32,
        done: &dyn Fn(&Picking<'_, '_>) -> bool,
        skip: &dyn Fn(usize) -> bool,
    ) {
        let g = &self.model.graph;
        let mut link: HashMap<usize, f32> = HashMap::new();
        for &i in &pick.out {
            self.linked(&mut link, i, g);
        }
        while !done(pick) {
            let best = list
                .iter()
                .enumerate()
                .filter(|(_, (_, i))| !skip(*i))
                .map(|(k, (key, i))| (k, key + unit * W_COHERE * link.get(i).copied().unwrap_or(0.0).tanh()))
                .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
            let Some((k, _)) = best else { break };
            let (_, i) = list.remove(k);
            let before = pick.len();
            pick.take(i);
            if pick.len() > before {
                self.linked(&mut link, i, g);
            }
        }
    }

    /// Adds what the graph links to the track at `i` to `link`.
    fn linked(&self, link: &mut HashMap<usize, f32>, i: usize, g: &crate::graph::SimGraph) {
        for &(c, w) in g.near(i) {
            *link.entry(c as usize).or_default() += w;
        }
        if let Some(a) = self.model.main_artist_at(i) {
            for &(b, w) in g.similar_artists(a) {
                for &j in self.model.of_artist(b) {
                    *link.entry(j as usize).or_default() += 0.5 * w;
                }
            }
        }
    }

    /// How many songs never played and how many old songs a mix of `n` from
    /// `pool` takes at most. The never played: their share, at least the
    /// `floor`, and what the songs heard now and the old ones' share leave
    /// empty while there are more of them. The old ones: their share, also
    /// when nearly everything loved is old (a history imported from a
    /// service, a library liked at once): what is left empty then stays so
    /// down to a mix's least length ([`Self::compose`]). «Любимые» are the
    /// likes whatever their age: its old songs fill what the others leave.
    pub(super) fn unheard_and_stale(&self, pool: &Pool, n: usize, floor: usize, liked: bool) -> (usize, usize) {
        let quarter = ((n as f32 * pool.stale).ceil() as usize).min(n);
        let heard_now = pool.tracks.iter().filter(|&&i| !self.stale[i] && self.a[i] > 0.0).count();
        if liked {
            let unheard = if self.history { ((n as f32 * pool.unheard).ceil() as usize).max(1) } else { usize::MAX };
            return (unheard, quarter.max(n.saturating_sub(heard_now)));
        }
        if !self.history {
            return (usize::MAX, quarter);
        }
        let cap = ((n as f32 * pool.unheard).ceil() as usize).max(1).max(floor);
        let never = pool.tracks.iter().filter(|&&i| self.unheard(i)).count();
        if !pool.widen {
            return (cap, quarter);
        }
        let empty = n.saturating_sub(heard_now + quarter + never.min(cap));
        (cap + empty.min(never.saturating_sub(cap)), quarter)
    }

}

/// A selection being filled under the caps.
struct Picking<'b, 'a> {
    b: &'b Builder<'a>,
    caps: Caps,
    quota: Option<HashMap<usize, usize>>,
    max_ms: u64,
    out: Vec<usize>,
    ms: u64,
    songs: HashSet<TrackId>,
    per_artist: HashMap<ArtistId, usize>,
    per_album: HashMap<u64, usize>,
    per_world: HashMap<u64, usize>,
    per_profile: HashMap<usize, usize>,
    /// A mix of quotas: a world's and an artist's songs of each profile.
    per_profile_world: HashMap<(usize, u64), usize>,
    per_profile_artist: HashMap<(usize, ArtistId), usize>,
    per_profile_album: HashMap<(usize, u64), usize>,
    unheard: usize,
    stale: usize,
}

impl<'b, 'a> Picking<'b, 'a> {
    fn new(b: &'b Builder<'a>, caps: Caps, quota: Option<HashMap<usize, usize>>, max_ms: u64) -> Self {
        Self {
            b,
            caps,
            quota,
            max_ms,
            out: Vec::new(),
            ms: 0,
            songs: HashSet::new(),
            per_artist: HashMap::new(),
            per_album: HashMap::new(),
            per_world: HashMap::new(),
            per_profile: HashMap::new(),
            per_profile_world: HashMap::new(),
            per_profile_artist: HashMap::new(),
            per_profile_album: HashMap::new(),
            unheard: 0,
            stale: 0,
        }
    }

    fn len(&self) -> usize {
        self.out.len()
    }

    fn full(&self, n: usize) -> bool {
        self.out.len() >= n || self.ms >= self.max_ms
    }

    /// Takes `i` if every cap allows it.
    fn take(&mut self, i: usize) {
        let t = self.b.recs.info(i);
        let song = t.song.unwrap_or(t.id);
        if self.songs.contains(&song) || self.songs.contains(&t.id) {
            return;
        }
        let album = self.b.model.release_at(i);
        let profile = self.b.model.profile_at(i);
        let unheard = self.b.unheard(i);
        let stale = self.b.stale[i];
        if t.artists.iter().any(|a| self.per_artist.get(a).is_some_and(|c| *c >= self.caps.artist))
            || album.is_some_and(|x| self.per_album.get(&x).is_some_and(|c| *c >= self.caps.album))
            || t.world.is_some_and(|w| self.per_world.get(&w).is_some_and(|c| *c >= self.caps.world))
            || (unheard && self.unheard >= self.caps.unheard)
            || (stale && self.stale >= self.caps.stale)
        {
            return;
        }
        if let Some(q) = &self.quota {
            let room = profile.and_then(|p| q.get(&p)).copied().unwrap_or(0);
            if profile.is_none_or(|p| self.per_profile.get(&p).copied().unwrap_or(0) >= room) {
                return;
            }
            // Its profile's share in blocks: a world or an artist half of
            // it at most, so a block can take turns.
            let half = room.div_ceil(2).max(2);
            let j = profile.unwrap_or(usize::MAX);
            // An album a quarter of it: its blocks stand near each other.
            let quarter = room.div_ceil(4).max(1);
            if album.is_some_and(|x| self.per_profile_album.get(&(j, x)).is_some_and(|c| *c >= quarter))
                || t.world.is_some_and(|w| self.per_profile_world.get(&(j, w)).is_some_and(|c| *c >= half))
                || t.artists.first().is_some_and(|a| self.per_profile_artist.get(&(j, *a)).is_some_and(|c| *c >= half))
            {
                return;
            }
        }
        self.songs.insert(song);
        self.songs.insert(t.id);
        for a in &t.artists {
            *self.per_artist.entry(*a).or_default() += 1;
        }
        if let Some(x) = album {
            *self.per_album.entry(x).or_default() += 1;
        }
        if let Some(w) = t.world {
            *self.per_world.entry(w).or_default() += 1;
        }
        if let Some(p) = profile {
            *self.per_profile.entry(p).or_default() += 1;
            if let Some(w) = t.world {
                *self.per_profile_world.entry((p, w)).or_default() += 1;
            }
            if let Some(a) = t.artists.first() {
                *self.per_profile_artist.entry((p, *a)).or_default() += 1;
            }
            if let Some(x) = album {
                *self.per_profile_album.entry((p, x)).or_default() += 1;
            }
        }
        self.unheard += usize::from(unheard);
        self.stale += usize::from(stale);
        self.ms += u64::from(t.duration_ms.unwrap_or(210_000));
        self.out.push(i);
    }
}

