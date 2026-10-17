//! The librespot session: one per account, created lazily, reused, and
//! replaced once when Spotify drops it. Plus the catalogue reads it serves,
//! which spend no Web API quota.
//!
//! The reads are ported (subset) from Spotifast `src/session_reads.rs`
//! (https://github.com/crmne/spotifast).
//! Copyright (c) 2026 Carmine Paolino, MIT License.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use librespot_core::authentication::Credentials;
use librespot_core::config::SessionConfig;
use librespot_core::spotify_id::SpotifyId;
use librespot_core::{Error, FileId, Session, SpotifyUri};
use librespot_metadata::Metadata as _;
use librespot_protocol::extended_metadata::{
    BatchedEntityRequest, BatchedExtensionResponse, EntityRequest, ExtensionQuery,
};
use librespot_protocol::extension_kind::ExtensionKind;
use nikgob_core::streaming::{AlbumRef, ArtistRef, StreamPlaylist, StreamTrack};
use protobuf::{EnumOrUnknown, Message as _};

use crate::convert::{isrc, rid};

const IMAGE_HOST: &str = "https://i.scdn.co/image/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Tracks per extended-metadata request.
const METADATA_BATCH: usize = 200;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Sessions {
    current: Mutex<Option<Session>>,
    /// Serializes connecting, so concurrent callers share one session.
    connecting: tokio::sync::Mutex<()>,
    device_id: String,
    tmp_dir: PathBuf,
    /// librespot's file cache for the next session: its folder and size
    /// limit (0 = off). It keeps the files **encrypted**, as Spotify sends
    /// them, like the official client; the key is fetched on every play.
    audio_cache: Mutex<(Option<PathBuf>, u64)>,
}

impl Sessions {
    pub fn new(device_id: String, tmp_dir: PathBuf) -> Self {
        Self {
            current: Mutex::new(None),
            connecting: tokio::sync::Mutex::new(()),
            device_id,
            tmp_dir,
            audio_cache: Mutex::new((None, crate::DEFAULT_CACHE_LIMIT)),
        }
    }

    /// Where the encrypted audio cache lives (used by sessions made later).
    pub fn set_cache_dir(&self, dir: PathBuf) {
        lock(&self.audio_cache).0 = Some(dir);
    }

    /// The cache's size limit for sessions made later; 0 turns it off.
    pub fn set_cache_limit(&self, bytes: u64) {
        lock(&self.audio_cache).1 = bytes;
    }

    pub fn cache_dir(&self) -> Option<PathBuf> {
        lock(&self.audio_cache).0.clone()
    }

    fn cache(&self) -> Option<librespot_core::cache::Cache> {
        let (dir, limit) = lock(&self.audio_cache).clone();
        let dir = dir.filter(|_| limit > 0)?;
        match librespot_core::cache::Cache::new(None, None, Some(&dir), Some(limit)) {
            Ok(c) => Some(c),
            Err(e) => {
                log::info!("spotify: no audio cache in {}: {e}", dir.display());
                None
            }
        }
    }

    /// The session, if one is up.
    pub fn current(&self) -> Option<Session> {
        lock(&self.current).clone().filter(|s| !s.is_invalid())
    }

    /// The session, connecting with `credentials` when none is up.
    pub async fn get(&self, credentials: Credentials) -> Result<Session, Error> {
        if let Some(s) = self.current() {
            return Ok(s);
        }
        let _serial = self.connecting.lock().await;
        if let Some(s) = self.current() {
            return Ok(s);
        }
        self.connect(credentials).await
    }

    /// A new session, replacing any current one.
    pub async fn connect(&self, credentials: Credentials) -> Result<Session, Error> {
        let config = SessionConfig {
            device_id: self.device_id.clone(),
            tmp_dir: self.tmp_dir.clone(),
            ..SessionConfig::default()
        };
        let _ = std::fs::create_dir_all(&self.tmp_dir);
        let session = Session::new(config, self.cache());
        match tokio::time::timeout(CONNECT_TIMEOUT, session.connect(credentials, false)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err(Error::deadline_exceeded("connecting to Spotify")),
        }
        log::info!("spotify: session connected");
        if let Some(old) = lock(&self.current).replace(session.clone()) {
            old.shutdown();
        }
        Ok(session)
    }

    /// Drops the session (logout). No network.
    pub fn close(&self) {
        if let Some(s) = lock(&self.current).take() {
            s.shutdown();
        }
    }
}

/// The account type the session reports, waiting a little for the product
/// attributes Spotify sends right after connecting.
pub async fn premium(session: &Session) -> Option<bool> {
    for _ in 0..50 {
        if let Some(kind) = session.get_user_attribute("type") {
            return Some(kind == "premium");
        }
        if session.is_invalid() {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

fn base62(uri: &SpotifyUri) -> Option<String> {
    uri.to_id().ok()
}

fn track_uri(id: &str) -> Result<SpotifyUri, Error> {
    Ok(SpotifyUri::Track {
        id: SpotifyId::from_base62(id)?,
    })
}

fn image_url(file: &FileId) -> String {
    format!("{IMAGE_HOST}{file}")
}

/// A session track as a core track.
pub fn track(t: &librespot_metadata::Track) -> Option<StreamTrack> {
    let id = base62(&t.id)?;
    let mut s = StreamTrack::new(rid(&id), t.name.clone());
    s.artists = t
        .artists
        .iter()
        .map(|a| ArtistRef::new(base62(&a.id).map(|i| rid(&i)), a.name.clone()))
        .collect();
    let album = &t.album;
    if !album.name.is_empty() {
        let mut r = AlbumRef::new(base62(&album.id).map(|i| rid(&i)), album.name.clone());
        r.artists = album
            .artists
            .iter()
            .map(|a| ArtistRef::new(base62(&a.id).map(|i| rid(&i)), a.name.clone()))
            .collect();
        r.year = Some(album.date.year()).filter(|y| *y > 1);
        s.album = Some(r);
    }
    s.duration_ms = u64::try_from(t.duration).ok().filter(|d| *d > 0);
    s.isrc = isrc(
        t.external_ids
            .iter()
            .find(|e| e.external_type.eq_ignore_ascii_case("isrc"))
            .map(|e| e.id.as_str()),
    );
    s.track_no = u32::try_from(t.number).ok().filter(|n| *n > 0);
    s.disc_no = u32::try_from(t.disc_number).ok().filter(|n| *n > 0);
    // The largest cover no wider than 640 px (Spotify's DEFAULT is 300,
    // LARGE 640).
    s.cover_url = album
        .covers
        .iter()
        .filter(|c| c.width <= crate::convert::COVER_MAX as i32)
        .max_by_key(|c| c.width)
        .or_else(|| album.covers.first())
        .map(|c| image_url(&c.id));
    Some(s)
}

pub async fn track_by_id(session: &Session, id: &str) -> Result<StreamTrack, Error> {
    let t = librespot_metadata::Track::get(session, &track_uri(id)?).await?;
    track(&t).ok_or_else(|| Error::not_found("track"))
}

/// Details of many tracks, by id, in batched requests. Ids Spotify has no
/// track for are absent.
pub async fn tracks(
    session: &Session,
    ids: &[String],
) -> Result<HashMap<String, StreamTrack>, Error> {
    let mut out = HashMap::new();
    let uris: Vec<SpotifyUri> = ids.iter().filter_map(|id| track_uri(id).ok()).collect();
    for chunk in uris.chunks(METADATA_BATCH) {
        let (request, asked) = batch(chunk);
        if asked.is_empty() {
            continue;
        }
        let response = session.spclient().get_extended_metadata(request).await?;
        out.extend(answers(response));
    }
    Ok(out)
}

/// One request for the details of every track among `uris`, asked once each.
fn batch(uris: &[SpotifyUri]) -> (BatchedEntityRequest, BTreeSet<String>) {
    let mut request = BatchedEntityRequest::new();
    let mut asked = BTreeSet::new();
    for uri in uris {
        let Ok(text) = uri.to_uri() else { continue };
        if !matches!(uri, SpotifyUri::Track { .. }) || !asked.insert(text.clone()) {
            continue;
        }
        request.entity_request.push(EntityRequest {
            entity_uri: text,
            query: vec![ExtensionQuery {
                extension_kind: EnumOrUnknown::new(ExtensionKind::TRACK_V4),
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    (request, asked)
}

/// The tracks Spotify answered with, by base-62 id. A refused or unreadable
/// entry is left out (the caller keeps what it had).
fn answers(response: BatchedExtensionResponse) -> HashMap<String, StreamTrack> {
    let mut out = HashMap::new();
    for array in response.extended_metadata {
        if array.extension_kind.enum_value() != Ok(ExtensionKind::TRACK_V4) {
            continue;
        }
        for data in array.extension_data {
            if !matches!(data.header.status_code, 0 | 200) {
                continue;
            }
            let Some(any) = data.extension_data.as_ref() else {
                continue;
            };
            let Ok(message) = librespot_protocol::metadata::Track::parse_from_bytes(&any.value)
            else {
                continue;
            };
            let Ok(t) = librespot_metadata::Track::try_from(&message) else {
                continue;
            };
            if let Some(s) = track(&t) {
                out.insert(s.id.id.clone(), s);
            }
        }
    }
    out
}

/// The artist's most popular tracks in the account's market (or worldwide),
/// in Spotify's order. The Web API removed this endpoint in February 2026.
pub async fn artist_top_tracks(session: &Session, id: &str) -> Result<Vec<StreamTrack>, Error> {
    let uri = SpotifyUri::Artist {
        id: SpotifyId::from_base62(id)?,
    };
    let artist = librespot_metadata::Artist::get(session, &uri).await?;
    let country = session.country();
    let Some(top) = artist
        .top_tracks
        .iter()
        .find(|t| t.country == country)
        .or_else(|| artist.top_tracks.iter().find(|t| t.country == "ZZ"))
    else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> = top.tracks.iter().take(10).filter_map(base62).collect();
    let found = tracks(session, &ids).await?;
    Ok(ids.iter().filter_map(|id| found.get(id).cloned()).collect())
}

/// A whole playlist with its tracks. `me` (the account's user id) decides
/// whether it is editable. A list Spotify sent truncated is an error, so the
/// caller pages it over the Web API instead.
pub async fn playlist(
    session: &Session,
    id: &str,
    me: Option<&str>,
) -> Result<StreamPlaylist, Error> {
    let uri = SpotifyUri::Playlist {
        id: SpotifyId::from_base62(id)?,
        user: None,
    };
    let list = librespot_metadata::Playlist::get(session, &uri).await?;
    let items = &list.contents.items;
    let length = usize::try_from(list.length).unwrap_or(0);
    if list.contents.is_truncated || items.len() < length {
        return Err(Error::unavailable("truncated playlist"));
    }
    let owner = match &list.id {
        SpotifyUri::Playlist { user: Some(u), .. } if !u.is_empty() => Some(u.clone()),
        _ => None,
    };
    let ids: Vec<String> = items
        .iter()
        .filter(|i| matches!(i.id, SpotifyUri::Track { .. }))
        .filter_map(|i| base62(&i.id))
        .collect();
    let found = tracks(session, &ids).await?;
    let tracks = ids.iter().filter_map(|id| found.get(id).cloned()).collect();
    let attributes = &list.attributes;
    let owner_name = match owner.as_deref() {
        Some(user) => user_name(session, user).await.or_else(|| owner.clone()),
        None => None,
    };
    let cover = attributes
        .picture_sizes
        .iter()
        .find(|p| p.target_name == "large")
        .or_else(|| {
            attributes
                .picture_sizes
                .iter()
                .find(|p| p.target_name == "default")
        })
        .and_then(|p| picture_url(&p.url))
        .or_else(|| {
            (!attributes.picture.is_empty())
                .then(|| image_url(&FileId::from_raw(&attributes.picture)))
        });
    Ok(StreamPlaylist {
        id: rid(id),
        name: attributes.name.clone(),
        editable: attributes.is_collaborative || (owner.is_some() && owner.as_deref() == me),
        owner: owner_name,
        cover_url: cover,
        // The Web API's snapshot id is the revision in base64, so both
        // sources give the same version.
        version: Some(base64::engine::general_purpose::STANDARD_NO_PAD.encode(&list.revision)),
        total_tracks: u32::try_from(list.length).ok(),
        tracks,
    })
}

/// A picture reference as a playlist gives it: a web address, or an image
/// URI naming a file on Spotify's image host.
fn picture_url(reference: &str) -> Option<String> {
    if reference.starts_with("https://") || reference.starts_with("http://") {
        return Some(reference.into());
    }
    reference
        .strip_prefix("spotify:image:")
        .map(|hex| format!("{IMAGE_HOST}{hex}"))
}

/// A user's display name from their profile view.
async fn user_name(session: &Session, user: &str) -> Option<String> {
    if user == "spotify" {
        return Some("Spotify".into());
    }
    let bytes = session
        .spclient()
        .get_user_profile(user, Some(0), Some(0))
        .await
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    json["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use librespot_protocol::entity_extension_data::EntityExtensionData;
    use librespot_protocol::extended_metadata::EntityExtensionDataArray;

    fn track_message(gid: u8, isrc: &str) -> librespot_protocol::metadata::Track {
        let mut t = librespot_protocol::metadata::Track::new();
        t.set_gid(vec![gid; 16]);
        t.set_name(format!("Song {gid}"));
        t.set_duration(180_000);
        t.set_number(3);
        t.set_disc_number(1);
        let mut ext = librespot_protocol::metadata::ExternalId::new();
        ext.set_type("isrc".into());
        ext.set_id(isrc.into());
        t.external_id.push(ext);
        let mut artist = librespot_protocol::metadata::Artist::new();
        artist.set_gid(vec![9; 16]);
        artist.set_name("Artist".into());
        t.artist.push(artist);
        let mut album = librespot_protocol::metadata::Album::new();
        album.set_gid(vec![8; 16]);
        album.set_name("Album".into());
        let mut group = librespot_protocol::metadata::ImageGroup::new();
        for (file, width) in [(1u8, 300), (2, 640), (3, 1280)] {
            let mut image = librespot_protocol::metadata::Image::new();
            image.set_file_id(vec![file; 20]);
            image.set_width(width);
            image.set_height(width);
            group.image.push(image);
        }
        album.cover_group = protobuf::MessageField::some(group);
        t.album = protobuf::MessageField::some(album);
        t
    }

    fn id_of(gid: u8) -> String {
        SpotifyId::from_raw(&[gid; 16])
            .unwrap()
            .to_base62()
            .unwrap()
    }

    #[test]
    fn session_tracks_map_with_isrc_and_a_640_cover() {
        let message = track_message(1, "us-abc-12-34567");
        let t = librespot_metadata::Track::try_from(&message).unwrap();
        let s = track(&t).unwrap();
        assert_eq!(s.id.id, id_of(1));
        assert_eq!(s.isrc.as_deref(), Some("USABC1234567"));
        assert_eq!(s.duration_ms, Some(180_000));
        assert_eq!((s.disc_no, s.track_no), (Some(1), Some(3)));
        assert_eq!(s.artist_line(), "Artist");
        assert_eq!(s.album.as_ref().unwrap().title, "Album");
        assert_eq!(
            s.cover_url.as_deref(),
            Some(format!("{IMAGE_HOST}{}", "02".repeat(20)).as_str())
        );
    }

    #[test]
    fn a_batch_is_read_by_kind_and_refusals_are_left_out() {
        let uris: Vec<SpotifyUri> = [1u8, 2, 1]
            .iter()
            .map(|g| track_uri(&id_of(*g)).unwrap())
            .collect();
        let (request, asked) = batch(&uris);
        assert_eq!(asked.len(), 2, "a repeated song is asked once");
        assert_eq!(request.entity_request.len(), 2);

        let mut array = EntityExtensionDataArray::new();
        array.extension_kind = EnumOrUnknown::new(ExtensionKind::TRACK_V4);
        for (gid, status) in [(1u8, 200), (2, 451)] {
            let mut data = EntityExtensionData::new();
            data.entity_uri = format!("spotify:track:{}", id_of(gid));
            data.header.mut_or_insert_default().status_code = status;
            let mut any = protobuf::well_known_types::any::Any::new();
            any.value = track_message(gid, "ZZ0000000001").write_to_bytes().unwrap();
            data.extension_data = protobuf::MessageField::some(any);
            array.extension_data.push(data);
        }
        let mut response = BatchedExtensionResponse::new();
        response.extended_metadata.push(array);
        let found = answers(response);
        assert_eq!(found.len(), 1);
        assert!(found.contains_key(&id_of(1)));
    }

    #[test]
    fn playlist_pictures_resolve_to_the_image_host() {
        assert_eq!(
            picture_url("spotify:image:ab67").as_deref(),
            Some("https://i.scdn.co/image/ab67")
        );
        assert_eq!(
            picture_url("https://mosaic.scdn.co/640/x").as_deref(),
            Some("https://mosaic.scdn.co/640/x")
        );
        assert_eq!(picture_url("nonsense"), None);
    }
}
