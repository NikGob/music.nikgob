//! How well a search hit matches the query: one local score for the rows of
//! every source, so the merged «Всё» answer reads as one list (docs/stage3.md
//! §3.2) instead of each service's own order.
//!
//! * Text is compared normalised: lower case, `ё` → `е`, no diacritics, no
//!   punctuation, single spaces — without transliteration, so the script of
//!   a hit can be told apart.
//! * Signals, strongest first: the exact title (or name), the title starting
//!   with the query, every query word in the title, the words split between
//!   the artist and the title («morgenshtern aristocrat», «кино группа
//!   крови»), the query as a prefix of the words (still typing), then the
//!   artist and the album alone.
//! * A Cyrillic query that only meets a Latin hit (YouTube Music
//!   transliterates «лед» and finds Led Zeppelin) and the reverse count for
//!   much less; clips and uploads less than official songs.
//! * Ties keep the source's own order (Deezer ranks by popularity, YouTube
//!   by its relevance), then the source (library, Spotify, Deezer, YouTube).
//! * The top result is an artist whose name is the query; else the best of
//!   the best track and the best album (a track on a tie), if it matches at
//!   all well.

use nikgob_core::streaming::TrackKind;

/// The query, normalised once.
#[derive(Clone, Debug)]
pub(crate) struct Query {
    norm: String,
    words: Vec<String>,
    cyrillic: bool,
    latin: bool,
}

/// Scores at or above this can be the top result.
pub(crate) const TOP_MIN: f64 = 0.5;

fn fold_char(c: char, out: &mut String) {
    let s = match c {
        'ё' => "е",
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'ŕ' | 'ř' => "r",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'ť' | 'ţ' | 'ț' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'ł' => "l",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        // Combining marks of decomposed text.
        '\u{0300}'..='\u{036F}' => "",
        // Apostrophes join a word («don't» → «dont»).
        '\'' | '’' | 'ʼ' | '`' | '´' => "",
        c if c.is_alphanumeric() => {
            out.push(c);
            return;
        }
        _ => " ",
    };
    out.push_str(s);
}

/// Lower case, `ё` → `е`, no diacritics or punctuation, single spaces.
pub(crate) fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        fold_char(c, &mut out);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

fn scripts(s: &str) -> (bool, bool) {
    (s.chars().any(is_cyrillic), s.chars().any(|c| c.is_ascii_alphabetic()))
}

/// The title without what follows its first bracket, dash or «feat.»:
/// «Лёд (feat. X)» → «Лёд».
fn base_title(t: &str) -> &str {
    let mut end = t.len();
    for sep in [" (", " [", " - ", " – ", " — ", " feat", " ft.", " Feat", " FEAT"] {
        if let Some(i) = t.find(sep)
            && i > 0
        {
            end = end.min(i);
        }
    }
    &t[..end]
}

impl Query {
    pub(crate) fn new(q: &str) -> Self {
        let norm = norm(q);
        let (cyrillic, latin) = scripts(&norm);
        Self {
            words: norm.split(' ').filter(|w| !w.is_empty()).map(str::to_owned).collect(),
            norm,
            cyrillic,
            latin,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// How well one text field (already normalised) matches, 0..=1.
    fn field(&self, text: &str) -> f64 {
        if text.is_empty() || self.is_empty() {
            return 0.0;
        }
        if text == self.norm {
            return 1.0;
        }
        if text.len() > self.norm.len() && text.starts_with(&self.norm) && text[self.norm.len()..].starts_with(' ') {
            return 0.8;
        }
        let words: Vec<&str> = text.split(' ').collect();
        let have = self.words.iter().filter(|w| words.contains(&w.as_str())).count();
        if have == self.words.len() {
            // Every word: the fewer extra words, the better.
            return 0.6 + 0.15 * self.words.len() as f64 / words.len().max(1) as f64;
        }
        if self.words.iter().all(|w| words.iter().any(|t| t.starts_with(w.as_str()))) {
            return 0.5;
        }
        0.3 * have as f64 / self.words.len() as f64
    }

    /// The query's words split between two fields (artist + title), each
    /// giving at least one: «кино группа крови».
    fn split(&self, a: &str, b: &str) -> f64 {
        if self.words.len() < 2 || a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let wa: Vec<&str> = a.split(' ').collect();
        let wb: Vec<&str> = b.split(' ').collect();
        let in_a = |w: &String| wa.contains(&w.as_str());
        let in_b = |w: &String| wb.contains(&w.as_str());
        let all = self.words.iter().all(|w| in_a(w) || in_b(w));
        let both = self.words.iter().any(in_a) && self.words.iter().any(in_b);
        if !(all && both) {
            return 0.0;
        }
        // The fields hold nothing but the query: as good as exact.
        let extra = (wa.len() + wb.len()).saturating_sub(self.words.len());
        if extra == 0 { 0.95 } else { 0.85 }
    }

    /// The hit is in the other script only (a transliteration's false
    /// friend): «лед» → «Led Zeppelin», «kino» → «Кино».
    fn script_penalty(&self, text: &str) -> f64 {
        let (cyr, lat) = scripts(text);
        if self.cyrillic && !self.latin && !cyr && lat {
            0.35
        } else if self.latin && !self.cyrillic && cyr && !lat {
            0.35
        } else {
            1.0
        }
    }

    /// A track: its title, its artists, its album.
    pub(crate) fn track(&self, title: &str, artists: &[&str], album: Option<&str>, kind: TrackKind) -> f64 {
        let full = norm(title);
        let base = norm(base_title(title));
        let title_s = self.field(&full).max(self.field(&base));
        let artist_norms: Vec<String> = artists.iter().map(|a| norm(a)).collect();
        let artist_s = artist_norms.iter().map(|a| self.field(a)).fold(0.0, f64::max);
        let split_s = artist_norms
            .iter()
            .map(|a| self.split(a, &base).max(self.split(a, &full)))
            .fold(0.0, f64::max);
        let album_s = album.map_or(0.0, |a| self.field(&norm(a)));
        let mut s = title_s.max(split_s).max(0.55 * artist_s).max(0.45 * album_s);
        s *= self.script_penalty(&format!("{full} {}", artist_norms.join(" ")));
        s *= match kind {
            TrackKind::Song => 1.0,
            TrackKind::Video => 0.85,
            TrackKind::Upload => 0.7,
        };
        s
    }

    /// An album: its title, its artist.
    pub(crate) fn album(&self, title: &str, artist: &str) -> f64 {
        let t = norm(title);
        let base = norm(base_title(title));
        let a = norm(artist);
        let s = self
            .field(&t)
            .max(self.field(&base))
            .max(self.split(&a, &base))
            .max(0.5 * self.field(&a));
        s * self.script_penalty(&format!("{t} {a}"))
    }

    /// An artist: its name.
    pub(crate) fn artist(&self, name: &str) -> f64 {
        let n = norm(name);
        self.field(&n) * self.script_penalty(&n)
    }

    /// A playlist: its name.
    pub(crate) fn playlist(&self, name: &str) -> f64 {
        let n = norm(name);
        self.field(&n) * self.script_penalty(&n)
    }
}

/// Which kind of hit is the top result, and its index in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Top {
    Artist(usize),
    Track(usize),
    Album(usize),
}

/// The top result from the scores of each list (in the lists' order): an
/// artist whose name is the query, else the best track or album (a track on
/// a tie) when it matches well enough, else an artist that matches better
/// than both.
pub(crate) fn top(artists: &[f64], tracks: &[f64], albums: &[f64]) -> Option<Top> {
    let best = |v: &[f64]| {
        v.iter()
            .copied()
            .enumerate()
            .fold(None::<(usize, f64)>, |b, (i, s)| match b {
                Some((_, bs)) if bs >= s => b,
                _ => Some((i, s)),
            })
    };
    let (a, t, al) = (best(artists), best(tracks), best(albums));
    if let Some((i, s)) = a
        && s >= 0.999
    {
        return Some(Top::Artist(i));
    }
    let ts = t.map_or(0.0, |(_, s)| s);
    let als = al.map_or(0.0, |(_, s)| s);
    let as_ = a.map_or(0.0, |(_, s)| s);
    if as_ > ts && as_ > als && as_ >= 0.8 {
        return a.map(|(i, _)| Top::Artist(i));
    }
    if ts >= als && ts >= TOP_MIN {
        return t.map(|(i, _)| Top::Track(i));
    }
    if als >= TOP_MIN {
        return al.map(|(i, _)| Top::Album(i));
    }
    None
}

/// Sorts `items` by score (best first); ties keep their order (the source's
/// own ranking, then the source). Scores are compared at 0.01, as the
/// merged answer compares them.
#[cfg(test)]
pub(crate) fn order(scores: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    let q = |s: f64| (s * 100.0).round() as i64;
    idx.sort_by_key(|&i| std::cmp::Reverse(q(scores[i])));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Hit {
        title: &'static str,
        artist: &'static str,
        kind: TrackKind,
    }

    fn song(title: &'static str, artist: &'static str) -> Hit {
        Hit { title, artist, kind: TrackKind::Song }
    }

    /// The titles in the order the ranking gives (hits listed in the
    /// sources' own order).
    fn ranked(q: &str, hits: &[Hit]) -> Vec<&'static str> {
        let q = Query::new(q);
        let scores: Vec<f64> = hits.iter().map(|h| q.track(h.title, &[h.artist], None, h.kind)).collect();
        order(&scores).into_iter().map(|i| hits[i].title).collect()
    }

    #[test]
    fn normalising_folds_case_yo_diacritics_and_punctuation() {
        assert_eq!(norm("  Лёд!  "), "лед");
        assert_eq!(norm("Björk — Jóga"), "bjork joga");
        assert_eq!(norm("Don't  Stop (Remix)"), "dont stop remix");
        assert_eq!(norm("MORGENSHTERN"), "morgenshtern");
        assert_eq!(norm("Cafe\u{0301}"), "cafe");
    }

    #[test]
    fn led_puts_the_russian_songs_above_whole_lotta_love() {
        // YouTube Music transliterates «лед»: Led Zeppelin comes first
        // there, the wanted songs are lower and in Deezer.
        let hits = [
            song("Whole Lotta Love", "Led Zeppelin"),
            song("Stairway to Heaven", "Led Zeppelin"),
            song("Лёд", "Монеточка"),
            song("Лед", "Stigmata"),
            song("Лёд (feat. Тима Белорусских)", "NILETTO"),
        ];
        let got = ranked("лед", &hits);
        let pos = |t: &str| got.iter().position(|x| *x == t).unwrap();
        assert!(pos("Лёд") < pos("Whole Lotta Love"), "{got:?}");
        assert!(pos("Лед") < pos("Whole Lotta Love"), "{got:?}");
        assert!(pos("Лёд (feat. Тима Белорусских)") < pos("Whole Lotta Love"), "{got:?}");
        assert_eq!(&got[3..], ["Whole Lotta Love", "Stairway to Heaven"], "the false friends last, in order");
        // «лёд» reads the same.
        assert_eq!(ranked("лёд", &hits)[0], "Лёд");
        // A Latin-only artist matching a Cyrillic query counts for little.
        let q = Query::new("лед");
        assert!(q.track("Led", &["Led Zeppelin"], None, TrackKind::Song) < TOP_MIN);
    }

    #[test]
    fn kino_makes_the_band_the_top_result() {
        let q = Query::new("кино");
        let artists = [q.artist("Kino"), q.artist("Кино"), q.artist("Кино и немцы")];
        let tracks = [
            q.track("Группа крови", &["Кино"], Some("Группа крови"), TrackKind::Song),
            q.track("Кино", &["Ирина Аллегрова"], None, TrackKind::Song),
        ];
        let albums = [q.album("Кино", "Сплин")];
        assert_eq!(top(&artists, &tracks, &albums), Some(Top::Artist(1)), "{artists:?}");
        // Without the artist: the exact title wins over the band's song.
        assert_eq!(top(&[], &tracks, &[]), Some(Top::Track(1)));
    }

    #[test]
    fn morgenshtern_aristocrat_puts_aristocrat_by_morgenshtern_on_top() {
        let q = Query::new("morgenshtern aristocrat");
        let hits = [
            ("Cadillac", "MORGENSHTERN", TrackKind::Song),
            ("Aristocrat (Remix)", "DJ Someone", TrackKind::Song),
            ("MORGENSHTERN - ARISTOCRAT (Клип)", "MORGENSHTERN", TrackKind::Video),
            ("ARISTOCRAT", "MORGENSHTERN", TrackKind::Song),
        ];
        let tracks: Vec<f64> = hits.iter().map(|(t, a, k)| q.track(t, &[a], None, *k)).collect();
        let artists = [q.artist("MORGENSHTERN")];
        let albums = [q.album("LAST ONE", "MORGENSHTERN")];
        assert_eq!(top(&artists, &tracks, &albums), Some(Top::Track(3)), "{tracks:?}");
        let order = order(&tracks);
        assert_eq!(order[0], 3, "the song before its clip: {tracks:?}");
        assert_eq!(order[1], 2, "then the clip");
        // «кино группа крови»: the band and the title.
        let q = Query::new("кино группа крови");
        let s = q.track("Группа крови", &["Кино"], Some("Группа крови"), TrackKind::Song);
        let other = q.track("Группа крови", &["Ария"], None, TrackKind::Song);
        assert!(s > other && s >= 0.9, "{s} {other}");
    }

    #[test]
    fn an_exact_title_beats_a_partial_one() {
        let hits = [
            song("Звезда по имени Солнце", "Кино"),
            song("Звёздочка", "Someone"),
            song("Звезда", "Кино"),
        ];
        assert_eq!(ranked("звезда", &hits), ["Звезда", "Звезда по имени Солнце", "Звёздочка"]);
        let q = Query::new("daft punk");
        let artists = [q.artist("Daft Punk Tribute Band"), q.artist("Daft Punk")];
        assert_eq!(top(&artists, &[], &[]), Some(Top::Artist(1)));
        // Ties keep the sources' order.
        assert_eq!(ranked("x", &[song("A", "B"), song("C", "D")]), ["A", "C"]);
    }

    #[test]
    fn weak_matches_make_no_top_result() {
        let q = Query::new("совсем другое");
        let tracks = [q.track("Something", &["Else"], None, TrackKind::Song)];
        assert_eq!(top(&[], &tracks, &[]), None);
        assert!(Query::new("  ").is_empty());
    }
}
