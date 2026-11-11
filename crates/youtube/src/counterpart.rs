//! The song of a music video ([`StreamService::song_for_video`]).
//!
//! YouTube Music links an official video (OMV) to its song (ATV): the
//! «Песня / Видео» switch of its own clients, the `counterpart` of the
//! video's `playlistPanelVideoWrapperRenderer` in a `next` answer (YouTube
//! gives it to a signed-in session; without the login the queue holds the
//! video alone). [`parse_watch`] reads it. When there is none, a song
//! search finds it: the video's title cleaned of what clips add («Official
//! Video», «MV», «(Lyrics)», a leading «Artist - »), both halves of a dual
//! title («インソムニア - Insomnia») tried ([`nikgob_core::clip_title`]), and a hit taken only by the same
//! main artist, the same folded title and a length that fits a clip: the
//! song up to [`SHORTER_MS`] shorter than the video (an intro, an outro, a
//! scene), never more than [`LONGER_MS`] longer ([`pick_song`]).
//!
//! [`StreamService::song_for_video`]: nikgob_core::streaming::StreamService::song_for_video

use nikgob_core::streaming::{AlbumRef, ArtistRef, StreamTrack, TrackKind};
use serde_json::Value;

use nikgob_core::clip_title::{fold, song_titles};

use crate::convert;

/// How much shorter than its video a song may be.
pub(crate) const SHORTER_MS: u64 = 60_000;
/// How much longer than its video a song may be.
pub(crate) const LONGER_MS: u64 = 3_000;

/// What a `next` answer says about one video.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Watch {
    /// The item itself.
    pub(crate) item: StreamTrack,
    /// Its song, when the item is a video YouTube Music links to one.
    pub(crate) counterpart: Option<StreamTrack>,
}

/// The item `id` of a `next` answer (searched by key: the queue's
/// layout moves now and then) and its counterpart.
pub(crate) fn parse_watch(v: &Value, id: &str) -> Option<Watch> {
    let mut found = None;
    walk(v, &mut |map| {
        if found.is_some() {
            return;
        }
        if let Some(w) = map.get("playlistPanelVideoWrapperRenderer") {
            let primary = w
                .get("primaryRenderer")
                .and_then(|p| p.get("playlistPanelVideoRenderer"))
                .and_then(panel_track);
            if let Some(item) = primary.filter(|t| t.id.id == id) {
                let counterpart = w
                    .get("counterpart")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        c.get("counterpartRenderer")
                            .and_then(|r| r.get("playlistPanelVideoRenderer"))
                            .and_then(panel_track)
                    })
                    .find(|t| t.kind == TrackKind::Song);
                found = Some(Watch { item, counterpart });
            }
        } else if let Some(r) = map.get("playlistPanelVideoRenderer")
            && let Some(item) = panel_track(r).filter(|t| t.id.id == id)
        {
            found = Some(Watch {
                item,
                counterpart: None,
            });
        }
    });
    found
}

/// Calls `f` on every object, depth first.
fn walk(v: &Value, f: &mut impl FnMut(&serde_json::Map<String, Value>)) {
    match v {
        Value::Object(map) => {
            f(map);
            map.values().for_each(|c| walk(c, f));
        }
        Value::Array(a) => a.iter().for_each(|c| walk(c, f)),
        _ => {}
    }
}

fn runs(v: Option<&Value>) -> Vec<&Value> {
    v.and_then(|t| t.get("runs"))
        .and_then(Value::as_array)
        .map(|r| r.iter().collect())
        .unwrap_or_default()
}

fn text_of(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return Some(s.to_owned());
    }
    let s: String = runs(Some(v))
        .iter()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .collect();
    (!s.is_empty()).then_some(s)
}

/// "3:27", "1:02:03" → ms.
fn duration_ms(s: &str) -> Option<u64> {
    let mut secs = 0u64;
    for part in s.trim().split(':') {
        secs = secs * 60 + part.trim().parse::<u64>().ok()?;
    }
    Some(secs * 1000)
}

fn browse(run: &Value) -> Option<(&str, &str)> {
    let b = run.get("navigationEndpoint")?.get("browseEndpoint")?;
    let id = b.get("browseId")?.as_str()?;
    let page = b
        .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
        .and_then(Value::as_str)
        .unwrap_or("");
    Some((id, page))
}

/// A `playlistPanelVideoRenderer` as a track: the byline is
/// "Artist, Artist • Album • 2020" for a song, "Artist • 20M views" for a
/// video.
fn panel_track(r: &Value) -> Option<StreamTrack> {
    let id = r.get("videoId").and_then(Value::as_str)?;
    let title = text_of(r.get("title"))?;
    let mut t = StreamTrack::new(convert::rid(id), title);
    let kind = r
        .pointer("/navigationEndpoint/watchEndpoint/watchEndpointMusicSupportedConfigs/watchEndpointMusicConfig/musicVideoType")
        .and_then(Value::as_str);
    t.kind = match kind {
        Some("MUSIC_VIDEO_TYPE_ATV") => TrackKind::Song,
        Some("MUSIC_VIDEO_TYPE_PODCAST_EPISODE") => TrackKind::Upload,
        _ => TrackKind::Video,
    };
    let byline = runs(r.get("longBylineText"));
    let mut in_artists = true;
    for run in byline {
        let text = run.get("text").and_then(Value::as_str).unwrap_or("");
        match browse(run) {
            Some((bid, page)) if page == "MUSIC_PAGE_TYPE_ALBUM" || bid.starts_with("MPRE") => {
                t.album = Some(AlbumRef::new(Some(convert::rid(bid)), text));
                in_artists = false;
            }
            Some((bid, _)) if in_artists => {
                t.artists.push(ArtistRef::new(Some(convert::rid(bid)), text));
            }
            _ if text.trim() == "•" => in_artists = false,
            // The tail of a cut list («и ещё 1 исполнитель»): no artist.
            _ if in_artists && !t.artists.is_empty() && nikgob_core::streaming::more_artists_note(text) => {}
            _ if in_artists && !matches!(text.trim(), "" | "," | "&") => {
                t.artists.push(ArtistRef::new(None, text.trim()));
            }
            _ => {}
        }
    }
    if let Some(a) = t.album.as_mut() {
        a.artists = t.artists.clone();
    }
    t.duration_ms = text_of(r.get("lengthText")).as_deref().and_then(duration_ms);
    t.cover_url = r
        .pointer("/thumbnail/thumbnails")
        .and_then(Value::as_array)
        .and_then(|thumbs| {
            thumbs
                .iter()
                .filter_map(|th| {
                    let url = th.get("url")?.as_str()?;
                    let w = th.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    let h = th.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    Some((w * h, url, w, h))
                })
                .max_by_key(|(area, ..)| *area)
                .map(|(_, url, w, h)| convert::upscale(url, w, h))
        });
    Some(t)
}

// ------------------------------------------------------------------ titles

/// Song searches for a clip, best first: "artist half" for each half of
/// a dual title (a Latin half first: YouTube Music finds romanized titles
/// best), at most two.
pub(crate) fn queries(video: &StreamTrack) -> Vec<String> {
    let artist = main_artist(video);
    let titles = song_titles(&video.title, &artist);
    let mut halves: Vec<&String> = if titles.len() > 1 { titles[1..].iter().collect() } else { titles.iter().collect() };
    halves.sort_by_key(|h| !h.is_ascii());
    halves
        .into_iter()
        .take(2)
        .map(|h| if artist.is_empty() { h.clone() } else { format!("{artist} {h}") })
        .collect()
}

fn main_artist(t: &StreamTrack) -> String {
    t.artists.first().map(|a| a.name.clone()).unwrap_or_default()
}

/// The song among `hits` for the clip `video` (see the module docs).
pub(crate) fn pick_song(hits: &[StreamTrack], video: &StreamTrack) -> Option<StreamTrack> {
    let artist = fold(&main_artist(video));
    if artist.is_empty() {
        return None;
    }
    let want: Vec<String> = song_titles(&video.title, &main_artist(video))
        .iter()
        .map(|t| fold(t))
        .filter(|t| !t.is_empty())
        .collect();
    hits.iter()
        .filter(|h| h.kind == TrackKind::Song && h.playable && h.id != video.id)
        .filter(|h| h.artists.iter().any(|a| fold(&a.name) == artist))
        .filter(|h| match (h.duration_ms, video.duration_ms) {
            (Some(song), Some(clip)) => song <= clip + LONGER_MS && song + SHORTER_MS >= clip,
            (None, _) => false,
            (Some(_), None) => true,
        })
        .find(|h| {
            song_titles(&h.title, &main_artist(h))
                .iter()
                .any(|t| want.contains(&fold(t)))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{RemoteId, Service};

    fn track(id: &str, title: &str, artist: &str, secs: u64, kind: TrackKind) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = Some(secs * 1000);
        t.kind = kind;
        t
    }

    #[test]
    fn searches_try_each_half_latin_first() {
        let v = track("v", "インソムニア - Insomnia", "Eve", 241, TrackKind::Video);
        assert_eq!(queries(&v), ["Eve Insomnia", "Eve インソムニア"]);
        let v = track("v", "Eve - Ghost Avenue (Official Video)", "Eve", 200, TrackKind::Video);
        assert_eq!(queries(&v), ["Eve Ghost Avenue"]);
    }

    #[test]
    fn the_song_has_the_artist_the_title_and_a_clip_length() {
        let clip = track("35nV_M3asRs", "インソムニア - Insomnia", "Eve", 241, TrackKind::Video);
        let song = track("0VlKjNUmUKE", "Insomnia", "Eve", 207, TrackKind::Song);
        let other = track("dEfl", "Dramaturgy", "Eve", 239, TrackKind::Song);
        let remix = track("rmx", "Insomnia (Remix)", "Eve", 230, TrackKind::Song);
        let cover = track("cov", "Insomnia", "Trickle", 240, TrackKind::Song);
        let video = track("v2", "Insomnia", "Eve", 240, TrackKind::Video);
        let hits = [other.clone(), remix.clone(), cover, video, song.clone()];
        assert_eq!(pick_song(&hits, &clip).unwrap().id.id, "0VlKjNUmUKE");
        // The Japanese half matches too.
        let jp = track("jp", "インソムニア", "Eve", 230, TrackKind::Song);
        assert_eq!(pick_song(&[jp], &clip).unwrap().id.id, "jp");
        // Up to a minute shorter, never more than 3 s longer.
        let short = track("s", "Insomnia", "Eve", 180, TrackKind::Song);
        assert!(pick_song(&[short], &clip).is_none(), "61 s shorter");
        let long = track("l", "Insomnia", "Eve", 245, TrackKind::Song);
        assert!(pick_song(&[long], &clip).is_none(), "4 s longer");
        let edge = track("e", "Insomnia", "Eve", 244, TrackKind::Song);
        assert!(pick_song(&[edge], &clip).is_some(), "3 s longer");
        assert!(pick_song(&[remix, other], &clip).is_none());
        let natsuko = track("n", "夏子 - Natsuko", "Funky Monkey Bλby's", 300, TrackKind::Video);
        let song = track("ns", "夏子", "Funky Monkey Bλby's", 285, TrackKind::Song);
        assert_eq!(pick_song(&[song], &natsuko).unwrap().id.id, "ns");
    }

    #[test]
    fn an_artist_music_video_tail_finds_the_song() {
        let clip = track("aiyou-v", "AIYOU - Eve Music Video", "Eve", 165, TrackKind::Video);
        assert_eq!(queries(&clip), ["Eve AIYOU"]);
        let song = track("aiyou", "AIYOU", "Eve", 160, TrackKind::Song);
        assert_eq!(pick_song(&[song], &clip).unwrap().id.id, "aiyou");
        let clip = track("v", "AIYOU MV", "Eve", 165, TrackKind::Video);
        assert_eq!(queries(&clip), ["Eve AIYOU"]);
    }

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/next_counterpart.json")).unwrap()
    }

    #[test]
    fn a_next_answer_gives_the_video_and_its_song() {
        let w = parse_watch(&fixture(), "35nV_M3asRs").unwrap();
        assert_eq!(w.item.title, "インソムニア - Insomnia");
        assert_eq!(w.item.kind, TrackKind::Video);
        assert_eq!(w.item.artist_line(), "Eve");
        assert_eq!(w.item.duration_ms, Some(241_000));
        assert_eq!(w.item.album, None);
        let song = w.counterpart.unwrap();
        assert_eq!(song.id.id, "0VlKjNUmUKE");
        assert_eq!(song.title, "Insomnia");
        assert_eq!(song.kind, TrackKind::Song);
        assert_eq!(song.duration_ms, Some(207_000));
        assert_eq!(song.artists[0].id.as_ref().unwrap().id, "UCXW81UHV3UeWUjVMr11ov2A");
        let album = song.album.unwrap();
        assert_eq!(album.title, "Under Blue");
        assert_eq!(album.id.unwrap().id, "MPREb_eve_underblue");
        assert_eq!(
            song.cover_url.as_deref(),
            Some("https://lh3.googleusercontent.com/under-blue=w544-h544-l90-rj")
        );
        // Another video of the queue is not this one.
        assert!(parse_watch(&fixture(), "nope").is_none());
    }

    #[test]
    fn a_next_answer_without_a_counterpart_gives_the_video_alone() {
        let v: Value = serde_json::from_str(include_str!("../tests/fixtures/next_video_only.json")).unwrap();
        let w = parse_watch(&v, "35nV_M3asRs").unwrap();
        assert_eq!(w.item.kind, TrackKind::Video);
        assert_eq!(w.counterpart, None);
        assert!(w.item.cover_url.as_deref().unwrap().starts_with("https://i.ytimg.com/vi/35nV_M3asRs/"));
        assert_eq!(duration_ms("1:02:03"), Some(3_723_000));
    }
}
