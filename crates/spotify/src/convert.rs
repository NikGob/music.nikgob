//! Web API shapes → core streaming types.

use nikgob_core::model::ReleaseType;
use nikgob_core::streaming::{
    AlbumRef, ArtistRef, RemoteId, Service, StreamAlbum, StreamArtist, StreamPlaylist, StreamTrack,
    TrackKind,
};

use crate::api::models::{self, Image};

/// Covers are asked for at most this wide: plenty for any tile, a fraction
/// of the bytes of Spotify's largest.
pub const COVER_MAX: u32 = 640;

pub fn rid(id: &str) -> RemoteId {
    RemoteId::new(Service::Spotify, id)
}

/// The widest image no wider than [`COVER_MAX`], else the narrowest.
pub fn cover(images: &[Image]) -> Option<String> {
    let usable = images.iter().filter(|i| !i.url.is_empty());
    usable
        .clone()
        .filter(|i| i.width.is_none_or(|w| w <= COVER_MAX))
        .max_by_key(|i| i.width.unwrap_or(0))
        .or_else(|| usable.min_by_key(|i| i.width.unwrap_or(u32::MAX)))
        .map(|i| i.url.clone())
}

/// Upper case, no dashes or spaces; `None` when blank.
pub fn isrc(raw: Option<&str>) -> Option<String> {
    let clean: String = raw?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (!clean.is_empty()).then_some(clean)
}

pub fn year(release_date: Option<&str>) -> Option<i32> {
    release_date?.get(..4)?.parse().ok()
}

/// `album_group` (an artist's listing) wins over `album_type`; "appears on"
/// falls back to the album's own type.
pub fn release_type(album_type: Option<&str>, album_group: Option<&str>) -> ReleaseType {
    let kind = album_group
        .filter(|g| *g != "appears_on")
        .or(album_type)
        .unwrap_or("album");
    match kind.to_ascii_lowercase().as_str() {
        "album" => ReleaseType::Album,
        "single" => ReleaseType::Single,
        "ep" => ReleaseType::Ep,
        "compilation" => ReleaseType::Compilation,
        _ => ReleaseType::Other,
    }
}

fn artists(list: &[models::ArtistRef]) -> Vec<ArtistRef> {
    list.iter()
        .filter(|a| !a.name.is_empty())
        .map(|a| ArtistRef::new(a.id.as_deref().map(rid), a.name.clone()))
        .collect()
}

fn album_ref(album: &models::Album) -> Option<AlbumRef> {
    if album.name.is_empty() {
        return None;
    }
    let mut r = AlbumRef::new(album.id.as_deref().map(rid), album.name.clone());
    r.artists = artists(&album.artists);
    r.year = year(album.release_date.as_deref());
    Some(r)
}

/// A track, or `None` for a local file or an entry without an id. A
/// relinked track keeps the id that was asked for (`linked_from`): the id
/// the library stores stays stable, and librespot follows the alternative
/// itself when playing. `album` fills in the album of a simplified track
/// (album listings leave it out).
pub fn track(t: &models::Track, album: Option<&models::Album>) -> Option<StreamTrack> {
    if t.is_local || t.kind.as_deref().is_some_and(|k| k != "track") {
        return None;
    }
    let id = t
        .linked_from
        .as_ref()
        .and_then(|l| l.id.as_deref())
        .or(t.id.as_deref())?;
    let album = t.album.as_ref().or(album);
    let mut s = StreamTrack::new(rid(id), t.name.clone());
    s.artists = artists(&t.artists);
    s.album = album.and_then(album_ref);
    s.duration_ms = t.duration_ms.filter(|&d| d > 0);
    s.isrc = isrc(t.external_ids.isrc.as_deref());
    s.disc_no = t.disc_number.filter(|&n| n > 0);
    s.track_no = t.track_number.filter(|&n| n > 0);
    s.cover_url = album.and_then(|a| cover(&a.images));
    s.kind = TrackKind::Song;
    s.playable = t.is_playable.unwrap_or(true) && t.restrictions.is_none();
    Some(s)
}

/// An album; its tracks are whatever page of them the object carries.
pub fn album(a: &models::Album) -> Option<StreamAlbum> {
    let id = a.id.as_deref()?;
    let tracks = a
        .tracks
        .as_ref()
        .map(|p| p.items.iter().filter_map(|t| track(t, Some(a))).collect())
        .unwrap_or_default();
    Some(StreamAlbum {
        id: rid(id),
        title: a.name.clone(),
        artists: artists(&a.artists),
        year: year(a.release_date.as_deref()),
        // Only a full date: Spotify also dates albums by year or month.
        release_date: a.release_date.clone().filter(|d| d.len() == 10),
        release_type: release_type(a.album_type.as_deref(), a.album_group.as_deref()),
        cover_url: cover(&a.images),
        total_tracks: a.total_tracks,
        tracks,
    })
}

pub fn artist(a: &models::Artist) -> Option<StreamArtist> {
    (!a.id.is_empty()).then(|| StreamArtist {
        id: rid(&a.id),
        name: a.name.clone(),
        image_url: cover(&a.images),
        albums: Vec::new(),
    })
}

/// A playlist header (no tracks). Editable when `account` owns it or it is
/// collaborative.
pub fn playlist(p: &models::Playlist, account: Option<&str>) -> Option<StreamPlaylist> {
    if p.id.is_empty() {
        return None;
    }
    let owned = account.is_some_and(|me| p.owner.id.as_deref() == Some(me));
    Some(StreamPlaylist {
        id: rid(&p.id),
        name: p.name.clone(),
        owner: p
            .owner
            .display_name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| p.owner.id.clone()),
        cover_url: cover(&p.images),
        version: p.snapshot_id.clone(),
        editable: owned || p.collaborative,
        total_tracks: p.total(),
        tracks: Vec::new(),
    })
}

/// The tracks of one page of playlist entries, in order, without local
/// files, episodes and removed songs.
pub fn entries(e: &models::PlaylistEntries) -> Vec<StreamTrack> {
    e.items
        .iter()
        .flatten()
        .filter_map(|i| i.track())
        .filter_map(|t| track(t, None))
        .collect()
}

/// `played_at` (RFC 3339, UTC) as unix ms.
pub fn rfc3339_ms(text: &str) -> Option<i64> {
    // 2026-10-17T08:15:30.123Z
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut ms = 0;
    let mut rest = &text[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(|c| c.is_ascii_digit()).collect();
        rest = &frac[digits.len()..];
        let padded = format!("{digits:0<3}");
        ms = padded[..3].parse().ok()?;
    }
    let offset_s = match rest {
        "Z" | "z" | "" => 0,
        o if o.len() == 6 => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            sign * (o[1..3].parse::<i64>().ok()? * 3600 + o[4..6].parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant).
    let (y2, m2) = if mo <= 2 {
        (y - 1, mo + 9)
    } else {
        (y, mo - 3)
    };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 + s) - offset_s) * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::*;

    const TRACK: &str = r#"{
        "id": "3n3Ppam7vgaVa1iaRUc9Lp", "type": "track", "name": "Mr. Brightside",
        "duration_ms": 222075, "disc_number": 1, "track_number": 2, "is_local": false,
        "is_playable": true,
        "external_ids": { "isrc": "usir-20400274" },
        "artists": [{ "id": "0C0XlULifJtAgn6ZNCW2eu", "name": "The Killers" }],
        "album": {
            "id": "4OHNH3sDzIxnmUADXzv2kT", "name": "Hot Fuss", "album_type": "album",
            "release_date": "2004-06-07", "total_tracks": 11,
            "artists": [{ "id": "0C0XlULifJtAgn6ZNCW2eu", "name": "The Killers" }],
            "images": [
                { "url": "https://i.scdn.co/image/big", "width": 1000, "height": 1000 },
                { "url": "https://i.scdn.co/image/640", "width": 640, "height": 640 },
                { "url": "https://i.scdn.co/image/300", "width": 300, "height": 300 },
                { "url": "https://i.scdn.co/image/64", "width": 64, "height": 64 }
            ]
        }
    }"#;

    #[test]
    fn a_full_track_maps_with_isrc_numbers_and_cover() {
        let t: Track = serde_json::from_str(TRACK).unwrap();
        let s = track(&t, None).unwrap();
        assert_eq!(s.id, rid("3n3Ppam7vgaVa1iaRUc9Lp"));
        assert_eq!(s.title, "Mr. Brightside");
        assert_eq!(s.artist_line(), "The Killers");
        assert_eq!(s.isrc.as_deref(), Some("USIR20400274"));
        assert_eq!((s.disc_no, s.track_no), (Some(1), Some(2)));
        assert_eq!(s.duration_ms, Some(222_075));
        assert_eq!(s.cover_url.as_deref(), Some("https://i.scdn.co/image/640"));
        let album = s.album.unwrap();
        assert_eq!(album.title, "Hot Fuss");
        assert_eq!(album.year, Some(2004));
        assert_eq!(album.id, Some(rid("4OHNH3sDzIxnmUADXzv2kT")));
        assert!(s.playable);
    }

    #[test]
    fn relinked_and_restricted_tracks() {
        let t: Track = serde_json::from_str(
            r#"{"id":"market","name":"X","linked_from":{"id":"asked"},
                "restrictions":{"reason":"market"},"is_playable":false}"#,
        )
        .unwrap();
        let s = track(&t, None).unwrap();
        assert_eq!(s.id.id, "asked");
        assert!(!s.playable);
        let local: Track =
            serde_json::from_str(r#"{"id":null,"name":"L","is_local":true}"#).unwrap();
        assert!(track(&local, None).is_none());
        let episode: Track = serde_json::from_str(r#"{"id":"e","type":"episode"}"#).unwrap();
        assert!(track(&episode, None).is_none());
    }

    #[test]
    fn a_full_album_maps_with_its_simplified_tracks() {
        let a: Album = serde_json::from_str(
            r#"{
            "id": "al", "name": "Single Thing", "album_type": "single", "total_tracks": 2,
            "release_date": "2019", "images": [{"url":"https://c/300","width":300}],
            "artists": [{"id":"ar","name":"Artist"}],
            "tracks": { "items": [
                {"id":"t1","name":"One","track_number":1,"disc_number":1,"duration_ms":1000,
                 "artists":[{"id":"ar","name":"Artist"}]},
                null,
                {"id":"t2","name":"Two","track_number":2,"disc_number":1}
            ], "total": 2, "limit": 50, "offset": 0, "next": null }
        }"#,
        )
        .unwrap();
        let s = album(&a).unwrap();
        assert_eq!(s.release_type, ReleaseType::Single);
        assert_eq!(s.year, Some(2019));
        assert_eq!(s.release_date, None, "a year only is no day");
        assert_eq!(s.total_tracks, Some(2));
        assert_eq!(s.tracks.len(), 2);
        let t1 = &s.tracks[0];
        assert_eq!(t1.album.as_ref().unwrap().title, "Single Thing");
        assert_eq!(t1.cover_url.as_deref(), Some("https://c/300"));
        assert_eq!(t1.track_no, Some(1));
    }

    #[test]
    fn release_types_follow_group_then_type() {
        assert_eq!(
            release_type(Some("album"), Some("single")),
            ReleaseType::Single
        );
        assert_eq!(
            release_type(Some("compilation"), Some("appears_on")),
            ReleaseType::Compilation
        );
        assert_eq!(release_type(None, None), ReleaseType::Album);
        assert_eq!(release_type(Some("audiobook"), None), ReleaseType::Other);
    }

    #[test]
    fn artists_and_playlists_map() {
        let a: Artist = serde_json::from_str(
            r#"{"id":"ar","name":"Кино","images":[{"url":"https://a/1","width":160}],
                "followers":{"total":5},"popularity":50}"#,
        )
        .unwrap();
        let s = artist(&a).unwrap();
        assert_eq!(s.name, "Кино");
        assert_eq!(s.image_url.as_deref(), Some("https://a/1"));

        // Listing shape (2026 `items` count), owned by the account.
        let p: Playlist = serde_json::from_str(
            r#"{"id":"pl","name":"Mine","snapshot_id":"snap","collaborative":false,
                "owner":{"id":"me","display_name":"Me"},"images":null,
                "items":{"href":"x","total":1234}}"#,
        )
        .unwrap();
        let s = playlist(&p, Some("me")).unwrap();
        assert!(s.editable);
        assert_eq!(s.version.as_deref(), Some("snap"));
        assert_eq!(s.total_tracks, Some(1234));
        assert_eq!(s.owner.as_deref(), Some("Me"));
        assert!(!playlist(&p, Some("other")).unwrap().editable);

        // Classic shape with a page of entries, collaborative.
        let p: Playlist = serde_json::from_str(
            r#"{"id":"pl2","name":"Ours","collaborative":true,"owner":{"id":"x"},
                "tracks":{"total":3,"items":[
                    {"track":{"id":"a","type":"track","name":"A"}},
                    {"track":null},
                    {"item":{"id":"b","type":"track","name":"B"}},
                    {"item":{"id":"e","type":"episode","name":"E"}}
                ]}}"#,
        )
        .unwrap();
        let s = playlist(&p, Some("me")).unwrap();
        assert!(s.editable);
        assert_eq!(s.owner.as_deref(), Some("x"));
        let tracks = entries(p.entries().unwrap());
        assert_eq!(
            tracks.iter().map(|t| t.id.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn search_results_and_pages() {
        let r: SearchResults = serde_json::from_str(
            r#"{
            "tracks": {"items":[{"id":"t","name":"T"}],"total":100,"limit":10,"offset":0,"next":"n"},
            "albums": {"items":[{"id":"a","name":"A","album_type":"album"}],"next":null},
            "artists": {"items":[{"id":"r","name":"R"}]},
            "playlists": {"items":[null,{"id":"p","name":"P","owner":{"id":"o"}}]}
        }"#,
        )
        .unwrap();
        let tracks = r.tracks.unwrap();
        assert_eq!(tracks.next_offset(), Some(10));
        assert_eq!(tracks.items.len(), 1);
        assert_eq!(r.albums.unwrap().next_offset(), None);
        assert_eq!(r.playlists.unwrap().items.len(), 1, "nulls are skipped");

        let liked: Page<SavedTrack> = serde_json::from_str(
            r#"{"items":[{"added_at":"2026-01-01T00:00:00Z","track":{"id":"x","name":"X"}}],
                "total":2000,"limit":50,"offset":50,"next":"https://api/next"}"#,
        )
        .unwrap();
        assert_eq!(liked.next_offset(), Some(100));
        assert_eq!(track(&liked.items[0].track, None).unwrap().id.id, "x");
    }

    #[test]
    fn recently_played_times() {
        let page: CursorPage<PlayHistory> = serde_json::from_str(
            r#"{"items":[{"track":{"id":"x","name":"X"},"played_at":"2026-10-17T08:15:30.123Z"}],
                "next":null}"#,
        )
        .unwrap();
        let at = rfc3339_ms(page.items[0].played_at.as_deref().unwrap()).unwrap();
        assert_eq!(at, 1_792_224_930_123);
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_ms("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(rfc3339_ms("garbage"), None);
    }

    #[test]
    fn covers_and_isrcs() {
        assert_eq!(cover(&[]), None);
        let only_big = [Image {
            url: "u".into(),
            width: Some(2000),
        }];
        assert_eq!(cover(&only_big).as_deref(), Some("u"));
        assert_eq!(
            isrc(Some(" gb-aye-99-00001 ")).as_deref(),
            Some("GBAYE9900001")
        );
        assert_eq!(isrc(Some("")), None);
    }
}
