//! Who is this file?
//!
//! [`Identity`] is the metadata the pipeline works with: first what the tags
//! say, then, when the matcher is sure (or the user picks an option), the
//! MusicBrainz / Deezer candidate. [`lookup`] runs the online part and never
//! fails: any network problem just means "tags only".

use std::path::Path;
use std::time::Duration;

use nikgob_core::model::ReleaseType;
use nikgob_metadata::matcher::{
    Candidate, Confidence, LocalEvidence, MatchCandidate, MatchResult, Source as MatchSource,
    candidate_from_deezer, candidates_from_recording, match_candidates,
};
use nikgob_metadata::{AcoustIdResult, Metadata};

use crate::analysis::Fingerprint;
use crate::config::ImportConfig;
use crate::events::ConfirmOption;
use crate::layout::is_various_artists;
use crate::tags::{FileInfo, TagSet};

/// Candidates below this score are not worth bothering the user with.
pub const CONFIRM_MIN_SCORE: f64 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentifiedBy {
    Tags,
    MusicBrainz,
    Deezer,
}

/// The metadata of one track as the library will store it.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub title: String,
    /// Credited artists in order (never empty).
    pub artists: Vec<String>,
    /// Credit line as displayed, join phrases included.
    pub artist_line: String,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub date: Option<String>,
    /// 0 = unknown.
    pub track_no: u32,
    pub track_total: Option<u32>,
    pub disc_no: u32,
    pub disc_total: Option<u32>,
    pub release_type: ReleaseType,
    pub compilation: bool,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub isrc: Option<String>,
    pub acoustid_id: Option<String>,
    pub genre: Option<String>,
    pub cover_url: Option<String>,
    pub identified_by: IdentifiedBy,
    /// The library release the file is known to belong to (the release a
    /// wishlist download was for): it lands there whatever the title says.
    pub release_id: Option<nikgob_core::model::ReleaseId>,
}

/// Splits `"A feat. B; C"` into `[("", "A"), (" feat. ", "B"), ("; ", "C")]`
/// and `"A, B"` into `[("", "A"), (", ", "B")]` (see
/// [`nikgob_core::credit::split_credit`] for the rules).
pub fn parse_credit(line: &str) -> Vec<(String, String)> {
    nikgob_core::credit::split_credit(line)
}

/// Aligns individual `names` with the credit `line` to recover join phrases.
pub fn credit_parts(line: &str, names: &[String]) -> Vec<(String, String)> {
    if names.is_empty() {
        return parse_credit(line);
    }
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for (i, name) in names.iter().enumerate() {
        let (join, advance) = match line[cursor..].find(name.as_str()) {
            Some(pos) => {
                let phrase = if i == 0 {
                    String::new()
                } else {
                    line[cursor..cursor + pos].to_owned()
                };
                (phrase, pos + name.len())
            }
            None => (if i == 0 { String::new() } else { ", ".to_owned() }, 0),
        };
        cursor += advance;
        out.push((join, name.clone()));
    }
    out
}

fn guess_release_type(tags: &TagSet, title: &str) -> ReleaseType {
    match tags.album.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        None => ReleaseType::Single,
        Some(album) => {
            if tags.compilation
                || tags
                    .album_artist
                    .as_deref()
                    .is_some_and(is_various_artists)
            {
                ReleaseType::Compilation
            } else if tags.track_total.is_some_and(|t| t <= 1)
                && album.eq_ignore_ascii_case(title.trim())
            {
                ReleaseType::Single
            } else {
                ReleaseType::Album
            }
        }
    }
}

/// What the tags (and, failing that, the file name) say.
pub fn identity_from_tags(tags: &TagSet, path: &Path) -> Identity {
    let parsed = nikgob_metadata::text::parse_filename(&path.to_string_lossy());
    let title = tags
        .title
        .clone()
        .or(parsed.title)
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".to_owned())
        });
    let artist_line = tags
        .artist
        .clone()
        .or_else(|| tags.album_artist.clone())
        .or(parsed.artist)
        .unwrap_or_default();
    let artists: Vec<String> = if tags.artists.len() > 1 {
        tags.artists.clone()
    } else {
        parse_credit(&artist_line)
            .into_iter()
            .map(|(_, n)| n)
            .collect()
    };
    let artists = if artists.is_empty() {
        vec!["Unknown Artist".to_owned()]
    } else {
        artists
    };
    let artist_line = if artist_line.trim().is_empty() {
        artists.join(", ")
    } else {
        artist_line
    };
    let release_type = guess_release_type(tags, &title);
    Identity {
        compilation: release_type == ReleaseType::Compilation,
        release_type,
        title,
        artists,
        artist_line,
        album_artist: tags.album_artist.clone(),
        album: tags.album.clone(),
        year: tags.year,
        date: tags.date.clone(),
        track_no: tags.track_no.or(parsed.track_no).unwrap_or(0),
        track_total: tags.track_total,
        disc_no: tags.disc_no.unwrap_or(1).max(1),
        disc_total: tags.disc_total,
        recording_mbid: tags.recording_mbid.clone(),
        release_mbid: tags.release_mbid.clone(),
        release_group_mbid: tags.release_group_mbid.clone(),
        artist_mbid: tags.artist_mbid.clone(),
        isrc: tags.isrc.clone(),
        acoustid_id: tags.acoustid_id.clone(),
        genre: tags.genre.clone(),
        cover_url: None,
        identified_by: IdentifiedBy::Tags,
        release_id: None,
    }
}

/// The identity of the library track `track` (its title, credit, album and
/// numbers), for a file that is known to be that track whatever its tags
/// say (attached by the user, saved from the track's stream). What the
/// library does not know stays as in `base`.
pub fn identity_of_track(
    lib: &nikgob_library::Library,
    track: nikgob_core::model::TrackId,
    base: &Identity,
) -> Option<Identity> {
    let t = lib.get_track(track).ok()??;
    let release = t.release_id.and_then(|r| lib.get_release(r).ok().flatten());
    let mut id = base.clone();
    id.title = t.title.clone();
    let names: Vec<String> = t.artist_credit.0.iter().map(|p| p.name.clone()).collect();
    if !names.is_empty() {
        id.artists = names;
        id.artist_line = t.artist_credit.display();
    }
    id.track_no = t.track_no;
    id.disc_no = t.disc_no.max(1);
    id.isrc = t.isrc.clone().or(id.isrc);
    id.recording_mbid = t.recording_mbid.clone().or(id.recording_mbid);
    id.release_id = t.release_id;
    match release {
        Some(r) => {
            id.release_mbid = r.mbid.clone().or(id.release_mbid);
            id.release_group_mbid = r.release_group_mbid.clone().or(id.release_group_mbid);
            id.album = Some(r.title.clone());
            id.album_artist = Some(r.artist_credit.display()).filter(|a| !a.is_empty());
            id.year = r.year.or(id.year);
            id.release_type = r.release_type;
            id.compilation = r.release_type == ReleaseType::Compilation;
            id.track_total = r.total_tracks.or(id.track_total);
        }
        None => {
            id.album = None;
            id.release_type = ReleaseType::Single;
            id.compilation = false;
        }
    }
    Some(id)
}

fn year_of(date: &str) -> Option<i32> {
    let y: String = date.chars().take(4).collect();
    (y.len() == 4).then(|| y.parse().ok()).flatten().filter(|y| *y > 0)
}

fn release_type_of(meta: &nikgob_metadata::TrackMetadata) -> ReleaseType {
    if meta
        .release_secondary_types
        .iter()
        .any(|t| t.eq_ignore_ascii_case("compilation"))
    {
        return ReleaseType::Compilation;
    }
    match meta.release_type.as_deref().map(str::to_lowercase).as_deref() {
        Some("single") => ReleaseType::Single,
        Some("ep") => ReleaseType::Ep,
        Some("album") => ReleaseType::Album,
        Some(_) => ReleaseType::Other,
        None => {
            if meta.album.is_some() {
                ReleaseType::Album
            } else {
                ReleaseType::Single
            }
        }
    }
}

/// The identity after the user (or the matcher) accepted `cand`; fields the
/// candidate does not know stay as the tags had them.
pub fn apply_candidate(base: &Identity, cand: &MatchCandidate) -> Identity {
    let m = &cand.metadata;
    let mut id = base.clone();
    if !m.title.trim().is_empty() {
        id.title = m.title.trim().to_owned();
    }
    if !m.artist.trim().is_empty() {
        id.artist_line = m.artist.trim().to_owned();
        id.artists = if m.artists.is_empty() {
            parse_credit(&id.artist_line)
                .into_iter()
                .map(|(_, n)| n)
                .collect()
        } else {
            m.artists.clone()
        };
    }
    if let Some(a) = &m.album {
        id.album = Some(a.clone());
        id.release_type = release_type_of(m);
        id.compilation = id.release_type == ReleaseType::Compilation;
    }
    if let Some(a) = &m.album_artist {
        id.album_artist = Some(a.clone());
    }
    if let Some(d) = &m.date {
        id.date = Some(d.clone());
        id.year = year_of(d).or(id.year);
    }
    if let Some(n) = m.track_no.filter(|n| *n > 0) {
        id.track_no = n;
    }
    if let Some(n) = m.disc_no.filter(|n| *n > 0) {
        id.disc_no = n;
    }
    if let Some(isrc) = m.isrcs.first() {
        id.isrc = Some(isrc.clone());
    }
    let ids = &cand.ids;
    id.recording_mbid = ids.recording_mbid.clone().or(id.recording_mbid);
    id.release_mbid = ids.release_mbid.clone().or(id.release_mbid);
    id.release_group_mbid = ids.release_group_mbid.clone().or(id.release_group_mbid);
    id.artist_mbid = ids.artist_mbid.clone().or(id.artist_mbid);
    id.cover_url = m.cover_url.clone().or(id.cover_url);
    id.identified_by = match cand.source {
        MatchSource::MusicBrainz => IdentifiedBy::MusicBrainz,
        MatchSource::Deezer => IdentifiedBy::Deezer,
    };
    id
}

pub fn confirm_option(c: &MatchCandidate) -> ConfirmOption {
    ConfirmOption {
        title: c.metadata.title.clone(),
        artist: c.metadata.artist.clone(),
        album: c.metadata.album.clone(),
        year: c.metadata.date.as_deref().and_then(year_of),
        track_no: c.metadata.track_no,
        duration_ms: c.metadata.duration.map(|d| d.as_millis() as u32),
        score: c.score,
        source: match c.source {
            MatchSource::MusicBrainz => "musicbrainz",
            MatchSource::Deezer => "deezer",
        },
        recording_mbid: c.ids.recording_mbid.clone(),
        release_mbid: c.ids.release_mbid.clone(),
    }
}

/// What the file tells the matcher.
pub fn evidence(tags: &TagSet, info: &FileInfo, path: &Path) -> LocalEvidence {
    LocalEvidence {
        title: tags.title.clone(),
        artist: tags.artist.clone(),
        album_artist: tags.album_artist.clone(),
        album: tags.album.clone(),
        duration: (!info.duration.is_zero()).then_some(info.duration),
        track_no: tags.track_no,
        track_total: tags.track_total,
        disc_no: tags.disc_no,
        filename: path.file_name().map(|n| n.to_string_lossy().into_owned()),
        acoustid: Vec::new(),
    }
}

/// Outcome of the online identification.
#[derive(Debug)]
pub enum Lookup {
    /// Nothing usable online (offline, no match, or no keys): import with tags.
    None,
    /// The matcher is sure.
    Sure(Box<MatchCandidate>),
    /// Needs the user: up to three options, best first.
    Unsure(Vec<MatchCandidate>),
}

async fn timed<T>(
    cfg: &ImportConfig,
    what: &str,
    fut: impl std::future::Future<Output = nikgob_metadata::Result<T>>,
) -> Option<T> {
    match tokio::time::timeout(cfg.online_timeout, fut).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            log::debug!("{what}: {e}");
            None
        }
        Err(_) => {
            log::debug!("{what}: timed out");
            None
        }
    }
}

/// Folded single names of credit lines (`"A feat. B"`, `"A & B"` and
/// `"A, B"` give `a` and `b`; the line itself counts too, for a band named
/// `"Simon & Garfunkel"`).
pub fn artist_names<S: AsRef<str>>(lines: &[S]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        let line = line.as_ref();
        let mut push = |n: &str| {
            let k = nikgob_metadata::text::artist_key(n);
            if !k.is_empty() && !out.contains(&k) && !is_various_artists(n) {
                out.push(k);
            }
        };
        push(line);
        for (_, name) in parse_credit(line) {
            push(&name);
            if let Some(pair) = nikgob_core::credit::split_pair(&name) {
                for (_, n) in pair {
                    push(&n);
                }
            }
        }
    }
    out
}

/// Whether two lists of credit lines name one artist in common (see
/// [`artist_names`]). Nothing to compare (an empty side) is no veto.
pub fn shares_artist<A: AsRef<str>, B: AsRef<str>>(a: &[A], b: &[B]) -> bool {
    let (a, b) = (artist_names(a), artist_names(b));
    a.is_empty() || b.is_empty() || a.iter().any(|n| b.contains(n))
}

/// Drops candidates none of whose artists is one the file, its folder or
/// its torrent names (`artists`): a title alone is no match («Cash
/// Machine» by Oliver Tree is not DRAM's). A vetoed best candidate takes
/// the verdict «sure» with it.
pub fn veto(mut result: MatchResult, artists: &[String]) -> MatchResult {
    if artist_names(artists).is_empty() {
        return result;
    }
    let fits = |c: &MatchCandidate| {
        let mut lines = vec![c.metadata.artist.clone()];
        lines.extend(c.metadata.artists.iter().cloned());
        lines.extend(c.metadata.album_artist.iter().cloned());
        shares_artist(&lines, artists)
    };
    let best_ok = result.ranked.first().is_some_and(fits);
    result.ranked.retain(fits);
    result.options.retain(fits);
    if !best_ok {
        result.confidence = Confidence::Unsure;
    }
    result
}

/// AcoustID + MusicBrainz, with Deezer as the fallback. Never errors.
/// Candidates by other artists than `artists` are vetoed ([`veto`]).
pub async fn lookup(
    meta: &Metadata,
    cfg: &ImportConfig,
    mut ev: LocalEvidence,
    fingerprint: Option<&Fingerprint>,
    artists: &[String],
) -> Lookup {
    let duration = ev
        .duration
        .or_else(|| fingerprint.map(|f| f.duration))
        .unwrap_or(Duration::ZERO);

    if let (Some(acoustid), Some(fp)) = (&meta.acoustid, fingerprint)
        && let Some(results) = timed(cfg, "acoustid", acoustid.lookup(&fp.encoded, duration)).await
    {
        ev.acoustid = results;
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for with_release in [true, false] {
        let Some(query) = ev.recording_query(with_release) else {
            break;
        };
        if let Some(recs) = timed(cfg, "musicbrainz search", meta.musicbrainz.search_recordings(&query)).await {
            for r in &recs {
                candidates.extend(candidates_from_recording(r));
            }
        }
        if !candidates.is_empty() {
            break;
        }
    }
    // Recordings the fingerprint points at, if the search did not surface them.
    let known = |c: &[Candidate], id: &str| {
        c.iter()
            .any(|x| x.ids.recording_mbid.as_deref() == Some(id))
    };
    let mut wanted: Vec<&AcoustIdResult> = ev.acoustid.iter().filter(|r| r.score >= 0.7).collect();
    wanted.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut looked_up = 0;
    let ids: Vec<String> = wanted
        .iter()
        .flat_map(|r| r.recordings.iter().map(|rec| rec.id.clone()))
        .collect();
    for id in ids {
        if looked_up >= 3 {
            break;
        }
        if known(&candidates, &id) {
            continue;
        }
        looked_up += 1;
        if let Some(rec) = timed(cfg, "musicbrainz lookup", meta.musicbrainz.lookup_recording(&id)).await {
            candidates.extend(candidates_from_recording(&rec));
        }
    }

    let mut result = match_candidates(&ev, candidates.clone());
    if result.confidence != Confidence::Sure
        && let Some(title) = ev.resolved_title()
    {
        let artist = ev.resolved_artist();
        let found = timed(
            cfg,
            "deezer search",
            meta.deezer.search_track(artist.as_deref(), &title, 5),
        )
        .await
        .unwrap_or_default();
        if !found.is_empty() {
            candidates.extend(found.iter().map(candidate_from_deezer));
            result = match_candidates(&ev, candidates);
        }
    }
    decide_lookup(veto(result, artists))
}

/// Sure / worth asking about / nothing.
pub fn decide_lookup(result: MatchResult) -> Lookup {
    let Some(best) = result.ranked.first() else {
        return Lookup::None;
    };
    if result.confidence == Confidence::Sure {
        return Lookup::Sure(Box::new(best.clone()));
    }
    if best.score < CONFIRM_MIN_SCORE {
        return Lookup::None;
    }
    Lookup::Unsure(result.options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_metadata::matcher::{CandidateIds, TrackMetadata};

    #[test]
    fn credit_lines_are_split_on_feat_markers() {
        let p = parse_credit("Кино feat. Земфира; Bi-2");
        assert_eq!(
            p,
            vec![
                (String::new(), "Кино".to_owned()),
                (" feat. ".to_owned(), "Земфира".to_owned()),
                ("; ".to_owned(), "Bi-2".to_owned()),
            ]
        );
        assert_eq!(parse_credit("Simon & Garfunkel").len(), 1);
        assert_eq!(parse_credit("A Ft. B")[1].1, "B");
        assert!(parse_credit("  ").is_empty());
    }

    #[test]
    fn credit_parts_recover_join_phrases() {
        let names = vec!["Daft Punk".to_owned(), "Pharrell".to_owned(), "Nile".to_owned()];
        let parts = credit_parts("Daft Punk feat. Pharrell & Nile", &names);
        assert_eq!(parts[0].0, "");
        assert_eq!(parts[1].0, " feat. ");
        assert_eq!(parts[2].0, " & ");
        let mismatched = credit_parts("Someone", &names);
        assert_eq!(mismatched.len(), 3);
    }

    #[test]
    fn identity_from_tags_uses_filename_when_tags_are_empty() {
        let id = identity_from_tags(&TagSet::default(), Path::new("C:/in/03 - Кино - Звезда.flac"));
        assert_eq!(id.track_no, 3);
        assert!(id.title.contains("Звезда"), "{id:?}");
        assert_eq!(id.identified_by, IdentifiedBy::Tags);
        assert_eq!(id.release_type, ReleaseType::Single, "no album means a single");

        let tags = TagSet {
            title: Some("T".into()),
            artist: Some("A feat. B".into()),
            album: Some("Alb".into()),
            album_artist: Some("Various Artists".into()),
            track_no: Some(2),
            disc_no: Some(2),
            ..TagSet::default()
        };
        let id = identity_from_tags(&tags, Path::new("x.mp3"));
        assert_eq!(id.artists, vec!["A", "B"]);
        assert_eq!(id.release_type, ReleaseType::Compilation);
        assert!(id.compilation);
        assert_eq!(id.disc_no, 2);
    }

    #[test]
    fn candidate_overrides_tags_but_keeps_unknowns() {
        let base = identity_from_tags(
            &TagSet {
                title: Some("tag title".into()),
                artist: Some("tag artist".into()),
                genre: Some("Rock".into()),
                ..TagSet::default()
            },
            Path::new("a.flac"),
        );
        let cand = MatchCandidate {
            score: 0.95,
            source: MatchSource::MusicBrainz,
            ids: CandidateIds {
                recording_mbid: Some("rec".into()),
                release_mbid: Some("rel".into()),
                ..Default::default()
            },
            metadata: TrackMetadata {
                title: "Real Title".into(),
                artist: "Real A feat. Real B".into(),
                artists: vec!["Real A".into(), "Real B".into()],
                album: Some("Real Album".into()),
                date: Some("2019-05-03".into()),
                track_no: Some(4),
                release_type: Some("EP".into()),
                isrcs: vec!["ISRC1".into()],
                ..Default::default()
            },
            breakdown: Default::default(),
        };
        let id = apply_candidate(&base, &cand);
        assert_eq!(id.title, "Real Title");
        assert_eq!(id.artists, vec!["Real A", "Real B"]);
        assert_eq!(id.year, Some(2019));
        assert_eq!(id.track_no, 4);
        assert_eq!(id.release_type, ReleaseType::Ep);
        assert_eq!(id.recording_mbid.as_deref(), Some("rec"));
        assert_eq!(id.isrc.as_deref(), Some("ISRC1"));
        assert_eq!(id.genre.as_deref(), Some("Rock"), "unknown fields stay");
        assert_eq!(id.identified_by, IdentifiedBy::MusicBrainz);
        let opt = confirm_option(&cand);
        assert_eq!(opt.year, Some(2019));
        assert_eq!(opt.source, "musicbrainz");
    }

    fn cand(title: &str, artist: &str, score: f64) -> MatchCandidate {
        MatchCandidate {
            score,
            source: MatchSource::MusicBrainz,
            ids: CandidateIds::default(),
            metadata: TrackMetadata {
                title: title.into(),
                artist: artist.into(),
                artists: vec![artist.into()],
                ..Default::default()
            },
            breakdown: Default::default(),
        }
    }

    #[test]
    fn candidates_by_strangers_are_vetoed() {
        let all = vec![cand("Cash Machine", "DRAM", 0.9), cand("Cash Machine", "Oliver Tree", 0.7)];
        let result = MatchResult { confidence: Confidence::Sure, ranked: all.clone(), options: all };
        let kept = veto(result.clone(), &["Oliver Tree".into()]);
        assert_eq!(kept.ranked.len(), 1);
        assert_eq!(kept.options[0].metadata.artist, "Oliver Tree");
        assert_eq!(kept.confidence, Confidence::Unsure, "the sure one was a stranger");
        let none = veto(result.clone(), &["Oliver Tree feat. Little Ricky ZR3".into(), "Someone".into()]);
        assert_eq!(none.ranked.len(), 1, "a guest line still names the artist");
        assert!(matches!(decide_lookup(veto(result.clone(), &["Kino".into()])), Lookup::None));
        assert_eq!(veto(result, &[]).ranked.len(), 2, "nothing to compare: no veto");
        assert!(shares_artist(&["Kevin Penkin & Raj Ramayya"], &["Raj Ramayya"]));
        assert!(!shares_artist(&["Sean Kelly"], &["Oliver Tree"]));
    }

    #[test]
    fn lookup_decisions() {
        let none = decide_lookup(MatchResult {
            confidence: Confidence::Unsure,
            ranked: vec![],
            options: vec![],
        });
        assert!(matches!(none, Lookup::None));
    }
}
