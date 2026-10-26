//! Synthetic listeners (profiles-v1 §2.4, `scripts/synth.py`): users unlike
//! the real one, the truth known by construction. The general method finds
//! about as many taste profiles as there are kinds of songs played, tells
//! the background sessions apart, and keeps the four ways of listening in
//! the background apart without one rule breaking another: never in the
//! background, the background of one's ordinary songs, of lo-fi, and both;
//! and their «Миксы для тебя» make sense for each.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventContext, EventOrigin, EventType, ReleaseId, TrackId, UnixMs};
use nikgob_recs::background::{BackgroundScore, BackgroundTrack, Basic};
use nikgob_recs::{Catalog, DIM, Lang, ModelInput, Play, SongType, SongTypes, TasteInput, TrackInfo, UserModel, VectorIndex, Voice};
use uuid::Uuid;

const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;
const T0: UnixMs = 1_780_000_000_000;

/// xorshift64*: the same listeners on every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    fn u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn f32(&mut self) -> f32 {
        (self.u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn below(&mut self, n: usize) -> usize {
        (self.u64() % n.max(1) as u64) as usize
    }
    fn range(&mut self, a: usize, b: usize) -> usize {
        a + self.below(b - a + 1)
    }
    fn gauss(&mut self, mean: f32, sd: f32) -> f32 {
        let g: f32 = (0..12).map(|_| self.f32()).sum::<f32>() - 6.0;
        mean + sd * g
    }
}

/// One kind of songs: how many, its type, how much it is chosen, and
/// where its sound is.
#[derive(Clone)]
struct Style {
    n: usize,
    lang: Option<Lang>,
    voice: Voice,
    ost: bool,
    taste: f32,
    /// Near the sound of another style.
    near: Option<usize>,
    /// The words of its album (the background rule reads them).
    album: &'static str,
}

fn style(n: usize, lang: Option<Lang>, taste: f32) -> Style {
    let voice = if lang.is_some() { Voice::Sung } else { Voice::Instrumental };
    Style { n, lang, voice, ost: false, taste, near: None, album: "Album" }
}

fn ost(n: usize, taste: f32) -> Style {
    Style { ost: true, album: "Original Game Soundtrack", ..style(n, None, taste) }
}

fn lofi(n: usize, taste: f32) -> Style {
    Style { album: "lofi beats to study to", ..style(n, None, taste) }
}

/// How the history goes.
#[derive(Clone)]
struct Habits {
    days: i64,
    per_day: f32,
    /// A session is background this often, of one of `bg_styles`.
    p_bg: f32,
    bg_styles: Vec<usize>,
    never_skips: bool,
    playlists_only: bool,
    /// The window's state is logged.
    window: bool,
    /// Background with the window in view and some touches.
    bg_visible: bool,
    /// The types are known (else no language and no voice: a language the
    /// detector cannot tell).
    lang_known: bool,
}

impl Default for Habits {
    fn default() -> Self {
        Self { days: 90, per_day: 1.6, p_bg: 0.0, bg_styles: Vec::new(), never_skips: false, playlists_only: false, window: true, bg_visible: false, lang_known: true }
    }
}

struct Listener {
    catalog: Catalog,
    index: VectorIndex<TrackId>,
    types: SongTypes,
    input: TasteInput,
    /// The style of each track, by catalog position.
    style_of: Vec<usize>,
    albums: Vec<&'static str>,
    /// Each session's first start and whether it was background.
    truth: Vec<(UnixMs, bool)>,
    now: UnixMs,
}

fn tid(i: usize) -> TrackId {
    TrackId(Uuid::from_u128(i as u128 + 1))
}

fn listener(seed: u64, styles: &[Style], h: &Habits) -> Listener {
    let mut rng = Rng::new(seed);
    let mut centres: Vec<[f32; DIM]> = Vec::new();
    for s in styles {
        let c: [f32; DIM] = match s.near {
            Some(o) => std::array::from_fn(|d| rng.gauss(centres[o][d], 0.45)),
            None => std::array::from_fn(|_| rng.gauss(0.0, 1.0)),
        };
        centres.push(c);
    }
    let (mut tracks, mut style_of, mut albums, mut durations) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut index = VectorIndex::new();
    let mut types = SongTypes::default();
    for (si, s) in styles.iter().enumerate() {
        let artists: Vec<ArtistId> = (0..6).map(|a| ArtistId(Uuid::from_u128(((si as u128) << 32) | a))).collect();
        for k in 0..s.n {
            let i = tracks.len();
            let dur = (rng.f32() * 210.0 + 120.0) as u32 * 1000;
            let release = ReleaseId(Uuid::from_u128(((si as u128) << 32) | (k as u128 / 10) | (1 << 100)));
            let world = format!("style {si} world {}", k / 25);
            tracks.push(TrackInfo::new(tid(i), vec![artists[rng.below(6)]], Some(release)).with_duration(Some(dur)).with_world(&world));
            index.insert(tid(i), std::array::from_fn(|d| rng.gauss(centres[si][d], 0.7)));
            let ty = if h.lang_known {
                SongType { voice: s.voice, lang: s.lang, ost: s.ost, ..SongType::default() }
            } else {
                SongType { ost: s.ost, ..SongType::default() }
            };
            types.by_track.insert(tid(i), ty);
            style_of.push(si);
            albums.push(s.album);
            durations.push(i64::from(dur));
        }
    }
    let first_of: Vec<usize> = (0..styles.len()).map(|si| style_of.iter().position(|s| *s == si).unwrap()).collect();
    let active: Vec<usize> = (0..styles.len()).filter(|&si| styles[si].taste > 0.0).collect();
    let total: f32 = active.iter().map(|&si| styles[si].taste).sum();
    let pick_style = |rng: &mut Rng| {
        let mut x = rng.f32() * total;
        for &si in &active {
            x -= styles[si].taste;
            if x <= 0.0 {
                return si;
            }
        }
        *active.last().unwrap()
    };
    let (mut events, mut truth) = (Vec::new(), Vec::new());
    let mut last_end = 0;
    for d in 0..h.days {
        let n_s = rng.gauss(h.per_day, 0.9).round().max(0.0) as usize;
        let mut hours: Vec<f32> = (0..n_s).map(|_| 7.0 + rng.f32() * 19.0).collect();
        hours.sort_by(f32::total_cmp);
        for hr in hours {
            let mut ts = T0 + d * DAY + (hr * HOUR as f32) as i64;
            ts = ts.max(last_end + 2 * HOUR);
            let bg = !h.bg_styles.is_empty() && rng.f32() < h.p_bg;
            let (st, n, p_skip, p_manual, p_seek, hidden) = if bg {
                let st = vec![h.bg_styles[rng.below(h.bg_styles.len())]];
                if h.bg_visible {
                    (st, rng.range(8, 45), 0.06, 0.05, 0.03, rng.gauss(0.15, 0.1))
                } else {
                    (st, rng.range(14, 45), 0.02, 0.0, 0.0, rng.gauss(0.85, 0.15))
                }
            } else {
                let k = if rng.f32() < 0.55 { 1 } else { rng.range(2, 4) };
                let st: Vec<usize> = (0..k).map(|_| pick_style(&mut rng)).collect();
                let p_skip = if h.never_skips { 0.0 } else if h.playlists_only { 0.3 } else { 0.16 };
                let p_manual = if h.playlists_only { 0.0 } else { 0.3 };
                let p_seek = if h.never_skips { 0.0 } else { 0.08 };
                let hidden = if rng.f32() < 0.2 { rng.gauss(0.7, 0.2) } else { rng.gauss(0.15, 0.1) };
                (st, rng.range(4, 22), p_skip, p_manual, p_seek, hidden)
            };
            let hidden = hidden.clamp(0.0, 1.0);
            truth.push((ts, bg));
            let mut si = 0;
            for k in 0..n {
                if st.len() > 1 && rng.f32() < 0.25 {
                    si = rng.below(st.len());
                }
                let s = st[si];
                let pool = styles[s].n;
                let fav = (pool * 2 / 5).max(1);
                let i = first_of[s] + if rng.f32() < 0.75 { rng.below(fav) } else { rng.below(pool) };
                let skip = rng.f32() < p_skip;
                let manual = k == 0 || rng.f32() < p_manual;
                let origin = if manual {
                    EventOrigin::Manual
                } else if h.playlists_only {
                    EventOrigin::Queue
                } else {
                    EventOrigin::Autoplay
                };
                let context = if h.playlists_only { EventContext::Playlist } else { EventContext::Library };
                events.push(Play { origin: Some(origin), context, ..Play::new(tid(i), ts, EventType::Start, Some(0)) });
                if rng.f32() < p_seek {
                    events.push(Play::new(tid(i), ts + 30_000, EventType::Seek, Some(60_000)));
                }
                let attention = h.window.then_some(hidden);
                let (end, kind, pos) = if skip { (ts + 20_000, EventType::Skip, 20_000) } else { (ts + durations[i], EventType::Complete, durations[i] as u32) };
                events.push(Play { attention, ..Play::new(tid(i), end, kind, Some(pos)) });
                ts = end;
            }
            last_end = ts;
        }
    }
    let now = last_end + HOUR;
    let input = TasteInput { events, ..TasteInput::default() };
    Listener { catalog: Catalog::new(tracks), index, types, input, style_of, albums, truth, now }
}

struct Found {
    model: UserModel,
    /// Profiles with a majority style, and the styles with plays that lead
    /// one.
    profiles: usize,
    styles_led: usize,
    /// Share of the tracks with plays whose profile's majority is their
    /// own style.
    purity: f32,
    auc: Option<f32>,
}

fn run(l: &Listener) -> Found {
    let model = UserModel::build(&l.catalog, &l.input, ModelInput { index: &l.index, types: Some(&l.types), weights: None }, l.now);
    let played: HashSet<usize> = l.input.events.iter().filter_map(|p| l.catalog.position(p.track)).collect();
    let mut count: HashMap<usize, HashMap<usize, usize>> = HashMap::new();
    for &i in &played {
        if let Some(j) = model.profile_at(i) {
            *count.entry(j).or_default().entry(l.style_of[i]).or_default() += 1;
        }
    }
    let major: HashMap<usize, usize> =
        count.iter().map(|(j, c)| (*j, *c.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).unwrap().0)).collect();
    let pure = played.iter().filter(|&&i| model.profile_at(i).and_then(|j| major.get(&j)) == Some(&l.style_of[i])).count();
    let led: HashSet<usize> = major.values().copied().collect();
    // The background score of each session against the truth.
    let truth: HashMap<UnixMs, bool> = l.truth.iter().copied().collect();
    let scored: Vec<(f32, bool)> = model.listening.sessions.iter().filter_map(|s| Some((s.score, *truth.get(&s.start)?))).collect();
    let (pos, neg): (Vec<&(f32, bool)>, Vec<&(f32, bool)>) = scored.iter().partition(|x| x.1);
    let auc = (!pos.is_empty() && !neg.is_empty()).then(|| {
        let mut w = 0.0f32;
        for p in &pos {
            for q in &neg {
                w += if p.0 > q.0 { 1.0 } else if p.0 == q.0 { 0.5 } else { 0.0 };
            }
        }
        w / (pos.len() * neg.len()) as f32
    });
    Found { profiles: model.profiles.len(), styles_led: led.len(), purity: pure as f32 / played.len().max(1) as f32, auc, model }
}

/// The home's cards of `found`, with the basic background rule.
fn home(l: &Listener, found: &Found) -> nikgob_recs::HomeProfiles {
    found.model.home_profiles(6, nikgob_recs::profiles::BG_CARDS, |i| {
        let song = l.types.get(l.catalog.tracks()[i].id).copied().unwrap_or_default();
        Basic.suits(&BackgroundTrack { song: &song, title: "Track", album: Some(l.albums[i]), artist: "Artist", playlists: &[] })
    })
}

/// The profile most of a style's tracks are in.
fn profile_of_style(l: &Listener, m: &UserModel, s: usize) -> Option<usize> {
    let mut c: HashMap<usize, usize> = HashMap::new();
    for i in (0..l.style_of.len()).filter(|&i| l.style_of[i] == s) {
        if let Some(j) = m.profile_at(i) {
            *c.entry(j).or_default() += 1;
        }
    }
    c.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|x| x.0)
}

fn report(name: &str, f: &Found) {
    println!(
        "{name}: profiles {}, styles leading one {}, purity {:.2}, background AUC {:?}, background sessions {} of {}, has background {}",
        f.profiles,
        f.styles_led,
        f.purity,
        f.auc,
        f.model.listening.background_sessions(),
        f.model.listening.sessions.len(),
        f.model.listening.has_background
    );
}

/// Seven kinds of users with background sessions: about one profile a style
/// played, each style its own, the background told apart (AUC ≥ 0.9).
#[test]
fn profiles_and_background_of_seven_kinds_of_users() {
    use Lang::*;
    let users: Vec<(&str, Vec<Style>, Habits)> = vec![
        (
            "omnivore",
            vec![
                style(80, Some(Russian), 1.0),
                style(70, Some(Japanese), 0.8),
                style(50, Some(English), 0.7),
                style(40, Some(Korean), 0.4),
                style(40, Some(Spanish), 0.3),
                ost(120, 0.6),
                style(80, None, 0.4),
            ],
            Habits { p_bg: 0.15, bg_styles: vec![5], ..Habits::default() },
        ),
        (
            "heavy_background",
            vec![lofi(200, 0.05), style(60, Some(English), 1.0), style(50, Some(Japanese), 0.6)],
            Habits { p_bg: 0.7, bg_styles: vec![0], per_day: 2.2, ..Habits::default() },
        ),
        (
            "playlists_only",
            vec![style(90, Some(English), 1.0), style(70, Some(Russian), 0.7), style(60, None, 0.5)],
            Habits { playlists_only: true, p_bg: 0.2, bg_styles: vec![2], ..Habits::default() },
        ),
        (
            "never_skips",
            vec![style(90, Some(Japanese), 1.0), style(90, None, 0.6), style(50, Some(English), 0.5)],
            Habits { never_skips: true, p_bg: 0.25, bg_styles: vec![1], ..Habits::default() },
        ),
        (
            "undetected_language",
            vec![style(90, Some(Other), 1.0), style(80, Some(Other), 0.7), style(70, None, 0.5)],
            Habits { p_bg: 0.2, bg_styles: vec![2], lang_known: false, ..Habits::default() },
        ),
        (
            "background_window_visible",
            vec![style(90, Some(English), 1.0), style(100, None, 0.3), style(60, Some(Russian), 0.6)],
            Habits { p_bg: 0.35, bg_styles: vec![1], bg_visible: true, ..Habits::default() },
        ),
        (
            "no_window_info",
            vec![style(90, Some(Russian), 1.0), ost(90, 0.5), style(60, Some(Japanese), 0.6)],
            Habits { p_bg: 0.25, bg_styles: vec![1], window: false, ..Habits::default() },
        ),
    ];
    for (k, (name, styles, habits)) in users.iter().enumerate() {
        let l = listener(11 + k as u64, styles, habits);
        let f = run(&l);
        report(name, &f);
        let n = styles.len();
        assert!(f.profiles >= n.saturating_sub(1) && f.profiles <= n + 2, "{name}: {} profiles of {n} styles", f.profiles);
        assert!(f.styles_led + 1 >= n, "{name}: {} styles lead a profile of {n}", f.styles_led);
        assert!(f.purity >= 0.9, "{name}: purity {}", f.purity);
        let auc = f.auc.expect("background and not");
        assert!(auc >= 0.9, "{name}: background AUC {auc}");
        assert!(f.model.listening.has_background, "{name}");
    }
}

/// Never in the background: two close kinds of one language and nothing
/// else. No background mode, no play counted less, no «Фон».
#[test]
fn never_background() {
    let mut near = style(60, Some(Lang::Russian), 0.5);
    near.near = Some(0);
    let l = listener(3, &[style(120, Some(Lang::Russian), 1.0), near], &Habits::default());
    let f = run(&l);
    report("never_background", &f);
    assert!(!f.model.listening.has_background);
    assert_eq!(f.model.listening.background_sessions(), 0);
    assert!(f.model.profile_bg.iter().all(|b| *b == 0.0), "{:?}", f.model.profile_bg);
    assert!(f.profiles <= 2, "{}", f.profiles);
    let h = home(&l, &f);
    assert!(h.background.iter().all(Vec::is_empty));
    assert_eq!(h.cards.iter().sum::<usize>(), 6);
}

/// The background of one's ordinary songs (English songs on while
/// working): the sessions are found and count less, yet the songs are no
/// background music, so no «Фон» card; their profile keeps its own cards.
#[test]
fn background_of_ordinary_songs() {
    let styles = [style(90, Some(Lang::English), 0.6), style(90, Some(Lang::Russian), 1.0), style(60, Some(Lang::Japanese), 0.5)];
    let l = listener(5, &styles, &Habits { p_bg: 0.35, bg_styles: vec![0], ..Habits::default() });
    let f = run(&l);
    report("background_of_ordinary_songs", &f);
    assert!(f.model.listening.has_background);
    assert!(f.auc.is_some_and(|a| a >= 0.9), "{:?}", f.auc);
    let en = profile_of_style(&l, &f.model, 0).unwrap();
    assert!(f.model.is_background_profile(en), "{:?}", f.model.profile_bg);
    // The skip model learnt that a song on in the background is no early
    // skip, the sound aside (these are songs either way).
    assert!(f.model.weights.skip.w[nikgob_recs::model::G_BACKGROUND] < -0.2, "{:?}", f.model.weights.skip.w);
    let h = home(&l, &f);
    assert!(h.background.iter().all(Vec::is_empty), "no «Фон» of songs");
    assert!(h.cards[en] >= 1, "{:?}", h.cards);
    for s in 1..3 {
        assert!(h.cards[profile_of_style(&l, &f.model, s).unwrap()] >= 1, "{:?}", h.cards);
    }
}

/// The background of lo-fi: up to three «Фон» cards of lo-fi only, split
/// by energy, and every active profile its card.
#[test]
fn background_of_lofi() {
    let styles = [lofi(200, 0.05), style(60, Some(Lang::English), 1.0), style(50, Some(Lang::Japanese), 0.6)];
    let l = listener(7, &styles, &Habits { p_bg: 0.7, bg_styles: vec![0], per_day: 2.2, ..Habits::default() });
    let f = run(&l);
    report("background_of_lofi", &f);
    let bg = profile_of_style(&l, &f.model, 0).unwrap();
    assert!(f.model.is_background_profile(bg));
    let h = home(&l, &f);
    let cards = &h.background[bg];
    assert!((1..=3).contains(&cards.len()) && cards.len() == h.cards[bg], "{:?} {:?}", h.cards, cards.len());
    assert!(cards.iter().flatten().all(|&i| l.style_of[i] == 0), "lo-fi only");
    for s in 1..3 {
        assert!(h.cards[profile_of_style(&l, &f.model, s).unwrap()] >= 1, "{:?}", h.cards);
    }
    assert_eq!(h.cards.iter().sum::<usize>(), 6);
    // Counted less, the lo-fi takes less of the balance than of the plays.
    let off = {
        let mut input = l.input.clone();
        input.profiles.background_less = false;
        UserModel::build(&l.catalog, &input, ModelInput { index: &l.index, types: Some(&l.types), weights: None }, l.now)
    };
    let j = profile_of_style(&l, &off, 0).unwrap();
    assert!(f.model.profile_share[bg] + 0.1 < off.profile_share[j], "{} vs {}", f.model.profile_share[bg], off.profile_share[j]);
}

/// Both: lo-fi and one's own songs in the background. The lo-fi makes
/// «Фон», the songs stay a profile of songs with their cards.
#[test]
fn background_mixed() {
    let styles = [lofi(150, 0.05), style(90, Some(Lang::English), 0.6), style(90, Some(Lang::Russian), 1.0)];
    let l = listener(9, &styles, &Habits { p_bg: 0.45, bg_styles: vec![0, 1], per_day: 2.0, ..Habits::default() });
    let f = run(&l);
    report("background_mixed", &f);
    assert!(f.model.listening.has_background);
    let of = |s| profile_of_style(&l, &f.model, s).unwrap();
    let (lo, en, ru) = (of(0), of(1), of(2));
    assert!(lo != en && en != ru && lo != ru);
    let h = home(&l, &f);
    assert!(!h.background[lo].is_empty(), "«Фон» of lo-fi: {:?}", h.cards);
    assert!(h.background[en].is_empty() && h.cards[en] >= 1, "{:?}", h.cards);
    assert!(h.cards[ru] >= 1);
    assert!(h.background.iter().flatten().flatten().all(|&i| l.style_of[i] == 0));
}

/// «Миксы для тебя» of the same synthetic listeners (profiles-v1 §15 P5,
/// recs-v2 §15 phase 3): sensible for each kind of user.
mod mixes {
    use std::collections::{HashMap, HashSet};

    use nikgob_recs::background::{BackgroundScore, BackgroundTrack, Basic};
    use nikgob_recs::mixes::{Builder, Kept, MIN_SONGS, MOMENTS, Mix, MixKey, Settings};
    use nikgob_recs::{DayPart, Lang, Live, Recs, UserModel};

    use super::*;

    /// The lineup of `l` (the basic background rule) and its builder's
    /// other mixes by `f`.
    fn lineup<T>(l: &Listener, m: &UserModel, f: impl FnOnce(&Builder<'_>, Vec<Mix>) -> T) -> T {
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
        let suits = |i: usize| {
            let song = l.types.get(l.catalog.tracks()[i].id).copied().unwrap_or_default();
            Basic.suits(&BackgroundTrack { song: &song, title: "Track", album: Some(l.albums[i]), artist: "Artist", playlists: &[] })
        };
        let day = m.now().div_euclid(DAY);
        let b = Builder::new(&recs, m, &|_| true, &suits, day, DayPart::Day, Live::default(), Settings::default());
        let (mixes, _) = b.lineup(&Kept::default());
        f(&b, mixes)
    }

    /// Nothing empty, no two alike, none of one world, no song twice.
    fn sensible(name: &str, l: &Listener, mixes: &[Mix]) {
        assert!(!mixes.is_empty(), "{name}: no mixes");
        assert!(mixes.len() <= MOMENTS, "{name}: {}", mixes.len());
        let titles: HashSet<&str> = mixes.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles.len(), mixes.len(), "{name}: {:?}", mixes.iter().map(|m| &m.title).collect::<Vec<_>>());
        for m in mixes {
            assert!(m.tracks.len() >= MIN_SONGS && !m.title.is_empty(), "{name}: «{}» of {}", m.title, m.tracks.len());
            let set: HashSet<usize> = m.tracks.iter().copied().collect();
            assert_eq!(set.len(), m.tracks.len(), "{name}: «{}» has a song twice", m.title);
            let worlds: HashSet<u64> = m.tracks.iter().filter_map(|&i| l.catalog.tracks()[i].world).collect();
            assert!(worlds.len() >= 3, "{name}: «{}» of {} worlds", m.title, worlds.len());
        }
        println!("{name}: {}", mixes.iter().map(|m| format!("«{}» {}", m.title, m.tracks.len())).collect::<Vec<_>>().join(", "));
    }

    fn sd(x: &[f32]) -> f32 {
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        (x.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// The longest run of one profile in `mix`.
    fn longest_run(m: &UserModel, mix: &[usize]) -> usize {
        let (mut best, mut run) = (1, 1);
        for w in mix.windows(2) {
            run = if m.profile_at(w[0]) == m.profile_at(w[1]) { run + 1 } else { 1 };
            best = best.max(run);
        }
        best
    }

    fn omnivore() -> Listener {
        use Lang::*;
        let styles = vec![
            style(80, Some(Russian), 1.0),
            style(70, Some(Japanese), 0.8),
            style(50, Some(English), 0.7),
            style(40, Some(Korean), 0.4),
            ost(120, 0.6),
            style(80, None, 0.4),
        ];
        listener(21, &styles, &Habits { p_bg: 0.15, bg_styles: vec![4], ..Habits::default() })
    }

    /// «Фон» stays steady: its energy spreads less than half as much as an
    /// ordinary mix of the same profile's.
    #[test]
    fn background_is_steady() {
        let styles = [lofi(200, 0.05), style(60, Some(Lang::English), 1.0), style(50, Some(Lang::Japanese), 0.6)];
        let l = listener(7, &styles, &Habits { p_bg: 0.7, bg_styles: vec![0], per_day: 2.2, ..Habits::default() });
        let f = run(&l);
        let m = &f.model;
        lineup(&l, m, |b, mixes| {
            sensible("heavy_background", &l, &mixes);
            let bg: Vec<&Mix> = mixes.iter().filter(|x| matches!(x.key, MixKey::Background { .. })).collect();
            assert!((1..=3).contains(&bg.len()), "{:?}", mixes.iter().map(|x| &x.title).collect::<Vec<_>>());
            let MixKey::Background { profile, .. } = bg[0].key else { unreachable!() };
            let active = b.mix(MixKey::Profile(profile), &Kept::default()).expect("the profile's own mix");
            let energy = |t: &[usize]| t.iter().map(|&i| m.energy_at(i).unwrap()).collect::<Vec<f32>>();
            for x in &bg {
                assert!(x.tracks.iter().all(|&i| l.style_of[i] == 0), "lo-fi only");
                let (calm, all) = (sd(&energy(&x.tracks)), sd(&energy(&active.tracks)));
                assert!(calm < 0.5 * all, "«{}» {calm} against {all}", x.title);
                let steps: Vec<f32> = x.tracks.windows(2).map(|w| (m.energy_at(w[0]).unwrap() - m.energy_at(w[1]).unwrap()).abs()).collect();
                let active_steps: Vec<f32> = active.tracks.windows(2).map(|w| (m.energy_at(w[0]).unwrap() - m.energy_at(w[1]).unwrap()).abs()).collect();
                assert!(steps.iter().sum::<f32>() / (steps.len() as f32) < active_steps.iter().sum::<f32>() / active_steps.len() as f32);
            }
        });
    }

    /// A blend of two profiles takes turns in pairs, half each.
    #[test]
    fn blend_alternates_blocks() {
        let l = omnivore();
        let f = run(&l);
        let m = &f.model;
        let key = |s: usize| m.profiles.list[profile_of_style(&l, m, s).unwrap()].key;
        let blend = MixKey::blend(&[key(0), key(1)]).unwrap();
        lineup(&l, m, |b, _| {
            let mix = b.mix(blend, &Kept::default()).expect("a blend");
            let of = |s: usize| mix.tracks.iter().filter(|&&i| m.profile_at(i) == profile_of_style(&l, m, s)).count();
            assert!(of(0) * 10 >= mix.tracks.len() * 4 && of(1) * 10 >= mix.tracks.len() * 4, "{} and {} of {}", of(0), of(1), mix.tracks.len());
            let runs: Vec<Option<usize>> = mix.tracks.iter().map(|&i| m.profile_at(i)).collect();
            assert!(longest_run(m, &mix.tracks) <= 3, "{runs:?}");
            let runs = mix.tracks.windows(2).filter(|w| m.profile_at(w[0]) != m.profile_at(w[1])).count();
            assert!(runs + 1 >= mix.tracks.len() / 3, "blocks of two take turns: {runs} changes in {}", mix.tracks.len());
            // Named by both: «Русские и японские песни».
            assert!(mix.title.contains(" + ") || mix.title.contains(" и "), "{}", mix.title);
        });
    }

    /// «Всё вперемешку» gives each active profile its share with a floor,
    /// in blocks of three or four.
    #[test]
    fn everything_follows_shares() {
        let l = omnivore();
        let f = run(&l);
        let m = &f.model;
        lineup(&l, m, |b, mixes| {
            sensible("omnivore", &l, &mixes);
            let mix = b.mix(MixKey::Everything, &Kept::default()).expect("everything");
            let mut count: HashMap<usize, usize> = HashMap::new();
            for &i in &mix.tracks {
                *count.entry(m.profile_at(i).unwrap()).or_default() += 1;
            }
            let active: Vec<usize> = (0..m.profiles.len()).filter(|&j| m.profile_share[j] >= nikgob_recs::profiles::ACTIVE_SHARE).collect();
            let shares: Vec<f32> = active.iter().map(|&j| m.profile_share[j]).collect();
            let want = nikgob_recs::profiles::quota(&shares, nikgob_recs::mixes::MIX_LEN, &vec![false; active.len()], nikgob_recs::mixes::MIX_LEN);
            for (k, &j) in active.iter().enumerate() {
                let got = count.get(&j).copied().unwrap_or(0);
                assert!(got.abs_diff(want[k]) <= 1, "profile {j}: {got} for {} ({count:?})", want[k]);
            }
            assert!(longest_run(m, &mix.tracks) <= 4, "{}", longest_run(m, &mix.tracks));
        });
    }

    /// One kind of song only: its own mix and its two sides, no «Всё
    /// вперемешку» of one profile.
    #[test]
    fn one_genre() {
        let l = listener(31, &[style(160, Some(Lang::Russian), 1.0)], &Habits::default());
        let f = run(&l);
        lineup(&l, &f.model, |_, mixes| {
            sensible("one_genre", &l, &mixes);
            assert!(mixes.iter().all(|m| m.key != MixKey::Everything));
            assert!(mixes.iter().any(|m| matches!(m.key, MixKey::Profile(_))));
            assert!(mixes.iter().any(|m| m.key == MixKey::Calm) && mixes.iter().any(|m| m.key == MixKey::Lively));
        });
    }

    /// Never in the background: no «Фон», whatever is calm.
    #[test]
    fn never_background_has_no_background_mix() {
        let styles = [style(120, Some(Lang::Russian), 1.0), ost(100, 0.5), style(60, Some(Lang::Japanese), 0.5)];
        let l = listener(33, &styles, &Habits::default());
        let f = run(&l);
        assert!(!f.model.listening.has_background);
        lineup(&l, &f.model, |_, mixes| {
            sensible("never_background", &l, &mixes);
            assert!(mixes.iter().all(|m| !matches!(m.key, MixKey::Background { .. })));
            assert_eq!(mixes.len(), MOMENTS);
        });
    }

    /// Nothing heard yet: the library's kinds and its two sides, something
    /// to start from.
    #[test]
    fn cold_start() {
        let styles = [style(90, Some(Lang::Russian), 1.0), ost(90, 0.5), style(60, Some(Lang::English), 0.5)];
        let mut l = listener(35, &styles, &Habits::default());
        l.input.events.clear();
        let f = run(&l);
        assert_eq!(f.model.profiles.len(), 0);
        lineup(&l, &f.model, |_, mixes| {
            sensible("cold_start", &l, &mixes);
            assert!(mixes.len() >= 3, "{}", mixes.len());
            assert!(mixes.iter().any(|m| matches!(m.key, MixKey::Kind(_))));
        });
    }
}

