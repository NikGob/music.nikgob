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
use nikgob_recs::rng::Rng;
use nikgob_recs::tiles::franchise;
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use crate::streams::plural;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The shortcuts' plays of now fade by half in this time…
const HALF_LIFE_MS: f64 = 3.0 * DAY_MS as f64;
/// …and a place is played now from this score on (one play six days ago).
const NOW_MIN: f64 = 0.25;
/// Shortcuts kept (the page shows six, or eight on a wide window).
pub const SHORTCUTS: usize = 8;
/// What each place of the shortcuts is drawn from: two of what plays now,
/// two forgotten favourites, two the library offers by the taste, then one
/// more of the first two kinds for a wide window.
const SHORTCUT_SLOTS: [Draw; SHORTCUTS] = [Draw::Now, Draw::Now, Draw::Forgotten, Draw::Forgotten, Draw::Offered, Draw::Offered, Draw::Now, Draw::Forgotten];
/// The narrow page's shortcuts (shuffled among themselves).
const SHORTCUTS_NARROW: usize = 6;
/// A forgotten favourite: this many plays (a like counts two) and no play
/// for a quarter of the history, three days to three weeks.
const FORGOTTEN_MIN: f64 = 3.0;
const FORGOTTEN_DAYS: (i64, i64) = (3, 21);
/// What the library offers: an artist with this many tracks there or an
/// album with this many, at most a third of it heard; neither played much.
const OFFER_ARTIST_TRACKS: u32 = 3;
const OFFER_ALBUM_TRACKS: u32 = 4;
const OFFER_PLAYS: u32 = 5;
/// One artist's places among the shortcuts at most.
const SHORTCUT_PER_ARTIST: usize = 2;
/// What an epoch showed weighs this much in the next ones (the epoch
/// before first); a place played now keeps more of its weight.
const SHOWN: [f64; 3] = [0.2, 0.5, 0.8];
const SHOWN_NOW: [f64; 3] = [0.6, 0.85, 1.0];
/// «Слушать снова» draws by its score to this power.
const AGAIN_POWER: f64 = 1.5;
/// What the epochs showed, in the library's cache.
const SHOWN_KEY: &str = "home.shown";
/// «Слушать снова»: songs kept, at most this many of one artist.
const LISTEN_AGAIN: usize = 12;
const PER_ARTIST: usize = 2;
/// A favourite comes back after this many days without a play (two weeks
/// once the history is a year long).
const AGAIN_DAYS: i64 = 3;
const AGAIN_DAYS_LONG: i64 = 14;
/// The weight of what was on repeat lately in «Слушать снова».
const AGAIN_HOT: f32 = 2.0;
/// «Из твоей библиотеки»: albums kept, with at least this many tracks with
/// a file, at most half of them ever played.
const SHELF: usize = 12;
const SHELF_MIN: u32 = 5;

/// Where a shortcut leads, as the epochs keep it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Key {
    Release(ReleaseId),
    Playlist(PlaylistId),
    Artist(ArtistId),
}

impl Key {
    fn token(self) -> u64 {
        let (k, id) = match self {
            Self::Release(r) => (1u64, r.0.as_u128()),
            Self::Playlist(p) => (2, p.0.as_u128()),
            Self::Artist(a) => (3, a.0.as_u128()),
        };
        token(id) ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

/// A number of an id for the lotteries.
fn token(id: u128) -> u64 {
    (id as u64) ^ ((id >> 64) as u64).rotate_left(17)
}

/// The kinds of the shortcuts' places ([`SHORTCUT_SLOTS`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Draw {
    /// What plays now (without a history: the recently added).
    Now,
    /// Loved or played much, not lately.
    Forgotten,
    /// The library by the taste: an artist and an album hardly played.
    Offered,
}

/// What an epoch showed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Shown {
    epoch: String,
    shortcuts: Vec<Key>,
    again: Vec<TrackId>,
    /// Drawn before the taste profiles were read (the first build of a
    /// run): drawn again once they are.
    #[serde(default)]
    provisional: bool,
}

/// How much less what the epochs before showed weighs now.
#[derive(Default)]
struct Exposure {
    shortcuts: HashMap<Key, (f64, f64)>,
    again: HashMap<TrackId, f64>,
}

impl Exposure {
    /// Of `before` (the latest first).
    fn of(before: &[Shown]) -> Self {
        let mut e = Self::default();
        for (k, s) in before.iter().take(SHOWN.len()).enumerate() {
            for key in &s.shortcuts {
                let w = e.shortcuts.entry(*key).or_insert((1.0, 1.0));
                w.0 *= SHOWN[k];
                w.1 *= SHOWN_NOW[k];
            }
            for t in &s.again {
                *e.again.entry(*t).or_insert(1.0) *= SHOWN[k];
            }
        }
        e
    }

    fn shortcut(&self, key: Key, draw: Draw) -> f64 {
        self.shortcuts.get(&key).map_or(1.0, |w| if draw == Draw::Now { w.1 } else { w.0 })
    }
}

/// `items` in the order a weighted lottery of `seed` draws them
/// (Efraimidis–Spirakis: each its own chance by its token, so one item's
/// lot does not hang on the others); a weight not positive never comes.
fn lottery<T: Copy>(items: impl IntoIterator<Item = (T, f64)>, seed: u64, tok: impl Fn(T) -> u64) -> Vec<T> {
    let mut keyed: Vec<(f64, u64, T)> = items
        .into_iter()
        .filter(|(_, w)| *w > 0.0 && w.is_finite())
        .map(|(x, w)| {
            let t = tok(x);
            let u = Rng::derive(seed, t).f64().max(f64::MIN_POSITIVE);
            (-u.ln() / w, t, x)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|k| k.2).collect()
}

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
    pub worlds: Vec<WorldParts>,
}

/// The home's epoch (the user's decisions of 2026-10-29): a run of the app
/// within one part of the day. Its shortcuts, «Слушать снова» and «Миксы
/// для тебя» stay while it lasts, whatever the visits and the builds; a
/// start of the app or the next part of the day draws them again. Only
/// what is shown is kept between runs (what each epoch showed, so the next
/// shows other things), never the run's number: a start is a new epoch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Epoch {
    /// The run's number and the part of the day («…​.20390.Evening»).
    pub key: String,
    pub seed: u64,
}

impl Epoch {
    /// The epoch of the run `run` at `now`.
    pub fn of(run: u64, now: UnixMs, utc_offset_min: i32) -> Self {
        let (_, _, part) = crate::recs::daylist_key(now, utc_offset_min);
        let key = format!("{run:016x}.{part}");
        let seed = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3));
        Self { key, seed }
    }

    /// The number of this run: `NIKGOB_HOME_EPOCH=n` in a screenshot run or
    /// the demo (the same home on each), else a new one at each start.
    pub fn run() -> u64 {
        let pinned = std::env::var_os("NIKGOB_SHOT").is_some() || std::env::var_os("NIKGOB_DEMO").is_some();
        pinned
            .then(|| std::env::var("NIKGOB_HOME_EPOCH").ok()?.trim().parse::<u64>().ok())
            .flatten()
            .unwrap_or_else(|| fastrand::u64(..))
    }
}

/// What the parts depend on besides the library.
#[derive(Clone, Default)]
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
    /// The library's sound vectors: the unstarted worlds go by how close
    /// they sound to what plays.
    pub sound: Option<crate::analysis::SoundIndex>,
    /// The listener's taste profiles: «Слушать снова» draws from each,
    /// the shortcuts offer what is close to them.
    pub profiles: Option<std::sync::Arc<crate::recs::TrackProfiles>>,
    /// The home's epoch (none: nothing kept, the seed 0).
    pub epoch: Epoch,
}

impl Visit {
    pub fn now(playing: Option<TrackId>, prefs: &v::Prefs) -> Self {
        Self {
            playing,
            local_only: prefs.mixes_downloaded_only,
            hidden_worlds: prefs.home_hidden_worlds.clone(),
            utc_offset_min: jiff::Zoned::now().offset().seconds() / 60,
            now: now_ms(),
            sound: None,
            profiles: None,
            epoch: Epoch::default(),
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
    /// The hot affinity of the history at the visit ([`nikgob_recs::hot_affinity`]).
    hot: HashMap<TrackId, f32>,
}

impl Facts {
    fn read(lib: &Library, now: UnixMs) -> Self {
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
        let plays: Vec<nikgob_recs::Play> = events.iter().map(crate::recs::play_of).collect();
        let hot = nikgob_recs::hot_affinity(&plays, |t| facts.get(&t).and_then(|f| f.duration_ms), now);
        Self { rows, facts, starts, completes, last_start, liked, hot }
    }
}

pub fn build(lib: &Library, visit: &Visit) -> HomeV2 {
    let f = Facts::read(lib, visit.now);
    let history = !f.starts.is_empty() || !f.liked.is_empty();
    let releases: Vec<ReleaseRow> = lib.releases(0, 20_000, Sort::desc(ReleaseSortKey::Added)).unwrap_or_default();
    let worlds = worlds(&f, &releases, visit);
    let from_library = shelf(&f, &releases, visit, history);
    let places = Places::read(lib, &releases, visit.local_only);
    // The epoch's shortcuts and «Слушать снова», kept; drawn at its first
    // visit (again once the taste profiles are read), by what the epochs
    // before showed.
    let keyed = !visit.epoch.key.is_empty();
    let mut shown: Vec<Shown> =
        if keyed { lib.cache_get(SHOWN_KEY).ok().flatten().and_then(|(_, json)| serde_json::from_str(&json).ok()).unwrap_or_default() } else { Vec::new() };
    let ours = keyed && shown.first().is_some_and(|s| s.epoch == visit.epoch.key);
    let provisional = visit.profiles.is_none();
    let kept = shown.first().filter(|s| ours && (provisional || !s.provisional)).cloned();
    let exposure = Exposure::of(if ours { &shown[1..] } else { &shown[..] });
    let shelved: HashSet<ReleaseId> = from_library.iter().take(SHORTCUTS_NARROW).map(|a| a.0.release.id).collect();
    let draw = |exposure: &Exposure| shortcuts(lib, &f, &releases, &places, visit, history, &shelved, exposure);
    let (shortcuts, listen_again) = match &kept {
        Some(k) => {
            let mut sc: Vec<(Key, ShortcutRef)> = k.shortcuts.iter().filter_map(|key| Some((*key, places.get(*key)?))).collect();
            if sc.len() < k.shortcuts.len() {
                // Something of it gone: the epoch's draw fills its place.
                let taken: HashSet<Key> = sc.iter().map(|x| x.0).collect();
                let more: Vec<(Key, ShortcutRef)> = draw(&exposure).into_iter().filter(|x| !taken.contains(&x.0)).collect();
                sc.extend(more.into_iter().take(k.shortcuts.len() - sc.len()));
            }
            let fits = |t: &TrackId| f.rows.get(t).is_some_and(|r| r.in_library && (r.has_local || !visit.local_only));
            let again = k.again.iter().filter(|t| fits(t)).filter_map(|t| lib.get_track_row(*t).ok().flatten()).collect();
            (sc, again)
        }
        None => (draw(&exposure), listen_again(lib, &f, visit, &exposure)),
    };
    if keyed && kept.is_none() {
        let now = Shown {
            epoch: visit.epoch.key.clone(),
            shortcuts: shortcuts.iter().map(|x| x.0).collect(),
            again: listen_again.iter().map(|t: &TrackRow| t.id).collect(),
            provisional,
        };
        if ours {
            shown[0] = now;
        } else {
            shown.insert(0, now);
        }
        shown.truncate(SHOWN.len() + 1);
        if let Ok(json) = serde_json::to_string(&shown)
            && let Err(e) = lib.cache_put(SHOWN_KEY, visit.now, &json)
        {
            log::warn!("home: keeping what it shows: {e}");
        }
    }
    HomeV2 { history, shortcuts: shortcuts.into_iter().map(|x| x.1).collect(), listen_again, from_library, worlds }
}

/// The places a shortcut can lead to, as the page shows them.
struct Places<'a> {
    releases: HashMap<ReleaseId, &'a ReleaseRow>,
    playlists: HashMap<PlaylistId, PlaylistSummary>,
    artists: HashMap<ArtistId, ArtistRow>,
    local_only: bool,
}

impl<'a> Places<'a> {
    fn read(lib: &Library, releases: &'a [ReleaseRow], local_only: bool) -> Self {
        Self {
            releases: releases.iter().map(|r| (r.release.id, r)).collect(),
            playlists: lib.list_playlists().unwrap_or_default().into_iter().filter_map(|p| Some((p.id?, p))).collect(),
            artists: lib.artists(0, 20_000, Sort::asc(ArtistSortKey::Name)).unwrap_or_default().into_iter().map(|a| (a.artist.id, a)).collect(),
            local_only,
        }
    }

    /// The shortcut of `key`, if it has something to play.
    fn get(&self, key: Key) -> Option<ShortcutRef> {
        match key {
            Key::Release(r) => {
                let r = self.releases.get(&r).filter(|r| r.local_tracks > 0 || (!self.local_only && r.known_tracks > 0))?;
                Some(ShortcutRef::Release((*r).clone()))
            }
            Key::Playlist(p) => Some(ShortcutRef::Playlist(self.playlists.get(&p).filter(|p| p.track_count > 0)?.clone())),
            Key::Artist(a) => Some(ShortcutRef::Artist(self.artists.get(&a).filter(|a| a.local_track_count > 0 || !self.local_only)?.clone())),
        }
    }

    /// The artist a shortcut is of (a playlist's: none).
    fn artist(&self, key: Key) -> Option<ArtistId> {
        match key {
            Key::Release(r) => self.releases.get(&r).and_then(|r| r.release.artist_credit.artist_ids().next()),
            Key::Artist(a) => Some(a),
            Key::Playlist(_) => None,
        }
    }
}

/// The shortcuts of an epoch (the user's decisions of 2026-10-29), each
/// place of [`SHORTCUT_SLOTS`] by its own lottery of the epoch's seed: what
/// plays now — the playlist the plays of the last days started in, the
/// artist page, else the album, by plays fading with a half-life of three
/// days, not always the strict top; forgotten favourites — played much or
/// loved, not for a quarter of the history; what the library offers by the
/// taste ([`offered`]). A kind with nothing left gives its place to the
/// others, then to the library's albums by their likes; what the epochs
/// before showed is less likely, one artist two places at most. Without a
/// history: the recently added, the library by the taste and the rest.
/// The six of a narrow page are shuffled among themselves, so are the two
/// more.
#[allow(clippy::too_many_arguments)]
fn shortcuts(
    lib: &Library,
    f: &Facts,
    releases: &[ReleaseRow],
    places: &Places<'_>,
    visit: &Visit,
    history: bool,
    shelved: &HashSet<ReleaseId>,
    exposure: &Exposure,
) -> Vec<(Key, ShortcutRef)> {
    let seed = visit.epoch.seed;
    let mut now: HashMap<Key, f64> = HashMap::new();
    let mut long: HashMap<Key, f64> = HashMap::new();
    let mut last: HashMap<Key, UnixMs> = HashMap::new();
    let mut in_playlist: HashMap<TrackId, PlaylistId> = HashMap::new();
    for p in &lib.playlist_tracks().unwrap_or_default() {
        for t in &p.tracks {
            in_playlist.entry(*t).or_insert(p.id);
        }
    }
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
        *now.entry(key).or_default() += 0.5_f64.powf(age / HALF_LIFE_MS);
        *long.entry(key).or_default() += 1.0;
        last.insert(key, *ts);
    }
    for t in &f.liked {
        let Some(row) = f.rows.get(t).filter(|r| r.in_library) else { continue };
        if let Some(key) = row.release.map(Key::Release).or_else(|| row.artists.first().copied().map(Key::Artist)) {
            *long.entry(key).or_default() += 2.0;
        }
    }
    let ok = |k: Key| places.get(k).is_some() && !matches!(k, Key::Release(r) if shelved.contains(&r));
    let tok = |k: Key| k.token();
    let playing = |k: &Key| now.get(k).is_some_and(|s| *s >= NOW_MIN);
    let first = f.starts.first().map_or(visit.now, |s| s.0);
    let forget = ((visit.now - first) / 4).clamp(FORGOTTEN_DAYS.0 * DAY_MS, FORGOTTEN_DAYS.1 * DAY_MS);
    let now_drawn: Vec<Key> = if history {
        let playing_now = now.iter().filter(|(k, _)| playing(k) && ok(**k));
        lottery(playing_now.map(|(k, s)| (*k, s * exposure.shortcut(*k, Draw::Now))), seed ^ 0x006e_6f77, tok)
    } else {
        let recent = crate::home::recently_added(lib, releases.iter().filter(|r| ok(Key::Release(r.release.id))).cloned(), SHORTCUTS * 2);
        let recent = recent.iter().enumerate().map(|(n, r)| {
            let k = Key::Release(r.release.id);
            (k, exposure.shortcut(k, Draw::Now) / (1.0 + n as f64))
        });
        lottery(recent, seed ^ 0x006e_6f77, tok)
    };
    let forgotten = long.iter().filter(|(k, l)| **l >= FORGOTTEN_MIN && ok(**k) && !playing(k) && last.get(*k).is_none_or(|ts| visit.now - ts >= forget));
    let forgotten = lottery(forgotten.map(|(k, l)| (*k, l * exposure.shortcut(*k, Draw::Forgotten))), seed ^ 0x006f_6c64, tok);
    let offered = offered(f, places, visit, shelved, exposure, &|k: &Key| playing(k));
    // The rest: the library's albums by their likes and their size.
    let mut liked_in: HashMap<ReleaseId, f64> = HashMap::new();
    for t in &f.liked {
        if let Some(r) = f.rows.get(t).and_then(|r| r.release) {
            *liked_in.entry(r).or_default() += 1.0;
        }
    }
    let rest = releases.iter().filter(|r| ok(Key::Release(r.release.id))).map(|r| {
        let k = Key::Release(r.release.id);
        let size = f64::from(r.local_tracks.max(r.known_tracks).max(1)).sqrt();
        (k, (1.0 + liked_in.get(&r.release.id).copied().unwrap_or(0.0)) * size * exposure.shortcut(k, Draw::Forgotten))
    });
    let rest = lottery(rest, seed ^ 0x7265_7374, tok);
    let mut lists = [now_drawn.into_iter(), forgotten.into_iter(), offered.into_iter(), rest.into_iter()];
    let mut out: Vec<(Key, ShortcutRef)> = Vec::new();
    let mut taken: HashSet<Key> = HashSet::new();
    let mut per_artist: HashMap<ArtistId, usize> = HashMap::new();
    for slot in SHORTCUT_SLOTS {
        let order = match slot {
            Draw::Now => [0, 1, 2, 3],
            Draw::Forgotten => [1, 0, 2, 3],
            Draw::Offered => [2, 1, 0, 3],
        };
        'slot: for g in order {
            for key in lists[g].by_ref() {
                let artist = places.artist(key);
                if taken.contains(&key) || artist.is_some_and(|a| per_artist.get(&a).copied().unwrap_or(0) >= SHORTCUT_PER_ARTIST) {
                    continue;
                }
                let Some(sc) = places.get(key) else { continue };
                taken.insert(key);
                if let Some(a) = artist {
                    *per_artist.entry(a).or_default() += 1;
                }
                out.push((key, sc));
                break 'slot;
            }
        }
    }
    let mut rng = Rng::derive(seed, 0x7368_7566);
    let narrow = out.len().min(SHORTCUTS_NARROW);
    rng.shuffle(&mut out[..narrow]);
    rng.shuffle(&mut out[narrow..]);
    out
}

/// What the library offers by the taste: artists with a few tracks there
/// and albums of which at most a third was heard, neither played much, by
/// how close their songs are to the listener's taste profiles (by the
/// profiles' shares; without profiles, all alike) and their likes; an
/// artist and an album by turns. `playing` tells what plays now (left to
/// the shortcuts of now).
fn offered(f: &Facts, places: &Places<'_>, visit: &Visit, shelved: &HashSet<ReleaseId>, exposure: &Exposure, playing: &dyn Fn(&Key) -> bool) -> Vec<Key> {
    #[derive(Default)]
    struct Tally {
        n: u32,
        fit: f64,
        liked: u32,
        plays: u32,
        heard: u32,
    }
    let profiles = visit.profiles.as_deref().filter(|p| !p.of.is_empty());
    let top = profiles.map_or(1.0, |p| p.shares.iter().copied().fold(0.0f32, f32::max).max(1e-6));
    let fit = |t: &TrackId| -> f64 {
        match profiles {
            Some(p) => p.of.get(t).and_then(|j| p.shares.get(*j)).filter(|s| **s >= nikgob_recs::regions::ACTIVE_SHARE).map_or(0.0, |s| f64::from(s / top)),
            None => 0.5,
        }
    };
    let mut plays: HashMap<TrackId, u32> = HashMap::new();
    for (_, t, _) in &f.starts {
        *plays.entry(*t).or_default() += 1;
    }
    let mut artists: HashMap<ArtistId, Tally> = HashMap::new();
    let mut albums: HashMap<ReleaseId, Tally> = HashMap::new();
    for (t, row) in &f.rows {
        if !row.in_library || (visit.local_only && !row.has_local) {
            continue;
        }
        let (x, p, l) = (fit(t), plays.get(t).copied().unwrap_or(0), u32::from(f.liked.contains(t)));
        let add = |tally: &mut Tally| {
            tally.n += 1;
            tally.fit += x;
            tally.liked += l;
            tally.plays += p;
            tally.heard += u32::from(p > 0);
        };
        if let Some(a) = row.artists.first() {
            add(artists.entry(*a).or_default());
        }
        if let Some(r) = row.release {
            add(albums.entry(r).or_default());
        }
    }
    let worth = |t: &Tally| t.fit / f64::from(t.n.max(1)) * (1.0 + 0.5 * f64::from(t.liked.min(4)));
    let near = |t: &Tally| t.fit / f64::from(t.n.max(1)) >= 0.2;
    let various = |a: &ArtistId| places.artists.get(a).is_some_and(|a| a.artist.name.trim().eq_ignore_ascii_case("various artists"));
    let artists = artists.iter().filter(|(a, t)| t.n >= OFFER_ARTIST_TRACKS && t.plays <= OFFER_PLAYS && near(t) && !various(a)).map(|(a, t)| {
        let k = Key::Artist(*a);
        (k, worth(t) * f64::from(t.n).ln_1p() * exposure.shortcut(k, Draw::Offered))
    });
    let artists: Vec<Key> = lottery(artists.filter(|(k, _)| places.get(*k).is_some() && !playing(k)), visit.epoch.seed ^ 0x0061_7274, Key::token);
    let albums = albums.iter().filter(|(r, t)| t.n >= OFFER_ALBUM_TRACKS && t.heard * 3 <= t.n && t.plays <= OFFER_PLAYS && near(t) && !shelved.contains(r)).map(|(r, t)| {
        let k = Key::Release(*r);
        (k, worth(t) * exposure.shortcut(k, Draw::Offered))
    });
    let albums: Vec<Key> = lottery(albums.filter(|(k, _)| places.get(*k).is_some() && !playing(k)), visit.epoch.seed ^ 0x0061_6c62, Key::token);
    let (mut a, mut b) = (artists.into_iter(), albums.into_iter());
    if visit.epoch.seed & 1 == 1 {
        std::mem::swap(&mut a, &mut b);
    }
    let mut out = Vec::new();
    loop {
        let (x, y) = (a.next(), b.next());
        if x.is_none() && y.is_none() {
            break;
        }
        out.extend(x);
        out.extend(y);
    }
    out
}

/// «Слушать снова»: liked or replayed songs not played for a few days, drawn
/// by the epoch's lottery by how loved they are (at night what is played at
/// night weighs more) to the power [`AGAIN_POWER`], what the epochs before
/// showed less likely ([`SHOWN`]); two of one artist at most, spread over
/// the listener's taste profiles by their shares (each active one at least
/// once, [`nikgob_recs::regions::spread`]).
fn listen_again(lib: &Library, f: &Facts, visit: &Visit, exposure: &Exposure) -> Vec<TrackRow> {
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
        // What was on repeat lately weighs more (the hot affinity, recs-v2
        // §4.2: it tells the songs started again the next day better).
        let hot = (f.hot.get(&t).copied().unwrap_or(0.0) / 2.0).tanh();
        let mut s = if loved { 3.0 } else { 0.0 } + (n as f32).sqrt() + 0.5 * done as f32 + AGAIN_HOT * hot;
        if night_now && n > 0 {
            s *= 1.0 + night as f32 / n as f32;
        }
        scored.push((s, t));
    }
    let weighed = scored.into_iter().map(|(s, t)| (t, f64::from(s.max(0.05)).powf(AGAIN_POWER) * exposure.again.get(&t).copied().unwrap_or(1.0)));
    let drawn = lottery(weighed, visit.epoch.seed ^ 0x0061_676e, |t: TrackId| token(t.0.as_u128()));
    let mut per_artist: HashMap<Option<ArtistId>, usize> = HashMap::new();
    let mut songs = HashSet::new();
    let mut eligible: Vec<(TrackId, Option<usize>)> = Vec::new();
    for t in drawn {
        if eligible.len() == LISTEN_AGAIN * 4 {
            break;
        }
        let Some(row) = f.rows.get(&t) else { continue };
        let a = row.artists.first().copied();
        if per_artist.get(&a).copied().unwrap_or(0) >= PER_ARTIST || !songs.insert(row.song.clone()) {
            continue;
        }
        *per_artist.entry(a).or_default() += 1;
        eligible.push((t, visit.profiles.as_ref().and_then(|p| p.of.get(&t).copied())));
    }
    let picked: Vec<TrackId> = match &visit.profiles {
        Some(p) => nikgob_recs::regions::spread(&eligible, &p.shares, LISTEN_AGAIN),
        None => eligible.into_iter().take(LISTEN_AGAIN).map(|e| e.0).collect(),
    };
    picked.into_iter().filter_map(|t| lib.get_track_row(t).ok().flatten()).collect()
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
    } else if let Some(i) = lower.find(&format!("\"{w}")).or_else(|| lower.find(&format!(" {w}")))
        && lower.len() == s.len()
    {
        // Named inside: «TV Anime "Mushoku Tensei ~…~" Theme Song Collection».
        s.replace_range(i + 1..i + 1 + w.len(), "");
    }
    for prefix in ["TV Anime ", "TVアニメ"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_owned();
        }
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
    // «(OST)» after a name is noise, and so is «Special Edition», and so are
    // the quotes the name was taken out of.
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s = s.replace("(OST)", "").replace("[OST]", "").replace(" Special Edition", "").replace("\"\"", "").replace("\" \"", "");
    if s.matches('"').count() == 1 {
        s = s.replace('"', "");
    }
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

/// A world before it is drawn: the view (without covers) and what its page
/// lists.
#[derive(Clone, Debug)]
pub struct WorldParts {
    pub world: v::World,
    /// Its albums in order, with the tracks of each ever started and all.
    pub albums: Vec<(ReleaseRow, u32, u32)>,
    /// Its liked tracks, in album order.
    pub liked: Vec<TrackId>,
}

/// A world's albums before their tracks are read: a franchise of
/// soundtracks or an artist's discography.
#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub key: String,
    pub title: String,
    pub kind: v::WorldKind,
    /// The keys of the names it goes by («no game no life», «ノーゲーム
    /// ノーライフ»).
    names: Vec<String>,
    /// The names as written, its albums are named inside of.
    pub titles: Vec<String>,
    pub albums: Vec<ReleaseRow>,
}

/// An artist's releases make a world from this many with files (singles, and
/// albums of which a track or two are here, are not a discography) …
const ARTIST_RELEASES: usize = 2;
/// … counting a release with this many tracks with files …
const RELEASE_MIN: u32 = 3;
/// … or from this many tracks with files.
const ARTIST_TRACKS: u32 = 15;
/// A franchise with fewer tracks with files is not a world to travel.
const FRANCHISE_MIN: u32 = 5;

/// A name as compared: lower case, words of letters and digits only.
fn name_key(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A world's albums in order: by year (an album without one last), then
/// by title.
fn by_year(albums: &mut [ReleaseRow]) {
    albums.sort_by(|a, b| {
        let y = |r: &ReleaseRow| r.release.year.unwrap_or(i32::MAX);
        y(a).cmp(&y(b)).then_with(|| a.release.title.cmp(&b.release.title))
    });
}

fn various(r: &ReleaseRow) -> bool {
    r.release.release_type == nikgob_core::model::ReleaseType::Compilation
        || r.release.artist_credit.0.first().is_none_or(|p| p.name.trim().eq_ignore_ascii_case("various artists"))
}

/// The consonants of a name as it sounds, so a katakana name and the English
/// one it spells meet: «メイドインアビス» and «made in abyss» are both
/// «mdnbs». `None` for a name in another script.
fn sound_skeleton(name: &str) -> Option<String> {
    let mut out = String::new();
    let push = |c: char, out: &mut String| {
        if !out.ends_with(c) {
            out.push(c);
        }
    };
    let kana = name.chars().any(|c| matches!(c, '\u{3041}'..='\u{30ff}'));
    if kana {
        for c in name.chars() {
            // Hiragana as katakana.
            let k = match c {
                '\u{3041}'..='\u{3096}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
                _ => c,
            };
            let consonant = match k {
                'カ' | 'キ' | 'ク' | 'ケ' | 'コ' => 'k',
                'ガ' | 'ギ' | 'グ' | 'ゲ' | 'ゴ' => 'g',
                'サ' | 'シ' | 'ス' | 'セ' | 'ソ' => 's',
                'ザ' | 'ジ' | 'ズ' | 'ゼ' | 'ゾ' => 'z',
                'タ' | 'チ' | 'ツ' | 'テ' | 'ト' => 't',
                'ダ' | 'ヂ' | 'ヅ' | 'デ' | 'ド' => 'd',
                'ナ' | 'ニ' | 'ヌ' | 'ネ' | 'ノ' | 'ン' => 'n',
                'フ' => 'f',
                'バ' | 'ビ' | 'ブ' | 'ベ' | 'ボ' | 'ヴ' => 'b',
                'パ' | 'ピ' | 'プ' | 'ペ' | 'ポ' => 'p',
                'マ' | 'ミ' | 'ム' | 'メ' | 'モ' => 'm',
                'ラ' | 'リ' | 'ル' | 'レ' | 'ロ' => 'r',
                // Vowels, ハ-row h, ヤ/ワ glides, small kana, ー.
                '\u{30a1}'..='\u{30fc}' => continue,
                c if c.is_whitespace() || !c.is_alphanumeric() => continue,
                // Kanji or Latin among the kana: not one spelling.
                _ => return None,
            };
            push(consonant, &mut out);
        }
    } else {
        let lower = name.to_lowercase();
        if !lower.chars().all(|c| c.is_ascii() || !c.is_alphanumeric()) {
            return None;
        }
        let b: Vec<char> = lower.chars().filter(char::is_ascii_alphabetic).collect();
        let mut i = 0;
        while i < b.len() {
            let next = b.get(i + 1).copied();
            let (c, skip) = match (b[i], next) {
                ('p', Some('h')) => (Some('f'), 2),
                ('c', Some('h')) => (Some('t'), 2),
                ('s' | 't', Some('h')) => (Some('s'), 2),
                ('c', Some('k')) | ('q', Some('u')) => (Some('k'), 2),
                ('g', Some('h')) => (None, 2),
                ('k', Some('n')) => (Some('n'), 2),
                ('c', Some('e' | 'i' | 'y')) => (Some('s'), 1),
                ('c' | 'q', _) => (Some('k'), 1),
                ('x', _) => {
                    push('k', &mut out);
                    (Some('s'), 1)
                }
                ('l', _) => (Some('r'), 1),
                ('v', _) => (Some('b'), 1),
                ('j', _) => (Some('z'), 1),
                ('a' | 'e' | 'i' | 'o' | 'u' | 'y' | 'h' | 'w', _) => (None, 1),
                (c, _) => (Some(c), 1),
            };
            if let Some(c) = c {
                push(c, &mut out);
            }
            i += skip;
        }
    }
    Some(out)
}

/// The one name two franchise names share, if they are one franchise: the
/// same name, one a sequel of the other («NO GAME NO LIFE ZERO» of «NO GAME
/// NO LIFE»), or a katakana name and the English one it spells. The Latin
/// one is shown.
fn same_franchise(a: &str, b: &str) -> Option<String> {
    let (ka, kb) = (name_key(a), name_key(b));
    if ka == kb {
        return Some(a.to_owned());
    }
    if ka.starts_with(&format!("{kb} ")) {
        return Some(b.to_owned());
    }
    if kb.starts_with(&format!("{ka} ")) {
        return Some(a.to_owned());
    }
    // Word-prefixes of each against the other whole.
    let prefixes = |n: &str| -> Vec<String> {
        let words: Vec<&str> = n.split(|c: char| c.is_whitespace() || c == '・' || c == '･').filter(|w| !w.is_empty()).collect();
        (1..=words.len()).rev().map(|k| words[..k].join(" ")).collect()
    };
    for (x, y) in [(a, b), (b, a)] {
        let Some(whole) = sound_skeleton(y).filter(|s| s.len() >= 4) else { continue };
        for p in prefixes(x) {
            if sound_skeleton(&p).is_some_and(|s| s == whole) {
                let latin = |n: &str| n.chars().all(|c| c.is_ascii() || !c.is_alphanumeric());
                return Some(if latin(&p) || !latin(y) { p } else { y.to_owned() });
            }
        }
    }
    None
}

/// Whether an album without a soundtrack's marker belongs to franchise `g`:
/// its title begins with a name of it, or names it in two words or more
/// («"No Game No Life" Complete Songs», «TV Anime "Mushoku Tensei ~…~"
/// Theme Song Collection»). A one-word name («OMORI») only takes an album of
/// the same artist.
fn names_franchise(r: &ReleaseRow, g: &Group) -> bool {
    let title = name_key(&r.release.title);
    let words: Vec<&str> = title.split(' ').collect();
    let artist = r.release.artist_credit.artist_ids().next();
    g.names.iter().any(|n| {
        let nw: Vec<&str> = n.split(' ').collect();
        let begins = words.len() > nw.len() && words.starts_with(&nw);
        let inside = nw.len() >= 2 && words.windows(nw.len()).any(|w| w == nw.as_slice());
        let same_artist = g.albums.iter().any(|a| a.release.artist_credit.artist_ids().next() == artist && artist.is_some());
        (begins || inside) && (nw.len() >= 2 || same_artist)
    })
}

/// The worlds of `releases`: soundtrack franchises (by
/// `recs::tiles::franchise`; one franchise under two names is one world, and
/// albums without a soundtrack's marker that name it join it), then the
/// artists with a real discography in what is left — a franchise wins over
/// its composer. Compilations and «Various Artists» make no artist world.
pub(crate) fn group_worlds(releases: &[ReleaseRow]) -> Vec<Group> {
    let mut sorted: Vec<&ReleaseRow> = releases.iter().collect();
    sorted.sort_by(|a, b| a.release.year.cmp(&b.release.year).then_with(|| a.release.title.cmp(&b.release.title)));
    let mut groups: Vec<Group> = Vec::new();
    let mut taken: HashSet<ReleaseId> = HashSet::new();
    for r in &sorted {
        let Some((_, t)) = franchise(&r.release.title) else { continue };
        let title = series_name(&t);
        let key = name_key(&title);
        if key.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|g| g.key == key) {
            Some(g) => g.albums.push((*r).clone()),
            None => groups.push(Group {
                key: key.clone(),
                titles: vec![title.clone()],
                title,
                kind: v::WorldKind::Soundtrack,
                names: vec![key],
                albums: vec![(*r).clone()],
            }),
        }
        taken.insert(r.release.id);
    }
    // One franchise under two names.
    let mut i = 0;
    while i < groups.len() {
        let mut j = i + 1;
        while j < groups.len() {
            if let Some(name) = same_franchise(&groups[i].title, &groups[j].title) {
                let g = groups.remove(j);
                let w = &mut groups[i];
                w.albums.extend(g.albums);
                w.names.extend(g.names);
                w.titles.extend(g.titles);
                w.title = name;
                w.key = name_key(&w.title);
                if !w.names.contains(&w.key) {
                    w.names.push(w.key.clone());
                }
                j = i + 1;
            } else {
                j += 1;
            }
        }
        i += 1;
    }
    for r in &sorted {
        if taken.contains(&r.release.id) {
            continue;
        }
        if let Some(g) = groups.iter_mut().find(|g| names_franchise(r, g)) {
            g.albums.push((*r).clone());
            taken.insert(r.release.id);
        }
    }
    // Artists, by the main credited one.
    let mut artists: Vec<(ArtistId, Vec<ReleaseRow>)> = Vec::new();
    for r in &sorted {
        if taken.contains(&r.release.id) || various(r) {
            continue;
        }
        let Some(a) = r.release.artist_credit.artist_ids().next() else { continue };
        match artists.iter_mut().find(|x| x.0 == a) {
            Some(x) => x.1.push((*r).clone()),
            None => artists.push((a, vec![(*r).clone()])),
        }
    }
    for (a, albums) in artists {
        let releases = albums
            .iter()
            .filter(|r| r.local_tracks >= RELEASE_MIN && r.release.release_type != nikgob_core::model::ReleaseType::Single)
            .count();
        let tracks: u32 = albums.iter().map(|r| r.local_tracks).sum();
        if releases < ARTIST_RELEASES && tracks < ARTIST_TRACKS {
            continue;
        }
        // The name it is credited by most.
        let mut names: Vec<(String, usize)> = Vec::new();
        for r in &albums {
            if let Some(p) = r.release.artist_credit.0.iter().find(|p| p.artist_id == Some(a)) {
                match names.iter_mut().find(|n| n.0 == p.name) {
                    Some(n) => n.1 += 1,
                    None => names.push((p.name.clone(), 1)),
                }
            }
        }
        let title = names.iter().max_by_key(|n| n.1).map(|n| n.0.clone()).unwrap_or_default();
        groups.push(Group { key: format!("artist:{a}"), title, kind: v::WorldKind::Artist, names: Vec::new(), titles: Vec::new(), albums });
    }
    groups.retain(|g| g.kind == v::WorldKind::Artist || g.albums.iter().map(|a| a.local_tracks).sum::<u32>() >= FRANCHISE_MIN);
    for g in &mut groups {
        by_year(&mut g.albums);
    }
    groups
}

/// The world of each of `releases` that is in one ([`group_worlds`]): its
/// key and title. The recommendations keep one world from filling a mix
/// by it.
pub(crate) fn worlds_of_releases(releases: &[ReleaseRow]) -> HashMap<ReleaseId, (String, String)> {
    let mut out = HashMap::new();
    for g in group_worlds(releases) {
        for a in &g.albums {
            out.insert(a.release.id, (g.key.clone(), g.title.clone()));
        }
    }
    out
}

/// The world key of a track: its album's world, else its main artist's
/// (the same key as the artist's own world).
pub(crate) fn track_world_key(album_world: Option<&str>, main_artist: Option<ArtistId>) -> Option<String> {
    album_world.map(str::to_owned).or_else(|| main_artist.map(|a| format!("artist:{a}")))
}

/// What the main button of a world plays, from its tracks in album order:
/// «Дослушать» the unheard ones with a file, «Начать» everything when
/// nothing was heard, «С начала» everything when all with a file were.
pub(crate) fn world_start(order: &[TrackId], heard: &HashSet<TrackId>, has_file: impl Fn(&TrackId) -> bool, local_only: bool) -> (v::WorldStart, Vec<TrackId>) {
    let everything: Vec<TrackId> = order.iter().copied().filter(|t| has_file(t) || !local_only).collect();
    if heard.is_empty() {
        return (v::WorldStart::Begin, everything);
    }
    let unheard: Vec<TrackId> = order.iter().copied().filter(|t| !heard.contains(t) && has_file(t)).collect();
    if unheard.is_empty() { (v::WorldStart::Again, everything) } else { (v::WorldStart::Finish, unheard) }
}

/// The tracks of each album of the library in order (disc, then number).
fn album_tracks(f: &Facts) -> HashMap<ReleaseId, Vec<(u32, TrackId)>> {
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
    tracks_of
}

/// What «Автовоспроизведение» plays after `track` of an album when it
/// continues the album's world ([`v::Prefs::autoplay_continue_world`]):
/// the world's tracks after it in album order — the ones never heard with
/// a file («Дослушать»), else any — of the next album that has some, none
/// of `queued`. Empty past the world's end (a radio goes on then).
pub fn world_next(lib: &Library, track: TrackId, queued: &HashSet<TrackId>, local_only: bool) -> Vec<TrackId> {
    let f = Facts::read(lib, now_ms());
    let Some(release) = f.rows.get(&track).and_then(|r| r.release) else { return Vec::new() };
    let releases: Vec<ReleaseRow> = lib.releases(0, 20_000, Sort::desc(ReleaseSortKey::Added)).unwrap_or_default();
    let usable: Vec<ReleaseRow> = releases.into_iter().filter(|r| r.local_tracks > 0 || !local_only).collect();
    let Some(g) = group_worlds(&usable).into_iter().find(|g| g.albums.iter().any(|a| a.release.id == release)) else {
        return Vec::new();
    };
    let tracks_of = album_tracks(&f);
    let order: Vec<(ReleaseId, TrackId)> = g
        .albums
        .iter()
        .flat_map(|a| tracks_of.get(&a.release.id).into_iter().flatten().map(|(_, t)| (a.release.id, *t)))
        .collect();
    let Some(at) = order.iter().position(|o| o.1 == track) else { return Vec::new() };
    let has_file = |t: &TrackId| f.rows.get(t).is_some_and(|r| r.has_local);
    let after: Vec<(ReleaseId, TrackId)> =
        order[at + 1..].iter().copied().filter(|(_, t)| !queued.contains(t) && (has_file(t) || !local_only)).collect();
    let unheard: Vec<(ReleaseId, TrackId)> = after.iter().copied().filter(|(_, t)| !f.last_start.contains_key(t) && has_file(t)).collect();
    let pool = if unheard.is_empty() { after } else { unheard };
    let Some(album) = pool.first().map(|o| o.0) else { return Vec::new() };
    pool.into_iter().filter(|o| o.0 == album).map(|o| o.1).collect()
}

/// The worlds of the library, by the last play; the unstarted ones last,
/// the closest by sound to what plays (else to the last started track)
/// first. Every one: the row pages through them all.
fn worlds(f: &Facts, releases: &[ReleaseRow], visit: &Visit) -> Vec<WorldParts> {
    let usable: Vec<ReleaseRow> = releases.iter().filter(|r| r.local_tracks > 0 || !visit.local_only).cloned().collect();
    let mut groups = group_worlds(&usable);
    groups.retain(|g| g.albums.iter().any(|a| a.local_tracks > 0) && !visit.hidden_worlds.contains(&g.key));
    let tracks_of = album_tracks(f);
    let titles: HashMap<TrackId, &str> = f.rows.iter().map(|(id, r)| (*id, r.title.as_str())).collect();
    let duration = |t: &TrackId| u64::from(f.facts.get(t).and_then(|x| x.duration_ms).unwrap_or(0));
    let local = |t: &TrackId| f.rows.get(t).is_some_and(|r| r.has_local);
    let playing_release = visit.playing.and_then(|t| f.rows.get(&t)).and_then(|r| r.release);
    // What the unstarted worlds are compared with by sound.
    let sound = visit.sound.as_ref().map(|s| s.read());
    let reference = sound.as_ref().and_then(|idx| {
        let t = visit.playing.or_else(|| f.starts.last().map(|s| s.1))?;
        idx.get(t).copied()
    });

    let mut out: Vec<(Option<UnixMs>, Option<f32>, usize, WorldParts)> = Vec::new();
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
        let ids: Vec<TrackId> = order.iter().map(|o| o.2).collect();
        let heard: HashSet<TrackId> = ids.iter().copied().filter(|t| f.last_start.contains_key(t)).collect();
        let last = ids.iter().filter_map(|t| f.last_start.get(t)).max().copied();
        let n = order.len();
        let short = |r: ReleaseId| -> String {
            let Some(a) = g.albums.iter().find(|a| a.release.id == r) else { return String::new() };
            if g.kind == v::WorldKind::Artist {
                return a.release.title.clone();
            }
            // Inside the name it is written with («VOLUME.02» of «ノーゲーム・ノーライフ»).
            let inside = |title: &str| {
                g.titles.iter().chain([&g.title]).map(|n| short_album(title, n)).min_by_key(|s| s.chars().count()).unwrap_or_default()
            };
            let numbered = g.albums.iter().any(|a| {
                let s = inside(&a.release.title);
                s.starts_with("OST ") && s[4..].starts_with(|c: char| c.is_ascii_digit())
            });
            let s = inside(&a.release.title);
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

        let k = g.albums.len();
        let mut facts = vec![g.kind.label().to_owned()];
        match g.kind {
            v::WorldKind::Soundtrack => {
                // Its artists, the one with the most tracks first.
                let mut artists: Vec<(String, u32)> = Vec::new();
                for a in &g.albums {
                    let name = a.release.artist_credit.display();
                    let words = |n: &str| {
                        let mut w: Vec<String> = n.to_lowercase().split_whitespace().map(str::to_owned).collect();
                        w.sort();
                        w
                    };
                    if name.is_empty() || various(a) {
                        continue;
                    }
                    match artists.iter_mut().find(|x| words(&x.0) == words(&name)) {
                        Some(x) => x.1 += a.local_tracks.max(1),
                        None => artists.push((name, a.local_tracks.max(1))),
                    }
                }
                artists.sort_by(|a, b| b.1.cmp(&a.1));
                facts.push(artists.iter().take(2).map(|a| a.0.as_str()).collect::<Vec<_>>().join(", "));
                if k > 1 {
                    facts.push(count(k, "альбом", "альбома", "альбомов"));
                }
            }
            v::WorldKind::Artist => {
                if k > 1 {
                    facts.push(count(k, "релиз", "релиза", "релизов"));
                }
            }
        }
        facts.push(count(n, "трек", "трека", "треков"));
        facts.retain(|x| !x.is_empty());

        let (start, primary) = world_start(&ids, &heard, local, visit.local_only);
        let primary_ms: u64 = primary.iter().map(duration).sum();
        let primary_tip = format!(
            "{}, {}: {}",
            count(primary.len(), "трек", "трека", "треков"),
            hours_minutes(primary_ms),
            match start {
                v::WorldStart::Finish => "что ещё не слышал, по порядку альбомов",
                v::WorldStart::Begin => "с первого трека первого альбома",
                v::WorldStart::Again => "всё с первого трека",
            }
        );
        let mut shuffled: Vec<TrackId> = ids.iter().copied().filter(|t| local(t) || !visit.local_only).collect();
        fastrand::shuffle(&mut shuffled);
        let fresh = heard.is_empty();

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
            let title = format!("{}, трек {}", short(o.0), o.1 + 1);
            if !cards.iter().any(|c| c.title == title) {
                cards.push(v::WorldCard { icon: v::WorldIcon::Next, kicker: "По порядку".into(), title, go: v::WorldGo::Play(from(o.0, o.1)) });
            }
        }
        let untouched = g
            .albums
            .iter()
            .find(|a| a.local_tracks > 0 && tracks_of.get(&a.release.id).is_none_or(|l| l.iter().all(|x| !heard.contains(&x.1))));
        let liked: Vec<TrackId> = ids.iter().copied().filter(|t| f.liked.contains(t)).collect();
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

        // How close it sounds: its centre against the reference.
        let closeness = match (&sound, reference) {
            (Some(idx), Some(r)) if last.is_none() => {
                let vs: Vec<&nikgob_recs::Vector> = ids.iter().filter_map(|t| idx.get(*t)).collect();
                (!vs.is_empty()).then(|| {
                    let mut c = [0.0_f32; nikgob_recs::DIM];
                    for v in &vs {
                        for (x, y) in c.iter_mut().zip(v.iter()) {
                            *x += y / vs.len() as f32;
                        }
                    }
                    idx.distance(&c, &r)
                })
            }
            _ => None,
        };
        let albums = g
            .albums
            .iter()
            .map(|a| {
                let list = tracks_of.get(&a.release.id).map(Vec::as_slice).unwrap_or(&[]);
                let h = list.iter().filter(|x| heard.contains(&x.1)).count() as u32;
                (a.clone(), h, list.len() as u32)
            })
            .collect();
        let kicker = if playing { "Слушаешь сейчас".to_owned() } else { heard_when(visit, last) };
        out.push((
            last,
            closeness,
            n,
            WorldParts {
                world: v::World {
                    key: g.key,
                    title: g.title,
                    kind: g.kind,
                    release: stage,
                    cover: None,
                    cover_hd: None,
                    kicker,
                    facts: facts.join(" · "),
                    heard: heard.len() as u32,
                    total: n as u32,
                    start,
                    playing,
                    primary,
                    primary_tip,
                    shuffled,
                    albums: g.albums.iter().map(|a| a.release.id).collect(),
                    cards,
                },
                albums,
                liked,
            },
        ));
    }
    out.sort_by(|a, b| world_order((a.0, a.1, a.2), (b.0, b.1, b.2)));
    out.into_iter().map(|x| x.3).collect()
}

/// The order of the worlds by (last play, closeness by sound, tracks): the
/// last played first; the unstarted after them, the closest first, the ones
/// without a sound after those, the largest first.
fn world_order(a: (Option<UnixMs>, Option<f32>, usize), b: (Option<UnixMs>, Option<f32>, usize)) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.0, b.0) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => match (a.1, b.1) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => b.2.cmp(&a.2),
        },
    }
}

/// One world of the library for its page.
pub fn world(lib: &Library, visit: &Visit, key: &str) -> Option<WorldParts> {
    let mut visit = visit.clone();
    // A world taken off the row still has its page.
    visit.hidden_worlds.clear();
    let f = Facts::read(lib, visit.now);
    let releases: Vec<ReleaseRow> = lib.releases(0, 20_000, Sort::desc(ReleaseSortKey::Added)).unwrap_or_default();
    worlds(&f, &releases, &visit).into_iter().find(|w| w.world.key == key)
}

/// A new album of the world among the services' new releases: «Новый
/// альбом» for its third card (the release then leaves «Новые релизы» while
/// the card shows it).
pub fn world_release(world: &v::World, releases: &[v::StreamAlbumCard]) -> Option<usize> {
    if world.kind == v::WorldKind::Artist {
        // An artist's new album: by the artist's name.
        let name = name_key(&world.title);
        return releases.iter().position(|a| a.artist.split(", ").any(|n| name_key(n) == name));
    }
    // A release without a soundtrack's marker («MADE IN ABYSS FILM CONCERT
    // LIVE RECORDING CD») is the world's when it begins with its name.
    let name = world.title.to_lowercase();
    releases.iter().position(|a| {
        franchise(&a.title).is_some_and(|(_, t)| name_key(&series_name(&t)) == world.key)
            || (name.chars().count() >= 4 && a.title.to_lowercase().strip_prefix(&name).is_some_and(|rest| rest.starts_with(' ')))
    })
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

    fn rel(title: &str, artist: &str, id: ArtistId, local: u32, kind: nikgob_core::model::ReleaseType) -> ReleaseRow {
        let mut r = nikgob_core::model::Release::new(title, nikgob_core::model::ArtistCredit::single(artist, Some(id)));
        r.release_type = kind;
        ReleaseRow { release: r, local_tracks: local, known_tracks: local, liked: false, rows: local }
    }

    fn album(title: &str, artist: &str, id: ArtistId, local: u32) -> ReleaseRow {
        rel(title, artist, id, local, nikgob_core::model::ReleaseType::Album)
    }

    fn worlds_of(releases: &[ReleaseRow]) -> Vec<(String, v::WorldKind, usize)> {
        group_worlds(releases).into_iter().map(|g| (g.title, g.kind, g.albums.len())).collect()
    }

    #[test]
    fn a_franchise_wins_over_its_composer_and_an_artist_needs_a_discography() {
        let (penkin, pirate, mell, va) = (ArtistId::new(), ArtistId::new(), ArtistId::new(), ArtistId::new());
        let mut releases = vec![
            album("MADE IN ABYSS ORIGINAL SOUNDTRACK", "Kevin Penkin", penkin, 52),
            album("MADE IN ABYSS ORIGINAL SOUNDTRACK 2", "Kevin Penkin", penkin, 30),
            album("MADE IN ABYSS ORIGINAL SOUNDTRACK 3", "Kevin Penkin", penkin, 32),
            album("Прости малышка", "Серега Пират", pirate, 5),
            album("Вот мой альбом, сори", "Серега Пират", pirate, 6),
            album("Разгром", "MellSher", mell, 1),
            rel("Хвост", "MellSher", mell, 1, nikgob_core::model::ReleaseType::Single),
            rel("Best of", "Various Artists", va, 30, nikgob_core::model::ReleaseType::Compilation),
        ];
        let w = worlds_of(&releases);
        assert_eq!(
            w,
            [("MADE IN ABYSS".to_owned(), v::WorldKind::Soundtrack, 3), ("Серега Пират".to_owned(), v::WorldKind::Artist, 2)],
            "Kevin Penkin's albums are all MADE IN ABYSS; one-track releases and a compilation make no world"
        );
        // His album outside the franchise, large enough, is his own world.
        releases.push(album("The Rising of the Shield Hero Theme", "Kevin Penkin", penkin, 20));
        let w = worlds_of(&releases);
        assert!(w.contains(&("Kevin Penkin".to_owned(), v::WorldKind::Artist, 1)), "{w:?}");
        assert!(w.contains(&("MADE IN ABYSS".to_owned(), v::WorldKind::Soundtrack, 3)), "{w:?}");
    }

    #[test]
    fn one_franchise_under_its_japanese_and_english_names_is_one_world() {
        let (sweep, fujisawa, va, other) = (ArtistId::new(), ArtistId::new(), ArtistId::new(), ArtistId::new());
        let releases = vec![
            album("ノーゲーム・ノーライフ VOLUME.02 スペシャルCD サウンドトラック Vol.1", "Super sweep", sweep, 19),
            album("ノーゲーム・ノーライフ VOLUME.04 スペシャルCD サウンドトラック Vol.2", "Super sweep", sweep, 16),
            album("NO GAME NO LIFE ZERO Original Soundtrack", "藤澤慶昌", fujisawa, 34),
            rel("\"No Game No Life\" Complete Songs \"NO SONG NO LIFE\"", "Various Artists", va, 10, nikgob_core::model::ReleaseType::Compilation),
            album("Mushoku Tensei ~Isekai Ittara Honki Dasu~ Original Soundtrack 1 Special Edition", "藤澤慶昌", fujisawa, 40),
            album("TV Anime \"Mushoku Tensei ~Isekai Ittara Honki Dasu~\" Theme Song Collection", "大原ゆい子", other, 8),
        ];
        let w = worlds_of(&releases);
        assert_eq!(
            w,
            [
                ("Mushoku Tensei".to_owned(), v::WorldKind::Soundtrack, 2),
                ("NO GAME NO LIFE".to_owned(), v::WorldKind::Soundtrack, 4),
            ],
            "the composer of both makes no world of his own either"
        );
        assert_eq!(same_franchise("メイドインアビス", "MADE IN ABYSS").as_deref(), Some("MADE IN ABYSS"));
        assert_eq!(same_franchise("Mushoku Tensei", "NO GAME NO LIFE"), None);
        assert_eq!(
            short_album("TV Anime \"Mushoku Tensei ~Isekai Ittara Honki Dasu~\" Theme Song Collection", "Mushoku Tensei"),
            "Theme Song Collection"
        );
    }

    #[test]
    fn the_main_button_finishes_begins_or_starts_again() {
        let t: Vec<TrackId> = (0..5).map(|_| TrackId::new()).collect();
        let file = |x: &TrackId| *x != t[4];
        let (start, list) = world_start(&t, &HashSet::new(), file, false);
        assert_eq!((start, list), (v::WorldStart::Begin, t.clone()), "«Начать»: everything from the first track");
        let (_, list) = world_start(&t, &HashSet::new(), file, true);
        assert_eq!(list, t[..4], "only the files with «Только скачанное»");
        let heard: HashSet<TrackId> = [t[0], t[2]].into();
        assert_eq!(world_start(&t, &heard, file, false), (v::WorldStart::Finish, vec![t[1], t[3]]), "«Дослушать»: the unheard files in order");
        let heard: HashSet<TrackId> = t[..4].iter().copied().collect();
        assert_eq!(world_start(&t, &heard, file, false), (v::WorldStart::Again, t.clone()), "all files heard: «С начала»");
    }

    #[test]
    fn unstarted_worlds_go_last_the_closest_by_sound_first() {
        let mut w = vec![(None, None, 50), (Some(5), None, 10), (None, Some(2.0), 5), (Some(9), None, 1), (None, Some(1.0), 3)];
        w.sort_by(|a, b| world_order(*a, *b));
        assert_eq!(w, [(Some(9), None, 1), (Some(5), None, 10), (None, Some(1.0), 3), (None, Some(2.0), 5), (None, None, 50)]);
    }

    /// Three albums of one franchise: after the first, autoplay goes on with
    /// the second, the unheard tracks of it, then the third; past the
    /// world's end, nothing (the radio comes).
    #[test]
    fn autoplay_continues_the_world() {
        use nikgob_core::model::{Artist, ArtistCredit, Codec, Event, Release, Source, SourceKind, Track};
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("Kevin Penkin")).unwrap();
        let credit = ArtistCredit::single("Kevin Penkin", Some(a.id));
        let mut ids: Vec<Vec<TrackId>> = Vec::new();
        for (k, title) in ["MADE IN ABYSS ORIGINAL SOUNDTRACK", "MADE IN ABYSS ORIGINAL SOUNDTRACK 2", "MADE IN ABYSS ORIGINAL SOUNDTRACK 3"].iter().enumerate() {
            let mut r = Release::new(*title, credit.clone());
            r.year = Some(2017 + k as i32);
            let r = lib.upsert_release(&r).unwrap();
            let mut album = Vec::new();
            for n in 0..3 {
                let mut t = Track::new(format!("{title} {n}"), credit.clone());
                t.release_id = Some(r.id);
                t.track_no = n + 1;
                let t = lib.upsert_track(&t).unwrap();
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("x/{k}/{n}.flac"), Codec::Flac)).unwrap();
                album.push(t.id);
            }
            ids.push(album);
        }
        let first: HashSet<TrackId> = ids[0].iter().copied().collect();
        assert_eq!(world_next(&lib, ids[0][2], &first, false), ids[1], "the next album, in order");
        // Its first track was heard already: the rest of it.
        lib.record_event_raw(&Event::new(ids[1][0], EventType::Start, now_ms() - 1000, lib.device_id())).unwrap();
        assert_eq!(world_next(&lib, ids[0][2], &first, false), ids[1][1..]);
        // What the queue has does not come again.
        let queued: HashSet<TrackId> = first.iter().copied().chain(ids[1].iter().copied()).collect();
        assert_eq!(world_next(&lib, ids[1][2], &queued, false), ids[2]);
        assert!(world_next(&lib, ids[2][2], &HashSet::new(), false).is_empty(), "the world's end");
    }

    #[test]
    fn a_world_page_has_its_albums_with_what_was_heard_and_its_likes() {
        use nikgob_core::model::{Artist, ArtistCredit, Codec, Event, Release, Source, SourceKind, Track};
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("Kevin Penkin")).unwrap();
        let credit = ArtistCredit::single("Kevin Penkin", Some(a.id));
        let now = now_ms();
        let mut ids = Vec::new();
        for (k, title) in ["MADE IN ABYSS ORIGINAL SOUNDTRACK", "MADE IN ABYSS ORIGINAL SOUNDTRACK 2"].iter().enumerate() {
            let mut r = Release::new(*title, credit.clone());
            r.year = Some(2017 + k as i32 * 3);
            let r = lib.upsert_release(&r).unwrap();
            for n in 0..3 {
                let mut t = Track::new(format!("{title} {n}"), credit.clone());
                t.release_id = Some(r.id);
                t.track_no = n + 1;
                t.duration_ms = Some(60_000);
                let t = lib.upsert_track(&t).unwrap();
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("x/{k}/{n}.flac"), Codec::Flac)).unwrap();
                ids.push(t.id);
            }
        }
        // Heard: the first track of each album; liked: three.
        for (i, t) in [ids[0], ids[3]].iter().enumerate() {
            lib.record_event_raw(&Event::new(*t, EventType::Start, now - 1000 + i as i64, lib.device_id())).unwrap();
        }
        for t in [ids[1], ids[2], ids[5]] {
            lib.like(t).unwrap();
        }
        let visit = Visit { now, ..Visit::default() };
        let w = world(&lib, &visit, "made in abyss").expect("the world");
        assert_eq!(w.world.title, "MADE IN ABYSS");
        assert_eq!(w.world.kind, v::WorldKind::Soundtrack);
        assert_eq!(w.world.facts, "Саундтрек · Kevin Penkin · 2 альбома · 6 треков");
        assert_eq!((w.world.heard, w.world.total), (2, 6));
        assert_eq!(w.world.start, v::WorldStart::Finish);
        assert_eq!(w.world.primary, [ids[1], ids[2], ids[4], ids[5]]);
        let albums: Vec<(&str, u32, u32)> = w.albums.iter().map(|(r, h, t)| (r.release.title.as_str(), *h, *t)).collect();
        assert_eq!(albums, [("MADE IN ABYSS ORIGINAL SOUNDTRACK", 1, 3), ("MADE IN ABYSS ORIGINAL SOUNDTRACK 2", 1, 3)]);
        assert_eq!(w.liked, [ids[1], ids[2], ids[5]]);
        let mut shuffled = w.world.shuffled.clone();
        shuffled.sort();
        let mut all = ids.clone();
        all.sort();
        assert_eq!(shuffled, all, "«Вперемешку» is every track");
        // Taken off the row, it still has its page.
        let hidden = Visit { hidden_worlds: vec!["made in abyss".into()], ..visit };
        assert!(build(&lib, &hidden).worlds.is_empty());
        assert!(world(&lib, &hidden, "made in abyss").is_some());
    }

    /// A synthetic library: `artists` artists of `albums` albums of
    /// `tracks` tracks, each with a file; the ids by artist, album, track.
    fn synth(lib: &Library, artists: usize, albums: usize, tracks: usize) -> Vec<Vec<Vec<TrackId>>> {
        use nikgob_core::model::{Artist, ArtistCredit, Codec, Release, Source, SourceKind, Track};
        let mut out = Vec::new();
        for a in 0..artists {
            let name = format!("Artist {a}");
            let artist = lib.upsert_artist(&Artist::new(&name)).unwrap();
            let credit = ArtistCredit::single(&name, Some(artist.id));
            let mut of_artist = Vec::new();
            for b in 0..albums {
                let mut r = Release::new(format!("Album {a}.{b}"), credit.clone());
                r.year = Some(2000 + b as i32);
                let r = lib.upsert_release(&r).unwrap();
                let mut album = Vec::new();
                for n in 0..tracks {
                    let mut t = Track::new(format!("Song {a}.{b}.{n}"), credit.clone());
                    t.release_id = Some(r.id);
                    t.track_no = n as u32 + 1;
                    t.duration_ms = Some(200_000);
                    let t = lib.upsert_track(&t).unwrap();
                    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("x/{a}/{b}/{n}.flac"), Codec::Flac)).unwrap();
                    album.push(t.id);
                }
                of_artist.push(album);
            }
            out.push(of_artist);
        }
        out
    }

    fn play(lib: &Library, t: TrackId, ts: UnixMs) {
        use nikgob_core::model::Event;
        lib.record_event_raw(&Event::new(t, EventType::Start, ts, lib.device_id())).unwrap();
        lib.record_event_raw(&Event::new(t, EventType::Complete, ts + 200_000, lib.device_id())).unwrap();
    }

    /// Every track of the library in one taste profile.
    fn one_profile(lib: &Library) -> std::sync::Arc<crate::recs::TrackProfiles> {
        let of = lib.rec_tracks().unwrap().into_iter().map(|t| (t.id, 0)).collect();
        std::sync::Arc::new(crate::recs::TrackProfiles { of, shares: vec![1.0] })
    }

    fn sc_key(s: &ShortcutRef) -> String {
        match s {
            ShortcutRef::Release(r) => format!("r {}", r.release.title),
            ShortcutRef::Playlist(p) => format!("p {}", p.name),
            ShortcutRef::Artist(a) => format!("a {}", a.artist.name),
        }
    }

    fn picks(h: &HomeV2) -> (Vec<String>, Vec<TrackId>) {
        (h.shortcuts.iter().map(sc_key).collect(), h.listen_again.iter().map(|t| t.id).collect())
    }

    /// An omnivore with a history: twelve artists of two albums; four of
    /// them played these days, six played much two months ago, some songs
    /// liked. In one epoch the home is the same at every visit, after a
    /// play too; thirty epochs draw other compositions, each with what
    /// plays now, a forgotten favourite and what the library offers, and
    /// every album played now or long ago comes at least once (the ids, and
    /// so the lots, are new at each run of the test: ten epochs left an
    /// album out now and then).
    #[test]
    fn the_home_keeps_its_epoch_and_draws_again_in_the_next() {
        let lib = Library::open_in_memory().unwrap();
        let ids = synth(&lib, 12, 2, 6);
        let now = 400 * DAY_MS + 13 * 3_600_000;
        let mut k = 0i64;
        for artist in &ids[..4] {
            for day in 0..6 {
                for t in &artist[0][..4] {
                    k += 1;
                    play(&lib, *t, now - (day + 1) * DAY_MS / 2 - k * 1000);
                }
            }
        }
        for artist in &ids[4..10] {
            for round in 0..4 {
                for t in &artist[0] {
                    k += 1;
                    play(&lib, *t, now - 60 * DAY_MS + round * DAY_MS - k * 1000);
                }
            }
        }
        for artist in &ids[..10] {
            lib.like(artist[0][5]).unwrap();
        }
        let profiles = one_profile(&lib);
        let visit = |run: u64| Visit { now, utc_offset_min: 0, epoch: Epoch::of(run, now, 0), profiles: Some(profiles.clone()), ..Visit::default() };
        let first = build(&lib, &visit(1));
        let (sc, again) = picks(&first);
        assert_eq!(sc.len(), SHORTCUTS, "{sc:?}");
        assert_eq!(again.len(), LISTEN_AGAIN, "{again:?}");
        assert_eq!(picks(&build(&lib, &visit(1))), (sc.clone(), again.clone()), "the next visit of the epoch");
        // A song played from «Слушать снова»: it stays for the epoch.
        play(&lib, again[0], now + 60_000);
        let later = Visit { now: now + 120_000, ..visit(1) };
        assert_eq!(picks(&build(&lib, &later)), (sc.clone(), again.clone()), "after a play");

        let now_albums: Vec<String> = (0..4).map(|a| format!("r Album {a}.0")).collect();
        let old_albums: Vec<String> = (4..10).map(|a| format!("r Album {a}.0")).collect();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut songs: HashSet<TrackId> = HashSet::new();
        let mut lineups: HashSet<Vec<String>> = HashSet::new();
        let mut before: Option<Vec<TrackId>> = None;
        let mut shared = Vec::new();
        for run in 2..32 {
            let h = build(&lib, &visit(run));
            let (sc, again) = picks(&h);
            println!("{run}: {sc:?}");
            assert_eq!(sc.len(), SHORTCUTS);
            assert_eq!(sc.iter().collect::<HashSet<_>>().len(), sc.len(), "no shortcut twice: {sc:?}");
            let narrow = &sc[..SHORTCUTS_NARROW];
            assert!(narrow.iter().any(|s| now_albums.contains(s)), "what plays now: {sc:?}");
            assert!(narrow.iter().any(|s| old_albums.contains(s)), "a forgotten favourite: {sc:?}");
            assert!(narrow.iter().any(|s| !now_albums.contains(s) && !old_albums.contains(s)), "what the library offers: {sc:?}");
            for s in &sc {
                *seen.entry(s.clone()).or_default() += 1;
            }
            if let Some(b) = &before {
                let same = again.iter().filter(|t| b.contains(t)).count();
                assert!(same <= LISTEN_AGAIN * 2 / 3, "«Слушать снова» shares {same} with the epoch before");
                shared.push(same);
            }
            songs.extend(again.iter().copied());
            before = Some(again);
            lineups.insert(sc);
        }
        assert!(lineups.len() >= 8, "{lineups:?}");
        assert!(shared.iter().sum::<usize>() * 3 <= shared.len() * LISTEN_AGAIN, "«Слушать снова» shares {shared:?} with the epochs before");
        // The album of the song played above plays now and is no longer
        // forgotten: it may come or not.
        let replayed = (0..10).find(|a| ids[*a][0].contains(&first.listen_again[0].id)).map(|a| format!("r Album {a}.0"));
        for a in now_albums.iter().chain(&old_albums).filter(|a| replayed.as_ref() != Some(*a)) {
            assert!(seen.contains_key(a), "{a} never came: {seen:?}");
        }
        // Every song liked or played much long ago (four of six of each
        // forgotten album, the liked of all) comes at least once.
        let mut pool: HashSet<TrackId> = (4..10).flat_map(|a| ids[a][0].iter().copied()).chain((0..4).map(|a| ids[a][0][5])).collect();
        pool.remove(&again[0]);
        let missing: Vec<&TrackId> = pool.iter().filter(|t| !songs.contains(t)).collect();
        assert!(missing.is_empty(), "{} of {} never came", missing.len(), pool.len());
    }

    /// Nothing heard, nothing liked: the shortcuts are the recently added
    /// and the library, other ones in each epoch; nothing to hear again.
    #[test]
    fn a_cold_start_has_shortcuts_of_the_library() {
        let lib = Library::open_in_memory().unwrap();
        synth(&lib, 6, 3, 5);
        let now = 400 * DAY_MS;
        let mut lineups = HashSet::new();
        for run in 0..6 {
            let visit = Visit { now, epoch: Epoch::of(run, now, 0), ..Visit::default() };
            let h = build(&lib, &visit);
            let (sc, again) = picks(&h);
            assert!(!h.history && again.is_empty());
            assert_eq!(sc.len(), SHORTCUTS, "{sc:?}");
            assert_eq!(sc.iter().collect::<HashSet<_>>().len(), sc.len(), "{sc:?}");
            lineups.insert(sc);
        }
        assert!(lineups.len() >= 4, "{lineups:?}");
    }

    /// One artist on repeat, two albums: the few places there are, each
    /// once, nothing breaks; whatever was played these days does not come
    /// to «Слушать снова».
    #[test]
    fn a_tiny_library_on_repeat() {
        let lib = Library::open_in_memory().unwrap();
        let ids = synth(&lib, 1, 2, 5);
        let now = 400 * DAY_MS;
        for k in 0..300 {
            play(&lib, ids[0][(k / 5) % 2][k % 5], now - DAY_MS + k as i64 * 60_000);
        }
        let profiles = one_profile(&lib);
        for run in 0..5 {
            let visit = Visit { now: now + DAY_MS / 2, epoch: Epoch::of(run, now, 0), profiles: Some(profiles.clone()), ..Visit::default() };
            let h = build(&lib, &visit);
            let (sc, again) = picks(&h);
            assert!((1..=3).contains(&sc.len()), "{sc:?}");
            assert_eq!(sc.iter().collect::<HashSet<_>>().len(), sc.len(), "{sc:?}");
            assert!(again.is_empty(), "{again:?}");
        }
    }

    /// One genre, one profile, one album on repeat lately, the rest of the
    /// library never touched: what the library offers changes with the
    /// epochs and every artist of it comes in time, as itself or its album.
    #[test]
    fn one_genre_is_offered_the_rest_of_its_library() {
        let lib = Library::open_in_memory().unwrap();
        let ids = synth(&lib, 16, 1, 6);
        let now = 400 * DAY_MS;
        for k in 0..40 {
            play(&lib, ids[0][0][k % 6], now - DAY_MS - k as i64 * 3_600_000);
        }
        let profiles = one_profile(&lib);
        let mut offered: HashSet<String> = HashSet::new();
        // Twenty-five: the ids, and so the lots, are new at each run of the
        // test, and ten epochs left an artist out now and then.
        for run in 0..25 {
            let visit = Visit { now, epoch: Epoch::of(run, now, 0), profiles: Some(profiles.clone()), ..Visit::default() };
            let (sc, _) = picks(&build(&lib, &visit));
            assert_eq!(sc.iter().collect::<HashSet<_>>().len(), sc.len(), "{sc:?}");
            assert!(sc.contains(&"r Album 0.0".to_owned()), "what plays now: {sc:?}");
            offered.extend(sc);
        }
        for a in 1..16 {
            assert!(offered.contains(&format!("a Artist {a}")) || offered.contains(&format!("r Album {a}.0")), "Artist {a}: {offered:?}");
        }
    }
}
