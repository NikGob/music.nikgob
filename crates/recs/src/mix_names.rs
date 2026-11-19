//! What a mix is called by what it is made of: «Хип-хоп микс», «Микс:
//! ZUTOMAYO», «Микс 90-х», «Спокойный микс».
//!
//! A name is the first that holds for the mix and no other card has:
//!
//! 1. **The genre** its songs share ([`genre`]): the tags other listeners
//!    gave its artists (MusicBrainz's tags and genres, as the graph of
//!    similar music keeps them), each folded into a family a person names
//!    ([`family`]). It names the mix when [`TAGGED_SHARE`] of the songs have
//!    a family and one family holds [`GENRE_SHARE`] of the mix.
//! 2. **The leading artist**, when [`ARTIST_SHARE`] of the songs (three at
//!    least) are theirs.
//! 3. **The era**: a decade with [`ERA_SHARE`] of the dated songs, unless it
//!    is the decade most of the library is of anyway.
//! 4. **The mood**: the calm or the lively end of the library's energy.
//! 5. **The kind** its taste profile is of («Русские песни»).
//! 6. The second genre, then every artist in turn, then a song: something
//!    of the mix itself, so the name is never empty and never a number.
//!
//! A family is also what a genre mix is built by ([`crate::mixes`]): an
//! artist is of the family that leads their tags with [`LEAD_SHARE`] of
//! them ([`leading`]), a song of its main artist's, and the mix is called
//! by the family's name ([`family_title`]). A named mix whose own genre
//! has such a mix in the row is that mix again and is not shown.
//!
//! Pure: the runtime brings the tags, the names and the years.

use nikgob_core::i18n::{Locale, MixTitle, genre_family_title_in};

use crate::profiles::{Kind, Loudness};

/// A song's main artist's tags with their votes.
pub type Tags = [(String, f32)];

/// This share of a mix's songs must have a known family for a genre to
/// name it…
pub const TAGGED_SHARE: f32 = 0.5;
/// …and one family this share of the whole mix.
pub const GENRE_SHARE: f32 = 0.4;
/// A second family names a mix (after the other names) from this share.
const GENRE_SECOND: f32 = 0.25;
/// The leading artist names a mix with this share of its songs…
pub const ARTIST_SHARE: f32 = 0.15;
/// …and this many of them.
pub const ARTIST_SONGS: usize = 3;
/// A decade names a mix with this share of its dated songs, when this
/// share of the songs are dated.
pub const ERA_SHARE: f32 = 0.6;
const DATED_SHARE: f32 = 0.5;
/// The calm and the lively end: where the mix's energy stands among the
/// library's songs.
pub const CALM_BELOW: f32 = 0.25;
pub const LIVELY_ABOVE: f32 = 0.75;

/// An artist is of a family when it has this share of what their tags say
/// of genres: below it they are of several and of no one's mix.
pub const LEAD_SHARE: f32 = 0.5;

/// The families of genres with their numbers (titles live in
/// [`nikgob_core::i18n::genre_family_title_in`]), the more special first:
/// a tag is of the first family one of whose keys it has. A key is a run
/// of whole words («hip hop» in «southern hip hop»); one starting with `*`
/// is found anywhere in a word («*rock» in «krautrock»). A number is the
/// family's for good (the caches and the mixes left out keep it): a new
/// family takes a new one wherever it stands, none is used again.
const FAMILIES: &[(u8, &[&str])] = &[
    (1, &["j pop", "jpop", "japanese pop", "city pop", "kayokyoku", "anison", "vocaloid", "utaite"]),
    (2, &["k pop", "kpop", "korean pop"]),
    (3, &["j rock", "jrock", "japanese rock", "visual kei"]),
    (4, &["post punk", "new wave", "coldwave", "darkwave", "gothic rock"]),
    (5, &["latin", "latino", "latina", "*reggaeton", "salsa", "bachata", "cumbia", "bossa nova"]),
    (6, &["*reggae", "dancehall", "ska", "dub"]),
    (7, &["hip hop", "hiphop", "*rap", "drill", "grime", "boom bap", "phonk"]),
    (8, &["*metal", "deathcore", "djent", "grindcore"]),
    (9, &["punk", "emo"]),
    (10, &["*indie"]),
    (11, &["*rock", "grunge", "shoegaze"]),
    (12, &["*pop"]),
    (13, &["soundtrack", "soundtracks", "score", "film score", "video game", "video game music", "vgm", "game music", "anime", "ost"]),
    (14, &["lo fi", "lofi", "chillhop"]),
    (15, &["ambient", "drone", "new age"]),
    (
        16,
        &["*electro", "house", "*techno", "trance", "edm", "dance", "drum and bass", "dnb", "dubstep", "garage", "breakbeat", "idm", "*synth", "future bass", "footwork", "hardstyle"],
    ),
    (17, &["r&b", "rnb", "*soul", "*funk", "motown"]),
    (18, &["*jazz", "swing", "bebop"]),
    (19, &["*blues"]),
    (20, &["classical", "baroque", "opera", "orchestral", "symphony", "chamber music"]),
    (21, &["country", "bluegrass", "americana"]),
    (22, &["*folk", "celtic"]),
    (23, &["*disco"]),
];

/// Whether `id` is a known genre family.
pub fn has_family(id: u8) -> bool {
    FAMILIES.iter().any(|f| f.0 == id)
}

/// The family ID of the tag `tag`, if it is of one: «russian rap» is hip hop
/// (`7`), «2010s» and «vtuber» are no genre.
pub fn family(tag: &str) -> Option<u8> {
    let words: Vec<String> = tag
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '&'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    if words.is_empty() {
        return None;
    }
    let padded = format!(" {} ", words.join(" "));
    let has = |key: &str| match key.strip_prefix('*') {
        Some(part) => words.iter().any(|w| w.contains(part)),
        None => padded.contains(&format!(" {key} ")),
    };
    FAMILIES.iter().find(|(_, keys)| keys.iter().any(|k| has(k))).map(|(id, _)| *id)
}

/// The localized name of the mix of the family of the number `id`.
pub fn family_title_in(id: u8, loc: Locale) -> Option<&'static str> {
    has_family(id).then(|| genre_family_title_in(loc, id)).flatten()
}

/// The number of the family whose mix is called `title` in `loc`.
pub fn family_id_in(title: &str, loc: Locale) -> Option<u8> {
    FAMILIES.iter().find(|f| family_title_in(f.0, loc) == Some(title)).map(|f| f.0)
}

/// The family an artist of `tags` is of (its number): the one that leads
/// them with [`LEAD_SHARE`]; `None` for an artist nobody tagged with a
/// genre, or of several genres alike.
pub fn leading(tags: &Tags) -> Option<u8> {
    let of = families(tags);
    let (id, share) = of.first()?;
    (*share >= LEAD_SHARE).then_some(*id)
}

/// The families of an artist's `tags` (by family ID) with their shares of the
/// artist (they sum to 1), the largest first; empty when no tag is of a
/// family. A tag counts by its votes and one more (a tag nobody voted for
/// still says something).
pub fn families(tags: &Tags) -> Vec<(u8, f32)> {
    let mut out: Vec<(u8, f32)> = Vec::new();
    for (tag, votes) in tags {
        let Some(f) = family(tag) else { continue };
        let w = votes.max(0.0) + 1.0;
        match out.iter_mut().find(|o| o.0 == f) {
            Some(o) => o.1 += w,
            None => out.push((f, w)),
        }
    }
    let sum: f32 = out.iter().map(|o| o.1).sum();
    if sum > 0.0 {
        for o in &mut out {
            o.1 /= sum;
        }
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// What a mix is made of, for its name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Makeup {
    /// The tags of each song's main artist (empty: none known).
    pub tags: Vec<Vec<(String, f32)>>,
    /// Its artists with their songs in it, most songs first.
    pub artists: Vec<(String, usize)>,
    /// Each song's year, where the library knows it.
    pub years: Vec<Option<i32>>,
    /// Where its energy stands among the library's songs, 0 (the calmest)
    /// to 1; `None` without a sound.
    pub energy: Option<f32>,
    /// Its taste profile's kind and optional loudness disambiguator.
    pub kind: Option<(Kind, Option<Loudness>)>,
    /// Its first songs' titles (the last resort).
    pub songs: Vec<String>,
}

impl Makeup {
    fn len(&self) -> usize {
        self.tags.len().max(self.years.len()).max(self.artists.iter().map(|a| a.1).sum::<usize>()).max(1)
    }
}

/// The genre family IDs that name `m`, the first its own and the second one
/// that may stand in ([`GENRE_SHARE`], [`GENRE_SECOND`]).
pub fn genre(m: &Makeup) -> Vec<u8> {
    let n = m.tags.len();
    if n == 0 {
        return Vec::new();
    }
    let mut tagged = 0usize;
    let mut sum: Vec<(u8, f32)> = Vec::new();
    for tags in &m.tags {
        let of = families(tags);
        if of.is_empty() {
            continue;
        }
        tagged += 1;
        for (f, share) in of {
            match sum.iter_mut().find(|s| s.0 == f) {
                Some(s) => s.1 += share,
                None => sum.push((f, share)),
            }
        }
    }
    if (tagged as f32) < TAGGED_SHARE * n as f32 {
        return Vec::new();
    }
    sum.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out = Vec::new();
    if let Some((first, weight)) = sum.first()
        && *weight >= GENRE_SHARE * n as f32
    {
        out.push(*first);
        if let Some((second, weight)) = sum.get(1)
            && *weight >= GENRE_SECOND * n as f32
        {
            out.push(*second);
        }
    }
    out
}

/// «Микс 90-х», «Микс 2010-х» / «90s Mix», «2010s Mix».
pub fn era_title_in(decade: i32, loc: Locale) -> String {
    MixTitle::Era(decade).title_in(loc).into_owned()
}

/// «Микс: ZUTOMAYO» / «ZUTOMAYO Mix».
pub fn artist_title_in(name: &str, loc: Locale) -> String {
    MixTitle::Artist(name.to_owned()).title_in(loc).into_owned()
}

/// «Спокойный микс» / «Бодрый микс» / «Calm Mix» / «Upbeat Mix».
pub fn energy_title_in(calm: bool, loc: Locale) -> String {
    MixTitle::Energy { calm }.title_in(loc).into_owned()
}

/// The decade that names `m` ([`ERA_SHARE`]); `common` is the decade most
/// of the library is of, which tells no mix apart.
pub fn era(m: &Makeup, common: Option<i32>) -> Option<i32> {
    let dated: Vec<i32> = m.years.iter().flatten().filter(|y| (1900..2100).contains(*y)).map(|y| y / 10 * 10).collect();
    if dated.is_empty() || (dated.len() as f32) < DATED_SHARE * m.years.len() as f32 {
        return None;
    }
    let mut count: Vec<(i32, usize)> = Vec::new();
    for d in &dated {
        match count.iter_mut().find(|c| c.0 == *d) {
            Some(c) => c.1 += 1,
            None => count.push((*d, 1)),
        }
    }
    count.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    let (decade, n) = count[0];
    (n as f32 >= ERA_SHARE * dated.len() as f32 && Some(decade) != common).then_some(decade)
}

/// Typed candidate name for a named daily mix, before rendering to a [`Locale`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MixName {
    Genre(u8),
    Artist(String),
    Era(i32),
    Energy { calm: bool },
    Kind { kind: Kind, loudness: Option<Loudness> },
    Song(String),
}

impl MixName {
    /// Renders `self` in `loc`.
    pub fn title_in(&self, loc: Locale) -> String {
        match self {
            Self::Genre(id) => family_title_in(*id, loc).unwrap_or_default().to_owned(),
            Self::Artist(name) | Self::Song(name) => artist_title_in(name, loc),
            Self::Era(decade) => era_title_in(*decade, loc),
            Self::Energy { calm } => energy_title_in(*calm, loc),
            Self::Kind { kind, loudness } => match loudness {
                Some(loud) => format!("{}, {}", kind.title_in(loc), loud.word_in(loc)),
                None => kind.title_in(loc),
            },
        }
    }

    fn dedup_key(&self) -> DedupKey {
        match self {
            Self::Genre(id) => DedupKey::Genre(*id),
            Self::Era(d) => DedupKey::Era(*d),
            Self::Energy { calm } => DedupKey::Energy(*calm),
            Self::Kind { kind, loudness } => DedupKey::Kind(*kind, *loudness),
            Self::Artist(s) | Self::Song(s) => DedupKey::Subject(s.trim().to_lowercase()),
        }
    }
}

#[derive(PartialEq, Eq)]
enum DedupKey {
    Genre(u8),
    Era(i32),
    Energy(bool),
    Kind(Kind, Option<Loudness>),
    Subject(String),
}

/// The typed candidate names `m` may have, the best first (see the module docs).
pub fn candidates(m: &Makeup, common_decade: Option<i32>) -> Vec<MixName> {
    let mut out: Vec<MixName> = Vec::new();
    let genres = genre(m);
    if let Some(&g) = genres.first()
        && has_family(g)
    {
        out.push(MixName::Genre(g));
    }
    let n = m.len();
    if let Some((name, songs)) = m.artists.first()
        && !name.trim().is_empty()
        && *songs >= ARTIST_SONGS
        && *songs as f32 >= ARTIST_SHARE * n as f32
    {
        out.push(MixName::Artist(name.trim().to_owned()));
    }
    if let Some(d) = era(m, common_decade) {
        out.push(MixName::Era(d));
    }
    match m.energy {
        Some(e) if e <= CALM_BELOW => out.push(MixName::Energy { calm: true }),
        Some(e) if e >= LIVELY_ABOVE => out.push(MixName::Energy { calm: false }),
        _ => {}
    }
    if let Some((kind, loudness)) = m.kind {
        out.push(MixName::Kind { kind, loudness });
    }
    if let Some(&g) = genres.get(1)
        && has_family(g)
    {
        out.push(MixName::Genre(g));
    }
    out.extend(m.artists.iter().map(|a| a.0.trim()).filter(|n| !n.is_empty()).map(|n| MixName::Artist(n.to_owned())));
    out.extend(m.songs.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| MixName::Song(s.to_owned())));
    let mut seen: Vec<DedupKey> = Vec::new();
    out.retain(|c| {
        let key = c.dedup_key();
        let fresh = !seen.contains(&key);
        if fresh {
            seen.push(key);
        }
        fresh
    });
    out
}

/// A name for each of `mixes` in their order in `loc`: the first of its
/// candidates no card before it and none of `taken` (the other cards of the
/// row) has. `None`: nothing of the mix can name it (it has no artist and
/// no song with a name), so it is not shown.
pub fn assign_in(mixes: &[Makeup], taken: &[String], common_decade: Option<i32>, loc: Locale) -> Vec<Option<(MixName, String)>> {
    let mut used: Vec<String> = taken.iter().map(|t| t.trim().to_lowercase()).collect();
    mixes
        .iter()
        .map(|m| {
            let (cand, rendered) = candidates(m, common_decade)
                .into_iter()
                .map(|c| {
                    let t = c.title_in(loc);
                    (c, t)
                })
                .find(|(_, t)| {
                    let key = t.trim().to_lowercase();
                    !key.is_empty() && !used.contains(&key)
                })?;
            used.push(rendered.trim().to_lowercase());
            Some((cand, rendered))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(t: &[(&str, f32)]) -> Vec<(String, f32)> {
        t.iter().map(|(n, w)| ((*n).to_owned(), *w)).collect()
    }

    fn artists(a: &[(&str, usize)]) -> Vec<(String, usize)> {
        a.iter().map(|(n, k)| ((*n).to_owned(), *k)).collect()
    }

    #[test]
    fn a_tag_is_of_its_family() {
        for (tag, want) in [
            ("hip hop", Some("Хип-хоп микс")),
            ("Southern Hip-Hop", Some("Хип-хоп микс")),
            ("russian rap", Some("Хип-хоп микс")),
            ("pop rap", Some("Хип-хоп микс")),
            ("trap", Some("Хип-хоп микс")),
            ("cloud rap", Some("Хип-хоп микс")),
            ("hardcore hip hop", Some("Хип-хоп микс")),
            ("j-pop", Some("J-pop микс")),
            ("j-rock", Some("J-rock микс")),
            ("k-pop", Some("K-pop микс")),
            ("vocaloid", Some("J-pop микс")),
            ("pop", Some("Поп-микс")),
            ("electropop", Some("Поп-микс")),
            ("synth-pop", Some("Поп-микс")),
            ("indie pop", Some("Инди-микс")),
            ("indie rock", Some("Инди-микс")),
            ("alternative rock", Some("Рок-микс")),
            ("pop rock", Some("Рок-микс")),
            ("garage rock", Some("Рок-микс")),
            ("pop punk", Some("Панк-микс")),
            ("post-punk", Some("Пост-панк микс")),
            ("heavy metal", Some("Метал-микс")),
            ("metalcore", Some("Метал-микс")),
            ("electronic", Some("Электронный микс")),
            ("future bass", Some("Электронный микс")),
            ("dubstep", Some("Электронный микс")),
            ("house", Some("Электронный микс")),
            ("r&b", Some("R&B микс")),
            ("contemporary r&b", Some("R&B микс")),
            ("soul", Some("R&B микс")),
            ("reggaeton", Some("Латинский микс")),
            ("trap latino", Some("Латинский микс")),
            ("reggae", Some("Регги-микс")),
            ("video game music", Some("Микс саундтреков")),
            ("vgm", Some("Микс саундтреков")),
            ("soundtrack", Some("Микс саундтреков")),
            ("lo-fi", Some("Лоу-фай микс")),
            ("jazz fusion", Some("Джазовый микс")),
            ("singer-songwriter", None),
            ("cyberpunk", None),
            ("platinum", None),
            ("2010s", None),
            ("vtuber", None),
            ("english", None),
            ("russian", None),
            // «hardcore» has «score» in it and is no soundtrack.
            ("hardcore", None),
            ("", None),
        ] {
            assert_eq!(family(tag).and_then(|id| family_title_in(id, Locale::Ru)), want, "{tag}");
        }
        assert_eq!(family("hip hop").and_then(|id| family_title_in(id, Locale::En)), Some("Hip-Hop Mix"));
        assert_eq!(family("soundtrack").and_then(|id| family_title_in(id, Locale::En)), Some("Soundtrack Mix"));
        let mut ids: Vec<u8> = Vec::new();
        for (id, keys) in FAMILIES {
            assert!(!keys.is_empty(), "{id}");
            assert!(!ids.contains(id), "the number {id} is another family's");
            ids.push(*id);
            let ru = family_title_in(*id, Locale::Ru).expect("ru title in nikgob-core");
            let en = family_title_in(*id, Locale::En).expect("en title in nikgob-core");
            assert!(ru.contains("икс"), "{ru}");
            assert!(en.ends_with("Mix"), "{en}");
            assert_eq!(genre_family_title_in(Locale::Ru, *id), Some(ru));
            assert_eq!(genre_family_title_in(Locale::En, *id), Some(en));
            assert_eq!(family_id_in(ru, Locale::Ru), Some(*id));
            assert_eq!(family_id_in(en, Locale::En), Some(*id));
        }
        assert_eq!(family_title_in(0, Locale::Ru), None);
        assert_eq!(family_title_in(0, Locale::En), None);
    }

    #[test]
    fn an_artists_families_share_them() {
        let f = families(&tags(&[("hip hop", 9.0), ("pop rap", 3.0), ("pop", 3.0), ("2010s", 5.0), ("relic inn", 0.0)]));
        assert_eq!(f[0].0, 7);
        assert!((f[0].1 - 14.0 / 18.0).abs() < 1e-4, "{f:?}");
        assert_eq!(f[1].0, 12);
        assert!((f.iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(families(&tags(&[("2010s", 5.0)])).is_empty());
        // The artist is a rapper; one of two genres alike is of neither,
        // one nobody tagged with a genre of none.
        assert_eq!(leading(&tags(&[("hip hop", 9.0), ("pop rap", 3.0), ("pop", 3.0)])).and_then(|id| family_title_in(id, Locale::Ru)), Some("Хип-хоп микс"));
        assert_eq!(leading(&tags(&[("hip hop", 9.0), ("pop rap", 3.0), ("pop", 3.0)])).and_then(|id| family_title_in(id, Locale::En)), Some("Hip-Hop Mix"));
        assert_eq!(leading(&tags(&[("rock", 2.0), ("jazz", 2.0), ("folk", 2.0)])), None);
        assert_eq!(leading(&tags(&[("2010s", 5.0)])), None);
        assert_eq!(leading(&[]), None);
    }

    /// A mix of mostly rappers is «Хип-хоп микс» / «Hip-Hop Mix»; one whose
    /// artists nobody tagged is named by its leading artist; one without a
    /// leader by its decade, its mood or its kind.
    #[test]
    fn a_mix_is_named_by_what_it_is_made_of() {
        use crate::songtype::Lang;
        let rap = tags(&[("hip hop", 5.0), ("trap", 1.0)]);
        let pop = tags(&[("pop", 4.0)]);
        let hip_hop = Makeup {
            tags: (0..20).map(|k| if k < 12 { rap.clone() } else if k < 15 { pop.clone() } else { Vec::new() }).collect(),
            artists: artists(&[("MORGENSHTERN", 5), ("SLAVA MARLOW", 4)]),
            years: vec![Some(2021); 20],
            energy: Some(0.8),
            kind: Some((Kind::Sung(Lang::Russian), None)),
            songs: vec!["Cadillac".into()],
        };
        assert_eq!(genre(&hip_hop), [7]);
        let c: Vec<String> = candidates(&hip_hop, Some(2020)).iter().map(|x| x.title_in(Locale::Ru)).collect();
        assert_eq!(c[..5], ["Хип-хоп микс", "Микс: MORGENSHTERN", "Бодрый микс", "Русские песни", "Микс: SLAVA MARLOW"], "{c:?}");
        assert!(c.contains(&"Микс: Cadillac".to_owned()));
        let c_en: Vec<String> = candidates(&hip_hop, Some(2020)).iter().map(|x| x.title_in(Locale::En)).collect();
        assert_eq!(c_en[..5], ["Hip-Hop Mix", "MORGENSHTERN Mix", "Upbeat Mix", "Russian Songs", "SLAVA MARLOW Mix"], "{c_en:?}");
        // The library is not all of the 2020s: the decade is a name too.
        assert!(candidates(&hip_hop, Some(2010)).iter().map(|x| x.title_in(Locale::Ru)).any(|t| t == "Микс 2020-х"));
        assert!(candidates(&hip_hop, Some(2010)).iter().map(|x| x.title_in(Locale::En)).any(|t| t == "2020s Mix"));

        // Too few songs with a genre: no genre, the leading artist.
        let untagged = Makeup {
            tags: (0..20).map(|k| if k < 6 { rap.clone() } else { Vec::new() }).collect(),
            artists: artists(&[("ZUTOMAYO", 6), ("Eve", 2)]),
            ..Makeup::default()
        };
        assert!(genre(&untagged).is_empty());
        assert_eq!(candidates(&untagged, None)[0].title_in(Locale::Ru), "Микс: ZUTOMAYO");
        assert_eq!(candidates(&untagged, None)[0].title_in(Locale::En), "ZUTOMAYO Mix");

        // Tagged, but of every genre a little: no genre either.
        let spread = Makeup {
            tags: ["rock", "jazz", "pop", "hip hop", "folk", "metal"].iter().cycle().take(18).map(|t| tags(&[(t, 1.0)])).collect(),
            artists: artists(&[("A", 1), ("B", 1)]),
            years: (0..18).map(|k| Some(if k < 14 { 1994 } else { 2015 })).collect(),
            energy: Some(0.1),
            kind: None,
            songs: Vec::new(),
        };
        assert!(genre(&spread).is_empty());
        let spread_ru: Vec<String> = candidates(&spread, Some(2010)).iter().map(|x| x.title_in(Locale::Ru)).collect();
        let spread_en: Vec<String> = candidates(&spread, Some(2010)).iter().map(|x| x.title_in(Locale::En)).collect();
        assert_eq!(spread_ru, ["Микс 90-х", "Спокойный микс", "Микс: A", "Микс: B"]);
        assert_eq!(spread_en, ["90s Mix", "Calm Mix", "A Mix", "B Mix"]);
        assert_eq!(era_title_in(2000, Locale::Ru), "Микс 2000-х");
        assert_eq!(era_title_in(1980, Locale::Ru), "Микс 80-х");
        assert_eq!(era_title_in(2000, Locale::En), "2000s Mix");
        assert_eq!(era_title_in(1980, Locale::En), "80s Mix");
        // Too few songs dated: no decade.
        let undated = Makeup { years: (0..18).map(|k| (k < 4).then_some(1994)).collect(), ..Makeup::default() };
        assert_eq!(era(&undated, None), None);
    }

    /// No two cards of one name, none of a name the row has already, none
    /// empty or numbered; a mix nothing can name is left out.
    #[test]
    fn names_are_never_shared_empty_or_numbered() {
        use crate::songtype::Lang;
        let rap = tags(&[("hip hop", 5.0)]);
        let mix = |lead: &str, other: &str| Makeup {
            tags: vec![rap.clone(); 16],
            artists: artists(&[(lead, 5), (other, 2)]),
            kind: Some((Kind::Sung(Lang::Russian), None)),
            songs: vec![format!("{lead} song")],
            ..Makeup::default()
        };
        let mixes = [mix("A", "B"), mix("A", "B"), mix("A", "B"), mix("A", "B"), mix("A", "B"), Makeup::default()];
        let names: Vec<Option<String>> = assign_in(&mixes, &["Русские песни".to_owned(), "Любимые".to_owned()], None, Locale::Ru)
            .into_iter()
            .map(|o| o.map(|(_, t)| t))
            .collect();
        assert_eq!(
            names,
            [
                Some("Хип-хоп микс".to_owned()),
                Some("Микс: A".to_owned()),
                // «Русские песни» is another card's already.
                Some("Микс: B".to_owned()),
                Some("Микс: A song".to_owned()),
                None,
                None,
            ]
        );
        let names_en: Vec<Option<String>> = assign_in(&mixes, &["Russian Songs".to_owned(), "Liked Songs".to_owned()], None, Locale::En)
            .into_iter()
            .map(|o| o.map(|(_, t)| t))
            .collect();
        assert_eq!(
            names_en,
            [
                Some("Hip-Hop Mix".to_owned()),
                Some("A Mix".to_owned()),
                Some("B Mix".to_owned()),
                Some("A song Mix".to_owned()),
                None,
                None,
            ]
        );
        let given: Vec<&String> = names.iter().flatten().collect();
        for n in &given {
            assert!(!n.trim().is_empty() && !n.chars().any(|c| c.is_ascii_digit()), "{n}");
        }
        let mut unique = given.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), given.len());
        // A name differs by more than its letters' case.
        let picked = assign_in(&[mix("a", "b")], &["хип-хоп микс".to_owned(), "микс: A".to_owned()], None, Locale::Ru);
        assert_eq!(
            picked,
            [Some((MixName::Kind { kind: Kind::Sung(Lang::Russian), loudness: None }, "Русские песни".to_owned()))]
        );
        let picked_en = assign_in(&[mix("a", "b")], &["hip-hop mix".to_owned(), "a mix".to_owned()], None, Locale::En);
        assert_eq!(
            picked_en,
            [Some((MixName::Kind { kind: Kind::Sung(Lang::Russian), loudness: None }, "Russian Songs".to_owned()))]
        );
        // Empty artist/song names never leak "Микс:" or " Mix".
        let blank = Makeup { artists: artists(&[("   ", 4)]), songs: vec!["".into()], ..Makeup::default() };
        assert!(candidates(&blank, None).is_empty());
    }
}
