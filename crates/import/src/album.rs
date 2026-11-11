//! A downloaded folder is an album, not a pile of files (docs/stage4.md
//! §12.4).
//!
//! The files of one folder (one disc) are identified together:
//! * [`Evidence`]: their tags, the folder's name and the torrent's name vote
//!   for one artist and one album («Oliver Tree - Ugly Is Beautiful (2020)
//!   [FLAC]» names both when the tags say nothing);
//! * [`Reference`]: a torrent fetched for a «Хочу скачать» item lands on the
//!   item's release, whose tracks (stream tracks with their numbers, titles
//!   and lengths) the files attach to, each track to one file; the
//!   release's tracklist numbers them, never the files' own tags (a folder
//!   of a bigger edition), a file that is none of its tracks follows its
//!   last one, and a song the user deleted from it is left out;
//!   MusicBrainz is not asked;
//! * otherwise MusicBrainz is asked for the release as a whole
//!   ([`lookup_album`]: by artist and title, then the track count and the
//!   lengths pick the edition), and each file takes its track of that
//!   release ([`assign`]). Files are looked up one by one only in a folder
//!   of many artists (a compilation) or a loose file.

use std::path::Path;

use nikgob_core::model::{Release, ReleaseType, Track, TrackId};
use nikgob_library::{Library, TrackRow, Unwanted};
use nikgob_metadata::Metadata;
use nikgob_metadata::matcher::{MatchCandidate, candidates_from_release};
use nikgob_metadata::musicbrainz::Release as MbRelease;
use nikgob_metadata::text::{album_key, artist_key, similarity, title_key};

use crate::config::ImportConfig;
use crate::decide::{DURATION_SLACK_MS, ImportTargets};
use crate::folder::primary_credit;
use crate::identity::{Identity, shares_artist};
use crate::layout::is_various_artists;

/// Releases asked for in one MusicBrainz search.
const SEARCH_LIMIT: u32 = 10;
/// Title similarity (0..=1) for a file and a track of the same number.
const NUMBERED_TITLE: f64 = 0.5;
/// Title similarity for a file and a track found by title alone.
const TITLE: f64 = 0.85;

/// "Artist - Album (2020) [FLAC] [Hi-Res 24-Bit]" → ("Artist", "Album"):
/// a folder or torrent name in the usual release form. Brackets (year,
/// format, edition) go; `None` without an « - ».
pub fn split_release_name(name: &str) -> Option<(String, String)> {
    let mut plain = String::with_capacity(name.len());
    let mut depth = 0usize;
    for c in name.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(if c == '_' { ' ' } else { c }),
            _ => {}
        }
    }
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let (artist, album) = plain.split_once(" - ").or_else(|| plain.split_once(" – "))?;
    let album = album.trim().trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == ' ').trim();
    let artist = artist.trim();
    (!artist.is_empty() && !album.is_empty()).then(|| (artist.to_owned(), album.to_owned()))
}

/// What a folder says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Evidence {
    /// Credit lines naming the album's artist: the tags' album artist (or
    /// lead artist), the folder's and the torrent's names.
    pub artists: Vec<String>,
    /// The album: what most tags say, else the folder's or torrent's name.
    pub album: Option<String>,
    /// The artist the album is filed under, best guess first.
    pub album_artist: Option<String>,
    /// Many artists, no album artist: each file is its own question.
    pub compilation: bool,
    pub files: usize,
}

/// Most common value (by `key`), first one on ties.
fn top<'a>(values: impl Iterator<Item = &'a str>, key: impl Fn(&str) -> String) -> Option<(&'a str, usize)> {
    let mut groups: Vec<(String, &'a str, usize)> = Vec::new();
    for v in values {
        let k = key(v);
        if k.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(g, _, _)| *g == k) {
            Some((_, _, n)) => *n += 1,
            None => groups.push((k, v, 1)),
        }
    }
    let best = (0..groups.len()).max_by_key(|&i| (groups[i].2, std::cmp::Reverse(i)))?;
    Some((groups[best].1, groups[best].2))
}

impl Evidence {
    /// `bases`: what the tags of each file say; `names`: the folder's and
    /// the torrent's names.
    pub fn new(bases: &[&Identity], names: &[&str]) -> Self {
        let n = bases.len();
        let named: Vec<(String, String)> = names.iter().filter_map(|n| split_release_name(n)).collect();
        let tagged_aa = top(
            bases.iter().filter_map(|b| b.album_artist.as_deref()).map(str::trim),
            artist_key,
        );
        let leads: Vec<&str> = bases
            .iter()
            .map(|b| primary_credit(&b.artist_line))
            .filter(|a| !a.is_empty() && *a != "Unknown Artist")
            .collect();
        let lead = top(leads.iter().copied(), artist_key);
        let various_aa = tagged_aa.is_some_and(|(a, _)| is_various_artists(a));
        // Many lead artists and no album artist of their own.
        let compilation = various_aa
            || (tagged_aa.is_none() && n >= 3 && lead.is_some_and(|(_, c)| c * 2 < leads.len()));
        let mut artists: Vec<String> = Vec::new();
        let mut push = |a: &str| {
            if !a.trim().is_empty() && !artists.iter().any(|x| artist_key(x) == artist_key(a)) {
                artists.push(a.trim().to_owned());
            }
        };
        if let Some((a, _)) = tagged_aa.filter(|_| !various_aa) {
            push(a);
        }
        if let Some((a, c)) = lead
            && !compilation
            && c * 2 >= leads.len()
        {
            push(a);
        }
        for (a, _) in &named {
            push(a);
        }
        let album = top(
            bases.iter().filter_map(|b| b.album.as_deref()).map(str::trim),
            album_key,
        )
        .filter(|(_, c)| n < 2 || c * 3 >= n * 2)
        .map(|(a, _)| a.to_owned())
        .or_else(|| named.first().map(|(_, a)| a.clone()));
        Self {
            album_artist: artists.first().cloned(),
            artists,
            album,
            compilation,
            files: n,
        }
    }

    /// One album by one artist, worth identifying as a whole.
    pub fn is_album(&self) -> bool {
        self.files >= 2 && !self.compilation && self.album.is_some() && self.album_artist.is_some()
    }

    /// A file of the album that lacks album tags takes them from the folder.
    pub fn fill(&self, base: &mut Identity) {
        if base.album.as_deref().is_none_or(|a| a.trim().is_empty()) {
            base.album.clone_from(&self.album);
            if base.album.is_some() && base.release_type == ReleaseType::Single {
                base.release_type = ReleaseType::Album;
            }
        }
        if base.album_artist.is_none() {
            base.album_artist.clone_from(&self.album_artist);
        }
        if (base.artist_line.trim().is_empty() || base.artist_line == "Unknown Artist")
            && let Some(a) = &self.album_artist
        {
            base.artist_line = a.clone();
            base.artists = vec![a.clone()];
        }
    }

    /// The lines a candidate for `base` must share an artist with.
    pub fn artists_for(&self, base: &Identity) -> Vec<String> {
        let mut out = self.artists.clone();
        if base.artist_line != "Unknown Artist" {
            out.push(base.artist_line.clone());
        }
        out.extend(base.album_artist.iter().filter(|a| !is_various_artists(a)).cloned());
        out
    }
}

/// The «Хочу скачать» release a download was for, with its tracks: its
/// tracklist numbers the files, whatever numbers their tags carry (a folder
/// of a bigger edition, files from several sources).
#[derive(Clone, Debug)]
pub(crate) struct Reference {
    pub release: Release,
    pub tracks: Vec<TrackRow>,
    /// The songs the user deleted from it: their files are not needed.
    pub unwanted: Vec<Unwanted>,
}

/// What a file of the folder is on the release: one of its tracks, or a
/// song the user deleted from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnRelease {
    Track(TrackId),
    Unwanted,
}

/// A place on a release: a library track or a MusicBrainz track.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slot<'a> {
    pub title: &'a str,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration_ms: Option<u32>,
}

impl<'a> From<&'a TrackRow> for Slot<'a> {
    fn from(t: &'a TrackRow) -> Self {
        Self { title: &t.title, disc_no: t.disc_no, track_no: t.track_no, duration_ms: t.duration_ms }
    }
}

/// Whether a file (its tags) and a track are one, and how well: the same
/// number and a similar title (or the same length), or the same title;
/// lengths within the slack when both are known. A similar title outranks
/// a number alone: a folder of another edition numbers the songs its own
/// way, and its «4» of the same length is not the album's «4».
pub(crate) fn file_is_track(base: &Identity, duration_ms: Option<u32>, t: Slot<'_>) -> Option<f64> {
    let (have, want) = (title_key(t.title), title_key(&base.title));
    if have.variant != want.variant {
        return None;
    }
    let sim = similarity(&have.base, &want.base);
    let numbered = base.track_no != 0 && t.track_no == base.track_no && t.disc_no == base.disc_no.max(1);
    let close = match (duration_ms, t.duration_ms) {
        (Some(a), Some(b)) => Some(a.abs_diff(b) <= DURATION_SLACK_MS),
        _ => None,
    };
    let titled = sim >= TITLE;
    let fits = titled || (numbered && (sim >= NUMBERED_TITLE || close == Some(true)));
    let score = sim + if numbered { 1.0 } else { 0.0 } + if titled { 2.0 } else { 0.0 };
    (fits && close != Some(false)).then_some(score)
}

/// The names a file of a folder may go by, for [`folder_has_album`]: its
/// own («01 - Title», «Artist - Title»), and its last part after « - »
/// with the number before it («Artist - Album - 06 - Title» is «06 -
/// Title», «Artist - Album - 01 Title» is «01 Title»).
fn name_forms(name: &str) -> Vec<String> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out = vec![file.to_owned()];
    let parts: Vec<&str> = file.split(" - ").map(str::trim).collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1];
        let before = parts[parts.len() - 2];
        let numbered = !before.is_empty() && before.len() <= 3 && before.chars().all(|c| c.is_ascii_digit());
        out.push(if numbered { format!("{before} - {last}") } else { last.to_owned() });
    }
    out
}

/// A track of an album as [`folder_has_album`] compares it.
#[derive(Clone, Copy, Debug)]
pub struct AlbumTrack<'a> {
    pub title: &'a str,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration_ms: Option<u32>,
}

/// Whether a folder, known only by its files' names and lengths (a
/// Soulseek folder, a torrent's list, before anything is downloaded), is
/// the album whose tracks are `tracks`: at least half of them are there,
/// each file one track, by the importer's own test of a file and a track
/// (the title, or the number and a like title, the lengths agreeing when
/// both are known). Another artist's album under the same word has none of
/// them.
pub fn folder_has_album(tracks: &[AlbumTrack<'_>], files: &[(&str, Option<u32>)]) -> bool {
    if tracks.is_empty() {
        return true;
    }
    let none = crate::tags::TagSet::default();
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, (name, duration)) in files.iter().enumerate() {
        for form in name_forms(name) {
            let base = crate::identity::identity_from_tags(&none, Path::new(&form));
            for (j, t) in tracks.iter().enumerate() {
                let slot = Slot { title: t.title, disc_no: t.disc_no.max(1), track_no: t.track_no, duration_ms: t.duration_ms };
                if let Some(score) = file_is_track(&base, *duration, slot) {
                    pairs.push((score, i, j));
                }
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let (mut file_used, mut track_used) = (vec![false; files.len()], vec![false; tracks.len()]);
    let mut found = 0usize;
    for (_, i, j) in pairs {
        if file_used[i] || track_used[j] {
            continue;
        }
        file_used[i] = true;
        track_used[j] = true;
        found += 1;
    }
    found > 0 && found * 2 >= tracks.len()
}

/// Which downloaded file is which track of an album, by the importer's own
/// test of a file and a track (the title, or the number and a like title,
/// the lengths agreeing when both are known): for each of `tracks`, the
/// index in `files` of its file, `None` when none is. The best pairs go
/// first, each file and each track taken once, so a bonus song or an
/// instrumental of the folder never takes an album song's place. `files`:
/// each file's tags, its path (its name says what tags do not) and its
/// length in ms.
pub fn album_files(tracks: &[AlbumTrack<'_>], files: &[(&crate::tags::TagSet, &Path, Option<u32>)]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, (tags, path, duration)) in files.iter().enumerate() {
        let base = crate::identity::identity_from_tags(tags, path);
        for (j, t) in tracks.iter().enumerate() {
            let slot = Slot { title: t.title, disc_no: t.disc_no.max(1), track_no: t.track_no, duration_ms: t.duration_ms };
            if let Some(score) = file_is_track(&base, *duration, slot) {
                pairs.push((score, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out: Vec<Option<usize>> = vec![None; tracks.len()];
    let mut file_used = vec![false; files.len()];
    for (_, i, j) in pairs {
        if file_used[i] || out[j].is_some() {
            continue;
        }
        file_used[i] = true;
        out[j] = Some(i);
    }
    out
}

impl Reference {
    /// The release of `targets` (its own, else the one most of its tracks
    /// are on) with every track of it, outside the library too.
    pub fn load(lib: &Library, targets: &ImportTargets) -> Option<Self> {
        let release = targets.release.or_else(|| {
            let mut ids: Vec<_> = targets
                .tracks
                .iter()
                .filter_map(|t| lib.get_track(*t).ok().flatten()?.release_id)
                .collect();
            ids.sort();
            let mut best = None;
            for chunk in ids.chunk_by(|a, b| a == b) {
                if best.is_none_or(|(_, n)| chunk.len() > n) {
                    best = Some((chunk[0], chunk.len()));
                }
            }
            best.map(|(r, _)| r)
        })?;
        let release = lib.get_release(release).ok()??;
        let tracks = lib.tracks_of_release_all(release.id).unwrap_or_default();
        let unwanted = lib.unwanted_of_release(release.id).unwrap_or_default();
        Some(Self { release, tracks, unwanted })
    }

    /// What each file (`None`: not to be matched) is on the release: the
    /// best pairs first, each track (and deleted song) taken once, so two
    /// files never claim one place.
    pub fn match_files(&self, files: &[Option<(&Identity, Option<u32>)>]) -> Vec<Option<OnRelease>> {
        let slots: Vec<(Slot<'_>, OnRelease)> = self
            .tracks
            .iter()
            .map(|t| (Slot::from(t), OnRelease::Track(t.id)))
            .chain(self.unwanted.iter().map(|u| {
                let slot = Slot { title: &u.title, disc_no: u.disc_no.max(1), track_no: u.track_no, duration_ms: u.duration_ms };
                (slot, OnRelease::Unwanted)
            }))
            .collect();
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let Some((base, duration)) = f else { continue };
            for (j, (slot, _)) in slots.iter().enumerate() {
                if let Some(score) = file_is_track(base, *duration, *slot) {
                    pairs.push((score, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut out = vec![None; files.len()];
        let mut taken = vec![false; slots.len()];
        for (_, i, j) in pairs {
            if out[i].is_some() || taken[j] {
                continue;
            }
            taken[j] = true;
            out[i] = Some(slots[j].1);
        }
        out
    }

    /// The first place after the album's tracks (its count, its last
    /// line): where a file of the folder that is none of them goes, so a
    /// bonus song of another edition follows the album instead of taking
    /// the number its own edition gave it.
    pub fn after(&self) -> (u32, u32) {
        let disc = self.tracks.iter().map(|t| t.disc_no.max(1)).max().unwrap_or(1);
        let count = if disc == 1 { self.release.total_tracks.unwrap_or(0) } else { 0 };
        let last = self.tracks.iter().filter(|t| t.disc_no.max(1) == disc).map(|t| t.track_no).max().unwrap_or(0);
        (disc, count.max(last) + 1)
    }

    /// Whether the folder is this release: its album title (exactly, or a
    /// longer or shorter one when most files are its tracks anyway), and an
    /// artist in common when the folder names any.
    pub fn fits(&self, ev: &Evidence, matched: usize) -> bool {
        let artist = vec![self.release.artist_credit.display()];
        if !shares_artist(&ev.artists, &artist) {
            return false;
        }
        let most = matched * 2 >= ev.files.max(1);
        match ev.album.as_deref().map(album_key) {
            Some(k) if k == album_key(&self.release.title) => true,
            Some(k) => most && crate::folder::same_title(&k, &album_key(&self.release.title)),
            None => most,
        }
    }

    /// A file of the release that is none of its known tracks: the release's
    /// album fields, its own title, the place `place` (see [`Self::after`]).
    pub fn identity(&self, base: &Identity, place: (u32, u32)) -> Identity {
        let mut id = base.clone();
        (id.disc_no, id.track_no) = place;
        onto_release(&mut id, &self.release);
        id
    }
}

/// The album fields of `r` on a file's identity: it lands there.
fn onto_release(id: &mut Identity, r: &Release) {
    id.album = Some(r.title.clone());
    let line = r.artist_credit.display();
    id.album_artist = Some(line.clone()).filter(|a| !a.is_empty());
    if (id.artist_line.trim().is_empty() || id.artist_line == "Unknown Artist") && !line.is_empty() {
        id.artist_line = line.clone();
        id.artists = r.artist_credit.0.iter().map(|p| p.name.clone()).collect();
    }
    id.year = r.year.or(id.year);
    id.release_type = r.release_type;
    id.compilation = r.release_type == ReleaseType::Compilation;
    id.release_mbid = r.mbid.clone().or(id.release_mbid.take());
    id.release_group_mbid = r.release_group_mbid.clone().or(id.release_group_mbid.take());
    id.track_total = r.total_tracks.or(id.track_total);
    id.release_id = Some(r.id);
}

/// A file of a «Хочу скачать» download that is a track of the album the
/// download is for (found by its ids, a liked stream, «Подтверди»; not by
/// the album's tracklist) keeps the track's place and lands on that album:
/// a file never renumbers its album, nor takes the track to the album its
/// own tags name. `None`: nothing to change.
pub(crate) fn keep_place(lib: &Library, targets: &ImportTargets, track: &Track, id: &Identity) -> Option<Identity> {
    let wished = targets
        .release
        .or_else(|| targets.tracks.iter().find_map(|t| lib.get_track(*t).ok().flatten()?.release_id))?;
    if track.release_id != Some(wished) {
        return None;
    }
    let release = lib.get_release(wished).ok()??;
    let mut out = id.clone();
    if track.track_no > 0 {
        (out.disc_no, out.track_no) = (track.disc_no.max(1), track.track_no);
    }
    onto_release(&mut out, &release);
    (out != *id).then_some(out)
}

fn track_count(r: &MbRelease) -> usize {
    r.media
        .iter()
        .map(|m| (m.track_count as usize).max(m.tracks.len()))
        .sum()
}

/// The search result the folder is: the album's title, an artist in
/// common, official; the fewest tracks the folder fits on (the edition of
/// exactly its size first).
pub(crate) fn pick_release<'a>(found: &'a [MbRelease], ev: &Evidence) -> Option<&'a MbRelease> {
    let album = album_key(ev.album.as_deref()?);
    found
        .iter()
        .filter(|r| r.is_official())
        .filter(|r| album_key(&r.title) == album)
        .filter(|r| shares_artist(&[r.artist_credit_string()], &ev.artists))
        .filter(|r| track_count(r) == 0 || track_count(r) >= ev.files)
        .min_by_key(|r| track_count(r).abs_diff(ev.files))
}

/// Each file's track on `release` (a full lookup): by number and title,
/// else by title; `None` when the lengths say the folder is another
/// edition (most matched files off by more than the slack) or fewer than
/// half the files found a track.
pub(crate) fn assign(release: &MbRelease, files: &[(&Identity, Option<u32>)]) -> Option<Vec<Option<MatchCandidate>>> {
    let cands = candidates_from_release(release);
    let rows: Vec<Slot<'_>> = cands
        .iter()
        .map(|c| Slot {
            title: &c.metadata.title,
            track_no: c.metadata.track_no.unwrap_or(0),
            disc_no: c.metadata.disc_no.unwrap_or(1),
            duration_ms: c.metadata.duration.map(|d| d.as_millis() as u32),
        })
        .collect();
    let mut taken = vec![false; cands.len()];
    let mut out = Vec::with_capacity(files.len());
    let (mut found, mut close) = (0usize, 0usize);
    for (base, duration) in files {
        let best = rows
            .iter()
            .enumerate()
            .filter(|(i, _)| !taken[*i])
            .filter_map(|(i, r)| Some((file_is_track(base, *duration, *r)?, i)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, i)| i);
        match best {
            Some(i) => {
                taken[i] = true;
                found += 1;
                if matches!((duration, rows[i].duration_ms), (Some(a), Some(b)) if a.abs_diff(b) <= DURATION_SLACK_MS) {
                    close += 1;
                }
                out.push(Some(MatchCandidate {
                    score: 1.0,
                    source: cands[i].source,
                    ids: cands[i].ids.clone(),
                    metadata: cands[i].metadata.clone(),
                    breakdown: Default::default(),
                }));
            }
            None => out.push(None),
        }
    }
    (found * 2 >= files.len() && close * 3 >= found * 2).then_some(out)
}

/// The album as a whole on MusicBrainz (see the module docs). Never fails:
/// nothing found, offline or a timeout is `None` (the tags stay).
pub(crate) async fn lookup_album(
    meta: &Metadata,
    cfg: &ImportConfig,
    ev: &Evidence,
    files: &[(&Identity, Option<u32>)],
) -> Option<Vec<Option<MatchCandidate>>> {
    let (artist, album) = (ev.album_artist.as_deref()?, ev.album.as_deref()?);
    let found = tokio::time::timeout(cfg.online_timeout, meta.musicbrainz.search_releases(artist, album, SEARCH_LIMIT))
        .await
        .ok()?
        .map_err(|e| log::debug!("musicbrainz release search: {e}"))
        .ok()?;
    let pick = pick_release(&found, ev)?;
    let full = tokio::time::timeout(cfg.online_timeout, meta.musicbrainz.lookup_release(&pick.id))
        .await
        .ok()?
        .map_err(|e| log::debug!("musicbrainz release lookup: {e}"))
        .ok()?;
    assign(&full, files)
}

/// The folder's own name and the torrent's (the first part of the path in
/// the torrent), for [`Evidence::new`].
pub(crate) fn names_of(dir: &Path, path_in_torrent: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(n) = dir.file_name() {
        out.push(n.to_string_lossy().into_owned());
    }
    if let Some(top) = path_in_torrent.and_then(|p| p.split('/').next()).filter(|t| !out.iter().any(|o| o == t))
        && path_in_torrent.is_some_and(|p| p.contains('/'))
    {
        out.push(top.to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::identity_from_tags;
    use crate::tags::TagSet;
    use nikgob_metadata::musicbrainz::{ArtistCredit, Medium, Track as MbTrack};

    fn file(title: &str, no: u32, artist: Option<&str>, album: Option<&str>) -> Identity {
        identity_from_tags(
            &TagSet {
                title: Some(title.into()),
                artist: artist.map(Into::into),
                album: album.map(Into::into),
                track_no: Some(no),
                ..TagSet::default()
            },
            Path::new("x.flac"),
        )
    }

    const OLIVER: &[(&str, u32)] = &[
        ("Me, Myself & I", 173_175),
        ("1993 (feat. Little Ricky ZR3)", 159_832),
        ("Cash Machine", 176_950),
        ("Let Me Down", 111_210),
        ("Miracle Man", 125_052),
        ("Bury Me Alive", 164_188),
        ("Alien Boy", 164_313),
        ("Joke's On You!", 191_144),
        ("Again & Again", 174_119),
        ("Waste My Time", 207_120),
        ("Jerk", 135_021),
        ("Hurt", 145_147),
        ("Introspective", 136_880),
        ("I'm Gone", 185_022),
    ];

    #[test]
    fn release_names_give_artist_and_album() {
        assert_eq!(
            split_release_name("Oliver Tree - Ugly Is Beautiful (2020) [FLAC] [Hi-Res 24-Bit]"),
            Some(("Oliver Tree".into(), "Ugly Is Beautiful".into()))
        );
        assert_eq!(
            split_release_name("Oliver Tree - Ugly is Beautiful Shorter, Thicker & Uglier (Deluxe) (2020 Alternativa e indie) [Flac 24-44]"),
            Some(("Oliver Tree".into(), "Ugly is Beautiful Shorter, Thicker & Uglier".into()))
        );
        assert_eq!(split_release_name("Eve - Under Blue - 2024"), Some(("Eve".into(), "Under Blue".into())));
        assert_eq!(split_release_name("Discography"), None);
    }

    #[test]
    fn an_untagged_folder_is_named_by_its_folder_and_torrent() {
        let bases: Vec<Identity> = OLIVER.iter().enumerate().map(|(i, (t, _))| file(t, i as u32 + 1, None, None)).collect();
        let refs: Vec<&Identity> = bases.iter().collect();
        let ev = Evidence::new(&refs, &["Oliver Tree - Ugly Is Beautiful (2020) [FLAC] [Hi-Res 24-Bit]"]);
        assert!(ev.is_album(), "{ev:?}");
        assert_eq!(ev.album.as_deref(), Some("Ugly Is Beautiful"));
        assert_eq!(ev.album_artist.as_deref(), Some("Oliver Tree"));
        let mut b = bases[2].clone();
        ev.fill(&mut b);
        assert_eq!((b.album.as_deref(), b.artist_line.as_str()), (Some("Ugly Is Beautiful"), "Oliver Tree"));
        assert!(ev.artists_for(&b).iter().any(|a| a == "Oliver Tree"));
    }

    #[test]
    fn many_artists_without_an_album_artist_are_a_compilation() {
        let bases: Vec<Identity> = ["A", "B", "C", "D"].iter().map(|a| file("T", 1, Some(a), Some("Hits"))).collect();
        let refs: Vec<&Identity> = bases.iter().collect();
        let ev = Evidence::new(&refs, &["Hits"]);
        assert!(ev.compilation && !ev.is_album());
    }

    fn mb_release(title: &str, artist: &str, tracks: &[(&str, u32)]) -> MbRelease {
        MbRelease {
            id: "20ad494e-8b74-4ad7-8ab3-2ae8955b7400".into(),
            title: title.into(),
            status: Some("Official".into()),
            artist_credit: vec![ArtistCredit {
                name: artist.into(),
                joinphrase: String::new(),
                artist: nikgob_metadata::musicbrainz::ArtistRef { id: "a".into(), name: artist.into(), ..Default::default() },
            }],
            media: vec![Medium {
                position: Some(1),
                track_count: tracks.len() as u32,
                tracks: tracks
                    .iter()
                    .enumerate()
                    .map(|(i, (t, ms))| MbTrack {
                        position: i as u32 + 1,
                        number: (i + 1).to_string(),
                        title: (*t).into(),
                        length: Some(*ms),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn the_folder_takes_the_release_of_its_size_and_each_file_its_track() {
        let bases: Vec<Identity> = OLIVER
            .iter()
            .enumerate()
            .map(|(i, (t, _))| file(t, i as u32 + 1, Some("Oliver Tree"), Some("Ugly Is Beautiful")))
            .collect();
        let refs: Vec<&Identity> = bases.iter().collect();
        let ev = Evidence::new(&refs, &[]);
        let album = mb_release("Ugly Is Beautiful", "Oliver Tree", OLIVER);
        let mut deluxe = album.clone();
        deluxe.media[0].track_count = 21;
        deluxe.media[0].tracks.clear();
        let stranger = mb_release("Ugly Is Beautiful", "Sean Kelly", OLIVER);
        let other = mb_release("Light House Rocket", "Oliver Tree", OLIVER);
        let found = [deluxe, stranger, other, album.clone()];
        assert_eq!(pick_release(&found, &ev).map(|r| r.media[0].track_count), Some(14));

        let files: Vec<(&Identity, Option<u32>)> = bases.iter().zip(OLIVER).map(|(b, (_, ms))| (b, Some(*ms + 400))).collect();
        let got = assign(&album, &files).expect("the album");
        assert!(got.iter().all(Option::is_some));
        let cash = got[2].as_ref().unwrap();
        assert_eq!((cash.metadata.title.as_str(), cash.metadata.track_no), ("Cash Machine", Some(3)));
        assert_eq!(cash.metadata.album.as_deref(), Some("Ugly Is Beautiful"));
        // Another edition: the lengths are off.
        let far: Vec<(&Identity, Option<u32>)> = bases.iter().zip(OLIVER).map(|(b, (_, ms))| (b, Some(*ms + 20_000))).collect();
        assert!(assign(&album, &far).is_none());
    }

    /// «8 способов как бросить...»: five places of the wished album, the
    /// fifth deleted by the user.
    fn wished(lib: &Library) -> (Release, Vec<TrackId>) {
        use nikgob_core::model::{ArtistCredit, Track};
        let credit = ArtistCredit::single("Пошлая Молли", None);
        let mut r = Release::new("8 способов как бросить...", credit.clone());
        r.total_tracks = Some(5);
        let release = lib.upsert_release(&r).unwrap();
        let songs = [
            ("Любимая песня твоей сестры", 188_000),
            ("Даже моя бэйби не знает", 234_336),
            ("Нон стоп", 213_786),
            ("Ханнамонтана", 215_824),
            ("Супермаркет", 214_256),
        ];
        let ids = songs
            .iter()
            .enumerate()
            .map(|(i, (title, ms))| {
                let mut t = Track::new(*title, credit.clone());
                t.release_id = Some(release.id);
                t.track_no = i as u32 + 1;
                t.duration_ms = Some(*ms);
                lib.upsert_track(&t).unwrap().id
            })
            .collect::<Vec<_>>();
        lib.mark_track_deleted(ids[4], 1).unwrap();
        (release, ids)
    }

    #[test]
    fn a_folder_of_a_bigger_edition_takes_the_wished_albums_places() {
        let lib = Library::open_in_memory().unwrap();
        let (release, ids) = wished(&lib);
        let targets = ImportTargets { release: Some(release.id), ..ImportTargets::default() };
        let r = Reference::load(&lib, &targets).unwrap();
        assert_eq!(r.unwanted.len(), 1);
        // The folder numbers the songs its own way (an intro first): the
        // album's «4» is its «5», and its «4» is as long as the album's «4».
        let folder = [
            ("Визитка", 1, 188_000),
            ("Даже моя бэйби не знает", 3, 234_336),
            ("Нон стоп", 4, 213_786),
            ("Ханнамонтана", 5, 215_824),
            ("Супермаркет", 6, 214_256),
            ("Lin Ansty", 11, 152_790),
        ];
        let bases: Vec<Identity> = folder.iter().map(|(t, no, _)| file(t, *no, Some("Пошлая Молли"), Some("8 способов как бросить дрочить"))).collect();
        let files: Vec<Option<(&Identity, Option<u32>)>> = bases.iter().zip(&folder).map(|(b, (_, _, ms))| Some((b, Some(*ms)))).collect();
        let got = r.match_files(&files);
        assert_eq!(
            got,
            [
                Some(OnRelease::Track(ids[0])),
                Some(OnRelease::Track(ids[1])),
                Some(OnRelease::Track(ids[2])),
                Some(OnRelease::Track(ids[3])),
                Some(OnRelease::Unwanted),
                None,
            ],
            "titles first, each place once, the deleted song not needed"
        );
        // The bonus song follows the album, not at its own «11».
        assert_eq!(r.after(), (1, 6));
        let bonus = r.identity(&bases[5], r.after());
        assert_eq!((bonus.track_no, bonus.album.as_deref(), bonus.release_id), (6, Some("8 способов как бросить..."), Some(release.id)));
    }

    /// «OMORI OST» in the library; a folder of another «Omori»'s cassette
    /// has none of its songs, the OST's own folder has them.
    #[test]
    fn a_folder_is_the_album_only_with_its_songs() {
        let lib = [
            ("Title", 1, 61_000),
            ("WHITE SPACE", 2, 104_000),
            ("Acrophobia", 14, 168_000),
            ("Space Road 1979", 19, 120_000),
        ];
        let tracks: Vec<AlbumTrack<'_>> = lib
            .iter()
            .map(|(t, no, ms)| AlbumTrack { title: t, disc_no: 1, track_no: *no, duration_ms: Some(*ms) })
            .collect();
        let cassette = [
            ("Omori - Embryogenesis - 01 Deeper Dive.flac", Some(221_000)),
            ("Omori - Embryogenesis - 02 I didn't realize how fast it all ceased to be.flac", Some(110_000)),
            ("Omori - Embryogenesis - 03 Struggle against chaos.flac", Some(300_000)),
        ];
        assert!(!folder_has_album(&tracks, &cassette));
        let ost = [
            ("01 - Title.flac", Some(61_000)),
            ("02 - WHITE SPACE.flac", Some(104_000)),
            ("OMORI - OMORI OST - 14 - Acrophobia.flac", Some(168_000)),
            ("19. Space Road 1979.flac", None),
        ];
        assert!(folder_has_album(&tracks, &ost));
        // The same names, the lengths of another edition: not it.
        let other: Vec<(&str, Option<u32>)> = ost.iter().map(|(n, _)| (*n, Some(400_000))).collect();
        assert!(!folder_has_album(&tracks, &other));
        // A single's file named «Artist - Album - 06 - Title».
        let one = [AlbumTrack { title: "прости я не знаю", disc_no: 1, track_no: 6, duration_ms: Some(135_000) }];
        assert!(folder_has_album(&one, &[("СЕРЕГА ПИРАТ - Вот мой альбом, сори - 06 - прости я не знаю.flac", Some(135_000))]));
    }

    #[test]
    fn two_files_never_claim_one_place() {
        let lib = Library::open_in_memory().unwrap();
        let (release, ids) = wished(&lib);
        let r = Reference::load(&lib, &ImportTargets { release: Some(release.id), ..ImportTargets::default() }).unwrap();
        let a = file("Любимая песня твоей сестры", 2, None, None);
        let b = file("Визитка", 1, None, None);
        let got = r.match_files(&[Some((&a, Some(188_000))), Some((&b, Some(188_000)))]);
        assert_eq!(got, [Some(OnRelease::Track(ids[0])), None]);
    }

    #[test]
    fn a_track_of_the_wished_album_keeps_its_place_whatever_its_file_says() {
        let lib = Library::open_in_memory().unwrap();
        let (release, ids) = wished(&lib);
        let targets = ImportTargets { release: Some(release.id), ..ImportTargets::default() };
        let track = lib.get_track(ids[1]).unwrap().unwrap();
        // Found by its MusicBrainz id: the file says «3» of another album.
        let base = file("Даже моя бэйби не знает", 3, Some("Пошлая Молли"), Some("8 способов как бросить дрочить"));
        let kept = keep_place(&lib, &targets, &track, &base).unwrap();
        assert_eq!((kept.track_no, kept.disc_no), (2, 1));
        assert_eq!((kept.album.as_deref(), kept.release_id, kept.track_total), (Some("8 способов как бросить..."), Some(release.id), Some(5)));
        assert_eq!(keep_place(&lib, &targets, &track, &kept), None, "nothing more to change");
        // Another album's download: the file is left as it is.
        let other = ImportTargets { release: Some(nikgob_core::model::ReleaseId::new()), ..ImportTargets::default() };
        assert_eq!(keep_place(&lib, &other, &track, &base), None);
    }

    /// A better copy of an album: each downloaded file finds its track by
    /// its tags, else by its name, the lengths agreeing; an instrumental of
    /// a song, a bonus song and a stranger's file find none, and two files
    /// never take one track.
    #[test]
    fn the_files_of_a_better_copy_find_their_tracks() {
        let track = |title, no, secs: u32| AlbumTrack { title, disc_no: 1, track_no: no, duration_ms: Some(secs * 1000) };
        let tracks = [track("Интро", 1, 60), track("Сахарный", 2, 200), track("Последний", 3, 240)];
        let tagged = |title: &str, no: u32| TagSet { title: Some(title.into()), track_no: Some(no), ..TagSet::default() };
        let (last, intro, inst, bonus, stranger, twice) = (
            tagged("Последний", 3),
            tagged("Интро", 1),
            tagged("Сахарный (Instrumental)", 4),
            tagged("Бонус", 5),
            tagged("Совсем другая песня", 2),
            tagged("Последний", 3),
        );
        let untagged = TagSet::default();
        let files = [
            (&last, Path::new("/dl/Album/03 - Последний.flac"), Some(240_400)),
            (&inst, Path::new("/dl/Album/04 - Сахарный (Instrumental).flac"), Some(200_000)),
            (&intro, Path::new("/dl/Album/01 - Интро.flac"), Some(60_000)),
            (&bonus, Path::new("/dl/Album/05 - Бонус.flac"), Some(180_000)),
            (&untagged, Path::new("/dl/Album/02 - Сахарный.flac"), Some(199_000)),
            (&stranger, Path::new("/dl/Other/02 - Совсем другая песня.flac"), Some(320_000)),
            (&twice, Path::new("/dl/Album/CD2/03 - Последний.flac"), Some(240_000)),
        ];
        assert_eq!(album_files(&tracks, &files), [Some(2), Some(4), Some(0)]);
        // Lengths that disagree: not the track, whatever the title.
        let short = [(&last, Path::new("/dl/03 - Последний.flac"), Some(90_000))];
        assert_eq!(album_files(&tracks, &short), [None, None, None]);
        assert_eq!(album_files(&tracks, &[]), [None, None, None]);
    }
}
