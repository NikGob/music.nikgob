//! Torrent search through the configured [`Indexer`] (Prowlarr) and the
//! choice of a release for download (PLAN §7: format → release match →
//! seeders → sane size).

use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::{Candidate, Indexer, SearchQuery};
use nikgob_view as v;

use crate::Worker;
use crate::import::Internal;

/// The indexer from settings.toml, if configured.
pub(crate) fn indexer_from(path: Option<&std::path::Path>) -> Option<Arc<dyn Indexer>> {
    let settings = nikgob_core::settings::Settings::load(path?).ok()?;
    nikgob_indexer_prowlarr::Prowlarr::from_settings(&settings)
        .map(|p| Arc::new(p) as Arc<dyn Indexer>)
        .ok()
}

/// `needle` occurs in `hay` not glued to letters on either side ("wav" is not
/// in "new wave", "ape" not in "tape", "live" not in "alive"). `hay` and
/// `needle` are lowercase.
fn has_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + needle.len()..].chars().next();
        !before.is_some_and(char::is_alphabetic) && !after.is_some_and(char::is_alphabetic)
    })
}

/// Like [`has_word`] but the word may continue ("remaster" in "remastered");
/// it must still not start in the middle of another word ("live" in "olive").
fn has_stem(hay: &str, stem: &str) -> bool {
    hay.match_indices(stem)
        .any(|(i, _)| !hay[..i].chars().next_back().is_some_and(char::is_alphabetic))
}

/// "FLAC 24/96", "MP3 320"… from a release title.
pub(crate) fn guess_format(title: &str) -> Option<String> {
    let t = title.to_lowercase();
    let has = |w: &str| has_word(&t, w);
    let codec = if has("flac") {
        "FLAC"
    } else if has("alac") {
        "ALAC"
    } else if has("ape") && (has("cue") || has("image")) {
        "APE"
    } else if has("wav") {
        "WAV"
    } else if has("mp3") {
        "MP3"
    } else if has("aac") || has("m4a") {
        "AAC"
    } else if has("opus") {
        "Opus"
    } else {
        return None;
    };
    let mut out = codec.to_owned();
    for (needle, label) in [
        ("24/192", " 24/192"),
        ("24-192", " 24/192"),
        ("24/96", " 24/96"),
        ("24-96", " 24/96"),
        ("24/48", " 24/48"),
        ("24/44", " 24/44"),
        ("24bit", " 24 bit"),
        ("24 bit", " 24 bit"),
        ("hi-res", " Hi-Res"),
        ("320", " 320"),
        ("v0", " V0"),
    ] {
        if has_stem(&t, needle) {
            out.push_str(label);
            break;
        }
    }
    Some(out)
}

/// Higher is better for automatic download.
pub(crate) fn score(c: &Candidate, query: &str) -> f64 {
    let t = c.title.to_lowercase();
    let q = query.to_lowercase();
    let wanted = |w: &str| has_stem(&q, w);
    let mut s = 0.0;
    match guess_format(&c.title).as_deref().map(|f| f.split(' ').next().unwrap_or(f)) {
        Some("FLAC" | "ALAC" | "WAV" | "APE") => s += 100.0,
        Some("MP3" | "AAC" | "Opus") => s += 30.0,
        _ => {}
    }
    for (bad, w) in [
        ("live", 40.0),
        ("concert", 40.0),
        ("bootleg", 50.0),
        ("remaster", 15.0),
        ("deluxe", 5.0),
        ("karaoke", 80.0),
        ("instrumental", 30.0),
        ("cover", 20.0),
        ("tribute", 60.0),
    ] {
        if has_stem(&t, bad) && !wanted(bad) {
            s -= w;
        }
    }
    if (has_stem(&t, "discography") || has_stem(&t, "дискография") || has_stem(&t, "collection"))
        && !wanted("discography")
        && !wanted("дискография")
    {
        s -= 60.0;
    }
    // Words of the query found in the title.
    let words: Vec<&str> = q.split_whitespace().filter(|w| w.len() > 1).collect();
    if !words.is_empty() {
        let hit = words.iter().filter(|w| t.contains(**w)).count() as f64;
        s += 40.0 * hit / words.len() as f64;
    }
    if let Some(seed) = c.seeders {
        s += (f64::from(seed) + 1.0).log2() * 6.0;
        if seed == 0 {
            s -= 100.0;
        }
    }
    if let Some(size) = c.size_bytes {
        let gb = size as f64 / 1e9;
        if gb > 6.0 {
            s -= (gb - 6.0) * 3.0;
        }
    }
    s
}

/// Index of the best candidate.
pub(crate) fn recommend(cands: &[Candidate], query: &str) -> Option<usize> {
    cands
        .iter()
        .enumerate()
        .map(|(i, c)| (i, score(c, query)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

pub(crate) fn results_view(cands: &[Candidate], query: &str) -> Vec<v::TorrentResult> {
    let best = recommend(cands, query);
    cands
        .iter()
        .enumerate()
        .map(|(i, c)| v::TorrentResult {
            index: i,
            title: c.title.clone(),
            format: guess_format(&c.title),
            size_bytes: c.size_bytes,
            seeders: c.seeders,
            indexer: c.indexer_name.clone(),
            recommended: Some(i) == best,
        })
        .collect()
}

impl Worker {
    pub(crate) fn search_torrents(&mut self, query: String) {
        let q = query.trim().to_owned();
        if q.is_empty() {
            return;
        }
        let Some(indexer) = self.indexer.clone() else {
            self.publish(|s, _| {
                s.search.torrents = v::TorrentSearch {
                    query: q,
                    error: Some("Prowlarr не настроен — адрес и ключ в Настройках → Торренты".into()),
                    ..Default::default()
                };
            });
            return;
        };
        let Some(svc) = &self.import else { return };
        let tx = svc.internal();
        let q2 = q.clone();
        svc.spawn(async move {
            let mut query = SearchQuery::new(q2.clone());
            query.limit = Some(100);
            let r = tokio::time::timeout(Duration::from_secs(90), indexer.search(&query))
                .await
                .map_err(|_| "Prowlarr не ответил за 90 с".to_owned())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = tx.send(Internal::TorrentResults(q2, r));
        });
        self.publish(|s, _| {
            s.search.torrents = v::TorrentSearch {
                query: q,
                searching: true,
                ..Default::default()
            };
        });
    }

    pub(crate) fn torrent_results(&mut self, query: String, r: Result<Vec<Candidate>, String>) {
        if self.state.read().search.torrents.query != query {
            return;
        }
        match r {
            Ok(mut cands) => {
                cands.sort_by(|a, b| b.seeders.unwrap_or(0).cmp(&a.seeders.unwrap_or(0)));
                let results = results_view(&cands, &query);
                self.candidates = cands;
                self.publish(|s, _| {
                    s.search.torrents.searching = false;
                    s.search.torrents.error = results.is_empty().then(|| "Раздач не нашлось".to_owned());
                    s.search.torrents.results = results;
                });
            }
            Err(e) => {
                log::warn!("torrent search: {e}");
                self.publish(|s, _| {
                    s.search.torrents.searching = false;
                    s.search.torrents.error = Some(e);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::indexer::CandidateHandle;

    fn cand(title: &str, seeders: u32, gb: f64) -> Candidate {
        Candidate {
            id: title.into(),
            title: title.into(),
            indexer_name: "t".into(),
            size_bytes: Some((gb * 1e9) as u64),
            seeders: Some(seeders),
            leechers: None,
            published: None,
            categories: vec![],
            info_url: None,
            info_hash: None,
            handle: CandidateHandle::default(),
        }
    }

    #[test]
    fn formats() {
        assert_eq!(guess_format("Artist - Album (2019) [FLAC 24/96]").as_deref(), Some("FLAC 24/96"));
        assert_eq!(guess_format("Artist - Album MP3 320 kbps").as_deref(), Some("MP3 320"));
        assert_eq!(guess_format("Artist - Album"), None);
    }

    #[test]
    fn format_words_are_not_found_inside_other_words() {
        assert_eq!(guess_format("Artist - New Wave Hits MP3 320kbps").as_deref(), Some("MP3 320"));
        assert_eq!(guess_format("Buckingham Palace - Tape"), None);
        assert!(has_stem("remastered 2011", "remaster"));
        assert!(!has_stem("olive tree", "live"));
        assert!(!has_stem("alive 2007", "live"));
    }

    #[test]
    fn prefers_flac_studio_over_live_and_discography() {
        let c = vec![
            cand("Artist - Album (2019) MP3 320", 200, 0.1),
            cand("Artist - Album (Live) FLAC", 80, 0.5),
            cand("Artist - Discography 1990-2020 FLAC", 300, 40.0),
            cand("Artist - Album (2019) FLAC", 40, 0.4),
        ];
        assert_eq!(recommend(&c, "Artist Album"), Some(3));
    }
}
