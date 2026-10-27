//! Which song a video of an export is: the library's own track of that
//! song when it has one, else the song a service lists, else the video
//! itself (the cold import, [`super::cold`]).
//!
//! An export names a video by what YouTube shows: an upload's own title
//! («Hollow Knight OST - Enter Hallownest» by «Amellifera», «Artist - Title
//! (Official Video)», «01 Hive Knight (Hollow Knight: Gods & Nightmares)»)
//! and its channel. [`Probe::of`] reads from that the titles the song may
//! have (without a track number, the brackets of noise, the halves around a
//! dash: [`song_titles`]) and the names that may credit it (the channel,
//! the other halves and brackets, the album). A track is the song when
//! * its title is one of those titles by [`title_key`], the same version
//!   («Live» is another song) and not another take the video says it is
//!   ([`TAKES`]: a cover, a nightcore, a piano version);
//! * and something else agrees: one of its artists is one of the names
//!   ([`same_artist`]), or its album is the work a half or a bracket names
//!   («Hollow Knight OST» and «Hollow Knight: Original Soundtrack»,
//!   [`work_key`]);
//! * with lengths, where both are known, within [`LENGTH_SLACK_MS`].
//!
//! A title alone is never enough: «Dream» by an uploader is no song of the
//! library. Of several tracks the one with a file wins, then the one both
//! the artist and the album agree with, then the oldest.

use std::collections::{HashMap, HashSet};

use nikgob_core::clip_title::song_titles;
use nikgob_core::model::TrackId;
use nikgob_core::names::same_name;
use nikgob_core::streaming::{StreamTrack, TrackKind};
use nikgob_core::translit::same_translit;
use nikgob_library::MatchRow;
use nikgob_metadata::text::{Variant, artist_key, similarity, title_key};

use super::export::Item;

/// Lengths further apart than this are another recording (an upload may
/// have a short intro or outro).
pub(crate) const LENGTH_SLACK_MS: u64 = 15_000;

/// Words of a video's title that make it another take of the song: a track
/// is that take only when its own title or album says so too.
const TAKES: &[&str] = &[
    "cover", "covered", "кавер", "nightcore", "slowed", "reverb", "sped", "speed", "8d", "karaoke", "караоке",
    "piano", "orchestral", "orchestra", "acoustic", "instrumental", "inst", "remix", "ремикс", "live", "лайв",
    "lofi", "hour", "hours", "час", "reaction", "реакция", "tutorial", "минус", "mashup", "loop", "extended",
    "tiktok", "guitar", "violin", "music box",
];

/// Words a work's name carries besides itself («Hollow Knight OST»,
/// «MADE IN ABYSS ORIGINAL SOUNDTRACK 3»).
const WORK_NOISE: &[&str] = &[
    "ost", "soundtrack", "soundtracks", "original", "game", "anime", "music", "musics", "theme", "themes", "the",
    "from", "tv", "series", "movie", "film", "score", "vol", "volume", "collection", "complete", "official",
    "саундтрек", "оригинальный", "игра", "аниме", "тема", "of", "and", "ost's", "bgm",
];

/// What a video of an export says about its song.
#[derive(Clone, Debug, Default)]
pub(crate) struct Probe {
    /// The keys of the titles it may have, the whole title first.
    titles: Vec<(String, Variant)>,
    /// What else it says (halves, brackets, the channel, the album), as
    /// written: each may be an artist or a work.
    hints: Vec<String>,
    /// Take words of its title ([`TAKES`]).
    takes: Vec<&'static str>,
    length: Option<u64>,
    /// What to ask a service for it.
    pub(crate) query: String,
}

/// `01 Song`, `1. Song`, `03 - Song` → `Song`; a title that is only a
/// number, or a number that is part of the title («7 weeks», «10 секунд»),
/// stays.
fn without_number(title: &str) -> &str {
    let t = title.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return t;
    }
    let rest = &t[digits..];
    let padded = digits >= 2 && t.starts_with('0');
    let cut = [". ", ") ", " - ", " – ", " — ", "."]
        .iter()
        .find_map(|sep| rest.strip_prefix(sep))
        .or_else(|| padded.then(|| rest.strip_prefix(' ')).flatten());
    match cut.map(str::trim_start) {
        Some(r) if r.chars().next().is_some_and(char::is_alphabetic) => r,
        _ => t,
    }
}

/// The words of `s`, lower case.
fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.replace('ё', "е"))
        .collect()
}

fn takes_of(s: &str) -> Vec<&'static str> {
    let ws = words(s);
    let joined = format!(" {} ", ws.join(" "));
    TAKES
        .iter()
        .copied()
        .filter(|t| if t.contains(' ') { joined.contains(&format!(" {t} ")) } else { ws.iter().any(|w| w == t) })
        .collect()
}

/// [`WORK_NOISE`] as [`title_key`] spells it («soundtrak»).
static WORK_NOISE_KEYS: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(|| WORK_NOISE.iter().map(|w| title_key(w).base).collect());

/// A work's name as compared: its title key without [`WORK_NOISE`].
pub(crate) fn work_key(name: &str) -> String {
    title_key(name)
        .base
        .split(' ')
        .filter(|w| !w.is_empty() && !WORK_NOISE_KEYS.contains(*w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One work names the other: the words of the shorter, in order, within
/// the longer, at least five letters of them.
fn same_work(a: &str, b: &str) -> bool {
    let (a, b) = (work_key(a), work_key(b));
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.chars().filter(|c| c.is_alphanumeric()).count() >= 5
        && format!(" {long} ").contains(&format!(" {short} "))
}

/// The keys a track's own title goes by: the whole title, and its halves
/// and brackets («夏子 - Natsuko», «Song (From "Show")»), each with the
/// version of the whole when it names none.
fn own_keys(title: &str) -> Vec<(String, Variant)> {
    let whole = title_key(title);
    let mut out = vec![(whole.base.clone(), whole.variant)];
    for part in song_titles(title, "") {
        let k = title_key(&part);
        let v = if k.variant.is_empty() { whole.variant } else { k.variant };
        if !k.base.is_empty() && !out.iter().any(|(b, _)| *b == k.base) {
            out.push((k.base, v));
        }
    }
    out
}

/// Two names of one artist: the same key, the same name in another script
/// or order ([`same_name`], [`same_translit`]), or keys a letter or two
/// apart.
pub(crate) fn same_artist(a: &str, b: &str) -> bool {
    let (ka, kb) = (artist_key(a), artist_key(b));
    if ka.chars().count() < 2 || kb.chars().count() < 2 {
        return false;
    }
    ka == kb
        || same_name(a, b)
        || same_translit(a, b)
        || (ka.chars().count() >= 6 && similarity(&ka, &kb) >= 0.92)
}

impl Probe {
    /// What `item` says, `length` its length when known; `credited`: its
    /// artist is the song's (a «- Topic» channel, a song of the music
    /// library, a service's song), else it may be anyone's upload.
    pub(crate) fn of(item: &Item, length: Option<u64>, credited: bool) -> Self {
        let raw = item.title.trim();
        let mut p = Self { length, ..Self::default() };
        if raw.is_empty() {
            return p;
        }
        let bare = without_number(raw);
        let mut titles: Vec<String> = vec![raw.to_owned()];
        if bare != raw {
            titles.push(bare.to_owned());
        }
        let parts = song_titles(bare, &item.artist);
        // The hints are the parts alone: a whole title (or the halves
        // joined, which come first) holds the title itself and would agree
        // with any album named like it.
        let joined = parts.len() > 1 && parts[0].len() > parts[1].len() && parts[0].starts_with(parts[1].as_str());
        for t in &parts {
            if !titles.contains(t) {
                titles.push(t.clone());
            }
        }
        let version = title_key(bare).variant;
        for t in &titles {
            let k = title_key(t);
            // A half of «Song (Live) - Artist» keeps the version of the whole.
            let variant = if k.variant.is_empty() { version } else { k.variant };
            if !k.base.is_empty() && !p.titles.iter().any(|(b, _)| *b == k.base) {
                p.titles.push((k.base, variant));
            }
        }
        p.hints = parts.iter().skip(usize::from(joined)).cloned().collect();
        p.hints.extend(std::iter::once(&item.artist).chain(&item.featured).chain(&item.album).filter(|s| !s.trim().is_empty()).cloned());
        p.takes = takes_of(raw);
        let main = song_titles(bare, &item.artist).into_iter().next().unwrap_or_else(|| bare.to_owned());
        p.query = if credited && !item.artist.is_empty() && !nikgob_core::clip_title::fold(&main).contains(&nikgob_core::clip_title::fold(&item.artist)) {
            format!("{} {main}", item.artist)
        } else {
            main
        };
        p
    }

    /// The probe of a service's track (its own title, artists and album;
    /// the uploader of a clip may be anyone, YouTube Music does not tell an
    /// official clip from an upload).
    pub(crate) fn of_track(t: &StreamTrack) -> Self {
        let mut item = Item {
            service: t.id.service,
            id: Some(t.id.id.clone()),
            title: t.title.clone(),
            artist: t.artists.first().map(|a| a.name.clone()).unwrap_or_default(),
            featured: t.artists.iter().skip(1).map(|a| a.name.clone()).collect(),
            album: t.album.as_ref().map(|a| a.title.clone()),
        };
        item.title = item.title.trim().to_owned();
        Self::of(&item, t.duration_ms, t.kind == TrackKind::Song)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.titles.is_empty()
    }

    /// How well a track (`title` by `artists` on `album`, `length` long)
    /// is the song: `None` it is not, else 1 an artist or the album agrees,
    /// 2 both do.
    fn agrees(&self, title: &str, artists: &[String], album: Option<&str>, length: Option<u64>) -> Option<u8> {
        let own = own_keys(title);
        let hit = self.titles.iter().find(|t| own.contains(t))?;
        if let (Some(a), Some(b)) = (self.length, length)
            && a.abs_diff(b) > LENGTH_SLACK_MS
        {
            return None;
        }
        let said = format!("{} {}", title, album.unwrap_or_default());
        let own_takes = takes_of(&said);
        if self.takes.iter().any(|t| !own_takes.contains(t)) {
            return None;
        }
        // The hints besides the title that matched.
        let others = || self.hints.iter().filter(|h| title_key(h).base != hit.0);
        let artist = others().any(|h| artists.iter().any(|a| same_artist(a, h)));
        let work = album.is_some_and(|al| others().any(|h| same_work(h, al)));
        match (artist, work) {
            (true, true) => Some(2),
            (false, false) => None,
            _ => Some(1),
        }
    }

    /// The best of a service's tracks for this probe: an official song
    /// before a clip, never an upload; `None` when none agrees.
    pub(crate) fn pick(&self, hits: &[StreamTrack]) -> Option<StreamTrack> {
        hits.iter()
            .filter(|h| h.playable && h.kind != TrackKind::Upload)
            .filter_map(|h| {
                let artists: Vec<String> = h.artists.iter().map(|a| a.name.clone()).collect();
                let score = self.agrees(&h.title, &artists, h.album.as_ref().map(|a| a.title.as_str()), h.duration_ms)?;
                Some((h.kind == TrackKind::Song, score, h))
            })
            .max_by_key(|(song, score, _)| (*song, *score))
            .map(|(.., h)| h.clone())
    }
}

/// The library's tracks, by title key, to find songs among.
pub(crate) struct Shelf {
    rows: Vec<MatchRow>,
    by_title: HashMap<String, Vec<usize>>,
    ids: HashSet<TrackId>,
}

impl Shelf {
    pub(crate) fn new(rows: Vec<MatchRow>) -> Self {
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            for (k, _) in own_keys(&r.title) {
                by_title.entry(k).or_default().push(i);
            }
        }
        let ids = rows.iter().map(|r| r.id).collect();
        Self { rows, by_title, ids }
    }

    pub(crate) fn contains(&self, id: TrackId) -> bool {
        self.ids.contains(&id)
    }

    /// The library's track of the probe's song, if any (see the module
    /// docs).
    pub(crate) fn find(&self, p: &Probe) -> Option<TrackId> {
        let mut best: Option<((bool, u8, std::cmp::Reverse<usize>), TrackId)> = None;
        for (key, _) in &p.titles {
            for &i in self.by_title.get(key).into_iter().flatten() {
                let r = &self.rows[i];
                let Some(score) = p.agrees(&r.title, &r.artists, r.album.as_deref(), r.duration_ms.map(u64::from)) else {
                    continue;
                };
                let rank = (r.has_local, score, std::cmp::Reverse(i));
                if best.as_ref().is_none_or(|(b, _)| rank > *b) {
                    best = Some((rank, r.id));
                }
            }
        }
        best.map(|(_, id)| id)
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::streaming::{AlbumRef, ArtistRef, RemoteId, Service};

    use super::*;

    fn item(title: &str, artist: &str, album: Option<&str>) -> Item {
        Item {
            service: Service::Youtube,
            id: Some("video000001".into()),
            title: title.into(),
            artist: artist.into(),
            featured: Vec::new(),
            album: album.map(str::to_owned),
        }
    }

    fn row(n: u128, title: &str, artist: &str, album: &str, local: bool) -> MatchRow {
        MatchRow {
            id: TrackId::from_uuid(nikgob_core::model::Uuid::from_u128(n)),
            title: title.into(),
            artists: vec![artist.into()],
            album: Some(album.into()),
            has_local: local,
            duration_ms: None,
        }
    }

    fn id(n: u128) -> TrackId {
        TrackId::from_uuid(nikgob_core::model::Uuid::from_u128(n))
    }

    #[test]
    fn upload_titles_are_cleaned() {
        assert_eq!(without_number("01 Hive Knight (Hollow Knight: Gods & Nightmares)"), "Hive Knight (Hollow Knight: Gods & Nightmares)");
        assert_eq!(without_number("1. Song"), "Song");
        assert_eq!(without_number("03 - Song"), "Song");
        assert_eq!(without_number("7 weeks & 3 days"), "7 weeks & 3 days");
        assert_eq!(without_number("10 секунд тишины"), "10 секунд тишины");
        assert_eq!(without_number("1212"), "1212");
        assert_eq!(work_key("Hollow Knight: Original Soundtrack"), work_key("Hollow Knight"));
        assert_eq!(work_key("Hollow Knight OST"), work_key("Hollow Knight"));
        assert!(same_work("Made in Abyss", "MADE IN ABYSS ORIGINAL SOUNDTRACK 3"));
        assert!(same_work("Hollow Knight: Hidden Dreams", "Hidden Dreams"));
        assert!(!same_work("OST", "Original Soundtrack"), "nothing but noise");
        assert!(same_artist("Christopher Larkin", "christopher larkin"));
        assert!(same_artist("Татьяна Буланова", "Tatiana Bulanova"));
        assert!(!same_artist("Amellifera", "Christopher Larkin"));
    }

    #[test]
    fn an_upload_finds_the_library_song_by_its_work_or_artist() {
        let shelf = Shelf::new(vec![
            row(1, "Enter Hallownest", "Christopher Larkin", "Hollow Knight: Original Soundtrack", true),
            row(2, "Hive Knight", "Christopher Larkin", "Hollow Knight: Gods & Nightmares", true),
            row(3, "Riko's Theme", "Kevin Penkin", "MADE IN ABYSS ORIGINAL SOUNDTRACK", true),
            row(4, "Dream", "Someone Else", "Dreams", true),
            row(5, "Queen’s Gardens", "Christopher Larkin", "Hollow Knight: Original Soundtrack", true),
            row(6, "Faith", "Kevin Penkin", "MADE IN ABYSS ORIGINAL SOUNDTRACK 2", true),
            row(7, "Song", "Artist", "Album", true),
        ]);
        let find = |title: &str, channel: &str| shelf.find(&Probe::of(&item(title, channel, None), None, false));
        assert_eq!(find("Hollow Knight OST - Enter Hallownest", "Amellifera"), Some(id(1)));
        assert_eq!(find("Hollow Knight OST - Queen's Gardens", "Amellifera"), Some(id(5)));
        assert_eq!(find("01 Hive Knight (Hollow Knight: Gods & Nightmares)", "Кристофер Ларкин"), Some(id(2)));
        assert_eq!(find("Riko's Theme", "Kevin Penkin"), Some(id(3)), "a «- Topic» play");
        assert_eq!(find("Artist - Song (Official Video)", "ArtistOfficial"), Some(id(7)));
        assert_eq!(find("Artist - Song [HD]", "someone"), Some(id(7)));
        // A title alone is not enough; another take is another song.
        assert_eq!(find("Hollow Knight OST - Dream", "Amellifera"), None);
        assert_eq!(find("Riko's Theme", "Amellifera"), None);
        assert_eq!(find("Riko's Theme (Piano Cover) - Made in Abyss", "Pianist"), None);
        assert_eq!(find("Artist - Song (Live)", "someone"), None);
        assert_eq!(find("Восхождение Героя Щита опенинг 2 [FAITH] (Русский кавер от Jackie-O)", "Jackie-O"), None);
        // A length far off is another recording.
        let long = Probe::of(&item("Artist - Song", "x", None), Some(600_000), false);
        let shelf = Shelf::new(vec![MatchRow { duration_ms: Some(200_000), ..row(7, "Song", "Artist", "Album", true) }]);
        assert_eq!(shelf.find(&long), None);
    }

    #[test]
    fn the_file_wins_and_the_saved_song_finds_its_track() {
        let shelf = Shelf::new(vec![
            row(1, "Song", "Artist", "Single", false),
            row(2, "Song", "Artist", "Album", true),
        ]);
        let saved = Probe::of(&item("Song", "Artist", Some("Album")), None, true);
        assert_eq!(shelf.find(&saved), Some(id(2)));
        assert_eq!(saved.query, "Artist Song");
    }

    #[test]
    fn a_service_song_is_picked_over_clips_and_uploads() {
        let hit = |id: &str, title: &str, artist: &str, album: Option<&str>, kind: TrackKind| {
            let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
            t.artists = vec![ArtistRef::new(None, artist)];
            t.album = album.map(|a| AlbumRef::new(None, a));
            t.kind = kind;
            t
        };
        let p = Probe::of(&item("Hollow Knight OST - Enter Hallownest", "Amellifera", None), None, false);
        assert_eq!(p.query, "Hollow Knight OST Enter Hallownest");
        let hits = [
            hit("up", "Enter Hallownest", "Amellifera", None, TrackKind::Upload),
            hit("clip", "Enter Hallownest", "Christopher Larkin", Some("Hollow Knight"), TrackKind::Video),
            hit("song", "Enter Hallownest", "Christopher Larkin", Some("Hollow Knight (Original Soundtrack)"), TrackKind::Song),
            hit("other", "Enter the Hall", "Christopher Larkin", Some("Hollow Knight (Original Soundtrack)"), TrackKind::Song),
        ];
        assert_eq!(p.pick(&hits).map(|t| t.id.id), Some("song".to_owned()));
        assert_eq!(p.pick(&hits[..1]), None, "an upload is no song");
        let stranger = Probe::of(&item("Some Vlog - Enter Hallownest", "vlogger", None), None, false);
        assert_eq!(stranger.pick(&hits), None);
    }
}
