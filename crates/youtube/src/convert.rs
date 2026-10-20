//! RustyPipe's model types → the core streaming types.

use nikgob_core::model::ReleaseType;
use nikgob_core::streaming::{
    AlbumRef, ArtistRef, RemoteId, Service, StreamAlbum, StreamArtist, StreamPlaylist, StreamTrack,
    TrackKind,
};
use rustypipe::model::{
    AlbumId, AlbumItem, AlbumType, ArtistId, ArtistItem, MusicAlbum, MusicArtist, MusicPlaylist,
    MusicPlaylistItem, Thumbnail, TrackItem, TrackType,
};

/// Edge of the square cover asked from Google's image server (YouTube Music
/// shows covers at 544 px).
const COVER_PX: u32 = 544;

pub(crate) fn rid(id: &str) -> RemoteId {
    RemoteId::new(Service::Youtube, id)
}

/// The largest image; Google-hosted square covers (`…=w60-h60-l90-rj`) are
/// asked at [`COVER_PX`] instead of the small listing size.
pub(crate) fn best_image(thumbs: &[Thumbnail]) -> Option<String> {
    let t = thumbs.iter().max_by_key(|t| t.width * t.height)?;
    Some(upscale(&t.url, t.width, t.height))
}

pub(crate) fn upscale(url: &str, width: u32, height: u32) -> String {
    let googleusercontent = url.contains(".googleusercontent.com/") || url.contains(".ggpht.com/");
    if !googleusercontent || width != height || width >= COVER_PX {
        return url.to_owned();
    }
    let Some((base, params)) = url.rsplit_once('=') else {
        return url.to_owned();
    };
    let rest: Vec<&str> = params
        .split('-')
        .filter(|p| !(p.starts_with('w') || p.starts_with('h') || p.starts_with('s')))
        .collect();
    let mut out = format!("{base}=w{COVER_PX}-h{COVER_PX}");
    for p in rest {
        out.push('-');
        out.push_str(p);
    }
    out
}

pub(crate) fn artist_ref(a: &ArtistId) -> ArtistRef {
    ArtistRef::new(a.id.as_deref().map(rid), a.name.clone())
}

fn artist_refs(artists: &[ArtistId]) -> Vec<ArtistRef> {
    artists.iter().map(artist_ref).collect()
}

/// ATV songs are [`TrackKind::Song`], music videos [`TrackKind::Video`].
/// RustyPipe folds YouTube's user-generated videos (UGC) into the same
/// `Video` type as official ones (OMV), so only podcast episodes come out as
/// [`TrackKind::Upload`].
pub(crate) fn track_kind(t: TrackType) -> TrackKind {
    match t {
        TrackType::Track => TrackKind::Song,
        TrackType::Video => TrackKind::Video,
        TrackType::Episode => TrackKind::Upload,
    }
}

pub(crate) fn release_type(t: AlbumType, by_va: bool) -> ReleaseType {
    match t {
        _ if by_va => ReleaseType::Compilation,
        AlbumType::Album => ReleaseType::Album,
        AlbumType::Ep => ReleaseType::Ep,
        AlbumType::Single => ReleaseType::Single,
        _ => ReleaseType::Other,
    }
}

fn album_ref(a: &AlbumId) -> AlbumRef {
    AlbumRef::new(Some(rid(&a.id)), a.name.clone())
}

pub(crate) fn track(t: &TrackItem) -> StreamTrack {
    let mut out = StreamTrack::new(rid(&t.id), t.name.clone());
    out.artists = artist_refs(&t.artists);
    out.album = t.album.as_ref().map(album_ref);
    out.duration_ms = t.duration.map(|s| u64::from(s) * 1000);
    out.track_no = t.track_nr.filter(|&n| n > 0).map(u32::from);
    out.cover_url = best_image(&t.cover);
    out.kind = track_kind(t.track_type);
    out
}

pub(crate) fn album_item(a: &AlbumItem) -> StreamAlbum {
    StreamAlbum {
        id: rid(&a.id),
        title: a.name.clone(),
        artists: artist_refs(&a.artists),
        year: a.year.map(i32::from),
        release_date: None,
        release_type: release_type(a.album_type, a.by_va),
        cover_url: best_image(&a.cover),
        total_tracks: None,
        tracks: Vec::new(),
    }
}

/// An album with its tracks; each track carries the album (with artists and
/// year), its number and the album cover.
pub(crate) fn album(a: &MusicAlbum) -> StreamAlbum {
    let artists = artist_refs(&a.artists);
    let year = a.year.map(i32::from);
    let cover_url = best_image(&a.cover);
    let mut aref = AlbumRef::new(Some(rid(&a.id)), a.name.clone());
    aref.artists = artists.clone();
    aref.year = year;
    let tracks = a
        .tracks
        .iter()
        .map(|t| {
            let mut out = track(t);
            if out.artists.is_empty() {
                out.artists = artists.clone();
            }
            out.album = Some(aref.clone());
            out.disc_no = Some(1);
            if out.cover_url.is_none() {
                out.cover_url = cover_url.clone();
            }
            out
        })
        .collect();
    StreamAlbum {
        id: rid(&a.id),
        title: a.name.clone(),
        artists,
        year,
        release_date: None,
        release_type: release_type(a.album_type, a.by_va),
        cover_url,
        total_tracks: Some(u32::from(a.track_count)),
        tracks,
    }
}

pub(crate) fn artist_item(a: &ArtistItem) -> StreamArtist {
    StreamArtist {
        id: rid(&a.id),
        name: a.name.clone(),
        image_url: best_image(&a.avatar),
        albums: Vec::new(),
    }
}

/// The artist with albums, singles and EPs, newest first.
pub(crate) fn artist(a: &MusicArtist) -> StreamArtist {
    let mut albums: Vec<StreamAlbum> = a.albums.iter().map(album_item).collect();
    albums.sort_by_key(|al| std::cmp::Reverse(al.year));
    StreamArtist {
        id: rid(&a.id),
        name: a.name.clone(),
        image_url: best_image(&a.header_image),
        albums,
    }
}

/// What discovery reads off an artist page (one request): the top songs it
/// shows, the similar artists and the id of the artist's radio.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtistPage {
    pub(crate) top: Vec<StreamTrack>,
    pub(crate) similar: Vec<StreamArtist>,
    pub(crate) radio_id: Option<String>,
}

/// The discovery part of an artist page. The page lists its top songs and
/// then its videos as one list: the top is the songs (the videos only for
/// an artist without songs). A top song without artists is credited to
/// the artist.
pub(crate) fn artist_page(a: &MusicArtist) -> ArtistPage {
    let me = ArtistRef::new(Some(rid(&a.id)), a.name.clone());
    let any_song = a.tracks.iter().any(|t| t.track_type == TrackType::Track);
    ArtistPage {
        top: a
            .tracks
            .iter()
            .filter(|t| !any_song || t.track_type == TrackType::Track)
            .map(|t| {
                let mut t = track(t);
                if t.artists.is_empty() {
                    t.artists = vec![me.clone()];
                }
                t
            })
            .collect(),
        similar: a.similar_artists.iter().filter(|s| s.id != a.id).map(artist_item).collect(),
        radio_id: a.radio_id.clone().filter(|id| !id.is_empty()),
    }
}

/// Tracks of a radio or a related list without `seed` (a radio starts with
/// its own track) and without repeats, in their order.
pub(crate) fn tracks_without(items: &[TrackItem], seed: Option<&str>) -> Vec<StreamTrack> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .filter(|t| Some(t.id.as_str()) != seed && seen.insert(t.id.as_str()))
        .map(track)
        .collect()
}

/// `me`: the logged-in account's name; a playlist whose channel has that
/// name (or that has no channel, as the user's own often do) is editable.
fn editable(channel: Option<&str>, me: Option<&str>) -> bool {
    me.is_some() && (channel.is_none() || channel == me)
}

pub(crate) fn playlist_item(p: &MusicPlaylistItem, me: Option<&str>) -> StreamPlaylist {
    let owner = p.channel.as_ref().map(|c| c.name.clone());
    StreamPlaylist {
        id: rid(&p.id),
        name: p.name.clone(),
        editable: editable(owner.as_deref(), me),
        owner,
        cover_url: best_image(&p.thumbnail),
        version: None,
        total_tracks: p.track_count.map(|n| n as u32),
        tracks: Vec::new(),
    }
}

/// A playlist with the tracks fetched so far.
pub(crate) fn playlist(p: &MusicPlaylist, me: Option<&str>) -> StreamPlaylist {
    let owner = p.channel.as_ref().map(|c| c.name.clone());
    let tracks: Vec<StreamTrack> = p.tracks.items.iter().map(track).collect();
    StreamPlaylist {
        id: rid(&p.id),
        name: p.name.clone(),
        editable: editable(owner.as_deref(), me),
        owner,
        cover_url: best_image(&p.thumbnail),
        version: None,
        total_tracks: Some(p.track_count.map_or(tracks.len() as u32, |n| n as u32)),
        tracks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thumbs(px: u32) -> serde_json::Value {
        json!([
            {"url": "https://lh3.googleusercontent.com/abc=w60-h60-l90-rj", "width": 60, "height": 60},
            {"url": format!("https://lh3.googleusercontent.com/abc=w{px}-h{px}-l90-rj"), "width": px, "height": px},
        ])
    }

    fn track_json(id: &str, ty: &str, nr: Option<u16>) -> serde_json::Value {
        json!({
            "id": id, "name": "Группа крови", "duration": 285, "cover": thumbs(120),
            "artists": [{"id": "UCL9NQ06h7I0CRUcGxPWMtkQ", "name": "Кино"}],
            "artist_id": "UCL9NQ06h7I0CRUcGxPWMtkQ",
            "album": {"id": "MPREb_OLmD8O5IYNS", "name": "Группа крови"},
            "view_count": null, "track_type": ty, "track_nr": nr, "by_va": false
        })
    }

    #[test]
    fn track_maps_ids_artists_album_and_kind() {
        let t: TrackItem =
            serde_json::from_value(track_json("xtxjm7ciwmc", "track", None)).unwrap();
        let s = track(&t);
        assert_eq!(s.id, RemoteId::new(Service::Youtube, "xtxjm7ciwmc"));
        assert_eq!(s.title, "Группа крови");
        assert_eq!(s.artist_line(), "Кино");
        assert_eq!(
            s.artists[0].id.as_ref().unwrap().id,
            "UCL9NQ06h7I0CRUcGxPWMtkQ"
        );
        assert_eq!(
            s.album.as_ref().unwrap().id.as_ref().unwrap().id,
            "MPREb_OLmD8O5IYNS"
        );
        assert_eq!(s.duration_ms, Some(285_000));
        assert_eq!(s.kind, TrackKind::Song);
        assert_eq!(s.track_no, None);
        assert!(s.playable);
        assert_eq!(
            s.cover_url.as_deref(),
            Some("https://lh3.googleusercontent.com/abc=w544-h544-l90-rj")
        );
        let v: TrackItem = serde_json::from_value(track_json("v", "video", None)).unwrap();
        assert_eq!(track(&v).kind, TrackKind::Video);
        let e: TrackItem = serde_json::from_value(track_json("e", "episode", None)).unwrap();
        assert_eq!(track(&e).kind, TrackKind::Upload);
    }

    #[test]
    fn video_thumbnails_are_kept_as_they_are() {
        let t: Vec<Thumbnail> = serde_json::from_value(json!([
            {"url": "https://i.ytimg.com/vi/x/hqdefault.jpg", "width": 480, "height": 360},
            {"url": "https://i.ytimg.com/vi/x/sddefault.jpg", "width": 640, "height": 480},
        ]))
        .unwrap();
        assert_eq!(
            best_image(&t).as_deref(),
            Some("https://i.ytimg.com/vi/x/sddefault.jpg")
        );
        assert_eq!(best_image(&[]), None);
    }

    #[test]
    fn album_gives_tracks_numbers_year_and_release_type() {
        let a: MusicAlbum = serde_json::from_value(json!({
            "id": "MPREb_OLmD8O5IYNS", "playlist_id": "OLAK5uy_x", "name": "Группа крови",
            "cover": thumbs(544),
            "artists": [{"id": "UCL9NQ06h7I0CRUcGxPWMtkQ", "name": "Кино"}],
            "artist_id": "UCL9NQ06h7I0CRUcGxPWMtkQ", "description": null,
            "album_type": "album", "year": 1988, "by_va": false, "track_count": 2,
            "tracks": [track_json("a1", "track", Some(1)), track_json("a2", "track", Some(2))],
            "variants": []
        }))
        .unwrap();
        let s = album(&a);
        assert_eq!(s.title, "Группа крови");
        assert_eq!(s.year, Some(1988));
        assert_eq!(s.release_type, ReleaseType::Album);
        assert_eq!(s.total_tracks, Some(2));
        assert_eq!(s.tracks.len(), 2);
        let t2 = &s.tracks[1];
        assert_eq!(t2.track_no, Some(2));
        assert_eq!(t2.disc_no, Some(1));
        let aref = t2.album.as_ref().unwrap();
        assert_eq!(aref.year, Some(1988));
        assert_eq!(aref.artists[0].name, "Кино");
        assert_eq!(
            s.cover_url.as_deref(),
            Some("https://lh3.googleusercontent.com/abc=w544-h544-l90-rj")
        );
        assert_eq!(release_type(AlbumType::Single, false), ReleaseType::Single);
        assert_eq!(release_type(AlbumType::Ep, false), ReleaseType::Ep);
        assert_eq!(
            release_type(AlbumType::Album, true),
            ReleaseType::Compilation
        );
        assert_eq!(
            release_type(AlbumType::Audiobook, false),
            ReleaseType::Other
        );
    }

    #[test]
    fn artist_lists_albums_newest_first_without_tracks() {
        let album = |id: &str, year: u16, ty: &str| {
            json!({"id": id, "name": id, "cover": [], "artists": [], "artist_id": null,
                   "album_type": ty, "year": year, "by_va": false})
        };
        let a: MusicArtist = serde_json::from_value(json!({
            "id": "UCL9NQ06h7I0CRUcGxPWMtkQ", "name": "Кино", "header_image": thumbs(1200),
            "description": null, "wikipedia_url": null, "subscriber_count": null,
            "tracks": [], "playlists": [], "similar_artists": [],
            "albums": [album("old", 1985, "album"), album("single", 2020, "single"), album("mid", 1988, "ep")],
            "tracks_playlist_id": null, "videos_playlist_id": null, "radio_id": null
        }))
        .unwrap();
        let s = artist(&a);
        assert_eq!(s.name, "Кино");
        let ids: Vec<&str> = s.albums.iter().map(|a| a.id.id.as_str()).collect();
        assert_eq!(ids, ["single", "mid", "old"]);
        assert_eq!(s.albums[0].release_type, ReleaseType::Single);
        assert!(s.albums.iter().all(|a| a.tracks.is_empty()));
    }

    #[test]
    fn an_artist_page_gives_top_songs_similar_artists_and_the_radio() {
        let mut no_artist = track_json("a2", "track", None);
        no_artist["artists"] = json!([]);
        let a: MusicArtist = serde_json::from_value(json!({
            "id": "UCL9NQ06h7I0CRUcGxPWMtkQ", "name": "Кино", "header_image": [],
            "description": null, "wikipedia_url": null, "subscriber_count": null,
            "tracks": [track_json("a1", "track", None), no_artist, track_json("v1", "video", None)],
            "playlists": [],
            "similar_artists": [
                {"id": "UCaquarium", "name": "Аквариум", "avatar": thumbs(120), "subscriber_count": 100},
                {"id": "UCL9NQ06h7I0CRUcGxPWMtkQ", "name": "Кино", "avatar": [], "subscriber_count": null}
            ],
            "albums": [], "tracks_playlist_id": null, "videos_playlist_id": null,
            "radio_id": "RDEMkino"
        }))
        .unwrap();
        let p = artist_page(&a);
        assert_eq!(p.top.len(), 2, "songs, not the videos after them");
        assert_eq!(p.top[0].id.id, "a1");
        assert_eq!(p.top[1].artist_line(), "Кино", "credited to the artist");
        assert_eq!(p.top[1].artists[0].id.as_ref().unwrap().id, "UCL9NQ06h7I0CRUcGxPWMtkQ");
        assert_eq!(p.similar.len(), 1, "not itself");
        assert_eq!(p.similar[0].name, "Аквариум");
        assert!(p.similar[0].albums.is_empty());
        assert_eq!(
            p.similar[0].image_url.as_deref(),
            Some("https://lh3.googleusercontent.com/abc=w544-h544-l90-rj")
        );
        assert_eq!(p.radio_id.as_deref(), Some("RDEMkino"));
    }

    #[test]
    fn a_radio_drops_its_own_track_and_repeats() {
        let items: Vec<TrackItem> = serde_json::from_value(json!([
            track_json("seed", "track", None),
            track_json("b", "track", None),
            track_json("c", "video", None),
            track_json("b", "track", None)
        ]))
        .unwrap();
        let out = tracks_without(&items, Some("seed"));
        let ids: Vec<&str> = out.iter().map(|t| t.id.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
        assert_eq!(out[1].kind, TrackKind::Video, "clips stay marked as clips");
        assert_eq!(tracks_without(&items, None).len(), 3);
    }

    #[test]
    fn playlists_are_editable_only_for_their_owner() {
        let p: MusicPlaylistItem = serde_json::from_value(json!({
            "id": "PLx", "name": "Вечер", "thumbnail": [],
            "channel": {"id": "UCme", "name": "Nik"}, "track_count": 4,
            "from_ytm": false, "is_podcast": false
        }))
        .unwrap();
        let s = playlist_item(&p, Some("Nik"));
        assert!(s.editable);
        assert_eq!(s.owner.as_deref(), Some("Nik"));
        assert_eq!(s.total_tracks, Some(4));
        assert!(!playlist_item(&p, Some("Someone")).editable);
        assert!(!playlist_item(&p, None).editable);

        let full: MusicPlaylist = serde_json::from_value(json!({
            "id": "PLx", "name": "Вечер", "thumbnail": [], "channel": null,
            "description": null, "track_count": null, "from_ytm": false,
            "tracks": {"count": null, "items": [track_json("a1", "track", None)],
                       "ctoken": null, "endpoint": "music_browse"},
            "related_playlists": {"count": null, "items": [], "ctoken": null, "endpoint": "music_browse"}
        }))
        .unwrap();
        let s = playlist(&full, Some("Nik"));
        assert!(s.editable);
        assert_eq!(s.tracks.len(), 1);
        assert_eq!(s.total_tracks, Some(1));
    }
}
