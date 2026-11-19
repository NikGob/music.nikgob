//! `POST /v1/player/play`: what a spoken query means. My Wave,
//! Liked Songs and the part-of-day mixes (in Russian and English) are
//! found by their words; anything
//! else is searched in the library and the best match wins, ties going to
//! a playlist, then an artist, an album, a song.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use nikgob_core::model::{ArtistId, PlaylistId, ReleaseId, TrackId};
use nikgob_library::{Library, SearchResults};
use nikgob_view::{Command, LIKED_PLAYLIST};

use crate::error::ApiError;

/// What to play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Any,
    Track,
    Album,
    Artist,
    Playlist,
    Liked,
    Wave,
    /// The mix of a part of the day (“Good morning”, “Day”, “Evening”,
    /// “Night”): its id is the part, 0–3 from the morning.
    Part,
}

/// Where it goes in the queue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The queue is replaced and it plays at once.
    #[default]
    Replace,
    /// After the current track.
    Next,
    /// At the end of the queue.
    Enqueue,
}

/// What was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct Matched {
    pub kind: Kind,
    /// `null` for My Wave and Liked Songs.
    pub id: Option<String>,
    pub title: String,
}

/// One thing the search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub kind: Kind,
    pub id: String,
    pub title: String,
    /// Its artist line (empty for a playlist or an artist).
    pub artist: String,
    /// Its place in its category of the search (best first).
    pub order: usize,
}

/// Lowercase, `ё` as `е`, letters and digits only, one space between words.
pub fn normalize(s: &str) -> String {
    let mapped: String = s
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words a spoken request may start with that are no part of what to play.
const LEADING: [&str; 12] =
    ["включи", "включить", "поставь", "поставить", "сыграй", "играй", "запусти", "давай", "play", "put on", "start", "please"];

/// The query without a leading ask-word (its Russian words or "play").
fn bare(q: &str) -> String {
    let mut q = normalize(q);
    loop {
        let before = q.clone();
        for w in LEADING {
            if let Some(rest) = q.strip_prefix(w)
                && (rest.is_empty() || rest.starts_with(' '))
            {
                q = rest.trim_start().to_owned();
            }
        }
        if let Some(rest) = q.strip_suffix(" пожалуйста") {
            q = rest.to_owned();
        }
        if q == before {
            return q;
        }
    }
}

/// My Wave or Liked Songs by their words, Russian or English.
pub fn special(query: &str) -> Option<Kind> {
    const WAVE: [&str; 8] = ["моя волна", "мою волну", "волна", "волну", "моя волна пожалуйста", "my wave", "wave", "the wave"];
    const LIKED: [&str; 16] = [
        "любимые", "любимое", "любимые песни", "мои любимые", "мои любимые песни", "лайкнутые", "лайкнутые песни", "лайки", "избранное",
        "избранные", "liked", "liked songs", "my likes", "favorites", "favourites", "my favorites",
    ];
    let q = bare(query);
    if WAVE.contains(&q.as_str()) {
        Some(Kind::Wave)
    } else if LIKED.contains(&q.as_str()) {
        Some(Kind::Liked)
    } else if special_part(query).is_some() {
        Some(Kind::Part)
    } else {
        None
    }
}

/// The mixes of the parts of the day by their names, 0–3 from the morning.
pub const PARTS: [&str; 4] = ["Good morning", "Day", "Evening", "Night"];

/// The part of the day a spoken query asks the mix of (its Russian
/// “good morning” words), 0–3 from the morning. A bare “night” or “evening” is
/// a title as well (an album, a song), so only the mix's own words count.
pub fn special_part(query: &str) -> Option<u8> {
    const WORDS: [&[&str]; 4] = [
        &["доброе утро", "утренний микс", "утренняя музыка", "микс доброе утро", "good morning", "morning mix"],
        &["дневной микс", "дневная музыка", "микс на день", "day mix", "daytime mix"],
        &["вечерний микс", "вечерняя музыка", "микс на вечер", "evening mix"],
        &["ночной микс", "ночная музыка", "микс на ночь", "night mix"],
    ];
    let q = bare(query);
    WORDS.iter().position(|w| w.contains(&q.as_str())).map(|p| p as u8)
}

/// The part a query names when the part is asked for already: the mix's
/// words, a part's own name (“morning”, “evening”, “night”, or their Russian
/// words) or its number.
fn part_of(query: &str) -> Option<u8> {
    if let Some(p) = special_part(query) {
        return Some(p);
    }
    const NAMES: [&[&str]; 4] = [
        &["утро", "утром", "morning", "0"],
        &["день", "днем", "day", "daytime", "1"],
        &["вечер", "вечером", "evening", "2"],
        &["ночь", "ночью", "night", "3"],
    ];
    let q = bare(query);
    let q: &str = q.strip_suffix(" микс").unwrap_or(q.as_str());
    NAMES.iter().position(|w| w.contains(&q)).map(|p| p as u8)
}

fn part(p: u8) -> Target {
    Target { kind: Kind::Part, id: Some(p.to_string()), title: PARTS[usize::from(p)].to_owned() }
}

/// A part asked for that is none of the four.
fn no_such_part() -> ApiError {
    ApiError::bad_request("Part of the day: morning, day, evening or night")
}

/// How well `c` answers the normalized query `q` (higher is better).
fn closeness(q: &str, c: &Candidate) -> u32 {
    let t = normalize(&c.title);
    let a = normalize(&c.artist);
    let at_word = |hay: &str| hay == q || hay.starts_with(&format!("{q} ")) || hay.contains(&format!(" {q} ")) || hay.ends_with(&format!(" {q}"));
    if t == q {
        100
    } else if !a.is_empty() && (q == format!("{a} {t}") || q == format!("{t} {a}")) {
        95
    } else if t.starts_with(q) && t[q.len()..].starts_with(' ') {
        70
    } else if at_word(&t) {
        60
    } else if !a.is_empty() && q.contains(&t) && q.contains(&a) {
        55
    } else {
        30
    }
}

/// Ties: a playlist, then an artist, an album, a song.
fn kind_rank(k: Kind) -> u32 {
    match k {
        Kind::Playlist => 4,
        Kind::Artist => 3,
        Kind::Album => 2,
        Kind::Track => 1,
        _ => 0,
    }
}

/// The best of `candidates` for `query` (of `kind` only, unless `Any`).
pub fn best<'a>(query: &str, kind: Kind, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    let q = bare(query);
    if q.is_empty() {
        return None;
    }
    candidates
        .iter()
        .filter(|c| kind == Kind::Any || c.kind == kind)
        .max_by(|x, y| {
            let key = |c: &Candidate| (closeness(&q, c), kind_rank(c.kind), std::cmp::Reverse(c.order));
            key(x).cmp(&key(y))
        })
}

/// The library's answer as candidates: what can be played only (a song
/// with a source, an album or an artist with songs, a playlist with
/// entries).
pub fn candidates(r: &SearchResults) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (i, p) in r.playlists.iter().enumerate() {
        if let Some(id) = p.id.filter(|_| p.track_count > 0) {
            out.push(Candidate { kind: Kind::Playlist, id: id.to_string(), title: p.name.clone(), artist: String::new(), order: i });
        }
    }
    for (i, a) in r.artists.iter().enumerate() {
        if a.release_count > 0 || a.local_track_count > 0 {
            out.push(Candidate { kind: Kind::Artist, id: a.artist.id.to_string(), title: a.artist.name.clone(), artist: String::new(), order: i });
        }
    }
    for (i, rel) in r.releases.iter().enumerate() {
        if rel.known_tracks > 0 || rel.local_tracks > 0 {
            out.push(Candidate {
                kind: Kind::Album,
                id: rel.release.id.to_string(),
                title: rel.release.title.clone(),
                artist: rel.release.artist_credit.display(),
                order: i,
            });
        }
    }
    for (i, t) in r.tracks.iter().enumerate() {
        if t.active_kind.is_some() {
            out.push(Candidate { kind: Kind::Track, id: t.id.to_string(), title: t.title.clone(), artist: t.artist.clone(), order: i });
        }
    }
    out
}

/// What a query or an id resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    pub id: Option<String>,
    pub title: String,
}

impl Target {
    pub fn matched(&self) -> Matched {
        Matched { kind: self.kind, id: self.id.clone(), title: self.title.clone() }
    }
}

fn liked() -> Target {
    Target { kind: Kind::Liked, id: None, title: "Liked Songs".to_owned() }
}

fn wave() -> Target {
    Target { kind: Kind::Wave, id: None, title: "My Wave".to_owned() }
}

/// The library's thing of `kind` with `id` (blocking: call off the
/// server's threads).
pub fn exact(lib: &Library, kind: Kind, id: &str) -> Result<Target, ApiError> {
    let bad = || ApiError::bad_request("Invalid id");
    let gone = || ApiError::not_found("Not found");
    let db = |e: nikgob_library::LibraryError| ApiError::internal(e.to_string());
    match kind {
        Kind::Liked => Ok(liked()),
        Kind::Wave => Ok(wave()),
        Kind::Part => part_of(id).map(part).ok_or_else(no_such_part),
        Kind::Any => Err(ApiError::bad_request("With an id, a kind is required: track, album, artist or playlist")),
        Kind::Track => {
            let id: TrackId = id.parse().map_err(|_| bad())?;
            let t = lib.get_track_row(id).map_err(db)?.ok_or_else(gone)?;
            Ok(Target { kind, id: Some(t.id.to_string()), title: t.title })
        }
        Kind::Album => {
            let id: ReleaseId = id.parse().map_err(|_| bad())?;
            let r = lib.get_release(id).map_err(db)?.ok_or_else(gone)?;
            Ok(Target { kind, id: Some(r.id.to_string()), title: r.title })
        }
        Kind::Artist => {
            let id: ArtistId = id.parse().map_err(|_| bad())?;
            let a = lib.get_artist(id).map_err(db)?.ok_or_else(gone)?;
            Ok(Target { kind, id: Some(a.id.to_string()), title: a.name })
        }
        Kind::Playlist => {
            if id == "liked" || id == LIKED_PLAYLIST.to_string() {
                return Ok(liked());
            }
            let id: PlaylistId = id.parse().map_err(|_| bad())?;
            let p = lib.get_playlist(id).map_err(db)?.ok_or_else(gone)?;
            Ok(Target { kind, id: Some(p.id.to_string()), title: p.name })
        }
    }
}

/// What `query` means: My Wave or Liked Songs by their words, anything else
/// searched (blocking: call off the server's threads).
pub fn find(lib: &Library, query: &str, kind: Kind) -> Result<Target, ApiError> {
    match kind {
        Kind::Liked => return Ok(liked()),
        Kind::Wave => return Ok(wave()),
        Kind::Part => return part_of(query).map(part).ok_or_else(no_such_part),
        Kind::Any => match special(query) {
            Some(Kind::Liked) => return Ok(liked()),
            Some(Kind::Wave) => return Ok(wave()),
            Some(Kind::Part) => {
                if let Some(p) = special_part(query) {
                    return Ok(part(p));
                }
            }
            _ => {}
        },
        _ => {}
    }
    let q = bare(query);
    let found = lib.search(&q, 10).map_err(|e| ApiError::internal(e.to_string()))?;
    let all = candidates(&found);
    let c = best(&q, kind, &all).ok_or_else(|| ApiError::not_found("Not found"))?;
    Ok(Target { kind: c.kind, id: Some(c.id.clone()), title: c.title.clone() })
}

/// Shuffled or not when the request does not say: as the Play button does,
/// an artist and Liked Songs shuffled, the rest in order.
pub fn shuffled(kind: Kind, asked: Option<bool>) -> bool {
    asked.unwrap_or(matches!(kind, Kind::Artist | Kind::Liked))
}

/// The wave asked to go after the current track or at the end: it can only
/// replace the queue.
pub fn wave_not_queued() -> ApiError {
    ApiError::bad_request("The Wave can only replace the queue")
}

/// The commands that play `t`. A new queue (`replace`) gets `shuffle`
/// before it is made; the wave only replaces the queue (and keeps its own
/// order).
pub fn commands(t: &Target, shuffle: bool, mode: Mode) -> Result<Vec<Command>, ApiError> {
    let bad = || ApiError::bad_request("Invalid id");
    let id = t.id.as_deref().unwrap_or_default();
    let c = match (t.kind, mode) {
        (Kind::Wave, Mode::Replace) => return Ok(vec![Command::PlayWave { preset: None }]),
        (Kind::Wave, _) => return Err(wave_not_queued()),
        // The part's mix as My Wave of that part: the card of a part
        // is on the home only while it lasts, the wave of any part is.
        (Kind::Part, Mode::Replace) => {
            let p: u8 = id.parse().ok().filter(|p| usize::from(*p) < PARTS.len()).ok_or_else(no_such_part)?;
            return Ok(vec![Command::PlayWave { preset: Some(nikgob_view::MixKey::Part(p)) }]);
        }
        (Kind::Part, _) => return Err(wave_not_queued()),
        (Kind::Liked, Mode::Replace) => Command::PlayPlaylist { id: LIKED_PLAYLIST, start: None, shuffle },
        (Kind::Liked, Mode::Next) => Command::PlayPlaylistNext(LIKED_PLAYLIST),
        (Kind::Liked, Mode::Enqueue) => Command::EnqueuePlaylist(LIKED_PLAYLIST),
        (Kind::Track, m) => {
            let id: TrackId = id.parse().map_err(|_| bad())?;
            match m {
                Mode::Replace => Command::PlayTracks { tracks: vec![id], start: 0, context: nikgob_view::QueueOrigin::Search { service: None } },
                Mode::Next => Command::PlayNext(vec![id]),
                Mode::Enqueue => Command::Enqueue(vec![id]),
            }
        }
        (Kind::Album, m) => {
            let id: ReleaseId = id.parse().map_err(|_| bad())?;
            match m {
                Mode::Replace => Command::PlayRelease { id, start: None, shuffle },
                Mode::Next => Command::PlayReleaseNext(id),
                Mode::Enqueue => Command::EnqueueRelease(id),
            }
        }
        (Kind::Artist, m) => {
            let id: ArtistId = id.parse().map_err(|_| bad())?;
            match m {
                Mode::Replace => Command::PlayArtist { id, shuffle },
                Mode::Next => Command::PlayArtistNext(id),
                Mode::Enqueue => Command::EnqueueArtist(id),
            }
        }
        (Kind::Playlist, m) => {
            let id: PlaylistId = id.parse().map_err(|_| bad())?;
            match m {
                Mode::Replace => Command::PlayPlaylist { id, start: None, shuffle },
                Mode::Next => Command::PlayPlaylistNext(id),
                Mode::Enqueue => Command::EnqueuePlaylist(id),
            }
        }
        (Kind::Any, _) => return Err(ApiError::bad_request("Nothing to play is given")),
    };
    Ok(if mode == Mode::Replace { vec![Command::ApiShuffle(shuffle), c] } else { vec![c] })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(kind: Kind, title: &str, artist: &str, order: usize) -> Candidate {
        Candidate { kind, id: format!("{kind:?}-{title}"), title: title.into(), artist: artist.into(), order }
    }

    #[test]
    fn the_wave_and_liked_by_their_words() {
        for q in ["моя волна", "Моя волна!", "включи мою волну", "Волна", "my wave", "Play my wave", "поставь волну пожалуйста"] {
            assert_eq!(special(q), Some(Kind::Wave), "{q}");
        }
        for q in ["любимые", "Лайкнутые", "избранное", "включи любимые песни", "liked", "Liked songs", "my favorites"] {
            assert_eq!(special(q), Some(Kind::Liked), "{q}");
        }
        for q in ["radiohead", "волна за волной", "любимые песни на ночь", ""] {
            assert_eq!(special(q), None, "{q}");
        }
    }

    #[test]
    fn the_mix_of_a_part_of_the_day_by_its_words() {
        for (q, p) in [("Доброе утро", 0), ("включи утренний микс", 0), ("поставь дневной микс", 1), ("Вечерний микс!", 2), ("включи ночной микс", 3)] {
            assert_eq!(special_part(q), Some(p), "{q}");
            assert_eq!(special(q), Some(Kind::Part), "{q}");
        }
        // A bare “night” may be an album: searched as anything else.
        for q in ["ночь", "вечер", "день", "звезда по имени солнце"] {
            assert_eq!(special_part(q), None, "{q}");
        }
        // With the part asked for, its name or number is enough.
        for (q, p) in [("утро", 0), ("Ночь", 3), ("вечер", 2), ("1", 1), ("evening", 2), ("ночной микс", 3)] {
            assert_eq!(part_of(q), Some(p), "{q}");
        }
        assert_eq!(part_of("полдник"), None);
        let t = part(0);
        assert_eq!((t.kind, t.id.as_deref(), t.title.as_str()), (Kind::Part, Some("0"), "Good morning"));
        assert!(matches!(
            commands(&t, false, Mode::Replace).unwrap()[..],
            [Command::PlayWave { preset: Some(nikgob_view::MixKey::Part(0)) }]
        ));
        assert_eq!(commands(&t, false, Mode::Next).unwrap_err().code, "bad_request");
        let broken = Target { kind: Kind::Part, id: Some("7".into()), title: String::new() };
        assert_eq!(commands(&broken, false, Mode::Replace).unwrap_err().code, "bad_request");
    }

    #[test]
    fn an_exact_title_wins_and_ties_go_playlist_artist_album_song() {
        let all = [
            c(Kind::Track, "Radiohead", "Someone", 0),
            c(Kind::Album, "Radiohead", "Various", 0),
            c(Kind::Artist, "Radiohead", "", 0),
            c(Kind::Playlist, "Radiohead", "", 0),
        ];
        assert_eq!(best("Radiohead", Kind::Any, &all).map(|c| c.kind), Some(Kind::Playlist));
        assert_eq!(best("radiohead", Kind::Any, &all[..3]).map(|c| c.kind), Some(Kind::Artist));
        assert_eq!(best("radiohead", Kind::Any, &all[..2]).map(|c| c.kind), Some(Kind::Album));
        assert_eq!(best("radiohead", Kind::Track, &all).map(|c| c.kind), Some(Kind::Track));

        // An exact song beats a playlist that only starts with the words.
        let all = [c(Kind::Playlist, "Creep covers", "", 0), c(Kind::Track, "Creep", "Radiohead", 3)];
        assert_eq!(best("creep", Kind::Any, &all).map(|c| c.kind), Some(Kind::Track));
        // An “artist song” names the song.
        let all = [c(Kind::Artist, "Radiohead", "", 0), c(Kind::Track, "Creep", "Radiohead", 0), c(Kind::Album, "Pablo Honey", "Radiohead", 0)];
        assert_eq!(best("radiohead creep", Kind::Any, &all).map(|c| c.title.as_str()), Some("Creep"));
        assert_eq!(best("включи Radiohead", Kind::Any, &all).map(|c| c.kind), Some(Kind::Artist));
        // ё and е, case, punctuation.
        let all = [c(Kind::Album, "Звёзды", "Кино", 1), c(Kind::Track, "Звезда по имени Солнце", "Кино", 0)];
        assert_eq!(best("звезды", Kind::Any, &all).map(|c| c.kind), Some(Kind::Album));
        // Equal otherwise: the search's own order.
        let all = [c(Kind::Track, "Song A", "", 1), c(Kind::Track, "Song B", "", 0)];
        assert_eq!(best("song", Kind::Any, &all).map(|c| c.title.as_str()), Some("Song B"));
        assert!(best("", Kind::Any, &all).is_none());
        assert!(best("x", Kind::Album, &all).is_none());
    }

    #[test]
    fn what_plays_and_where() {
        let t = Target { kind: Kind::Artist, id: Some(ArtistId::new().to_string()), title: "R".into() };
        assert!(matches!(
            commands(&t, true, Mode::Replace).unwrap()[..],
            [Command::ApiShuffle(true), Command::PlayArtist { shuffle: true, .. }]
        ));
        assert!(matches!(commands(&t, false, Mode::Next).unwrap()[..], [Command::PlayArtistNext(_)]));
        assert!(matches!(commands(&wave(), false, Mode::Replace).unwrap()[..], [Command::PlayWave { preset: None }]));
        assert_eq!(commands(&wave(), false, Mode::Enqueue).unwrap_err().code, "bad_request");
        assert_eq!(commands(&wave(), false, Mode::Next).unwrap_err().code, "bad_request");
        assert!(matches!(
            commands(&liked(), true, Mode::Replace).unwrap()[..],
            [Command::ApiShuffle(true), Command::PlayPlaylist { id, start: None, shuffle: true }] if id == LIKED_PLAYLIST
        ));
        let track = Target { kind: Kind::Track, id: Some(TrackId::new().to_string()), title: "S".into() };
        assert!(matches!(commands(&track, false, Mode::Enqueue).unwrap()[..], [Command::Enqueue(_)]));
        // A single song replacing the queue is in order unless asked: the
        // shuffle of the queue before does not stay.
        assert!(matches!(
            commands(&track, false, Mode::Replace).unwrap()[..],
            [Command::ApiShuffle(false), Command::PlayTracks { start: 0, .. }]
        ));
        let album = Target { kind: Kind::Album, id: Some(ReleaseId::new().to_string()), title: "A".into() };
        assert!(matches!(
            commands(&album, shuffled(Kind::Album, None), Mode::Replace).unwrap()[..],
            [Command::ApiShuffle(false), Command::PlayRelease { shuffle: false, .. }]
        ));
        let broken = Target { kind: Kind::Album, id: Some("nope".into()), title: String::new() };
        assert_eq!(commands(&broken, false, Mode::Replace).unwrap_err().code, "bad_request");
        assert!(shuffled(Kind::Artist, None) && shuffled(Kind::Liked, None));
        assert!(!shuffled(Kind::Album, None) && !shuffled(Kind::Artist, Some(false)) && shuffled(Kind::Track, Some(true)));
    }
}
