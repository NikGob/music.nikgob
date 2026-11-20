//! The model-driven radio of a track or a list: the picks by the user
//! model, the plan they keep to, the seed's anchor and scene, and
//! autoplay's trajectory.

use super::*;

impl RadioState {
    pub(super) fn by_model(
        &self,
        recs: &Recs<'_>,
        model: &UserModel,
        plan: &Plan,
        n: usize,
        served: &HashSet<Item>,
        rules: &Diversity,
        rng: &mut Rng,
        taken: &mut HashSet<Item>,
        again: Option<&Slot>,
        pull: Option<&(Item, Option<Vector>)>,
    ) -> (Vec<Slot>, bool) {
        let weights = self.learned.unwrap_or(model.weights);
        // A focused session's radio: a world a fifth at most.
        let rules = &if plan.focus { Diversity { world_share: FOCUS_WORLD_SHARE, ..*rules } } else { *rules };
        let mut sim = self.session.clone();
        // A run of early skips turns the whole batch away (the simulated
        // plays after it would end the run at once).
        let shifted = sim.skip_streak(model) >= SKIP_SHIFT;
        // The taste profiles of the rejections running: two of them leave
        // their profile while another that goes with the seed is left.
        let left: HashSet<usize> = if shifted {
            sim.tracks()
                .rev()
                .take_while(|p| Session::verdict(p, model).rejects())
                .filter_map(|p| model.position(p.track).and_then(|i| model.profile_at(i)))
                .collect()
        } else {
            HashSet::new()
        };
        // The session goes on from its last start (a session of the radio's
        // own tracks has no clock: it starts from 0).
        let mut ts = sim.last_ts().unwrap_or(recs.now());
        let play = |sim: &mut Session, ts: &mut i64, t: TrackId| {
            *ts += SIM_MS;
            sim.start(t, *ts, nikgob_core::model::EventContext::Radio, None);
            sim.end(t, Ended::Complete);
        };
        // What is queued after the track playing plays first.
        let order: Vec<TrackId> = self.served.iter().filter_map(|x| match x {
            Item::Track(t) => Some(*t),
            Item::Stream(_) => None,
        }).collect();
        let queued: Vec<TrackId> = match sim.last().and_then(|p| order.iter().rposition(|t| *t == p.track)) {
            Some(k) => order[k + 1..].to_vec(),
            None => match &self.last {
                Some(Item::Track(t)) => vec![*t],
                _ => Vec::new(),
            },
        };
        for t in queued.into_iter().chain(again.and_then(|s| match s.item {
            Item::Track(t) => Some(t),
            Item::Stream(_) => None,
        })) {
            play(&mut sim, &mut ts, t);
        }
        let recent: HashSet<&Item> = self.recent.iter().collect();
        let seed = match &self.seed {
            RadioSeed::Track(t) => Some(*t),
            // A list with one track the model knows: that track's radio.
            RadioSeed::Tracks(tracks) => {
                let mut known = tracks.iter().copied().filter(|t| model.position(*t).is_some());
                known.next().filter(|_| known.next().is_none())
            }
            _ => None,
        };
        // What the session played, and what played lately, rest while
        // anything else is left («Не повторять сыгранное»).
        let in_session: HashSet<TrackId> = self.session.tracks().map(|p| p.track).collect();
        let base = |i: usize| {
            let t = recs.info(i);
            let item = Item::Track(t.id);
            !recs.blocked(i)
                && !served.contains(&item)
                && seed != Some(t.id)
                && (self.not_world.is_none() || t.world != self.not_world)
                && !recent.contains(&item)
        };
        let ok = |i: usize| base(i) && !in_session.contains(&recs.info(i).id) && !recs.played_within(i, self.no_repeat_ms);
        // Autoplay's trajectory (none: the seed alone).
        let traj: Vec<usize> = self.trajectory().iter().filter_map(|t| model.position(*t)).collect();
        let traj = if traj.len() >= TRAJECTORY_MIN { traj } else { Vec::new() };
        // «Похоже на X» stays near X: within its sound radius and of a
        // kind of song that goes with it; the session only leans it.
        // Autoplay stays near its trajectory so.
        let anchor = if traj.is_empty() { SeedAnchor::of(model, seed, self.seed_facts.as_ref()) } else { SeedAnchor::of_trajectory(model, &traj, seed) };
        // What the collective graph links to the seed goes too, beyond the
        // radius of its sound (a kind of song that goes with it); to the
        // trajectory, the more of it the more.
        let linked = if traj.is_empty() { seed_links(model, seed) } else { trajectory_links(model, &traj) };
        // A liked track's neighbours lead the first pick: near it in sound
        // (closer than a radio of it goes), or linked to it.
        let pulled: Option<(SeedAnchor, HashMap<usize, f32>)> = pull.and_then(|(item, raw)| {
            let t = match item {
                Item::Track(t) => model.position(*t).map(|_| *t),
                Item::Stream(_) => None,
            };
            let raw = raw.or_else(|| match item {
                Item::Stream(id) => self.new_pool.iter().find(|x| x.id == *id).and_then(|x| x.vector),
                Item::Track(_) => None,
            });
            let facts = SeedFacts { vector: raw, ..SeedFacts::default() };
            let mut a = SeedAnchor::of(model, t, Some(&facts));
            a.point?;
            a.radius *= PULL_RADIUS;
            Some((a, seed_links(model, t)))
        });
        let removed: Vec<Vector> = self.removed.iter().filter_map(|x| self.model_vector(model, x)).collect();
        let near = |i: usize| anchor.holds(model, i) || (linked.get(&i).is_some_and(|w| *w >= LINKED) && anchor.goes(model, i));
        // The seed's scene: its artists' tracks and what the graph links to
        // them, and its language. A seed with few unheard tracks of it near
        // keeps to it first, of any sound, before it crosses to another.
        let mut artists = self.seed_artists(recs);
        for &i in &traj {
            if let Some(a) = model.main_artist_at(i)
                && !artists.contains(&a)
            {
                artists.push(a);
            }
        }
        let kin = scene_of(model, seed, &artists, &linked);
        let lane = anchor.lane.filter(|l| *l != SongLane::Unknown);
        let same_lane = |i: usize| lane.is_some_and(|l| model.song_type_at(i).lane() == l);
        let in_scene = |i: usize| kin.contains_key(&i) || same_lane(i);
        let knows_scene = !kin.is_empty() || lane.is_some();
        let sparse = knows_scene && {
            let (mut of_scene, mut all) = (0usize, 0usize);
            for i in (0..model.len()).filter(|&i| ok(i) && near(i)) {
                all += 1;
                of_scene += usize::from(in_scene(i));
            }
            of_scene < SPARSE_SCENE || 2 * of_scene < all
        };
        // The profile it keeps to: its own tracks, and the
        // neighbouring profile's once two early skips ran or fewer than
        // [`UNHEARD_MIN`] of its own are left; a background radio never
        // widens, takes only what suits the background from a profile
        // without words, and keeps to its band of energy.
        let neighbour = plan.profile.filter(|_| !plan.background).and_then(|j| neighbour(model, j));
        let (m_own, m_next): (Vec<f32>, Vec<f32>) = match plan.profile {
            Some(j) => (0..model.len())
                .map(|i| {
                    let m = model.membership_at(i);
                    (m.get(j).copied().unwrap_or(0.0), neighbour.and_then(|o| m.get(o).copied()).unwrap_or(0.0))
                })
                .unzip(),
            None => (Vec::new(), Vec::new()),
        };
        // After a mixed session autoplay keeps to the profiles of its
        // trajectory.
        let traj_profiles: HashSet<usize> = if plan.profile.is_none() { traj.iter().filter_map(|&i| model.profile_at(i)).collect() } else { HashSet::new() };
        let own = |i: usize| match plan.profile {
            Some(_) => m_own[i] >= MEMBER,
            None => traj_profiles.is_empty() || model.profile_at(i).is_none_or(|p| traj_profiles.contains(&p)),
        };
        let fresh = || (0..model.len()).filter(|&i| m_own[i] >= MEMBER && ok(i) && anchor.goes(model, i)).count();
        let wide = neighbour.is_some() && (self.wide || shifted || fresh() < UNHEARD_MIN);
        // Widened, the profile beyond the seed's neighbourhood joins too.
        let next = |i: usize| wide && (m_next[i] >= MEMBER || m_own[i] >= MEMBER) && anchor.goes(model, i);
        let quiet = |i: usize| {
            !plan.background
                || ((!plan.wordless || recs.suits_background(i))
                    && plan.band.is_none_or(|(lo, hi)| model.energy_at(i).is_none_or(|e| (lo..=hi).contains(&e))))
        };
        // A mixed session's radio goes in blocks of a profile.
        let blocks = plan.profile.is_none() && !plan.background;
        // The album goes on only from the user's own album, in its order
        // while each pick is the next on it; a world never over the rules.
        let mut continuing =
            self.session.last().is_some_and(|p| p.context == nikgob_core::model::EventContext::Album) && self.session.album_session(model);
        let mut tail: Vec<Placed> = self.tail.clone();
        let mut per_world: HashMap<u64, usize> = HashMap::new();
        let world_cap = rules.world_cap(n);
        let jitter = rng.next_u64();
        // «Преимущественно новые песни»: picks unheard so far.
        let unheard_share = self.unheard_share();
        let mut fresh = 0usize;
        let mut out: Vec<Slot> = Vec::with_capacity(n);
        for _ in 0..n {
            let mut m = model.moment(&sim, ts);
            // The session's rejections by both sounds; the songs taken out
            // of the queue by bliss as before.
            let push = model.push_probe(&sim, Some(recs.index));
            let removed_push: Vec<(Vector, f32)> = removed.iter().map(|v| (*v, REMOVED_PUSH)).collect();
            m.album_session &= continuing;
            m.world_session = None;
            m.anchor = if traj.is_empty() { anchor.lean(m.anchor) } else { anchor.drift(sim.anchor_of(model, TRAJECTORY_RECENT)) };
            m.pushes.extend(removed.iter().map(|v| (*v, REMOVED_PUSH)));
            let first_pull = pulled.as_ref().filter(|_| out.is_empty());
            if let Some((pa, _)) = first_pull {
                m.anchor = pa.point;
            }
            // What the session played lately rests: back once
            // [`REPEAT_AFTER`] others played since it, at once when it
            // played twice running.
            let twice = sim.repeat_one().then(|| sim.last().map(|p| p.track)).flatten();
            let lately: HashSet<TrackId> = sim.tracks().rev().take(REPEAT_AFTER).map(|p| p.track).filter(|t| Some(*t) != twice).collect();
            let free = |i: usize| {
                let t = recs.info(i);
                !taken.contains(&Item::Track(t.id)) && !t.song.is_some_and(|s| taken.contains(&Item::Track(s))) && !lately.contains(&t.id)
            };
            // This pick is to be unheard: the set of candidates it is taken
            // from below keeps its unheard part apart (the nearest unheard,
            // not only the unheard of the nearest), so whatever may follow
            // stays as it is.
            let want_unheard = unheard_share > 0.0 && (fresh as f32) < unheard_share * (out.len() + 1) as f32 - 1e-3;
            let unheard: RefCell<Vec<usize>> = RefCell::new(Vec::new());
            let gather = |f: &dyn Fn(usize) -> bool| -> Vec<usize> {
                let c = model.candidates(&m, f);
                if want_unheard && !c.is_empty() {
                    *unheard.borrow_mut() = model.candidates(&m, &|i| f(i) && !recs.heard(i));
                }
                c
            };
            // A sparse scene first: what is near of it, then its artists'
            // and their neighbours' tracks of any sound, then its language.
            let mut cands = match first_pull {
                Some((pa, links)) => {
                    gather(&|i| ok(i) && free(i) && quiet(i) && (pa.holds(model, i) || links.get(&i).is_some_and(|w| *w >= LINKED)))
                }
                None => Vec::new(),
            };
            if cands.is_empty() && !left.is_empty() {
                cands = gather(&|i| {
                    ok(i) && free(i) && quiet(i) && (own(i) || next(i)) && anchor.goes(model, i) && model.profile_at(i).is_some_and(|p| !left.contains(&p))
                });
            }
            if cands.is_empty() && sparse {
                let mut c = gather(&|i| ok(i) && free(i) && quiet(i) && in_scene(i) && near(i));
                if c.is_empty() {
                    c = kin.keys().copied().filter(|&i| ok(i) && free(i) && quiet(i)).collect();
                    c.sort_unstable();
                    if want_unheard {
                        *unheard.borrow_mut() = c.iter().copied().filter(|&i| !recs.heard(i)).collect();
                    }
                }
                if c.is_empty() {
                    c = gather(&|i| ok(i) && free(i) && quiet(i) && same_lane(i));
                }
                cands = c;
            }
            if cands.is_empty() {
                cands = gather(&|i| ok(i) && free(i) && quiet(i) && ((own(i) && near(i)) || next(i)));
            }
            if cands.is_empty() && plan.profile.is_some() {
                // The profile beyond the seed's neighbourhood.
                cands = gather(&|i| ok(i) && free(i) && quiet(i) && own(i) && anchor.goes(model, i));
            }
            if cands.is_empty() && plan.background {
                // A background profile played through: what rested a day
                // and a half comes back.
                cands = gather(&|i| {
                    own(i) && quiet(i) && free(i) && !recs.blocked(i) && seed != Some(recs.info(i).id) && !recs.played_within(i, BG_REPEAT_MS)
                });
            }
            if cands.is_empty() {
                cands = gather(&|i| ok(i) && free(i) && near(i));
            }
            if cands.is_empty() && knows_scene {
                // Its scene of any sound before another scene.
                cands = gather(&|i| ok(i) && free(i) && in_scene(i));
            }
            if cands.is_empty() {
                cands = gather(&|i| ok(i) && free(i));
            }
            if cands.is_empty() {
                // Nothing unplayed is left: what played lately comes back.
                cands = gather(&|i| base(i) && free(i) && (!knows_scene || in_scene(i)));
            }
            if cands.is_empty() {
                cands = gather(&|i| base(i) && free(i));
            }
            if cands.is_empty() {
                break;
            }
            let mut w = weights.next;
            if !continuing {
                w.w[F_NEXT_ALBUM] = 0.0;
            }
            if shifted || m.skip_streak >= SKIP_SHIFT {
                w.w[F_SKIP_PUSH] = 2.0 * w.w[F_SKIP_PUSH].max(HAND[F_SKIP_PUSH]);
                w.w[F_SAME_LANE] = 2.0 * w.w[F_SAME_LANE].max(HAND[F_SAME_LANE]);
            }
            // A jump in sound costs twice in the background, half focused.
            if plan.background {
                w.w[F_SOUND] *= JUMP_BG;
            } else if plan.focus {
                w.w[F_SOUND] *= JUMP_FOCUS;
            }
            // The block going on: its profile and how long it ran.
            let (block, run) = if blocks {
                let ps: Vec<Option<usize>> =
                    sim.tracks().rev().filter(|p| p.ended != Ended::Early).map(|p| model.position(p.track).and_then(|i| model.profile_at(i))).collect();
                let first = ps.first().copied().flatten();
                (first, ps.iter().take_while(|p| first.is_some() && **p == first).count())
            } else {
                (None, 0)
            };
            let seq: Vec<Seen<'_>> = tail.iter().map(|p| (p.artists.as_slice(), p.album, p.world)).collect();
            // What a DJ would mind of the step from the track before.
            let prev_dj: Option<Dj> = m.prev.and_then(|p| recs.index.get(model.id(p))).map(Dj::of);
            let dj_step = |c: usize| match (&prev_dj, recs.index.get(model.id(c))) {
                (Some(a), Some(v)) => dj_cost(a, &Dj::of(v)),
                _ => 0.0,
            };
            let best_of = |cands: Vec<usize>| -> Option<((usize, f32), Slot, bool)> {
                let mut best: Option<((usize, f32), Slot, bool)> = None;
                for c in cands {
                    let mut f = model.features(&m, c);
                    if let Some(p) = &push {
                        f[F_SKIP_PUSH] = model.skip_push_by(p, c) + model.skip_push_of(&removed_push, c);
                    }
                    let parts: Vec<f32> = w.w.iter().zip(&f).map(|(a, b)| a * b).collect();
                    let relevance: f32 = parts.iter().sum();
                    let mut x = model.skip_features(&m, c, nikgob_core::model::EventContext::Radio);
                    x[0] = 1.0;
                    let p_skip = model.skip_risk(c, weights.skip.p(&x)).min(0.99);
                    let rested = model.tracks[c].last_start.map_or(1.0, |l| 1.0 - (-((m.now - l).max(0) as f64) / REST_HALF_MS as f64).exp2() as f32);
                    let joins = match (block, model.profile_at(c)) {
                        (Some(b), Some(p)) if b == p => match run {
                            r if r < BLOCK_LEN.0 => BLOCK,
                            r if r >= BLOCK_LEN.1 => -BLOCK,
                            _ => 0.0,
                        },
                        (Some(_), Some(_)) if run < BLOCK_LEN.0 => -BLOCK,
                        _ => 0.0,
                    };
                    let value = relevance + SKIP_VALUE * (1.0 - p_skip).ln() + RADIO_REST * rested + joins
                        - recs.taste.dislike_near_at(c)
                        - COOL * recs.taste.saturation_at(c)
                        - DJ_VALUE * dj_step(c)
                        + MODEL_JITTER * Rng::derive(jitter, c as u64).f32();
                    let reason = if parts[F_NEXT_ALBUM] > 0.0 && parts[F_NEXT_ALBUM] >= parts[F_COOC] {
                        Reason::NextOnAlbum
                    } else if parts[F_COOC].max(parts[F_PGRAPH]) > 0.5 && parts[F_COOC].max(parts[F_PGRAPH]) >= parts[F_SOUND] + 1.0 {
                        m.prev.map_or(Reason::SoundsLikeSeed, |p| Reason::FollowsOften(model.id(p)))
                    } else if parts[F_GRAPH] > 0.5 && parts[F_GRAPH] >= parts[F_SOUND] + 0.5 {
                        m.prev.or(seed.and_then(|t| model.position(t))).map_or(Reason::SoundsLikeSeed, |p| Reason::ListenedTogether(model.id(p)))
                    } else {
                        seed.map_or(Reason::SoundsLikeSeed, Reason::SimilarTo)
                    };
                    let slot = recs.slot(c, reason, value);
                    let free = (m.album_session && m.next_album == Some(c)) || (m.world_session.is_some() && slot.world == m.world_session);
                    let seen = (slot.artists.as_slice(), slot.album, slot.world);
                    let (violations, run) = if free {
                        (0, false)
                    } else {
                        let over = slot.world.is_some_and(|w| per_world.get(&w).is_some_and(|k| *k >= world_cap));
                        (rules.weighted(&seq, seen) + crate::pick::W_SHARE * usize::from(over), rules.world_run(&seq, seen))
                    };
                    let key = (violations, -value);
                    if best.as_ref().is_none_or(|(b, _, _)| key.0 < b.0 || (key.0 == b.0 && key.1 < b.1)) {
                        best = Some((key, slot, run));
                    }
                }
                best
            };
            let mut best = best_of(cands);
            // The best unheard one, unless it breaks more of the rules.
            let unheard = unheard.take();
            if !unheard.is_empty()
                && let Some(b) = best_of(unheard).filter(|b| best.as_ref().is_none_or(|a| b.0.0 <= a.0.0))
            {
                best = Some(b);
            }
            // Only one world left near the seed, and it ran twice: a track a
            // little farther, of a kind that goes with the seed, before a
            // world thrice running (the user's rule; the radius is ours).
            if best.as_ref().is_some_and(|b| b.2) {
                let wider = model.candidates(&m, &|i| {
                    ok(i) && free(i) && quiet(i) && !near(i) && anchor.goes(model, i) && (own(i) || next(i)) && (!sparse || in_scene(i))
                });
                // The wider step keeps «Преимущественно новые песни» too.
                let wider_unheard: Vec<usize> = if want_unheard {
                    wider.iter().copied().filter(|&i| !recs.heard(i)).collect()
                } else {
                    Vec::new()
                };
                let pick = best_of(wider);
                let pick_unheard = best_of(wider_unheard).filter(|b| pick.as_ref().is_none_or(|a| b.0.0 <= a.0.0));
                if let Some(b) = pick_unheard.or(pick).filter(|b| best.as_ref().is_some_and(|a| b.0.0 < a.0.0)) {
                    best = Some(b);
                }
            }
            let best = best.map(|(k, s, _)| (k, s));
            let Some((_, slot)) = best else { break };
            let Item::Track(id) = slot.item else { break };
            continuing &= m.next_album.is_some_and(|c| model.id(c) == id);
            fresh += usize::from(model.position(id).is_some_and(|i| !recs.heard(i)));

            take(taken, &slot);
            tail.push(Placed::of(&slot));
            if let Some(w) = slot.world {
                *per_world.entry(w).or_default() += 1;
            }
            play(&mut sim, &mut ts, id);
            out.push(slot);
        }
        (out, wide)
    }

    /// The profile and the mode a track's radio keeps to: «Похоже на X» is pure in X's profile, autoplay in the profile
    /// its session is pure in (none in a mixed one); a background session
    /// keeps to its profile in the background, a focused one wanders more.
    pub(super) fn plan(&self, model: &UserModel) -> Plan {
        let seed = match &self.seed {
            RadioSeed::Track(t) => model.position(*t),
            _ => None,
        };
        // A seed the catalog lacks: the profile most of its artists' tracks
        // are of.
        let own = seed.and_then(|i| model.profile_at(i)).or_else(|| {
            let f = self.seed_facts.as_ref()?;
            let mut count: HashMap<usize, usize> = HashMap::new();
            for a in &f.artists {
                for &i in model.of_artist(*a) {
                    if let Some(j) = model.profile_at(i as usize) {
                        *count.entry(j).or_default() += 1;
                    }
                }
            }
            count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(j, _)| j)
        });
        // A list: the profile most of it is of (more than half), else none
        // (it keeps to the list's profiles).
        let own = own.or_else(|| {
            let RadioSeed::Tracks(tracks) = &self.seed else { return None };
            list_profile(model, tracks)
        });
        // The session's profile only if the seed is of it: a seed of
        // another kind (picked after a mix) is followed by its own.
        let fits = |j: usize| match (seed, own) {
            (Some(i), _) => model.membership_at(i).get(j).is_some_and(|m| *m >= MEMBER),
            (None, Some(o)) => o == j,
            (None, None) => true,
        };
        let live = model.live(&self.session);
        let mut profile = if self.follow {
            match live.mode {
                Some(Mode::Pure(j)) if fits(j) => Some(j),
                Some(Mode::Pure(_)) => own,
                Some(Mode::Mixed) => None,
                None => own,
            }
        } else {
            own
        };
        let background = live.p_bg >= 0.5;
        if background {
            profile = profile.or(live.profile);
        }
        let mut plan = Plan { profile, background, focus: !background && live.focus, ..Plan::default() };
        if background && let Some((j, p)) = profile.and_then(|j| Some((j, model.profiles.list.get(j)?))) {
            plan.wordless = p.stats.instrumental >= WORDLESS;
            plan.lane = Some(p.lane);
            let mut e: Vec<f32> = (0..model.len()).filter(|&i| model.profile_at(i) == Some(j)).filter_map(|i| model.energy_at(i)).collect();
            e.sort_by(f32::total_cmp);
            if e.len() >= BAND_FROM {
                let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
                plan.band = Some((at(BG_BAND.0), at(BG_BAND.1)));
            }
        }
        plan
    }
}

/// A track is of a radio's profile with this membership in it…
const MEMBER: f32 = 0.35;
/// …and the radio widens to the neighbouring profile with fewer than this
/// many of its own left to play.
const UNHEARD_MIN: usize = 30;
/// A background profile this much without words: its radio takes only what
/// suits the background ([`crate::background`])…
const WORDLESS: f32 = 0.8;
/// …and only the middle of its energy (percentiles of the profile's own),
/// once it has this many tracks with a sound.
const BG_BAND: (f32, f32) = (0.15, 0.7);
const BAND_FROM: usize = 12;
/// A jump in sound costs this many times the model's own in the
/// background, and in a focused session.
const JUMP_BG: f32 = 2.0;
const JUMP_FOCUS: f32 = 0.5;
/// A focused session's radio: this many times the new music, a world a
/// fifth at most.
pub(super) const FOCUS_NEW: f32 = 1.5;
const FOCUS_WORLD_SHARE: f32 = 0.2;
/// New music in the background: after this much of the session, one in
/// six at most…
pub(super) const BG_NEW_AFTER_MS: i64 = 20 * 60_000;
pub(super) const BG_NEW_SHARE: f32 = 1.0 / 6.0;
/// …and a background profile played through plays what rested this long.
const BG_REPEAT_MS: i64 = 36 * HOUR_MS;
/// A mixed session's radio keeps a profile for a block of three or four:
/// this much for the block going on before three, against it from four.
const BLOCK: f32 = 0.3;
const BLOCK_LEN: (usize, usize) = (3, 4);

/// What a track's radio keeps to ([`RadioState::plan`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Plan {
    /// The profile (none: a mixed session's radio takes all).
    profile: Option<usize>,
    pub(super) background: bool,
    pub(super) focus: bool,
    /// Background: the profile is mostly without words, and its kind.
    pub(super) wordless: bool,
    lane: Option<LaneKey>,
    /// Background: the profile's band of energy.
    band: Option<(f32, f32)>,
}

impl Plan {
    /// New music sounding `v` (embedded) does not get in the way of a
    /// background radio: of its profile by its sound, in
    /// its band of energy.
    pub(super) fn quiet_new(&self, model: &UserModel, v: Option<&Vector>) -> bool {
        let (Some(v), Some(j)) = (v, self.profile) else { return false };
        let kind = match self.lane {
            Some(LaneKey::Sung(l)) => SongType { voice: Voice::Sung, lang: Some(l), ..SongType::default() },
            Some(LaneKey::Instrumental) => SongType { voice: Voice::Instrumental, ..SongType::default() },
            _ => SongType::default(),
        };
        model.profiles.membership(v, &kind).get(j).is_some_and(|m| *m >= MEMBER)
            && self.band.is_none_or(|(lo, hi)| (lo..=hi).contains(&model.energy_of(v)))
    }
}

/// The profile nearest `j` of a kind of song the user goes on to from its
/// kind ([`UserModel::lanes_go`]).
fn neighbour(model: &UserModel, j: usize) -> Option<usize> {
    let lane = model.profiles.list.get(j)?.lane.song_lane();
    (0..model.profiles.len())
        .filter(|&o| o != j && model.lanes_go(lane, model.profiles.list[o].lane.song_lane()))
        .min_by(|a, b| model.profiles.distance(j, *a).total_cmp(&model.profiles.distance(j, *b)).then(a.cmp(b)))
}

/// A track the collective graph links to the seed this much goes in its
/// radio beyond the seed's sound radius.
const LINKED: f32 = 0.3;

/// What the collective graph links to the track `seed`: its neighbours,
/// and the tracks of the artists like its at [`crate::model`]'s weight of
/// an artist, by catalog position.
fn seed_links(model: &UserModel, seed: Option<TrackId>) -> HashMap<usize, f32> {
    let mut out: HashMap<usize, f32> = HashMap::new();
    let Some(s) = seed.and_then(|t| model.position(t)) else { return out };
    for &(c, w) in model.graph.near(s) {
        out.insert(c as usize, w);
    }
    if let Some(a) = model.main_artist_at(s) {
        for &(b, w) in model.graph.similar_artists(a) {
            for &i in model.of_artist(b) {
                let e = out.entry(i as usize).or_default();
                *e = e.max(0.5 * w);
            }
        }
    }
    out
}

/// The seed's scene: the tracks of its artists (1) and of the artists the
/// graph links to them (half the link), and the tracks `linked` to it.
fn scene_of(model: &UserModel, seed: Option<TrackId>, artists: &[ArtistId], linked: &HashMap<usize, f32>) -> HashMap<usize, f32> {
    let mut out: HashMap<usize, f32> = linked.iter().filter(|(_, w)| **w >= LINKED).map(|(i, w)| (*i, *w)).collect();
    for &a in artists {
        for &i in model.of_artist(a) {
            out.insert(i as usize, 1.0);
        }
        for &(b, w) in model.graph.similar_artists(a) {
            for &i in model.of_artist(b) {
                let e = out.entry(i as usize).or_default();
                *e = e.max(0.5 * w);
            }
        }
    }
    if let Some(p) = seed.and_then(|t| model.position(t)) {
        out.remove(&p);
    }
    out
}

/// The share of a model radio's anchor that is its seed: the session's
/// sound leans it, never leads it away.
const SEED_HOLD: f32 = 0.6;
/// A track of a radio is at most this many median distances from the seed…
pub(super) const SEED_RADIUS: f32 = 1.0;
/// …or as far as the seed's nearest eighth of the library, if nearer…
const SEED_SHARE: usize = 8;
/// …but always as far as its nearest this many.
const SEED_AT_LEAST: usize = 30;

/// Where «Похоже на X» may go: near X in sound, songs of a kind the user
/// goes on to from X's ([`UserModel::lanes_go`]: no rap after a
/// soundtrack unless the user does so), a track without a vector only of
/// X's world or artist.

pub(super) struct SeedAnchor {
    pub(super) point: Option<Vector>,
    pub(super) radius: f32,
    lane: Option<SongLane>,
    world: Option<u64>,
    artist: Option<ArtistId>,
    /// Autoplay's trajectory: its tracks' sounds (a track may follow
    /// within the radius of any of them, not only of the middle)…
    points: Vec<Vector>,
    /// …and the other kinds of song it had twice or more.
    lanes: Vec<SongLane>,
}

impl SeedAnchor {
    pub(super) fn of(model: &UserModel, seed: Option<TrackId>, facts: Option<&SeedFacts>) -> Self {
        let pos = seed.and_then(|t| model.position(t));
        // A seed the catalog lacks: its own sound if analysed, else its
        // artists' middle; their commonest kind of song, world and artist.
        let theirs: Vec<usize> = match (pos, facts) {
            (None, Some(f)) => f.artists.iter().flat_map(|a| model.of_artist(*a).iter().map(|&i| i as usize)).collect(),
            _ => Vec::new(),
        };
        let commonest = |v: Vec<u64>| {
            let mut count: HashMap<u64, usize> = HashMap::new();
            for x in v {
                *count.entry(x).or_default() += 1;
            }
            count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(x, _)| x)
        };
        let middle = || -> Option<Vector> {
            let vs: Vec<&Vector> = theirs.iter().filter_map(|&i| model.embedded_at(i)).collect();
            (!vs.is_empty()).then(|| std::array::from_fn(|k| vs.iter().map(|v| v[k]).sum::<f32>() / vs.len() as f32))
        };
        let their_lane = || {
            let lanes = [SongLane::Instrumental, SongLane::Russian, SongLane::Japanese, SongLane::Western];
            let v: Vec<u64> =
                theirs.iter().filter_map(|&i| lanes.iter().position(|l| *l == model.song_type_at(i).lane()).map(|k| k as u64)).collect();
            commonest(v).map(|k| lanes[k as usize]).or_else(|| facts.and_then(|f| f.lane))
        };
        let point = pos
            .and_then(|p| model.embedded_at(p).copied())
            .or_else(|| facts.and_then(|f| f.vector).map(|v| model.embed(&v)))
            .or_else(|| if pos.is_none() { middle() } else { None });
        let radius = point.map_or(f32::INFINITY, |sv| radius_around(model, &sv));
        let (lane, world, artist) = match pos {
            Some(p) => (Some(model.song_type_at(p).lane()), model.world_at(p), model.main_artist_at(p)),
            None => (
                their_lane(),
                commonest(theirs.iter().filter_map(|&i| model.world_at(i)).collect()),
                facts.and_then(|f| f.artists.iter().copied().find(|a| !model.of_artist(*a).is_empty())),
            ),
        };
        Self { point, radius, lane, world, artist, points: Vec::new(), lanes: Vec::new() }
    }

    /// Track `i` is of a kind of song that goes with the seed's (or with
    /// another kind the trajectory had).
    fn goes(&self, model: &UserModel, i: usize) -> bool {
        let l = model.song_type_at(i).lane();
        self.lane.is_none_or(|a| model.lanes_go(a, l)) || self.lanes.iter().any(|a| model.lanes_go(*a, l))
    }

    /// Track `i` may follow.
    fn holds(&self, model: &UserModel, i: usize) -> bool {
        if !self.goes(model, i) {
            return false;
        }
        match (&self.point, model.embedded_at(i)) {
            (Some(p), Some(v)) if self.points.is_empty() => euclid(p, v) <= self.radius,
            (Some(_), Some(v)) => self.points.iter().any(|q| euclid(q, v) <= self.radius),
            (Some(_), None) => {
                (self.world.is_some() && model.world_at(i) == self.world) || (self.artist.is_some() && model.main_artist_at(i) == self.artist)
            }
            (None, _) => true,
        }
    }

    /// The anchor of the moment, held to the seed.
    fn lean(&self, session: Option<Vector>) -> Option<Vector> {
        match (self.point, session) {
            (Some(p), Some(s)) => Some(std::array::from_fn(|k| SEED_HOLD * p[k] + (1.0 - SEED_HOLD) * s[k])),
            (Some(p), None) => Some(p),
            (None, s) => s,
        }
    }

    /// Where autoplay may go after the trajectory `traj` (catalog
    /// positions) that ends in `seed`: near any of its tracks, as far as a
    /// radio of the seed goes, songs of a kind that goes with the kind most
    /// of it is (or with another it had twice or more); its middle is the
    /// context the anchor drifts from; the seed's world and artist for a
    /// track without a sound.
    fn of_trajectory(model: &UserModel, traj: &[usize], seed: Option<TrackId>) -> Self {
        let base = Self::of(model, seed, None);
        let points: Vec<Vector> = traj.iter().filter_map(|&i| model.embedded_at(i).copied()).collect();
        if points.is_empty() {
            return base;
        }
        let point: Vector = std::array::from_fn(|k| points.iter().map(|v| v[k]).sum::<f32>() / points.len() as f32);
        let radius = radius_around(model, base.point.as_ref().unwrap_or(&point));
        let mut count: HashMap<SongLane, usize> = HashMap::new();
        for &i in traj {
            *count.entry(model.song_type_at(i).lane()).or_default() += 1;
        }
        let best = count.values().copied().max().unwrap_or(0);
        let lane = match base.lane {
            Some(l) if count.get(&l) == Some(&best) => Some(l),
            _ => count.iter().filter(|(_, c)| **c == best).map(|(l, _)| *l).min_by_key(|l| crate::model::lane_index(*l)),
        };
        let mut lanes: Vec<SongLane> = count.iter().filter(|(l, c)| **c >= 2 && Some(**l) != lane && **l != SongLane::Unknown).map(|(l, _)| *l).collect();
        lanes.sort_by_key(|l| crate::model::lane_index(*l));
        Self { point: Some(point), radius, lane, world: base.world, artist: base.artist, points, lanes }
    }

    /// The anchor of autoplay's moment: [`TRAJECTORY_DRIFT`] the session's
    /// last approved (`recent`), the trajectory's middle the rest.
    fn drift(&self, recent: Option<Vector>) -> Option<Vector> {
        match (self.point, recent) {
            (Some(p), Some(r)) => Some(std::array::from_fn(|k| TRAJECTORY_DRIFT * r[k] + (1.0 - TRAJECTORY_DRIFT) * p[k])),
            (p, r) => r.or(p),
        }
    }
}

/// How far a radio of a track sounding `sv` (embedded) may go: its nearest
/// eighth of the library or [`SEED_RADIUS`] median distances, the nearer,
/// but as far as its nearest [`SEED_AT_LEAST`].
fn radius_around(model: &UserModel, sv: &Vector) -> f32 {
    let mut d: Vec<f32> = (0..model.len()).filter_map(|i| model.embedded_at(i)).map(|v| euclid(v, sv)).collect();
    d.sort_by(f32::total_cmp);
    let last = d.len().saturating_sub(1);
    let share = d.get((d.len() / SEED_SHARE).min(last)).copied().unwrap_or(f32::INFINITY);
    let floor = d.get(SEED_AT_LEAST.min(last)).copied().unwrap_or(f32::INFINITY);
    share.min(SEED_RADIUS * model.scale()).max(floor)
}

/// Autoplay's trajectory in `session`: the last [`TRAJECTORY_MAX`] tracks
/// not skipped ([`Session::played_through`]: to the end or to the outro,
/// liked, or still playing; a skip past the half was a skip all the same),
/// back to the user's own last pick (where the context it goes on from
/// began); fewer than [`TRAJECTORY_MIN`]: none.
pub(super) fn trajectory_of(session: &Session, model: &UserModel) -> Vec<TrackId> {
    let mut out: Vec<TrackId> = Vec::new();
    for p in session.tracks().rev() {
        if Session::played_through(p, model) && !out.contains(&p.track) {
            out.push(p.track);
        }
        if out.len() == TRAJECTORY_MAX || p.origin == Some(nikgob_core::model::EventOrigin::Manual) {
            break;
        }
    }
    out.reverse();
    if out.len() < TRAJECTORY_MIN {
        out.clear();
    }
    out
}

/// What the collective graph links to the trajectory `traj` (catalog
/// positions): each one's links ([`seed_links`]), the strongest, gaining
/// [`TRAJECTORY_MEET`] for each more track of the trajectory linking it.
fn trajectory_links(model: &UserModel, traj: &[usize]) -> HashMap<usize, f32> {
    let mut best: HashMap<usize, (f32, usize)> = HashMap::new();
    for &t in traj {
        for (c, w) in seed_links(model, Some(model.id(t))) {
            let e = best.entry(c).or_insert((0.0, 0));
            e.0 = e.0.max(w);
            e.1 += 1;
        }
    }
    best.into_iter()
        .filter(|(c, _)| !traj.contains(c))
        .map(|(c, (w, k))| (c, (w * (1.0 + TRAJECTORY_MEET * (k - 1) as f32)).min(1.0)))
        .collect()
}
