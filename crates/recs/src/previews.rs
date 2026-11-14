//! Which new songs get one of the day's few previews (YouTube Music's
//! player answers about sixty a day before it asks to wait): those whose
//! verdict a preview can change, not the first found.
//!
//! * **Exposure.** A song a running wave or radio is about to play is
//!   heard for sure; one of the discoveries less the lower it ranks.
//! * **Doubt.** A preview matters where the blind ranking is unsure: near
//!   the cut-off of what is shown and played ([`PreviewCandidate::cut`]),
//!   when the library's songs that found it disagree (of different taste
//!   profiles), when its artist is new to the library.
//! * **Cost of a mistake.** A wrong song breaks a strict filter (calm, the
//!   background, a niche) far more than an ordinary mix.
//!
//! The value is their product ([`preview_value`]). Songs Deezer has a free
//! 30-second preview of are heard from there first and cost nothing of the
//! day (the runtime's analysis).

/// What [`preview_value`] reads of a candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewCandidate {
    /// Its rank among the finds (0 the best).
    pub rank: usize,
    /// The rank where what is shown and played ends.
    pub cut: usize,
    /// About to be played by a running wave or radio.
    pub soon: bool,
    /// The share of the library's songs that found it of their most common
    /// taste profile (1: all agree); `None` with fewer than two of them.
    pub agree: Option<f32>,
    /// The library has songs of its artist.
    pub known_artist: bool,
    /// It is a candidate of a strict filter (calm, the background, a
    /// niche): one wrong song there costs more.
    pub strict: bool,
}

/// The cost of a wrong song in a strict filter, against an ordinary one.
pub const STRICT_COST: f32 = 2.5;
/// Exposure halves this many ranks down.
const EXPOSURE_RANKS: f32 = 25.0;
/// Doubt by the cut-off spreads over this share of the cut's rank.
const CUT_SPREAD: f32 = 0.35;
/// The doubt of a song of an artist new to the library.
const NEW_ARTIST_DOUBT: f32 = 0.6;
/// The least doubt: a preview still tells a little.
const DOUBT_FLOOR: f32 = 0.2;

/// A song about to be played stands this much above every other.
const SOON: f32 = 3.0;

/// How much a preview of the candidate is worth (see the module docs):
/// exposure × doubt × the cost of a mistake; a song about to be played
/// above all others.
pub fn preview_value(c: &PreviewCandidate) -> f32 {
    let exposure = if c.soon { 1.0 } else { 1.0 / (1.0 + c.rank as f32 / EXPOSURE_RANKS) };
    let cut = c.cut.max(1) as f32;
    let z = (c.rank as f32 - cut) / (CUT_SPREAD * cut);
    let boundary = (-z * z).exp();
    let seeds = c.agree.map_or(0.5, |a| (1.0 - a).clamp(0.0, 1.0) * 2.0).min(1.0);
    let artist = if c.known_artist { 0.0 } else { NEW_ARTIST_DOUBT };
    let doubt = DOUBT_FLOOR + (1.0 - DOUBT_FLOOR) * boundary.max(seeds).max(artist);
    let cost = if c.strict { STRICT_COST } else { 1.0 };
    if c.soon { SOON + doubt * cost } else { exposure * doubt * cost }
}

/// The order of `cands` to preview, the most worth first (ties by rank).
pub fn preview_order(cands: &[PreviewCandidate]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| preview_value(&cands[b]).total_cmp(&preview_value(&cands[a])).then(cands[a].rank.cmp(&cands[b].rank)));
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    fn cand(rank: usize) -> PreviewCandidate {
        PreviewCandidate { rank, cut: 30, soon: false, agree: Some(1.0), known_artist: true, strict: false }
    }

    /// A song about to play goes first; near the cut-off before the top
    /// that is shown whatever; doubtful seeds, a new artist and a strict
    /// filter each raise a song.
    #[test]
    fn previews_go_where_they_can_change_the_verdict() {
        assert!(preview_value(&PreviewCandidate { soon: true, ..cand(200) }) > preview_value(&cand(0)));
        assert!(preview_value(&cand(28)) > preview_value(&cand(2)), "near the cut-off");
        assert!(preview_value(&cand(28)) > preview_value(&cand(150)));
        let base = preview_value(&cand(5));
        assert!(preview_value(&PreviewCandidate { agree: Some(0.5), ..cand(5) }) > base);
        assert!(preview_value(&PreviewCandidate { known_artist: false, ..cand(5) }) > base);
        assert!(preview_value(&PreviewCandidate { strict: true, ..cand(5) }) > base);
        let order = preview_order(&[cand(0), cand(29), PreviewCandidate { soon: true, ..cand(90) }]);
        assert_eq!(order, vec![2, 1, 0]);
    }

    /// The budgeted reveal (see the status of this round): `K` previews of
    /// a pool by their value against the first `K` by rank. Each song has a
    /// hidden fit; its blind score is the fit plus noise that is larger
    /// where the seeds disagree or the artist is new; strict songs that
    /// miss cost [`STRICT_COST`]. The top `cut` by score (a heard song by
    /// its fit) are played. A miss played is a song played whose fit is
    /// below zero; fewer misses with the value's order.
    #[test]
    #[ignore = "a report"]
    fn budgeted_reveal_report() {
        let (pool, cut, k) = (300usize, 30usize, 60usize);
        let mut sums = [0.0f32; 2];
        let runs = 200;
        for run in 0..runs {
            let mut rng = Rng::new(1000 + run);
            let mut fit = Vec::new();
            let mut cands = Vec::new();
            for r in 0..pool {
                let agree = 0.4 + 0.6 * rng.f32();
                let known = rng.f32() < 0.5;
                let strict = rng.f32() < 0.2;
                let noise = 0.3 + 1.2 * (1.0 - agree) + if known { 0.0 } else { 0.5 };
                let f = rng.f32() * 2.0 - 1.0 - r as f32 / pool as f32;
                fit.push((f, noise, strict));
                cands.push(PreviewCandidate { rank: r, cut, soon: false, agree: Some(agree), known_artist: known, strict });
            }
            let blind: Vec<f32> = fit.iter().map(|(f, n, _)| f + n * (rng.f32() * 2.0 - 1.0)).collect();
            for (policy, order) in [(0, (0..pool).collect::<Vec<_>>()), (1, preview_order(&cands))] {
                let heard: std::collections::HashSet<usize> = order.into_iter().take(k).collect();
                let mut score: Vec<(f32, usize)> = (0..pool).map(|i| (if heard.contains(&i) { fit[i].0 } else { blind[i] }, i)).collect();
                score.sort_by(|a, b| b.0.total_cmp(&a.0));
                let misses: f32 = score.iter().take(cut).filter(|(_, i)| fit[*i].0 < 0.0).map(|(_, i)| if fit[*i].2 { STRICT_COST } else { 1.0 }).sum();
                sums[policy] += misses;
            }
        }
        eprintln!("budgeted reveal, K = {k} of {pool}, {cut} played: misses (strict weighed) top-K {:.2}, by value {:.2}", sums[0] / runs as f32, sums[1] / runs as f32);
        assert!(sums[1] < sums[0]);
    }
}
