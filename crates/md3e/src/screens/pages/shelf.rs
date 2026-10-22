//! The pure side of Библиотека (docs/stage4.md §12.6): the instant filter,
//! the group headers of the album grid, what the «Всё» overview shows and
//! how an artist without a photo looks.

use nikgob_shapes::MaterialShape;
use nikgob_view::{ArtistCard, PlaylistCard, ReleaseCard, ReleaseSort};

/// The query, folded: lowercase, «ё» as «е», split into words.
pub fn words(query: &str) -> Vec<String> {
    fold(query).split_whitespace().map(str::to_owned).collect()
}

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

/// Every word of the query is in one of `fields` (any order, any part of a
/// word: «кин гру» finds «Кино — Группа крови»). No words match everything.
pub fn matches(words: &[String], fields: &[&str]) -> bool {
    if words.is_empty() {
        return true;
    }
    let hay: Vec<String> = fields.iter().map(|f| fold(f)).collect();
    words.iter().all(|w| hay.iter().any(|h| h.contains(w.as_str())))
}

pub fn release_matches(words: &[String], r: &ReleaseCard) -> bool {
    matches(words, &[&r.title, &r.artist])
}

pub fn playlist_matches(words: &[String], p: &PlaylistCard) -> bool {
    matches(words, &[&p.name])
}

pub fn artist_matches(words: &[String], a: &ArtistCard) -> bool {
    matches(words, &[&a.name])
}

/// A run of the album grid under one header: `label` over `items[range]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub label: String,
    pub range: std::ops::Range<usize>,
}

const DAY_MS: i64 = 24 * 3600 * 1000;

/// The header an album falls under for a sort: the first letter of the
/// artist or the title (digits and signs under «#»), the decade, or how
/// long ago it was added (`now` in Unix ms).
pub fn group_key(sort: ReleaseSort, r: &ReleaseCard, now: i64) -> String {
    match sort {
        ReleaseSort::Artist => letter(&r.artist),
        ReleaseSort::Title => letter(&r.title),
        ReleaseSort::Year => match r.year {
            Some(y) if y > 0 => format!("{}-е", y - y.rem_euclid(10)),
            _ => "Без года".to_owned(),
        },
        ReleaseSort::Added => {
            let days = (now - r.added_at).max(0) / DAY_MS;
            match days {
                0 => "Сегодня",
                1..7 => "За неделю",
                7..31 => "За месяц",
                31..365 => "За год",
                _ => "Раньше",
            }
            .to_owned()
        }
    }
}

fn letter(s: &str) -> String {
    match s.chars().find(|c| c.is_alphanumeric()) {
        Some(c) if c.is_alphabetic() => c.to_uppercase().collect(),
        _ => "#".to_owned(),
    }
}

/// Splits `keys` (one per album, in the grid's order) into headed runs.
/// Letters are many and mostly small: neighbouring letters join until a
/// run fills `min` cards (one row), and the header then names the span
/// («A – D»). Decades and dates stay apart whatever their size.
pub fn groups(sort: ReleaseSort, keys: &[String], min: usize) -> Vec<Group> {
    let mut runs: Vec<Group> = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        match runs.last_mut() {
            Some(g) if g.label == *k => g.range.end = i + 1,
            _ => runs.push(Group { label: k.clone(), range: i..i + 1 }),
        }
    }
    if !matches!(sort, ReleaseSort::Artist | ReleaseSort::Title) {
        return runs;
    }
    let min = min.max(1);
    // (first letter, last letter, range)
    let mut out: Vec<(String, String, std::ops::Range<usize>)> = Vec::new();
    for run in runs {
        match out.last_mut() {
            Some(g) if g.2.len() < min => {
                g.1 = run.label;
                g.2.end = run.range.end;
            }
            _ => out.push((run.label.clone(), run.label, run.range)),
        }
    }
    // A short tail joins the run before it.
    if out.len() > 1 && out.last().is_some_and(|g| g.2.len() < min) {
        if let Some((_, last, range)) = out.pop()
            && let Some(g) = out.last_mut()
        {
            g.1 = last;
            g.2.end = range.end;
        }
    }
    out.into_iter()
        .map(|(first, last, range)| Group {
            label: if first == last { first } else { format!("{first} – {last}") },
            range,
        })
        .collect()
}

/// The sections of the «Всё» overview, in order; empty ones are left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overview {
    Playlists,
    RecentlyAdded,
    OftenPlayed,
    Albums,
    Artists,
}

/// How many of each there are.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub recent: usize,
    pub often: usize,
    pub albums: usize,
    pub artists: usize,
}

pub fn overview(c: Counts) -> Vec<Overview> {
    // Playlists always: their row starts with «Новый плейлист». The two
    // album rows only say something when the grid below them would not
    // show the same few albums again.
    let rows_worth = c.albums > 8;
    [
        (Overview::Playlists, true),
        (Overview::RecentlyAdded, rows_worth && c.recent > 0),
        (Overview::OftenPlayed, c.often > 0),
        (Overview::Albums, c.albums > 0),
        (Overview::Artists, c.artists > 0),
    ]
    .into_iter()
    .filter_map(|(s, on)| on.then_some(s))
    .collect()
}

/// How many albums the overview's grid shows: two rows, or all when only
/// a few more would be left behind «Все альбомы».
pub fn overview_albums(total: usize, cols: usize) -> usize {
    let first = cols.max(1) * 2;
    if total <= first + cols.max(1) / 2 { total } else { first }
}

/// The scheme roles an artist's initials sit in: `(container, on)` pairs
/// picked by [`artist_look`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Primary,
    Secondary,
    Tertiary,
    Surface,
}

/// Shapes the initials take (the circle is the photo's).
const SHAPES: [MaterialShape; 8] = [
    MaterialShape::Cookie9Sided,
    MaterialShape::Cookie12Sided,
    MaterialShape::Clover8Leaf,
    MaterialShape::SoftBurst,
    MaterialShape::Flower,
    MaterialShape::Cookie6Sided,
    MaterialShape::Puffy,
    MaterialShape::Clover4Leaf,
];
const ROLES: [Role; 4] = [Role::Primary, Role::Secondary, Role::Tertiary, Role::Surface];

/// How an artist without a photo looks: a role and a shape from a hash of
/// the name, the same on every run and machine (FNV-1a, not the std hasher).
pub fn artist_look(name: &str) -> (Role, MaterialShape) {
    let h = fold(name.trim())
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let role = ROLES[(h % ROLES.len() as u64) as usize];
    let shape = SHAPES[((h >> 16) % SHAPES.len() as u64) as usize];
    (role, shape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::{ArtistLinks, ReleaseId, ReleaseType};

    fn card(artist: &str, title: &str, year: Option<i32>, added_at: i64) -> ReleaseCard {
        ReleaseCard {
            id: ReleaseId::new(),
            title: title.into(),
            artist: artist.into(),
            artists: ArtistLinks::default(),
            year,
            release_type: ReleaseType::Album,
            cover: None,
            local_tracks: 1,
            total_tracks: Some(1),
            liked: false,
            added_at,
        }
    }

    #[test]
    fn the_filter_takes_words_in_any_order_and_folds_yo() {
        let w = words("  гру КИН ");
        assert!(matches(&w, &["Группа крови", "Кино"]));
        assert!(!matches(&w, &["Группа крови", "Земфира"]));
        assert!(matches(&words("ежик"), &["Ёжик в тумане"]));
        assert!(matches(&words(""), &["что угодно"]));
    }

    #[test]
    fn letters_join_until_a_row_is_full() {
        let keys: Vec<String> = ["A", "A", "B", "C", "D", "D", "D", "К", "М"].map(String::from).to_vec();
        // A+B fill a row of 3; C+D go on; the short tail К+М joins them.
        assert_eq!(
            groups(ReleaseSort::Artist, &keys, 3),
            vec![Group { label: "A – B".into(), range: 0..3 }, Group { label: "C – М".into(), range: 3..9 }]
        );
    }

    #[test]
    fn a_full_letter_keeps_its_own_header() {
        let keys: Vec<String> = ["A", "A", "B", "B"].map(String::from).to_vec();
        let g = groups(ReleaseSort::Title, &keys, 2);
        assert_eq!(g, vec![Group { label: "A".into(), range: 0..2 }, Group { label: "B".into(), range: 2..4 }]);
        assert_eq!(groups(ReleaseSort::Title, &[], 2), vec![]);
        let one = groups(ReleaseSort::Title, &["Z".to_owned()], 4);
        assert_eq!(one, vec![Group { label: "Z".into(), range: 0..1 }]);
    }

    #[test]
    fn decades_and_dates_never_merge() {
        let now = 400 * DAY_MS;
        let rs = [
            card("a", "x", Some(1997), now),
            card("a", "y", Some(1991), now - 3 * DAY_MS),
            card("a", "z", Some(2007), now - 40 * DAY_MS),
            card("a", "w", None, now - 399 * DAY_MS),
        ];
        let years: Vec<String> = rs.iter().map(|r| group_key(ReleaseSort::Year, r, now)).collect();
        assert_eq!(years, ["1990-е", "1990-е", "2000-е", "Без года"]);
        assert_eq!(groups(ReleaseSort::Year, &years, 8).len(), 3);
        let added: Vec<String> = rs.iter().map(|r| group_key(ReleaseSort::Added, r, now)).collect();
        assert_eq!(added, ["Сегодня", "За неделю", "За год", "Раньше"]);
        assert_eq!(group_key(ReleaseSort::Artist, &card("9 Grams", "x", None, 0), now), "#");
        assert_eq!(group_key(ReleaseSort::Artist, &card("the xx", "x", None, 0), now), "T");
    }

    #[test]
    fn the_overview_leaves_out_what_it_has_none_of() {
        let all = Counts { recent: 12, often: 6, albums: 40, artists: 20 };
        assert_eq!(overview(all).len(), 5);
        let small = Counts { recent: 5, often: 0, albums: 5, artists: 3 };
        assert_eq!(overview(small), vec![Overview::Playlists, Overview::Albums, Overview::Artists]);
        assert_eq!(overview(Counts::default()), vec![Overview::Playlists]);
        assert_eq!(overview_albums(40, 6), 12);
        assert_eq!(overview_albums(14, 6), 14);
        assert_eq!(overview_albums(5, 6), 5);
    }

    #[test]
    fn artists_look_different_and_keep_their_look() {
        let names = ["Кино", "Radiohead", "Daft Punk", "Земфира", "Aphex Twin", "Molchat Doma", "Massive Attack", "RADWIMPS", "Boards of Canada", "Мумий Тролль", "The Prodigy"];
        let looks: Vec<_> = names.iter().map(|n| artist_look(n)).collect();
        let roles: std::collections::HashSet<_> = looks.iter().map(|l| l.0).collect();
        let shapes: std::collections::HashSet<_> = looks.iter().map(|l| l.1 as usize).collect();
        assert!(roles.len() >= 3, "{looks:?}");
        assert!(shapes.len() >= 4, "{looks:?}");
        assert_eq!(artist_look("Кино"), artist_look(" кино "));
        assert!(looks.iter().all(|l| l.1 != MaterialShape::Circle));
    }
}
