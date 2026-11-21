//! One moment of a session as the models see it, and the features of a candidate at that moment for the next start and for the early skip.

use std::collections::HashSet;

use super::replay::{best_n, step};
use super::stats::*;
use super::*;

use crate::taste::HOUR_MS;

impl UserModel {
    /// The moment of `session` at `now` (at least the session's last start).
    pub fn moment(&self, session: &Session, now: UnixMs) -> Moment {
        self.moment_with(session, now, true)
    }

    /// [`Self::moment`] without graph walks (for skip scoring).
    pub(crate) fn moment_light(&self, session: &Session, now: UnixMs) -> Moment {
        self.moment_with(session, now, false)
    }

    /// [`Self::moment`]; the neighbours of the session in the graphs and in
    /// the sessions before only when it will `rank` candidates (the walk of
    /// the history needs them for a step alone).
    pub(super) fn moment_with(&self, session: &Session, now: UnixMs, rank: bool) -> Moment {
        let now = now.max(session.last_ts().unwrap_or(now));
        let sess: Vec<(usize, Ended, EventContext)> =
            session.tracks().filter_map(|p| Some((self.position(p.track)?, p.ended, p.context))).collect();
        let prev = session.last().and_then(|p| self.position(p.track));
        // The graphs around the last three approved.
        let mut graph_near: HashMap<usize, f32> = HashMap::new();
        let mut graph_artists: HashMap<ArtistId, f32> = HashMap::new();
        let mut pgraph_near: HashMap<usize, f32> = HashMap::new();
        // What the session approved ([`Session::seeds`]: played through or
        // liked) and the song playing now. A song skipped, early or past its
        // half, seeds nothing.
        let through: Vec<usize> = if rank {
            session.seeds(self).filter_map(|p| self.position(p.track)).collect()
        } else {
            Vec::new()
        };
        let recent: Vec<usize> = through.iter().take(3).copied().collect();
        let mut bag_near: HashMap<usize, f32> = HashMap::new();
        for (k, s) in through.iter().take(BAG_WINDOW).enumerate() {
            let f = 0.8f32.powi(k as i32);
            for &(c, w) in self.bag.row(*s) {
                *bag_near.entry(c as usize).or_default() += f * w;
            }
        }
        let own: HashSet<ArtistId> = recent.iter().filter_map(|&s| self.artist[s]).collect();
        // The walk of the collective graph from them (its first step the
        // neighbours of each, as one hop always counted).
        let seeds: Vec<(usize, f32)> = recent.iter().enumerate().map(|(k, &s)| (s, 0.6f32.powi(k as i32))).collect();
        if !seeds.is_empty() {
            graph_near = self.graph.walk(&seeds);
        }
        for (k, &s) in recent.iter().enumerate() {
            let f = 0.6f32.powi(k as i32);
            if let Some(a) = self.artist[s] {
                for &(b, w) in self.graph.similar_artists(a) {
                    if !own.contains(&b) {
                        *graph_artists.entry(b).or_default() += f * w;
                    }
                }
            }
            for &(c, w) in self.personal.get(s).map_or(&[][..], Vec::as_slice) {
                *pgraph_near.entry(c as usize).or_default() += f * w;
            }
        }
        Moment {
            bag_near,
            graph_near,
            graph_artists,
            pgraph_near,
            now,
            next_album: prev.and_then(|i| self.next_in_album(i)),
            prev,
            sess,
            anchor: session.anchor(self),
            pushes: session.pushes(self),
            lane: session.lane(self),
            part: self.part_of(now),
            skip_streak: session.skip_streak(self),
            album_session: session.album_session(self),
            world_session: session.world_session(self),
            repeat_one: session.repeat_one(),
            p_bg: self.listening.p_now(&SessionSignals::of_session(session, |t| self.position(t).and_then(|i| self.duration[i]))),
        }
    }

    /// The features of candidate `c` at moment `m` ([`FEATURES`]).
    pub fn features(&self, m: &Moment, c: usize) -> [f32; F] {
        let t = &self.tracks[c];
        let mut f = [0.0f32; F];
        f[0] = t.aff_long(m.now);
        f[1] = t.aff_hot(m.now);
        f[2] = if t.liked(m.now) { t.love_fresh() } else { 0.0 };
        let e = self.emb[c].as_ref();
        f[F_SOUND] = match (e, &m.anchor) {
            (Some(e), Some(a)) => -euclid(e, a) / self.scale,
            _ => -1.0,
        };
        if e.is_some() {
            f[F_SKIP_PUSH] = self.skip_push_of(&m.pushes, c);
        }
        f[F_NEXT_ALBUM] = f32::from(u8::from(m.next_album == Some(c)));
        if let Some(p) = m.prev {
            f[F_SAME_ALBUM] = f32::from(u8::from(self.release[c].is_some() && self.release[c] == self.release[p]));
            let (wc, wp) = (self.world[c], self.world[p]);
            f[F_SAME_WORLD] = f32::from(u8::from(wp.is_some() && wc == wp));
            if let (Some(wp), Some(wc)) = (wp, wc)
                && let Some(out) = self.trans.get(&wp)
            {
                let total = self.trans_total.get(&wp).copied().unwrap_or(0.0);
                f[8] = out.get(&wc).copied().unwrap_or(0.0) / (total + 2.0);
            }
        }
        let mut co = 0.0;
        for (k, (s, _, _)) in m.sess.iter().rev().take(3).enumerate() {
            co += self.co.get(*s, c) * 0.6f32.powi(k as i32);
        }
        f[F_COOC] = co.tanh();
        let in_s = m.sess.iter().filter(|(s, _, _)| *s == c).count();
        f[10] = in_s.min(2) as f32;
        f[11] = f32::from(u8::from(in_s == 0 && t.last_start.is_some_and(|l| m.now - l < 3 * HOUR_MS)));
        if let Some(w) = self.world[c].and_then(|w| self.worlds.get(&w)) {
            let n: u32 = w.part.iter().sum();
            let all: u32 = self.part_total.iter().sum();
            if n >= 3 && all > 0 {
                f[12] = w.part[m.part] as f32 / n as f32 - self.part_total[m.part] as f32 / all as f32;
            }
        }
        f[F_SAME_LANE] = f32::from(u8::from(m.lane.is_some_and(|l| l == self.types[c].lane())));
        f[14] = f32::from(u8::from(t.starts == 0));
        f[15] = f32::from(u8::from(self.duration[c].is_some_and(|d| d < INTERLUDE_MS)));
        let like_artist = self.artist[c].and_then(|a| m.graph_artists.get(&a)).copied().unwrap_or(0.0);
        f[F_GRAPH] = (m.graph_near.get(&c).copied().unwrap_or(0.0) + GRAPH_ARTIST * like_artist).tanh();
        f[F_PGRAPH] = m.pgraph_near.get(&c).copied().unwrap_or(0.0).tanh();
        f[F_PRIOR] = self.prior_at(c);
        f[F_SESS_CO] = (m.bag_near.get(&c).copied().unwrap_or(0.0) / 2.0).tanh();
        f[F_IS_PREV] = f32::from(u8::from(m.prev == Some(c)));
        f[F_ALBUM_FIRST] = f32::from(u8::from(self.album_first[c] && m.prev.is_none_or(|p| self.release[p] != self.release[c])));
        f
    }

    /// [`F_SKIP_PUSH`] of `c` by the sounds `pushes` (embedded, each with
    /// how hard it pushes): 0 for a track without a sound.
    pub fn skip_push_of(&self, pushes: &[(Vector, f32)], c: usize) -> f32 {
        let Some(e) = self.emb[c].as_ref() else { return 0.0 };
        -pushes.iter().map(|(p, w)| w * (1.0 - euclid(e, p) / self.scale).max(0.0)).sum::<f32>()
    }

    /// The tracks `session` rejected, to measure candidates against by
    /// both sounds where the neural model heard them ([`Self::skip_push_by`]);
    /// none without rejections or before the index blends.
    pub fn push_probe<'a>(&self, session: &Session, index: Option<&'a VectorIndex<TrackId>>) -> Option<PushProbe<'a>> {
        let index = index?;
        let pushed = session.pushed(self);
        if pushed.is_empty() {
            return None;
        }
        let ids: Vec<TrackId> = pushed.iter().map(|p| p.0).collect();
        let probe = index.sound_probe(&ids)?;
        Some(PushProbe { index, probe, pushed })
    }

    /// [`F_SKIP_PUSH`] of `c` by the session's rejections as `probe` holds
    /// them: by the blend of bliss and the neural model where both tracks
    /// have the model's vector (an instrumental beat and a sung track of one
    /// tempo stand apart), by bliss else. 0 for a track without a sound.
    pub fn skip_push_by(&self, probe: &PushProbe<'_>, c: usize) -> f32 {
        let Some(e) = self.emb[c].as_ref() else { return 0.0 };
        let mut sum = 0.0f32;
        for (k, (t, w)) in probe.pushed.iter().enumerate() {
            let d = probe.index.probe_distance(&probe.probe, k, self.ids[c]).or_else(|| self.pos.get(t).and_then(|&j| self.emb[j].as_ref()).map(|p| euclid(e, p)));
            if let Some(d) = d {
                sum += w * (1.0 - d / self.scale).max(0.0);
            }
        }
        -sum
    }

    /// The relevance of `c` by the model of the next start.
    pub fn score(&self, m: &Moment, c: usize) -> f32 {
        self.weights.next.score(&self.features(m, c))
    }

    /// The features of starting `c` now for its early skip
    /// ([`SKIP_FEATURES`]), started from `ctx`.
    pub fn skip_features(&self, m: &Moment, c: usize, ctx: EventContext) -> [f32; G] {
        let t = &self.tracks[c];
        let ty = &self.types[c];
        let mut x = [0.0f32; G];
        let b = |v: bool| f32::from(u8::from(v));
        x[0] = 1.0;
        x[1] = t.aff_today(m.now);
        x[2] = t.aff_long(m.now);
        x[3] = t.aff_hot(m.now);
        x[4] = (f32::from(t.early) + 0.2) / (f32::from(t.early) + t.completes as f32 + 1.0);
        x[5] = b(t.starts == 0);
        x[6] = b(t.liked(m.now));
        x[7] = b(m.sess.is_empty());
        x[8] = (m.sess.len() as f32).ln_1p();
        x[9] = b(ctx == EventContext::Album);
        x[10] = b(ctx == EventContext::Playlist);
        x[11] = b(ctx == EventContext::Artist);
        x[12] = b(matches!(ctx, EventContext::HomeSection | EventContext::Radio));
        if let Some(p) = m.prev {
            x[13] = b(self.release[c].is_some() && self.release[c] == self.release[p]);
            x[14] = b(self.world[p].is_some() && self.world[c] == self.world[p]);
        }
        x[15] = match (m.prev.and_then(|p| self.emb[p].as_ref()), self.emb[c].as_ref()) {
            (Some(a), Some(v)) => euclid(a, v) / self.scale,
            _ => 1.0,
        };
        let local = m.now + i64::from(self.utc_offset_min) * 60_000;
        x[16] = b(local.rem_euclid(DAY_MS) / HOUR_MS < 6);
        x[17] = b(ty.ost);
        x[18] = b(ty.vocal());
        x[19] = b(self.duration[c].is_some_and(|d| d > LONG_MS));
        x[20] = b(self.duration[c].is_some_and(|d| d < INTERLUDE_MS));
        let mut ends = m.sess.iter().rev().map(|(_, e, _)| *e);
        let (e1, e2) = (ends.next(), ends.next());
        x[21] = b(e1 == Some(Ended::Early));
        x[22] = b(e1 == Some(Ended::Early) && e2 == Some(Ended::Early));
        x[G_BACKGROUND] = m.p_bg;
        x
    }

    /// The chance `c` started now from `ctx` is skipped in its first 30 s.
    pub fn p_skip(&self, m: &Moment, c: usize, ctx: EventContext) -> f32 {
        self.skip_risk(c, self.weights.skip.p(&self.skip_features(m, c, ctx)))
    }

    /// The features of starting a new track of a service now from `ctx`
    /// ([`SKIP_FEATURES`]): never played, no affinity, its sound `vector`
    /// (embedded) and length when known.
    pub fn skip_features_new(&self, m: &Moment, vector: Option<&Vector>, duration_ms: Option<u32>, ctx: EventContext) -> [f32; G] {
        let mut x = [0.0f32; G];
        let b = |v: bool| f32::from(u8::from(v));
        x[0] = 1.0;
        x[4] = 0.2;
        x[5] = 1.0;
        x[7] = b(m.sess.is_empty());
        x[8] = (m.sess.len() as f32).ln_1p();
        x[9] = b(ctx == EventContext::Album);
        x[10] = b(ctx == EventContext::Playlist);
        x[11] = b(ctx == EventContext::Artist);
        x[12] = b(matches!(ctx, EventContext::HomeSection | EventContext::Radio));
        x[15] = match (m.prev.and_then(|p| self.emb[p].as_ref()), vector) {
            (Some(a), Some(v)) => euclid(a, v) / self.scale,
            _ => 1.0,
        };
        let local = m.now + i64::from(self.utc_offset_min) * 60_000;
        x[16] = b(local.rem_euclid(DAY_MS) / HOUR_MS < 6);
        x[19] = b(duration_ms.is_some_and(|d| d > LONG_MS));
        x[20] = b(duration_ms.is_some_and(|d| d < INTERLUDE_MS));
        let mut ends = m.sess.iter().rev().map(|(_, e, _)| *e);
        let (e1, e2) = (ends.next(), ends.next());
        x[21] = b(e1 == Some(Ended::Early));
        x[22] = b(e1 == Some(Ended::Early) && e2 == Some(Ended::Early));
        x[G_NEW_SERVICE] = 1.0;
        x[G_BACKGROUND] = m.p_bg;
        x
    }

    /// The chance a new track of a service started now is skipped in its
    /// first 30 s.
    pub fn p_skip_new(&self, m: &Moment, vector: Option<&Vector>, duration_ms: Option<u32>, ctx: EventContext) -> f32 {
        self.weights.skip.p(&self.skip_features_new(m, vector, duration_ms, ctx))
    }


    /// The candidates of the next start at `m`: the nearest to the
    /// anchor, what followed the last three, the next on the album, the
    /// previous track's world, the hot and the long favourites; `ok`
    /// filters them.
    pub fn candidates(&self, m: &Moment, ok: &dyn Fn(usize) -> bool) -> Vec<usize> {
        let mut out: HashSet<usize> = HashSet::new();
        if let Some(a) = &m.anchor {
            let near: Vec<(f32, usize)> = self.emb.iter().enumerate().filter_map(|(i, e)| Some((euclid(e.as_ref()?, a), i))).filter(|(_, i)| ok(*i)).collect();
            out.extend(best_n(near, 200, |x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1))).into_iter().map(|(_, i)| i));
        }
        let mut co: HashMap<usize, f32> = HashMap::new();
        for (k, (s, _, _)) in m.sess.iter().rev().take(3).enumerate() {
            for (c, w) in self.co.after(*s) {
                *co.entry(c).or_default() += w * 0.6f32.powi(k as i32);
            }
        }
        let mut co: Vec<(usize, f32)> = co.into_iter().filter(|(c, _)| ok(*c)).collect();
        co.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        out.extend(co.into_iter().take(50).map(|(c, _)| c));
        // The graphs: the neighbours of the session, the best known tracks
        // of the artists like its.
        let strongest = |map: &HashMap<usize, f32>| {
            let mut v: Vec<(usize, f32)> = map.iter().map(|(c, w)| (*c, *w)).filter(|(c, _)| ok(*c)).collect();
            v.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            v.into_iter().take(GRAPH_CANDIDATES).map(|(c, _)| c).collect::<Vec<usize>>()
        };
        out.extend(strongest(&m.graph_near));
        out.extend(strongest(&m.bag_near));
        out.extend(strongest(&m.pgraph_near));
        let mut artists: Vec<(ArtistId, f32)> = m.graph_artists.iter().map(|(a, w)| (*a, *w)).collect();
        artists.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        for (a, _) in artists.into_iter().take(GRAPH_ARTISTS) {
            let mut theirs: Vec<usize> = self.by_artist.get(&a).map_or(&[][..], Vec::as_slice).iter().map(|&i| i as usize).filter(|&i| ok(i)).collect();
            let key = |i: usize| self.tracks[i].aff_long(m.now) + self.prior_at(i);
            theirs.sort_by(|x, y| key(*y).total_cmp(&key(*x)).then(x.cmp(y)));
            out.extend(theirs.into_iter().take(GRAPH_ARTIST_TRACKS));
        }
        out.extend(m.next_album.filter(|&c| ok(c)));
        if let Some(w) = m.prev.and_then(|p| self.world[p]) {
            out.extend(self.by_world.get(&w).map_or(&[][..], Vec::as_slice).iter().map(|&i| i as usize).filter(|&i| ok(i)));
        }
        let mut top = |key: &dyn Fn(&TrackStats) -> f32, n: usize| {
            let v: Vec<(f32, usize)> = (0..self.len()).filter(|&i| ok(i)).map(|i| (key(&self.tracks[i]), i)).filter(|(k, _)| *k > 0.0).collect();
            out.extend(best_n(v, n, |x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1))).into_iter().map(|(_, i)| i));
        };
        top(&|t| t.aff_hot(m.now), 100);
        top(&|t| t.aff_long(m.now), 200);
        let mut out: Vec<usize> = out.into_iter().collect();
        out.sort_unstable();
        out
    }

    /// Teaches `next` the start of `target` at `m`, the user's choice,
    /// among the candidates of the moment: the runtime's step between
    /// builds (its weights, this model's stats).
    pub fn teach_next(&self, next: &mut Model, m: &Moment, target: usize) {
        let cands = self.candidates(m, &|c| c != target);
        let scored = cands.iter().map(|&c| self.features(m, c)).collect();
        step(next, self.features(m, target), scored);
    }
}
