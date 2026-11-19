//! What new music is never proposed: the uploads around a
//! song that are not the song. Every candidate the services give for
//! «Новое для тебя», «С новым», a radio's new music and the services'
//! suggestions passes [`junk`] first; the library is never filtered (a
//! cover the user keeps is theirs).
//!
//! * **Words** of a version nobody asked for, in any part of the title or
//!   the album: nightcore, slowed, sped up, reverb, 8D, bass boosted, an
//!   hour of it, TikTok, karaoke, a lyric video, an AI cover, a Russian
//!   cover; and in a bracket or a « - » tail of an upload that is not an
//!   official release: cover, remix, instrumental, piano version, loop,
//!   extended, lyrics. A release of a label (Deezer, a YouTube Music album
//!   track) may well be «(Piano Version)» or a cover album.
//! * **Channels**: Deezer and Spotify are canonical, and so is a YouTube
//!   Music album track (a «- Topic» channel). A video is kept only from the
//!   artist's own channel or a label's: a «Name - Song» title naming someone
//!   else than the channel is a reupload (unless the title names the
//!   channel, is a title and its translation, or a title and what music it
//!   is, «Kallipolian Dance - Ancient Greek Music»; a Latin name before a
//!   title in another script, «FACE - ЮМОРИСТ», is an artist, not a
//!   translation), a channel named after nightcore or lyrics is one too
//!   (its name as written, «RusLyrics»), and a user's upload is never a
//!   release.
//! * **Unknown kind**: a YouTube track known only by its id, title and
//!   channel (graph lists kept before their kind was) is judged as a video
//!   too ([`junk_any_kind`]), unless its channel is official (a «- Topic»
//!   channel, a label's): a song YouTube Music lists is not told from an
//!   upload there. A track whose kind is known is judged by it. Names match in either script («Борислав Славов» is
//!   «Borislav Slavov») and without a channel's «VEVO», «Official» or
//!   «Music»; a channel of the canonical artist is the artist's own.
//! * **Length**: more than [`DURATION_SLACK`] off the canonical length
//!   (Deezer's, by ISRC or the song) is a re-cut or a speed-up; longer than
//!   [`LOOP_MS`] is a loop.
//! * **Duplicates** fold by [`song_key`](crate::discover::song_key) and by
//!   ISRC ([`same_song`]).

use nikgob_core::streaming::{Service, StreamTrack, TrackKind};

use crate::discover::{artist_key, stream_song_key};

/// The share a candidate may be off the canonical length.
pub const DURATION_SLACK: f64 = 0.08;
/// Longer than this is an hour of one song, whatever the title says.
pub const LOOP_MS: u64 = 20 * 60_000;

/// Why a candidate is not proposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Junk {
    /// A version nobody asked for, by this word of its title.
    Word(&'static str),
    /// An upload of someone else than the artist.
    Channel,
    /// Off the canonical length.
    Duration,
    /// Too long to be the song.
    Loop,
}

/// What the canonical release of a song says (Deezer's, matched by ISRC or
/// by [`song_key`](crate::discover::song_key)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Canonical {
    /// [`artist_key`] of its main artist.
    pub artist: String,
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
}

impl Canonical {
    pub fn of(t: &StreamTrack) -> Self {
        Self {
            artist: t.artists.first().map(|a| artist_key(&a.name)).unwrap_or_default(),
            duration_ms: t.duration_ms,
            isrc: t.isrc.clone(),
        }
    }
}

/// Words that make a candidate junk wherever they stand.
const ALWAYS: &[&str] = &[
    "nightcore",
    "slowed",
    "sped up",
    "speed up",
    "speeded up",
    "reverb",
    "reverbed",
    "8d",
    "8d audio",
    "bass boosted",
    "bassboosted",
    "1 hour",
    "10 hours",
    "1 hour loop",
    "1 час",
    "10 часов",
    "tiktok",
    "tik tok",
    "karaoke",
    "караоке",
    "минусовка",
    "lyric video",
    "lyrics video",
    "ai cover",
    "ии кавер",
    "rus cover",
    "ru cover",
    "russian cover",
    "на русском",
    "кавер на русском",
    "phonk remix",
];

/// Words that make an unofficial upload junk in a bracket or a tail.
const UNOFFICIAL: &[&str] = &[
    "cover",
    "кавер",
    "remix",
    "ремикс",
    "instrumental",
    "piano version",
    "piano cover",
    "loop",
    "extended",
    "lyrics",
    "текст",
];

/// A channel named so uploads others' songs.
const CHANNEL_WORDS: &[&str] = &[
    "nightcore", "lyrics", "lyric", "slowed", "sped", "8d", "bass boosted", "karaoke", "reverb", "тексты", "текст песен",
    "караоке",
];

/// What a label's channel is called by: it uploads its artists' songs.
const LABEL_WORDS: &[&str] = &[
    "records",
    "recordings",
    "entertainment",
    "music group",
    "label",
    "рекордс",
    "лейбл",
    "звукозапись",
];

/// What a channel adds to the artist's name («TheWeekndVEVO», «Eve
/// Official», «Farya Faraji Music»).
const CHANNEL_SUFFIXES: &[&str] = &["vevo", "official", "officialchannel", "music", "tv", "channel", "официальный"];

/// What a title's tail says when it names the music, not the artist
/// («Ancient Greek Music»).
const MUSIC_WORDS: &[&str] = &["music", "musik", "musique", "музыка", "музика"];

/// Lower case, `ё` → `е`, words split by anything but letters and digits,
/// joined by single spaces with a space at both ends («(Slowed + Reverb)» →
/// « slowed reverb »), so a phrase is found as whole words.
fn spaced(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push(' ');
    let mut gap = true;
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = if c == 'ё' { 'е' } else { c };
        if c.is_alphanumeric() {
            out.push(c);
            gap = false;
        } else if !gap {
            out.push(' ');
            gap = true;
        }
    }
    if !gap {
        out.push(' ');
    }
    out
}

fn has_phrase(spaced: &str, phrase: &str) -> bool {
    spaced.contains(&format!(" {phrase} "))
}

/// The brackets and the « - » tails of a title: where a version says what
/// it is («Song (Piano Version)», «Song - Remix»).
fn decorations(title: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[', '【', '「']) {
        let ch = rest[open..].chars().next().unwrap_or('(');
        let close = match ch {
            '(' => ')',
            '[' => ']',
            '【' => '】',
            _ => '」',
        };
        let after = &rest[open + ch.len_utf8()..];
        match after.find(close) {
            Some(c) => {
                out.push(&after[..c]);
                rest = &after[c + close.len_utf8()..];
            }
            None => {
                out.push(after);
                break;
            }
        }
    }
    for dash in [" - ", " – ", " — ", " | "] {
        if let Some(at) = title.find(dash) {
            out.push(&title[at + dash.len()..]);
            out.push(&title[..at]);
        }
    }
    out
}

/// An official release: canonical services, a YouTube Music album track.
fn official(t: &StreamTrack) -> bool {
    match t.id.service {
        Service::Deezer | Service::Spotify => true,
        // SoundCloud: a release with its publisher's metadata, not an upload.
        Service::Youtube | Service::Soundcloud => t.kind == TrackKind::Song,
    }
}

/// `title` names `channel` as a whole word or words («AIYOU - Eve Music
/// Video» of Eve).
fn mentions(title: &str, channel: &str) -> bool {
    let ch = spaced(channel);
    let ch = ch.trim();
    !ch.is_empty() && has_phrase(&spaced(title), ch)
}

/// The writing system most letters of `s` are in: Latin, Cyrillic, Greek,
/// the Japanese, Chinese and Korean scripts as one, or another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
    Greek,
    Cjk,
    Other,
}

fn script(s: &str) -> Option<Script> {
    let mut n = [0usize; 5];
    for c in s.chars().filter(|c| c.is_alphabetic()) {
        let k = match c as u32 {
            0x41..=0x24F => Script::Latin,
            0x370..=0x3FF => Script::Greek,
            0x400..=0x52F => Script::Cyrillic,
            0x3040..=0x30FF | 0x3400..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F => Script::Cjk,
            _ => Script::Other,
        };
        n[k as usize] += 1;
    }
    let (i, top) = n.iter().enumerate().max_by_key(|x| *x.1)?;
    (*top > 0).then(|| [Script::Latin, Script::Cyrillic, Script::Greek, Script::Cjk, Script::Other][i])
}

/// `a` and `b` are one artist's name: the same [`artist_key`] with no
/// spaces, or the same name in Cyrillic and in Latin letters.
fn same_name(a: &str, b: &str) -> bool {
    let (ka, kb) = (artist_key(a), artist_key(b));
    if ka.is_empty() || kb.is_empty() {
        return false;
    }
    ka.replace(' ', "") == kb.replace(' ', "") || nikgob_core::translit::same_translit(&ka, &kb)
}

/// The names a channel may stand for: its own, and without what channels
/// add to the artist's name (see [`CHANNEL_SUFFIXES`]).
fn channel_names(channel: &str) -> Vec<String> {
    let key = artist_key(channel);
    let mut out = vec![key.clone()];
    let mut rest = key.as_str();
    loop {
        let before = rest;
        for suffix in CHANNEL_SUFFIXES {
            if let Some(r) = rest.strip_suffix(suffix) {
                let r = r.trim_end();
                if !r.is_empty() {
                    rest = r;
                }
            }
        }
        if rest == before {
            break;
        }
        out.push(rest.to_owned());
    }
    out
}

/// `name` is the artist the channel is named after.
fn names_channel(name: &str, channel: &str) -> bool {
    channel_names(channel).iter().any(|c| same_name(name, c))
}

/// A label's channel (see [`LABEL_WORDS`]).
fn label(channel: &str) -> bool {
    let ch = spaced(channel);
    LABEL_WORDS.iter().any(|w| has_phrase(&ch, w))
}

/// A tail that says what music it is rather than naming a song: two words
/// or more, the last of them «music».
fn names_the_music(tail: &str) -> bool {
    let bare = tail.split(['(', '[']).next().unwrap_or(tail);
    let words: Vec<&str> = bare.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    words.len() >= 2 && words.last().is_some_and(|w| MUSIC_WORDS.contains(&w.to_lowercase().as_str()))
}

/// `head` and `tail` are one title twice, in two languages or spellings
/// («A Horny Money World ～伝説の夜～ - A Horny Money World 〜legendary
/// night〜»): they share at least two words and half the words of the
/// shorter one.
fn one_title_twice(head: &str, tail: &str) -> bool {
    let (h, t) = (spaced(head), spaced(tail));
    let hw: Vec<&str> = h.split_whitespace().collect();
    let tw: Vec<&str> = t.split_whitespace().collect();
    let shared = tw.iter().filter(|w| hw.contains(w)).count();
    shared >= 2 && 2 * shared >= hw.len().min(tw.len())
}

/// What a « - » tail says when it only names a version of the record
/// («Song - Remastered 2011», «Song - Radio Edit»), not a song.
const VERSION_TAIL: &[&str] = &[
    "remaster", "remastered", "live", "edit", "radio", "version", "ver", "mix", "acoustic", "demo", "mono",
    "stereo", "original", "single", "album", "deluxe", "edition", "bonus", "track", "mixed", "session",
];

/// A tail of only [`VERSION_TAIL`] words and numbers.
fn only_version(tail: &str) -> bool {
    let words: Vec<String> = spaced(tail).split_whitespace().map(str::to_owned).collect();
    !words.is_empty()
        && words.iter().any(|w| VERSION_TAIL.contains(&w.as_str()))
        && words.iter().all(|w| VERSION_TAIL.contains(&w.as_str()) || w.chars().all(|c| c.is_ascii_digit()))
}

/// `head - tail` is one title in two scripts («暴徒 - Bouto», «Звезда -
/// Star»): the original first, then its reading or translation. A Latin
/// head before a title in another script names the artist («FACE -
/// ЮМОРИСТ»), as Latin names of Russian, Japanese or Korean artists go.
fn translated(head: &str, tail: &str) -> bool {
    let (h, t) = (script(head), script(tail));
    h != t && h != Some(Script::Latin)
}

/// A channel's name in words: split where it is written as one
/// («RusLyrics» → «rus lyrics», «NightcoreReality» → «nightcore reality»).
fn channel_words(channel: &str) -> String {
    let mut out = String::with_capacity(channel.len() + 4);
    let mut prev: Option<char> = None;
    for c in channel.chars() {
        if c.is_uppercase() && prev.is_some_and(char::is_lowercase) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    spaced(&out)
}

/// A «- Topic» channel (YouTube Music's own for an artist's releases) or
/// a label's.
fn official_channel(channel: &str) -> bool {
    channel.trim_end().to_lowercase().ends_with("- topic") || label(channel)
}

/// A channel named after what reuploads others' songs ([`CHANNEL_WORDS`]):
/// as a word of its name, or a word that ends in «lyrics» («ruslyrics»).
fn junk_channel(channel: &str) -> bool {
    let words = channel_words(channel);
    CHANNEL_WORDS.iter().any(|w| has_phrase(&words, w))
        || words.split_whitespace().any(|w| w.ends_with("lyrics"))
}

/// The channel of a YouTube upload, as the services give it (its first
/// artist).
fn channel(t: &StreamTrack) -> &str {
    t.artists.first().map_or("", |a| a.name.as_str())
}

/// Why `t` is not proposed, if it is junk (see the module docs);
/// `canonical` is what the canonical release of its song says, when known.
pub fn junk(t: &StreamTrack, canonical: Option<&Canonical>) -> Option<Junk> {
    let official = official(t);
    let mut text = t.title.clone();
    if let Some(a) = &t.album {
        text.push_str(" (");
        text.push_str(&a.title);
        text.push(')');
    }
    let all = spaced(&text);
    if let Some(w) = ALWAYS.iter().find(|w| has_phrase(&all, w)) {
        return Some(Junk::Word(w));
    }
    if !official {
        for part in decorations(&t.title) {
            let part = spaced(part);
            if let Some(w) = UNOFFICIAL.iter().find(|w| has_phrase(&part, w)) {
                return Some(Junk::Word(w));
            }
        }
    }
    if t.id.service == Service::Youtube {
        match t.kind {
            TrackKind::Song => {}
            TrackKind::Upload => return Some(Junk::Channel),
            TrackKind::Video => {
                let ch = channel(t);
                if junk_channel(ch) {
                    return Some(Junk::Channel);
                }
                // A label uploads its artists' songs.
                if label(ch) {
                    return duration_junk(t, canonical);
                }
                let named = |name: &str| names_channel(name, ch);
                match canonical.filter(|c| !c.artist.is_empty()) {
                    // The song's artist's own channel: whatever the title.
                    Some(c) if named(&c.artist) => return duration_junk(t, canonical),
                    Some(_) => return Some(Junk::Channel),
                    None => {}
                }
                // «Someone - Song» on another channel: a reupload. Not when
                // the title names the channel («Song - Eve MV», «A, Eve –
                // Song»), a title with its own translation in another script
                // («暴徒 - Bouto»), nor a title and what music it is
                // («Kallipolian Dance - Ancient Greek Music»).
                for dash in [" - ", " – ", " — "] {
                    if let Some(at) = t.title.find(dash) {
                        let (head, tail) = (&t.title[..at], &t.title[at + dash.len()..]);
                        let own = mentions(&t.title, ch) || translated(head, tail) || names_the_music(tail)
                            || one_title_twice(head, tail) || only_version(tail);
                        if !own && !named(head) && !head.chars().any(|c| matches!(c, '(' | '[' | '「' | '【')) {
                            return Some(Junk::Channel);
                        }
                        break;
                    }
                }
            }
        }
    }
    duration_junk(t, canonical)
}

/// [`junk`] of a track whose kind is not known (see the module docs): a
/// YouTube one is judged as a video too, unless its channel is official.
pub fn junk_any_kind(t: &StreamTrack, canonical: Option<&Canonical>) -> Option<Junk> {
    if let Some(j) = junk(t, canonical) {
        return Some(j);
    }
    if t.id.service != Service::Youtube || t.kind != TrackKind::Song || official_channel(channel(t)) {
        return None;
    }
    let video = StreamTrack { kind: TrackKind::Video, ..t.clone() };
    junk(&video, canonical)
}

/// A loop, or off the canonical length.
fn duration_junk(t: &StreamTrack, canonical: Option<&Canonical>) -> Option<Junk> {
    let d = t.duration_ms?;
    if d > LOOP_MS {
        return Some(Junk::Loop);
    }
    let c = canonical.and_then(|c| c.duration_ms).filter(|c| *c > 0)?;
    let off = (d as f64 - c as f64).abs() / c as f64;
    (off > DURATION_SLACK).then_some(Junk::Duration)
}

/// One song across uploads: the same [`stream_song_key`] or the same ISRC.
pub fn same_song(a: &StreamTrack, b: &StreamTrack) -> bool {
    if let (Some(x), Some(y)) = (&a.isrc, &b.isrc)
        && x == y
    {
        return true;
    }
    stream_song_key(a) == stream_song_key(b)
}

/// The canonical release of `t` among `pool` (Deezer's tracks of the same
/// ISRC, else of the same song).
pub fn canonical_in<'a>(t: &StreamTrack, pool: impl IntoIterator<Item = &'a StreamTrack>) -> Option<Canonical> {
    let key = stream_song_key(t);
    let mut by_song = None;
    for c in pool {
        if c.id.service != Service::Deezer || c.id == t.id {
            continue;
        }
        if t.isrc.is_some() && c.isrc == t.isrc {
            return Some(Canonical::of(c));
        }
        if by_song.is_none() && stream_song_key(c) == key {
            by_song = Some(Canonical::of(c));
        }
    }
    by_song
}

#[cfg(test)]
mod tests {
    use nikgob_core::streaming::{AlbumRef, ArtistRef, RemoteId};

    use super::*;

    fn yt(title: &str, channel: &str, kind: TrackKind, secs: u64) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, title), title);
        t.artists = vec![ArtistRef::new(None, channel)];
        t.kind = kind;
        t.duration_ms = Some(secs * 1000);
        t
    }

    fn dz(title: &str, artist: &str, secs: u64) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Deezer, title), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = Some(secs * 1000);
        t
    }

    /// The uploads YouTube Music's radios bring next to a song.
    #[test]
    fn nightcore_slowed_cover_out() {
        let v = TrackKind::Video;
        let cases = [
            yt("Nightcore - Bad Apple!!", "NightcoreReality", v, 200),
            yt("Kyoto (slowed + reverb)", "Phoebe Bridgers", v, 290),
            yt("Мама Люба (Speed Up)", "SEREBRO", v, 150),
            yt("Lovely - Billie Eilish (8D AUDIO)", "8D Tunes", v, 200),
            yt("Daddy Cool [Bass Boosted]", "Boney M.", v, 210),
            yt("Group Blood 1 Hour", "Кино", v, 3600),
            yt("Иду курю TikTok version", "Cream Soda", TrackKind::Song, 160),
            yt("Gurenge (Karaoke)", "LiSA", v, 240),
            yt("Lilac (Lyric Video)", "IU", v, 214),
            yt("Ветер перемен (AI Cover)", "Shaman AI", v, 200),
            yt("There is a reason (RUS COVER)", "j.am", v, 230),
            yt("Unravel (Piano Cover)", "Animenz", v, 250),
            yt("Blinding Lights - The Weeknd (Lyrics)", "7clouds", v, 200),
            yt("Shape of You (Phonk Remix)", "Ed Sheeran", v, 180),
        ];
        for t in &cases {
            assert!(junk(t, None).is_some(), "{} by {} is junk", t.title, t.artists[0].name);
        }
        assert_eq!(junk(&cases[1], None), Some(Junk::Word("slowed")));
        assert_eq!(junk(&cases[10], None), Some(Junk::Word("rus cover")));
    }

    /// Releases of labels and the artist's own clips stay, a cover album
    /// and a piano version of the label too.
    #[test]
    fn topic_channel_kept() {
        let mut topic = yt("Gurenge", "LiSA - Topic", TrackKind::Song, 239);
        topic.album = Some(AlbumRef { id: None, title: "LEO-NiNE".into(), artists: Vec::new(), year: None, release_type: None });
        let kept = [
            topic,
            yt("Kick Back", "Kenshi Yonezu", TrackKind::Song, 193),
            yt("米津玄師 MV「KICK BACK」", "米津玄師", TrackKind::Video, 194),
            yt("Eve - Ghost Avenue", "Eve", TrackKind::Video, 220),
            yt("Группа крови", "КИНО", TrackKind::Video, 286),
            yt("Blinding Lights (Official Video)", "TheWeekndVEVO", TrackKind::Video, 262),
            dz("Liebestraum (Piano Version)", "Lang Lang", 280),
            dz("Hurt (Cover)", "Johnny Cash", 218),
            yt("Loop", "Royal Blood", TrackKind::Song, 200),
            yt("Cover Me Up", "Jason Isbell", TrackKind::Song, 324),
        ];
        for t in &kept {
            assert_eq!(junk(t, None), None, "{} by {}", t.title, t.artists[0].name);
        }
        // The artist's own clips with their song named after a dash, or a
        // title with its translation in another script.
        for (title, channel) in [
            ("AIYOU - Eve Music Video", "Eve"),
            ("トーキョーゲットー - Eve MV", "Eve"),
            ("暴徒 - Bouto", "Eve"),
            ("SLAVA MARLOW, MellSher – СКУЧНО", "MellSher"),
            ("ラブソング feat. Eve / キタニタツヤ - Love Song feat. Eve / Tatsuya Kitani", "Tatsuya Kitani"),
        ] {
            assert_eq!(junk(&yt(title, channel, TrackKind::Video, 200), None), None, "{title} by {channel}");
        }
        assert_eq!(junk(&yt("Кино - Группа крови", "Rock Archive", TrackKind::Video, 286), None), Some(Junk::Channel));
        // A user's upload or a reupload of someone else's song is not.

        assert_eq!(junk(&yt("Kick Back", "anime fan 2004", TrackKind::Upload, 193), None), Some(Junk::Channel));
        assert_eq!(junk(&yt("Kenshi Yonezu - Kick Back", "Anime Vibes", TrackKind::Video, 193), None), Some(Junk::Channel));
        assert_eq!(junk(&yt("Kick Back", "Kick Back Lyrics", TrackKind::Video, 193), None), Some(Junk::Channel));
        let canon = Canonical { artist: artist_key("Kenshi Yonezu"), duration_ms: Some(193_000), isrc: None };
        assert_eq!(junk(&yt("KICK BACK", "Music Lab", TrackKind::Video, 193), Some(&canon)), Some(Junk::Channel));
        assert_eq!(junk(&yt("KICK BACK", "Kenshi Yonezu", TrackKind::Video, 193), Some(&canon)), None);
    }

    /// The artists' own channels the reupload rule cut (Farya Faraji's
    /// «Kallipolian Dance - Ancient Greek Music», as YouTube Music's
    /// discovery gave it; Борислав Славов's channel and Deezer's «Borislav
    /// Slavov»), their «- Topic» channels and labels stay; reuploads of the
    /// same songs by others still go.
    #[test]
    fn own_channels_in_any_script_and_labels_kept() {
        let v = TrackKind::Video;
        let kept = [
            yt("Kallipolian Dance - Ancient Greek Music", "Farya Faraji", v, 165),
            yt("Hymn to Apollo - Ancient Greek Music", "Farya Faraji", v, 240),
            yt("Gothic Song - Ancient Germanic Music", "Farya Faraji", v, 200),
            yt("Borislav Slavov - Down by the River", "Борислав Славов", v, 230),
            yt("Борислав Славов - I Want to Live", "Borislav Slavov", v, 260),
            yt("Борислав Славов - Down by the River", "Борислав Славов Official", v, 230),
            yt("Борислав Славов - Down by the River", "Борислав Славов - Topic", v, 230),
            yt("Kenshi Yonezu - Kick Back", "Kenshi Yonezu Official", v, 193),
            yt("Linkin Park - Numb", "Warner Records", v, 187),
            yt("Кино - Группа крови", "Мелодия Звукозапись", v, 286),
            yt("Lindemann - Steh auf", "Universal Music Group", v, 230),
            yt("A Horny Money World ～伝説の夜～ - A Horny Money World 〜legendary night〜", "Houshou Marine", v, 230),
        ];
        for t in &kept {
            assert_eq!(junk(t, None), None, "{} by {}", t.title, t.artists[0].name);
        }
        // The canonical artist's own channel, in another script, keeps even
        // a title the rule alone would cut.
        let canon = Canonical { artist: artist_key("Borislav Slavov"), duration_ms: Some(230_000), isrc: None };
        assert_eq!(junk(&yt("Baldur's Gate 3 - Down by the River", "Борислав Славов", v, 230), Some(&canon)), None);
        let canon = Canonical { artist: artist_key("Farya Faraji"), duration_ms: Some(165_000), isrc: None };
        assert_eq!(junk(&yt("Kallipolian Dance - Ancient Greek Music", "Farya Faraji", v, 165), Some(&canon)), None);
        // Still a speed-up on the own channel.
        assert_eq!(junk(&yt("Kallipolian Dance - Ancient Greek Music", "Farya Faraji", v, 140), Some(&canon)), Some(Junk::Duration));

        let cut = [
            yt("Farya Faraji - Kallipolian Dance", "Epic Music World", v, 165),
            yt("Борислав Славов - Вниз по реке", "Game OST Archive", v, 230),
            yt("Borislav Slavov - I Want to Live", "BG3 Fan", v, 260),
            yt("Madonna - Music", "80s Hits", v, 225),
            yt("Bob Marley - Redemption Song", "Reggae Classics", v, 227),
            yt("Kallipolian Dance (Nightcore)", "Farya Faraji", v, 140),
            // Real ones of the discovery cache.
            yt("Zankyou No Terror OST - nc17", "Gracious Raigeki", v, 200),
            yt("Fox Sailor - Lord of Fire", "My Soul Your Beats", v, 200),
            yt("Daft Punk - One More Time", "Daft Punk Fan Music", v, 320),
        ];
        for t in &cut {
            assert!(junk(t, None).is_some(), "{} by {}", t.title, t.artists[0].name);
        }
        let canon = Canonical { artist: artist_key("Borislav Slavov"), duration_ms: Some(230_000), isrc: None };
        assert_eq!(junk(&yt("Down by the River", "Game OST Archive", v, 230), Some(&canon)), Some(Junk::Channel));
        assert_eq!(junk(&yt("Down by the River", "Борислав Славов", v, 230), Some(&canon)), None);
    }

    #[test]
    fn channel_names_without_what_channels_add() {
        assert!(names_channel("The Weeknd", "TheWeekndVEVO"));
        assert!(names_channel("Eve", "Eve Official"));
        assert!(names_channel("Farya Faraji", "Farya Faraji Music"));
        assert!(names_channel("Borislav Slavov", "Борислав Славов"));
        assert!(names_channel("Кино", "КИНО - Topic"));
        assert!(!names_channel("Kino", "Rock Archive"));
        // A channel that is only a word channels add is no artist's.
        assert!(!names_channel("Music", "Rock Music"));
        assert!(names_the_music("Ancient Greek Music"));
        assert!(names_the_music("Древняя греческая музыка (live)"));
        assert!(!names_the_music("Music"));
        assert!(!names_the_music("Redemption Song"));
    }

    /// A speed-up or a re-cut is off the canonical length; an hour is a loop.
    #[test]
    fn duration_mismatch_out() {
        let canon = dz("Группа крови", "Кино", 286);
        let c = Canonical::of(&canon);
        assert_eq!(junk(&yt("Группа крови", "Кино - Topic", TrackKind::Song, 290), Some(&c)), None, "1.4 % off");
        assert_eq!(junk(&yt("Группа крови", "Кино - Topic", TrackKind::Song, 250), Some(&c)), Some(Junk::Duration));
        assert_eq!(junk(&yt("Группа крови", "Кино - Topic", TrackKind::Song, 320), Some(&c)), Some(Junk::Duration));
        assert_eq!(junk(&yt("Группа крови", "Кино - Topic", TrackKind::Song, 25 * 60), None), Some(Junk::Loop));
        // The canonical one by ISRC, else by the song.
        let mut a = yt("Группа крови (Remastered)", "Кино - Topic", TrackKind::Song, 286);
        a.isrc = Some("RUA1D8800001".into());
        let mut other = dz("Blood Type", "Kino", 300);
        other.isrc = Some("RUA1D8800001".into());
        let pool = [canon.clone(), other.clone()];
        assert_eq!(canonical_in(&a, &pool).and_then(|c| c.duration_ms), Some(300_000), "the ISRC first");
        a.isrc = None;
        assert_eq!(canonical_in(&a, &pool).and_then(|c| c.duration_ms), Some(286_000));
        assert!(same_song(&canon, &yt("Группа крови", "Кино", TrackKind::Video, 286)));
        assert!(same_song(&other, &StreamTrack { isrc: Some("RUA1D8800001".into()), ..yt("x", "y", TrackKind::Song, 1) }));
    }

    /// The wave played «FACE - ЮМОРИСТ (ТЕКСТ И ПЕСНЯ)» of the channel
    /// «RusLyrics»: out on every count, and out when its kind is not known
    /// (the graph of similar music keeps none).
    #[test]
    fn a_lyrics_channel_reupload_is_out_on_every_count() {
        let t = yt("FACE - ЮМОРИСТ (ТЕКСТ И ПЕСНЯ)", "RusLyrics", TrackKind::Video, 144);
        assert_eq!(junk(&t, None), Some(Junk::Word("текст")));
        // Each count alone.
        assert_eq!(junk(&yt("FACE - ЮМОРИСТ", "RusLyrics", TrackKind::Video, 144), None), Some(Junk::Channel));
        assert_eq!(junk(&yt("FACE - ЮМОРИСТ", "Хиты 2018", TrackKind::Video, 144), None), Some(Junk::Channel));
        assert_eq!(junk(&yt("ЮМОРИСТ (ТЕКСТ И ПЕСНЯ)", "FACE", TrackKind::Video, 144), None), Some(Junk::Word("текст")));
        assert!(junk_channel("RusLyrics") && junk_channel("ruslyrics") && junk_channel("Тексты песен"));
        assert!(!junk_channel("Lyrical Lemonade") && !junk_channel("FACE") && !junk_channel("Контекст"));
        // Known only by its id, title and channel: judged as a video.
        let unknown = StreamTrack { kind: TrackKind::Song, ..t.clone() };
        assert_eq!(junk(&unknown, None), None, "a song of YouTube Music is a release");
        assert!(junk_any_kind(&unknown, None).is_some());
        let unknown = yt("FACE - ЮМОРИСТ", "RusLyrics", TrackKind::Song, 144);
        assert_eq!(junk_any_kind(&unknown, None), Some(Junk::Channel));
        // Known as a song: an official remix or cover stays.
        assert_eq!(junk(&yt("Song (Remix)", "Artist", TrackKind::Song, 200), None), None);
        assert_eq!(junk(&yt("Hurt (Cover)", "Artist", TrackKind::Song, 200), None), None);
        // Unknown kind from an official channel: judged as the song it is.
        assert_eq!(junk_any_kind(&yt("Song (Remix)", "Artist - Topic", TrackKind::Song, 200), None), None);
        assert_eq!(junk_any_kind(&yt("Song (Remix)", "Warner Records", TrackKind::Song, 200), None), None);
        // The artist's own songs stay, a version tail too.
        for (title, channel) in [("ЮМОРИСТ", "FACE"), ("Starboy", "The Weeknd"), ("Numb - Remastered 2011", "Linkin Park"), ("Song - Radio Edit", "Band")] {
            assert_eq!(junk_any_kind(&yt(title, channel, TrackKind::Song, 200), None), None, "{title} by {channel}");
        }
        // A title in Cyrillic and its translation stays one title.
        assert_eq!(junk(&yt("Звезда - Star", "Кино", TrackKind::Video, 200), None), None);
    }
}
