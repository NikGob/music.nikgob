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

/// The families of genres with their numbers and their mixes' names, the
/// more special first: a tag is of the first family one of whose keys it
/// has. A key is a run of whole words («hip hop» in «southern hip hop»);
/// one starting with `*` is found anywhere in a word («*rock» in
/// «krautrock»). A number is the family's for good (the caches and the
/// mixes left out keep it): a new family takes a new one wherever it
/// stands, none is used again.
const FAMILIES: &[(u8, &str, &[&str])] = &[
    (1, "J-pop микс", &["j pop", "jpop", "japanese pop", "city pop", "kayokyoku", "anison", "vocaloid", "utaite"]),
    (2, "K-pop микс", &["k pop", "kpop", "korean pop"]),
    (3, "J-rock микс", &["j rock", "jrock", "japanese rock", "visual kei"]),
    (4, "Пост-панк микс", &["post punk", "new wave", "coldwave", "darkwave", "gothic rock"]),
    (5, "Латинский микс", &["latin", "latino", "latina", "*reggaeton", "salsa", "bachata", "cumbia", "bossa nova"]),
    (6, "Регги-микс", &["*reggae", "dancehall", "ska", "dub"]),
    (7, "Хип-хоп микс", &["hip hop", "hiphop", "*rap", "drill", "grime", "boom bap", "phonk"]),
    (8, "Метал-микс", &["*metal", "deathcore", "djent", "grindcore"]),
    (9, "Панк-микс", &["punk", "emo"]),
    (10, "Инди-микс", &["*indie"]),
    (11, "Рок-микс", &["*rock", "grunge", "shoegaze"]),
    (12, "Поп-микс", &["*pop"]),
    (13, "Микс саундтреков", &["soundtrack", "soundtracks", "score", "film score", "video game", "video game music", "vgm", "game music", "anime", "ost"]),
    (14, "Лоу-фай микс", &["lo fi", "lofi", "chillhop"]),
    (15, "Эмбиент-микс", &["ambient", "drone", "new age"]),
    (
        16,
        "Электронный микс",
        &["*electro", "house", "*techno", "trance", "edm", "dance", "drum and bass", "dnb", "dubstep", "garage", "breakbeat", "idm", "*synth", "future bass", "footwork", "hardstyle"],
    ),
    (17, "R&B микс", &["r&b", "rnb", "*soul", "*funk", "motown"]),
    (18, "Джазовый микс", &["*jazz", "swing", "bebop"]),
    (19, "Блюзовый микс", &["*blues"]),
    (20, "Классический микс", &["classical", "baroque", "opera", "orchestral", "symphony", "chamber music"]),
    (21, "Кантри-микс", &["country", "bluegrass", "americana"]),
    (22, "Фолк-микс", &["*folk", "celtic"]),
    (23, "Диско-микс", &["*disco"]),
];

/// The family of the tag `tag` (its mix's name), if it is of one: «russian
/// rap» is hip hop, «2010s» and «vtuber» are no genre.
pub fn family(tag: &str) -> Option<&'static str> {
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
    FAMILIES.iter().find(|(_, _, keys)| keys.iter().any(|k| has(k))).map(|(_, title, _)| *title)
}

/// The name of the mix of the family of the number `id`.
pub fn family_title(id: u8) -> Option<&'static str> {
    FAMILIES.iter().find(|f| f.0 == id).map(|f| f.1)
}

/// The number of the family whose mix is called `title`.
pub fn family_id(title: &str) -> Option<u8> {
    FAMILIES.iter().find(|f| f.1 == title).map(|f| f.0)
}

/// The family an artist of `tags` is of (its number): the one that leads
/// them with [`LEAD_SHARE`]; `None` for an artist nobody tagged with a
/// genre, or of several genres alike.
pub fn leading(tags: &Tags) -> Option<u8> {
    let of = families(tags);
    let (title, share) = of.first()?;
    if *share < LEAD_SHARE {
        return None;
    }
    family_id(title)
}

/// The families of an artist's `tags` with their shares of the artist
/// (they sum to 1), the largest first; empty when no tag is of a family.
/// A tag counts by its votes and one more (a tag nobody voted for still
/// says something).
pub fn families(tags: &Tags) -> Vec<(&'static str, f32)> {
    let mut out: Vec<(&'static str, f32)> = Vec::new();
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
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
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
    /// The name of its taste profile's kind («Русские песни»).
    pub kind: Option<String>,
    /// Its first songs' titles (the last resort).
    pub songs: Vec<String>,
}

impl Makeup {
    fn len(&self) -> usize {
        self.tags.len().max(self.years.len()).max(self.artists.iter().map(|a| a.1).sum::<usize>()).max(1)
    }
}

/// The genres that name `m`, the first its own and the second one that
/// may stand in ([`GENRE_SHARE`], [`GENRE_SECOND`]).
pub fn genre(m: &Makeup) -> Vec<&'static str> {
    let n = m.tags.len();
    if n == 0 {
        return Vec::new();
    }
    let mut tagged = 0usize;
    let mut sum: Vec<(&'static str, f32)> = Vec::new();
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
    sum.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
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

/// «Микс 90-х», «Микс 2010-х».
pub fn era_title(decade: i32) -> String {
    if (1900..2000).contains(&decade) { format!("Микс {}-х", decade % 100) } else { format!("Микс {decade}-х") }
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

fn artist_title(name: &str) -> String {
    format!("Микс: {name}")
}

/// The names `m` may have, the best first (see the module docs).
pub fn candidates(m: &Makeup, common_decade: Option<i32>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let genres = genre(m);
    if let Some(g) = genres.first() {
        out.push((*g).to_owned());
    }
    let n = m.len();
    if let Some((name, songs)) = m.artists.first()
        && *songs >= ARTIST_SONGS
        && *songs as f32 >= ARTIST_SHARE * n as f32
    {
        out.push(artist_title(name));
    }
    if let Some(d) = era(m, common_decade) {
        out.push(era_title(d));
    }
    match m.energy {
        Some(e) if e <= CALM_BELOW => out.push("Спокойный микс".to_owned()),
        Some(e) if e >= LIVELY_ABOVE => out.push("Бодрый микс".to_owned()),
        _ => {}
    }
    if let Some(k) = m.kind.as_deref().filter(|k| !k.trim().is_empty()) {
        out.push(k.to_owned());
    }
    if let Some(g) = genres.get(1) {
        out.push((*g).to_owned());
    }
    out.extend(m.artists.iter().map(|a| artist_title(&a.0)));
    out.extend(m.songs.iter().map(|s| format!("Микс: {s}")));
    let mut seen: Vec<String> = Vec::new();
    out.retain(|t| {
        let key = t.trim().to_lowercase();
        let fresh = !key.is_empty() && key != "микс:" && !seen.contains(&key);
        if fresh {
            seen.push(key);
        }
        fresh
    });
    out
}

/// A name for each of `mixes` in their order: the first of its candidates
/// no card before it and none of `taken` (the other cards of the row) has.
/// `None`: nothing of the mix can name it (it has no artist and no song
/// with a name), so it is not shown.
pub fn assign(mixes: &[Makeup], taken: &[String], common_decade: Option<i32>) -> Vec<Option<String>> {
    let mut used: Vec<String> = taken.iter().map(|t| t.trim().to_lowercase()).collect();
    mixes
        .iter()
        .map(|m| {
            let name = candidates(m, common_decade).into_iter().find(|c| !used.contains(&c.trim().to_lowercase()))?;
            used.push(name.trim().to_lowercase());
            Some(name)
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
            assert_eq!(family(tag), want, "{tag}");
        }
        let mut ids: Vec<u8> = Vec::new();
        for (id, title, keys) in FAMILIES {
            assert!(title.contains("икс") && !keys.is_empty(), "{title}");
            assert!(!ids.contains(id), "{title}: the number {id} is another family's");
            ids.push(*id);
            assert_eq!(family_title(*id), Some(*title));
            assert_eq!(family_id(title), Some(*id));
        }
        assert_eq!(family_title(0), None);
    }

    #[test]
    fn an_artists_families_share_them() {
        let f = families(&tags(&[("hip hop", 9.0), ("pop rap", 3.0), ("pop", 3.0), ("2010s", 5.0), ("relic inn", 0.0)]));
        assert_eq!(f[0].0, "Хип-хоп микс");
        assert!((f[0].1 - 14.0 / 18.0).abs() < 1e-4, "{f:?}");
        assert_eq!(f[1].0, "Поп-микс");
        assert!((f.iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(families(&tags(&[("2010s", 5.0)])).is_empty());
        // The artist is a rapper; one of two genres alike is of neither,
        // one nobody tagged with a genre of none.
        assert_eq!(leading(&tags(&[("hip hop", 9.0), ("pop rap", 3.0), ("pop", 3.0)])).and_then(family_title), Some("Хип-хоп микс"));
        assert_eq!(leading(&tags(&[("rock", 2.0), ("jazz", 2.0), ("folk", 2.0)])), None);
        assert_eq!(leading(&tags(&[("2010s", 5.0)])), None);
        assert_eq!(leading(&[]), None);
    }

    /// A mix of mostly rappers is «Хип-хоп микс»; one whose artists nobody
    /// tagged is named by its leading artist; one without a leader by its
    /// decade, its mood or its kind.
    #[test]
    fn a_mix_is_named_by_what_it_is_made_of() {
        let rap = tags(&[("hip hop", 5.0), ("trap", 1.0)]);
        let pop = tags(&[("pop", 4.0)]);
        let hip_hop = Makeup {
            tags: (0..20).map(|k| if k < 12 { rap.clone() } else if k < 15 { pop.clone() } else { Vec::new() }).collect(),
            artists: artists(&[("MORGENSHTERN", 5), ("SLAVA MARLOW", 4)]),
            years: vec![Some(2021); 20],
            energy: Some(0.8),
            kind: Some("Русские песни".into()),
            songs: vec!["Cadillac".into()],
        };
        assert_eq!(genre(&hip_hop), ["Хип-хоп микс"]);
        let c = candidates(&hip_hop, Some(2020));
        assert_eq!(c[..5], ["Хип-хоп микс", "Микс: MORGENSHTERN", "Бодрый микс", "Русские песни", "Микс: SLAVA MARLOW"], "{c:?}");
        assert!(c.contains(&"Микс: Cadillac".to_owned()));
        // The library is not all of the 2020s: the decade is a name too.
        assert!(candidates(&hip_hop, Some(2010)).contains(&"Микс 2020-х".to_owned()));

        // Too few songs with a genre: no genre, the leading artist.
        let untagged = Makeup {
            tags: (0..20).map(|k| if k < 6 { rap.clone() } else { Vec::new() }).collect(),
            artists: artists(&[("ZUTOMAYO", 6), ("Eve", 2)]),
            ..Makeup::default()
        };
        assert!(genre(&untagged).is_empty());
        assert_eq!(candidates(&untagged, None)[0], "Микс: ZUTOMAYO");

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
        assert_eq!(candidates(&spread, Some(2010)), ["Микс 90-х", "Спокойный микс", "Микс: A", "Микс: B"]);
        assert_eq!(era_title(2000), "Микс 2000-х");
        assert_eq!(era_title(1980), "Микс 80-х");
        // Too few songs dated: no decade.
        let undated = Makeup { years: (0..18).map(|k| (k < 4).then_some(1994)).collect(), ..Makeup::default() };
        assert_eq!(era(&undated, None), None);
    }

    /// No two cards of one name, none of a name the row has already, none
    /// empty or numbered; a mix nothing can name is left out.
    #[test]
    fn names_are_never_shared_empty_or_numbered() {
        let rap = tags(&[("hip hop", 5.0)]);
        let mix = |lead: &str, other: &str| Makeup {
            tags: vec![rap.clone(); 16],
            artists: artists(&[(lead, 5), (other, 2)]),
            kind: Some("Русские песни".into()),
            songs: vec![format!("{lead} song")],
            ..Makeup::default()
        };
        let mixes = [mix("A", "B"), mix("A", "B"), mix("A", "B"), mix("A", "B"), mix("A", "B"), Makeup::default()];
        let names = assign(&mixes, &["Русские песни".to_owned(), "Любимые".to_owned()], None);
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
        let given: Vec<&String> = names.iter().flatten().collect();
        for n in &given {
            assert!(!n.trim().is_empty() && !n.chars().any(|c| c.is_ascii_digit()), "{n}");
        }
        let mut unique = given.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), given.len());
        // A name differs by more than its letters' case.
        assert_eq!(assign(&[mix("a", "b")], &["хип-хоп микс".to_owned(), "микс: A".to_owned()], None), [Some("Русские песни".to_owned())]);
    }
}
