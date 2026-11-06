//! The JSON of SoundCloud's api-v2 (only the fields we read) and its
//! conversion to the [`nikgob_core::streaming`] types.
//!
//! Every object is lenient: a missing field is its default. A playlist
//! lists its first few tracks in full and the rest as stubs (`id`,
//! `kind`, `policy` only); [`Track::is_stub`] tells them apart, the
//! service fills them with `/tracks?ids=`.
//!
//! Ids: a track, a user (an artist) and a playlist (an album is a playlist
//! whose `set_type` is album / ep / single / compilation) are numbers; a
//! system playlist (stations, «Weekly Wave», genre charts) is its urn,
//! `soundcloud:system-playlists:…`.

use nikgob_core::model::ReleaseType;
use nikgob_core::streaming::{
    AlbumRef, ArtistRef, RemoteId, Service, StreamAlbum, StreamArtist, StreamPlaylist, StreamTrack, TrackKind,
};
use serde::Deserialize;

/// A page of a listing: `{"collection": [...], "next_href": "…"}`.
#[derive(Debug, Deserialize)]
pub(crate) struct Page<T> {
    #[serde(default = "Vec::new")]
    pub(crate) collection: Vec<T>,
    #[serde(default)]
    pub(crate) next_href: Option<String>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            collection: Vec::new(),
            next_href: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct User {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) avatar_url: Option<String>,
    #[serde(default)]
    pub(crate) permalink_url: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Format {
    #[serde(default)]
    pub(crate) protocol: String,
    #[serde(default)]
    pub(crate) mime_type: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Transcoding {
    #[serde(default)]
    pub(crate) url: String,
    /// `aac_160k`, `aac_96k`, `abr_sq`, `mp3_1_0`, `opus_0_0`.
    #[serde(default)]
    pub(crate) preset: String,
    /// A 30 s preview (a track the account may not play in full).
    #[serde(default)]
    pub(crate) snipped: bool,
    #[serde(default)]
    pub(crate) format: Format,
}

impl Transcoding {
    /// Encrypted HLS (`cbc-encrypted-hls`, `ctr-encrypted-hls`): DRM, never
    /// decrypted here.
    pub(crate) fn is_drm(&self) -> bool {
        self.format.protocol.contains("encrypted")
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Media {
    #[serde(default)]
    pub(crate) transcodings: Vec<Transcoding>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Publisher {
    #[serde(default)]
    pub(crate) artist: Option<String>,
    #[serde(default)]
    pub(crate) album_title: Option<String>,
    #[serde(default)]
    pub(crate) isrc: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Track {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) title: Option<String>,
    /// Milliseconds of what plays (30 000 for a preview).
    #[serde(default)]
    pub(crate) duration: Option<u64>,
    /// Milliseconds of the whole track.
    #[serde(default)]
    pub(crate) full_duration: Option<u64>,
    #[serde(default)]
    pub(crate) artwork_url: Option<String>,
    #[serde(default)]
    pub(crate) user: Option<User>,
    #[serde(default)]
    pub(crate) publisher_metadata: Option<Publisher>,
    #[serde(default)]
    pub(crate) media: Option<Media>,
    #[serde(default)]
    pub(crate) track_authorization: Option<String>,
    /// `ALLOW`, `MONETIZE`, `SNIP` (preview only), `BLOCK`.
    #[serde(default)]
    pub(crate) policy: Option<String>,
    #[serde(default)]
    pub(crate) streamable: Option<bool>,
    #[serde(default)]
    pub(crate) permalink_url: Option<String>,
    #[serde(default)]
    pub(crate) release_date: Option<String>,
}

impl Track {
    /// A playlist's stub: only the id (the service fills it).
    pub(crate) fn is_stub(&self) -> bool {
        self.title.is_none() && self.user.is_none()
    }

    /// The stream to play: AAC 160 first, then the other clear HLS ones;
    /// none for a preview or an encrypted-only track.
    pub(crate) fn playable_transcoding(&self) -> Option<&Transcoding> {
        let list = &self.media.as_ref()?.transcodings;
        let rank = |t: &Transcoding| match t.preset.as_str() {
            "aac_160k" => 0,
            "aac_256k" => 0,
            "aac_96k" => 1,
            _ => 2,
        };
        list.iter()
            .filter(|t| !t.snipped && !t.is_drm() && t.format.protocol == "hls" && t.format.mime_type.contains("mp4"))
            .min_by_key(|t| rank(t))
    }

    /// Only encrypted streams in full (DRM): see [`crate::Availability`].
    pub(crate) fn drm_only(&self) -> bool {
        let Some(m) = &self.media else { return false };
        self.playable_transcoding().is_none() && m.transcodings.iter().any(|t| t.is_drm() && !t.snipped)
    }

    /// Only a preview for this account (or nothing at all).
    pub(crate) fn preview_only(&self) -> bool {
        self.policy.as_deref() == Some("SNIP")
            || self.media.as_ref().is_some_and(|m| !m.transcodings.is_empty() && m.transcodings.iter().all(|t| t.snipped))
    }
}

/// A playlist, an album, or a system playlist (station, mix, chart).
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Playlist {
    #[serde(default)]
    pub(crate) id: Option<serde_json::Value>,
    /// System playlists: `soundcloud:system-playlists:…`.
    #[serde(default)]
    pub(crate) urn: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) user: Option<User>,
    #[serde(default)]
    pub(crate) artwork_url: Option<String>,
    #[serde(default)]
    pub(crate) calculated_artwork_url: Option<String>,
    #[serde(default)]
    pub(crate) track_count: Option<u32>,
    #[serde(default)]
    pub(crate) tracks: Vec<Track>,
    /// `album`, `ep`, `single`, `compilation`, `` (a playlist).
    #[serde(default)]
    pub(crate) set_type: Option<String>,
    #[serde(default)]
    pub(crate) release_date: Option<String>,
    #[serde(default)]
    pub(crate) published_at: Option<String>,
    #[serde(default)]
    pub(crate) display_date: Option<String>,
    #[serde(default)]
    pub(crate) last_modified: Option<String>,
    #[serde(default)]
    pub(crate) last_updated: Option<String>,
    #[serde(default)]
    pub(crate) permalink_url: Option<String>,
    #[serde(default)]
    pub(crate) kind: Option<String>,
}

impl Playlist {
    /// Its id as the service hands it out: the number, or the urn of a
    /// system playlist.
    pub(crate) fn key(&self) -> Option<String> {
        if self.kind.as_deref() == Some("system-playlist") || self.id.as_ref().is_some_and(|v| v.is_string()) {
            return self.urn.clone().or_else(|| self.id.as_ref().and_then(|v| v.as_str().map(str::to_owned)));
        }
        match &self.id {
            Some(serde_json::Value::Number(n)) => n.as_u64().map(|n| n.to_string()),
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => self.urn.clone(),
        }
    }
}

/// `{"track": {...}}` of a like, `{"playlist": {...}}` of a liked
/// playlist, `{"played_at": ms, "track": {...}}` of the play history,
/// `{"type": "track-repost", "track": …}` of a repost.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Wrapped {
    #[serde(default)]
    pub(crate) track: Option<Track>,
    #[serde(default)]
    pub(crate) playlist: Option<Playlist>,
    #[serde(default)]
    pub(crate) system_playlist: Option<Playlist>,
    #[serde(default)]
    pub(crate) played_at: Option<i64>,
}

/// One shelf of `/mixed-selections`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Selection {
    #[serde(default)]
    pub(crate) urn: Option<String>,
    #[serde(default)]
    pub(crate) items: Page<Playlist>,
}

/// `/me`.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Me {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) full_name: Option<String>,
    #[serde(default)]
    pub(crate) likes_count: Option<u64>,
    #[serde(default)]
    pub(crate) public_favorites_count: Option<u64>,
}

impl Me {
    /// How many tracks the account liked, as `/me` says.
    pub(crate) fn likes(&self) -> Option<u64> {
        self.likes_count.or(self.public_favorites_count)
    }
}

/// The resolve of a transcoding: `{"url": "…/playlist.m3u8?…"}`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamUrl {
    #[serde(default)]
    pub(crate) url: String,
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

pub(crate) fn rid(id: impl Into<String>) -> RemoteId {
    RemoteId::new(Service::Soundcloud, id)
}

/// The biggest square SoundCloud makes of an artwork (`-large` is 100 px,
/// `-t500x500` 500 px).
pub(crate) fn big_image(url: Option<&str>) -> Option<String> {
    let url = url.filter(|u| !u.is_empty())?;
    Some(url.replace("-large.", "-t500x500."))
}

fn year_of(date: Option<&str>) -> Option<i32> {
    date?.get(..4)?.parse().ok().filter(|y| (1000..=9999).contains(y))
}

fn day_of(date: Option<&str>) -> Option<String> {
    let d = date?.get(..10)?;
    nikgob_core::streaming::date_ms(d).map(|_| d.to_owned())
}

/// The uploader as an artist.
pub(crate) fn user_ref(u: &User) -> ArtistRef {
    ArtistRef::new((u.id > 0).then(|| rid(u.id.to_string())), u.username.trim())
}

pub(crate) fn artist(u: &User) -> StreamArtist {
    StreamArtist {
        id: rid(u.id.to_string()),
        name: u.username.trim().to_owned(),
        image_url: big_image(u.avatar_url.as_deref()).filter(|a| !a.contains("default_avatar")),
        albums: Vec::new(),
    }
}

pub(crate) fn track(t: &Track) -> StreamTrack {
    let user = t.user.clone().unwrap_or_default();
    let publisher = t.publisher_metadata.clone().unwrap_or_default();
    let published_artist = publisher.artist.as_deref().map(str::trim).filter(|a| !a.is_empty());
    // A label's or a fan's upload credits the record's artist in the
    // publisher metadata; that artist has no SoundCloud id here.
    let artists = match published_artist {
        Some(a) if !a.eq_ignore_ascii_case(user.username.trim()) => {
            a.split(", ").map(str::trim).filter(|n| !n.is_empty()).map(|n| ArtistRef::new(None, n)).collect()
        }
        _ if user.username.trim().is_empty() => Vec::new(),
        _ => vec![user_ref(&user)],
    };
    let isrc = publisher
        .isrc
        .as_deref()
        .map(|i| i.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect::<String>())
        .filter(|i| i.len() == 12);
    let album = publisher.album_title.as_deref().map(str::trim).filter(|a| !a.is_empty()).map(|a| {
        let mut r = AlbumRef::new(None, a);
        r.year = year_of(t.release_date.as_deref());
        r
    });
    let cover = big_image(t.artwork_url.as_deref()).or_else(|| big_image(user.avatar_url.as_deref()));
    let preview = t.preview_only();
    StreamTrack {
        id: rid(t.id.to_string()),
        title: t.title.clone().unwrap_or_default().trim().to_owned(),
        artists,
        album,
        // A preview's `duration` is the 30 s it plays.
        duration_ms: t.full_duration.or(t.duration).filter(|d| *d > 0),
        kind: if isrc.is_some() || published_artist.is_some() { TrackKind::Song } else { TrackKind::Upload },
        isrc,
        disc_no: None,
        track_no: None,
        cover_url: cover,
        // DRM-only tracks play through the official widget, on Windows only.
        playable: !preview
            && t.policy.as_deref() != Some("BLOCK")
            && t.streamable != Some(false)
            && (cfg!(windows) || !t.drm_only()),
    }
}

pub(crate) fn album(p: &Playlist, tracks: &[Track]) -> StreamAlbum {
    let user = p.user.clone().unwrap_or_default();
    let date = p.release_date.as_deref().or(p.published_at.as_deref()).or(p.display_date.as_deref());
    let release_type = match p.set_type.as_deref() {
        Some("single") => ReleaseType::Single,
        Some("ep") => ReleaseType::Ep,
        Some("compilation") => ReleaseType::Compilation,
        Some("album") => ReleaseType::Album,
        _ => ReleaseType::Other,
    };
    let me = user_ref(&user);
    let list: Vec<StreamTrack> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut s = track(t);
            s.track_no = u32::try_from(i + 1).ok();
            if s.album.is_none() {
                let mut a = AlbumRef::new(p.key().map(rid), p.title.clone().unwrap_or_default());
                a.artists = vec![me.clone()];
                a.year = year_of(date);
                s.album = Some(a);
            }
            s
        })
        .collect();
    StreamAlbum {
        id: rid(p.key().unwrap_or_default()),
        title: p.title.clone().unwrap_or_default().trim().to_owned(),
        artists: if user.username.is_empty() { Vec::new() } else { vec![me] },
        year: year_of(date),
        release_date: day_of(date),
        release_type,
        cover_url: big_image(p.artwork_url.as_deref())
            .or_else(|| list.first().and_then(|t| t.cover_url.clone())),
        total_tracks: p.track_count.or_else(|| u32::try_from(tracks.len()).ok().filter(|n| *n > 0)),
        tracks: list,
    }
}

pub(crate) fn playlist(p: &Playlist, tracks: &[Track]) -> StreamPlaylist {
    let list: Vec<StreamTrack> = tracks.iter().filter(|t| !t.is_stub()).map(track).collect();
    StreamPlaylist {
        id: rid(p.key().unwrap_or_default()),
        name: p.title.clone().unwrap_or_default().trim().to_owned(),
        owner: p.user.as_ref().map(|u| u.username.clone()).filter(|u| !u.is_empty()),
        cover_url: big_image(p.artwork_url.as_deref())
            .or_else(|| p.calculated_artwork_url.clone())
            .or_else(|| list.first().and_then(|t| t.cover_url.clone())),
        version: p.last_modified.clone().or_else(|| p.last_updated.clone()),
        // Read only: nothing is ever written to SoundCloud's playlists.
        editable: false,
        total_tracks: p.track_count.or_else(|| u32::try_from(tracks.len()).ok().filter(|n| *n > 0)),
        tracks: list,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACK: &str = r#"{"id":199428706,"title":"One More Time","duration":322606,"full_duration":322616,
        "artwork_url":"https://i1.sndcdn.com/artworks-x-large.jpg","policy":"ALLOW","streamable":true,
        "track_authorization":"auth","user":{"id":203812859,"username":"Daft Punk"},
        "publisher_metadata":{"artist":"Daft Punk","album_title":"Discovery","isrc":"GB-DUW-00-00053"},
        "media":{"transcodings":[
          {"url":"https://api-v2.soundcloud.com/media/x/stream/hls","preset":"aac_160k","snipped":false,"format":{"protocol":"hls","mime_type":"audio/mp4; codecs=\"mp4a.40.2\""}},
          {"url":"https://api-v2.soundcloud.com/media/y/stream/hls","preset":"aac_96k","snipped":false,"format":{"protocol":"hls","mime_type":"audio/mp4; codecs=\"mp4a.40.2\""}},
          {"url":"https://api-v2.soundcloud.com/media/z/stream/hls","preset":"abr_sq","snipped":false,"format":{"protocol":"hls","mime_type":"audio/mpegurl"}}]}}"#;

    #[test]
    fn a_full_track_converts_with_its_best_stream() {
        let t: Track = serde_json::from_str(TRACK).unwrap();
        assert!(!t.is_stub());
        assert_eq!(t.playable_transcoding().unwrap().preset, "aac_160k");
        assert!(!t.drm_only());
        let s = track(&t);
        assert_eq!(s.id, rid("199428706"));
        assert_eq!(s.artists, vec![ArtistRef::new(Some(rid("203812859")), "Daft Punk")]);
        assert_eq!(s.isrc.as_deref(), Some("GBDUW0000053"));
        assert_eq!(s.album.unwrap().title, "Discovery");
        assert_eq!(s.duration_ms, Some(322_616));
        assert_eq!(s.cover_url.as_deref(), Some("https://i1.sndcdn.com/artworks-x-t500x500.jpg"));
        assert_eq!(s.kind, TrackKind::Song);
        assert!(s.playable);
    }

    #[test]
    fn previews_drm_and_stubs() {
        let snip: Track = serde_json::from_str(
            r#"{"id":1,"title":"T","policy":"SNIP","duration":30000,"full_duration":320357,"user":{"id":2,"username":"U"},
            "media":{"transcodings":[{"url":"u","preset":"mp3_1_0","snipped":true,"format":{"protocol":"hls","mime_type":"audio/mpeg"}}]}}"#,
        )
        .unwrap();
        assert!(snip.preview_only());
        assert!(snip.playable_transcoding().is_none());
        assert!(!track(&snip).playable);
        assert_eq!(track(&snip).duration_ms, Some(320_357));
        let drm: Track = serde_json::from_str(
            r#"{"id":3,"title":"T","policy":"ALLOW","user":{"id":2,"username":"U"},
            "media":{"transcodings":[{"url":"u","preset":"aac_160k","snipped":false,"format":{"protocol":"ctr-encrypted-hls","mime_type":"audio/mp4"}},
            {"url":"v","preset":"aac_160k","snipped":false,"format":{"protocol":"cbc-encrypted-hls","mime_type":"audio/mp4"}}]}}"#,
        )
        .unwrap();
        assert!(drm.drm_only());
        // Windows plays them through the official widget; elsewhere nothing does.
        assert_eq!(track(&drm).playable, cfg!(windows));
        let stub: Track = serde_json::from_str(r#"{"id":4,"kind":"track","policy":"ALLOW"}"#).unwrap();
        assert!(stub.is_stub());
    }

    #[test]
    fn a_label_upload_credits_the_record_artist() {
        let t: Track = serde_json::from_str(
            r#"{"id":5,"title":"Snake Eyes","user":{"id":8553751,"username":"Monstercat"},"publisher_metadata":{"artist":"Feint"}}"#,
        )
        .unwrap();
        assert_eq!(track(&t).artists, vec![ArtistRef::new(None, "Feint")]);
        let fan: Track = serde_json::from_str(r#"{"id":6,"title":"mix","user":{"id":9,"username":"dj"}}"#).unwrap();
        assert_eq!(track(&fan).kind, TrackKind::Upload);
    }

    #[test]
    fn albums_and_system_playlists() {
        let p: Playlist = serde_json::from_str(
            r#"{"id":243583729,"title":"Discovery","set_type":"album","is_album":true,"release_date":"2001-03-12T00:00:00Z",
            "track_count":14,"user":{"id":203812859,"username":"Daft Punk"},"tracks":[]}"#,
        )
        .unwrap();
        let a = album(&p, &[serde_json::from_str(TRACK).unwrap()]);
        assert_eq!(a.id, rid("243583729"));
        assert_eq!(a.release_date.as_deref(), Some("2001-03-12"));
        assert_eq!(a.year, Some(2001));
        assert_eq!(a.release_type, ReleaseType::Album);
        assert_eq!(a.tracks[0].track_no, Some(1));
        let sys: Playlist = serde_json::from_str(
            r#"{"id":"soundcloud:system-playlists:trending-by-genre:trap","urn":"soundcloud:system-playlists:trending-by-genre:trap",
            "kind":"system-playlist","title":"Trap","last_updated":"2026-11-06T00:00:00Z","tracks":[]}"#,
        )
        .unwrap();
        assert_eq!(sys.key().as_deref(), Some("soundcloud:system-playlists:trending-by-genre:trap"));
        let pl = playlist(&sys, &[]);
        assert!(!pl.editable);
        assert_eq!(pl.version.as_deref(), Some("2026-11-06T00:00:00Z"));
    }
}
