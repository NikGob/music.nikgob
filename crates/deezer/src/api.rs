//! The JSON of Deezer's public API (only the fields we read) and its
//! conversion to the [`nikgob_core::streaming`] types.
//!
//! Every object is lenient: a missing field is its default, so a listing
//! (search hits, album tracks, artist albums) and a full object (`/track/{id}`)
//! read with the same types. Deezer answers errors with HTTP 200 and an
//! `{"error": {...}}` body ([`ApiError`]).

use nikgob_core::model::ReleaseType;
use nikgob_core::streaming::{
    AlbumRef, ArtistRef, RemoteId, Service, StreamAlbum, StreamArtist, StreamPlaylist,
    StreamTrack, TrackKind,
};
use serde::Deserialize;

/// `{"error": {"type": "Exception", "message": "Quota limit exceeded", "code": 4}}`
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ApiError {
    #[serde(default, rename = "type")]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) code: i64,
}

/// Deezer's error codes we tell apart.
pub(crate) mod code {
    /// Quota limit exceeded (50 requests per 5 s).
    pub(crate) const QUOTA: i64 = 4;
    /// The service is busy.
    pub(crate) const BUSY: i64 = 700;
    /// No data (no such id, no track with this ISRC).
    pub(crate) const NO_DATA: i64 = 800;
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Envelope {
    #[serde(default)]
    pub(crate) error: Option<ApiError>,
}

/// A paged listing: `{"data": [...], "total": 120, "next": "…"}` (only
/// the data is read: the next page is asked by index).
#[derive(Debug, Deserialize)]
pub(crate) struct List<T> {
    #[serde(default = "Vec::new")]
    pub(crate) data: Vec<T>,
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self { data: Vec::new() }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Artist {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) picture_xl: Option<String>,
    #[serde(default)]
    pub(crate) picture_big: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Album {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) cover_xl: Option<String>,
    #[serde(default)]
    pub(crate) cover_big: Option<String>,
    #[serde(default)]
    pub(crate) md5_image: Option<String>,
    #[serde(default)]
    pub(crate) release_date: Option<String>,
    #[serde(default)]
    pub(crate) record_type: Option<String>,
    #[serde(default)]
    pub(crate) nb_tracks: Option<u32>,
    #[serde(default)]
    pub(crate) artist: Option<Artist>,
    #[serde(default)]
    pub(crate) contributors: Vec<Artist>,
    /// Only in `/album/{id}`.
    #[serde(default)]
    pub(crate) tracks: Option<List<Track>>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Track {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) title: String,
    /// Can be played in Deezer where the request came from; false for
    /// withdrawn tracks. Ours play from YouTube anyway, so this only marks
    /// a track that no longer exists.
    #[serde(default = "yes")]
    pub(crate) readable: bool,
    /// Seconds.
    #[serde(default)]
    pub(crate) duration: Option<u64>,
    #[serde(default)]
    pub(crate) isrc: Option<String>,
    #[serde(default)]
    pub(crate) track_position: Option<u32>,
    #[serde(default)]
    pub(crate) disk_number: Option<u32>,
    #[serde(default)]
    pub(crate) release_date: Option<String>,
    #[serde(default)]
    pub(crate) md5_image: Option<String>,
    #[serde(default)]
    pub(crate) artist: Option<Artist>,
    /// Only in `/track/{id}`: every credited artist, main first.
    #[serde(default)]
    pub(crate) contributors: Vec<Artist>,
    #[serde(default)]
    pub(crate) album: Option<Album>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct User {
    #[serde(default)]
    pub(crate) name: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Playlist {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) picture_xl: Option<String>,
    #[serde(default)]
    pub(crate) picture_big: Option<String>,
    #[serde(default)]
    pub(crate) nb_tracks: Option<u32>,
    #[serde(default)]
    pub(crate) checksum: Option<String>,
    /// In search hits.
    #[serde(default)]
    pub(crate) user: Option<User>,
    /// In `/playlist/{id}`.
    #[serde(default)]
    pub(crate) creator: Option<User>,
    #[serde(default)]
    pub(crate) tracks: Option<List<Track>>,
}

fn rid(id: u64) -> RemoteId {
    RemoteId::new(Service::Deezer, id.to_string())
}

/// A real image url: Deezer gives a placeholder url with an empty md5
/// (`…/images/artist//1000x1000-…`) for an artist without a picture.
fn image(url: Option<&String>) -> Option<String> {
    let u = url?.trim();
    (!u.is_empty() && !u.contains("//1000x1000") && !u.contains("//500x500") && u.starts_with("https://"))
        .then(|| u.to_owned())
}

/// The cover from its md5 when the object has no url (album tracks).
fn cover_of_md5(md5: Option<&String>) -> Option<String> {
    let m = md5?.trim();
    (!m.is_empty() && m.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("https://cdn-images.dzcdn.net/images/cover/{m}/1000x1000-000000-80-0-0.jpg"))
}

/// A real `YYYY-MM-DD` day (Deezer writes `0000-00-00` for none).
pub(crate) fn day(date: Option<&String>) -> Option<String> {
    let d = date?.trim();
    let ok = d.len() == 10
        && d.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() })
        && year(Some(&d.to_owned())).is_some()
        && &d[5..7] != "00"
        && &d[8..] != "00";
    ok.then(|| d.to_owned())
}

pub(crate) fn year(date: Option<&String>) -> Option<i32> {
    let y: i32 = date?.get(..4)?.parse().ok()?;
    (y > 1000).then_some(y)
}

fn artist_ref(a: &Artist) -> ArtistRef {
    ArtistRef::new((a.id > 0).then(|| rid(a.id)), a.name.clone())
}

fn release_type(t: Option<&str>) -> ReleaseType {
    match t.unwrap_or("album") {
        "single" => ReleaseType::Single,
        "ep" => ReleaseType::Ep,
        "compile" | "compilation" => ReleaseType::Compilation,
        "album" => ReleaseType::Album,
        _ => ReleaseType::Other,
    }
}

impl Album {
    fn cover(&self) -> Option<String> {
        image(self.cover_xl.as_ref())
            .or_else(|| image(self.cover_big.as_ref()))
            .or_else(|| cover_of_md5(self.md5_image.as_ref()))
    }

    fn artists(&self) -> Vec<ArtistRef> {
        match &self.artist {
            Some(a) if !a.name.is_empty() => vec![artist_ref(a)],
            _ => self.contributors.iter().take(1).map(artist_ref).collect(),
        }
    }
}

/// A track. `album` stands in for the track's own album when the track is
/// listed inside it (album tracks carry none).
pub(crate) fn track(t: &Track, album: Option<&Album>) -> StreamTrack {
    let mut out = StreamTrack::new(rid(t.id), t.title.trim());
    out.artists = if t.contributors.is_empty() {
        t.artist.iter().map(artist_ref).collect()
    } else {
        // Main artist first, as `artist` says.
        let mut v: Vec<ArtistRef> = t.contributors.iter().map(artist_ref).collect();
        if let Some(main) = &t.artist
            && let Some(i) = v.iter().position(|a| a.name == main.name)
        {
            let m = v.remove(i);
            v.insert(0, m);
        }
        v
    };
    let al = t.album.as_ref().or(album);
    if let Some(a) = al {
        let mut r = AlbumRef::new((a.id > 0).then(|| rid(a.id)), a.title.clone());
        r.artists = a.artists();
        r.year = year(a.release_date.as_ref()).or_else(|| year(t.release_date.as_ref()));
        r.release_type = a.record_type.as_deref().map(|k| release_type(Some(k)));
        out.album = Some(r);
        out.cover_url = a.cover();
    }
    if out.cover_url.is_none() {
        out.cover_url = cover_of_md5(t.md5_image.as_ref());
    }
    out.duration_ms = t.duration.filter(|d| *d > 0).map(|d| d * 1000);
    out.isrc = t
        .isrc
        .as_deref()
        .map(|s| s.trim().to_ascii_uppercase().replace('-', ""))
        .filter(|s| s.len() == 12 && s.bytes().all(|b| b.is_ascii_alphanumeric()));
    out.track_no = t.track_position.filter(|n| *n > 0);
    out.disc_no = t.disk_number.filter(|n| *n > 0);
    out.kind = TrackKind::Song;
    out.playable = t.readable;
    out
}

/// An album; its tracks when the object has them (`/album/{id}`), numbered
/// in order when Deezer leaves the positions out.
pub(crate) fn album(a: &Album) -> StreamAlbum {
    let mut tracks: Vec<StreamTrack> = a
        .tracks
        .as_ref()
        .map(|l| l.data.iter().map(|t| track(t, Some(a))).collect())
        .unwrap_or_default();
    for (i, t) in tracks.iter_mut().enumerate() {
        if t.track_no.is_none() {
            t.track_no = u32::try_from(i + 1).ok();
        }
        if t.disc_no.is_none() {
            t.disc_no = Some(1);
        }
        if let Some(r) = &mut t.album {
            r.id = Some(rid(a.id));
        }
    }
    StreamAlbum {
        id: rid(a.id),
        title: a.title.trim().to_owned(),
        artists: a.artists(),
        year: year(a.release_date.as_ref()),
        release_date: day(a.release_date.as_ref()),
        release_type: release_type(a.record_type.as_deref()),
        cover_url: a.cover(),
        total_tracks: a.nb_tracks.or_else(|| u32::try_from(tracks.len()).ok().filter(|n| *n > 0)),
        tracks,
    }
}

pub(crate) fn artist(a: &Artist) -> StreamArtist {
    StreamArtist {
        id: rid(a.id),
        name: a.name.trim().to_owned(),
        image_url: image(a.picture_xl.as_ref()).or_else(|| image(a.picture_big.as_ref())),
        albums: Vec::new(),
    }
}

pub(crate) fn playlist(p: &Playlist) -> StreamPlaylist {
    let tracks: Vec<StreamTrack> = p
        .tracks
        .as_ref()
        .map(|l| l.data.iter().map(|t| track(t, None)).collect())
        .unwrap_or_default();
    StreamPlaylist {
        id: rid(p.id),
        name: p.title.trim().to_owned(),
        owner: p
            .creator
            .as_ref()
            .or(p.user.as_ref())
            .map(|u| u.name.clone())
            .filter(|n| !n.is_empty()),
        cover_url: image(p.picture_xl.as_ref()).or_else(|| image(p.picture_big.as_ref())),
        version: p.checksum.clone().filter(|c| !c.is_empty()),
        editable: false,
        total_tracks: p.nb_tracks,
        tracks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_pictures_are_no_picture() {
        let empty = "https://cdn-images.dzcdn.net/images/artist//1000x1000-000000-80-0-0.jpg".to_owned();
        assert_eq!(image(Some(&empty)), None);
        let real = "https://cdn-images.dzcdn.net/images/artist/abc123/1000x1000-000000-80-0-0.jpg".to_owned();
        assert_eq!(image(Some(&real)).as_deref(), Some(real.as_str()));
        assert_eq!(
            cover_of_md5(Some(&"0a1b2c".to_owned())).as_deref(),
            Some("https://cdn-images.dzcdn.net/images/cover/0a1b2c/1000x1000-000000-80-0-0.jpg")
        );
        assert_eq!(year(Some(&"2001-03-07".to_owned())), Some(2001));
        assert_eq!(year(Some(&"0000-00-00".to_owned())), None);
        assert_eq!(day(Some(&"2026-09-25".to_owned())).as_deref(), Some("2026-09-25"));
        assert_eq!(day(Some(&"0000-00-00".to_owned())), None);
        assert_eq!(day(Some(&"2026".to_owned())), None);
        assert_eq!(release_type(Some("compile")), ReleaseType::Compilation);
        assert_eq!(release_type(Some("ep")), ReleaseType::Ep);
    }
}
