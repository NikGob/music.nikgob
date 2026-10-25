//! Главная v2 (home-v2 spec §3–§8, §15): the parts read straight from the
//! library at each visit — the shortcuts, «Слушать снова», «Из твоей
//! библиотеки» and the worlds of the card at the top. The mixes and the
//! tiles of the day come from the recommendations (`recs`).

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventContext, EventType, PlaylistId, ReleaseId, TrackId, UnixMs};
use nikgob_library::{
    ArtistRow, ArtistSortKey, Library, PlaylistSummary, RecTrack, ReleaseRow, ReleaseSortKey, Sort, TrackFactsRow,
    TrackRow, now_ms,
};
use nikgob_recs::tiles::franchise;
use nikgob_view as v;

use crate::streams::plural;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The shortcuts' plays fade by half in this time.
const HALF_LIFE_MS: f64 = 3.0 * DAY_MS as f64;
/// Shortcuts kept (the page shows six, or eight on a wide window).
pub const SHORTCUTS: usize = 8;
/// «Слушать снова»: songs kept, at most this many of one artist.
const LISTEN_AGAIN: usize = 12;
const PER_ARTIST: usize = 2;
/// A favourite comes back after this many days without a play (two weeks
/// once the history is a year long).
const AGAIN_DAYS: i64 = 3;
const AGAIN_DAYS_LONG: i64 = 14;
/// «Из твоей библиотеки»: albums kept, with at least this many tracks with
/// a file, at most half of them ever played.
const SHELF: usize = 12;
const SHELF_MIN: u32 = 5;
/// The card's row has at most this many worlds.
pub const WORLDS: usize = 6;
/// «8 ч за 2 недели».
const LISTENED_WINDOW_MS: i64 = 14 * DAY_MS;

/// A shortcut before it is drawn.
pub enum ShortcutRef {
    Release(ReleaseRow),
    Playlist(PlaylistSummary),
    Artist(ArtistRow),
}

/// The parts of the home read at a visit.
#[derive(Default)]
pub struct HomeV2 {
    pub history: bool,
    pub shortcuts: Vec<ShortcutRef>,
    pub listen_again: Vec<TrackRow>,
    pub from_library: Vec<(ReleaseRow, u32, u32)>,
    /// Without covers (the resolver adds them).
    pub worlds: Vec<v::World>,
}

/// What the parts depend on besides the library.
#[derive(Clone, Debug, Default)]
pub struct Visit {
    /// The track the player has now.
    pub playing: Option<TrackId>,
    /// «Только скачанное».
    pub local_only: bool,
    /// Worlds taken off the row.
    pub hidden_worlds: Vec<String>,
    /// Minutes east of UTC.
    pub utc_offset_min: i32,
    pub now: UnixMs,
}

impl Visit {
    pub fn now(playing: Option<TrackId>, prefs: &v::Prefs) -> Self {
        Self {
            playing,
            local_only: prefs.mixes_downloaded_only,
            hidden_worlds: prefs.home_hidden_worlds.clone(),
            utc_offset_min: jiff::Zoned::now().offset().seconds() / 60,
            now: now_ms(),
        }
    }

    /// The home's day of `ts` (it begins at 06:00).
    fn day(&self, ts: UnixMs) -> i64 {
        (ts + i64::from(self.utc_offset_min) * 60_000 - crate::recs::DAY_STARTS_HOUR * 3_600_000).div_euclid(DAY_MS)
    }
}

/// What the parts read, once.
struct Facts {
    rows: HashMap<TrackId, RecTrack>,
    facts: HashMap<TrackId, TrackFactsRow>,
    /// Start events, oldest first.
    starts: Vec<(UnixMs, TrackId, EventContext)>,
    completes: HashMap<TrackId, u32>,
    last_start: HashMap<TrackId, UnixMs>,
    liked: HashSet<TrackId>,
}

pub fn build(lib: &Library, visit: &Visit) -> HomeV2 {
    let mut events = lib.listening_events().unwrap_or_default();
    events.sort_by_key(|e| e.ts);
    let rows: HashMap<TrackId, RecTrack> = lib.rec_tracks().unwrap_or_default().into_iter().map(|r| (r.id, r)).collect();
    let facts: HashMap<TrackId, TrackFactsRow> =
        lib.track_facts().unwrap_or_default().into_iter().map(|f| (f.id, f)).collect();
    let liked: HashSet<TrackId> = lib.liked_track_ids().unwrap_or_default().into_iter().collect();
    let mut starts = Vec::new();
    let mut completes: HashMap<TrackId, u32> = HashMap::new();
    let mut last_start = HashMap::new();
    for e in &events {
        match e.event_type {
            EventType::Start => {
                starts.push((e.ts, e.track_id, e.context));
                last_start.insert(e.track_id, e.ts);
            }
            EventType::Complete => *completes.entry(e.track_id).or_default() += 1,
            _ => {}
        }
    }
    let f = Facts { rows, facts, starts, completes, last_start, liked };
    let history = !f.starts.is_empty() || !f.liked.is_empty();

    let releases: Vec<ReleaseRow> = lib.releases(0, 20_000, Sort::desc(ReleaseSortKey::Added)).unwrap_or_default();
    let worlds = worlds(lib, &f, &releases, visit);
    HomeV2 {
        history,
        shortcuts: shortcuts(lib, &f, &releases, visit, history),
        listen_again: listen_again(lib, &f, visit),
        from_library: shelf(&f, &releases, visit, history),
        worlds,
    }
}

/// The shortcuts: what the plays of the last days come from — the playlist
/// they started in, the artist page, else the album — by plays that fade
/// with a half-life of three days. Without a history: the recently added.
fn shortcuts(lib: &Library, f: &Facts, releases: &[ReleaseRow], visit: &Visit, history: bool) -> Vec<ShortcutRef> {
    let by_id: HashMap<ReleaseId, &ReleaseRow> = releases.iter().map(|r| (r.release.id, r)).collect();
    let fits = |r: &ReleaseRow| r.local_tracks > 0 || (!visit.local_only && r.known_tracks > 0);
    if !history {
        return releases.iter().filter(|r| fits(r)).take(SHORTCUTS).cloned().map(ShortcutRef::Release).collect();
    }
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Release(ReleaseId),
        Playlist(PlaylistId),
        Artist(ArtistId),
    }
    let playlists = lib.playlist_tracks().unwrap_or_default();
    let mut in_playlist: HashMap<TrackId, PlaylistId> = HashMap::new();
    for p in &playlists {
        for t in &p.tracks {
            in_playlist.entry(*t).or_insert(p.id);
        }
    }
    let mut score: HashMap<Key, f64> = HashMap::new();
    for (ts, t, ctx) in &f.starts {
        let Some(row) = f.rows.get(t) else { continue };
        let key = match ctx {
            EventContext::Playlist => in_playlist.get(t).copied().map(Key::Playlist),
            EventContext::Artist => row.artists.first().copied().map(Key::Artist),
            _ => None,
        }
        .or_else(|| row.release.map(Key::Release));
        let Some(key) = key else { continue };
        let age = (visit.now - ts).max(0) as f64;
        *score.entry(key).or_default() += 0.5_f64.powf(age / HALF_LIFE_MS);
    }
    let mut ranked: Vec<(Key, f64)> = score.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let summaries: HashMap<PlaylistId, PlaylistSummary> =
        lib.list_playlists().unwrap_or_default().into_iter().filter_map(|p| Some((p.id?, p))).collect();
    let mut artists: Option<HashMap<ArtistId, ArtistRow>> = None;
    let mut out = Vec::new();
    for (key, _) in ranked {
        if out.len() == SHORTCUTS {
            break;
        }
        match key {
            Key::Release(r) => {
                if let Some(r) = by_id.get(&r).filter(|r| fits(r)) {
                    out.push(ShortcutRef::Release((*r).clone()));
                }
            }
            Key::Playlist(p) => {
                if let Some(p) = summaries.get(&p).filter(|p| p.track_count > 0) {
                    out.push(ShortcutRef::Playlist(p.clone()));
                }
            }
            Key::Artist(a) => {
                let all = artists.get_or_insert_with(|| {
                    lib.artists(0, 20_000, Sort::asc(ArtistSortKey::Name))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| (a.artist.id, a))
                        .collect()
                });
                if let Some(a) = all.get(&a).filter(|a| a.local_track_count > 0 || !visit.local_only) {
                    out.push(ShortcutRef::Artist(a.clone()));
                }
            }
        }
    }
    out
}

/// «Слушать снова»: liked or replayed songs not played for a few days, the
/// most loved first (at night what is played at night weighs more), two
/// of one artist at most.
fn listen_again(lib: &Library, f: &Facts, visit: &Visit) -> Vec<TrackRow> {
    let first = f.starts.first().map_or(visit.now, |s| s.0);
    let days = if visit.now - first >= 365 * DAY_MS { AGAIN_DAYS_LONG } else { AGAIN_DAYS };
    let night_now = is_night(visit.now, visit.utc_offset_min);
    let mut plays: HashMap<TrackId, (u32, u32)> = HashMap::new();
    for (ts, t, _) in &f.starts {
        let e = plays.entry(*t).or_default();
        e.0 += 1;
        if is_night(*ts, visit.utc_offset_min) {
            e.1 += 1;
        }
    }
    let mut scored: Vec<(f32, TrackId)> = Vec::new();
    let candidates: HashSet<TrackId> = f.liked.iter().copied().chain(plays.keys().copied()).collect();
    for t in candidates {
        let Some(row) = f.rows.get(&t) else { continue };
        if !row.in_library || (visit.local_only && !row.has_local) {
            continue;
        }
        let (n, night) = plays.get(&t).copied().unwrap_or((0, 0));
        let done = f.completes.get(&t).copied().unwrap_or(0);
        let loved = f.liked.contains(&t);
        if !loved && n < 3 && done < 2 {
            continue;
        }
        if f.last_start.get(&t).is_some_and(|ts| visit.now - ts < days * DAY_MS) {
            continue;
        }
        let mut s = if loved { 3.0 } else { 0.0 } + (n as f32).sqrt() + 0.5 * done as f32;
        if night_now && n > 0 {
            s *= 1.0 + night as f32 / n as f32;
        }
        scored.push((s, t));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut per_artist: HashMap<Option<ArtistId>, usize> = HashMap::new();
    let mut songs = HashSet::new();
    let mut out = Vec::new();
    for (_, t) in scored {
        if out.len() == LISTEN_AGAIN {
            break;
        }
        let Some(row) = f.rows.get(&t) else { continue };
        let a = row.artists.first().copied();
        if per_artist.get(&a).copied().unwrap_or(0) >= PER_ARTIST || !songs.insert(row.song.clone()) {
            continue;
        }
        if let Ok(Some(r)) = lib.get_track_row(t) {
            *per_artist.entry(a).or_default() += 1;
            out.push(r);
        }
    }
    out
}

/// 22:00–05:59 local.
fn is_night(ts: UnixMs, utc_offset_min: i32) -> bool {
    let h = (ts + i64::from(utc_offset_min) * 60_000).rem_euclid(DAY_MS) / 3_600_000;
    !(6..22).contains(&h)
}

/// «Из твоей библиотеки»: albums with a few tracks with a file of which at
/// most half were ever played, the untouched first, other ones each day.
/// Without a history («С чего начать»): the largest albums.
fn shelf(f: &Facts, releases: &[ReleaseRow], visit: &Visit, history: bool) -> Vec<(ReleaseRow, u32, u32)> {
    let mut heard: HashMap<ReleaseId, HashSet<TrackId>> = HashMap::new();
    for (_, t, _) in &f.starts {
        if let Some(r) = f.rows.get(t).and_then(|r| r.release) {
            heard.entry(r).or_default().insert(*t);
        }
    }
    let today = visit.day(visit.now) as u64;
    let mut out: Vec<(bool, u64, ReleaseRow, u32, u32)> = releases
        .iter()
        .filter(|r| r.local_tracks >= SHELF_MIN)
        .filter_map(|r| {
            let total = r.known_tracks.max(r.local_tracks);
            let h = heard.get(&r.release.id).map_or(0, |s| s.len() as u32);
            (h * 2 <= total).then(|| {
                let spin = mix(today, &r.release.id.to_string());
                (h > 0, spin, r.clone(), h, total)
            })
        })
        .collect();
    if history {
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    } else {
        out.sort_by(|a, b| b.4.cmp(&a.4).then(a.1.cmp(&b.1)));
    }
    out.into_iter().take(SHELF).map(|(_, _, r, h, t)| (r, h, t)).collect()
}

/// A stable number of a day and a key (a shuffle that changes daily).
fn mix(day: u64, key: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ day.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for b in key.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// The album's name inside its world: «OST 4» for «MADE IN ABYSS ORIGINAL
/// SOUNDTRACK 4», «Silksong» for «Hollow Knight: Silksong (Original
/// Soundtrack)».
pub fn short_album(album: &str, world: &str) -> String {
    let mut s = album.to_owned();
    let lower = s.to_lowercase();
    let w = world.to_lowercase();
    if lower.starts_with(&w) {
        s = s.chars().skip(world.chars().count()).collect();
    }
    // «~Isekai Ittara Honki Dasu~»: a subtitle between tildes.
    for (open, close) in [('~', '~'), ('〜', '〜')] {
        if let Some(a) = s.find(open)
            && let Some(b) = s[a + open.len_utf8()..].find(close)
        {
            s.replace_range(a..a + open.len_utf8() + b + close.len_utf8(), " ");
        }
    }
    for marker in [
        "Original Motion Picture Soundtrack",
        "Motion Picture Soundtrack",
        "Original Soundtrack",
        "Soundtrack",
        "サウンドトラック",
        "OST",
    ] {
        let lower = s.to_lowercase();
        if let Some(i) = lower.find(&marker.to_lowercase()) {
            s.replace_range(i..i + marker.len(), "OST");
            break;
        }
    }
    // «(OST)» after a name is noise, and so is «Special Edition».
    let mut s = s.replace("(OST)", "").replace("[OST]", "").replace(" Special Edition", "");
    // «II: Jobless Reincarnation OST 1» → «II · OST 1».
    if let (Some(colon), Some(ost)) = (s.find(':'), s.find("OST"))
        && colon < ost
        && colon > 0
    {
        s = format!("{} · {}", s[..colon].trim(), &s[ost..]);
    }
    s = s.trim_matches(|c: char| c.is_whitespace() || ":-–—·.,(".contains(c)).to_owned();
    let words: Vec<&str> = s.split_whitespace().collect();
    // «VOLUME.02 スペシャルCD OST Vol.1»: the volume says it.
    let s = match words.first() {
        Some(w) if words.len() > 1 && !w.eq_ignore_ascii_case("ost") && w.chars().any(|c| c.is_ascii_digit()) => (*w).to_owned(),
        _ => words.join(" "),
    };
    if s.is_empty() { "OST".to_owned() } else { s }
}

/// A franchise's name without a volume after it: «ノーゲーム・ノーライフ» for
/// «ノーゲーム・ノーライフ VOLUME.02 スペシャルCD» (its volumes are one world).
fn series_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let cut = words.iter().skip(1).position(|w| w.chars().any(|c| c.is_ascii_digit())).map_or(words.len(), |i| i + 1);
    words[..cut].join(" ")
}

/// «Слушал вчера», «Слушал 4 дня назад», «Давно не слушал».
fn heard_when(visit: &Visit, last: Option<UnixMs>) -> String {
    let Some(ts) = last else { return "Ещё не начинал".to_owned() };
    let d = visit.day(visit.now) - visit.day(ts);
    match d {
        ..=0 => "Слушал сегодня".to_owned(),
        1 => "Слушал вчера".to_owned(),
        2 => "Слушал позавчера".to_owned(),
        3..=6 => format!("Слушал {d} {} назад", plural(d as usize, "день", "дня", "дней")),
        7..=9 => "Слушал неделю назад".to_owned(),
        10..=17 => "Слушал две недели назад".to_owned(),
        _ => "Давно не слушал".to_owned(),
    }
}

/// «1 ч 4 мин», «2 ч», «45 мин».
fn hours_minutes(ms: u64) -> String {
    let m = ((ms + 30_000) / 60_000).max(1);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} мин"),
        (h, 0) => format!("{h} ч"),
        (h, m) => format!("{h} ч {m} мин"),
    }
}

fn count(n: usize, one: &'static str, few: &'static str, many: &'static str) -> String {
    format!("{n} {}", plural(n, one, few, many))
}

/// The worlds: soundtrack series of the library (one franchise, by
/// `recs::tiles::franchise`; an album without a soundtrack mark that
/// starts with a franchise's name joins it), with at least one album with
/// files; by the last play, the unstarted ones last; at most [`WORLDS`].
fn worlds(lib: &Library, f: &Facts, releases: &[ReleaseRow], visit: &Visit) -> Vec<v::World> {
    struct W {
        key: String,
        title: String,
        albums: Vec<ReleaseRow>,
    }
    let mut groups: Vec<W> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut rest: Vec<&ReleaseRow> = Vec::new();
    let mut sorted: Vec<&ReleaseRow> = releases.iter().filter(|r| r.local_tracks > 0 || !visit.local_only).collect();
    sorted.sort_by(|a, b| a.release.year.cmp(&b.release.year).then_with(|| a.release.title.cmp(&b.release.title)));
    for r in sorted {
        match franchise(&r.release.title).map(|(_, t)| series_name(&t)) {
            Some(title) => {
                let key = title.to_lowercase();
                let i = *index.entry(key.clone()).or_insert_with(|| {
                    groups.push(W { key, title, albums: Vec::new() });
                    groups.len() - 1
                });
                groups[i].albums.push(r.clone());
            }
            None => rest.push(r),
        }
    }
    for r in rest {
        let lower = r.release.title.to_lowercase();
        if let Some(g) = groups.iter_mut().find(|g| {
            lower.strip_prefix(&g.key).is_some_and(|tail| tail.starts_with([':', ' ', '-', '(']))
                && g.albums.iter().any(|a| a.release.artist_credit.artist_ids().next() == r.release.artist_credit.artist_ids().next())
        }) {
            g.albums.push(r.clone());
            g.albums.sort_by(|a, b| a.release.year.cmp(&b.release.year).then_with(|| a.release.title.cmp(&b.release.title)));
        }
    }
    groups.retain(|g| g.albums.iter().any(|a| a.local_tracks > 0) && !visit.hidden_worlds.contains(&g.key));

    // The tracks of each album in order.
    let mut tracks_of: HashMap<ReleaseId, Vec<(u32, TrackId)>> = HashMap::new();
    for (id, fact) in &f.facts {
        if let Some(row) = f.rows.get(id)
            && row.in_library
            && let Some(r) = row.release
        {
            tracks_of.entry(r).or_default().push((fact.disc_no.saturating_mul(1000).saturating_add(fact.track_no), *id));
        }
    }
    for list in tracks_of.values_mut() {
        list.sort();
    }
    let titles: HashMap<TrackId, &str> = f.rows.iter().map(|(id, r)| (*id, r.title.as_str())).collect();
    let duration = |t: &TrackId| u64::from(f.facts.get(t).and_then(|x| x.duration_ms).unwrap_or(0));
    let local = |t: &TrackId| f.rows.get(t).is_some_and(|r| r.has_local);
    // Listened time: each start until the next one, at most the track.
    let mut listened: HashMap<TrackId, u64> = HashMap::new();
    for (k, (ts, t, _)) in f.starts.iter().enumerate() {
        if visit.now - ts > LISTENED_WINDOW_MS {
            continue;
        }
        let gap = f.starts.get(k + 1).map_or(u64::MAX, |n| (n.0 - ts).max(0) as u64);
        *listened.entry(*t).or_default() += gap.min(duration(t));
    }
    let playing_release = visit.playing.and_then(|t| f.rows.get(&t)).and_then(|r| r.release);

    let mut out: Vec<(Option<UnixMs>, usize, v::World)> = Vec::new();
    for g in groups {
        let order: Vec<(ReleaseId, usize, TrackId)> = g
            .albums
            .iter()
            .flat_map(|a| {
                let list = tracks_of.get(&a.release.id).map(Vec::as_slice).unwrap_or(&[]);
                list.iter().enumerate().map(|(k, (_, t))| (a.release.id, k, *t)).collect::<Vec<_>>()
            })
            .collect();
        if order.is_empty() {
            continue;
        }
        let heard: HashSet<TrackId> = order.iter().map(|o| o.2).filter(|t| f.last_start.contains_key(t)).collect();
        let last = order.iter().filter_map(|o| f.last_start.get(&o.2)).max().copied();
        let n = order.len();
        let numbered = g.albums.iter().any(|a| {
            let s = short_album(&a.release.title, &g.title);
            s.starts_with("OST ") && s[4..].starts_with(|c: char| c.is_ascii_digit())
        });
        let short = |r: ReleaseId| {
            let s = g.albums.iter().find(|a| a.release.id == r).map_or_else(String::new, |a| short_album(&a.release.title, &g.title));
            if numbered && s == "OST" { "OST 1".to_owned() } else { s }
        };
        // The stage: the album playing, else the last played, else the first.
        let last_album = order
            .iter()
            .filter_map(|o| f.last_start.get(&o.2).map(|ts| (*ts, o.0)))
            .max_by_key(|x| x.0)
            .map(|x| x.1);
        let playing = playing_release.is_some_and(|p| g.albums.iter().any(|a| a.release.id == p));
        let stage = if playing { playing_release } else { last_album.or_else(|| g.albums.first().map(|a| a.release.id)) };

        let mut artists: Vec<String> = Vec::new();
        for a in &g.albums {
            let name = a.release.artist_credit.display();
            let words = |n: &str| {
                let mut w: Vec<String> = n.to_lowercase().split_whitespace().map(str::to_owned).collect();
                w.sort();
                w
            };
            if !name.is_empty() && !artists.iter().any(|a| words(a) == words(&name)) && name != "Various Artists" {
                artists.push(name);
            }
        }
        artists.truncate(2);
        let mut facts = vec![artists.join(", ")];
        let k = g.albums.len();
        if k > 1 {
            facts.push(count(k, "альбом", "альбома", "альбомов"));
        }
        facts.push(count(n, "трек", "трека", "треков"));
        let all_ms: u64 = order.iter().map(|o| duration(&o.2)).sum();
        if heard.is_empty() {
            facts.push(hours_minutes(all_ms));
        } else {
            facts.push(format!("ты слышал {}", heard.len()));
        }
        let hours = order.iter().filter_map(|o| listened.get(&o.2)).sum::<u64>() as f64 / 3_600_000.0;
        if hours >= 1.0 {
            facts.push(format!("{} ч за 2 недели", hours.round() as u64));
        }
        facts.retain(|x| !x.is_empty());

        let fresh = heard.is_empty();
        let unheard: Vec<TrackId> = order.iter().map(|o| o.2).filter(|t| !heard.contains(t) && local(t)).collect();
        let everything: Vec<TrackId> = order.iter().map(|o| o.2).filter(|t| local(t) || !visit.local_only).collect();
        let primary = if fresh { everything.clone() } else { unheard.clone() };
        let primary_ms: u64 = primary.iter().map(duration).sum();
        let primary_tip = format!(
            "{}, {}: {}",
            count(primary.len(), "трек", "трека", "треков"),
            hours_minutes(primary_ms),
            if fresh { "с первого альбома" } else { "по порядку альбомов" }
        );
        let mut shuffled = everything.clone();
        fastrand::shuffle(&mut shuffled);

        // The three cards: never what the stage's buttons do.
        let mut cards: Vec<v::WorldCard> = Vec::new();
        let from = |r: ReleaseId, k: usize| -> Vec<TrackId> {
            tracks_of.get(&r).map_or_else(Vec::new, |l| l.iter().skip(k).map(|x| x.1).filter(|t| local(t) || !visit.local_only).collect())
        };
        if playing && let Some(t) = visit.playing {
            cards.push(v::WorldCard {
                icon: v::WorldIcon::NowPlaying,
                kicker: "Сейчас играет".into(),
                title: titles.get(&t).copied().unwrap_or_default().to_owned(),
                go: v::WorldGo::Player,
            });
        } else if let Some(r) = last_album {
            // The track after the last one started on that album.
            let list = tracks_of.get(&r).map(Vec::as_slice).unwrap_or(&[]);
            let last_k = list
                .iter()
                .enumerate()
                .filter_map(|(k, x)| f.last_start.get(&x.1).map(|ts| (*ts, k)))
                .max()
                .map_or(0, |x| x.1);
            let next = if last_k + 1 < list.len() { last_k + 1 } else { 0 };
            cards.push(v::WorldCard {
                icon: v::WorldIcon::Play,
                kicker: "Продолжить".into(),
                title: format!("{}, трек {}", short(r), next + 1),
                go: v::WorldGo::Play(from(r, next)),
            });
        } else if let Some(a) = g.albums.first() {
            cards.push(v::WorldCard {
                icon: v::WorldIcon::Play,
                kicker: "С начала".into(),
                title: format!("{}, трек 1", short(a.release.id)),
                go: v::WorldGo::Play(from(a.release.id, 0)),
            });
        }
        if !fresh && let Some(o) = order.iter().find(|o| !heard.contains(&o.2) && local(&o.2)) {
            cards.push(v::WorldCard {
                icon: v::WorldIcon::Next,
                kicker: "По порядку".into(),
                title: format!("{}, трек {}", short(o.0), o.1 + 1),
                go: v::WorldGo::Play(from(o.0, o.1)),
            });
        }
        let untouched = g
            .albums
            .iter()
            .find(|a| a.local_tracks > 0 && tracks_of.get(&a.release.id).is_none_or(|l| l.iter().all(|x| !heard.contains(&x.1))));
        let liked: Vec<TrackId> = order.iter().map(|o| o.2).filter(|t| f.liked.contains(t)).collect();
        let newest = g.albums.iter().max_by_key(|a| (a.release.year, a.release.added_at));
        let mut third: Vec<v::WorldCard> = Vec::new();
        if !fresh && let Some(a) = untouched {
            third.push(v::WorldCard {
                icon: v::WorldIcon::Album,
                kicker: "Ни разу не слушал".into(),
                title: short(a.release.id),
                go: v::WorldGo::Release(a.release.id),
            });
        }
        if liked.len() >= 3 {
            third.push(v::WorldCard {
                icon: v::WorldIcon::Favorite,
                kicker: "Любимое".into(),
                title: count(liked.len(), "трек", "трека", "треков"),
                go: v::WorldGo::Play(liked.clone()),
            });
        }
        if k > 1 && let Some(a) = newest {
            third.push(v::WorldCard {
                icon: v::WorldIcon::Album,
                kicker: a.release.year.map_or_else(|| "Новее всего".to_owned(), |y| format!("Новее всего, {y}")),
                title: short(a.release.id),
                go: v::WorldGo::Release(a.release.id),
            });
        }
        for c in third {
            if cards.len() >= 3 {
                break;
            }
            if !cards.iter().any(|x| x.title == c.title) {
                cards.push(c);
            }
        }

        let kicker = if playing { "Слушаешь сейчас".to_owned() } else { heard_when(visit, last) };
        out.push((
            last,
            n,
            v::World {
                key: g.key,
                title: g.title,
                release: stage,
                cover: None,
                kicker,
                facts: facts.join(" · "),
                fresh,
                playing,
                primary,
                primary_tip,
                shuffled,
                albums: g.albums.iter().map(|a| a.release.id).collect(),
                cards,
            },
        ));
    }
    // By the last play; the unstarted last, the largest of them first.
    out.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.1.cmp(&a.1),
    });
    let _ = lib;
    out.into_iter().take(WORLDS).map(|x| x.2).collect()
}

/// A new album of the world among the services' new releases: «Новый
/// альбом» for its third card (the release then leaves «Новые релизы» while
/// the card shows it).
pub fn world_release(world: &v::World, releases: &[v::StreamAlbumCard]) -> Option<usize> {
    releases.iter().position(|a| franchise(&a.title).is_some_and(|(k, _)| k == world.key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn albums_are_named_inside_their_world() {
        assert_eq!(short_album("MADE IN ABYSS ORIGINAL SOUNDTRACK 4", "MADE IN ABYSS"), "OST 4");
        assert_eq!(short_album("Hollow Knight: Silksong (Original Soundtrack)", "Hollow Knight"), "Silksong");
        assert_eq!(short_album("Hollow Knight: Original Soundtrack", "Hollow Knight"), "OST");
        assert_eq!(short_album("Hollow Knight: Gods & Nightmares", "Hollow Knight"), "Gods & Nightmares");
        assert_eq!(short_album("OMORI OST", "OMORI"), "OST");
        assert_eq!(short_album("ノーゲーム・ノーライフ VOLUME.02 スペシャルCD サウンドトラック Vol.1", "ノーゲーム・ノーライフ"), "VOLUME.02");
        assert_eq!(
            short_album("Mushoku Tensei II: Jobless Reincarnation Original Soundtrack 1 Special Edition", "Mushoku Tensei"),
            "II · OST 1"
        );
        assert_eq!(series_name("ノーゲーム・ノーライフ VOLUME.02 スペシャルCD"), "ノーゲーム・ノーライフ");
        assert_eq!(series_name("MADE IN ABYSS"), "MADE IN ABYSS");
        assert_eq!(
            short_album("Mushoku Tensei ~Isekai Ittara Honki Dasu~ Original Soundtrack 2 Special Edition", "Mushoku Tensei"),
            "OST 2"
        );
    }

    #[test]
    fn the_kicker_says_when_with_the_day_ending_at_six() {
        let visit = Visit { utc_offset_min: 0, now: 20_000 * DAY_MS + 2 * 3_600_000, ..Visit::default() };
        // 02:00: 23:00 yesterday is still today's night.
        assert_eq!(heard_when(&visit, Some(20_000 * DAY_MS - 3_600_000)), "Слушал сегодня");
        assert_eq!(heard_when(&visit, Some(20_000 * DAY_MS - 20 * 3_600_000)), "Слушал вчера");
        assert_eq!(heard_when(&visit, Some(20_000 * DAY_MS - 4 * DAY_MS)), "Слушал 4 дня назад");
        assert_eq!(heard_when(&visit, Some(20_000 * DAY_MS - 8 * DAY_MS)), "Слушал неделю назад");
        assert_eq!(heard_when(&visit, None), "Ещё не начинал");
        assert_eq!(hours_minutes(64 * 60_000), "1 ч 4 мин");
        assert_eq!(hours_minutes(120 * 60_000), "2 ч");
    }
}
