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

/// Another name of an artist (`inc=aliases`, and in search results).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Alias {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub sort_name: String,
    /// `en`, `ja`, `ja_Latn`…
    pub locale: Option<String>,
    /// `Artist name`, `Legal name`, `Search hint`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub primary: bool,
    #[serde(deserialize_with = "null_default")]
    pub ended: bool,
}

/// A link of an artist (`inc=url-rels`): its Wikidata item, its site…
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Relation {
    /// `wikidata`, `official homepage`…
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    pub url: Option<RelationUrl>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RelationUrl {
    #[serde(deserialize_with = "null_default")]
    pub resource: String,
}

/// An artist with its other names and links: a search hit, or
/// [`MusicBrainz::artist_info`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ArtistInfo {
    pub id: Mbid,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub sort_name: String,
    /// `Person`, `Group`, `Character`…
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Search relevance, 0..=100 (search results only).
    pub score: Option<u32>,
    #[serde(deserialize_with = "null_default")]
    pub aliases: Vec<Alias>,
    #[serde(deserialize_with = "null_default")]
    pub relations: Vec<Relation>,
}

impl ArtistInfo {
    /// A role (an anime character credited for its songs), not a person.
    pub fn is_character(&self) -> bool {
        self.kind.as_deref().is_some_and(|k| k.eq_ignore_ascii_case("character"))
    }

    /// Its Wikidata item (`Q…`), from its links.
    pub fn wikidata(&self) -> Option<String> {
        self.relations
            .iter()
            .filter(|r| r.kind.eq_ignore_ascii_case("wikidata"))
            .filter_map(|r| r.url.as_ref())
            .find_map(|u| wikidata_id(&u.resource))
    }

    /// Its Latin name ([`latin_alias`]).
    pub fn latin_name(&self) -> Option<String> {
        latin_alias(&self.name, &self.sort_name, &self.aliases)
    }

    /// Its name or one of its aliases is `name` (folded like artist names).
    pub fn answers_to(&self, name: &str) -> bool {
        let want = crate::text::artist_key(name);
        !want.is_empty()
            && (crate::text::artist_key(&self.name) == want
                || self.aliases.iter().any(|a| crate::text::artist_key(&a.name) == want))
    }
}

/// `https://www.wikidata.org/wiki/Q42` → `Q42`.
pub fn wikidata_id(url: &str) -> Option<String> {
    let id = url.trim().trim_end_matches('/').rsplit('/').next()?;
    let digits = id.strip_prefix('Q')?;
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then(|| id.to_owned())
}

/// The Latin name of an artist from its MusicBrainz names, best first:
///
/// 1. its own name, when MusicBrainz writes it in Latin letters;
/// 2. an alias in English (`en`) or in a Latin-script locale (`ja-Latn`):
///    a primary English one, a primary Latin-script one, then the others
///    in that order;
/// 3. another primary alias in Latin letters;
/// 4. any other Latin alias that is an artist or legal name;
/// 5. its sort name in reading order (`Saso, Ayako` → `Ayako Saso`).
///
/// Search hints and ended aliases never count (they are misspellings and
/// old names).
pub fn latin_alias(name: &str, sort_name: &str, aliases: &[Alias]) -> Option<String> {
    let latin = |s: &str| nikgob_core::names::is_latin(s.trim());
    if latin(name) {
        return Some(name.trim().to_owned());
    }
    let rank = |a: &Alias| -> Option<u8> {
        let hint = a.kind.as_deref().is_some_and(|k| k.eq_ignore_ascii_case("search hint"));
        if a.ended || hint || !latin(&a.name) {
            return None;
        }
        let locale = a.locale.as_deref().unwrap_or_default().to_ascii_lowercase().replace('_', "-");
        let english = locale == "en" || locale.starts_with("en-");
        let script = locale.ends_with("-latn");
        Some(match (english, script, a.primary) {
            (true, _, true) => 0,
            (false, true, true) => 1,
            (true, _, false) => 2,
            (false, true, false) => 3,
            (false, false, true) => 4,
            (false, false, false) => 5,
        })
    };
    if let Some((_, best)) = aliases.iter().filter_map(|a| rank(a).map(|r| (r, a))).min_by_key(|(r, _)| *r) {
        return Some(best.name.trim().to_owned());
    }
    let sort = sort_name.trim();
    if !latin(sort) {
        return None;
    }
    Some(match sort.split_once(", ") {
        Some((last, first)) if !last.trim().is_empty() && !first.trim().is_empty() && !first.contains(',') => {
            format!("{} {}", first.trim(), last.trim())
        }
        _ => sort.to_owned(),
    })
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

    /// Releases called `title` by `artist` (an album found as a whole: a
    /// downloaded folder is matched to one of them by its track count and
    /// lengths). Search results list media with their track counts only;
    /// [`Self::lookup_release`] gives the tracks.
    pub async fn search_releases(&self, artist: &str, title: &str, limit: u32) -> Result<Vec<Release>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            releases: Vec<Release>,
        }
        let lucene = format!("release:{} AND artist:{}", phrase(title), phrase(artist));
        let limit = limit.clamp(1, PAGE_MAX).to_string();
        let raw: Raw = self
            .get("release", &[("query", lucene.as_str()), ("limit", limit.as_str())])
            .await?;
        Ok(raw.releases)
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

    /// The artist called `name` (the best hit whose name folds to it, a
    /// score of 90 at least) with its tags and genres, one request: the
    /// graph of similar music reads them.
    pub async fn search_artist_tags(&self, name: &str) -> Result<Option<ArtistTags>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            artists: Vec<RawTagged>,
        }
        let query = format!("artist:{}", phrase(name));
        let raw: Raw = self.get("artist", &[("query", query.as_str()), ("limit", "5")]).await?;
        let want = crate::text::artist_key(name);
        Ok(raw
            .artists
            .into_iter()
            .filter(|a| a.score.unwrap_or(0) >= 90 && (crate::text::artist_key(&a.name) == want || a.aliases.iter().any(|x| crate::text::artist_key(&x.name) == want)))
            .map(RawTagged::tags)
            .next())
    }

    /// Artists called `name` by their name or one of their aliases (folded
    /// like artist names), with a score of 90 at least, best first.
    pub async fn search_artists(&self, name: &str, limit: u32) -> Result<Vec<ArtistInfo>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            artists: Vec<ArtistInfo>,
        }
        let p = phrase(name);
        let query = format!("artist:{p} OR alias:{p}");
        let limit = limit.clamp(1, PAGE_MAX).to_string();
        let raw: Raw = self.get("artist", &[("query", query.as_str()), ("limit", limit.as_str())]).await?;
        Ok(raw
            .artists
            .into_iter()
            .filter(|a| a.score.unwrap_or(0) >= 90 && a.answers_to(name))
            .collect())
    }

    /// The artist `mbid` with its aliases and its links (`inc=aliases+url-rels`).
    pub async fn artist_info(&self, mbid: &str) -> Result<ArtistInfo> {
        check_mbid(mbid)?;
        self.get(&format!("artist/{mbid}"), &[("inc", "aliases+url-rels")]).await
    }

    /// How many distinct titles of `titles` (the library's titles of an
    /// artist found by name) the artist `mbid` has as recordings or
    /// release groups, generic ones not counted ([`crate::text::titles_shared`]):
    /// two are the proof that it is the library's. A recording search for
    /// up to eight of the titles, then the first page of its release
    /// groups when that is not enough.
    pub async fn artist_shared_titles(&self, mbid: &str, titles: &[String]) -> Result<usize> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            recordings: Vec<Recording>,
        }
        check_mbid(mbid)?;
        let mut asked: Vec<String> = Vec::new();
        for t in titles {
            let t = crate::text::search_title(t);
            if !t.trim().is_empty() && !crate::text::generic_title(&crate::text::album_key(&t)) && !asked.contains(&t) {
                asked.push(t);
            }
            if asked.len() == 8 {
                break;
            }
        }
        if asked.is_empty() {
            return Ok(0);
        }
        let any = asked.iter().map(|t| format!("recording:{}", phrase(t))).collect::<Vec<_>>().join(" OR ");
        let query = format!("arid:{mbid} AND ({any})");
        let raw: Raw = self.get("recording", &[("query", query.as_str()), ("limit", "25")]).await?;
        let mut theirs: Vec<String> = raw.recordings.into_iter().map(|r| r.title).collect();
        let shared = crate::text::titles_shared(titles, &theirs);
        if shared >= crate::text::SHARED_TITLES_PROOF {
            return Ok(shared);
        }
        let groups = self.browse_artist_release_groups(mbid, PAGE_MAX, 0).await?;
        theirs.extend(groups.items.into_iter().map(|g| g.title));
        Ok(crate::text::titles_shared(titles, &theirs))
    }

    /// The tags and genres of the artist `mbid`.
    pub async fn artist_tags(&self, mbid: &str) -> Result<ArtistTags> {
        check_mbid(mbid)?;
        let raw: RawTagged = self.get(&format!("artist/{mbid}"), &[("inc", "tags+genres")]).await?;
        Ok(raw.tags())
    }
}

/// An artist's MusicBrainz id and its tags and genres with their votes
/// (a genre counts as its tag; the larger vote stands).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistTags {
    pub id: String,
    pub name: String,
    pub tags: Vec<(String, u32)>,
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
    #[serde(default)]
    count: i64,
}

#[derive(Deserialize)]
struct RawAlias {
    name: String,
}

#[derive(Deserialize)]
struct RawTagged {
    id: String,
    name: String,
    #[serde(default)]
    score: Option<u32>,
    #[serde(default, deserialize_with = "null_default")]
    aliases: Vec<RawAlias>,
    #[serde(default, deserialize_with = "null_default")]
    tags: Vec<RawTag>,
    #[serde(default, deserialize_with = "null_default")]
    genres: Vec<RawTag>,
}

impl RawTagged {
    fn tags(self) -> ArtistTags {
        let mut by: Vec<(String, u32)> = Vec::new();
        for t in self.tags.into_iter().chain(self.genres) {
            let name = t.name.trim().to_lowercase();
            let votes = u32::try_from(t.count.max(0)).unwrap_or(0);
            if name.is_empty() {
                continue;
            }
            match by.iter_mut().find(|x| x.0 == name) {
                Some(x) => x.1 = x.1.max(votes),
                None => by.push((name, votes)),
            }
        }
        by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ArtistTags { id: self.id, name: self.name, tags: by }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(name: &str, locale: Option<&str>, kind: &str, primary: bool) -> Alias {
        Alias {
            name: name.into(),
            sort_name: String::new(),
            locale: locale.map(str::to_owned),
            kind: Some(kind.into()),
            primary,
            ended: false,
        }
    }

    #[test]
    fn the_latin_name_is_the_english_or_latin_script_alias_first() {
        let aliases = vec![
            alias("さそう あやこ", Some("ja"), "Artist name", true),
            alias("Sasou Ayako", Some("ja_Latn"), "Artist name", true),
            alias("Ayako Saso", Some("en"), "Artist name", true),
            alias("Ayako Sasō", None, "Legal name", false),
            alias("Ayaco", None, "Search hint", false),
        ];
        assert_eq!(latin_alias("佐宗綾子", "Saso, Ayako", &aliases).as_deref(), Some("Ayako Saso"));
        // Without an English one, the Latin-script locale.
        assert_eq!(latin_alias("佐宗綾子", "Saso, Ayako", &aliases[..2]).as_deref(), Some("Sasou Ayako"));
        // A primary alias beats one that is not.
        let mixed = vec![alias("Saso", Some("en"), "Artist name", false), alias("Sasou Ayako", Some("ja-Latn"), "Artist name", true)];
        assert_eq!(latin_alias("佐宗綾子", "", &mixed).as_deref(), Some("Sasou Ayako"));
        // Search hints and ended names never; then the sort name read in order.
        let mut ended = alias("Old Name", Some("en"), "Artist name", true);
        ended.ended = true;
        let poor = vec![alias("Ayaco", None, "Search hint", false), ended];
        assert_eq!(latin_alias("佐宗綾子", "Saso, Ayako", &poor).as_deref(), Some("Ayako Saso"));
        assert_eq!(latin_alias("佐宗綾子", "佐宗綾子", &poor), None);
        // MusicBrainz's own Latin name stands.
        assert_eq!(latin_alias("YOASOBI", "YOASOBI", &aliases).as_deref(), Some("YOASOBI"));
        assert_eq!(latin_alias("ザ・ビートルズ", "Beatles, The", &[]).as_deref(), Some("The Beatles"));
    }

    #[test]
    fn an_artist_reads_its_type_aliases_and_wikidata_link() {
        let json = r#"{"id": "0b2f0b5a-0000-4000-8000-000000000001", "name": "リコ", "sort-name": "Riko",
            "type": "Character", "disambiguation": null,
            "aliases": [{"name": "Riko", "sort-name": "Riko", "locale": "en", "type": "Artist name",
                         "primary": null, "ended": false, "begin": null, "end": null}],
            "relations": [{"type": "wikidata", "url": {"resource": "https://www.wikidata.org/wiki/Q1234567"}},
                          {"type": "official homepage", "url": {"resource": "https://example.invalid/"}}]}"#;
        let a: ArtistInfo = serde_json::from_str(json).unwrap();
        assert!(a.is_character());
        assert_eq!(a.wikidata().as_deref(), Some("Q1234567"));
        assert_eq!(a.latin_name().as_deref(), Some("Riko"));
        assert!(a.answers_to("リコ") && a.answers_to("riko") && !a.answers_to("Reg"));
        assert_eq!(wikidata_id("https://www.wikidata.org/wiki/Q42/"), Some("Q42".into()));
        assert_eq!(wikidata_id("https://www.wikidata.org/wiki/Property:P18"), None);
    }
}
