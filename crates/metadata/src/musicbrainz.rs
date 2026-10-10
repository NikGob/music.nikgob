//! MusicBrainz web service v2 (`fmt=json`). Rate limit: 1 request/second, shared
//! by every clone of the [`Http`] this client was built from.

use std::time::Duration;

use serde::{Deserialize, Deserializer};
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url};

const DEFAULT_BASE: &str = "https://musicbrainz.org/ws/2/";
const PAGE_MAX: u32 = 100;
/// Safety valve for [`MusicBrainz::artist_discography`].
const DISCOGRAPHY_MAX_PAGES: u32 = 10;

/// MusicBrainz identifier (UUID string).
pub type Mbid = String;

// ---------------------------------------------------------------- types

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ArtistRef {
    pub id: Mbid,
    pub name: String,
    pub sort_name: String,
    #[serde(deserialize_with = "null_default")]
    pub disambiguation: String,
}

/// One slot of an artist credit: the name as credited, the artist, and the
/// phrase that joins it to the next slot (`" feat. "`, `" & "`, ...).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ArtistCredit {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub joinphrase: String,
    pub artist: ArtistRef,
}

/// Full credit line: `name + joinphrase` of every slot.
pub fn artist_credit_string(credits: &[ArtistCredit]) -> String {
    let mut out = String::new();
    for c in credits {
        let name = if c.name.is_empty() {
            &c.artist.name
        } else {
            &c.name
        };
        out.push_str(name);
        out.push_str(&c.joinphrase);
    }
    out.trim().to_owned()
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ReleaseGroup {
    pub id: Mbid,
    pub title: String,
    /// `Album`, `Single`, `EP`, `Broadcast`, `Other`.
    pub primary_type: Option<String>,
    /// `Live`, `Compilation`, `Soundtrack`, `Remix`, ...
    #[serde(deserialize_with = "null_default")]
    pub secondary_types: Vec<String>,
    #[serde(deserialize_with = "empty_none")]
    pub first_release_date: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub artist_credit: Vec<ArtistCredit>,
    /// Search relevance, 0..=100 (search results only).
    pub score: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Label {
    pub id: Mbid,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct LabelInfo {
    pub catalog_number: Option<String>,
    pub label: Option<Label>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Track {
    /// Track id (not the recording id).
    pub id: Mbid,
    /// Number as printed on the release (`"A1"`, `"3"`).
    #[serde(deserialize_with = "null_default")]
    pub number: String,
    /// 1-based position within the medium (0 when the response omits it).
    pub position: u32,
    pub title: String,
    pub length: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub artist_credit: Vec<ArtistCredit>,
    pub recording: Option<Box<Recording>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Medium {
    /// Disc number, 1-based.
    pub position: Option<u32>,
    pub format: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub title: String,
    pub track_count: u32,
    /// Search results call this array `track`, lookups call it `tracks`.
    #[serde(alias = "track", deserialize_with = "null_default")]
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Release {
    pub id: Mbid,
    pub title: String,
    /// `Official`, `Promotion`, `Bootleg`, `Pseudo-Release`.
    pub status: Option<String>,
    /// Possibly partial (`"2011"`, `"2011-05"`).
    #[serde(deserialize_with = "empty_none")]
    pub date: Option<String>,
    pub country: Option<String>,
    #[serde(deserialize_with = "empty_none")]
    pub barcode: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub artist_credit: Vec<ArtistCredit>,
    pub release_group: Option<ReleaseGroup>,
    #[serde(deserialize_with = "null_default")]
    pub label_info: Vec<LabelInfo>,
    #[serde(deserialize_with = "null_default")]
    pub media: Vec<Medium>,
}

impl Release {
    pub fn artist_credit_string(&self) -> String {
        artist_credit_string(&self.artist_credit)
    }

    pub fn label_name(&self) -> Option<&str> {
        self.label_info
            .iter()
            .filter_map(|l| l.label.as_ref())
            .map(|l| l.name.as_str())
            .find(|n| !n.is_empty())
    }

    pub fn is_official(&self) -> bool {
        self.status
            .as_deref()
            .is_none_or(|s| s.eq_ignore_ascii_case("official"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Recording {
    pub id: Mbid,
    pub title: String,
    /// Milliseconds.
    pub length: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub disambiguation: String,
    #[serde(deserialize_with = "null_default")]
    pub artist_credit: Vec<ArtistCredit>,
    #[serde(deserialize_with = "empty_none")]
    pub first_release_date: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub isrcs: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub releases: Vec<Release>,
    /// Search relevance, 0..=100 (search results only).
    pub score: Option<u32>,
}

impl Recording {
    pub fn artist_credit_string(&self) -> String {
        artist_credit_string(&self.artist_credit)
    }

    pub fn duration(&self) -> Option<Duration> {
        self.length.map(|ms| Duration::from_millis(u64::from(ms)))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Artist {
    pub id: Mbid,
    pub name: String,
    pub sort_name: String,
    #[serde(deserialize_with = "null_default")]
    pub disambiguation: String,
    pub country: Option<String>,
    /// `Person`, `Group`, ...
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

/// A page of a browse request.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub total: u32,
    pub offset: u32,
    pub items: Vec<T>,
}

/// Parameters of a recording search; rendered to a Lucene query.
#[derive(Debug, Clone)]
pub struct RecordingQuery {
    pub title: String,
    pub artist: Option<String>,
    pub release: Option<String>,
    pub duration: Option<Duration>,
    /// Window half-width around `duration`.
    pub duration_tolerance: Duration,
    pub limit: u32,
}

impl RecordingQuery {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            artist: None,
            release: None,
            duration: None,
            duration_tolerance: Duration::from_secs(10),
            limit: 10,
        }
    }

    #[must_use]
    pub fn artist(mut self, artist: impl Into<String>) -> Self {
        self.artist = Some(artist.into());
        self
    }

    #[must_use]
    pub fn release(mut self, release: impl Into<String>) -> Self {
        self.release = Some(release.into());
        self
    }

    #[must_use]
    pub fn duration(mut self, d: Duration) -> Self {
        self.duration = Some(d);
        self
    }

    #[must_use]
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// The Lucene query string sent as `query=`.
    pub fn to_lucene(&self) -> String {
        let mut parts = vec![format!("recording:{}", phrase(&self.title))];
        if let Some(a) = non_blank(self.artist.as_deref()) {
            parts.push(format!("artist:{}", phrase(a)));
        }
        if let Some(r) = non_blank(self.release.as_deref()) {
            parts.push(format!("release:{}", phrase(r)));
        }
        if let Some(d) = self.duration {
            let ms = d.as_millis() as u64;
            let tol = self.duration_tolerance.as_millis() as u64;
            parts.push(format!("dur:[{} TO {}]", ms.saturating_sub(tol), ms + tol));
        }
        parts.join(" AND ")
    }
}

fn non_blank(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// Quoted Lucene phrase with `\` and `"` escaped.
fn phrase(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.trim().chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

// ------------------------------------------------------------- client

#[derive(Clone, Debug)]
pub struct MusicBrainz {
    http: Http,
    base: Url,
}

impl MusicBrainz {
    /// Requires a contact in the config the `Http` was built from: MusicBrainz
    /// throttles anonymous user agents.
    pub fn new(http: Http) -> Result<Self> {
        if !http.has_contact() {
            return Err(MetadataError::NotConfigured(
                "MusicBrainz requires a contact in the User-Agent (MetadataConfig::contact)",
            ));
        }
        Ok(Self::with_base_url_unchecked(http, DEFAULT_BASE))
    }

    /// Points the client at another server (tests, mirrors).
    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            http,
            base: base_url(base)?,
        })
    }

    fn with_base_url_unchecked(http: Http, base: &str) -> Self {
        Self {
            http,
            base: base_url(base).expect("default base URL is valid"),
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let mut all = vec![("fmt", "json")];
        all.extend_from_slice(params);
        let url = build_url(&self.base, path, &all)?;
        self.http.get_json(Service::MusicBrainz, url).await
    }

    /// `inc=artists+releases+release-groups+isrcs+media`
    pub async fn lookup_recording(&self, mbid: &str) -> Result<Recording> {
        check_mbid(mbid)?;
        self.get(
            &format!("recording/{mbid}"),
            &[("inc", "artists+releases+release-groups+isrcs+media")],
        )
        .await
    }

    pub async fn search_recordings(&self, query: &RecordingQuery) -> Result<Vec<Recording>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            recordings: Vec<Recording>,
        }
        let lucene = query.to_lucene();
        let limit = query.limit.clamp(1, PAGE_MAX).to_string();
        let raw: Raw = self
            .get(
                "recording",
                &[("query", lucene.as_str()), ("limit", limit.as_str())],
            )
            .await?;
        Ok(raw.recordings)
    }

    /// `inc=recordings+artist-credits+labels+media+release-groups`
    pub async fn lookup_release(&self, mbid: &str) -> Result<Release> {
        check_mbid(mbid)?;
        self.get(
            &format!("release/{mbid}"),
            &[(
                "inc",
                "recordings+artist-credits+labels+media+release-groups",
            )],
        )
        .await
    }

    /// All editions of a release group.
    pub async fn browse_release_group_releases(
        &self,
        release_group_mbid: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Page<Release>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(rename = "release-count", default)]
            count: u32,
            #[serde(rename = "release-offset", default)]
            offset: u32,
            #[serde(default, deserialize_with = "null_default")]
            releases: Vec<Release>,
        }
        check_mbid(release_group_mbid)?;
        let limit = limit.clamp(1, PAGE_MAX).to_string();
        let offset = offset.to_string();
        let raw: Raw = self
            .get(
                "release",
                &[
                    ("release-group", release_group_mbid),
                    ("inc", "artist-credits+labels+media+release-groups"),
                    ("limit", limit.as_str()),
                    ("offset", offset.as_str()),
                ],
            )
            .await?;
        Ok(Page {
            total: raw.count,
            offset: raw.offset,
            items: raw.releases,
        })
    }

    pub async fn lookup_artist(&self, mbid: &str) -> Result<Artist> {
        check_mbid(mbid)?;
        self.get(&format!("artist/{mbid}"), &[]).await
    }

    /// One page of an artist's release groups (albums, EPs, singles, ...).
    pub async fn browse_artist_release_groups(
        &self,
        artist_mbid: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Page<ReleaseGroup>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(rename = "release-group-count", default)]
            count: u32,
            #[serde(rename = "release-group-offset", default)]
            offset: u32,
            #[serde(rename = "release-groups", default, deserialize_with = "null_default")]
            groups: Vec<ReleaseGroup>,
        }
        check_mbid(artist_mbid)?;
        let limit = limit.clamp(1, PAGE_MAX).to_string();
        let offset = offset.to_string();
        let raw: Raw = self
            .get(
                "release-group",
                &[
                    ("artist", artist_mbid),
                    ("inc", "artist-credits"),
                    ("limit", limit.as_str()),
                    ("offset", offset.as_str()),
                ],
            )
            .await?;
        Ok(Page {
            total: raw.count,
            offset: raw.offset,
            items: raw.groups,
        })
    }

    /// Every release group of the artist (pages through the browse endpoint,
    /// at most 1000 entries) for the "not in library" discography view.
    pub async fn artist_discography(&self, artist_mbid: &str) -> Result<Vec<ReleaseGroup>> {
        let mut all = Vec::new();
        for _ in 0..DISCOGRAPHY_MAX_PAGES {
            let offset = u32::try_from(all.len()).unwrap_or(u32::MAX);
            let page = self
                .browse_artist_release_groups(artist_mbid, PAGE_MAX, offset)
                .await?;
            let got = page.items.len();
            all.extend(page.items);
            if got == 0 || all.len() >= page.total as usize {
                break;
            }
        }
        Ok(all)
    }
}

/// MBIDs go into URL paths; refuse anything that is not UUID-shaped.
fn check_mbid(mbid: &str) -> Result<()> {
    let ok = mbid.len() == 36 && mbid.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(MetadataError::InvalidArgument(format!(
            "not a MusicBrainz id: {mbid:?}"
        )))
    }
}

fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn empty_none<'de, D>(d: D) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.filter(|s| !s.trim().is_empty()))
}
