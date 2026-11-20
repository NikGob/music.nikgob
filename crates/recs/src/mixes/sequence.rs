use std::collections::HashMap;

use crate::index::euclid;
use crate::pick::{Diversity, Reason, Seen, Slot, repair, repair_within};

use super::*;

impl<'a> Builder<'a> {
    /// Every song never played of `tracks` stands between two loved
    /// familiar ones ([`Builder::anchors`]), not in the first two places.
    pub(super) fn sandwiched(&self, tracks: &[usize]) -> bool {
        let anchors = self.anchors(tracks);
        (0..tracks.len()).all(|k| !self.unheard(tracks[k]) || (k >= 2 && k + 1 < tracks.len() && anchors[k - 1] && anchors[k + 1]))
    }

    /// Familiar and loved: above the lowest quarter of the mix's familiar
    /// ones.
    pub(super) fn anchors(&self, tracks: &[usize]) -> Vec<bool> {
        let mut a: Vec<f32> = tracks.iter().filter(|&&i| !self.unheard(i)).map(|&i| self.a[i]).collect();
        a.sort_by(f32::total_cmp);
        let low = a.get(a.len() / 4).copied().unwrap_or(0.0);
        tracks.iter().map(|&i| !self.unheard(i) && self.a[i] >= low && self.a[i] > 0.0).collect()
    }

    /// The energy percentile of each of `tracks` among them (ties
    /// averaged, a track without a sound in the middle).
    fn percentiles(&self, tracks: &[usize]) -> HashMap<usize, f32> {
        let raw: Vec<Option<f32>> = tracks.iter().map(|&i| self.model.energy_at(i)).collect();
        let known: Vec<f32> = raw.iter().flatten().copied().collect();
        let ranks = crate::listening::pct_rank(&known);
        let mut k = 0;
        let mut out = HashMap::new();
        for (t, r) in tracks.iter().zip(&raw) {
            let p = match r {
                Some(_) => {
                    k += 1;
                    ranks[k - 1]
                }
                None => 0.5,
            };
            out.insert(*t, p);
        }
        out
    }

    fn slot(&self, i: usize) -> Slot {
        self.recs.slot(i, Reason::Favourite, self.value[i].unwrap_or(0.0))
    }

    fn distance(&self, a: usize, b: usize, m: f32) -> f32 {
        match (self.model.embedded_at(a), self.model.embedded_at(b)) {
            (Some(x), Some(y)) => euclid(x, y) / m,
            _ => {
                let (x, y) = (self.recs.info(a), self.recs.info(b));
                if x.release.is_some() && x.release == y.release {
                    0.5
                } else if x.artists.iter().any(|z| y.artists.contains(z)) {
                    0.6
                } else {
                    1.5
                }
            }
        }
    }

    /// The typical distance between `tracks` (1 without sounds).
    fn typical(&self, tracks: &[usize]) -> f32 {
        let vs: Vec<&crate::analysis::Vector> = tracks.iter().filter_map(|&i| self.model.embedded_at(i)).collect();
        let half = vs.len() / 2;
        let mut d: Vec<f32> = (0..half).map(|k| euclid(vs[k], vs[k + half])).collect();
        d.sort_by(f32::total_cmp);
        d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(1.0)
    }

    /// `tracks` in the order they play (see the module docs): on `arc`,
    /// `steady` steps («Фон»), the language before kept in a mix of many
    /// (`cross`).
    pub fn sequence(&self, tracks: &[usize], arc: Arc, steady: bool, cross: bool) -> Vec<usize> {
        let (order, m) = self.path(tracks, arc, steady, cross, None);
        self.mended(order, m, None, true)
    }

    /// `tracks` of `pool` in the order they play on `arc`: one by one, or
    /// in blocks. With `settle` a song never played that finds no place
    /// between loved ones leaves; without, it stays where it breaks least.
    pub(super) fn order(&self, tracks: &[usize], pool: &Pool, arc: Arc, steady: bool, settle: bool) -> Vec<usize> {
        match pool.blocks {
            Some(size) => {
                let (order, m) = self.path(tracks, arc, false, true, Some(size));
                self.mended(order, m, Some(if size > 0 { size + 1 } else { 4 }), settle)
            }
            None => {
                let (order, m) = self.path(tracks, arc, steady, pool.cross, None);
                self.mended(order, m, None, settle)
            }
        }
    }

    /// [`Builder::sequence`] before it is mended, and the typical distance.
    /// With `blocks` (`Some(size)`, 0: three or four by the profile's
    /// count) the songs of a profile come in blocks of that many.
    fn path(&self, tracks: &[usize], arc: Arc, steady: bool, cross: bool, blocks: Option<usize>) -> (Vec<Slot>, f32) {
        if tracks.len() < 3 {
            return (tracks.iter().map(|&i| self.slot(i)).collect(), 1.0);
        }
        let e = self.percentiles(tracks);
        let anchors: HashMap<usize, bool> = tracks.iter().copied().zip(self.anchors(tracks)).collect();
        let m = self.typical(tracks);
        let long = |i: usize| self.recs.info(i).duration_ms.is_some_and(|d| d > EDGE_MS);
        let mut left: Vec<usize> = tracks.to_vec();
        let total = tracks.len();
        let targets = arc.targets(total);
        let want = |k: usize| targets.as_ref().map_or(0.4, |t| t[k.min(total - 1)]);
        // The last: calmer than the middle, often heard out, familiar.
        let end = want(total - 1).min(0.45);
        let closer = left
            .iter()
            .copied()
            .filter(|&i| !self.unheard(i) && !long(i))
            .min_by(|&a, &b| {
                let c = |i: usize| {
                    let s = &self.model.tracks[i];
                    4.0 * (e[&i] - end).powi(2) - s.completes as f32 / s.starts.max(1) as f32
                };
                c(a).total_cmp(&c(b)).then(a.cmp(&b))
            });
        if let Some(c) = closer {
            left.retain(|&i| i != c);
        }
        // The first: loved, familiar, near the arc's start.
        let first = left
            .iter()
            .copied()
            .filter(|&i| !self.unheard(i) && !long(i))
            .min_by(|&a, &b| {
                let c = |i: usize| (e[&i] - want(0)).abs() - 0.5 * self.a[i];
                c(a).total_cmp(&c(b)).then(a.cmp(&b))
            })
            .unwrap_or(left[0]);
        left.retain(|&i| i != first);
        // The blocks: each profile's length of one and what is left of it.
        let mut count: HashMap<Option<usize>, usize> = HashMap::new();
        for &i in tracks {
            *count.entry(self.model.profile_at(i)).or_default() += 1;
        }
        let size = |p: Option<usize>| match blocks {
            Some(n) if n > 0 => n,
            _ if count.get(&p).copied().unwrap_or(0) >= 8 => 4,
            _ => 3,
        };
        let most = blocks.map(|n| if n > 0 { n + 1 } else { 4 });
        let mut rest = count.clone();
        *rest.entry(self.model.profile_at(first)).or_default() -= 1;
        if let Some(c) = closer {
            *rest.entry(self.model.profile_at(c)).or_default() -= 1;
        }
        let mut run = 1;
        let rules = Diversity { world_share: WORLD_SHARE, ..Diversity::default() };
        let mut out: Vec<Slot> = vec![self.slot(first)];
        let mut order: Vec<usize> = vec![first];
        while !left.is_empty() {
            let k = order.len();
            let seq: Vec<Seen<'_>> = out.iter().map(|s| (s.artists.as_slice(), s.album, s.world)).collect();
            let prev = order[k - 1];
            let prev2 = k.checked_sub(2).map(|p| order[p]);
            let room = left.len() - 1 + usize::from(closer.is_some());
            let crowd = Crowd::of(self, &left, room, &rules);
            let mut best: Option<((usize, f32), usize)> = None;
            for (n, &c) in left.iter().enumerate() {
                let s = self.slot(c);
                let (artist, album, world) = rules.broken(&seq, (&s.artists, s.album, s.world));
                let mut v = 2 * artist + 4 * usize::from(world);
                // The last two places before the closer: the closer too.
                if let Some(cl) = closer
                    && left.len() <= 2
                {
                    let rest: Vec<Slot> = left.iter().filter(|&&o| o != c).map(|&o| self.slot(o)).collect();
                    let mut then = seq.clone();
                    then.push((&s.artists, s.album, s.world));
                    then.extend(rest.iter().map(|r| (r.artists.as_slice(), r.album, r.world)));
                    let last = self.slot(cl);
                    let (artist, _, world) = rules.broken(&then, (&last.artists, last.album, last.world));
                    v += 2 * artist + 4 * usize::from(world);
                }
                if self.unheard(c) && (k < 2 || self.unheard(prev) || !anchors[&prev]) {
                    v += W_UNHEARD;
                }
                if self.unheard(prev) && !anchors[&c] {
                    v += W_UNHEARD;
                }
                let world = self.recs.info(c).world;
                let mut cost = self.distance(prev, c, m) * if steady { C_STEADY } else { 1.0 }
                    + if targets.is_some() { C_ARC * (e[&c] - want(k)).abs() } else { 0.0 }
                    + if album { C_ALBUM } else { 0.0 }
                    + C_CROWD * crowd.over(&s) as f32;
                if world.is_some() && world == self.recs.info(prev).world {
                    cost += C_WORLD;
                }
                if world.is_some() && prev2.is_some_and(|p| self.recs.info(p).world == world) {
                    cost += C_WORLD2;
                }
                if cross && self.model.song_type_at(c).lane() != self.model.song_type_at(prev).lane() {
                    cost += C_LANE;
                }
                if let Some(most) = most {
                    // Blocks are mended inside a profile only: an album thrice
                    // is better not made at all.
                    v += usize::from(album);
                    let (p, was) = (self.model.profile_at(c), self.model.profile_at(prev));
                    if p == was {
                        if run >= most {
                            v += W_BLOCK;
                        } else if run >= size(was) {
                            cost += C_BLOCK;
                        }
                    } else if run < size(was) && rest.get(&was).is_some_and(|n| *n > 0) {
                        cost += C_BLOCK;
                    }
                }
                if best.as_ref().is_none_or(|(b, _)| v < b.0 || (v == b.0 && cost < b.1)) {
                    best = Some(((v, cost), n));
                }
            }
            let (_, n) = best.expect("left is not empty");
            let c = left.swap_remove(n);
            run = if self.model.profile_at(c) == self.model.profile_at(prev) { run + 1 } else { 1 };
            if let Some(r) = rest.get_mut(&self.model.profile_at(c)) {
                *r = r.saturating_sub(1);
            }
            out.push(self.slot(c));
            order.push(c);
        }
        if let Some(c) = closer {
            out.push(self.slot(c));
        }
        (out, m)
    }

    /// The order of `slots` mended by [`repair`], then each song never
    /// played put in its place ([`Builder::sandwich`]), as catalog
    /// positions.
    fn mended(&self, mut out: Vec<Slot>, m: f32, blocks: Option<usize>, settle: bool) -> Vec<usize> {
        let rules = Diversity { world_share: WORLD_SHARE, ..Diversity::default() };
        if blocks.is_some() {
            // Inside one profile's songs only: the blocks stay.
            let profile = |s: &Slot| self.position(s).and_then(|i| self.model.profile_at(i));
            repair_within(self.recs, &mut out, &[], None, &rules, m, &|a, b| profile(a) == profile(b));
            // What the blocks could not mend (a world thrice, an artist
            // twice in three, an album thrice in ten): the rule weighs more
            // than a block, any swap may.
            let seq: Vec<Seen<'_>> = out.iter().map(|s| (s.artists.as_slice(), s.album, s.world)).collect();
            let stuck = (0..seq.len()).any(|k| {
                let (artist, album, world) = rules.broken(&seq[..k], seq[k]);
                world || album || artist > 0
            });
            if let Some(most) = blocks
                && stuck
            {
                // Taken only while no block grows past its length.
                let mut freer = out.clone();
                repair(self.recs, &mut freer, &[], None, &rules, m);
                let profiles: Vec<Option<usize>> = freer.iter().map(profile).collect();
                let longest = profiles.chunk_by(|a, b| a == b).map(<[_]>::len).max().unwrap_or(0);
                if longest <= most {
                    out = freer;
                }
            }
        } else {
            repair(self.recs, &mut out, &[], None, &rules, m);
        }
        let order: Vec<usize> = out.iter().filter_map(|s| self.position(s)).collect();
        self.sandwich(order, &rules, blocks, m, settle)
    }

    fn position(&self, s: &Slot) -> Option<usize> {
        match s.item {
            crate::pick::Item::Track(id) => self.recs.catalog.position(id),
            crate::pick::Item::Stream(_) => None,
        }
    }

    /// Every song never played between two loved familiar ones and not in
    /// the first two places: each in turn is taken out (the others staying)
    /// and put back in the gap between two of them nearest to where it
    /// stood that breaks no rule of diversity more than it did (of
    /// `blocks`, next to its own profile's, no block past its length); with
    /// no such gap it leaves the mix (`settle`; else it stands where it
    /// breaks least), and what its leaving broke is mended by swaps of
    /// familiar songs that keep the gaps.
    fn sandwich(&self, order: Vec<usize>, rules: &Diversity, blocks: Option<usize>, m: f32, settle: bool) -> Vec<usize> {
        let anchors: HashMap<usize, bool> = order.iter().copied().zip(self.anchors(&order)).collect();
        let fits = |o: &[usize], k: usize| k >= 2 && k + 1 < o.len() && anchors[&o[k - 1]] && anchors[&o[k + 1]];
        if (0..order.len()).all(|k| !self.unheard(order[k]) || fits(&order, k)) {
            return order;
        }
        let weight = |o: &[usize]| {
            let slots: Vec<Slot> = o.iter().map(|&i| self.slot(i)).collect();
            let seq: Vec<Seen<'_>> = slots.iter().map(|s| (s.artists.as_slice(), s.album, s.world)).collect();
            rules.weight_of(&seq, 0)
        };
        let todo: Vec<usize> = order.iter().copied().filter(|&i| self.unheard(i)).collect();
        let mut out = order;
        let mut dropped = false;
        for u in todo {
            let Some(was) = out.iter().position(|&i| i == u) else { continue };
            if fits(&out, was) {
                continue;
            }
            let base = weight(&out);
            out.remove(was);
            let mut best: Option<(usize, usize, usize)> = None;
            let own = self.model.profile_at(u);
            for g in 2..out.len() {
                if self.unheard(out[g - 1]) || self.unheard(out[g]) || !(anchors[&out[g - 1]] && anchors[&out[g]]) {
                    continue;
                }
                // In a mix of blocks, inside a block of its own profile.
                if blocks.is_some() && self.model.profile_at(out[g - 1]) != own && self.model.profile_at(out[g]) != own {
                    continue;
                }
                out.insert(g, u);
                // Nor makes the block longer than a block may be.
                let long = blocks.is_some_and(|most| {
                    let same = |k: &usize| self.model.profile_at(out[*k]) == own;
                    1 + (0..g).rev().take_while(same).count() + (g + 1..out.len()).take_while(same).count() > most
                });
                let w = weight(&out);
                out.remove(g);
                if long || w > base {
                    continue;
                }
                let key = (w, g.abs_diff(was));
                if best.is_none_or(|b| key < (b.0, b.1)) {
                    best = Some((w, g.abs_diff(was), g));
                }
            }
            // The day's songs stay: between loved ones past a block's
            // length, else where it breaks least, nearest its place.
            if best.is_none() && !settle {
                for g in 2..out.len() {
                    if self.unheard(out[g - 1]) || self.unheard(out[g]) || !(anchors[&out[g - 1]] && anchors[&out[g]]) {
                        continue;
                    }
                    out.insert(g, u);
                    let w = weight(&out);
                    out.remove(g);
                    if w <= base && best.is_none_or(|b| g.abs_diff(was) < b.1) {
                        best = Some((w, g.abs_diff(was), g));
                    }
                }
            }
            if best.is_none() && !settle {
                for g in 2..=out.len() {
                    out.insert(g, u);
                    let key = (weight(&out), g.abs_diff(was));
                    out.remove(g);
                    if best.is_none_or(|b| key < (b.0, b.1)) {
                        best = Some((key.0, key.1, g));
                    }
                }
            }
            match best {
                Some((_, _, g)) => out.insert(g, u),
                None => dropped = true,
            }
        }
        if dropped && weight(&out) > 0 {
            let mut slots: Vec<Slot> = out.iter().map(|&i| self.slot(i)).collect();
            let of = |s: &Slot| self.position(s);
            let keeps = |a: &Slot, b: &Slot| match (of(a), of(b)) {
                (Some(x), Some(y)) => {
                    !self.unheard(x) && !self.unheard(y) && anchors[&x] == anchors[&y] && (blocks.is_none() || self.model.profile_at(x) == self.model.profile_at(y))
                }
                _ => false,
            };
            repair_within(self.recs, &mut slots, &[], None, rules, m, &keeps);
            out = slots.iter().filter_map(|s| self.position(s)).collect();
        }
        out
    }

    /// `tracks` of many profiles in blocks of one profile (`size` songs; 0:
    /// four of a profile with eight or more, else three), the blocks
    /// joined (see the module docs).
    pub fn sequence_blocks(&self, tracks: &[usize], size: usize, arc: Arc) -> Vec<usize> {
        let (order, m) = self.path(tracks, arc, false, true, Some(size));
        self.mended(order, m, Some(if size > 0 { size + 1 } else { 4 }), true)
    }

}

/// How many of each rule's kind the places left can still hold, and what
/// a candidate would leave over it (an artist with many songs is not
/// cornered at the end).
struct Crowd {
    artist: HashMap<u64, usize>,
    album: HashMap<u64, usize>,
    world: HashMap<u64, usize>,
    caps: (usize, usize, usize),
}

impl Crowd {
    fn of(b: &Builder<'_>, left: &[usize], room: usize, rules: &Diversity) -> Self {
        let mut c = Self { artist: HashMap::new(), album: HashMap::new(), world: HashMap::new(), caps: rules.caps(room) };
        for &i in left {
            let s = b.slot(i);
            for a in s.artists {
                *c.artist.entry(a).or_default() += 1;
            }
            if let Some(x) = s.album {
                *c.album.entry(x).or_default() += 1;
            }
            if let Some(w) = s.world {
                *c.world.entry(w).or_default() += 1;
            }
        }
        c
    }

    fn over(&self, s: &Slot) -> usize {
        let over = |c: usize, mine: bool, cap: usize| (c - usize::from(mine)).saturating_sub(cap);
        self.artist.iter().map(|(a, c)| over(*c, s.artists.contains(a), self.caps.0)).sum::<usize>()
            + self.album.iter().map(|(x, c)| over(*c, s.album == Some(*x), self.caps.1)).sum::<usize>()
            + self.world.iter().map(|(w, c)| over(*c, s.world == Some(*w), self.caps.2)).sum::<usize>()
    }
}

