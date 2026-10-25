//! «Миксы для тебя» (home v2 §3, §8): the lanes the user listens in. Songs
//! by the language their lyrics are sung in (the title is a poor guess for
//! this library: Japanese artists write in Latin), else the language their
//! artist's lyrics are in, else what the title says plainly («RUS COVER»,
//! kana, Cyrillic) — a Latin title alone says nothing, and a song of an
//! unknown language goes in no lane by language; soundtracks apart and
//! split by energy into «Тихие» and «Бодрые», and «На повторе». The set and
//! its order follow the lanes' share of starts over four weeks; the songs
//! are picked anew each day (at 06:00, `local_day`) or with «Собрать
//! заново».

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{EventType, TrackId};
use nikgob_core::streaming::StreamTrack;
use nikgob_recs::tiles::{Script, franchise, script_of};
use nikgob_recs::{Axes, TilesInput};
use nikgob_view as v;

use super::{BuildInput, CoverRef, Snapshot, day_seed};

/// A song shorter than this is an interlude: no mix picks it (OMORI has
/// 67 of 179 under a minute).
const INTERLUDE_MS: u32 = 75_000;
/// About two hours a mix.
const MIX_MS: u64 = 2 * 60 * 60 * 1000;
/// A song of unknown length counts so.
const GUESS_MS: u32 = 210_000;
/// A lane needs this many songs to be a mix.
const MIN_SONGS: usize = 12;
/// At most this many mixes.
pub(crate) const MAX_LANES: usize = 6;
/// Of one album at most.
const PER_ALBUM: usize = 8;
/// Japanese is a lane of its own with this many songs and this share of
/// the starts; else it joins English as «Песни не на русском».
const JAPANESE_SONGS: usize = 40;
const JAPANESE_SHARE: f32 = 0.05;
/// The window of the lanes' shares.
const SHARE_DAYS: i64 = 28;
/// «На повторе» looks this far back (30 once the history is longer).
const REPEAT_DAYS_SHORT: i64 = 14;
const REPEAT_DAYS_LONG: i64 = 30;
/// About every fifth song of a mix is new «С новым».
const NEW_EVERY: usize = 5;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// One mix of «Миксы для тебя».
pub(crate) struct LaneHome {
    pub lane: v::Lane,
    pub tracks: Vec<TrackId>,
    /// Its artists, most played first.
    pub artists: Vec<String>,
    pub covers: Vec<CoverRef>,
    /// «С новым»: new songs of the services in the lane's language.
    pub new: Vec<StreamTrack>,
}

impl LaneHome {
    /// What it plays: its songs, with «С новым» a new one after every
    /// fourth.
    pub fn picks(&self, with_new: bool) -> Vec<nikgob_recs::Pick> {
        use nikgob_recs::{Item, Pick, Reason};
        let mut out = Vec::with_capacity(self.tracks.len() + self.new.len());
        let mut new = self.new.iter();
        for (i, t) in self.tracks.iter().enumerate() {
            out.push(Pick { item: Item::Track(*t), reason: Reason::Favourite });
            if with_new
                && (i + 1) % (NEW_EVERY - 1) == 0
                && let Some(n) = new.next()
            {
                out.push(Pick { item: Item::Stream(n.id.clone()), reason: Reason::New });
            }
        }
        out
    }
}

/// What a song is sung in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Lang {
    Russian,
    English,
    Japanese,
    /// German, Spanish, Korean…: never «Песни на английском», maybe «Песни
    /// не на русском».
    Other,
}

/// Lyrics in Latin letters need this many words to tell their language …
const LATIN_WORDS: usize = 6;
/// … and the detector this much confidence.
const LATIN_CONFIDENCE: f64 = 0.5;

/// Uploaders whose name stands where the artist's should (a channel of
/// anime openings): they cast no vote for a language.
const UPLOADERS: &[&str] = &["crunchyroll", "funimation", "aniplex", "muse asia", "ani-one asia", "various artists"];

/// Whether a word is spelt like romanised Japanese: syllables of an
/// optional onset and a vowel, «n», or a doubled consonant.
fn romaji_word(w: &str) -> bool {
    const ONSETS: [&str; 30] = [
        "ky", "gy", "sh", "ch", "ts", "ny", "hy", "by", "py", "my", "ry", "j", "k", "g", "s", "z", "t", "d", "n", "h",
        "b", "p", "m", "y", "r", "w", "f", "", "", "",
    ];
    let b = w.as_bytes();
    let mut i = 0;
    if b.is_empty() {
        return false;
    }
    'next: while i < b.len() {
        // A doubled consonant («kk», «tt»): the first is a pause.
        if i + 1 < b.len() && b[i] == b[i + 1] && b"kstpgdbcmz".contains(&b[i]) {
            i += 1;
            continue;
        }
        for o in ONSETS {
            let o = o.as_bytes();
            if b[i..].starts_with(o) && b.get(i + o.len()).is_some_and(|c| b"aiueo".contains(c)) {
                i += o.len() + 1;
                continue 'next;
            }
        }
        if b[i] == b'n' {
            i += 1;
            continue;
        }
        return false;
    }
    true
}

/// Whether Latin lyrics are romanised Japanese: nearly every word spelt
/// so, and its particles among them.
fn romaji(words: &[String]) -> bool {
    const PARTICLES: [&str; 11] = ["wa", "no", "ga", "wo", "ni", "de", "to", "mo", "yo", "ne", "ka"];
    let shaped = words.iter().filter(|w| romaji_word(w)).count();
    let particles = words.iter().filter(|w| PARTICLES.contains(&w.as_str())).count();
    shaped * 100 >= words.len() * 85 && particles >= 2
}

/// The language of lyrics in Latin letters, when they say it clearly.
fn latin_lang(text: &str) -> Option<Lang> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.len() < LATIN_WORDS {
        return None;
    }
    if romaji(&words) {
        return Some(Lang::Japanese);
    }
    match whatlang::detect(text) {
        Some(info) if info.is_reliable() || info.confidence() >= LATIN_CONFIDENCE => Some(match info.lang() {
            whatlang::Lang::Eng => Lang::English,
            whatlang::Lang::Rus => Lang::Russian,
            whatlang::Lang::Jpn => Lang::Japanese,
            _ => Lang::Other,
        }),
        // A line or two the detector is unsure of: English by its little
        // words («my», «the», «in»).
        _ => english_words(&words).then_some(Lang::English),
    }
}

/// Whether a quarter of the words, two at least, are English little words.
fn english_words(words: &[String]) -> bool {
    const LITTLE: [&str; 40] = [
        "the", "i", "you", "my", "in", "and", "to", "of", "is", "it", "me", "we", "on", "for", "that", "your", "be",
        "all", "with", "what", "this", "but", "are", "was", "can", "just", "don't", "i'm", "it's", "know", "when",
        "never", "they", "our", "his", "her", "from", "at", "will", "have",
    ];
    let hits = words.iter().filter(|w| LITTLE.contains(&w.as_str())).count();
    hits >= 2 && hits * 4 >= words.len()
}

/// What a title says of its song's language plainly: a marker («RUS
/// COVER», «Japanese ver.»), else its script — Cyrillic, kana, kanji or
/// Japanese punctuation, hangul. A Latin title says nothing.
fn title_lang(text: &str) -> Option<Lang> {
    const RUSSIAN: [&str; 12] = [
        "rus cover", "ru cover", "russian cover", "(russian)", "[russian]", "на русском", "rus ver", "rus.ver",
        "russian ver", "(rus)", "[rus]", "кавер на русском",
    ];
    const JAPANESE: [&str; 7] = ["japanese ver", "jp ver", "jpn ver", "jp.ver", "japanese version", "(japanese)", "[japanese]"];
    let lower = text.to_lowercase();
    if RUSSIAN.iter().any(|m| lower.contains(m)) {
        return Some(Lang::Russian);
    }
    if JAPANESE.iter().any(|m| lower.contains(m)) {
        return Some(Lang::Japanese);
    }
    match script_of(text)? {
        Script::Cyrillic => Some(Lang::Russian),
        Script::Japanese => Some(Lang::Japanese),
        Script::Korean => Some(Lang::Other),
        Script::Latin => None,
    }
}

/// The words of a lyrics text as kept: LRC without its time tags, an
/// online source's JSON lines by their `"text"` values, TTML without tags.
fn lyric_words(raw: &str) -> String {
    let raw = raw.trim_start();
    let mut out = String::new();
    if raw.starts_with('[') && raw.contains("\"text\":\"") {
        for part in raw.split("\"text\":\"").skip(1) {
            if let Some(end) = part.find('"') {
                out.push_str(&part[..end]);
                out.push(' ');
            }
        }
        return out;
    }
    let mut depth_sq = 0;
    let mut depth_angle = 0;
    for c in raw.chars() {
        match c {
            '[' => depth_sq += 1,
            ']' => depth_sq = (depth_sq - 1).max(0),
            '<' => depth_angle += 1,
            '>' => depth_angle = (depth_angle - 1).max(0),
            _ if depth_sq == 0 && depth_angle == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The language of a lyrics text, if it has enough words: by its script,
/// and Latin letters by the words themselves.
fn lyrics_lang(raw: &str) -> Option<Lang> {
    let words = lyric_words(raw);
    if words.chars().filter(|c| c.is_alphabetic()).count() < 12 {
        return None;
    }
    match script_of(&words)? {
        Script::Cyrillic => Some(Lang::Russian),
        Script::Japanese => Some(Lang::Japanese),
        Script::Korean => Some(Lang::Other),
        Script::Latin => latin_lang(&words),
    }
}

/// What the lanes know of the songs' languages.
struct Langs {
    /// By the song's own lyrics.
    sung: HashMap<TrackId, Lang>,
    /// The language most of an artist's lyrics are in.
    artist: HashMap<nikgob_core::model::ArtistId, Lang>,
    /// … and by the artist's name, for new songs of the services.
    named: HashMap<String, Lang>,
}

impl Langs {
    /// A song's language: its lyrics, else its artist's, else what its
    /// title and artist say plainly; `None` when nothing tells.
    fn of(&self, id: TrackId, artist: Option<&nikgob_core::model::ArtistId>, title: &str, credit: &str) -> Option<Lang> {
        self.sung
            .get(&id)
            .copied()
            .or_else(|| artist.and_then(|a| self.artist.get(a)).copied())
            .or_else(|| title_lang(&format!("{title} {credit}")))
    }

    /// A new song of a service: its artist's language, else its title's.
    fn of_new(&self, t: &StreamTrack) -> Option<Lang> {
        t.artists
            .iter()
            .find_map(|a| self.named.get(&a.name.to_lowercase()).copied())
            .or_else(|| title_lang(&format!("{} {}", t.title, t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(" "))))
    }
}

/// The languages known from `sung` lyrics: an artist sings in the language
/// most of their lyrics are in (an uploader's channel votes for none; a
/// title guess never votes).
fn langs(snap: &Snapshot, tiles: &TilesInput, sung: HashMap<TrackId, Lang>) -> Langs {
    let name = |a: &nikgob_core::model::ArtistId| tiles.artist_names.get(a).map(|n| n.to_lowercase());
    let mut votes: HashMap<nikgob_core::model::ArtistId, HashMap<Lang, u32>> = HashMap::new();
    for t in snap.catalog.tracks() {
        let (Some(a), Some(l)) = (t.artists.first(), sung.get(&t.id)) else { continue };
        if name(a).is_some_and(|n| UPLOADERS.contains(&n.as_str())) {
            continue;
        }
        *votes.entry(*a).or_default().entry(*l).or_default() += 1;
    }
    let artist: HashMap<_, Lang> = votes
        .into_iter()
        .filter_map(|(a, v)| v.into_iter().max_by_key(|(l, n)| (*n, *l as u8)).map(|(l, _)| (a, l)))
        .collect();
    let named = artist.iter().filter_map(|(a, l)| Some((name(a)?, *l))).collect();
    Langs { sung, artist, named }
}

/// A number in 0..1 of `seed` and a track (a stable shuffle).
fn jitter(seed: u64, t: TrackId) -> f32 {
    let mut h = seed ^ 0x9e37_79b9_7f4a_7c15;
    for b in t.to_string().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    h ^= h >> 33;
    (h % 10_000) as f32 / 10_000.0
}

/// The lanes of today.
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> Vec<LaneHome> {
    let lib = &input.lib;
    let samples = lib.lyrics_samples().unwrap_or_else(|e| {
        log::warn!("recs: lyrics for the lanes: {e}");
        Vec::new()
    });
    let mut instrumental: HashSet<TrackId> = HashSet::new();
    let mut sung: HashMap<TrackId, Lang> = HashMap::new();
    for s in &samples {
        if s.instrumental {
            instrumental.insert(s.id);
        } else if let Some(l) = lyrics_lang(&s.text) {
            sung.insert(s.id, l);
        }
    }
    let langs = langs(snap, tiles, sung);

    let norm = snap.index.normalizer();
    let mut by_lane: HashMap<v::Lane, Vec<(usize, TrackId)>> = HashMap::new();
    let mut soundtracks: Vec<(usize, TrackId, Option<f32>)> = Vec::new();
    let mut other: Vec<(usize, TrackId)> = Vec::new();
    for (i, t) in snap.catalog.tracks().iter().enumerate() {
        if snap.taste.blocks(t) || (snap.local_only && !t.local) {
            continue;
        }
        let Some(f) = tiles.facts.get(&t.id) else { continue };
        if f.duration_ms.is_some_and(|d| d < INTERLUDE_MS) {
            continue;
        }
        let soundtrack = f.album.as_deref().and_then(franchise).is_some() || instrumental.contains(&t.id);
        if soundtrack {
            let energy = snap.index.get(t.id).map(|v| Axes::of(&norm.z(v)).energy);
            soundtracks.push((i, t.id, energy));
            continue;
        }
        let lane = match langs.of(t.id, t.artists.first(), &f.title, &f.artist) {
            Some(Lang::Russian) => v::Lane::Russian,
            Some(Lang::Japanese) => v::Lane::Japanese,
            Some(Lang::English) => v::Lane::English,
            // Not English: only «Песни не на русском» takes it.
            Some(Lang::Other) => {
                other.push((i, t.id));
                continue;
            }
            // Unknown: no lane by language («На повторе» still may).
            None => continue,
        };
        by_lane.entry(lane).or_default().push((i, t.id));
    }
    // Soundtracks: below the median energy calm, above it loud; without
    // a vector, the side of their album's analysed songs, else calm.
    let mut energies: Vec<f32> = soundtracks.iter().filter_map(|s| s.2).collect();
    energies.sort_by(f32::total_cmp);
    let median = energies.get(energies.len() / 2).copied().unwrap_or(0.0);
    for (i, id, e) in soundtracks {
        let lane = if e.is_some_and(|e| e >= median) { v::Lane::DriveSoundtracks } else { v::Lane::CalmSoundtracks };
        by_lane.entry(lane).or_default().push((i, id));
    }

    // The starts of four weeks by lane.
    let now = snap.taste.now();
    let lane_of: HashMap<TrackId, v::Lane> =
        by_lane.iter().flat_map(|(l, ts)| ts.iter().map(move |(_, t)| (*t, *l))).collect();
    let mut starts: HashMap<v::Lane, u32> = HashMap::new();
    let mut total_starts = 0u32;
    for p in tiles.events.iter().filter(|p| p.kind == EventType::Start && p.ts >= now - SHARE_DAYS * DAY_MS) {
        total_starts += 1;
        if let Some(l) = lane_of.get(&p.track) {
            *starts.entry(*l).or_default() += 1;
        }
    }
    let ja = by_lane.get(&v::Lane::Japanese).map_or(0, Vec::len);
    let ja_share = if total_starts == 0 {
        0.0
    } else {
        starts.get(&v::Lane::Japanese).copied().unwrap_or(0) as f32 / total_starts as f32
    };
    if ja < JAPANESE_SONGS || (snap.taste.has_history() && ja_share < JAPANESE_SHARE) {
        let mut merged = by_lane.remove(&v::Lane::English).unwrap_or_default();
        merged.extend(by_lane.remove(&v::Lane::Japanese).unwrap_or_default());
        merged.extend(other);
        let s = starts.remove(&v::Lane::English).unwrap_or(0) + starts.remove(&v::Lane::Japanese).unwrap_or(0);
        if !merged.is_empty() {
            by_lane.insert(v::Lane::NotRussian, merged);
            starts.insert(v::Lane::NotRussian, s);
        }
    }

    let seed = day_seed(&input.day) ^ input.lanes_salt;
    let mut lanes: Vec<(v::Lane, Vec<TrackId>)> = Vec::new();
    for (lane, pool) in &by_lane {
        let picked = pick(snap, tiles, pool, seed ^ (*lane as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
        if picked.len() >= MIN_SONGS {
            lanes.push((*lane, picked));
        }
    }
    // The order of the lanes: by their share of the starts (by size without
    // a history); stable among equals.
    lanes.sort_by(|a, b| {
        let key = |l: v::Lane| (starts.get(&l).copied().unwrap_or(0), by_lane.get(&l).map_or(0, Vec::len));
        let (ka, kb) = (key(a.0), key(b.0));
        if snap.taste.has_history() { kb.cmp(&ka) } else { kb.1.cmp(&ka.1) }.then(a.0.cmp(&b.0))
    });
    lanes.truncate(MAX_LANES - usize::from(snap.taste.has_history()));
    if snap.taste.has_history()
        && let Some(repeat) = on_repeat(snap, tiles)
    {
        lanes.push((v::Lane::OnRepeat, repeat));
    }
    lanes.truncate(MAX_LANES);

    let mut used_new: HashSet<String> = HashSet::new();
    lanes
        .into_iter()
        .map(|(lane, tracks)| {
            let artists = artists_of(snap, tiles, &tracks);
            let covers = covers(snap, &tracks);
            let new = if snap.local_only { Vec::new() } else { new_for(snap, input, &langs, lane, tracks.len(), &mut used_new) };
            LaneHome { lane, tracks, artists, covers, new }
        })
        .collect()
}

/// About two hours of a lane: the best liked first (with a little chance
/// mixed in by the day), no more than a fifth by one artist and
/// [`PER_ALBUM`] of one album, then put in an order where one artist does
/// not play twice running.
fn pick(snap: &Snapshot, tiles: &TilesInput, pool: &[(usize, TrackId)], seed: u64) -> Vec<TrackId> {
    let mut scored: Vec<(f32, usize, TrackId)> = pool
        .iter()
        .map(|&(i, t)| {
            let a = snap.taste.affinity_at(i);
            (a.max(0.0).sqrt() + 0.6 * jitter(seed, t), i, t)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)));
    let mut per_artist: HashMap<Option<nikgob_core::model::ArtistId>, usize> = HashMap::new();
    let mut per_album: HashMap<nikgob_core::model::ReleaseId, usize> = HashMap::new();
    let artist_cap = (pool.len() / 5).clamp(4, 10);
    let mut total: u64 = 0;
    let mut out: Vec<(Option<nikgob_core::model::ArtistId>, TrackId)> = Vec::new();
    let mut songs = HashSet::new();
    for (_, i, t) in scored {
        if total >= MIX_MS {
            break;
        }
        let info = &snap.catalog.tracks()[i];
        if !songs.insert(info.song.unwrap_or(info.id)) {
            continue;
        }
        let artist = info.artists.first().copied();
        if per_artist.get(&artist).copied().unwrap_or(0) >= artist_cap {
            continue;
        }
        if let Some(r) = info.release
            && per_album.get(&r).copied().unwrap_or(0) >= PER_ALBUM
        {
            continue;
        }
        *per_artist.entry(artist).or_default() += 1;
        if let Some(r) = info.release {
            *per_album.entry(r).or_default() += 1;
        }
        total += u64::from(tiles.facts.get(&t).and_then(|f| f.duration_ms).unwrap_or(GUESS_MS));
        out.push((artist, t));
    }
    // One artist not twice running where another can come between.
    let mut ordered: Vec<TrackId> = Vec::with_capacity(out.len());
    let mut last = None;
    while !out.is_empty() {
        let k = out.iter().position(|(a, _)| Some(*a) != last).unwrap_or(0);
        let (a, t) = out.remove(k);
        last = Some(a);
        ordered.push(t);
    }
    ordered
}

/// «На повторе»: the songs started most over the last two weeks (a month
/// once the history is that long), at least twice each, most first.
fn on_repeat(snap: &Snapshot, tiles: &TilesInput) -> Option<Vec<TrackId>> {
    let now = snap.taste.now();
    let first = tiles.events.iter().map(|p| p.ts).min()?;
    let days = if now - first >= 30 * DAY_MS { REPEAT_DAYS_LONG } else { REPEAT_DAYS_SHORT };
    let mut n: HashMap<TrackId, u32> = HashMap::new();
    for p in tiles.events.iter().filter(|p| p.ts >= now - days * DAY_MS) {
        match p.kind {
            EventType::Start => *n.entry(p.track).or_default() += 1,
            EventType::Complete => *n.entry(p.track).or_default() += 1,
            _ => {}
        }
    }
    let mut top: Vec<(u32, TrackId)> = n
        .into_iter()
        .filter(|(t, c)| {
            *c >= 2
                && snap.catalog.get(*t).is_some_and(|i| !snap.taste.blocks(i) && (i.local || !snap.local_only))
                && tiles.facts.get(t).is_none_or(|f| f.duration_ms.is_none_or(|d| d >= INTERLUDE_MS))
        })
        .map(|(t, c)| (c, t))
        .collect();
    top.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut total = 0u64;
    let mut out = Vec::new();
    for (_, t) in top {
        if total >= MIX_MS {
            break;
        }
        total += u64::from(tiles.facts.get(&t).and_then(|f| f.duration_ms).unwrap_or(GUESS_MS));
        out.push(t);
    }
    (out.len() >= MIN_SONGS).then_some(out)
}

/// The main artists of `tracks`, by how many of them they have (then by
/// their first place), as the library writes them.
fn artists_of(snap: &Snapshot, tiles: &TilesInput, tracks: &[TrackId]) -> Vec<String> {
    let mut count: HashMap<nikgob_core::model::ArtistId, (usize, usize)> = HashMap::new();
    for (k, t) in tracks.iter().enumerate() {
        if let Some(a) = snap.catalog.get(*t).and_then(|i| i.artists.first()) {
            let e = count.entry(*a).or_insert((0, k));
            e.0 += 1;
        }
    }
    let mut ranked: Vec<_> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    let mut seen = HashSet::new();
    ranked
        .into_iter()
        .filter_map(|(a, _)| tiles.artist_names.get(&a).cloned())
        .filter(|n| seen.insert(n.to_lowercase()))
        .take(12)
        .collect()
}

/// Covers of the mix's first albums, each once.
fn covers(snap: &Snapshot, tracks: &[TrackId]) -> Vec<CoverRef> {
    let mut out: Vec<CoverRef> = Vec::new();
    for r in tracks.iter().filter_map(|t| snap.catalog.get(*t).and_then(|i| i.release)) {
        let c = CoverRef::Release(r);
        if !out.contains(&c) {
            out.push(c);
            if out.len() == super::COVER_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// «С новым»: new songs the services found, in the lane's language (any
/// for the soundtracks and «На повторе»), each in one mix only.
fn new_for(snap: &Snapshot, input: &BuildInput, langs: &Langs, lane: v::Lane, songs: usize, used: &mut HashSet<String>) -> Vec<StreamTrack> {
    let want = (songs / (NEW_EVERY - 1)).max(1);
    let fits = |t: &StreamTrack| {
        let lang = langs.of_new(t);
        match lane {
            v::Lane::Russian => lang == Some(Lang::Russian),
            v::Lane::English => lang == Some(Lang::English),
            v::Lane::Japanese => lang == Some(Lang::Japanese),
            v::Lane::NotRussian => matches!(lang, Some(Lang::English | Lang::Japanese | Lang::Other)),
            v::Lane::CalmSoundtracks | v::Lane::DriveSoundtracks => {
                t.album.as_ref().is_some_and(|a| franchise(&a.title).is_some())
            }
            v::Lane::OnRepeat => true,
        }
    };
    let mut out = Vec::new();
    for t in &input.new {
        if out.len() >= want {
            break;
        }
        if snap.is_new(t) && fits(t) && used.insert(t.id.to_string()) {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_tell_the_language_whatever_the_format() {
        let lrc = "[00:12.34]Город в огне, и мы идём домой\n[00:15.00]По ночным дворам";
        assert_eq!(lyrics_lang(lrc), Some(Lang::Russian));
        let json = r#"[{"start":{"secs":0,"nanos":5},"text":"My grandpa fought in World War II","words":[]}]"#;
        assert_eq!(lyrics_lang(json), Some(Lang::English));
        let ja = "[00:01.00]君の名前を呼んでいた 夜の向こうで";
        assert_eq!(lyrics_lang(ja), Some(Lang::Japanese));
        // The JSON's own keys are no words.
        assert_eq!(lyric_words(r#"[{"text":"Да","words":[]}]"#).trim(), "Да");
        assert_eq!(lyrics_lang("[00:01.00]"), None);
    }

    fn langs_of(lyrics: &[(TrackId, &str)]) -> Langs {
        let sung = lyrics.iter().filter_map(|(t, l)| Some((*t, lyrics_lang(l)?))).collect();
        Langs { sung, artist: HashMap::new(), named: HashMap::new() }
    }

    #[test]
    fn a_latin_title_alone_is_no_english() {
        let ids: Vec<TrackId> = (0..5).map(|_| TrackId::new()).collect();
        let uzumaki = "[00:10.00]I keep on walking in the circles that I made\n[00:14.00]And every time I turn around the road is gone again\n[00:18.00]I can hear the noise inside my head";
        let langs = langs_of(&[(ids[4], uzumaki)]);
        // No lyrics: the title decides only when it says so.
        assert_eq!(langs.of(ids[0], None, "Meine Freundin ist kaputt", "Con Fetti Pesto"), None);
        assert_eq!(langs.of(ids[1], None, "There is a reason (RUS COVER)", "j.am"), Some(Lang::Russian));
        assert_eq!(langs.of(ids[2], None, "A Horny Money World 〜legendary night〜", "Houshou Marine"), Some(Lang::Japanese));
        assert_eq!(
            langs.of(ids[3], None, "Solo Leveling Season 2 -Arise from the Shadow- Opening | \"ReawakeR\" by LiSA feat Felix of Stray Kids", "Crunchyroll"),
            None
        );
        // The lyrics win over a Japanese title.
        assert_eq!(langs.of(ids[4], None, "うずまき", "Machine Girl"), Some(Lang::English));
        assert_eq!(title_lang("Gurenge ～Japanese ver.～"), Some(Lang::Japanese));
        assert_eq!(title_lang("Blinding Lights (на русском)"), Some(Lang::Russian));
    }

    #[test]
    fn lyrics_in_latin_letters_are_told_apart() {
        let de = "[00:01.00]Ich gehe durch die Straßen und denke nur an dich\n[00:05.00]Die Nacht ist kalt und meine Freundin ist nicht hier\n[00:09.00]Und alles was ich habe ist ein Lied für dich";
        let es = "[00:01.00]Camino por las calles y solo pienso en ti\n[00:05.00]La noche es fría y mi corazón no está aquí\n[00:09.00]Y todo lo que tengo es una canción para ti";
        let romaji = "[00:01.00]kimi no na wo yonde ita yoru no mukou de\n[00:05.00]boku wa mada koko ni iru yo kimi ga inakute mo\n[00:09.00]sora ga naite iru kara namida wo misenai";
        let en = "[00:01.00]I walk along the streets and only think of you\n[00:05.00]The night is cold and my heart is not here with you\n[00:09.00]And everything I have is just a song for you";
        assert_eq!(lyrics_lang(de), Some(Lang::Other));
        assert_eq!(lyrics_lang(es), Some(Lang::Other));
        assert_eq!(lyrics_lang(romaji), Some(Lang::Japanese));
        assert_eq!(lyrics_lang(en), Some(Lang::English));
        // Too few words to tell.
        assert_eq!(latin_lang("hello there my friend"), None);
        assert!(romaji_word("kimi") && romaji_word("shinjitsu") && romaji_word("kitto") && romaji_word("n"));
        assert!(!romaji_word("the") && !romaji_word("love") && !romaji_word("street"));
    }
}
