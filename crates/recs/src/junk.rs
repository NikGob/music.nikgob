//! What new music is never proposed (recs-v2 §11.3): the uploads around a
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
//!   artist's own channel: a «Name - Song» title naming someone else than
//!   the channel is a reupload (unless the title names the channel, or is
//!   a title and its translation in another script), a channel named after
//!   nightcore or lyrics is one too, and a user's upload is never a release.
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
const CHANNEL_WORDS: &[&str] = &["nightcore", "lyrics", "lyric", "slowed", "sped", "8d", "bass boosted", "karaoke", "reverb"];

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
        Service::Youtube => t.kind == TrackKind::Song,
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
                let ch_spaced = spaced(ch);
                if CHANNEL_WORDS.iter().any(|w| has_phrase(&ch_spaced, w)) {
                    return Some(Junk::Channel);
                }
                let ch_key = artist_key(ch);
                let folded_ch = ch_key.replace(' ', "");
                let vevo = folded_ch.ends_with("vevo");
                let named = |name: &str| {
                    let k = artist_key(name).replace(' ', "");
                    !k.is_empty() && (k == folded_ch || (vevo && folded_ch.strip_suffix("vevo") == Some(k.as_str())))
                };
                if let Some(c) = canonical.filter(|c| !c.artist.is_empty())
                    && !named(&c.artist)
                {
                    return Some(Junk::Channel);
                }
                // «Someone - Song» on another channel: a reupload. Not when
                // the title names the channel («Song - Eve MV», «A, Eve –
                // Song»), nor a title with its own translation in another
                // script («暴徒 - Bouto»).
                for dash in [" - ", " – ", " — "] {
                    if let Some(at) = t.title.find(dash) {
                        let (head, tail) = (&t.title[..at], &t.title[at + dash.len()..]);
                        let own = mentions(&t.title, ch) || script(head) != script(tail);
                        if !own && !named(head) && !head.chars().any(|c| matches!(c, '(' | '[' | '「' | '【')) {
                            return Some(Junk::Channel);
                        }
                        break;
                    }
                }
            }
        }
    }
    if let Some(d) = t.duration_ms {
        if d > LOOP_MS {
            return Some(Junk::Loop);
        }
        if let Some(c) = canonical.and_then(|c| c.duration_ms).filter(|c| *c > 0) {
            let off = (d as f64 - c as f64).abs() / c as f64;
            if off > DURATION_SLACK {
                return Some(Junk::Duration);
            }
        }
    }
    None
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
        topic.album = Some(AlbumRef { id: None, title: "LEO-NiNE".into(), artists: Vec::new(), year: None });
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
}
