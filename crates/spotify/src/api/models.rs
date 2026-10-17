//! Spotify Web API response shapes.
//!
//! Ported (subset) from Spotifast `src/api/models.rs`
//! (https://github.com/crmne/spotifast).
//! Copyright (c) 2026 Carmine Paolino, MIT License.
//!
//! Every field Spotify may omit, null or rename is optional or defaulted, so
//! a response that changed shape degrades to a blank field instead of a
//! failed page. The February 2026 shapes (`items` / `item` in playlists) are
//! accepted next to the classic ones (`tracks` / `track`).

use serde::{Deserialize, Deserializer};

fn skip_nulls<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let items: Option<Vec<Option<T>>> = Option::deserialize(d)?;
    Ok(items.unwrap_or_default().into_iter().flatten().collect())
}

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One page of an offset-paged collection.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Page<T> {
    #[serde(default, deserialize_with = "skip_nulls")]
    pub items: Vec<T>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub next: Option<String>,
}

impl<T> Page<T> {
    /// The offset of the next page, `None` on the last one.
    pub fn next_offset(&self) -> Option<u32> {
        let consumed = self.limit.unwrap_or(0).max(self.items.len() as u32);
        (self.next.is_some() && consumed > 0).then(|| self.offset.unwrap_or(0) + consumed)
    }
}

/// A cursor-paged collection (recently played).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct CursorPage<T> {
    #[serde(default, deserialize_with = "skip_nulls")]
    pub items: Vec<T>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Image {
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ArtistRef {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Artist {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub images: Vec<Image>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Album {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default)]
    pub album_type: Option<String>,
    /// Only in an artist's album listing (removed for Development Mode apps).
    #[serde(default)]
    pub album_group: Option<String>,
    #[serde(default)]
    pub total_tracks: Option<u32>,
    #[serde(default, deserialize_with = "null_default")]
    pub images: Vec<Image>,
    #[serde(default, deserialize_with = "null_default")]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub release_date: Option<String>,
    /// First page of the tracks (simplified, without album) in a full album.
    #[serde(default)]
    pub tracks: Option<Page<Track>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ExternalIds {
    #[serde(default)]
    pub isrc: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct LinkedTrack {
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub id: Option<String>,
    /// `"track"` or `"episode"` where Spotify mixes both.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default, deserialize_with = "null_default")]
    pub artists: Vec<ArtistRef>,
    #[serde(default)]
    pub album: Option<Album>,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub disc_number: Option<u32>,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub is_playable: Option<bool>,
    #[serde(default)]
    /// Present (`{"reason": "market"}`) when the track cannot be played.
    pub restrictions: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "null_default")]
    pub external_ids: ExternalIds,
    /// The originally requested track when Spotify relinked to a release
    /// playable in the account's market.
    #[serde(default)]
    pub linked_from: Option<LinkedTrack>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Owner {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A playlist's entries: `{ "total": N }` in listings, a page of entries in
/// a full playlist. Spotify called it `tracks` before February 2026 and
/// `items` since.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlaylistEntries {
    #[serde(default)]
    pub total: Option<u32>,
    /// Kept positional: a null entry is an unplayable row.
    #[serde(default, deserialize_with = "null_default")]
    pub items: Vec<Option<PlaylistItem>>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Playlist {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub images: Vec<Image>,
    #[serde(default, deserialize_with = "null_default")]
    pub owner: Owner,
    #[serde(default, deserialize_with = "null_default")]
    pub collaborative: bool,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub tracks: Option<PlaylistEntries>,
    #[serde(default)]
    pub items: Option<PlaylistEntries>,
}

impl Playlist {
    pub fn entries(&self) -> Option<&PlaylistEntries> {
        self.items.as_ref().or(self.tracks.as_ref())
    }

    pub fn total(&self) -> Option<u32> {
        self.entries().and_then(|e| e.total)
    }
}

/// An entry of a playlist. `item` is the 2026 name, `track` the classic one.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlaylistItem {
    #[serde(default)]
    pub item: Option<Track>,
    #[serde(default)]
    pub track: Option<Track>,
}

impl PlaylistItem {
    /// The track of the entry; `None` for an episode or a removed song.
    pub fn track(&self) -> Option<&Track> {
        self.item
            .as_ref()
            .or(self.track.as_ref())
            .filter(|t| t.kind.as_deref().is_none_or(|k| k == "track"))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SavedTrack {
    #[serde(default, deserialize_with = "null_default")]
    pub track: Track,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlayHistory {
    #[serde(default, deserialize_with = "null_default")]
    pub track: Track,
    #[serde(default)]
    pub played_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SearchResults {
    #[serde(default)]
    pub tracks: Option<Page<Track>>,
    #[serde(default)]
    pub artists: Option<Page<Artist>>,
    #[serde(default)]
    pub albums: Option<Page<Album>>,
    #[serde(default)]
    pub playlists: Option<Page<Playlist>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub product: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Tracks {
    #[serde(default, deserialize_with = "skip_nulls")]
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub error: ApiErrorDetail,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ApiErrorDetail {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub reason: Option<String>,
}
