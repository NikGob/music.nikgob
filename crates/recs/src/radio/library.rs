//! The seed-based library radio: the candidates gathered by the seed, the
//! sound score, the unheard share and the user model's say in a pick.

use super::*;

impl RadioState {
    /// Up to `n` library slots, selected under the diversity caps.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn library(
        &self,
        recs: &Recs<'_>,
        n: usize,
        sound: &Sound,
        exclude: &HashSet<Item>,
        strict: bool,
        rules: &Diversity,
        rng: &mut Rng,
        taken: &mut HashSet<Item>,
        pull: Option<&Vector>,
    ) -> Vec<Slot> {
        if n == 0 {
            return Vec::new();
        }
        // A mix leaves what played lately for later; a radio only what
        // played among the last few.
        let mix = self.is_mix();
        let recent: HashSet<&Item> = self.recent.iter().collect();
        let in_session: HashSet<TrackId> = self.session.tracks().map(|p| p.track).collect();
        let resting = |i: usize, id: TrackId| {
            if mix {
                recs.played_within(i, RECENT_MS)
            } else {
                recent.contains(&Item::Track(id)) || in_session.contains(&id) || recs.played_within(i, self.no_repeat_ms)
            }
        };
        let ok = |i: usize| {
            let id = recs.info(i).id;
            !recs.blocked(i)
                && !exclude.contains(&Item::Track(id))
                && !taken.contains(&Item::Track(id))
                && self.seed != RadioSeed::Track(id)
                && (self.not_world.is_none() || recs.info(i).world != self.not_world)
                && !(strict && resting(i, id))
        };
        // Candidates with the reason and the relation bonus they came by.
        let mut cands: HashMap<usize, (Reason, f32)> = HashMap::new();
        fn add(cands: &mut HashMap<usize, (Reason, f32)>, i: usize, reason: Reason, bonus: f32) {
            let e = cands.entry(i).or_insert((reason.clone(), bonus));
            if bonus > e.1 {
                *e = (reason, bonus);
            }
        }
        let seed_artists = self.seed_artists(recs);
        let seed_track = match &self.seed {
            RadioSeed::Track(id) => Some(*id),
            _ => None,
        };
        let raw_seed: Option<Vector> = match &self.seed {
            RadioSeed::Track(id) => recs.index.get(*id).copied(),
            RadioSeed::Stream { vector, .. } => *vector,
            _ => None,
        };
        // Sound neighbours of the seed (by the blend with the neural
        // model's vector when the seed has one).
        if let Some(q) = raw_seed {
            for (id, _) in recs.index.nearest_to(seed_track, &q, SEED_NEIGHBOURS, |id| recs.catalog.position(id).is_some_and(ok)) {
                let i = recs.catalog.position(id).expect("filtered");
                add(&mut cands, i, seed_track.map_or(Reason::SoundsLikeSeed, Reason::SimilarTo), 0.0);
            }
        } else if let Some(v) = sound.seed {
            // An artist's or a style's point: scan by the embedded distance.
            for (i, _) in nearest_embedded(recs, &v, SEED_NEIGHBOURS, &ok) {
                let reason = match &self.seed {
                    RadioSeed::Style { index, .. } => Reason::Style(*index),
                    RadioSeed::Daylist { part, .. } => Reason::DayPart(*part),
                    _ => Reason::SoundsLikeSeed,
                };
                add(&mut cands, i, reason, 0.0);
            }
        }
        for item in &self.liked {
            if let Item::Track(id) = item {
                for (nid, _) in recs.index.neighbours(*id, LIKED_NEIGHBOURS, |k| recs.catalog.position(k).is_some_and(ok)) {
                    add(&mut cands, recs.catalog.position(nid).expect("filtered"), Reason::SoundsLikeSeed, 0.0);
                }
            }
        }
        // A liked track's neighbours.
        let mut pulled: HashSet<usize> = HashSet::new();
        if let Some(pv) = pull {
            for (i, _) in nearest_embedded(recs, pv, LIKED_NEIGHBOURS, &ok) {
                add(&mut cands, i, Reason::SoundsLikeSeed, 1.0);
                pulled.insert(i);
            }
        }
        // The seed's artists and the artists co-listened with them.
        for &a in &seed_artists {
            for &i in recs.catalog.of_artist(a) {
                if ok(i as usize) {
                    add(&mut cands, i as usize, Reason::SameArtist(a), 1.0);
                }
            }
            for &(b, count) in recs.taste.co_listened(a).iter().take(CO_ARTISTS) {
                let mut theirs: Vec<usize> = recs.catalog.of_artist(b).iter().map(|&i| i as usize).filter(|&i| ok(i)).collect();
                theirs.sort_by(|x, y| recs.taste.affinity_at(*y).total_cmp(&recs.taste.affinity_at(*x)).then(x.cmp(y)));
                for i in theirs.into_iter().take(CO_TRACKS) {
                    add(&mut cands, i, Reason::CoListened(a), (count / 3.0).min(1.0) * 0.7);
                }
            }
            // The artists other listeners hear with it (the collective
            // graph), their best known first.
            if let Some(m) = recs.model {
                for &(b, w) in m.graph.similar_artists(a).iter().take(CO_ARTISTS) {
                    let key = |i: usize| recs.taste.affinity_at(i) + m.prior_at(i);
                    let mut theirs: Vec<usize> = recs.catalog.of_artist(b).iter().map(|&i| i as usize).filter(|&i| ok(i)).collect();
                    theirs.sort_by(|x, y| key(*y).total_cmp(&key(*x)).then(x.cmp(y)));
                    for i in theirs.into_iter().take(CO_TRACKS) {
                        add(&mut cands, i, Reason::CoListened(a), w * 0.7);
                    }
                }
            }
        }
        match &self.seed {
            RadioSeed::Mood(m) => {
                let pool = recs.mood_pool(*m);
                let (top, bottom) = (pool.first().map_or(0.0, |p| p.1), pool.last().map_or(0.0, |p| p.1));
                let span = (top - bottom).max(1e-3);
                for (id, s) in pool {
                    if let Some(i) = recs.catalog.position(id).filter(|&i| ok(i)) {
                        add(&mut cands, i, Reason::Mood(*m), (s - bottom) / span);
                    }
                }
            }
            RadioSeed::Style { index, artists, .. } => {
                for &a in artists {
                    for &i in recs.catalog.of_artist(a) {
                        if ok(i as usize) {
                            add(&mut cands, i as usize, Reason::Style(*index), 1.0);
                        }
                    }
                }
            }
            RadioSeed::Daylist { part, centre } => {
                for (id, closeness) in recs.daylist_pool(Axes::from_array(*centre)) {
                    if let Some(i) = recs.catalog.position(id).filter(|&i| ok(i)) {
                        add(&mut cands, i, Reason::DayPart(*part), closeness);
                    }
                }
                // Tracks without a sound join by being played in this part.
                for i in 0..recs.catalog.len() {
                    let plays = recs.taste.daypart_plays_at(i);
                    let here = plays[part.index()];
                    if here > 0.0 && ok(i) && !recs.index.contains(recs.info(i).id) {
                        let share = here / plays.iter().sum::<f32>();
                        add(&mut cands, i, Reason::DayPart(*part), share);
                    }
                }
            }
            _ => {}
        }
        // Never dry: favourites, then anything.
        if cands.len() < n {
            let mut fav: Vec<usize> = (0..recs.catalog.len()).filter(|&i| recs.taste.affinity_at(i) > 0.0 && ok(i)).collect();
            fav.sort_by(|x, y| recs.taste.affinity_at(*y).total_cmp(&recs.taste.affinity_at(*x)).then(x.cmp(y)));
            for i in fav.into_iter().take(FALLBACK) {
                add(&mut cands, i, Reason::Favourite, 0.0);
            }
        }
        if cands.len() < n && !recs.catalog.is_empty() {
            for _ in 0..FALLBACK {
                let i = rng.below(recs.catalog.len());
                if ok(i) {
                    add(&mut cands, i, Reason::Random, 0.0);
                }
            }
        }

        let (part, w_daypart) = match &self.seed {
            RadioSeed::Daylist { part, .. } => (*part, W_DAYLIST_DAYPART),
            _ => (recs.taste.day_part(), W_DAYPART),
        };
        let rest = if mix { MIX_REST } else { RADIO_REST };
        // A radio the runtime plays: the user model's say, and what is worn
        // cools.
        let ranker = self.ranker(recs);
        let cool = if self.attuned { COOL } else { 0.0 };
        // Each track its own draw, whatever else is a candidate.
        let jitter = rng.next_u64();
        let mut keys: Vec<usize> = cands.keys().copied().collect();
        keys.sort_unstable();
        let mut slots: Vec<Slot> = keys
            .into_iter()
            .map(|i| {
                let (reason, bonus) = cands[&i].clone();
                let mut s = recs.slot(i, reason, 0.0);
                let snd = match &s.vec {
                    Some(v) => sound.score(v),
                    None => -1.0,
                };
                let aff = (recs.taste.affinity_at(i) / 3.0).tanh();
                let lately = recs.taste.lately_at(i);
                s.score = snd
                    + W_AFFINITY * aff
                    + W_RELATION * bonus
                    + w_daypart * recs.taste.daypart_bonus_at(i, part)
                    - rest * lately
                    - recs.taste.dislike_near_at(i)
                    - cool * recs.taste.saturation_at(i)
                    + ranker.as_ref().map_or(0.0, |r| r.value(i))
                    + Rng::derive(jitter, i as u64).f32() * JITTER;

                s
            })
            .collect();

        if let RadioSeed::Artist(a) = self.seed {
            // The artist's own tracks by affinity, then the similar rest.
            let own_token = token_of_artist(a);
            let (mut own, mut rest): (Vec<Slot>, Vec<Slot>) = slots.into_iter().partition(|s| s.artists.contains(&own_token));
            for s in &mut own {
                let i = match s.item {
                    Item::Track(id) => recs.catalog.position(id).unwrap_or(0),
                    Item::Stream(_) => 0,
                };
                s.score = (recs.taste.affinity_at(i) / 3.0).tanh() + 0.3 * s.score;
            }
            sort_by_score(&mut own);
            sort_by_score(&mut rest);
            let want_own = ((n as f32) * ARTIST_OWN_SHARE).round() as usize;
            let mut out = select_unheard(recs, own, want_own, self.unheard_share(), rules, taken);
            let left = n - out.len();
            out.extend(select_unheard(recs, rest, left, self.unheard_share(), rules, taken));
            return out;
        }
        sort_by_score(&mut slots);
        // The best of a liked track's neighbours is taken first.
        if let Some(k) = slots.iter().position(|s| matches!(s.item, Item::Track(id) if recs.catalog.position(id).is_some_and(|i| pulled.contains(&i)))) {
            let s = slots.remove(k);
            slots.insert(0, s);
        }
        select_unheard(recs, slots, n, self.unheard_share(), rules, taken)
    }
}

/// The slot is new music or a track the user has not heard.
fn slot_unheard(recs: &Recs<'_>, s: &Slot) -> bool {
    match &s.item {
        Item::Track(id) => recs.catalog.position(*id).is_some_and(|i| !recs.heard(i)),
        Item::Stream(_) => true,
    }
}

/// What a session served takes its whole song along: a served version's
/// song (an off vocal or a TV size of it, by [`crate::taste::TrackInfo::song`])
/// and the versions of a served song are not picked in the same session.
/// Returns what it marked taken that was not, so a radio running dry can
/// give it back ([`RadioState::extend`] then marks the works of the
/// recently served only, as it keeps out only those).
pub(super) fn take_works(recs: &Recs<'_>, served: &HashSet<Item>, taken: &mut HashSet<Item>) -> Vec<Item> {
    let ids: HashSet<TrackId> = served
        .iter()
        .filter_map(|x| match x {
            Item::Track(id) => Some(*id),
            Item::Stream(_) => None,
        })
        .collect();
    let mut marked = Vec::new();
    if ids.is_empty() {
        return marked;
    }
    for t in recs.catalog.tracks() {
        if let Some(song) = t.song
            && (ids.contains(&song) || ids.contains(&t.id))
        {
            for item in [Item::Track(song), Item::Track(t.id)] {
                if taken.insert(item.clone()) {
                    marked.push(item);
                }
            }
        }
    }
    marked
}

/// Every catalog track of the works of `items` (by
/// [`crate::taste::Catalog::work_of`]: a song, its copies and its
/// versions).
pub(super) fn works_of(recs: &Recs<'_>, items: &HashSet<Item>) -> HashSet<Item> {
    let keys: HashSet<TrackId> = items
        .iter()
        .filter_map(|x| match x {
            Item::Track(id) => Some(recs.catalog.work_of(*id)),
            Item::Stream(_) => None,
        })
        .collect();
    if keys.is_empty() {
        return HashSet::new();
    }
    recs.catalog.tracks().iter().filter(|t| keys.contains(&t.song.unwrap_or(t.id))).map(|t| Item::Track(t.id)).collect()
}

/// The best `n` of `slots` (sorted) under the rules, at least `share` of
/// them unheard by the user while there are («Преимущественно новые
/// песни»).
fn select_unheard(recs: &Recs<'_>, slots: Vec<Slot>, n: usize, share: f32, rules: &Diversity, taken: &mut HashSet<Item>) -> Vec<Slot> {
    if share <= 0.0 {
        return select_diverse(slots, n, rules, taken);
    }
    let want = ((n as f32 * share).ceil() as usize).min(n);
    let fresh: Vec<Slot> = slots.iter().filter(|s| slot_unheard(recs, s)).cloned().collect();
    let mut out = select_more(fresh, want, n, rules, taken, &[]);
    let more = select_more(slots, n - out.len(), n, rules, taken, &out);
    out.extend(more);
    out
}

/// «Только новые»: the library's picks the user has heard (the last ones
/// first) make way for the new music of `reserve` that sounds like the
/// seed, while there is; `new` takes it.
pub(super) fn make_way(recs: &Recs<'_>, lib: &mut Vec<Slot>, new: &mut Vec<Slot>, reserve: Vec<Slot>, rules: &Diversity, taken: &mut HashSet<Item>) {
    if reserve.is_empty() {
        return;
    }
    let heard: Vec<usize> = (0..lib.len()).filter(|&k| !slot_unheard(recs, &lib[k])).collect();
    if heard.is_empty() {
        return;
    }
    let more = select_more(reserve, heard.len(), new.len() + heard.len(), rules, taken, new);
    let gone: HashSet<usize> = heard.iter().rev().take(more.len()).copied().collect();
    let mut k = 0;
    lib.retain(|_| {
        k += 1;
        !gone.contains(&(k - 1))
    });
    new.extend(more);
}

pub(super) fn sort_by_score(v: &mut [Slot]) {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
}

/// The `k` catalog tracks nearest to an embedded point (`ok` ones).
fn nearest_embedded(recs: &Recs<'_>, q: &Vector, k: usize, ok: &dyn Fn(usize) -> bool) -> Vec<(usize, f32)> {
    let mut all: Vec<(usize, f32)> = recs
        .index
        .iter()
        .filter_map(|(id, v)| {
            let i = recs.catalog.position(id)?;
            ok(i).then(|| (i, euclid(&recs.norm.embed(v), q)))
        })
        .collect();
    all.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    all.truncate(k);
    all
}

/// The median distance between tracks of the library, from a sample of
/// about 64 of them (2.0 without analysis).
pub(super) fn library_scale(recs: &Recs<'_>) -> f32 {
    let step = (recs.index.len() / 64).max(1);
    let vs: Vec<Vector> = recs.index.iter().step_by(step).map(|(_, v)| recs.norm.embed(v)).collect();
    let mut d: Vec<f32> = vs.windows(2).map(|w| euclid(&w[0], &w[1])).collect();
    d.sort_by(f32::total_cmp);
    d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(2.0)
}

/// The sound part of a score: pulled to the seed and the completed tracks,
/// pushed from the early-skipped ones, in units of the typical distance.
pub(super) struct Sound {
    seed: Option<Vector>,
    liked: Vec<Vector>,
    skipped: Vec<Vector>,
    scale: f32,
}

impl Sound {
    /// The scale is the median distance between the anchors (the seed and
    /// the lessons: an early skip pushes away everything about as close to
    /// it as it is to the seed), `fallback` with fewer than two anchors.
    pub(super) fn new(seed: Option<Vector>, liked: Vec<Vector>, skipped: Vec<Vector>, fallback: f32) -> Self {
        let scale = {
            let anchors: Vec<&Vector> = seed.iter().chain(&liked).chain(&skipped).collect();
            let mut d: Vec<f32> = Vec::new();
            for (i, a) in anchors.iter().enumerate() {
                for b in &anchors[i + 1..] {
                    d.push(euclid(*a, *b));
                }
            }
            d.sort_by(f32::total_cmp);
            d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(fallback)
        };
        Self {
            seed,
            liked,
            skipped,
            scale,
        }
    }

    pub(super) fn score(&self, v: &Vector) -> f32 {
        let mut pull = 0.0;
        let mut w = 0.0;
        if let Some(s) = &self.seed {
            pull += euclid(v, s);
            w += 1.0;
        }
        for l in &self.liked {
            pull += LIKED_PULL * euclid(v, l);
            w += LIKED_PULL;
        }
        let attract = if w > 0.0 { -pull / w / self.scale } else { 0.0 };
        let push: f32 = self.skipped.iter().map(|s| (1.0 - euclid(v, s) / self.scale).max(0.0)).sum();
        attract - SKIP_PUSH * push
    }
}

/// The user model's say in a pick of a radio by its seed (one ranker for
/// every radio): [`W_MODEL`] of its model of the next start at the
/// session's moment and of the chance of an early skip.
pub(super) struct Ranker<'a> {
    pub(super) model: &'a UserModel,
    pub(super) m: crate::model::Moment,
    pub(super) weights: Weights,
}

impl Ranker<'_> {
    fn value(&self, c: usize) -> f32 {
        let relevance = self.weights.next.score(&self.model.features(&self.m, c));
        let mut x = self.model.skip_features(&self.m, c, nikgob_core::model::EventContext::Radio);
        x[0] = 1.0;
        let p = self.model.skip_risk(c, self.weights.skip.p(&x)).min(0.99);
        W_MODEL * ((relevance / 2.0).tanh() + 0.5 * SKIP_VALUE * (1.0 - p).ln())
    }
}
