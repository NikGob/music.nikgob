//! YouTube Music's own suggestions, read with the
//! user's cookies: the home (`FEmusic_home`), whose «Быстрый выбор» /
//! «Quick picks» and «Слушать снова» / «Listen again» shelves say what the
//! user plays, and the history (`FEmusic_history`). The other shelves of the
//! home (mixes, charts, new releases) are left alone on purpose.
//!
//! YouTube changes this markup now and then, so everything is read
//! defensively: shelves are found by their renderer anywhere in the answer,
//! told apart by known titles in Russian and English and, failing that, by
//! their make-up (quick picks are the grid of song rows, `numItemsPerColumn`;
//! listen again is the shelf whose header carries the user's avatar), and
//! an item missing a part is skipped, never an error.

use rustypipe::client::RustyPipe;
use serde_json::{Value, json};

use nikgob_core::streaming::{
    AlbumRef, ArtistRef, StreamError, StreamTrack, SuggestedArtist, SuggestedTrack, SuggestionKind, Suggestions,
    TrackKind, rank_weight,
};

use crate::convert::{rid, upscale};
use crate::innertube::{call, text};

/// History items read (newest first).
const HISTORY_ITEMS: usize = 50;
/// Items read from one home shelf.
const SHELF_ITEMS: usize = 40;
/// An album card of «Слушать снова» says this much about its artist,
/// against an artist card's 1.
const ALBUM_ARTIST_SHARE: f32 = 0.6;

/// Titles of the quick picks shelf, folded ([`fold`]).
const QUICK_PICKS: &[&str] = &["quick picks", "быстрый выбор", "быстрая подборка"];
/// Titles of the listen again shelf, folded.
const LISTEN_AGAIN: &[&str] = &["listen again", "слушать снова", "послушать снова", "снова слушать", "прослушать снова"];

/// The home: quick picks and listen again.
pub(crate) async fn home(rp: &RustyPipe) -> Result<Suggestions, StreamError> {
    let v = call(rp, "browse", json!({ "browseId": "FEmusic_home" })).await?;
    Ok(parse_home(&v))
}

/// The history: what was played lately, newest first.
pub(crate) async fn history(rp: &RustyPipe) -> Result<Suggestions, StreamError> {
    let v = call(rp, "browse", json!({ "browseId": "FEmusic_history" })).await?;
    Ok(parse_history(&v))
}

fn fold(s: &str) -> String {
    s.trim().chars().flat_map(char::to_lowercase).map(|c| if c == 'ё' { 'е' } else { c }).collect()
}

/// Every object under `key`, depth first (not looking inside a found one).
fn collect<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, c) in map {
                if k == key {
                    out.push(c);
                } else {
                    collect(c, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|c| collect(c, key, out)),
        _ => {}
    }
}

/// The first value under `key`, depth first.
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map.get(key).or_else(|| map.values().find_map(|c| find(c, key))),
        Value::Array(a) => a.iter().find_map(|c| find(c, key)),
        _ => None,
    }
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    find(v, key).and_then(Value::as_str)
}

/// A shelf of the home.
struct Shelf<'a> {
    title: String,
    /// The header carries a picture (the user's avatar on listen again).
    avatar: bool,
    /// Rows in columns, the make-up of quick picks.
    grid: bool,
    items: Vec<&'a Value>,
}

fn shelf(v: &Value) -> Shelf<'_> {
    let header = v.get("header");
    let title = header
        .and_then(|h| find(h, "title"))
        .and_then(text)
        .unwrap_or_default();
    let avatar = header.is_some_and(|h| find(h, "strapline").is_some() && find(h, "thumbnail").is_some());
    let items: Vec<&Value> = v
        .get("contents")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let grid = v.get("numItemsPerColumn").is_some()
        && items.iter().any(|i| i.get("musicResponsiveListItemRenderer").is_some());
    Shelf {
        title: fold(&title),
        avatar,
        grid,
        items,
    }
}

/// Quick picks and listen again of a home answer.
pub(crate) fn parse_home(v: &Value) -> Suggestions {
    let mut found = Vec::new();
    collect(v, "musicCarouselShelfRenderer", &mut found);
    let shelves: Vec<Shelf<'_>> = found.into_iter().map(shelf).collect();
    let titled = |names: &[&str]| shelves.iter().position(|s| names.contains(&s.title.as_str()));
    let quick = titled(QUICK_PICKS).or_else(|| shelves.iter().position(|s| s.grid));
    let again = titled(LISTEN_AGAIN).or_else(|| shelves.iter().position(|s| s.avatar && !s.grid)).filter(|i| Some(*i) != quick);
    let mut out = Suggestions::default();
    if let Some(i) = quick {
        add_shelf(&shelves[i], SuggestionKind::QuickPicks, &mut out);
    }
    if let Some(i) = again {
        add_shelf(&shelves[i], SuggestionKind::ListenAgain, &mut out);
    }
    out
}

fn add_shelf(s: &Shelf<'_>, kind: SuggestionKind, out: &mut Suggestions) {
    let items: Vec<Item> = s.items.iter().filter_map(|i| item(i)).take(SHELF_ITEMS).collect();
    let n = items.len();
    for (rank, it) in items.into_iter().enumerate() {
        let weight = rank_weight(rank, n);
        match it {
            Item::Track(track) => {
                if !out.tracks.iter().any(|t| t.track.id == track.id) {
                    out.tracks.push(SuggestedTrack { track: *track, kind, weight });
                }
            }
            Item::Artists(artists, share) => {
                for artist in artists {
                    if !out.artists.iter().any(|a| a.artist.name == artist.name) {
                        out.artists.push(SuggestedArtist {
                            artist,
                            kind,
                            weight: weight * share,
                        });
                    }
                }
            }
        }
    }
}

/// The history's tracks, newest first.
pub(crate) fn parse_history(v: &Value) -> Suggestions {
    let mut shelves = Vec::new();
    collect(v, "musicShelfRenderer", &mut shelves);
    let mut rows = Vec::new();
    if shelves.is_empty() {
        collect(v, "musicResponsiveListItemRenderer", &mut rows);
    } else {
        for s in shelves {
            collect(s, "musicResponsiveListItemRenderer", &mut rows);
        }
    }
    let mut tracks: Vec<StreamTrack> = Vec::new();
    for r in rows {
        if let Some(t) = row_track(r)
            && !tracks.iter().any(|x| x.id == t.id)
        {
            tracks.push(t);
            if tracks.len() == HISTORY_ITEMS {
                break;
            }
        }
    }
    let n = tracks.len();
    Suggestions {
        tracks: tracks
            .into_iter()
            .enumerate()
            .map(|(rank, track)| SuggestedTrack {
                track,
                kind: SuggestionKind::Recent,
                weight: rank_weight(rank, n),
            })
            .collect(),
        ..Suggestions::default()
    }
}

/// What a card of a shelf stands for.
enum Item {
    Track(Box<StreamTrack>),
    /// Artists, with how much the card says about them.
    Artists(Vec<ArtistRef>, f32),
}

fn item(v: &Value) -> Option<Item> {
    if let Some(r) = v.get("musicResponsiveListItemRenderer") {
        return row_track(r).map(|t| Item::Track(Box::new(t)));
    }
    let r = v.get("musicTwoRowItemRenderer")?;
    two_row(r)
}

/// What a link opens: `MUSIC_PAGE_TYPE_ARTIST`, `…_ALBUM`, `…_PLAYLIST`.
fn page_type(v: &Value) -> Option<&str> {
    str_at(v, "pageType")
}

/// An artist or an album of a run's link.
enum Link {
    Artist(ArtistRef),
    Album(AlbumRef),
    Other,
}

fn link(run: &Value) -> Option<Link> {
    let browse = run.get("navigationEndpoint")?.get("browseEndpoint")?;
    let id = browse.get("browseId").and_then(Value::as_str)?;
    let name = run.get("text").and_then(Value::as_str)?.trim();
    if name.is_empty() {
        return None;
    }
    let kind = page_type(browse);
    Some(match kind {
        Some("MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL") => Link::Artist(ArtistRef::new(Some(rid(id)), name)),
        Some("MUSIC_PAGE_TYPE_ALBUM") => Link::Album(AlbumRef::new(Some(rid(id)), name)),
        None if id.starts_with("UC") => Link::Artist(ArtistRef::new(Some(rid(id)), name)),
        None if id.starts_with("MPRE") => Link::Album(AlbumRef::new(Some(rid(id)), name)),
        _ => Link::Other,
    })
}

/// Words that label a run instead of naming an artist («Песня • Кино»).
const LABELS: &[&str] = &[
    "song", "video", "episode", "single", "album", "ep", "песня", "видео", "клип", "выпуск", "сингл", "альбом", "мини-альбом",
];

fn is_separator(s: &str) -> bool {
    s.trim().is_empty() || matches!(s.trim(), "•" | "·" | "&" | "," | "и" | "and")
}

/// The artists and the album of subtitle runs. Without linked artists, the
/// first plain run that is no label or count stands for the artist.
fn credits(runs: &[Value]) -> (Vec<ArtistRef>, Option<AlbumRef>) {
    let mut artists = Vec::new();
    let mut album = None;
    for r in runs {
        match link(r) {
            Some(Link::Artist(a)) => artists.push(a),
            Some(Link::Album(a)) => album = album.or(Some(a)),
            _ => {}
        }
    }
    if artists.is_empty() {
        let plain = runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).find(|t| {
            let f = fold(t);
            !is_separator(t) && !LABELS.contains(&f.as_str()) && !f.chars().next().is_some_and(|c| c.is_ascii_digit())
        });
        if let Some(name) = plain {
            artists.push(ArtistRef::new(None, name.trim()));
        }
    }
    (artists, album)
}

/// ATV songs, music videos (official or not), podcast episodes.
fn kind_of(v: &Value) -> TrackKind {
    match str_at(v, "musicVideoType") {
        None | Some("MUSIC_VIDEO_TYPE_ATV") => TrackKind::Song,
        Some("MUSIC_VIDEO_TYPE_PODCAST_EPISODE") => TrackKind::Upload,
        Some(_) => TrackKind::Video,
    }
}

/// The largest picture of a thumbnail list under `v`.
fn cover(v: &Value) -> Option<String> {
    let thumbs = find(v, "thumbnails")?.as_array()?;
    let best = thumbs.iter().max_by_key(|t| {
        let w = t.get("width").and_then(Value::as_u64).unwrap_or(0);
        let h = t.get("height").and_then(Value::as_u64).unwrap_or(0);
        w * h
    })?;
    let url = best.get("url").and_then(Value::as_str)?;
    let dim = |k: &str| best.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    Some(upscale(url, dim("width"), dim("height")))
}

/// "3:45", "1:02:03" → milliseconds.
fn duration_ms(s: &str) -> Option<u64> {
    let mut total = 0u64;
    let parts: Vec<&str> = s.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    for p in parts {
        total = total * 60 + p.trim().parse::<u64>().ok()?;
    }
    Some(total * 1000)
}

fn runs_of(v: Option<&Value>) -> Vec<Value> {
    v.and_then(|t| t.get("runs")).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// A track of a list row (quick picks, history): its video id, title,
/// credits, length and picture.
fn row_track(r: &Value) -> Option<StreamTrack> {
    let id = r
        .get("playlistItemData")
        .and_then(|d| d.get("videoId"))
        .and_then(Value::as_str)
        .or_else(|| find(r, "watchEndpoint").and_then(|w| w.get("videoId")).and_then(Value::as_str))?;
    let columns: Vec<&Value> = r
        .get("flexColumns")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|c| c.get("musicResponsiveListItemFlexColumnRenderer").and_then(|c| c.get("text")))
        .collect();
    let title = columns.first().and_then(|t| text(t))?;
    let runs: Vec<Value> = columns.iter().skip(1).flat_map(|c| runs_of(Some(c))).collect();
    let (artists, album) = credits(&runs);
    let mut t = StreamTrack::new(rid(id), title.trim());
    t.artists = artists;
    t.album = album;
    t.kind = kind_of(r);
    t.duration_ms = r
        .get("fixedColumns")
        .and_then(|c| find(c, "text"))
        .and_then(text)
        .and_then(|s| duration_ms(&s));
    t.cover_url = r.get("thumbnail").and_then(cover);
    Some(t)
}

/// A card of listen again: a song or a video to play, an artist, or an
/// album (its artists); playlists and mixes say nothing about one artist.
fn two_row(r: &Value) -> Option<Item> {
    let title = r.get("title").and_then(text)?;
    let title = title.trim();
    let nav = r.get("navigationEndpoint")?;
    let subtitle = runs_of(r.get("subtitle"));
    if let Some(id) = nav.get("watchEndpoint").and_then(|w| w.get("videoId")).and_then(Value::as_str) {
        let (artists, album) = credits(&subtitle);
        let mut t = StreamTrack::new(rid(id), title);
        t.artists = artists;
        t.album = album;
        t.kind = kind_of(nav);
        t.cover_url = r.get("thumbnailRenderer").and_then(cover);
        return Some(Item::Track(Box::new(t)));
    }
    let browse = nav.get("browseEndpoint")?;
    let id = browse.get("browseId").and_then(Value::as_str)?;
    match page_type(browse) {
        Some("MUSIC_PAGE_TYPE_ARTIST") => Some(Item::Artists(vec![ArtistRef::new(Some(rid(id)), title)], 1.0)),
        Some("MUSIC_PAGE_TYPE_ALBUM") => {
            let (artists, _) = credits(&subtitle);
            (!artists.is_empty()).then_some(Item::Artists(artists, ALBUM_ARTIST_SHARE))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    fn titles(s: &Suggestions, kind: SuggestionKind) -> Vec<&str> {
        s.tracks.iter().filter(|t| t.kind == kind).map(|t| t.track.title.as_str()).collect()
    }

    #[test]
    fn the_english_home_gives_quick_picks_and_listen_again() {
        let s = parse_home(&fixture("ytm_home_en.json"));
        assert_eq!(titles(&s, SuggestionKind::QuickPicks), ["Paper Lanterns", "Night Bus", "Glass Harbour (Official Video)"]);
        let first = &s.tracks[0];
        assert_eq!(first.track.id.id, "qp000000001");
        assert_eq!(first.track.artists[0].name, "The Lanterns");
        assert_eq!(first.track.artists[0].id.as_ref().unwrap().id, "UCartist000000000000001");
        assert_eq!(first.track.album.as_ref().unwrap().title, "Small Hours");
        assert_eq!(first.track.kind, TrackKind::Song);
        assert_eq!(first.weight, 1.0);
        assert!(first.track.cover_url.as_deref().unwrap().contains("w544-h544"), "{:?}", first.track.cover_url);
        let video = s.tracks.iter().find(|t| t.track.id.id == "qp000000003").unwrap();
        assert_eq!(video.track.kind, TrackKind::Video);
        assert_eq!(video.weight, 0.5);
        // Listen again: a song, an artist, an album's artist; the playlist
        // and the mixes and charts shelves are not suggestions.
        assert_eq!(titles(&s, SuggestionKind::ListenAgain), ["Old Favourite"]);
        let artists: Vec<(&str, SuggestionKind)> = s.artists.iter().map(|a| (a.artist.name.as_str(), a.kind)).collect();
        assert_eq!(artists, [("Night Owls", SuggestionKind::ListenAgain), ("Harbour Lights", SuggestionKind::ListenAgain)]);
        assert!(s.artists[1].weight < s.artists[0].weight, "an album says less than the artist's own card");
        assert!(!s.tracks.iter().any(|t| t.track.title.contains("Chart")), "charts are left alone");
        assert!(!s.needs_reconnect);
    }

    #[test]
    fn the_russian_home_reads_the_same() {
        let s = parse_home(&fixture("ytm_home_ru.json"));
        assert_eq!(titles(&s, SuggestionKind::QuickPicks), ["Бумажные фонари", "Ночной автобус"]);
        assert_eq!(s.tracks[0].track.artists[0].name, "Фонари");
        assert_eq!(s.tracks[1].track.artists[0].name, "Сова", "a credit without a link");
        assert_eq!(titles(&s, SuggestionKind::ListenAgain), ["Старое любимое"]);
        assert_eq!(s.artists.len(), 1);
        assert_eq!(s.artists[0].artist.name, "Ночные совы");
    }

    #[test]
    fn unknown_titles_are_found_by_their_make_up() {
        // German titles: the grid of rows is quick picks, the header with
        // the avatar is listen again.
        let s = parse_home(&fixture("ytm_home_de.json"));
        assert_eq!(titles(&s, SuggestionKind::QuickPicks), ["Papierlaterne"]);
        assert_eq!(titles(&s, SuggestionKind::ListenAgain), ["Alter Liebling"]);
    }

    #[test]
    fn odd_answers_give_nothing_rather_than_fail() {
        assert_eq!(parse_home(&json!({})), Suggestions::default());
        assert_eq!(parse_history(&json!({"contents": []})), Suggestions::default());
        // A row without a video id or a title is skipped.
        let v = json!({"contents": [{"musicCarouselShelfRenderer": {
            "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Quick picks"}]}}},
            "numItemsPerColumn": "4",
            "contents": [
                {"musicResponsiveListItemRenderer": {"flexColumns": []}},
                {"musicResponsiveListItemRenderer": {"playlistItemData": {"videoId": "x"}, "flexColumns": []}},
                {"somethingNew": {}}
            ]
        }}]});
        assert!(parse_home(&v).tracks.is_empty());
    }

    #[test]
    fn the_history_is_recent_newest_first() {
        let s = parse_history(&fixture("ytm_history.json"));
        let got: Vec<(&str, &str, Option<u64>)> = s
            .tracks
            .iter()
            .map(|t| (t.track.title.as_str(), t.track.artists[0].name.as_str(), t.track.duration_ms))
            .collect();
        assert_eq!(
            got,
            [
                ("Night Bus", "Night Owls", Some(225_000)),
                ("Paper Lanterns", "The Lanterns", Some(201_000)),
                ("Long Song", "Slow Band", Some(3_723_000)),
            ],
            "the repeat of Night Bus yesterday counts once"
        );
        assert!(s.tracks.iter().all(|t| t.kind == SuggestionKind::Recent));
        assert_eq!(s.tracks[0].weight, 1.0);
        assert!(s.artists.is_empty());
    }

    #[test]
    fn small_parts() {
        assert_eq!(duration_ms("3:45"), Some(225_000));
        assert_eq!(duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(duration_ms("live"), None);
        assert_eq!(fold(" Быстрый Выбор "), "быстрый выбор");
        assert_eq!(fold("Ёлка"), "елка");
    }
}
