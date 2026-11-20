//! The prequential replay of a history: every start predicted before it is learnt, and the metrics of the predictions.

use std::collections::HashSet;

use super::stats::*;
use super::*;

/// The best `n` of `v` by `cmp` (a total order, the best first), in no
/// order: what sorting and taking `n` keeps, in linear time.
pub(super) fn best_n<T>(mut v: Vec<T>, n: usize, cmp: impl Fn(&T, &T) -> std::cmp::Ordering) -> Vec<T> {
    if n == 0 {
        v.clear();
    } else if v.len() > n {
        v.select_nth_unstable_by(n - 1, &cmp);
        v.truncate(n);
    }
    v
}

/// One step of `next` towards `target` over the [`EXPECT_OVER`] best of

/// the other candidates' features.
pub(super) fn step(next: &mut Model, target: [f32; F], cands: Vec<[f32; F]>) {
    let mut scored: Vec<(f32, [f32; F])> = cands.into_iter().map(|f| (next.score(&f), f)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(EXPECT_OVER);
    let cands: Vec<[f32; F]> = scored.into_iter().map(|(_, f)| f).collect();
    next.observe(&target, &cands);
}

/// The area under the ROC curve of `(score, positive)` (ties count half).
pub fn auc(rows: &[(f32, bool)]) -> Option<f32> {
    let pos: Vec<f32> = rows.iter().filter(|r| r.1).map(|r| r.0).collect();
    let neg: Vec<f32> = rows.iter().filter(|r| !r.1).map(|r| r.0).collect();
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut win = 0.0f64;
    for a in &pos {
        for b in &neg {
            win += if a > b { 1.0 } else if a == b { 0.5 } else { 0.0 };
        }
    }
    Some((win / (pos.len() * neg.len()) as f64) as f32)
}

/// Hits of a replay: how many of the real next starts were first, in the
/// first ten, in the first fifty, the mean reciprocal rank and the NDCG of
/// the first ten (one relevant track: `1 / log2(rank + 1)` within ten).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hits {
    pub n: usize,
    pub hit1: f32,
    pub hit10: f32,
    pub hit50: f32,
    pub mrr: f32,
    pub ndcg10: f32,
}

impl Hits {
    fn of(ranks: &[f32]) -> Self {
        let n = ranks.len().max(1) as f32;
        Self {
            n: ranks.len(),
            hit1: ranks.iter().filter(|r| **r <= 1.0).count() as f32 / n,
            hit10: ranks.iter().filter(|r| **r <= 10.0).count() as f32 / n,
            hit50: ranks.iter().filter(|r| **r <= 50.0).count() as f32 / n,
            mrr: ranks.iter().map(|r| 1.0 / r).sum::<f32>() / n,
            ndcg10: ranks.iter().filter(|r| **r <= 10.0).map(|r| 1.0 / (r + 1.0).log2()).sum::<f32>() / n,
        }
    }
}

/// What [`replay`] measured: the next start among the whole
/// catalog by the affinity of phase 1, the hand weights and the learnt
/// ones, on every next start, the free ones (not the next on the album),
/// the next ones on the album and the completed ones; the early skip on
/// the second half of the history.
#[derive(Clone, Debug, Default)]
pub struct ReplayReport {
    pub splits: Vec<(&'static str, &'static str, Hits)>,
    pub skip_auc: Option<f32>,
    pub skip_auc_affinity: Option<f32>,
    pub skip_auc_history: Option<f32>,
    pub skip_test: usize,
    pub weights: Weights,
    /// The share of the catalog the learnt model's first ten ever held.
    pub coverage: f32,
    /// The share of the scored starts whose track was among the candidates
    /// of the moment ([`UserModel::candidates`]: what a radio ranks).
    pub cand_recall: f32,
}

impl ReplayReport {
    pub fn get(&self, split: &str, model: &str) -> Hits {
        self.splits.iter().find(|(s, m, _)| *s == split && *m == model).map_or_else(Hits::default, |x| x.2)
    }
}

/// 1-based rank of `scores[target]`, ties counted half.
fn rank_of(scores: &[f32], target: usize) -> f32 {
    let s = scores[target];
    let above = scores.iter().filter(|v| **v > s).count();
    let ties = scores.iter().filter(|v| **v == s).count() - 1;
    above as f32 + 1.0 + ties as f32 / 2.0
}

/// Replays `input`'s history in order of time, prequentially, from the
/// hand weights (see [`ReplayReport`]).
pub fn replay(catalog: &Catalog, input: &TasteInput, extra: ModelInput<'_>) -> ReplayReport {
    replay_with(catalog, input, extra, &[], &HashSet::new())
}

/// [`replay`] with `dislikes` as the runtime has them (out of every
/// ranking, their sound neighbours a little lower, [`crate::dislike`]),
/// and the starts of `unscored` left out of the hits (to compare runs with
/// and without dislikes on the same starts).
pub fn replay_with(
    catalog: &Catalog,
    input: &TasteInput,
    extra: ModelInput<'_>,
    dislikes: &[TrackId],
    unscored: &HashSet<TrackId>,
) -> ReplayReport {
    let order = ordered(&input.events);
    let weights = event_weights(&input.events, input.target_device);
    let events: Vec<&Play> = order.iter().map(|&k| &input.events[k]).collect();
    // The user's own history: imported plays are walked, never scored.
    let own: Vec<UnixMs> = events.iter().filter(|p| p.origin != Some(EventOrigin::Import)).map(|p| p.ts).collect();
    let (Some(&first), Some(&last)) = (own.first(), own.last()) else {
        return ReplayReport::default();
    };
    let mid = first + (last - first) / 2;
    // The outcome of every start (for the splits), by its place in `events`.
    let mut outcome: HashMap<usize, Ended> = HashMap::new();
    let mut open: HashMap<(TrackId, crate::context::PlayDevice), usize> = HashMap::new();
    for (k, p) in events.iter().enumerate() {
        match p.kind {
            EventType::Start => {
                open.insert((p.track, p.device), k);
                outcome.insert(k, Ended::Open);
            }
            EventType::Complete | EventType::Skip => {
                if let Some(j) = open.remove(&(p.track, p.device)) {
                    outcome.insert(j, match p.kind {
                        EventType::Complete => Ended::Complete,
                        _ if p.end_reason == Some(EndReason::AppClosed) => Ended::Open,
                        _ if p.position_ms.is_some_and(|x| x < EARLY_SKIP_MS) => Ended::Early,
                        _ => Ended::Late,
                    });
                }
            }
            _ => {}
        }
    }
    let taste = Taste::build(catalog, &TasteInput::default(), first);
    let mut m = UserModel::empty(catalog, taste, extra, input.utc_offset_min, first);
    if input.personal_graph {
        m.personal = crate::graph::personal(catalog, &input.events, &input.playlists);
    }
    m.learn = true;
    m.device = input.target_device;
    m.offsets = input.utc_offsets.clone();
    m.listen(input, &order, &weights);
    let disliked: Vec<usize> = dislikes.iter().filter_map(|t| m.position(*t)).collect();
    let near = crate::dislike::near_dislikes(&m.emb, &disliked, m.scale);
    let mut shown: HashSet<usize> = HashSet::new();
    let mut ranks: HashMap<(&'static str, &'static str), Vec<f32>> = HashMap::new();
    let mut skips: Vec<(f32, f32, f32, bool)> = Vec::new();
    let mut held: Vec<bool> = Vec::new();
    let mut k_of: HashMap<*const Play, usize> = HashMap::new();
    for (k, p) in events.iter().enumerate() {
        k_of.insert(*p as *const Play, k);
    }
    for p in &events {
        let k = k_of[&(*p as *const Play)];
        let ended = outcome.get(&k).copied().unwrap_or(Ended::Open);
        // The skip model's prediction before it learns from this start.
        if p.kind == EventType::Start
            && p.origin != Some(EventOrigin::Import)
            && p.ts >= mid
            && matches!(ended, Ended::Complete | Ended::Early)
            && let Some(i) = m.position(p.track)
        {
            let walk = if m.walk.last_ts().is_none_or(|l| p.ts - l >= SESSION_GAP_MS) { Session::default() } else { m.walk.clone() };
            let mo = m.moment(&walk, p.ts);
            let x = m.skip_features(&mo, i, p.context);
            skips.push((m.weights.skip.p(&x), -x[1], -x[2] + x[4], ended == Ended::Early));
        }
        let mut hook = |model: &UserModel, mo: &Moment, target: usize, _: &Play| {
            if unscored.contains(&model.id(target)) || disliked.contains(&target) {
                return;
            }
            let free = mo.next_album != Some(target);
            let mut splits: Vec<&'static str> = vec!["all", if free { "free" } else { "album_next" }];
            if ended == Ended::Complete {
                splits.push("completed");
                if free {
                    splits.push("completed_free");
                }
            }
            if p.ts >= mid {
                splits.push(if free { "free:second_half" } else { "album_next:second_half" });
            }
            // Listened: not skipped early (what the user went on with).
            if ended != Ended::Early {
                splits.push("listened");
                if free {
                    splits.push("listened_free");
                }
                if p.ts >= mid {
                    splits.push("listened:second_half");
                }
            }
            held.push(model.candidates(mo, &|_| true).binary_search(&target).is_ok());
            let feats: Vec<[f32; F]> = (0..model.len()).map(|c| model.features(mo, c)).collect();
            let hand = Model::default();
            // A disliked track is out, its neighbours a little lower.
            let calibrated = |mut s: Vec<f32>| -> Vec<f32> {
                for (c, x) in s.iter_mut().enumerate() {
                    *x -= near[c];
                }
                for &d in &disliked {
                    s[d] = f32::NEG_INFINITY;
                }
                s
            };
            let scores = |w: &Model| -> Vec<f32> { calibrated(feats.iter().map(|f| w.score(f)).collect()) };
            let today: Vec<f32> =
                calibrated((0..model.len()).map(|c| model.tracks[c].aff_today(mo.now) + 1e-6 * (c as f32 / model.len() as f32)).collect());
            let learned = scores(&model.weights.next);
            let mut top: Vec<usize> = (0..learned.len()).collect();
            let k = 10.min(top.len());
            if k > 0 {
                top.select_nth_unstable_by(k - 1, |a, b| learned[*b].total_cmp(&learned[*a]));
                shown.extend(top[..k].iter().copied());
            }
            for (name, s) in [("phase1_affinity", today), ("hand", scores(&hand)), ("learned", learned)] {
                let r = rank_of(&s, target);
                for split in &splits {
                    ranks.entry((split, name)).or_default().push(r);
                }
            }
        };
        m.event(p, weights[order[k]], Some(&mut hook));
    }
    let mut splits: Vec<(&'static str, &'static str, Hits)> = ranks.into_iter().map(|((s, n), r)| (s, n, Hits::of(&r))).collect();
    splits.sort_by_key(|(s, n, _)| (*s, *n));
    let rows = |pick: fn(&(f32, f32, f32, bool)) -> f32| -> Vec<(f32, bool)> { skips.iter().map(|r| (pick(r), r.3)).collect() };
    ReplayReport {
        splits,
        skip_auc: auc(&rows(|r| r.0)),
        skip_auc_affinity: auc(&rows(|r| r.1)),
        skip_auc_history: auc(&rows(|r| r.2)),
        skip_test: skips.len(),
        weights: m.weights,
        coverage: shown.len() as f32 / m.len().max(1) as f32,
        cand_recall: held.iter().filter(|h| **h).count() as f32 / held.len().max(1) as f32,
    }
}
