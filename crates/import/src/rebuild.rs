//! Rebuilds library rows from the files in `Music/` (PLAN section 4: the track
//! id and MBIDs live in the file tags, so the database can be recreated).
//!
//! Tracks keep the id from `NIKGOB_TRACK_ID`; files without one get a fresh id,
//! which is written back into the file. Releases are grouped by MBID or by
//! (album, album artist). The lossless rank of a converted file is restored
//! from the master sidecars in `.musiclib/masters/`.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{Palette, ReleaseId, Source, SourceKind, Track, TrackId, quality_score};
use nikgob_library::{Library, ReleaseRow, ReleaseSort};
use serde::Deserialize;

use crate::config::ImportConfig;
use crate::cover::derive_visuals;
use crate::error::Result;
use crate::fsutil;
use crate::identity::identity_from_tags;
use crate::sanitize::long_path;
use crate::store::{Store, lead_key, release_key};
use crate::tags::{self, TagSet};
use nikgob_metadata::text::title_key;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebuildReport {
    /// Audio files found.
    pub files: usize,
    /// Tracks that did not exist in the database before.
    pub tracks_created: usize,
    /// Files whose track already existed (their source row was refreshed).
    pub tracks_existing: usize,
    /// Files that had no track id; one was generated and written to the file.
    pub ids_assigned: usize,
    /// Files that could not be read.
    pub unreadable: usize,
    pub palettes: usize,
}

#[derive(Deserialize)]
struct SidecarScore {
    track_id: TrackId,
    /// Every track of an image (a single-file rip of a disc).
    #[serde(default)]
    track_ids: Vec<TrackId>,
    quality_score: i32,
}

fn master_scores(cfg: &ImportConfig) -> HashMap<TrackId, i32> {
    let mut out: HashMap<TrackId, i32> = HashMap::new();
    let default_dir = cfg.musiclib_dir().join("masters");
    let configured_dir = cfg.masters_dir();
    let dirs = if configured_dir == default_dir {
        vec![configured_dir]
    } else {
        vec![configured_dir, default_dir]
    };
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(long_path(&dir)) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(bytes) = std::fs::read(&p)
                && let Ok(s) = serde_json::from_slice::<SidecarScore>(&bytes)
            {
                for t in std::iter::once(s.track_id).chain(s.track_ids) {
                    out.entry(t)
                        .and_modify(|q| *q = (*q).max(s.quality_score))
                        .or_insert(s.quality_score);
                }
            }
        }
    }
    out
}

/// Walks `cfg.library_dir` and recreates artist, release, track and source rows.
pub fn rebuild_from_files(library: &Library, cfg: &ImportConfig) -> Result<RebuildReport> {
    let (files, _) = fsutil::scan(std::slice::from_ref(&cfg.library_dir));
    let masters_dir = cfg.masters_dir();
    let masters = master_scores(cfg);
    let mut store = Store::new(library.clone());
    let mut report = RebuildReport::default();
    for path in files {
        if path.starts_with(&masters_dir) {
            continue;
        }
        report.files += 1;
        let (tags, info) = match tags::read_tags(&path) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("rebuild: skipping {}: {e}", path.display());
                report.unreadable += 1;
                continue;
            }
        };
        let id = identity_from_tags(&tags, &path);
        let track_id = match tags.track_id {
            Some(t) => t,
            None => {
                let t = TrackId::new();
                let patch = TagSet {
                    track_id: Some(t),
                    ..TagSet::default()
                };
                if let Err(e) = tags::write_tags(&path, &patch) {
                    log::warn!("rebuild: cannot write a track id into {}: {e}", path.display());
                }
                report.ids_assigned += 1;
                t
            }
        };
        let existed = library.get_track(track_id)?.is_some();

        let dir = path.parent().unwrap_or(&cfg.library_dir).to_path_buf();
        let cover_path = dir.join("cover.jpg");
        let has_cover = long_path(&cover_path).exists();
        let cover_ref = has_cover.then(|| {
            cover_path
                .strip_prefix(&cfg.library_dir)
                .unwrap_or(&cover_path)
                .to_string_lossy()
                .replace('\\', "/")
        });
        let release = store.ensure_release(&id, cover_ref)?;
        let credit = store.track_credit(&id)?;
        let mut track = Track::new(id.title.clone(), credit);
        track.id = track_id;
        track.recording_mbid = id.recording_mbid.clone();
        track.isrc = id.isrc.clone();
        track.acoustid = id.acoustid_id.clone();
        track.release_id = Some(release.id);
        track.disc_no = id.disc_no;
        track.track_no = id.track_no;
        track.duration_ms = Some(info.duration_ms());
        let track = library.upsert_track(&track)?;

        let mut source = Source::new(
            track.id,
            SourceKind::LocalFile,
            path.to_string_lossy().into_owned(),
            info.codec.clone(),
        );
        source.bitrate = info.bitrate_kbps;
        source.sample_rate = info.sample_rate;
        source.bit_depth = info.bit_depth;
        source.quality_score = masters.get(&track.id).copied().unwrap_or_else(|| {
            quality_score(
                &info.codec,
                info.bitrate_kbps,
                info.sample_rate,
                info.bit_depth,
                false,
            )
        });
        source.file_hash = fsutil::hash_file(&path).ok();
        source.size_bytes = fsutil::file_size(&path).ok();
        library.add_source(&source)?;

        if existed {
            report.tracks_existing += 1;
        } else {
            report.tracks_created += 1;
        }

        if has_cover && library.get_palette(release.id)?.is_none() {
            let derived = std::fs::read(long_path(&cover_path))
                .map_err(fsutil::io_err(&cover_path))
                .and_then(|b| derive_visuals(&b));
            match derived {
                Ok(v) => {
                    let mut p = Palette::new(release.id, v.seed_argb.unwrap_or(cfg.fallback_seed_argb));
                    p.ambient_blur = Some(v.ambient_png);
                    library.put_palette(&p)?;
                    report.palettes += 1;
                }
                Err(e) => log::warn!("rebuild: palette of {}: {e}", cover_path.display()),
            }
        }
    }
    Ok(report)
}

/// Joins releases of one album that were imported in pieces: same title and
/// the same credit (case, punctuation), and a release credited to "A, B" (a
/// guest on a track artist line) into the plain "A" one when that is the only
/// such pair. Two different artist credits, two MusicBrainz editions, or the
/// same track numbers twice are left alone. Returns how many releases were
/// merged away.
pub fn merge_split_releases(library: &Library) -> Result<usize> {
    let mut merged = 0;
    let mut by_credit: HashMap<(String, String), Vec<ReleaseRow>> = HashMap::new();
    for row in all_releases(library)? {
        let r = &row.release;
        by_credit
            .entry(release_key(&r.title, &r.artist_credit.display()))
            .or_default()
            .push(row);
    }
    for (_, rows) in by_credit {
        merged += merge_group(library, rows)?;
    }
    // What is left: "A" next to a single "A, B" of the same album.
    let mut by_lead: HashMap<(String, String), Vec<ReleaseRow>> = HashMap::new();
    for row in all_releases(library)? {
        let r = &row.release;
        by_lead
            .entry(lead_key(&r.title, &r.artist_credit.display()))
            .or_default()
            .push(row);
    }
    for (_, rows) in by_lead {
        let (plain, joint): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| {
            let credit = r.release.artist_credit.display();
            lead_key("", &credit) == release_key("", &credit)
        });
        if plain.len() == 1 && joint.len() == 1 {
            merged += merge_group(library, plain.into_iter().chain(joint).collect())?;
        }
    }
    Ok(merged)
}

/// Editions of one album found in the library (one MusicBrainz release
/// group: the Japanese release next to the international one, a reissue;
/// without MusicBrainz, releases of one artist sharing most songs: the
/// album and its «Special Edition»), for the user to fold together or keep
/// apart.
#[derive(Clone, Debug, PartialEq)]
pub struct EditionGroup {
    /// The release group MBID, or `songs:` and the fullest release's id.
    pub group: String,
    /// The fullest edition: the others would join it.
    pub keep: ReleaseRow,
    pub others: Vec<ReleaseRow>,
    /// Songs the others have that `keep` already has (they would become one
    /// track each) and songs only they have (they would join the album).
    pub same_songs: u32,
    pub new_songs: u32,
}

/// Share of the smaller release's songs two releases without a common
/// release group must have in common to be editions.
const SHARED_SONGS: f64 = 0.6;

/// Songs two releases without a common release group must have in common
/// at least (a single shares all its songs with its album).
const MIN_SHARED_SONGS: usize = 3;

/// Finds releases that are editions of one album. Nothing changes.
pub fn find_release_editions(library: &Library) -> Result<Vec<EditionGroup>> {
    let releases = all_releases(library)?;
    let mut groups: HashMap<String, Vec<ReleaseRow>> = HashMap::new();
    for row in &releases {
        if let Some(g) = row.release.release_group_mbid.clone() {
            groups.entry(g).or_default().push(row.clone());
        }
    }
    let mut found = Vec::new();
    for (group, mut rows) in groups {
        if rows.len() < 2 {
            continue;
        }
        rows.sort_by_key(|r| (std::cmp::Reverse(r.known_tracks), r.release.added_at));
        let keep = rows.remove(0);
        let album = songs(library, keep.release.id)?;
        let artist = release_key("", &keep.release.artist_credit.display()).1;
        let (mut same_songs, mut new_songs) = (0, 0);
        let mut others = Vec::new();
        for row in rows {
            // The release group alone may come from a wrong lookup: an
            // edition must also be by the same artist and share at least
            // one song with the album.
            if release_key("", &row.release.artist_credit.display()).1 != artist {
                continue;
            }
            let fates = plan(&album, &songs(library, row.release.id)?);
            let same = fates.iter().filter(|(_, f)| matches!(f, Fate::Same(_))).count() as u32;
            if same == 0 {
                continue;
            }
            same_songs += same;
            new_songs += fates.len() as u32 - same;
            others.push(row);
        }
        if !others.is_empty() {
            found.push(EditionGroup { group, keep, others, same_songs, new_songs });
        }
    }
    let grouped: Vec<ReleaseId> = found
        .iter()
        .flat_map(|g| std::iter::once(&g.keep).chain(&g.others).map(|r| r.release.id))
        .collect();
    let rest = releases.into_iter().filter(|r| !grouped.contains(&r.release.id)).collect();
    found.extend(editions_by_songs(library, rest)?);
    found.sort_by(|a, b| a.keep.release.title.cmp(&b.keep.release.title));
    Ok(found)
}

/// Editions MusicBrainz does not tie together: releases of one artist where
/// the other shares at least [`SHARED_SONGS`] of the smaller one's songs
/// (and [`MIN_SHARED_SONGS`]). Two different release groups are two albums
/// (the volumes of a soundtrack share no songs anyway).
fn editions_by_songs(library: &Library, releases: Vec<ReleaseRow>) -> Result<Vec<EditionGroup>> {
    let mut by_artist: HashMap<String, Vec<ReleaseRow>> = HashMap::new();
    for row in releases {
        let artist = release_key("", &row.release.artist_credit.display()).1;
        if !artist.is_empty() {
            by_artist.entry(artist).or_default().push(row);
        }
    }
    let mut found = Vec::new();
    for (_, mut rows) in by_artist {
        if rows.len() < 2 {
            continue;
        }
        rows.sort_by_key(|r| (std::cmp::Reverse(r.known_tracks), r.release.added_at));
        let mut tracks = Vec::with_capacity(rows.len());
        for r in &rows {
            tracks.push(songs(library, r.release.id)?);
        }
        let mut taken = vec![false; rows.len()];
        for k in 0..rows.len() {
            if taken[k] || tracks[k].is_empty() {
                continue;
            }
            let (mut same_songs, mut new_songs) = (0, 0);
            let mut others = Vec::new();
            for o in k + 1..rows.len() {
                let apart = match (&rows[k].release.release_group_mbid, &rows[o].release.release_group_mbid) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                };
                if taken[o] || apart || tracks[o].is_empty() {
                    continue;
                }
                let fates = plan(&tracks[k], &tracks[o]);
                let same = fates.iter().filter(|(_, f)| matches!(f, Fate::Same(_))).count();
                let smaller = tracks[k].len().min(tracks[o].len());
                if same < MIN_SHARED_SONGS || (same as f64) < SHARED_SONGS * smaller as f64 {
                    continue;
                }
                taken[o] = true;
                same_songs += same as u32;
                new_songs += (fates.len() - same) as u32;
                others.push(rows[o].clone());
            }
            if !others.is_empty() {
                taken[k] = true;
                found.push(EditionGroup {
                    group: format!("songs:{}", rows[k].release.id),
                    keep: rows[k].clone(),
                    others,
                    same_songs,
                    new_songs,
                });
            }
        }
    }
    Ok(found)
}

/// Folds the other editions of `g` into its fullest one: a song both have
/// becomes one track (the extra file a spare source of it, the better file
/// playing), songs only the others have join the album, which keeps its
/// title and ids. A song whose place is unknown keeps its number when that
/// is free, else takes the next number after the last of its disc (the
/// disc its file's folder names, `CD2`, else its own): no two songs at one
/// place. Nothing on disk changes.
pub fn fold_editions(library: &Library, g: &EditionGroup) -> Result<()> {
    let target = g.keep.release.id;
    let album = songs(library, target)?;
    let mut taken: HashSet<(u32, u32)> = album.iter().map(|s| s.number).filter(|n| n.1 > 0).collect();
    for row in &g.others {
        // Gone or folded meanwhile.
        if library.get_release(row.release.id)?.is_none() {
            continue;
        }
        for (song, fate) in plan(&album, &songs(library, row.release.id)?) {
            match fate {
                Fate::Same(same) => {
                    library.merge_track_into(song, same)?;
                }
                // Numbered the way this album counts.
                Fate::Join(Some((disc, no))) => {
                    if let Some(mut t) = library.get_track(song)? {
                        t.disc_no = disc;
                        t.track_no = no;
                        library.upsert_track(&t)?;
                        taken.insert((disc, no));
                    }
                }
                Fate::Join(None) => {
                    if let Some(mut t) = library.get_track(song)? {
                        let disc = folder_disc(library, song)?.unwrap_or(t.disc_no.max(1));
                        let no = free_number(&taken, disc, t.track_no);
                        if (disc, no) != (t.disc_no, t.track_no) {
                            log::info!(
                                "import: «{}» joins at {disc}-{no} (was {}-{})",
                                t.title,
                                t.disc_no,
                                t.track_no
                            );
                            t.disc_no = disc;
                            t.track_no = no;
                            library.upsert_track(&t)?;
                        }
                        taken.insert((disc, no));
                    }
                }
            }
        }
        library.absorb_edition(row.release.id, target)?;
    }
    Ok(())
}

/// The disc the folder of a track's file names (`CD2`, `Disc 2`).
fn folder_disc(library: &Library, track: TrackId) -> Result<Option<u32>> {
    Ok(library
        .active_source(track)?
        .filter(|s| s.kind.is_local())
        .and_then(|s| {
            let dir = std::path::Path::new(&s.locator).parent()?.file_name()?.to_string_lossy().into_owned();
            nikgob_core::names::disc_of_folder(&dir)
        }))
}

/// `no` on `disc` when that place is free, else the one after the last
/// taken number of the disc.
pub(crate) fn free_number(taken: &HashSet<(u32, u32)>, disc: u32, no: u32) -> u32 {
    if no > 0 && !taken.contains(&(disc, no)) {
        return no;
    }
    taken.iter().filter(|n| n.0 == disc).map(|n| n.1).max().unwrap_or(0) + 1
}

fn songs(library: &Library, release: ReleaseId) -> Result<Vec<Song>> {
    library
        .tracks_of_release(release)?
        .iter()
        .map(|t| {
            let key = title_key(&t.title);
            Ok(Song {
                id: t.id,
                recording: library.get_track(t.id)?.and_then(|t| t.recording_mbid),
                voiceless: title_key(&nikgob_core::names::voiceless(&t.title)).base,
                base: key.base,
                variant: key.variant,
                number: (t.disc_no, t.track_no),
                duration_ms: t.duration_ms,
            })
        })
        .collect()
}

/// What tells two tracks of two editions apart.
struct Song {
    id: TrackId,
    recording: Option<String>,
    /// [`Self::base`] without sound marks: `パウロ` and `バウロ` are one.
    voiceless: String,
    base: String,
    variant: nikgob_metadata::text::Variant,
    number: (u32, u32),
    duration_ms: Option<u32>,
}

/// What becomes of a track of another edition.
#[derive(Debug, PartialEq)]
enum Fate {
    /// The album has this song already: folds into that track.
    Same(TrackId),
    /// Joins the album, renumbered when its place is known.
    Join(Option<(u32, u32)>),
}

/// Matches another edition's tracks against the album. Editions number
/// differently (the Japanese one straight through, 1..30, where the
/// international one has two discs), so the songs both have tell where the
/// rest go: one 5 places after a shared song sits 5 places after it here
/// too. A track that lands on a song of the same length is that song under
/// another title (a translation: "Ningyō x 龍" is "Doll x Dragon"); one of
/// the same title and length elsewhere is that song moved (a special
/// edition puts new tracks in between).
fn plan(album: &[Song], other: &[Song]) -> Vec<(TrackId, Fate)> {
    let mut pairs: Vec<((u32, u32), (u32, u32))> = Vec::new();
    let mut fates: Vec<(TrackId, Option<Fate>)> = Vec::new();
    for o in other {
        let found = album.iter().find(|a| a.same_song(o)).or_else(|| album.iter().find(|a| a.moved_song(o)));
        match found {
            Some(a) => {
                if o.number.1 > 0 && a.number.1 > 0 {
                    pairs.push((o.number, a.number));
                }
                fates.push((o.id, Some(Fate::Same(a.id))));
            }
            None => fates.push((o.id, None)),
        }
    }
    let taken = |slot: (u32, u32)| album.iter().find(|a| a.number == slot);
    fates
        .into_iter()
        .zip(other)
        .map(|((id, fate), o)| {
            if let Some(f) = fate {
                return (id, f);
            }
            let (disc, no) = o.number;
            if no == 0 {
                return (id, Fate::Join(None));
            }
            // The nearest shared songs before and after it on its disc.
            let on_disc = || pairs.iter().filter(|(from, _)| from.0 == disc);
            let before = on_disc().filter(|(from, _)| from.1 < no).max_by_key(|(from, _)| from.1);
            let after = on_disc().filter(|(from, _)| from.1 > no).min_by_key(|(from, _)| from.1);
            let slot = match (before, after) {
                // Between two discs of the album (its first disc ended in
                // between): counted back from the song after it.
                (Some(b), Some(a)) if b.1.0 != a.1.0 && a.1.1 > a.0.1 - no => Some((a.1.0, a.1.1 - (a.0.1 - no))),
                (Some(&(from, to)), _) => Some((to.0, to.1 + (no - from.1))),
                (None, Some(&(from, to))) if to.1 > from.1 - no => Some((to.0, to.1 - (from.1 - no))),
                _ => None,
            };
            let Some(slot) = slot else {
                return (id, Fate::Join(None));
            };
            match taken(slot) {
                Some(a) => (id, if a.close(o) { Fate::Same(a.id) } else { Fate::Join(None) }),
                None => (id, Fate::Join(Some(slot))),
            }
        })
        .collect()
}

impl Song {
    /// The same recording, or the same title (and version) in the same
    /// place on the album; editions may carry different recording ids for
    /// one song, and a file may have lost its number.
    fn same_song(&self, other: &Song) -> bool {
        if self.recording.is_some() && self.recording == other.recording {
            return true;
        }
        let numbered = self.number.1 > 0 && other.number.1 > 0;
        self.base == other.base
            && self.variant == other.variant
            && (!numbered || self.number == other.number)
    }

    /// The same title (and version) and length in another place; or one
    /// rip of the song under a title a sound mark apart (lengths within
    /// 50 ms).
    fn moved_song(&self, other: &Song) -> bool {
        if self.variant != other.variant {
            return false;
        }
        (!self.base.is_empty() && self.base == other.base && self.close(other))
            || (!self.voiceless.is_empty()
                && self.voiceless == other.voiceless
                && matches!((self.duration_ms, other.duration_ms), (Some(x), Some(y)) if x.abs_diff(y) <= 50))
    }

    /// Both lengths known and within 3 s.
    fn close(&self, other: &Song) -> bool {
        matches!((self.duration_ms, other.duration_ms), (Some(x), Some(y)) if x.abs_diff(y) <= 3_000)
    }
}

fn all_releases(library: &Library) -> Result<Vec<ReleaseRow>> {
    const PAGE: u32 = 500;
    let mut all = Vec::new();
    loop {
        let page = library.releases(all.len() as u32, PAGE, ReleaseSort::asc(Default::default()))?;
        let last = page.len() < PAGE as usize;
        all.extend(page);
        if last {
            return Ok(all);
        }
    }
}

/// Merges `rows` into the fullest one, unless the group holds two different
/// MusicBrainz ids (editions); a release whose track numbers collide with the
/// survivor's stays.
fn merge_group(library: &Library, mut rows: Vec<ReleaseRow>) -> Result<usize> {
    let known = |id: fn(&ReleaseRow) -> Option<&String>| {
        let mut ids: Vec<&String> = rows.iter().filter_map(id).collect();
        ids.sort();
        ids.dedup();
        ids.len() > 1
    };
    if rows.len() < 2 || known(|r| r.release.mbid.as_ref()) || known(|r| r.release.release_group_mbid.as_ref()) {
        return Ok(0);
    }
    // The fullest release stays; the oldest one among equals.
    rows.sort_by_key(|r| (std::cmp::Reverse(r.known_tracks), r.release.added_at));
    let target = rows.remove(0).release;
    let numbered = |id| -> Result<Vec<(u32, u32)>> {
        Ok(library
            .tracks_of_release(id)?
            .iter()
            .filter(|t| t.track_no > 0)
            .map(|t| (t.disc_no, t.track_no))
            .collect())
    };
    let mut taken = numbered(target.id)?;
    let mut merged = 0;
    for row in rows {
        let theirs = numbered(row.release.id)?;
        if theirs.iter().any(|n| taken.contains(n)) {
            continue;
        }
        library.merge_releases(row.release.id, target.id)?;
        taken.extend(theirs);
        merged += 1;
    }
    Ok(merged)
}

#[cfg(test)]
mod edition_tests {
    use super::*;

    fn song(title: &str, number: (u32, u32), secs: u32) -> Song {
        let key = title_key(title);
        // Shared songs carry one recording id, as MusicBrainz tags give them.
        Song {
            id: TrackId::new(),
            recording: Some(title.to_owned()),
            voiceless: title_key(&nikgob_core::names::voiceless(title)).base,
            base: key.base,
            variant: key.variant,
            number,
            duration_ms: Some(secs * 1000),
        }
    }

    #[test]
    fn a_sound_mark_typo_of_one_rip_is_the_same_song() {
        let mut paulo = song("パウロ VS ルディ", (1, 19), 0);
        paulo.duration_ms = Some(135_602);
        paulo.recording = None;
        let album = vec![song("ギレーヌと二人きり", (1, 19), 120), Song { duration_ms: Some(135_602), number: (1, 23), ..song("バウロ VS ルディ", (1, 23), 0) }];
        let album: Vec<Song> = album.into_iter().map(|s| Song { recording: None, ..s }).collect();
        let fates = plan(&album, std::slice::from_ref(&paulo));
        assert_eq!(fates[0].1, Fate::Same(album[1].id));
        // 60 ms apart, or another song at the place: no.
        let mut far = song("パウロ VS ルディ", (1, 19), 0);
        far.recording = None;
        far.duration_ms = Some(135_662);
        assert!(!album.iter().any(|a| a.same_song(&far) || a.moved_song(&far)));
    }

    #[test]
    fn a_song_without_a_place_takes_a_free_number() {
        let taken: HashSet<(u32, u32)> = [(1, 1), (1, 2), (1, 19), (2, 1)].into_iter().collect();
        assert_eq!(free_number(&taken, 1, 5), 5, "its own number is free");
        assert_eq!(free_number(&taken, 1, 19), 20, "taken: after the last of the disc");
        assert_eq!(free_number(&taken, 1, 0), 20);
        assert_eq!(free_number(&taken, 2, 1), 2);
        assert_eq!(free_number(&taken, 3, 1), 1, "a new disc");
    }

    #[test]
    fn another_editions_numbers_are_mapped_through_the_songs_both_have() {
        // International: two discs of 15 and 17; Japanese: 1..32 straight.
        let album: Vec<Song> = (1..=15)
            .map(|n| song(&format!("A{n}"), (1, n), 100 + n))
            .chain((1..=17).map(|n| song(&format!("B{n}"), (2, n), 200 + n)))
            .collect();
        let jp = vec![
            song("A14", (1, 14), 114),
            song("A15", (1, 15), 115),
            // Only here, right where disc 2 starts: 2-1 is taken by B1
            // (another length), so it keeps its own number.
            song("Extra", (1, 16), 999),
            song("B2", (1, 17), 202),
            // Translated title, same place and length: the same song.
            song("Ningyō x 龍", (1, 30), 215),
            // After the last shared song, on a free place.
            song("Bonus", (1, 33), 300),
        ];
        let fates = plan(&album, &jp);
        let id = |n: (u32, u32)| album.iter().find(|a| a.number == n).unwrap().id;
        assert_eq!(fates[0].1, Fate::Same(id((1, 14))));
        assert_eq!(fates[2].1, Fate::Join(None));
        assert_eq!(fates[3].1, Fate::Same(id((2, 2))));
        assert_eq!(fates[4].1, Fate::Same(id((2, 15))));
        assert_eq!(fates[5].1, Fate::Join(Some((2, 18))));
    }
}
