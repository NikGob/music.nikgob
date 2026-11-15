//! Pure track matcher: local evidence + online candidates -> ranked candidates and a
//! Sure/Unsure decision. No I/O.
//!
//! Scoring is a weighted mean of the signals that are available (title, artist,
//! album, duration, track number, AcoustID), so missing evidence neither helps nor
//! hurts; multiplicative penalties then handle variant mismatches (live/remix/...),
//! durations further than 10 s apart and undesirable releases (live albums,
//! compilations, bootlegs). A strong fingerprint hit (AcoustID score >= 0.9) puts a
//! floor under the candidate's score, so wrong tags cannot hide a confirmed recording.
//!
//! The decision is [`Confidence::Sure`] when the best score is at least 0.85 and leads
//! the best *different* recording by at least 0.1. Different releases of the same
//! recording (same MBID, ISRC, or same title/artist/duration) are not competitors:
//! they would otherwise always produce a near-zero margin.

use std::time::Duration;

use crate::acoustid::AcoustIdResult;
use crate::deezer::DeezerTrack;
use crate::musicbrainz::{Recording, RecordingQuery, Release, artist_credit_string};
use crate::text::{
    Variant, album_key, artist_key, parse_filename, search_artist, search_title, similarity,
    title_key,
};

pub const SURE_MIN_SCORE: f64 = 0.85;
pub const SURE_MIN_MARGIN: f64 = 0.10;
/// How many options go to the "confirm" queue.
pub const MAX_OPTIONS: usize = 3;
/// AcoustID scores below this are ignored as noise.
const ACOUSTID_MIN_SCORE: f64 = 0.5;
/// AcoustID score that counts as a confirmed recording.
const ACOUSTID_STRONG: f64 = 0.9;
const ACOUSTID_FLOOR_FACTOR: f64 = 0.88;

const DURATION_STRONG: f64 = 3.0;
const DURATION_WEAK: f64 = 10.0;

const W_TITLE: f64 = 0.35;
const W_ARTIST: f64 = 0.30;
const W_ALBUM: f64 = 0.10;
const W_DURATION: f64 = 0.15;
const W_TRACK: f64 = 0.03;
const W_ACOUSTID: f64 = 0.30;
/// Scores this close (quantized, see [`match_candidates`]) are a tie
/// between an album and a single or EP of the same song.
const ALBUM_OVER_SINGLE: i64 = 20_000;

// ---------------------------------------------------------------- inputs

/// What we know about a local file before identification.
#[derive(Debug, Clone, Default)]
pub struct LocalEvidence {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub track_no: Option<u32>,
    /// The tracks of the file's release, as its tags say.
    pub track_total: Option<u32>,
    pub disc_no: Option<u32>,
    /// File name or path; used to fill gaps in the tags.
    pub filename: Option<String>,
    /// Results of the AcoustID fingerprint lookup, if it was run.
    pub acoustid: Vec<AcoustIdResult>,
}

impl LocalEvidence {
    /// Tag title, else the title guessed from the file name.
    pub fn resolved_title(&self) -> Option<String> {
        non_blank(self.title.as_deref())
            .map(str::to_owned)
            .or_else(|| self.parsed_filename().title)
    }

    /// Tag artist, else album artist, else the artist guessed from the file name.
    pub fn resolved_artist(&self) -> Option<String> {
        non_blank(self.artist.as_deref())
            .or_else(|| non_blank(self.album_artist.as_deref()))
            .map(str::to_owned)
            .or_else(|| self.parsed_filename().artist)
    }

    pub fn resolved_track_no(&self) -> Option<u32> {
        self.track_no.or_else(|| self.parsed_filename().track_no)
    }

    fn parsed_filename(&self) -> crate::text::ParsedFilename {
        self.filename
            .as_deref()
            .map(parse_filename)
            .unwrap_or_default()
    }

    /// MusicBrainz search for this file. Noise like "(Remastered 2011)" and
    /// "feat. X" is cut from the title/artist so the strict AND query still hits.
    pub fn recording_query(&self, include_release: bool) -> Option<RecordingQuery> {
        let title = self.resolved_title()?;
        let mut q = RecordingQuery::new(search_title(&title));
        if let Some(artist) = self.resolved_artist() {
            q = q.artist(search_artist(&artist));
        }
        if include_release && let Some(album) = non_blank(self.album.as_deref()) {
            q = q.release(search_title(album));
        }
        if let Some(d) = self.duration {
            q = q.duration(d);
        }
        Some(q)
    }
}

fn non_blank(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    MusicBrainz,
    Deezer,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateIds {
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub deezer_track_id: Option<u64>,
    pub deezer_album_id: Option<u64>,
}

/// Source-independent metadata in display form (trimmed, original script/case).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackMetadata {
    pub title: String,
    /// Full artist credit line (`"A feat. B"`).
    pub artist: String,
    /// Individual credited artists.
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration: Option<Duration>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    /// Possibly partial ISO date.
    pub date: Option<String>,
    pub isrcs: Vec<String>,
    /// MusicBrainz release status (`Official`, `Bootleg`, ...).
    pub release_status: Option<String>,
    /// `Album`, `Single`, `EP`, ...
    pub release_type: Option<String>,
    /// `Live`, `Compilation`, `Remix`, ...
    pub release_secondary_types: Vec<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    /// Recording disambiguation (`"live, 2011-05-01: Wembley"`).
    pub disambiguation: Option<String>,
    pub cover_url: Option<String>,
}

/// One possible identity for the file: a recording on a particular release.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub source: Source,
    pub ids: CandidateIds,
    pub metadata: TrackMetadata,
}

fn trimmed(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn credit_names(credits: &[crate::musicbrainz::ArtistCredit]) -> Vec<String> {
    credits
        .iter()
        .map(|c| {
            if c.artist.name.is_empty() {
                c.name.clone()
            } else {
                c.artist.name.clone()
            }
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// One candidate per release the recording appears on (or one without release data).
pub fn candidates_from_recording(rec: &Recording) -> Vec<Candidate> {
    let build = |release: Option<&Release>| -> Candidate {
        let mut meta = TrackMetadata {
            title: rec.title.trim().to_owned(),
            artist: rec.artist_credit_string(),
            artists: credit_names(&rec.artist_credit),
            duration: rec.duration(),
            date: rec.first_release_date.clone(),
            isrcs: rec.isrcs.clone(),
            disambiguation: trimmed(&rec.disambiguation),
            ..TrackMetadata::default()
        };
        let mut ids = CandidateIds {
            recording_mbid: Some(rec.id.clone()),
            artist_mbid: rec.artist_credit.first().map(|c| c.artist.id.clone()),
            ..CandidateIds::default()
        };
        if let Some(rel) = release {
            apply_release(&mut meta, &mut ids, rel);
            if let Some((disc, no)) = position_in_release(rec, rel) {
                meta.disc_no = Some(disc);
                meta.track_no = Some(no);
            }
        }
        Candidate {
            source: Source::MusicBrainz,
            ids,
            metadata: meta,
        }
    };
    if rec.releases.is_empty() {
        vec![build(None)]
    } else {
        rec.releases.iter().map(|r| build(Some(r))).collect()
    }
}

fn apply_release(meta: &mut TrackMetadata, ids: &mut CandidateIds, rel: &Release) {
    meta.album = trimmed(&rel.title);
    meta.album_artist = trimmed(&rel.artist_credit_string());
    if let Some(d) = &rel.date {
        meta.date = Some(d.clone());
    }
    meta.release_status = rel.status.clone();
    meta.country = rel.country.clone();
    meta.label = rel.label_name().map(str::to_owned);
    ids.release_mbid = Some(rel.id.clone());
    if let Some(rg) = &rel.release_group {
        ids.release_group_mbid = Some(rg.id.clone());
        meta.release_type = rg.primary_type.clone();
        meta.release_secondary_types = rg.secondary_types.clone();
    }
}

/// Disc and track number of the recording on the release, when the release lists tracks.
fn position_in_release(rec: &Recording, rel: &Release) -> Option<(u32, u32)> {
    let wanted = title_key(&rec.title).base;
    for (i, medium) in rel.media.iter().enumerate() {
        let disc = medium.position.unwrap_or(i as u32 + 1);
        for (j, track) in medium.tracks.iter().enumerate() {
            let by_id = track
                .recording
                .as_ref()
                .is_some_and(|r| !r.id.is_empty() && r.id == rec.id);
            let by_title = track.recording.is_none() && title_key(&track.title).base == wanted;
            if by_id || by_title {
                let no = if track.position > 0 {
                    track.position
                } else {
                    track.number.trim().parse().unwrap_or(j as u32 + 1)
                };
                return Some((disc, no));
            }
        }
    }
    None
}

/// One candidate per track of a fully looked-up release (`lookup_release`).
pub fn candidates_from_release(rel: &Release) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (i, medium) in rel.media.iter().enumerate() {
        let disc = medium.position.unwrap_or(i as u32 + 1);
        for (j, track) in medium.tracks.iter().enumerate() {
            let credits = [
                Some(&track.artist_credit),
                track.recording.as_ref().map(|r| &r.artist_credit),
                Some(&rel.artist_credit),
            ]
            .into_iter()
            .flatten()
            .find(|c| !c.is_empty())
            .unwrap_or(&rel.artist_credit);
            let rec = track.recording.as_deref();
            let mut meta = TrackMetadata {
                title: track.title.trim().to_owned(),
                artist: artist_credit_string(credits),
                artists: credit_names(credits),
                duration: track
                    .length
                    .or_else(|| rec.and_then(|r| r.length))
                    .map(|ms| Duration::from_millis(u64::from(ms))),
                track_no: Some(if track.position > 0 {
                    track.position
                } else {
                    j as u32 + 1
                }),
                disc_no: Some(disc),
                isrcs: rec.map(|r| r.isrcs.clone()).unwrap_or_default(),
                disambiguation: rec.and_then(|r| trimmed(&r.disambiguation)),
                ..TrackMetadata::default()
            };
            let mut ids = CandidateIds {
                recording_mbid: rec.map(|r| r.id.clone()).filter(|id| !id.is_empty()),
                artist_mbid: credits.first().map(|c| c.artist.id.clone()),
                ..CandidateIds::default()
            };
            apply_release(&mut meta, &mut ids, rel);
            out.push(Candidate {
                source: Source::MusicBrainz,
                ids,
                metadata: meta,
            });
        }
    }
    out
}

/// Deezer search hit or track lookup.
pub fn candidate_from_deezer(track: &DeezerTrack) -> Candidate {
    let title = track.title.trim().to_owned();
    let artist = track.artist_name().trim().to_owned();
    let album = track.album.as_ref();
    let record_type = album.and_then(|a| a.record_type.as_deref());
    let meta = TrackMetadata {
        title,
        artists: if artist.is_empty() {
            vec![]
        } else {
            vec![artist.clone()]
        },
        artist,
        album: album.and_then(|a| trimmed(&a.title)),
        album_artist: album
            .and_then(|a| a.artist.as_ref())
            .and_then(|a| trimmed(&a.name)),
        duration: track.duration(),
        track_no: track.track_position,
        disc_no: track.disk_number,
        date: track
            .release_date
            .clone()
            .or_else(|| album.and_then(|a| a.release_date.clone()))
            .filter(|d| !d.is_empty() && !d.starts_with("0000")),
        isrcs: track.isrc.iter().cloned().collect(),
        release_type: record_type.and_then(deezer_record_type),
        release_secondary_types: if record_type == Some("compile") {
            vec!["Compilation".to_owned()]
        } else {
            Vec::new()
        },
        label: album.and_then(|a| a.label.clone()),
        cover_url: album.and_then(|a| a.best_cover().map(str::to_owned)),
        ..TrackMetadata::default()
    };
    Candidate {
        source: Source::Deezer,
        ids: CandidateIds {
            deezer_track_id: Some(track.id),
            deezer_album_id: album.map(|a| a.id).filter(|id| *id != 0),
            ..CandidateIds::default()
        },
        metadata: meta,
    }
}

fn deezer_record_type(t: &str) -> Option<String> {
    match t {
        "album" => Some("Album".into()),
        "single" => Some("Single".into()),
        "ep" => Some("EP".into()),
        // Deezer's "compile" is an album plus the Compilation secondary type.
        "compile" => Some("Album".into()),
        _ => None,
    }
}

// --------------------------------------------------------------- outputs

/// How each signal contributed (`None` = not available for this pair).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScoreBreakdown {
    pub title: Option<f64>,
    pub artist: Option<f64>,
    pub album: Option<f64>,
    pub duration: Option<f64>,
    pub track_no: Option<f64>,
    pub acoustid: Option<f64>,
    /// Product of all multiplicative penalties (1.0 = none).
    pub penalty: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchCandidate {
    /// 0..=1.
    pub score: f64,
    pub source: Source,
    pub ids: CandidateIds,
    pub metadata: TrackMetadata,
    pub breakdown: ScoreBreakdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Sure,
    Unsure,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchResult {
    pub confidence: Confidence,
    /// Every candidate, best first.
    pub ranked: Vec<MatchCandidate>,
    /// Up to [`MAX_OPTIONS`] best *distinct recordings* (the "confirm" queue choices).
    /// Starts with the winner; empty when there were no candidates.
    pub options: Vec<MatchCandidate>,
}

impl MatchResult {
    pub fn best(&self) -> Option<&MatchCandidate> {
        self.ranked.first()
    }
}

// --------------------------------------------------------------- scoring

struct Prepared {
    title: Option<(String, Variant)>,
    artists: Vec<String>,
    album: Option<String>,
    duration: Option<f64>,
    track_no: Option<u32>,
    track_total: Option<u32>,
    disc_no: Option<u32>,
    /// `(recording mbid, score)` of usable fingerprint hits.
    hits: Vec<(String, f64)>,
    live_intent: bool,
    compilation_intent: bool,
}

const COMPILATION_WORDS: &[&str] = &[
    "compilation",
    "collection",
    "greatest",
    "hits",
    "best",
    "anthology",
    "сборник",
    "лучшее",
    "хиты",
];

const GENERIC_ARTISTS: &[&str] = &["various artists", "va", "various", "raznye ispolniteli"];

impl Prepared {
    fn new(ev: &LocalEvidence) -> Self {
        let title = ev.resolved_title().map(|t| {
            let k = title_key(&t);
            (k.base, k.variant)
        });
        let mut artists = Vec::new();
        let sources = [
            ev.resolved_artist(),
            non_blank(ev.album_artist.as_deref()).map(str::to_owned),
        ];
        for raw in sources.into_iter().flatten() {
            for part in std::iter::once(raw.as_str()).chain(raw.split([';', '/'])) {
                let key = artist_key(part);
                if !key.is_empty()
                    && !GENERIC_ARTISTS.contains(&key.as_str())
                    && !artists.contains(&key)
                {
                    artists.push(key);
                }
            }
        }
        let album_raw = non_blank(ev.album.as_deref());
        let album = album_raw.map(album_key).filter(|a| !a.is_empty());
        let lowered_name = ev.filename.as_deref().unwrap_or("").to_lowercase();
        let album_lower = album_raw.unwrap_or("").to_lowercase();
        let has_word = |text: &str, wanted: &[&str]| {
            text.split(|c: char| !c.is_alphanumeric())
                .any(|w| wanted.contains(&w))
        };
        let live_intent = title
            .as_ref()
            .is_some_and(|(_, v)| v.contains(Variant::LIVE))
            || has_word(&album_lower, &["live", "лайв"])
            || has_word(&lowered_name, &["live", "лайв"]);
        let compilation_intent = has_word(&album_lower, COMPILATION_WORDS);
        let mut hits: Vec<(String, f64)> = Vec::new();
        for result in &ev.acoustid {
            if result.score < ACOUSTID_MIN_SCORE {
                continue;
            }
            for rec in &result.recordings {
                match hits.iter_mut().find(|(id, _)| *id == rec.id) {
                    Some((_, s)) => *s = s.max(result.score),
                    None => hits.push((rec.id.clone(), result.score)),
                }
            }
        }
        Self {
            title,
            artists,
            album,
            duration: ev.duration.map(|d| d.as_secs_f64()),
            track_no: ev.resolved_track_no(),
            track_total: ev.track_total,
            disc_no: ev.disc_no,
            hits,
            live_intent,
            compilation_intent,
        }
    }

    fn score(&self, cand: &Candidate) -> (f64, ScoreBreakdown) {
        let meta = &cand.metadata;
        let cand_title = title_key(&meta.title);
        let mut bd = ScoreBreakdown {
            penalty: 1.0,
            ..ScoreBreakdown::default()
        };

        bd.title = self
            .title
            .as_ref()
            .filter(|_| !cand_title.base.is_empty())
            .map(|(base, _)| similarity(base, &cand_title.base));

        if !self.artists.is_empty() {
            let mut keys: Vec<String> = Vec::new();
            if !meta.artist.is_empty() {
                keys.push(artist_key(&meta.artist));
            }
            keys.extend(meta.artists.iter().map(|a| artist_key(a)));
            let best = self
                .artists
                .iter()
                .flat_map(|e| keys.iter().map(move |c| similarity(e, c)))
                .fold(None::<f64>, |acc, s| Some(acc.map_or(s, |a| a.max(s))));
            bd.artist = best;
        }

        let cand_album = meta
            .album
            .as_deref()
            .map(album_key)
            .filter(|a| !a.is_empty());
        bd.album = match (&self.album, &cand_album) {
            (Some(e), Some(c)) => Some(similarity(e, c)),
            _ => None,
        };
        let album_sim = bd.album.unwrap_or(0.0);

        let mut duration_penalty = 1.0;
        if let (Some(e), Some(c)) = (self.duration, meta.duration) {
            let diff = (e - c.as_secs_f64()).abs();
            bd.duration = Some(if diff <= DURATION_STRONG {
                1.0
            } else if diff <= DURATION_WEAK {
                0.4
            } else {
                duration_penalty = 0.9;
                0.0
            });
        }

        if album_sim >= 0.8
            && let (Some(e), Some(c)) = (self.track_no, meta.track_no)
        {
            let disc_ok = match (self.disc_no, meta.disc_no) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            };
            bd.track_no = Some(if e == c && disc_ok { 1.0 } else { 0.0 });
        }

        let fingerprint = cand
            .ids
            .recording_mbid
            .as_deref()
            .and_then(|id| self.hits.iter().find(|(h, _)| h == id).map(|(_, s)| *s));
        if !self.hits.is_empty() && cand.ids.recording_mbid.is_some() {
            bd.acoustid = Some(fingerprint.unwrap_or(0.0));
        }
        let confirmed = fingerprint.filter(|s| *s >= ACOUSTID_STRONG);

        let parts = [
            (bd.title, W_TITLE),
            (bd.artist, W_ARTIST),
            (bd.album, W_ALBUM),
            (bd.duration, W_DURATION),
            (bd.track_no, W_TRACK),
            (bd.acoustid, W_ACOUSTID),
        ];
        let (sum, weight) = parts
            .iter()
            .filter_map(|(v, w)| v.map(|v| (v * w, *w)))
            .fold((0.0, 0.0), |(s, w), (v, vw)| (s + v, w + vw));
        let mut tags = if weight > 0.0 { sum / weight } else { 0.0 };

        // Without both title and artist (and no confirmed fingerprint) the evidence is
        // too thin for a Sure verdict.
        if (bd.title.is_none() || bd.artist.is_none()) && confirmed.is_none() {
            tags = tags.min(0.8);
        }
        tags *= duration_penalty;
        if let Some((_, ev_variant)) = &self.title {
            tags *= variant_penalty(*ev_variant, cand_title.variant);
        }
        let mut score = tags;
        if let Some(s) = confirmed {
            score = score.max(ACOUSTID_FLOOR_FACTOR * s);
        }

        let mut release_penalty = 1.0;
        let cand_live = cand_title.variant.contains(Variant::LIVE)
            || has_secondary(meta, "live")
            || meta
                .disambiguation
                .as_deref()
                .is_some_and(|d| d.to_lowercase().starts_with("live"));
        let ev_live = self.live_intent;
        if confirmed.is_none() && cand_live != ev_live && album_sim < 0.9 {
            release_penalty *= 0.9;
        }
        let cand_remix = has_secondary(meta, "remix");
        let ev_remix = self
            .title
            .as_ref()
            .is_some_and(|(_, v)| v.contains(Variant::REMIX));
        if cand_remix && !ev_remix && confirmed.is_none() {
            release_penalty *= 0.9;
        }
        if has_secondary(meta, "compilation") && !self.compilation_intent && album_sim < 0.9 {
            release_penalty *= 0.96;
        }
        if has_secondary(meta, "soundtrack")
            || has_secondary(meta, "dj-mix")
            || has_secondary(meta, "mixtape/street")
        {
            release_penalty *= 0.97;
        }
        match meta
            .release_status
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
        {
            None | Some("official") => {}
            Some("promotion") => release_penalty *= 0.97,
            Some(_) => release_penalty *= 0.95,
        }
        if let Some(t) = meta.release_type.as_deref()
            && matches!(t.to_lowercase().as_str(), "other" | "broadcast")
        {
            release_penalty *= 0.95;
        }
        if cand.source == Source::Deezer {
            release_penalty *= 0.98;
        }
        score *= release_penalty;
        bd.penalty = release_penalty * duration_penalty;

        (score.clamp(0.0, 1.0), bd)
    }
}

fn has_secondary(meta: &TrackMetadata, wanted: &str) -> bool {
    meta.release_secondary_types
        .iter()
        .any(|t| t.eq_ignore_ascii_case(wanted))
}

/// Multiplier for a title whose claimed variant (remix/acoustic/...) differs.
/// Live is handled together with the release type.
fn variant_penalty(evidence: Variant, candidate: Variant) -> f64 {
    let mut p = 1.0;
    if evidence.contains(Variant::REMIX) != candidate.contains(Variant::REMIX) {
        p *= 0.85;
    }
    if evidence.contains(Variant::ACOUSTIC) != candidate.contains(Variant::ACOUSTIC) {
        p *= 0.9;
    }
    if evidence.contains(Variant::INSTRUMENTAL) != candidate.contains(Variant::INSTRUMENTAL) {
        p *= 0.9;
    }
    p
}

/// Same song, possibly on a different release or from a different source.
fn same_recording(a: &MatchCandidate, b: &MatchCandidate) -> bool {
    if let (Some(x), Some(y)) = (&a.ids.recording_mbid, &b.ids.recording_mbid) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.ids.deezer_track_id, b.ids.deezer_track_id) {
        return x == y;
    }
    let (ma, mb) = (&a.metadata, &b.metadata);
    if ma
        .isrcs
        .iter()
        .any(|i| mb.isrcs.iter().any(|j| i.eq_ignore_ascii_case(j)))
    {
        return true;
    }
    same_song(ma, mb)
}

/// The same title, version and artist, and lengths within two seconds.
fn same_song(ma: &TrackMetadata, mb: &TrackMetadata) -> bool {
    let (ta, tb) = (title_key(&ma.title), title_key(&mb.title));
    let close_in_time = match (ma.duration, mb.duration) {
        (Some(x), Some(y)) => (x.as_secs_f64() - y.as_secs_f64()).abs() <= 2.0,
        _ => true,
    };
    ta.base == tb.base
        && ta.variant == tb.variant
        && artist_key(&ma.artist) == artist_key(&mb.artist)
        && close_in_time
}

/// Ranks candidates against the evidence and decides whether the top one is safe to
/// apply without asking the user.
pub fn match_candidates(evidence: &LocalEvidence, candidates: Vec<Candidate>) -> MatchResult {
    let prepared = Prepared::new(evidence);
    let mut ranked: Vec<MatchCandidate> = candidates
        .into_iter()
        .map(|c| {
            let (score, breakdown) = prepared.score(&c);
            MatchCandidate {
                score,
                source: c.source,
                ids: c.ids,
                metadata: c.metadata,
                breakdown,
            }
        })
        .collect();

    // Scores are quantized so float noise cannot break ties between equal releases;
    // ties go to MusicBrainz, then to the earliest release.
    ranked.sort_by_key(|c| {
        (
            std::cmp::Reverse(quantized(c)),
            source_rank(c.source),
            date_key(c),
        )
    });
    prefer_album(&prepared, &mut ranked);

    let mut options: Vec<MatchCandidate> = Vec::new();
    for c in &ranked {
        if options.len() == MAX_OPTIONS {
            break;
        }
        if !options.iter().any(|o| same_recording(o, c)) {
            options.push(c.clone());
        }
    }

    let confidence = match options.as_slice() {
        [] => Confidence::Unsure,
        [only] => verdict(only.score, 0.0),
        [top, second, ..] => verdict(top.score, second.score),
    };
    MatchResult {
        confidence,
        ranked,
        options,
    }
}

fn quantized(c: &MatchCandidate) -> i64 {
    (c.score * 1e6).round() as i64
}

fn release_type_is(meta: &TrackMetadata, kinds: &[&str]) -> bool {
    meta.release_type
        .as_deref()
        .is_some_and(|t| kinds.iter().any(|k| t.eq_ignore_ascii_case(k)))
}

/// A file of an album (a track after the first, or a release of several
/// tracks) whose best match is a single or EP of its song, released
/// earlier and as good as the album's own: the album's wins. Its track
/// number must be the file's when both are known.
fn prefer_album(p: &Prepared, ranked: &mut Vec<MatchCandidate>) {
    let of_album = p.track_no.is_some_and(|n| n > 1) || p.track_total.is_some_and(|n| n > 1);
    let Some(top) = ranked.first() else { return };
    if !of_album || !release_type_is(&top.metadata, &["single", "ep"]) {
        return;
    }
    let best = quantized(top);
    let album = ranked.iter().position(|c| {
        best - quantized(c) <= ALBUM_OVER_SINGLE
            && release_type_is(&c.metadata, &["album"])
            && c.metadata.release_secondary_types.is_empty()
            && same_song(&top.metadata, &c.metadata)
            && match (p.track_no, c.metadata.track_no) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    });
    if let Some(i) = album {
        let c = ranked.remove(i);
        ranked.insert(0, c);
    }
}

fn verdict(top: f64, second: f64) -> Confidence {
    if top >= SURE_MIN_SCORE && top - second >= SURE_MIN_MARGIN {
        Confidence::Sure
    } else {
        Confidence::Unsure
    }
}

fn source_rank(s: Source) -> u8 {
    match s {
        Source::MusicBrainz => 0,
        Source::Deezer => 1,
    }
}

/// Earlier dates first; undated last.
fn date_key(c: &MatchCandidate) -> (bool, String) {
    match &c.metadata.date {
        Some(d) => (false, d.clone()),
        None => (true, String::new()),
    }
}
