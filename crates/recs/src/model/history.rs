//! The walk of the history: every event in order of time, the sessions, the pairs, the models learning as they go.

use super::replay::step;
use super::stats::*;
use super::*;

use crate::session::{NOT_NOW_MS, in_stream, skip_ended};
use crate::taste::{REPEAT_WITHIN_MS, SKIP_STREAK_FACTOR, SKIP_STREAK_FROM, W_COMPLETE, W_REPEAT, W_SKIP_EARLY, W_SKIP_LATE_HEARD, W_START_ONLY};

impl UserModel {
    /// One imported play's event (another service's history): its outcome
    /// teaches the long affinity of its track, nothing else.
    fn imported(&mut self, p: &Play) {
        let Some(&i) = self.pos.get(&p.track) else { return };
        if matches!(p.kind, EventType::Start | EventType::Complete) {
            if self.imported_at.len() < self.len() {
                self.imported_at.resize(self.len(), None);
            }
            self.imported_at[i] = self.imported_at[i].max(Some(p.ts));
        }
        match p.kind {
            EventType::Start => {
                if let Some(t0) = self.import_open.insert(i, p.ts) {
                    self.tracks[i].add_as(t0, R_OPEN, W_START_ONLY, false);
                }
            }
            EventType::Complete => {
                if self.import_open.remove(&i).is_some() {
                    self.tracks[i].add_as(p.ts, R_COMPLETE, W_COMPLETE, false);
                }
            }
            EventType::Skip => {
                if self.import_open.remove(&i).is_none() {
                    return;
                }
                let (r, today) = match p.position_ms {
                    Some(pos) if pos < EARLY_SKIP_MS => (R_EARLY, W_SKIP_EARLY),
                    pos => {
                        let heard = match (pos, self.duration[i]) {
                            (Some(pos), Some(d)) if d > 0 => (pos as f32 / d as f32).clamp(0.0, 1.0),
                            _ => 0.0,
                        };
                        (R_LATE * heard, W_SKIP_LATE_HEARD * heard)
                    }
                };
                self.tracks[i].add_as(p.ts, r, today, false);
            }
            _ => {}
        }
    }

    /// Imported starts that never ended count as such.
    pub(super) fn close_imported(&mut self) {
        let mut open: Vec<(usize, UnixMs)> = self.import_open.drain().collect();
        open.sort_unstable();
        for (i, t0) in open {
            self.tracks[i].add_as(t0, R_OPEN, W_START_ONLY, false);
        }
    }

    /// One event of the history, in order of time, `w` the weight of its
    /// context ([`crate::context`]): the outcomes count as much, the
    /// counts and the windows a play of [`FULL_PLAY`] at least, and the
    /// models learn from the owner's and an agent's plays only (the voice
    /// assistant's are background). The sessions of each kind of device
    /// are walked apart. `hook` sees every start that follows another of
    /// its session, before anything learns from it.
    pub(super) fn event(&mut self, p: &Play, w: f32, hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) {
        // A guest's: nothing.
        if w <= 0.0 {
            return;
        }
        if p.origin == Some(EventOrigin::Import) {
            self.imported(p);
            return;
        }
        if p.device != self.walk_device {
            std::mem::swap(&mut self.walk, &mut self.walk_other.0);
            std::mem::swap(&mut self.streak, &mut self.walk_other.1);
            std::mem::swap(&mut self.last_event, &mut self.walk_other.2);
            self.walk_device = p.device;
        }
        // The models learn from the owner's plays (and an agent's, which plays
        // for the owner): from a start of theirs, and from an outcome only
        // when its start and its end are both theirs (the assistant's
        // «дальше» on the owner's song teaches nothing).
        let teach = teaches_by(p.actor);
        let pause = self.last_event.is_none_or(|l| p.ts - l >= SESSION_GAP_MS);
        if pause {
            self.streak = 0;
        }
        self.last_event = Some(p.ts);
        // The likes' freshness as of this day (a day is far below its
        // grace and its half-life), or this session after a pause.
        if pause || self.fresh_at.is_none_or(|f| p.ts - f >= DAY_MS) {
            self.fresh_at = Some(p.ts);
            self.freshness_as_of(p.ts, p.kind == EventType::Start);
        }
        let Some(&i) = self.pos.get(&p.track) else {
            if p.kind == EventType::Start {
                self.walk.start(p.track, p.ts, p.context, p.origin);
            }
            return;
        };
        match p.kind {
            EventType::Like => {
                self.tracks[i].liked = Some(p.ts);
                self.walk.like(p.track);
            }
            EventType::Unlike => self.tracks[i].liked = None,
            EventType::Complete => {
                let part = self.part_of(p.ts);
                self.streak = 0;
                let opened = self.tracks[i].open.take().and_then(|(_, o)| o);
                let manual = opened == Some(EventOrigin::Manual);
                let imported = opened == Some(EventOrigin::Import) || p.origin == Some(EventOrigin::Import);
                let r = R_COMPLETE * if manual { MANUAL_POSITIVE } else { 1.0 };
                self.tracks[i].add_as(p.ts, r * w, W_COMPLETE * w, true);
                let t = &mut self.tracks[i];
                if w >= FULL_PLAY {
                    t.note(false);
                    t.completes += 1;
                    t.done[part] = t.done[part].saturating_add(1);
                }
                let before = if w >= FULL_PLAY { t.last_complete.replace(p.ts) } else { None };
                if let Some(b) = before
                    && !imported
                {
                    let gap = p.ts - b;
                    let bin = REPEAT_BINS.iter().position(|&x| gap < x).unwrap_or(REPEAT_BINS.len());
                    self.repeat_gaps[bin] += 1;
                }
                self.walk.end(p.track, Ended::Complete);
                if let Some(x) = self.pending.remove(&i).filter(|_| self.learn && teach) {
                    self.weights.skip.observe(&x, false);
                }
            }
            EventType::Skip => {
                let opened = self.tracks[i].open.take();
                let origin = opened.and_then(|(_, o)| o);
                // The app closed on it (or on it paused): no outcome — a
                // close says nothing of the song.
                if p.end_reason == Some(EndReason::AppClosed) {
                    self.pending.remove(&i);
                    return;
                }
                match p.position_ms {
                    // A short track's outro is no early skip.
                    Some(pos) if skip_ended(pos, self.duration[i]) == Ended::Early => {
                        self.streak += 1;
                        let leafing = self.streak >= SKIP_STREAK_FROM && matches!(p.context, EventContext::Playlist | EventContext::Library);
                        let lean = if leafing { SKIP_STREAK_FACTOR } else { 1.0 };
                        let who = match origin {
                            Some(EventOrigin::Manual) => MANUAL_EARLY,
                            Some(_) => AUTO_EARLY,
                            None => 1.0,
                        };
                        // A fast skip of a long-loved track in a stream is
                        // «not now»: the session's mood, nothing against the
                        // track in the long run.
                        let not_now = pos < NOT_NOW_MS && in_stream(p.context, origin) && self.tracks[i].loved(p.ts);
                        let full = w >= FULL_PLAY;
                        if full {
                            self.tracks[i].note(true);
                        }
                        if not_now {
                            self.tracks[i].add_hot(p.ts, R_EARLY * who * lean * w);
                        } else {
                            self.tracks[i].add_as(p.ts, R_EARLY * who * lean * w, W_SKIP_EARLY * lean * w, true);
                            if full {
                                let t = &mut self.tracks[i];
                                t.early = t.early.saturating_add(1);
                            }
                        }
                        self.walk.end_at(p.track, Ended::Early, pos, self.duration[i], None);
                        // «Not now» teaches the skip model nothing either.
                        if let Some(x) = self.pending.remove(&i).filter(|_| self.learn && teach && !not_now) {
                            self.weights.skip.observe(&x, true);
                        }
                    }
                    pos => {
                        self.streak = 0;
                        let heard = match (pos, self.duration[i]) {
                            (Some(pos), Some(d)) if d > 0 => (pos as f32 / d as f32).clamp(0.0, 1.0),
                            _ => 0.0,
                        };
                        let manual = origin == Some(EventOrigin::Manual);
                        let r = R_LATE * heard * if manual { MANUAL_POSITIVE } else { 1.0 };
                        self.tracks[i].add_as(p.ts, r * w, W_SKIP_LATE_HEARD * heard * w, true);
                        if w >= FULL_PLAY {
                            let t = &mut self.tracks[i];
                            t.note(false);
                            t.late = t.late.saturating_add(1);
                        }
                        match pos {
                            Some(pos) => self.walk.end_at(p.track, Ended::Late, pos, self.duration[i], None),
                            None => self.walk.end(p.track, Ended::Late),
                        }
                        self.pending.remove(&i);
                    }
                }
            }
            EventType::Start => self.start(i, p, w, teach, hook),
            EventType::Seek => {
                if let Some(to) = p.position_ms {
                    self.walk.seek(p.track, None, to);
                }
            }
            EventType::AddToPlaylist => {}
        }
    }

    /// A start of the track at `i`, `w` the weight of its context; the
    /// models learn from it when it `teach`es (not the assistant's).
    fn start(&mut self, i: usize, p: &Play, w: f32, teach: bool, hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) {
        let ts = p.ts;
        // A start after a pause begins a session of its own.
        if self.walk.last_ts().is_none_or(|l| ts - l >= SESSION_GAP_MS) {
            self.walk = Session::default();
        }
        let learn = self.learn && teach && teaches(p.origin, p.context) && ts >= self.learn_from;
        let m = self.moment_with(&self.walk, ts, hook.is_some() || learn);
        // The skip model learns from its end only when the start teaches.
        if teach {
            let x = self.skip_features(&m, i, p.context);
            self.pending.insert(i, x);
        } else {
            self.pending.remove(&i);
        }
        if m.prev.is_some() {
            match hook {
                Some(h) => {
                    h(self, &m, i, p);
                    if learn {
                        let all = (0..self.len()).filter(|&c| c != i).map(|c| self.features(&m, c)).collect();
                        let target = self.features(&m, i);
                        step(&mut self.weights.next, target, all);
                    }
                }
                None if learn => {
                    let mut next = self.weights.next;
                    self.teach_next(&mut next, &m, i);
                    self.weights.next = next;
                }
                None => {}
            }
        }
        // How often the user's own next start is of the same artist: both
        // the user's own picks, neither of an album's run.
        let own = |origin: Option<EventOrigin>, context: EventContext| origin == Some(EventOrigin::Manual) && context != EventContext::Album;
        if let Some(prev) = m.prev
            && teach
            && own(p.origin, p.context)
            && self.walk.last().is_some_and(|q| own(q.origin, q.context))
        {
            self.again.0 += 1;
            self.again.1 += u32::from(self.artist[prev].is_some() && self.artist[prev] == self.artist[i]);
        }
        // What this start says, as much as it weighs.
        for (k, (s, _, _)) in m.sess.iter().rev().take(3).enumerate() {
            self.co.add(*s, i, 0.5f32.powi(k as i32) * w);
        }
        if let Some(prev) = m.prev
            && let (Some(wp), Some(wc)) = (self.world[prev], self.world[i])
        {
            *self.trans.entry(wp).or_default().entry(wc).or_default() += w;
            *self.trans_total.entry(wp).or_default() += w;
        }
        if let Some(prev) = m.prev
            && teach
            && teaches(p.origin, p.context)
        {
            self.lane_trans[lane_index(self.types[prev].lane())][lane_index(self.types[i].lane())] += 1.0;
        }
        // Listened together within the session, either order.
        for (k, (s, e, _)) in m.sess.iter().rev().take(BAG_WINDOW).enumerate() {
            if *s != i && *e != Ended::Early {
                let near = w / (k + 1) as f32;
                self.bag.add(*s, i, near);
                self.bag.add(i, *s, near);
            }
        }
        let part = self.part_of(ts);
        if let Some((t0, o)) = self.tracks[i].open.take() {
            // The start before never ended.
            let r = R_OPEN * if o == Some(EventOrigin::Manual) { MANUAL_POSITIVE } else { 1.0 };
            let w0 = self.tracks[i].open_w;
            self.tracks[i].add_as(t0, r * w0, W_START_ONLY * w0, true);
        }
        if self.tracks[i].last_complete.is_some_and(|c| ts - c <= REPEAT_WITHIN_MS) {
            self.tracks[i].add_as(ts, R_REPEAT * w, W_REPEAT * w, true);
        }
        // The counts read as «heard» (a song played, a world visited, when
        // last) take a whole play: a speaker's long run heard from afar is
        // weighed in, never counted.
        let full = w >= FULL_PLAY;
        let t = &mut self.tracks[i];
        if full {
            t.starts += 1;
            t.last_start = Some(ts);
            t.part[part] = t.part[part].saturating_add(1);
        }
        t.open = Some((ts, p.origin));
        t.open_w = w;
        if let Some(world) = self.world[i] {
            let ws = self.worlds.entry(world).or_default();
            ws.hot = halve(ws.hot, ws.t_ref, ts, DAY_MS) + w;
            ws.t_ref = ts.max(ws.t_ref);
            if full {
                ws.last = Some(ts);
                ws.part[part] += 1;
                ws.n += 1;
            }
        }
        if full {
            self.part_total[part] += 1;
        }
        self.walk.start(p.track, ts, p.context, p.origin);
    }
}
