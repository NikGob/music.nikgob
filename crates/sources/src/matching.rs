//! Whether an offer is the wished track or album (docs/stage4.5.md): the
//! wish is the reference. An offer must name the artist (any of its
//! spellings) — another artist's version is vetoed, never a cover, a
//! karaoke or a tribute unless asked for —, and the album or the title. A
//! torrent of a soundtrack may name the game instead of its composer.
//!
//! An offer is *sure* when nothing about it is doubtful: a catalogue track
//! of the artist with the title and the length (or the ISRC), an album of
//! the artist with the album's name, a lossless seeded torrent naming the
//! album or the title, or the artist's discography or collection (its file
//! is looked for when the torrent's list comes, and it is skipped when it
//! is not there).

use nikgob_core::source::{Offer, OfferScope, Want, WantScope};
use nikgob_metadata::text::{artist_key, similarity, title_key};

use crate::text::{album_keys, artist_keys, franchise_of, is_soundtrack, title_keys};

/// Artist names this alike are one.
const ARTIST: f64 = 0.85;
/// Titles this alike are one song.
const TITLE: f64 = 0.85;
/// Lengths this far apart (ms) are one recording.
const SLACK_MS: u32 = 3_000;
/// Lengths this far apart are another recording for sure.
const OTHER_MS: u32 = 15_000;

/// Words of another version: refused unless the wish has them too.
const OTHER_VERSION: &[&str] = &[
    "cover", "covers", "karaoke", "tribute", "instrumental", "remix", "live", "acoustic", "piano version",
    "guitar", "8-bit", "8 bit", "lofi", "lo-fi", "nightcore", "sped up", "speed up", "slowed", "кавер", "караоке",
    "минус", "акустика", "カバー", "歌ってみた", "弾いてみた",
];

/// How good a match is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// 0..=1, how much of the wish the offer names.
    pub score: f64,
    pub sure: bool,
}

/// The folded words of `needle` all occur in `hay`.
pub fn contains_words(hay: &str, needle: &str) -> bool {
    !needle.is_empty() && needle.split(' ').all(|w| hay.split(' ').any(|h| h == w))
}

/// `w` in `hay` not glued to letters on either side (lowercase both).
fn has_word(hay: &str, w: &str) -> bool {
    hay.match_indices(w).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + w.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// A word of another version in `text` that the wish does not have.
pub fn other_version(text: &str, want_text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    let w = want_text.to_lowercase();
    OTHER_VERSION.iter().copied().find(|v| has_word(&t, v) && !has_word(&w, v))
}

/// The artist's name in a free text (a torrent title): its words, or the
/// head before " - " alike.
fn names_artist(hay_key: &str, raw: &str, keys: &[String]) -> bool {
    let head = raw.split(" - ").next().map(|h| title_key(h).base).unwrap_or_default();
    keys.iter().any(|k| contains_words(hay_key, k) || similarity(&head, k) >= ARTIST)
}

/// A credited artist alike one of the wish's.
pub fn same_artist(name: &str, want: &Want) -> bool {
    let got = artist_key(name);
    if got.is_empty() {
        return false;
    }
    let keys = artist_keys(want);
    keys.iter().any(|k| similarity(&got, k) >= ARTIST || contains_words(&got, k))
        || want.artist_names().any(|n| nikgob_core::names::same_name(n, name))
        // "A & B" credited as one name: the main artist is one of them.
        || name.split([',', '&', '/']).any(|part| {
            let p = artist_key(part);
            !p.is_empty() && keys.iter().any(|k| similarity(&p, k) >= ARTIST)
        })
}

/// A compilation's artist line ("Various Artists", «Разные исполнители»).
fn various(name: &str) -> bool {
    !name.trim().is_empty() && crate::text::main_artist(name).is_empty()
}

fn best_title(got: &str, want: &[String]) -> f64 {
    let got_keys = title_keys(got);
    got_keys
        .iter()
        .flat_map(|g| want.iter().map(move |w| similarity(g, w)))
        .fold(0.0, f64::max)
}

/// The verdict on `offer` for `want`, `None` when it is not it.
pub fn judge(offer: &Offer, want: &Want) -> Option<Verdict> {
    match offer.source {
        nikgob_core::source::SourceKey::Torrents => judge_release(offer, want),
        _ if offer.artist.is_some() => judge_catalogue(offer, want),
        _ => judge_release(offer, want),
    }
}

/// The performer and the song of an upload titled "Performer - Song" by
/// someone else («Gg2» uploading «Комсомольск - Мозги & деньги»).
fn performer_in_title(title: &str) -> Option<(&str, &str)> {
    let (head, tail) = title.split_once(" - ")?;
    (!head.trim().is_empty() && !tail.trim().is_empty()).then(|| (head.trim(), tail.trim()))
}

/// A wish for a sped-up or slowed upload: the release is the song at its
/// own length.
fn altered(title: &str) -> bool {
    ["speed up", "sped up", "speedup", "slowed", "nightcore"].iter().any(|w| has_word(&title.to_lowercase(), w))
}

/// A catalogue entry: artist, title, album and length said apart.
fn judge_catalogue(offer: &Offer, want: &Want) -> Option<Verdict> {
    let artist = offer.artist.as_deref().unwrap_or_default();
    let soundtrack = want.album.as_deref().is_some_and(is_soundtrack);
    let artist_ok = same_artist(artist, want);
    // The song's performer named in the wish's title: never sure, the
    // user confirms it.
    let performer = (!artist_ok)
        .then(|| want.title.as_deref().and_then(performer_in_title))
        .flatten()
        .filter(|(head, _)| {
            let (a, b) = (artist_key(head), artist_key(artist));
            !a.is_empty() && !b.is_empty() && similarity(&a, &b) >= ARTIST
        });
    // A compilation of nobody in particular ("Various Artists").
    let nobody = artist_keys(want).is_empty();
    if !artist_ok && !soundtrack && performer.is_none() && !nobody {
        return None;
    }
    let want_text = format!("{} {}", want.title.as_deref().unwrap_or_default(), want.album.as_deref().unwrap_or_default());
    match (want.scope, offer.scope) {
        (WantScope::Track, OfferScope::Track) => {
            let title = want.title.as_deref()?;
            let got = offer.track_title.as_deref().unwrap_or(&offer.title);
            if other_version(got, &want_text).is_some() {
                return None;
            }
            let isrc = matches!((&offer.isrc, &want.isrc), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b));
            let sim = best_title(got, &title_keys(performer.map_or(title, |(_, song)| song)));
            if sim < TITLE && !isrc {
                return None;
            }
            let length = match (offer.duration_ms, want.duration_ms) {
                (Some(a), Some(b)) if a.abs_diff(b) > OTHER_MS && !isrc && !altered(title) => return None,
                (Some(a), Some(b)) => a.abs_diff(b) <= SLACK_MS,
                _ => false,
            };
            let album = match (&offer.album, &want.album) {
                (Some(a), Some(b)) => album_keys(b).iter().any(|k| similarity(&title_key(a).base, k) >= TITLE),
                _ => false,
            };
            let score = sim * 0.6 + if length { 0.25 } else { 0.0 } + if album { 0.15 } else { 0.0 };
            Some(Verdict {
                score,
                sure: artist_ok && (isrc || length) && !altered(title),
            })
        }
        (WantScope::Album, OfferScope::Album | OfferScope::Collection) | (WantScope::Track, OfferScope::Album) => {
            let wanted = want.album.as_deref().or(want.title.as_deref())?;
            let got = offer.album.as_deref().unwrap_or(&offer.title);
            if other_version(got, &want_text).is_some() {
                return None;
            }
            let sim = album_keys(got)
                .iter()
                .flat_map(|g| album_keys(wanted).into_iter().map(move |w| similarity(g, &w)))
                .fold(0.0, f64::max);
            if sim < TITLE {
                return None;
            }
            Some(Verdict {
                score: sim * 0.8,
                sure: (artist_ok || (nobody && offer.artist.as_deref().is_some_and(various)))
                    && sim >= 0.95
                    && want.scope == WantScope::Album,
            })
        }
        _ => None,
    }
}

/// A release named in free text (a torrent, a khinsider album).
fn judge_release(offer: &Offer, want: &Want) -> Option<Verdict> {
    let hay = title_key(&offer.title).base;
    let keys = artist_keys(want);
    let want_text = format!("{} {}", want.title.as_deref().unwrap_or_default(), want.album.as_deref().unwrap_or_default());
    if other_version(&offer.title, &want_text).is_some() {
        return None;
    }
    let albums: Vec<String> = want.album.as_deref().map(album_keys).unwrap_or_default();
    let names_album = albums.iter().any(|a| contains_words(&hay, a));
    let titles: Vec<String> = want.title.as_deref().map(title_keys).unwrap_or_default();
    let names_title = want.scope == WantScope::Track && titles.iter().any(|t| contains_words(&hay, t));
    let franchise = want
        .title
        .as_deref()
        .and_then(franchise_of)
        .is_some_and(|f| contains_words(&hay, &title_key(&f).base));
    let soundtrack = want.album.as_deref().is_some_and(is_soundtrack) || franchise;
    let artist = names_artist(&hay, &offer.title, &keys);
    // A soundtrack's torrent names the game, not its composer.
    if !artist && !(soundtrack && (names_album || franchise)) {
        return None;
    }
    let collection = offer.scope == OfferScope::Collection;
    if !names_album && !names_title && !franchise && !collection && !artist {
        return None;
    }
    let seeded = offer.seeders.is_none_or(|s| s > 0);
    let lossless = offer.lossless();
    let named = names_album || names_title || franchise;
    let mut score = 0.0;
    if artist {
        score += 0.4;
    }
    if names_album {
        score += 0.35;
    }
    if names_title {
        score += 0.3;
    }
    if franchise {
        score += 0.2;
    }
    let sure = lossless && seeded && (named || (collection && artist));
    Some(Verdict {
        score: f64::min(score, 1.0),
        sure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::Codec;
    use nikgob_core::source::{Format, SourceKey};

    pub(crate) fn torrent(title: &str, seeders: u32) -> Offer {
        let format = crate::torrent::guess_format(title);
        Offer {
            source: SourceKey::Torrents,
            provider: "t".into(),
            id: title.into(),
            title: title.into(),
            artist: None,
            track_title: None,
            album: None,
            duration_ms: None,
            isrc: None,
            scope: crate::torrent::scope_of(title),
            format,
            size_bytes: Some(400 << 20),
            seeders: Some(seeders),
            info_url: None,
            handle: String::new(),
        }
    }

    fn track(artist: &str, title: &str, secs: u32) -> Offer {
        Offer {
            source: SourceKey::Qobuz,
            provider: "q".into(),
            id: title.into(),
            title: format!("{artist} — {title}"),
            artist: Some(artist.into()),
            track_title: Some(title.into()),
            album: None,
            duration_ms: Some(secs * 1000),
            isrc: None,
            scope: OfferScope::Track,
            format: Some(Format::new(Codec::Flac)),
            size_bytes: None,
            seeders: None,
            info_url: None,
            handle: String::new(),
        }
    }

    fn want(artist: &str, title: &str, album: &str, secs: u32) -> Want {
        Want {
            artists: vec![artist.into()],
            title: Some(title.into()),
            album: Some(album.into()),
            duration_ms: Some(secs * 1000),
            ..Want::default()
        }
    }

    #[test]
    fn a_catalogue_track_is_sure_with_the_artist_title_and_length() {
        let w = want("Eve", "Underdog", "Underdog", 194);
        assert!(judge(&track("Eve", "Underdog", 195), &w).unwrap().sure);
        assert!(!judge(&track("Eve", "Underdog", 199), &w).unwrap().sure, "5 s off: asked");
        assert!(judge(&track("Eve", "Underdog", 260), &w).is_none(), "another recording");
        assert!(judge(&track("Kyle Covers", "Underdog", 194), &w).is_none(), "another artist");
        assert!(judge(&track("Eve", "Underdog (Piano Version)", 194), &w).is_none(), "another version");
        // Two scripts of one title.
        let w = want("Yorushika", "火星人 - Martian", "second person", 233);
        assert!(judge(&track("ヨルシカ", "火星人", 233), &Want { aliases: vec!["ヨルシカ".into()], ..w.clone() }).unwrap().sure);
        assert!(judge(&track("Yorushika", "Martian", 233), &w).unwrap().sure);
    }

    #[test]
    fn transliterated_artists_are_the_artist() {
        let w = want("MORGENSHTERN", "Цветок", "Цветок", 156);
        assert!(judge(&track("МОРГЕНШТЕРН", "Цветок", 156), &w).is_some());
    }

    #[test]
    fn releases_must_name_the_artist_unless_they_are_the_soundtrack() {
        let w = want("Кино", "Война", "Группа крови", 0);
        assert!(judge(&torrent("Кино - Группа крови (1988) [FLAC]", 20), &w).unwrap().sure);
        assert!(judge(&torrent("Ария - Группа крови (cover) FLAC", 50), &w).is_none());
        assert!(!judge(&torrent("Кино - Группа крови (1988) MP3 320", 90), &w).unwrap().sure, "lossy: asked");
        assert!(judge(&torrent("Кино - Дискография (1982-2012) FLAC", 40), &w).unwrap().sure, "its file is looked for");
        let w = want("Yoshihisa Kato", "沈黙の魔女 - Chinmoku no Majo", "Secrets of the Silent Witch Original Soundtrack", 184);
        let ost = judge(&torrent("Secrets of the Silent Witch Original Soundtrack [FLAC]", 5), &w).unwrap();
        assert!(ost.sure, "the game named, not the composer");
        let w = want("Con Fetti Pesto", "Das Nennt Ihr Himmel (Supraland Six Inches Under Credits Song)", "Das Nennt Ihr Himmel", 188);
        assert!(judge(&torrent("Supraland Six Inches Under OST (2022) FLAC", 3), &w).unwrap().sure);
        assert!(judge(&torrent("Supraland OST", 3), &w).is_none(), "another game of the series");
    }
}
