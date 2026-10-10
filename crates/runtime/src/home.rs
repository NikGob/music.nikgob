//! The home feed (simple stage-1 version, PLAN §9): built from the event log
//! and the catalog. Recommendations proper arrive in stage 4.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, ReleaseId, TrackId};
use nikgob_library::{
    ArtistRow, ArtistSortKey, Library, ReleaseRow, ReleaseSortKey, Sort, TrackRow, now_ms,
};

pub struct Home {
    pub hour: u8,
    pub mix: Vec<TrackRow>,
    pub continue_listening: Vec<ReleaseRow>,
    pub artists: Vec<ArtistRow>,
    pub long_unplayed: Vec<ReleaseRow>,
    pub recently_added: Vec<ReleaseRow>,
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub fn build(lib: &Library) -> Home {
    let hour = local_hour();
    let releases = lib
        .releases(0, 20_000, Sort::desc(ReleaseSortKey::Added))
        .unwrap_or_default();
    let by_id: HashMap<ReleaseId, &ReleaseRow> =
        releases.iter().map(|r| (r.release.id, r)).collect();
    let stats = lib.all_track_stats().unwrap_or_default();
    let stat: HashMap<TrackId, _> = stats.iter().copied().collect();

    // Continue: releases of the most recently started tracks.
    let mut continue_listening = Vec::new();
    let mut seen = HashSet::new();
    for e in lib.recent_events(400).unwrap_or_default() {
        if continue_listening.len() >= 12 {
            break;
        }
        if let Ok(Some(t)) = lib.get_track(e.track_id)
            && let Some(rid) = t.release_id
            && seen.insert(rid)
            && let Some(r) = by_id.get(&rid)
        {
            continue_listening.push((*r).clone());
        }
    }

    // Mix: liked and often-completed tracks, skip-heavy ones out, shuffled.
    let mut pool: Vec<(f32, TrackId)> = stats
        .iter()
        .map(|(id, s)| {
            let score = s.completes as f32 * 1.0 + s.plays as f32 * 0.3 - s.skips as f32 * 0.8;
            (score, *id)
        })
        .filter(|(s, _)| *s > 0.0)
        .collect();
    for id in lib.liked_track_ids().unwrap_or_default() {
        pool.push((2.0 + stat.get(&id).map_or(0.0, |s| s.completes as f32 * 0.5), id));
    }
    pool.sort_by(|a, b| b.0.total_cmp(&a.0));
    // A liked track that also has play stats is in the pool twice, with
    // different scores, so the copies are not adjacent: keep the best-scored one.
    let mut in_pool = HashSet::new();
    pool.retain(|p| in_pool.insert(p.1));
    let mut picked: Vec<TrackId> = pool.iter().take(40).map(|p| p.1).collect();
    fastrand::shuffle(&mut picked);
    let mut mix: Vec<TrackRow> = picked
        .into_iter()
        .filter_map(|id| lib.get_track_row(id).ok().flatten())
        .filter(|r| r.has_local)
        .take(12)
        .collect();
    if mix.len() < 12 {
        // Cold start: fill from the library at random.
        let n = lib
            .count_tracks(nikgob_library::TrackFilter::LocalOnly)
            .unwrap_or(0);
        for _ in 0..(12 - mix.len()).min(n as usize) {
            let off = fastrand::u32(..n.max(1));
            if let Ok(rows) = lib.tracks_filtered(
                off,
                1,
                Sort::asc(nikgob_library::TrackSortKey::Title),
                nikgob_library::TrackFilter::LocalOnly,
            ) {
                for r in rows {
                    if !mix.iter().any(|m| m.id == r.id) {
                        mix.push(r);
                    }
                }
            }
        }
    }

    // Long unplayed: releases whose tracks were last played > 60 days ago.
    let now = now_ms();
    let mut last_by_release: HashMap<ReleaseId, i64> = HashMap::new();
    for (id, s) in &stats {
        if let (Some(lp), Ok(Some(t))) = (s.last_played, lib.get_track(*id))
            && let Some(rid) = t.release_id
        {
            let e = last_by_release.entry(rid).or_insert(lp);
            *e = (*e).max(lp);
        }
    }
    let mut long_unplayed: Vec<(i64, ReleaseRow)> = last_by_release
        .iter()
        .filter(|(_, lp)| now - **lp > 60 * DAY_MS)
        .filter_map(|(rid, lp)| by_id.get(rid).map(|r| (*lp, (*r).clone())))
        .collect();
    long_unplayed.sort_by_key(|(lp, _)| *lp);

    // Artist mixes: most played artists.
    let artists_all = lib
        .artists(0, 20_000, Sort::asc(ArtistSortKey::Name))
        .unwrap_or_default();
    let mut plays_by_artist: HashMap<ArtistId, u32> = HashMap::new();
    for (id, s) in &stats {
        if let Ok(Some(t)) = lib.get_track(*id) {
            for a in t.artist_credit.artist_ids() {
                *plays_by_artist.entry(a).or_default() += s.plays;
            }
        }
    }
    let mut artists: Vec<ArtistRow> = artists_all
        .into_iter()
        .filter(|a| a.local_track_count > 0)
        .collect();
    artists.sort_by_key(|a| std::cmp::Reverse(plays_by_artist.get(&a.artist.id).copied().unwrap_or(0)));
    artists.truncate(12);

    Home {
        hour,
        mix,
        continue_listening,
        artists,
        long_unplayed: long_unplayed.into_iter().take(12).map(|(_, r)| r).collect(),
        recently_added: releases.into_iter().take(12).collect(),
    }
}

fn local_hour() -> u8 {
    jiff::Zoned::now().hour() as u8
}
