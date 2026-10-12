//! Rebuilds library rows from the files in `Music/` (PLAN section 4: the track
//! id and MBIDs live in the file tags, so the database can be recreated).
//!
//! Tracks keep the id from `NIKGOB_TRACK_ID`; files without one get a fresh id,
//! which is written back into the file. Releases are grouped by MBID or by
//! (album, album artist). The lossless rank of a converted file is restored
//! from the master sidecars in `.musiclib/masters/`.

use std::collections::HashMap;

use nikgob_core::model::{Palette, Source, SourceKind, Track, TrackId, quality_score};
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
    quality_score: i32,
}

fn master_scores(cfg: &ImportConfig) -> HashMap<TrackId, i32> {
    let mut out = HashMap::new();
    let Ok(rd) = std::fs::read_dir(long_path(&cfg.masters_dir())) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Ok(bytes) = std::fs::read(&p)
            && let Ok(s) = serde_json::from_slice::<SidecarScore>(&bytes)
        {
            out.insert(s.track_id, s.quality_score);
        }
    }
    out
}

/// Walks `cfg.library_dir` and recreates artist, release, track and source rows.
pub fn rebuild_from_files(library: &Library, cfg: &ImportConfig) -> Result<RebuildReport> {
    let (files, _) = fsutil::scan(std::slice::from_ref(&cfg.library_dir));
    let masters = master_scores(cfg);
    let mut store = Store::new(library.clone());
    let mut report = RebuildReport::default();
    for path in files {
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
