//! A downloaded folder is an album, not a pile of files (docs/stage4.md
//! §12.4).
//!
//! The files of one folder (one disc) are identified together:
//! * [`Evidence`]: their tags, the folder's name and the torrent's name vote
//!   for one artist and one album («Oliver Tree - Ugly Is Beautiful (2020)
//!   [FLAC]» names both when the tags say nothing);
//! * [`Reference`]: a torrent fetched for a «Хочу скачать» item lands on the
//!   item's release, whose tracks (stream tracks with their numbers, titles
//!   and lengths) the files attach to; MusicBrainz is not asked;
//! * otherwise MusicBrainz is asked for the release as a whole
//!   ([`lookup_album`]: by artist and title, then the track count and the
//!   lengths pick the edition), and each file takes its track of that
//!   release ([`assign`]). Files are looked up one by one only in a folder
//!   of many artists (a compilation) or a loose file.

use std::path::Path;

use nikgob_core::model::{Release, ReleaseType, TrackId};
use nikgob_library::{Library, TrackRow};
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

/// The «Хочу скачать» release a download was for, with its tracks.
#[derive(Clone, Debug)]
pub(crate) struct Reference {
    pub release: Release,
    pub tracks: Vec<TrackRow>,
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
/// lengths within the slack when both are known.
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
    let fits = sim >= TITLE || (numbered && (sim >= NUMBERED_TITLE || close == Some(true)));
    (fits && close != Some(false)).then_some(sim + if numbered { 1.0 } else { 0.0 })
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
        Some(Self { release, tracks })
    }

    /// The track of the release a file is.
    pub fn track_of(&self, base: &Identity, duration_ms: Option<u32>) -> Option<TrackId> {
        self.tracks
            .iter()
            .filter_map(|t| Some((file_is_track(base, duration_ms, t.into())?, t.id)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
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
    /// album fields, its own title and number.
    pub fn identity(&self, base: &Identity) -> Identity {
        let r = &self.release;
        let mut id = base.clone();
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
        id.release_mbid = r.mbid.clone().or(id.release_mbid);
        id.release_group_mbid = r.release_group_mbid.clone().or(id.release_group_mbid);
        id.track_total = r.total_tracks.or(id.track_total);
        id.release_id = Some(r.id);
        id
    }
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
}
