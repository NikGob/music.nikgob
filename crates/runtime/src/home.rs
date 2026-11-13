//! The home feed (simple stage-1 version): built from the event log
//! and the catalog. Recommendations proper arrive in stage 4.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, Release, ReleaseId, TrackId};
use nikgob_core::streaming::{EntityKind, RemoteId};
use nikgob_library::{
    ArtistRow, ArtistSortKey, Library, RecTrack, ReleaseProgress, ReleaseRow, ReleaseSortKey, Sort, TrackRow,
    TrackStats, now_ms,
};

/// An album of «Продолжить».
pub enum Continue {
    /// In the library.
    Release(ReleaseRow),
    /// Streamed from a service without joining the library: its page opens.
    Stream {
        remote: RemoteId,
        release: Release,
        progress: Option<ReleaseProgress>,
    },
}

pub struct Home {
    pub hour: u8,
    /// No song in the library and none ever played ([`library_empty`]).
    pub library_empty: bool,
    /// Главная v2's parts read at the visit.
    pub v2: crate::home_v2::HomeV2,
    pub mix: Vec<TrackRow>,
    pub continue_listening: Vec<Continue>,
    pub artists: Vec<ArtistRow>,
    pub long_unplayed: Vec<ReleaseRow>,
    pub recently_added: Vec<ReleaseRow>,
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The home of a visit: what plays now and the prefs it is under.
pub fn build_with(lib: &Library, visit: &crate::home_v2::Visit) -> Home {
    let hour = local_hour();
    let v2 = crate::home_v2::build(lib, visit);
    let releases = lib
        .releases(0, 20_000, Sort::desc(ReleaseSortKey::Added))
        .unwrap_or_default();
    let by_id: HashMap<ReleaseId, &ReleaseRow> =
        releases.iter().map(|r| (r.release.id, r)).collect();
    let stats = lib.all_track_stats().unwrap_or_default();
    let stat: HashMap<TrackId, _> = stats.iter().copied().collect();

    // Continue: releases of the most recently started tracks; an album
    // streamed from a service (not in the library) opens its page.
    let mut continue_listening = Vec::new();
    let mut seen = HashSet::new();
    for e in lib.recent_events(400).unwrap_or_default() {
        if continue_listening.len() >= 12 {
            break;
        }
        let Ok(Some(t)) = lib.get_track(e.track_id) else { continue };
        let Some(rid) = t.release_id else { continue };
        if !seen.insert(rid) {
            continue;
        }
        if let Some(r) = by_id.get(&rid) {
            // A song or a few of an album are the user's songs, not an
            // album to go on with.
            if visit.album_shows(r) {
                continue_listening.push(Continue::Release((*r).clone()));
            }
        } else if let Some(remote) = lib
            .external_ids(EntityKind::Album, *rid.as_uuid())
            .unwrap_or_default()
            .into_iter()
            .next()
            && let Ok(Some(release)) = lib.get_release(rid)
        {
            let progress = lib.release_progress(rid).ok().flatten();
            continue_listening.push(Continue::Stream { remote, release, progress });
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
    // A song on two albums is in the pool once (its stats are the
    // standing copy's already).
    let spares = lib.recording_spares().unwrap_or_default();
    for id in lib.liked_track_ids().unwrap_or_default() {
        let id = spares.get(&id).copied().unwrap_or(id);
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
        .filter_map(|(rid, lp)| by_id.get(rid).filter(|r| visit.album_shows(r)).map(|r| (*lp, (*r).clone())))
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
        library_empty: library_empty(lib),
        v2,
        mix,
        continue_listening,
        artists,
        long_unplayed: long_unplayed.into_iter().take(12).map(|(_, r)| r).collect(),
        recently_added: recently_added(lib, releases.into_iter().filter(|r| visit.album_shows(r)), 12),
    }
}

/// The library has no song and nothing was ever played, as the library
/// counts them: the rows built from it leave out partial albums and small
/// libraries, so their being empty does not say this. A count that fails
/// says «not empty» (Главная never claims there is no music by mistake).
pub fn library_empty(lib: &Library) -> bool {
    lib.count_tracks(nikgob_library::TrackFilter::All).is_ok_and(|n| n == 0) && lib.count_events().is_ok_and(|n| n == 0)
}

/// «Недавно добавлено»: the latest albums of `releases` (newest first)
/// that someone added, not those only an import's playlists brought
/// ([`Library::releases_of_imported_playlists`]).
pub fn recently_added(lib: &Library, releases: impl IntoIterator<Item = ReleaseRow>, n: usize) -> Vec<ReleaseRow> {
    let brought = lib.releases_of_imported_playlists().unwrap_or_default();
    releases.into_iter().filter(|r| !brought.contains(&r.release.id)).take(n).collect()
}

/// «Часто слушаешь» of Библиотека: releases by the plays of their tracks
/// (a finished play counts double, a skip takes one back), the most played
/// first; a release needs a few plays to count at all.
pub fn often_played(tracks: &[RecTrack], stats: &[(TrackId, TrackStats)], limit: usize) -> Vec<ReleaseId> {
    let release: HashMap<TrackId, ReleaseId> = tracks.iter().filter_map(|t| Some((t.id, t.release?))).collect();
    let mut score: HashMap<ReleaseId, i64> = HashMap::new();
    for (id, s) in stats {
        if let Some(r) = release.get(id) {
            *score.entry(*r).or_default() += i64::from(s.plays) + i64::from(s.completes) - i64::from(s.skips);
        }
    }
    let mut ranked: Vec<(i64, ReleaseId)> = score.into_iter().filter(|(_, s)| *s >= 3).map(|(r, s)| (s, r)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ranked.into_iter().take(limit).map(|(_, r)| r).collect()
}

/// Library tracks per credited artist («12 треков» under an avatar); with
/// `only_local` the ones with a file.
pub fn artist_track_counts(tracks: &[RecTrack], only_local: bool) -> HashMap<ArtistId, u32> {
    let mut n: HashMap<ArtistId, u32> = HashMap::new();
    for t in tracks.iter().filter(|t| t.in_library && (t.has_local || !only_local)) {
        for a in &t.artists {
            *n.entry(*a).or_default() += 1;
        }
    }
    n
}

fn local_hour() -> u8 {
    jiff::Zoned::now().hour() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// «Музыки пока нет» is the library's own count, not the rows': one
    /// song of an album (a partial album, a library under the rows'
    /// minimum) is music.
    #[test]
    fn the_library_is_empty_by_its_own_count() {
        use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};
        let lib = Library::open_in_memory().unwrap();
        let visit = crate::home_v2::Visit { now: now_ms(), ..crate::home_v2::Visit::default() };
        assert!(library_empty(&lib) && build_with(&lib, &visit).library_empty, "a new library");
        let a = lib.upsert_artist(&Artist::new("Kevin Penkin")).unwrap();
        let credit = ArtistCredit::single("Kevin Penkin", Some(a.id));
        let r = lib.upsert_release(&Release::new("MADE IN ABYSS ORIGINAL SOUNDTRACK", credit.clone())).unwrap();
        let mut t = Track::new("Hanezeve Caradhina", credit);
        t.release_id = Some(r.id);
        t.track_no = 3;
        let t = lib.upsert_track(&t).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "x/3.flac", Codec::Flac)).unwrap();
        assert!(!library_empty(&lib) && !build_with(&lib, &visit).library_empty, "one song of an album is music");
    }

    fn track(release: Option<ReleaseId>, artists: Vec<ArtistId>, in_library: bool, has_local: bool) -> RecTrack {
        RecTrack {
            id: TrackId::new(),
            title: String::new(),
            artist: String::new(),
            artists,
            release,
            in_library,
            has_local,
            song: String::new(),
        }
    }

    fn stats(plays: u32, completes: u32, skips: u32) -> TrackStats {
        TrackStats { plays, completes, skips, last_played: None }
    }

    #[test]
    fn often_played_ranks_releases_by_their_tracks_plays() {
        let (a, b, c) = (ReleaseId::new(), ReleaseId::new(), ReleaseId::new());
        let ts = [
            track(Some(a), vec![], true, true),
            track(Some(a), vec![], true, true),
            track(Some(b), vec![], true, true),
            track(Some(c), vec![], true, true),
            track(None, vec![], true, true),
        ];
        let st = vec![
            (ts[0].id, stats(2, 2, 0)),
            (ts[1].id, stats(1, 1, 0)),
            (ts[2].id, stats(5, 4, 0)),
            // Skipped as often as started: not a favourite.
            (ts[3].id, stats(3, 0, 3)),
            (ts[4].id, stats(9, 9, 0)),
        ];
        assert_eq!(often_played(&ts, &st, 10), vec![b, a]);
        assert_eq!(often_played(&ts, &st, 1), vec![b]);
    }

    #[test]
    fn artist_counts_take_library_tracks_of_every_credited_artist() {
        let (x, y) = (ArtistId::new(), ArtistId::new());
        let ts = [
            track(None, vec![x], true, true),
            track(None, vec![x, y], true, false),
            // Only seen in a service: not in the library.
            track(None, vec![y], false, false),
        ];
        let all = artist_track_counts(&ts, false);
        assert_eq!((all.get(&x).copied(), all.get(&y).copied()), (Some(2), Some(1)));
        let local = artist_track_counts(&ts, true);
        assert_eq!((local.get(&x).copied(), local.get(&y).copied()), (Some(1), None));
    }
}
