//! Torrents as a [`Source`]: the indexer (Prowlarr) searched with the
//! query variants of [`crate::text::torrent_queries`], two at a time, until
//! a sure release turns up; releases become [`Offer`]s carrying the
//! indexer's [`Candidate`]. A fetch hands the torrent back: the torrent
//! client lists its files and [`pick_files`] takes the wished track (or
//! album) out of an album, a discography or a collection.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::{BoxFuture, Candidate, Indexer, SearchQuery};
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::{parse_filename, similarity, title_key};

use crate::matching::{contains_words, judge, other_version};
use crate::text::{album_keys, title_keys, torrent_queries};

/// Queries per item at most (each takes Prowlarr up to a minute).
const MAX_QUERIES: usize = 5;
/// Queries asked at once.
const AT_ONCE: usize = 2;
const QUERY_TIMEOUT: Duration = Duration::from_secs(90);

/// Torrents through an [`Indexer`].
pub struct IndexerSource {
    indexer: Arc<dyn Indexer>,
    name: String,
}

impl IndexerSource {
    pub fn new(indexer: Arc<dyn Indexer>) -> Self {
        let name = indexer.name().to_owned();
        Self { indexer, name }
    }
}

/// `needle` in `hay` not glued to letters on either side.
fn has_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + needle.len()..].chars().next();
        !before.is_some_and(char::is_alphabetic) && !after.is_some_and(char::is_alphabetic)
    })
}

/// The word may go on ("remaster" in "remastered").
fn has_stem(hay: &str, stem: &str) -> bool {
    hay.match_indices(stem).any(|(i, _)| !hay[..i].chars().next_back().is_some_and(char::is_alphabetic))
}

/// The format a release title names: "[FLAC 24/96]", "MP3 320", "Hi-Res".
pub fn guess_format(title: &str) -> Option<Format> {
    let t = title.to_lowercase();
    let has = |w: &str| has_word(&t, w);
    let codec = if has("flac") || has("lossless") || has("hi-res") || has("hires") {
        Codec::Flac
    } else if has("alac") {
        Codec::Alac
    } else if has("ape") && (has("cue") || has("image")) {
        Codec::Other("ape".into())
    } else if has("wav") || has("wavpack") {
        Codec::Wav
    } else if has("mp3") {
        Codec::Mp3
    } else if has("aac") || has("m4a") {
        Codec::Aac
    } else if has("opus") {
        Codec::Opus
    } else {
        return None;
    };
    let mut f = Format::new(codec);
    for (needle, bits, rate) in [
        ("24/192", 24, 192_000),
        ("24-192", 24, 192_000),
        ("24/96", 24, 96_000),
        ("24-96", 24, 96_000),
        ("24/48", 24, 48_000),
        ("48khz/24bit", 24, 48_000),
        ("24/44", 24, 44_100),
        ("24bit", 24, 44_100),
        ("24-bit", 24, 44_100),
        ("24 bit", 24, 44_100),
        ("hi-res", 24, 96_000),
    ] {
        if f.lossless() && has_stem(&t, needle) {
            f.bit_depth = Some(bits);
            f.sample_rate = Some(rate);
            break;
        }
    }
    if !f.lossless() {
        for (needle, kbps) in [("320", 320), ("v0", 245), ("256", 256), ("192", 192), ("128", 128)] {
            if has_stem(&t, needle) {
                f.bitrate_kbps = Some(kbps);
                break;
            }
        }
    }
    Some(f)
}

/// A release of several albums: a discography, a collection, the singles.
pub fn scope_of(title: &str) -> OfferScope {
    let t = title.to_lowercase();
    let many = [
        "discography",
        "дискография",
        "collection",
        "коллекция",
        "complete",
        "singles",
        "синглы",
        "anthology",
        "антология",
        "box set",
        "albums",
        "альбомы",
        "ディスコグラフィ",
    ];
    if many.iter().any(|w| has_stem(&t, w)) {
        OfferScope::Collection
    } else {
        OfferScope::Album
    }
}

/// A release as an offer (the candidate rides in the handle).
pub fn offer_of(c: Candidate) -> Offer {
    Offer {
        source: SourceKey::Torrents,
        provider: c.indexer_name.clone(),
        id: c.id.clone(),
        title: c.title.clone(),
        artist: None,
        track_title: None,
        album: None,
        duration_ms: None,
        isrc: None,
        scope: scope_of(&c.title),
        format: guess_format(&c.title),
        size_bytes: c.size_bytes,
        seeders: c.seeders,
        info_url: c.info_url.clone(),
        handle: serde_json::to_string(&c).unwrap_or_default(),
    }
}

/// The candidate of a torrent offer.
pub fn candidate_of(o: &Offer) -> Option<Candidate> {
    (o.source == SourceKey::Torrents).then(|| serde_json::from_str(&o.handle).ok()).flatten()
}

/// How a torrent ranks among torrents of the same format: live, bootleg,
/// karaoke and huge sizes lower, seeders higher.
pub fn release_bonus(o: &Offer, want: &Want) -> f64 {
    let t = o.title.to_lowercase();
    let wanted = format!("{} {}", want.title.as_deref().unwrap_or_default(), want.album.as_deref().unwrap_or_default()).to_lowercase();
    let mut s = 0.0;
    for (bad, w) in [("live", 0.4), ("concert", 0.4), ("bootleg", 0.5), ("remaster", 0.1), ("deluxe", 0.05)] {
        if has_stem(&t, bad) && !has_stem(&wanted, bad) {
            s -= w;
        }
    }
    if let Some(seed) = o.seeders {
        s += (f64::from(seed) + 1.0).log2() * 0.05;
    }
    // A discography of 20 GB for one track is fine (one file comes), but
    // an album of its own is better.
    if o.scope == OfferScope::Collection && want.scope == WantScope::Album {
        s -= 0.3;
    }
    if let Some(size) = o.size_bytes {
        let gb = size as f64 / 1e9;
        if gb > 6.0 && o.scope != OfferScope::Collection {
            s -= (gb - 6.0) * 0.02;
        }
    }
    s
}

/// A release this small is a single (or an EP) at most.
const SINGLE_MAX: u64 = 200 << 20;
/// A release this big that is not a collection is a whole album or more.
const BIG_ALBUM: u64 = 1 << 30;

/// Which of a wish's doubtful torrents to look into first, before any of
/// their lists comes (higher first), by what their titles and sizes say:
/// the song named (or its album) is likely; a discography or a collection
/// may have it; a release of a single's size that names neither is
/// another single, a big album that names neither another album; with no
/// seeders not even the list comes.
pub fn look_first(o: &Offer, want: &Want) -> f64 {
    let hay = title_key(&o.title).base;
    let names = |keys: Vec<String>| keys.iter().any(|k| contains_words(&hay, k));
    let title = want.scope == WantScope::Track && want.title.as_deref().is_some_and(|t| names(title_keys(t)));
    let album = !want.is_single() && want.album.as_deref().is_some_and(|a| names(album_keys(a)));
    let collection = o.scope == OfferScope::Collection;
    let mut s = 0.0;
    if title {
        s += 3.0;
    }
    if album {
        s += 2.0;
    }
    if collection {
        s += 1.0;
    }
    if !title && !album && !collection
        && let Some(size) = o.size_bytes
    {
        if size <= SINGLE_MAX {
            s -= 2.0;
        } else if size >= BIG_ALBUM {
            s -= 1.0;
        }
    }
    match o.seeders {
        Some(0) => s -= 2.0,
        Some(n) => s += (f64::from(n) + 1.0).log2() * 0.05,
        None => {}
    }
    s
}

impl Source for IndexerSource {
    fn key(&self) -> SourceKey {
        SourceKey::Torrents
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            let queries = torrent_queries(want, MAX_QUERIES);
            if queries.is_empty() {
                return Ok(Vec::new());
            }
            log::info!("torrents: searching {queries:?}");
            let mut found: Vec<Offer> = Vec::new();
            let mut failed: Option<SourceError> = None;
            let mut answered = false;
            for chunk in queries.chunks(AT_ONCE) {
                let asks = chunk.iter().map(|q| {
                    let mut query = SearchQuery::new(q.clone());
                    query.limit = Some(100);
                    let indexer = self.indexer.clone();
                    async move {
                        match tokio::time::timeout(QUERY_TIMEOUT, indexer.search(&query)).await {
                            Ok(r) => r.map_err(SourceError::from),
                            Err(_) => Err(SourceError::Unreachable("the indexer did not answer".into())),
                        }
                    }
                });
                for r in futures_util::future::join_all(asks).await {
                    match r {
                        Ok(cands) => {
                            answered = true;
                            for c in cands {
                                if !found.iter().any(|o| o.id == c.id) {
                                    found.push(offer_of(c));
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("torrents: {e}");
                            failed.get_or_insert(e);
                        }
                    }
                }
                // A sure lossless release: the rest of the queries can wait.
                if found.iter().any(|o| o.lossless() && judge(o, want).is_some_and(|v| v.sure)) {
                    break;
                }
            }
            match failed {
                Some(e) if !answered => Err(e),
                _ => Ok(found),
            }
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, _: &'a Want, _: &'a Path, _: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let c = candidate_of(offer).ok_or(SourceError::NothingToFetch)?;
            let payload = tokio::time::timeout(Duration::from_secs(90), self.indexer.fetch_torrent(&c))
                .await
                .map_err(|_| SourceError::Unreachable("the indexer did not give the torrent".into()))??;
            Ok(Fetched::Torrent(payload))
        })
    }
}

const AUDIO_EXT: &[&str] = &[
    "flac", "mp3", "m4a", "ogg", "opus", "wav", "ape", "wv", "aac", "alac", "aiff", "aif", "dsf",
];

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The disc a path is on, from a "CD2" / "Disc 2" / "Disk 2" folder.
pub fn disc_of(path: &str) -> Option<u32> {
    let lower = path.to_lowercase();
    for part in lower.split('/').rev().skip(1) {
        let p = part.trim();
        for prefix in ["cd", "disc", "disk", "диск"] {
            if let Some(rest) = p.strip_prefix(prefix) {
                let digits: String = rest
                    .trim_start_matches([' ', '_', '-', '.'])
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if let Ok(n) = digits.parse() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// A file name must be this close to the track title (0..=1)…
const FILE_TITLE: f64 = 0.85;
/// …or this close when its number is the track's in the album's folder.
const FILE_NUMBERED: f64 = 0.4;

/// The folders of a path, folded.
fn dirs_key(path: &str) -> String {
    title_key(path.rsplit_once('/').map_or("", |(d, _)| d)).base
}

/// The files to download for `w` out of a torrent's list (index, path,
/// size): the track's file — its title (any of its forms), or its number
/// with a like title in the album's folder, on its disc —, or every audio
/// file of the album (its folder when the torrent has several). A file of
/// another version (live, remix, instrumental) is not the track. `None`
/// when the torrent does not have it.
pub fn pick_files(files: &[(usize, String, u64)], w: &Want) -> Option<Vec<usize>> {
    let audio: Vec<&(usize, String, u64)> = files.iter().filter(|(_, p, _)| AUDIO_EXT.contains(&ext_of(p).as_str())).collect();
    if audio.is_empty() {
        return None;
    }
    let albums: Vec<String> = w.album.as_deref().map(album_keys).unwrap_or_default();
    let in_album = |p: &str| {
        let d = dirs_key(p);
        albums.iter().any(|a| contains_words(&d, a))
    };
    let album_files: Vec<&&(usize, String, u64)> = audio.iter().filter(|(_, p, _)| in_album(p)).collect();
    match (w.scope, &w.title) {
        (WantScope::Track, Some(title)) => {
            let want = title_keys(title);
            let want_text = format!("{title} {}", w.album.as_deref().unwrap_or_default());
            let mut best: Option<(f64, usize)> = None;
            for (index, path, _) in &audio {
                let album_dir = in_album(path) || album_files.is_empty();
                if album_dir
                    && let Some(d) = disc_of(path)
                    && d != w.disc_no.max(1)
                {
                    continue;
                }
                let name = path.rsplit('/').next().unwrap_or(path);
                if other_version(name, &want_text).is_some() {
                    continue;
                }
                let parsed = parse_filename(name);
                let stem = Path::new(name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let got: Vec<String> = [parsed.title.as_deref().unwrap_or(&stem), stem.as_str()]
                    .iter()
                    .flat_map(|s| title_keys(s))
                    .collect();
                let sim = got
                    .iter()
                    .flat_map(|g| want.iter().map(move |k| similarity(g, k)))
                    .fold(0.0, f64::max);
                let numbered = album_dir && w.track_no > 0 && parsed.track_no == Some(w.track_no);
                if sim >= FILE_TITLE || (numbered && sim >= FILE_NUMBERED) {
                    let mut rank = sim + if numbered { 0.5 } else { 0.0 };
                    // The album's own copy of a song before a compilation's.
                    if in_album(path) {
                        rank += 0.3;
                    }
                    if matches!(ext_of(path).as_str(), "flac" | "wav" | "alac" | "ape" | "wv" | "aiff") {
                        rank += 0.05;
                    }
                    if best.is_none_or(|(r, _)| rank > r) {
                        best = Some((rank, *index));
                    }
                }
            }
            best.map(|(_, i)| vec![i])
        }
        _ => {
            let mut picked: Vec<usize> = if album_files.is_empty() {
                audio.iter().map(|(i, ..)| *i).collect()
            } else {
                album_files.iter().map(|(i, ..)| *i).collect()
            };
            // The cue sheets of the picked folders: a single-file rip is
            // cut into its tracks by its sheet.
            let dirs: Vec<&str> = files
                .iter()
                .filter(|(i, ..)| picked.contains(i))
                .map(|(_, p, _)| p.rsplit_once('/').map_or("", |(d, _)| d))
                .collect();
            picked.extend(files.iter().filter_map(|(i, p, _)| {
                let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
                (ext_of(p) == "cue" && dirs.contains(&dir)).then_some(*i)
            }));
            picked.sort_unstable();
            Some(picked)
        }
    }
}

/// The files of a download (paths with `/` or `\\`) hold a single-file
/// rip: a folder with a cue sheet and no more audio files than sheets, each
/// image to be cut into its tracks by its sheet. A release of track files
/// that keeps its sheet has many more audio files than sheets beside it.
pub fn image_rip<'a>(paths: impl IntoIterator<Item = &'a str>) -> bool {
    let mut dirs: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    for p in paths {
        let p = p.replace('\\', "/");
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
        let ext = ext_of(&p);
        if ext == "cue" {
            dirs.entry(dir).or_default().0 += 1;
        } else if AUDIO_EXT.contains(&ext.as_str()) {
            dirs.entry(dir).or_default().1 += 1;
        }
    }
    dirs.values().any(|&(sheets, audio)| sheets > 0 && audio > 0 && audio <= sheets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::indexer::{CandidateHandle, IndexerError, TorrentPayload};
    use std::sync::Mutex;

    fn want(scope: WantScope) -> Want {
        Want {
            artists: vec!["Кино".into()],
            album: Some("Группа крови".into()),
            title: Some("Война".into()),
            track_no: 3,
            disc_no: 1,
            scope,
            ..Want::default()
        }
    }

    #[test]
    fn formats_and_collections_are_read_from_titles() {
        assert_eq!(guess_format("Eve - Under Blue [FLAC 48kHz/24bit]").unwrap().label(), "FLAC 24/48");
        assert_eq!(guess_format("Eve Kaizin (2022) Mp3 320kbps [PMEDIA]").unwrap().label(), "MP3 320");
        assert!(guess_format("Кино - Группа крови").is_none());
        assert!(guess_format("new wave tape").is_none());
        assert_eq!(scope_of("Eve - Discography (2017-2024) FLAC"), OfferScope::Collection);
        assert_eq!(scope_of("Кино - Группа крови (1988) FLAC"), OfferScope::Album);
    }

    #[test]
    fn picks_the_track_file_on_its_disc_or_the_whole_album() {
        let files = vec![
            (0, "Кино - Группа крови/01 - Группа крови.flac".into(), 30),
            (1, "Кино - Группа крови/02 - Закрой за мной дверь.flac".into(), 30),
            (2, "Кино - Группа крови/03 - Война.flac".into(), 30),
            (3, "Кино - Группа крови/cover.jpg".into(), 1),
        ];
        assert_eq!(pick_files(&files, &want(WantScope::Track)), Some(vec![2]));
        assert_eq!(pick_files(&files, &want(WantScope::Album)), Some(vec![0, 1, 2]));
        // A single-file rip comes with its cue sheet (not another folder's).
        let image = vec![
            (0, "Кино - Группа крови/CDImage.ape".into(), 300),
            (1, "Кино - Группа крови/CDImage.cue".into(), 1),
            (2, "Кино - Группа крови/CDImage.log".into(), 1),
            (3, "Other/CDImage.cue".into(), 1),
        ];
        assert_eq!(pick_files(&image, &want(WantScope::Album)), Some(vec![0, 1]));
        assert!(image_rip(["Кино - Группа крови/CDImage.ape", "Кино - Группа крови/CDImage.cue"]));
        assert!(image_rip([r"D:\dl\Кино\CDImage.flac", r"D:\dl\Кино\CDImage.cue"]), "Windows paths");
        // Track files with their sheet, or a sheet of another folder: not an image.
        assert!(!image_rip([
            "Кино - Группа крови/01 - Группа крови.flac",
            "Кино - Группа крови/02 - Закрой за мной дверь.flac",
            "Кино - Группа крови/Группа крови.cue",
        ]));
        assert!(!image_rip(["Кино - Группа крови/CDImage.ape", "Other/CDImage.cue"]));
        assert!(!image_rip(files.iter().map(|(_, p, _)| p.as_str())));
        let mut w = want(WantScope::Track);
        w.title = Some("Voyna (War)".into());
        assert_eq!(pick_files(&files, &w), Some(vec![2]), "by its number");
        let mut w = want(WantScope::Track);
        w.title = Some("Звезда".into());
        w.track_no = 9;
        assert_eq!(pick_files(&files, &w), None);
        let discs = vec![(0, "Album/CD1/03 - Война.flac".into(), 30), (1, "Album/CD2/03 - Война.flac".into(), 30)];
        let mut w = want(WantScope::Track);
        w.disc_no = 2;
        assert_eq!(pick_files(&discs, &w), Some(vec![1]));
        assert_eq!(disc_of("A/Disc 2/01.flac"), Some(2));
    }

    #[test]
    fn a_single_is_found_inside_a_discography() {
        let files = vec![
            (0, "Eve - Discography/2020 - Smile/01 - Kaikai Kitan.flac".into(), 30),
            (1, "Eve - Discography/2020 - Smile/05 - Underdog (Live).flac".into(), 30),
            (2, "Eve - Discography/Singles/2023 - Underdog/01 - Underdog.flac".into(), 30),
            (3, "Eve - Discography/2022 - Kaizin/03 - Avant.flac".into(), 30),
            (4, "Eve - Discography/2024 - Under Blue/07 - 火星人.flac".into(), 30),
        ];
        let w = Want {
            artists: vec!["Eve".into()],
            title: Some("Underdog".into()),
            album: Some("Underdog".into()),
            track_no: 1,
            scope: WantScope::Track,
            ..Want::default()
        };
        assert_eq!(pick_files(&files, &w), Some(vec![2]), "the studio file, not the live one");
        // The number alone does not pick a file outside the album's folder.
        let w = Want { title: Some("Avant".into()), album: Some("Kaizin".into()), track_no: 1, ..w };
        assert_eq!(pick_files(&files, &w), Some(vec![3]));
        // The original script of a title in two.
        let w = Want { title: Some("火星人 - Martian".into()), album: Some("second person".into()), track_no: 0, ..w };
        assert_eq!(pick_files(&files, &w), Some(vec![4]));
    }

    type Answer = Box<dyn Fn(&str) -> Result<Vec<Candidate>, IndexerError> + Send + Sync>;

    struct Fake {
        asked: Mutex<Vec<String>>,
        answer: Answer,
    }

    impl Indexer for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn search(&self, q: &SearchQuery) -> BoxFuture<'_, Result<Vec<Candidate>, IndexerError>> {
            self.asked.lock().unwrap().push(q.text.clone());
            let r = (self.answer)(&q.text);
            Box::pin(async move { r })
        }
        fn fetch_torrent(&self, _: &Candidate) -> BoxFuture<'_, Result<TorrentPayload, IndexerError>> {
            Box::pin(async { Ok(TorrentPayload::Magnet("magnet:?xt=urn:btih:x".into())) })
        }
    }

    pub(crate) fn cand(title: &str, seeders: u32) -> Candidate {
        Candidate {
            id: title.into(),
            title: title.into(),
            indexer_name: "RuTracker".into(),
            size_bytes: Some(400_000_000),
            seeders: Some(seeders),
            leechers: None,
            published: None,
            categories: vec![3040],
            info_url: None,
            info_hash: None,
            handle: CandidateHandle::default(),
        }
    }

    fn sized(title: &str, mb: u64, seeders: u32) -> Offer {
        offer_of(Candidate { size_bytes: Some(mb << 20), ..cand(title, seeders) })
    }

    /// The order a wish's doubtful torrents are looked into.
    fn looked(offers: &[Offer], w: &Want) -> Vec<String> {
        let mut v: Vec<&Offer> = offers.iter().collect();
        v.sort_by(|a, b| look_first(b, w).total_cmp(&look_first(a, w)));
        v.iter().map(|o| o.title.clone()).collect()
    }

    /// «怪獣» of sakanaction got the single «さよならはエモーション
    /// / 蓮の花» (46 MB, no seeders) as the one to take.
    #[test]
    fn another_single_of_the_artist_is_looked_into_last_and_has_no_file() {
        let w = Want {
            artists: vec!["sakanaction".into()],
            title: Some("怪獣".into()),
            album: Some("怪獣".into()),
            scope: WantScope::Track,
            ..Want::default()
        };
        let other = "サカナクション (sakanaction) - さよならはエモーション / 蓮の花 (2014) [FLAC]";
        let offers = vec![
            sized(other, 46, 0),
            sized("sakanaction - Discography (2007-2025) [FLAC]", 20_000, 4),
            sized("sakanaction - 怪獣 (2025) [FLAC]", 60, 7),
        ];
        assert_eq!(
            looked(&offers, &w),
            ["sakanaction - 怪獣 (2025) [FLAC]", "sakanaction - Discography (2007-2025) [FLAC]", other]
        );
        let v = judge(&offers[0], &w).expect("the artist's: asked, once its list has the song");
        assert!(!v.sure);
        let files = vec![
            (0, "sakanaction - さよならはエモーション/01. さよならはエモーション.flac".into(), 30),
            (1, "sakanaction - さよならはエモーション/02. 蓮の花.flac".into(), 16),
        ];
        assert_eq!(pick_files(&files, &w), None);
    }

    /// The clip «SawanoHiroyuki[nZk]:Rei『INERTIA』 Music Video»
    /// got three whole «Attack on Titan» soundtracks (1.3–2.4 GB).
    #[test]
    fn a_big_album_that_names_neither_the_song_nor_its_album_comes_last() {
        let w = Want {
            artists: vec!["SawanoHiroyuki[nZk]".into()],
            aliases: vec!["Hiroyuki Sawano".into()],
            title: Some("SawanoHiroyuki[nZk]:Rei『INERTIA』 Music Video".into()),
            scope: WantScope::Track,
            ..Want::default()
        };
        let s1 = "[Hi-Res] Attack on Titan Season 1 Original Soundtrack - Hiroyuki Sawano [FLAC 24/96]";
        let s2 = "Attack on Titan Season 2 Original Soundtrack (Hiroyuki Sawano) [FLAC]";
        let song = "SawanoHiroyuki[nZk]:Rei - INERTIA (2024) [FLAC]";
        let offers = vec![sized(s1, 2_400, 30), sized(s2, 1_300, 12), sized(song, 80, 3)];
        assert_eq!(looked(&offers, &w)[0], song);
        assert!(look_first(&offers[0], &w) < 0.0 && look_first(&offers[1], &w) < 0.0);
        let files = vec![
            (0, "Attack on Titan OST/01. Attack on Titan.flac".into(), 90),
            (1, "Attack on Titan OST/02. Vogel im Käfig.flac".into(), 90),
            (2, "Attack on Titan OST/03. YouSeeBIGGIRL T:T.flac".into(), 90),
        ];
        assert_eq!(pick_files(&files, &w), None);
    }

    #[tokio::test]
    async fn variants_stop_once_a_sure_release_is_found() {
        let fake = Arc::new(Fake {
            asked: Mutex::new(Vec::new()),
            answer: Box::new(|q| {
                Ok(if q == "Yorushika second person" {
                    vec![cand("ヨルシカ (Yorushika) - 二人称 (second person) [FLAC]", 9)]
                } else {
                    Vec::new()
                })
            }),
        });
        let src = IndexerSource::new(fake.clone());
        let w = Want {
            artists: vec!["Yorushika".into()],
            title: Some("火星人 - Martian".into()),
            album: Some("second person".into()),
            ..Want::default()
        };
        let offers = src.search(&w).await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].format.as_ref().unwrap().label(), "FLAC");
        assert_eq!(fake.asked.lock().unwrap().len(), 2, "the first two queries only");
        let fetched = src.fetch(&offers[0], &w, Path::new("."), Arc::new(|_, _| {})).await.unwrap();
        assert!(matches!(fetched, Fetched::Torrent(TorrentPayload::Magnet(_))));
    }

    #[tokio::test]
    async fn an_indexer_down_for_every_query_is_an_outage_not_a_miss() {
        let fake = Arc::new(Fake {
            asked: Mutex::new(Vec::new()),
            answer: Box::new(|_| Err(IndexerError::Unreachable("error sending request".into()))),
        });
        let w = want(WantScope::Track);
        let err = IndexerSource::new(fake).search(&w).await.unwrap_err();
        assert!(err.is_outage());
    }
}
