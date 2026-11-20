use nikgob_core::model::EventContext;

use super::replay::best_n;
use super::stats::*;
use super::*;
use crate::rng::Rng;
use crate::test_util::*;
use crate::taste::HOUR_MS;

pub(crate) const T0: UnixMs = 20_000 * DAY_MS;
const ALBUM: u128 = 10;

/// A fast skip of a loved track in a radio is «not now»: nothing
/// against it in the long run; the same skip of the user's own pick
/// counts.
#[test]
fn not_now_costs_a_loved_track_nothing_in_the_long_run() {
    let cat = Catalog::new((0..2u128).map(|i| info(i, &[1], Some(i)).with_duration(Some(200_000))));
    let idx = VectorIndex::new();
    let mut events = Vec::new();
    let mut ts = T0 - 10 * DAY_MS;
    for _ in 0..5 {
        for t in 0..2u128 {
            events.push(Play::new(tid(t), ts, EventType::Start, Some(0)));
            ts += 200_000;
            events.push(Play::new(tid(t), ts, EventType::Complete, Some(200_000)));
        }
        ts += HOUR_MS;
    }
    let skip = |t: u128, origin: EventOrigin, context: EventContext, ts: UnixMs| {
        [
            Play { context, origin: Some(origin), ..Play::new(tid(t), ts, EventType::Start, Some(0)) },
            Play { context, ..Play::new(tid(t), ts + 4_000, EventType::Skip, Some(4_000)) },
        ]
    };
    events.extend(skip(1, EventOrigin::Manual, EventContext::Playlist, T0 - DAY_MS));
    events.extend(skip(0, EventOrigin::Radio, EventContext::Radio, T0 - DAY_MS / 2));
    let build = |events: Vec<Play>| {
        UserModel::build(&cat, &TasteInput { events, ..TasteInput::default() }, ModelInput { index: &idx, types: None, weights: None, graph: None }, T0)
    };
    let m = build(events.clone());
    assert!(m.tracks[0].aff_long(T0) > m.tracks[1].aff_long(T0) + 0.005, "{} {}", m.tracks[0].aff_long(T0), m.tracks[1].aff_long(T0));
    assert_eq!((m.tracks[0].early, m.tracks[1].early), (0, 1));
    // The skip model learns from the user's own skip, not from «not now»:
    // as if that skip never was.
    let without: Vec<Play> = events.iter().filter(|p| !(p.track == tid(0) && p.kind == EventType::Skip)).cloned().collect();
    let w = build(without);
    assert_eq!(m.weights.skip.seen, w.weights.skip.seen);
    assert_eq!(m.weights.skip.w, w.weights.skip.w);
}

/// A listener of albums: 30 albums of ten tracks in ten worlds of three
/// (each world its own sound), sessions of one world's albums in order,
/// a favourite pair (after track 5 of an album often its track 9), a
/// skip now and then, `days` of it.
pub(crate) fn album_listener(days: i64, seed: u64) -> (Catalog, VectorIndex<TrackId>, TasteInput) {
    let mut rng = Rng::new(seed);
    let cat = Catalog::new((0..300u128).map(|i| {
        let album = i / ALBUM;
        info(i, &[(album / 3) as u32], Some(album))
            .with_world(&format!("w{}", album / 3))
            .with_position((i % ALBUM) as u32 + 1)
            .with_duration(Some(200_000))
    }));
    let mut idx = VectorIndex::with_weights([1.0; crate::DIM]);
    for i in 0..300u128 {
        let w = (i / ALBUM / 3) as f32;
        idx.insert(tid(i), point(w * 3.0 + rng.f32(), (w * 7.0) % 5.0 + rng.f32()));
    }
    let mut events = Vec::new();
    let mut ts = T0 - days * DAY_MS;
    while ts < T0 {
        let world = if rng.f32() < 0.6 { rng.below(3) as u128 } else { rng.below(10) as u128 };
        let mut album = world * 3 + rng.below(3) as u128;
        let mut track = rng.below(4) as u128;
        let mut ctx = EventContext::Album;
        for _ in 0..(4 + rng.below(8)) {
            let id = tid(album * ALBUM + track);
            events.push(Play { context: ctx, ..Play::new(id, ts, EventType::Start, Some(0)) });
            let early = rng.f32() < 0.1;
            ts += if early { 10_000 } else { 200_000 };
            let (kind, pos) = if early { (EventType::Skip, 10_000) } else { (EventType::Complete, 200_000) };
            events.push(Play { context: ctx, ..Play::new(id, ts, kind, Some(pos)) });
            ctx = EventContext::Queue;
            track = if track == 4 && rng.f32() < 0.7 { 8 } else { track + 1 };
            if track >= ALBUM {
                album = world * 3 + (album + 1) % 3;
                track = 0;
                ctx = EventContext::Album;
            }
        }
        ts += (2 + rng.below(10) as i64) * HOUR_MS;
    }
    (cat, idx, TasteInput { events, ..TasteInput::default() })
}

pub(crate) fn input(idx: &VectorIndex<TrackId>) -> ModelInput<'_> {
    ModelInput { index: idx, types: None, weights: None, graph: None }
}

/// The best of a list in linear time keeps what sorting it does.
#[test]
fn best_n_keeps_what_sorting_keeps() {
    let mut rng = Rng::new(5);
    for n in [0usize, 1, 7, 200, 500] {
        let v: Vec<(f32, usize)> = (0..300).map(|i| ((rng.below(50) as f32) / 7.0, i)).collect();
        let cmp = |x: &(f32, usize), y: &(f32, usize)| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1));
        let mut sorted = v.clone();
        sorted.sort_by(cmp);
        let mut want: Vec<usize> = sorted.into_iter().take(n).map(|x| x.1).collect();
        let mut got: Vec<usize> = best_n(v, n, cmp).into_iter().map(|x| x.1).collect();
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, want, "{n}");
    }
}

/// A build learning the weights anew steps on the latest
/// [`TEACH_LAST`] starts only: a long history does not take minutes;
/// a short one teaches whole.
#[test]
fn a_rebuild_teaches_the_latest_starts() {
    let (cat, idx, history) = album_listener(150, 6);
    let starts = history.events.iter().filter(|p| p.kind == EventType::Start).count();
    assert!(starts > 2 * TEACH_LAST, "{starts}");
    let m = UserModel::build(&cat, &history, input(&idx), T0);
    assert!(m.weights.next.seen as usize <= TEACH_LAST && m.weights.next.seen as usize > TEACH_LAST / 2, "{}", m.weights.next.seen);
    assert!(m.weights.next.w[F_NEXT_ALBUM] > HAND[F_NEXT_ALBUM], "{:?}", m.weights.next.w);
    // Every start is still in the stats and the pairs.
    assert_eq!(m.tracks.iter().map(|t| t.starts as usize).sum::<usize>(), starts);
    let (cat, idx, short) = album_listener(20, 6);
    let teaching = short.events.iter().filter(|p| p.kind == EventType::Start).count();
    assert!(teaching < TEACH_LAST);
    assert_eq!(teach_from(&short.events), UnixMs::MIN);
    assert!(UserModel::build(&cat, &short, input(&idx), T0).weights.next.seen > 200);
}

/// The features of the session: the track playing now again, the first
/// track of another album, what was listened together with the
/// session's tracks before.
#[test]
fn repeats_album_starts_and_listened_together() {
    let (cat, idx, history) = album_listener(20, 7);
    let m = UserModel::build(&cat, &history, input(&idx), T0);
    // Track 5 of album 3 playing.
    let now = T0 + DAY_MS;
    let playing = tid(3 * ALBUM + 4);
    let mut session = Session::default();
    session.start(playing, now - 100_000, EventContext::Album, Some(EventOrigin::Manual));
    let mo = m.moment(&session, now);
    let at = |i: u128| m.position(tid(i)).unwrap();
    let f = |i: u128| m.features(&mo, at(i));
    assert_eq!(f(3 * ALBUM + 4)[F_IS_PREV], 1.0);
    assert_eq!(f(3 * ALBUM + 5)[F_IS_PREV], 0.0);
    assert_eq!(f(4 * ALBUM)[F_ALBUM_FIRST], 1.0, "another album's first");
    assert_eq!(f(3 * ALBUM)[F_ALBUM_FIRST], 0.0, "the playing album's own first");
    assert_eq!(f(4 * ALBUM + 1)[F_ALBUM_FIRST], 0.0);
    // Listened together, either order: the tracks of its sessions, its
    // own album's and the favourite pair's; never one of another world
    // it never shared a session with.
    let most = (0..m.len()).max_by_key(|&i| (m.bag.row(i).len(), i)).unwrap();
    let mut session = Session::default();
    session.start(m.id(most), now - 100_000, EventContext::Album, Some(EventOrigin::Manual));
    let mo = m.moment(&session, now);
    let row = m.bag.row(most);
    assert!(row.len() >= 5, "{row:?}");
    assert!(row.iter().all(|&(c, w)| w > 0.0 && m.features(&mo, c as usize)[F_SESS_CO] > 0.0));
    assert!(row.iter().any(|&(c, _)| m.world_at(c as usize) == m.world_at(most)));
    let never_with = (0..m.len()).find(|&i| i != most && m.bag.get(most, i) == 0.0 && m.world_at(i) != m.world_at(most)).unwrap();
    assert_eq!(m.features(&mo, never_with)[F_SESS_CO], 0.0);
}

#[test]
fn learns_next_album() {
    let (cat, idx, history) = album_listener(20, 1);
    let m = UserModel::build(&cat, &history, input(&idx), T0);
    assert!(m.weights.next.seen > 200, "{}", m.weights.next.seen);
    assert!(m.weights.next.w[F_NEXT_ALBUM] > HAND[F_NEXT_ALBUM], "{:?}", m.weights.next.w);
    // Cached weights are kept as they are, the stats rebuilt.
    let again = UserModel::build(&cat, &history, ModelInput { weights: Some(m.weights), ..input(&idx) }, T0);
    assert_eq!(again.weights, m.weights);
    assert_eq!(again.tracks, m.tracks);
    // Weights of another feature version are learnt again.
    let old = Weights { version: FEATURE_VERSION + 1, ..Weights::default() };
    assert_eq!(UserModel::build(&cat, &history, ModelInput { weights: Some(old), ..input(&idx) }, T0).weights, m.weights);
}

/// The replay on the listener of albums (the replay test of a fixture;
/// the real library's is `runtime/tests/real_replay.rs`): the learnt
/// model finds the next start in its first ten in half of the starts at
/// least, far more often than the affinity of phase 1.
#[test]
fn prequential_beats_affinity() {
    let (cat, idx, history) = album_listener(15, 2);
    let r = replay(&cat, &history, input(&idx));
    let (learned, hand, today) = (r.get("all", "learned"), r.get("all", "hand"), r.get("all", "phase1_affinity"));
    assert!(learned.n > 300, "{learned:?}");
    assert!(learned.hit10 >= 0.5, "hit@10 {learned:?}");
    assert!(learned.hit10 > today.hit10 + 0.2 && learned.mrr > today.mrr, "{learned:?} against {today:?}");
    assert!(learned.mrr >= hand.mrr - 0.02, "{learned:?} against {hand:?}");
    // The favourite pair is learnt: what follows inside sessions.
    assert!(r.weights.next.w[F_COOC] > HAND[F_COOC], "{:?}", r.weights.next.w);
}

/// An early skip makes the next more likely to be skipped too.
#[test]
fn prev_early_predicts() {
    let mut rng = Rng::new(3);
    let cat = Catalog::new((0..200u128).map(|i| info(i, &[i as u32 % 20], Some(i / 10)).with_duration(Some(200_000))));
    let idx = VectorIndex::new();
    let mut events = Vec::new();
    let mut ts = T0 - 30 * DAY_MS;
    let mut prev_early = false;
    for k in 0..3_000u128 {
        if k % 12 == 0 {
            ts += 3 * HOUR_MS;
            prev_early = false;
        }
        let id = tid(rng.below(200) as u128);
        events.push(Play { context: EventContext::Playlist, ..Play::new(id, ts, EventType::Start, Some(0)) });
        let early = rng.f32() < if prev_early { 0.6 } else { 0.1 };
        ts += if early { 5_000 } else { 200_000 };
        let (kind, pos) = if early { (EventType::Skip, 5_000) } else { (EventType::Complete, 200_000) };
        events.push(Play::new(id, ts, kind, Some(pos)));
        prev_early = early;
    }
    let history = TasteInput { events, ..TasteInput::default() };
    let r = replay(&cat, &history, input(&idx));
    let w = r.weights.skip.w;
    assert!(w[21] > 0.5, "prev_early {w:?}");
    assert!(r.skip_auc.unwrap() > 0.65, "{:?}", r.skip_auc);
    let m = UserModel::build(&cat, &history, input(&idx), ts);
    assert!(m.weights.skip.seen > 2_000);
}

/// A listener of much rap and a few other kinds: a profile of each
/// kind, balanced by the user's own choices; hours of soundtracks the
/// app played on in the background raise them inside their profile but
/// hardly in the balance; a shelf of favourites shows the small kinds
/// at about their shares.
#[test]
fn profiles_balance_by_active_choices() {
    use crate::regions::spread;
    let (cat, idx, mut input, types) = crate::radio::tests::listener(true);
    let build = |input: &TasteInput| UserModel::build(&cat, input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, T0);
    let m = build(&input);
    assert!(m.regions.len() >= 4, "{:?}", m.regions.lanes);
    let kind = |i: usize| m.id(i).0.as_u128() / 1_000;
    let share_of = |m: &UserModel, k: u128| -> f32 {
        (0..m.regions.len())
            .filter(|&j| (0..m.len()).any(|i| m.profile_at(i) == Some(j) && kind(i) == k && m.tracks[i].starts > 0))
            .map(|j| m.profile_share[j])
            .sum()
    };
    let (rap, ost) = (share_of(&m, 0), share_of(&m, 1));
    assert!(rap > 0.45 && ost > 0.05, "rap {rap}, soundtracks {ost}");
    assert!(!m.listening.has_background, "sessions all alike: no background");
    // Three runs of a hundred soundtracks the app played on, on three
    // days, no action between.
    let ost_ids: Vec<TrackId> = (0..m.len()).filter(|&i| kind(i) == 1).map(|i| m.id(i)).collect();
    let runs = |origin: EventOrigin| {
        let mut out = Vec::new();
        for day in 1..=3 {
            let mut ts = T0 - day * DAY_MS;
            for k in 0..100 {
                let id = ost_ids[k % ost_ids.len()];
                out.push(Play { origin: Some(origin), ..Play::new(id, ts, EventType::Start, Some(0)) });
                ts += 200_000;
                out.push(Play::new(id, ts, EventType::Complete, Some(200_000)));
            }
        }
        out
    };
    let mut bg_input = input.clone();
    bg_input.events.extend(runs(EventOrigin::Autoplay));
    let bg = build(&bg_input);
    assert!(bg.listening.has_background && bg.listening.background_sessions() == 3, "{:?}", bg.listening.sessions.len());
    let (rap2, ost2) = (share_of(&bg, 0), share_of(&bg, 1));
    assert!(rap2 > ost2, "background: rap {rap2} soundtracks {ost2} (before {rap} {ost})");
    let j = bg.profile_at((0..bg.len()).find(|&i| kind(i) == 1).unwrap()).unwrap();
    assert!(bg.profile_bg[j] > 0.5, "a background profile: {:?}", bg.profile_bg);
    let i = (0..bg.len()).find(|&i| kind(i) == 1).unwrap();
    assert!(bg.tracks[i].aff_long(T0) > m.tracks[i].aff_long(T0), "counted inside its profile");
    // «Фон меньше влияет» off: the same runs count fully.
    bg_input.profiles.background_less = false;
    let full = share_of(&build(&bg_input), 1);
    assert!(full > ost2 + 0.15, "counted fully: {full} (down-weighted {ost2})");
    // The same soundtracks chosen one by one are listened to, not
    // background: an instrumental kind is never down-weighted as such.
    input.events.extend(runs(EventOrigin::Manual));
    let fg = build(&input);
    assert!(!fg.listening.has_background);
    let ost3 = share_of(&fg, 1);
    assert!(ost3 > ost2 + 0.15, "chosen soundtracks weigh: {ost3} (in the background {ost2})");

    // A shelf of ten favourites: every kind of the omnivore in it.
    let mut fav: Vec<usize> = (0..m.len()).filter(|&i| m.tracks[i].aff_long(T0) > 0.0).collect();
    fav.sort_by(|a, b| m.tracks[*b].aff_long(T0).total_cmp(&m.tracks[*a].aff_long(T0)));
    let items: Vec<(usize, Option<usize>)> = fav.iter().map(|&i| (i, m.profile_at(i))).collect();
    let shelf = spread(&items, &m.profile_share, 10);
    for k in 0..4 {
        assert!(shelf.iter().any(|&i| kind(i) == k), "kind {k} on the shelf: {:?}", shelf.iter().map(|&i| kind(i)).collect::<Vec<_>>());
    }
    // Within a profile its best loved stands at 1.
    let best = (0..m.len()).filter(|&i| kind(i) == 1).map(|i| m.profile_affinity(i)).fold(0.0f32, f32::max);
    assert!((best - 1.0).abs() < 1e-4, "{best}");
}

/// A thousand imported plays (another service's history) teach the
/// long affinity of their tracks and nothing else: no step of either
/// model, no pair, no world's presence, no transition, no hot
/// affinity, whether they are years old or amid the user's own; and a
/// replay scores the same starts. Years old, the weights come out about
/// the same (the affinity they gave has faded; their tracks only join
/// the candidates of a step).
#[test]
fn import_does_not_teach() {
    let (cat, idx, history) = album_listener(20, 4);
    let own = UserModel::build(&cat, &history, input(&idx), T0);
    let first = history.events.iter().map(|p| p.ts).min().unwrap();
    let with_imports = |from: UnixMs, step: i64| {
        let mut rng = Rng::new(9);
        let mut with = history.clone();
        for k in 0..1_000i64 {
            let id = tid(rng.below(300) as u128);
            let ts = from + k * step + rng.below(1000) as i64;
            let import = |kind, at, pos| Play { origin: Some(EventOrigin::Import), ..Play::new(id, at, kind, pos) };
            with.events.push(import(EventType::Start, ts, Some(0)));
            with.events.push(if k % 5 == 0 {
                import(EventType::Skip, ts + 10_000, Some(10_000))
            } else {
                import(EventType::Complete, ts + 200_000, Some(200_000))
            });
        }
        with
    };
    for (what, with) in [
        ("years old", with_imports(first - 3_000 * DAY_MS, 9 * HOUR_MS)),
        ("amid", with_imports(first, (T0 - first) / 1_000)),
    ] {
        let m = UserModel::build(&cat, &with, input(&idx), T0);
        assert_eq!((m.weights.next.seen, m.weights.skip.seen), (own.weights.next.seen, own.weights.skip.seen), "{what}");
        assert_eq!(m.co, own.co, "{what}: no pair");
        assert_eq!(m.worlds, own.worlds, "{what}: no world's presence");
        assert_eq!((m.lane_trans, &m.trans), (own.lane_trans, &own.trans), "{what}");
        let longer = (0..m.len()).filter(|&i| m.tracks[i].a_long > own.tracks[i].a_long + 1e-6).count();
        assert!(longer > 50, "{what}: the long affinity learnt ({longer})");
        for i in 0..m.len() {
            assert!((m.tracks[i].aff_hot(T0) - own.tracks[i].aff_hot(T0)).abs() < 1e-5, "{what}: hot {i}");
            assert_eq!((m.tracks[i].starts, m.tracks[i].early), (own.tracks[i].starts, own.tracks[i].early), "{what}");
        }
        let (a, b) = (replay(&cat, &history, input(&idx)), replay(&cat, &with, input(&idx)));
        assert_eq!((a.get("all", "learned").n, a.skip_test), (b.get("all", "learned").n, b.skip_test), "{what}");
        if what == "years old" {
            let close = |x: &[f32], y: &[f32]| x.iter().zip(y).all(|(a, b)| (a - b).abs() < 0.01);
            assert!(close(&m.weights.next.w, &own.weights.next.w) && close(&m.weights.skip.w, &own.weights.skip.w), "{:?}", m.weights);
        }
    }
}

/// The outcome weights: a late skip is a little plus, the app

/// closing on a track adds nothing, an early skip of the app's pick
/// weighs more than one of the user's own.
#[test]
fn outcomes_weigh_as_listened() {
    let cat = Catalog::new((0..4u128).map(|i| info(i, &[0], None).with_duration(Some(200_000))));
    let idx = VectorIndex::new();
    let start = |i: u128, o: EventOrigin, ts| Play { origin: Some(o), ..Play::new(tid(i), ts, EventType::Start, Some(0)) };
    let events = vec![
        start(0, EventOrigin::Manual, T0 - 100_000),
        Play::new(tid(0), T0 - 90_000, EventType::Skip, Some(5_000)),
        start(1, EventOrigin::Radio, T0 - 80_000),
        Play::new(tid(1), T0 - 70_000, EventType::Skip, Some(5_000)),
        start(2, EventOrigin::Manual, T0 - 60_000),
        Play { end_reason: Some(EndReason::AppClosed), ..Play::new(tid(2), T0 - 50_000, EventType::Skip, Some(5_000)) },
        start(3, EventOrigin::Queue, T0 - 40_000),
        Play::new(tid(3), T0 - 30_000, EventType::Skip, Some(150_000)),
    ];
    let m = UserModel::build(&cat, &TasteInput { events, ..TasteInput::default() }, input(&idx), T0);
    let a = |i: usize| m.tracks[i].aff_long(T0);
    assert!(a(0) < 0.0 && a(1) < a(0), "the app's pick skipped weighs more: {} {}", a(0), a(1));
    assert!(a(2).abs() < 1e-6, "closing the app says nothing: {}", a(2));
    assert!(a(3) > 0.0, "a late skip is a little plus: {}", a(3));
    assert!(m.tracks[2].open.is_none());
}

/// The library is a taste while the history is short: all of it
/// without one, fading with the starts, none from [`COLD_UNTIL`] on or
/// with «Без истории — по самой библиотеке» off.
#[test]
fn the_library_prior_fades_with_the_history() {
    let cat = Catalog::new((0..20u128).map(|i| info(i, &[i as u32 % 4], None).with_duration(Some(200_000))));
    let idx = VectorIndex::new();
    let built = |n: usize, prior: bool| {
        let mut events = Vec::new();
        for k in 0..n {
            let ts = T0 - (n - k) as i64 * 300_000;
            events.push(Play::new(tid(k as u128 % 20), ts, EventType::Start, Some(0)));
            events.push(Play::new(tid(k as u128 % 20), ts + 200_000, EventType::Complete, Some(200_000)));
        }
        UserModel::build(&cat, &TasteInput { events, library_prior: prior, ..TasteInput::default() }, input(&idx), T0)
    };
    let none = built(0, true);
    assert_eq!(none.cold(), 1.0);
    assert!(none.prior_mass(0) > 0.0);
    let some = built(30, true);
    assert!((some.cold() - 0.5).abs() < 1e-6, "{}", some.cold());
    assert!(some.prior_mass(0) < none.prior_mass(0) && some.prior_mass(0) > 0.0);
    let last = built(COLD_UNTIL - 1, true);
    assert!(last.cold() > 0.0);
    let warm = built(COLD_UNTIL, true);
    assert_eq!(warm.cold(), 0.0);
    assert_eq!(warm.prior_mass(0), 0.0);
    assert_eq!(built(0, false).prior_mass(0), 0.0);
}

fn build_of(cat: &Catalog, input: TasteInput) -> UserModel {
    let idx = VectorIndex::new();
    UserModel::build(cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, T0)
}

fn played(t: u128, ts: UnixMs, origin: EventOrigin, context: EventContext, end: Option<u32>, reason: Option<EndReason>) -> [Play; 2] {
    let start = Play { context, origin: Some(origin), ..Play::new(tid(t), ts, EventType::Start, Some(0)) };
    let end = match end {
        None => Play { context, ..Play::new(tid(t), ts + 200_000, EventType::Complete, Some(200_000)) },
        Some(at) => Play { context, end_reason: reason, ..Play::new(tid(t), ts + i64::from(at), EventType::Skip, Some(at)) },
    };
    [start, end]
}

/// A like is a state, not a moment: it counts whole while its song is
/// heard, less as it was last heard long before the latest listening,
/// and a bulk of likes never heard (a service's library brought at
/// once) counts half; the mixes still read the whole like.
#[test]
fn an_old_love_and_a_bulk_of_likes_count_less() {
    let cat = Catalog::new((0..30u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    // Song 0 heard lately, song 1 last heard half a year ago.
    events.extend(played(0, T0 - DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    events.extend(played(1, T0 - 180 * DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    // Liked by hand: 0 and 1; 2…26 in a bulk a year ago, never heard.
    let mut like_times = vec![(tid(0), T0 - DAY_MS), (tid(1), T0 - 181 * DAY_MS)];
    like_times.extend((2..27u128).map(|t| (tid(t), T0 - 365 * DAY_MS + t as i64 * 1_000)));
    let likes = like_times.iter().map(|l| l.0).collect();
    let m = build_of(&cat, TasteInput { events, likes, like_times, ..TasteInput::default() });
    assert_eq!(m.freshness_at(0), 1.0);
    let old = m.freshness_at(1);
    assert!(old < 0.6 && old >= STALE_FLOOR, "{old}");
    assert_eq!(m.freshness_at(5), UNHEARD_LOVE, "a bulk like says nothing of when");
    let (t0, t1, t5) = (&m.tracks[0], &m.tracks[1], &m.tracks[5]);
    assert!(t1.aff_long(T0) < t1.aff_long_whole(T0));
    assert!(t5.aff_long(T0) < t5.aff_long_whole(T0));
    assert!(t0.aff_long(T0) > t1.aff_long(T0));
    assert_eq!(t5.aff_long_whole(T0), m.tracks[6].aff_long_whole(T0));
    // The model's «liked» fades too.
    let mo = m.moment(&Session::default(), T0);
    assert_eq!(m.features(&mo, 0)[2], 1.0);
    assert!(m.features(&mo, 5)[2] < 1.0 && m.features(&mo, 5)[2] > 0.0);
}

/// Skipped early in most of its last plays, a liked song is worn out
/// now: its chance of an early skip is at least theirs whatever the
/// model says; one skip in three is no wear.
#[test]
fn a_liked_song_skipped_lately_is_worn_out() {
    let mut t = TrackStats::default();
    for early in [false, false, false, false, true, true, true] {
        t.note(early);
    }
    assert_eq!(t.burnt(0.1), None, "three of the last six is not most");
    t.note(true);
    let p = t.burnt(0.1).expect("four of the last six");
    assert!((p - (4.0 + 0.2) / 8.0).abs() < 1e-6, "{p}");
    let mut t = TrackStats::default();
    for early in [false, true, false, false] {
        t.note(early);
    }
    assert_eq!(t.burnt(0.1), None);
    let mut t = TrackStats::default();
    for early in [true, true] {
        t.note(early);
    }
    assert_eq!(t.burnt(0.1), None, "too few outcomes");

    let cat = Catalog::new((0..3u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    let mut ts = T0 - 20 * DAY_MS;
    for _ in 0..6 {
        events.extend(played(0, ts, EventOrigin::Manual, EventContext::Library, None, None));
        events.extend(played(1, ts + HOUR_MS, EventOrigin::Manual, EventContext::Library, None, None));
        ts += DAY_MS;
    }
    for _ in 0..4 {
        events.extend(played(0, ts, EventOrigin::Radio, EventContext::Radio, Some(15_000), Some(EndReason::UserNext)));
        ts += DAY_MS;
    }
    let likes = vec![tid(0), tid(1)];
    let m = build_of(&cat, TasteInput { events, likes, ..TasteInput::default() });
    assert!(m.tracks[0].liked(T0));
    let mo = m.moment(&Session::default(), T0);
    let (worn, kept) = (m.p_skip(&mo, 0, EventContext::Radio), m.p_skip(&mo, 1, EventContext::Radio));
    assert!(worn >= 0.55 && worn > kept + 0.1, "{worn} {kept}");
    assert_eq!(m.skip_risk(1, 0.2), 0.2);
}

/// A like learns as fresh as it was at each moment of the history, not
/// as it is at the end: at a start of the walk an old love and a bulk
/// of likes count what a model of the history up to that start counts
/// (the old code learnt them whole, then scored them faded).
#[test]
fn a_like_learns_as_fresh_as_it_was_then() {
    let cat = Catalog::new((0..30u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    events.extend(played(0, T0 - 200 * DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    events.extend(played(1, T0 - 100 * DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    let at = T0 - 10 * DAY_MS;
    events.extend(played(1, at, EventOrigin::Manual, EventContext::Library, None, None));
    events.extend(played(28, at + 300_000, EventOrigin::Manual, EventContext::Library, None, None));
    events.extend(played(29, T0 - DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    // Liked by hand: 0 when it was heard; 2…26 in a bulk, never heard.
    let mut like_times = vec![(tid(0), T0 - 200 * DAY_MS)];
    like_times.extend((2..27u128).map(|t| (tid(t), T0 - 300 * DAY_MS + t as i64 * 1_000)));
    let likes: Vec<TrackId> = like_times.iter().map(|l| l.0).collect();
    let input = TasteInput { events: events.clone(), likes: likes.clone(), like_times: like_times.clone(), ..TasteInput::default() };
    let mut seen = None;
    let mut hook = |m: &UserModel, _: &Moment, _: usize, p: &Play| {
        if p.ts == at + 300_000 {
            seen = Some((m.tracks[0].love_fresh(), m.tracks[2].love_fresh()));
        }
    };
    let idx = VectorIndex::new();
    let extra = ModelInput { index: &idx, types: None, weights: None, graph: None };
    UserModel::build_with(&cat, &input, extra, T0, Some(&mut hook));
    let (old, bulk) = seen.expect("the start was walked");
    let before: Vec<Play> = events.into_iter().filter(|p| p.ts < at + 300_000).collect();
    let then = UserModel::build(&cat, &TasteInput { events: before, likes, like_times, ..TasteInput::default() }, extra, at + 300_000);
    assert!((old - then.tracks[0].love_fresh()).abs() < 1e-3, "{old} {}", then.tracks[0].love_fresh());
    assert!(old < 0.7, "a love last heard 190 days before counts less: {old}");
    assert_eq!(bulk, UNHEARD_LOVE, "a bulk of likes never heard");
    assert_eq!(then.tracks[2].love_fresh(), UNHEARD_LOVE);
}

/// What the session did not approve seeds nothing: a song skipped
/// before its half (past its first 30 s) or past it leads the graphs and
/// the sessions before nowhere; heard out, it does.
#[test]
fn a_rejected_song_seeds_nothing() {
    let cat = Catalog::new((0..4u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    // 0 and 1 listened together, day after day.
    for k in 0..5 {
        let ts = T0 - (k + 2) * DAY_MS;
        events.extend(played(0, ts, EventOrigin::Manual, EventContext::Library, None, None));
        events.extend(played(1, ts + 250_000, EventOrigin::Manual, EventContext::Library, None, None));
    }
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    let session = |end: Option<u32>| {
        let mut s = Session::default();
        s.start(tid(0), T0 - 60_000, EventContext::Radio, Some(EventOrigin::Radio));
        match end {
            Some(at) => s.end_at(tid(0), Ended::Late, at, Some(200_000), None),
            None => s.end(tid(0), Ended::Complete),
        }
        s
    };
    let heard = m.moment(&session(None), T0);
    assert!(heard.bag_near.contains_key(&1), "heard out, it leads to what was listened with it");
    let rejected = session(Some(45_000));
    assert!(Session::verdict(rejected.last().unwrap(), &m).rejects());
    let mo = m.moment(&rejected, T0);
    assert!(mo.bag_near.is_empty() && mo.graph_near.is_empty() && mo.pgraph_near.is_empty(), "a rejection seeds nothing");
    // Approved alone seeds: a skip past the half (no rejection) does not.
    let past_half = session(Some(150_000));
    assert!(!Session::verdict(past_half.last().unwrap(), &m).rejects());
    assert!(m.moment(&past_half, T0).bag_near.is_empty(), "not approved: no seed");
}

/// How often the user's own next start is of the same artist, out of
/// an album, shrunk to a prior with few starts; worn: what a song's own
/// plays say of its next early skip.
#[test]
fn the_users_artist_again_and_a_worn_song() {
    let cat = Catalog::new((0..4u128).map(|i| info(i, &[(i / 2) as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    // 0 and 1 share an artist: 0 → 1 four times, 1 → 2 four times.
    for k in 0..4 {
        let ts = T0 - (k + 2) * DAY_MS;
        events.extend(played(0, ts, EventOrigin::Manual, EventContext::Library, None, None));
        events.extend(played(1, ts + 250_000, EventOrigin::Manual, EventContext::Library, None, None));
        events.extend(played(2, ts + 500_000, EventOrigin::Manual, EventContext::Library, None, None));
    }
    // A pick of the user's after the app's says nothing of it.
    events.extend(played(0, T0 - 30 * HOUR_MS, EventOrigin::Radio, EventContext::Radio, None, None));
    events.extend(played(1, T0 - 30 * HOUR_MS + 250_000, EventOrigin::Manual, EventContext::Library, None, None));
    // An album's run says nothing of it.
    events.extend(played(2, T0 - 3 * HOUR_MS, EventOrigin::Manual, EventContext::Album, None, None));
    events.extend(played(3, T0 - 3 * HOUR_MS + 250_000, EventOrigin::Manual, EventContext::Album, None, None));
    // 3 skipped early in two of its three plays.
    events.extend(played(3, T0 - 2 * HOUR_MS, EventOrigin::Radio, EventContext::Radio, Some(10_000), Some(EndReason::UserNext)));
    events.extend(played(3, T0 - HOUR_MS, EventOrigin::Radio, EventContext::Radio, Some(10_000), Some(EndReason::UserNext)));
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    assert!((m.artist_again(0.3) - (4.0 + 3.0) / (8.0 + 10.0)).abs() < 1e-6, "{}", m.artist_again(0.3));
    assert_eq!(m.worn(0), 0.0, "heard out every time");
    let w = m.worn(3);
    assert!((w - 2.0 / 5.0).abs() < 1e-6, "two early skips of its three last plays: {w}");
    assert!(m.skip_risk(3, 0.1) >= w);
}

/// A song skipped early for long and heard out six times since is not
/// worn any more: wear is what its last plays say.
#[test]
fn a_song_heard_out_again_is_not_worn() {
    let mut t = TrackStats::default();
    for _ in 0..100 {
        t.note(true);
    }
    assert!(t.burnt(0.0).is_some());
    for _ in 0..6 {
        t.note(false);
    }
    assert_eq!(t.burnt(0.0), None);
    let cat = Catalog::new((0..2u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    let mut ts = T0 - 40 * DAY_MS;
    for _ in 0..20 {
        events.extend(played(0, ts, EventOrigin::Radio, EventContext::Radio, Some(10_000), Some(EndReason::UserNext)));
        ts += DAY_MS;
    }
    let m = build_of(&cat, TasteInput { events: events.clone(), ..TasteInput::default() });
    assert!(m.worn(0) > 0.5);
    for _ in 0..6 {
        events.extend(played(0, ts, EventOrigin::Manual, EventContext::Library, None, None));
        ts += DAY_MS;
    }
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    assert_eq!(m.worn(0), 0.0, "{}", m.worn(0));
    assert!(m.skip_risk(0, 0.2) <= 0.2 + 1e-6);
}

/// An imported play is heard as of when it was: a saved song imported
/// long before a start and again after it counts, at that start, as
/// fresh as a model of the history up to the start counts it.
#[test]
fn an_imported_play_is_heard_as_of_when_it_was() {
    let cat = Catalog::new((0..8u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    let import = |ts: UnixMs| {
        [
            Play { origin: Some(EventOrigin::Import), ..Play::new(tid(5), ts, EventType::Start, Some(0)) },
            Play { origin: Some(EventOrigin::Import), ..Play::new(tid(5), ts + 200_000, EventType::Complete, Some(200_000)) },
        ]
    };
    events.extend(import(T0 - 300 * DAY_MS));
    let at = T0 - 200 * DAY_MS;
    events.extend(played(1, at, EventOrigin::Manual, EventContext::Library, None, None));
    events.extend(played(2, at + 300_000, EventOrigin::Manual, EventContext::Library, None, None));
    events.extend(import(T0 - 100 * DAY_MS));
    events.extend(played(3, T0 - DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    let saved = vec![tid(5)];
    let input = TasteInput { events: events.clone(), saved: saved.clone(), ..TasteInput::default() };
    let mut seen = None;
    let mut hook = |m: &UserModel, _: &Moment, _: usize, p: &Play| {
        if p.ts == at + 300_000 {
            seen = Some(m.tracks[5].love_fresh());
        }
    };
    let idx = VectorIndex::new();
    let extra = ModelInput { index: &idx, types: None, weights: None, graph: None };
    UserModel::build_with(&cat, &input, extra, T0, Some(&mut hook));
    let walked = seen.expect("the start was walked");
    let before: Vec<Play> = events.into_iter().filter(|p| p.ts < at + 300_000).collect();
    let then = UserModel::build(&cat, &TasteInput { events: before, saved, ..TasteInput::default() }, extra, at + 300_000);
    assert!((walked - then.tracks[5].love_fresh()).abs() < 1e-3, "{walked} {}", then.tracks[5].love_fresh());
    assert!(walked > UNHEARD_LOVE && walked < 1.0, "heard 100 days before, not unheard: {walked}");
}

/// The app closing on a song (playing or paused, the app's pick or the
/// user's, at any moment of it) says nothing of the song: no skip, no
/// outcome.
#[test]
fn the_app_closing_on_a_song_says_nothing() {
    let cat = Catalog::new((0..4u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    events.extend(played(0, T0 - 4 * HOUR_MS, EventOrigin::Radio, EventContext::Radio, Some(12_000), Some(EndReason::AppClosed)));
    events.extend(played(1, T0 - 3 * HOUR_MS, EventOrigin::Radio, EventContext::Radio, Some(500), Some(EndReason::AppClosed)));
    events.extend(played(2, T0 - 2 * HOUR_MS, EventOrigin::Manual, EventContext::Album, Some(12_000), Some(EndReason::AppClosed)));
    events.extend(played(3, T0 - HOUR_MS, EventOrigin::Mix, EventContext::HomeSection, Some(60_000), Some(EndReason::AppClosed)));
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    assert_eq!((m.tracks[0].early, m.tracks[1].early, m.tracks[2].early, m.tracks[3].early), (0, 0, 0, 0), "the app closed: no skip");
    assert_eq!((m.tracks[0].late, m.tracks[3].late), (0, 0), "the app closed: no outcome");
    assert!(m.tracks.iter().all(|t| t.burnt(0.0).is_none()), "nothing worn out by a close");
}

/// The graph's prior is a cold start: whole for a song never started,
/// fading with its own starts.
#[test]
fn the_graphs_prior_fades_with_a_songs_plays() {
    let cat = Catalog::new((0..2u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut events = Vec::new();
    for k in 0..3 {
        events.extend(played(1, T0 - (k + 1) * DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
    }
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    assert_eq!(m.tracks[1].starts, 3);
    // No graph here: both priors are 0, and the fade keeps them so.
    assert_eq!(m.prior_at(0), m.graph.prior(0));
    assert_eq!(m.prior_at(1), m.graph.prior(1) / 4.0);
}

/// A session's rejections push the candidates by both sounds where the
/// neural model heard them: two songs bliss cannot tell apart from the
/// skipped one, one of its genre and one of another, are pushed apart
/// by the model's vectors; without a rejection there is nothing to
/// measure.
#[test]
fn a_rejection_pushes_by_both_sounds() {
    use crate::embedding::{COMPACT_DIM, Compact};
    let n = 40u128;
    let cat = Catalog::new((0..n).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let mut idx = VectorIndex::new();
    let mut rng = Rng::new(5);
    for i in 0..n {
        let x = match i {
            0 => 0.0,
            1 | 2 => 0.05,
            _ => i as f32 * 0.3,
        };
        idx.insert(tid(i), line(x));
        // Two genres: the even dimensions and the odd ones.
        let genre = match i {
            1 => 1,
            0 | 2 => 0,
            _ => (i % 2) as usize,
        };
        let v: [f32; COMPACT_DIM] = std::array::from_fn(|j| if j % 2 == genre { 1.0 } else { 0.0 } + 0.3 * rng.f32());
        assert!(idx.set_embedding(tid(i), Compact::of_unit(&v).unwrap()));
    }
    let m = UserModel::build(&cat, &TasteInput::default(), ModelInput { index: &idx, types: None, weights: None, graph: None }, T0);
    let mut s = Session::default();
    s.start(tid(0), T0 - 60_000, EventContext::Radio, Some(EventOrigin::Radio));
    s.end_at(tid(0), Ended::Early, 5_000, Some(200_000), None);
    assert_eq!(s.pushed(&m).len(), s.pushes(&m).len());
    let probe = m.push_probe(&s, Some(&idx)).expect("a rejection, an index that blends");
    let (one, two) = (m.position(tid(1)).unwrap(), m.position(tid(2)).unwrap());
    let mo = m.moment(&s, T0);
    let bliss = (m.skip_push_of(&mo.pushes, one), m.skip_push_of(&mo.pushes, two));
    assert!(bliss.1 < 0.0 && (bliss.0 - bliss.1).abs() < 1e-6, "{bliss:?}");
    let (other, same) = (m.skip_push_by(&probe, one), m.skip_push_by(&probe, two));
    assert!(same < other - 0.05, "the skipped genre is pushed harder: {same} {other}");
    assert!(m.push_probe(&Session::default(), Some(&idx)).is_none());
    assert!(m.push_probe(&s, None).is_none());
}

/// How soon a user hears a song again is their median time between two
/// completes of one song: unknown with few repeats (the default rest),
/// a day's bin for one who plays their songs again every 20 hours; an
/// imported history's repeats say nothing of it. A version's work is
/// its song's track.
#[test]
fn the_users_own_time_between_repeats() {
    let cat = Catalog::new((0..3u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
    let few = build_of(&cat, TasteInput { events: played(0, T0 - DAY_MS, EventOrigin::Manual, EventContext::Library, None, None).into_iter().collect(), ..TasteInput::default() });
    assert_eq!(few.repeat_rest(), REPEAT_DEFAULT_MS);
    let mut events = Vec::new();
    let mut imported = Vec::new();
    for k in 0..30i64 {
        for t in 0..2u128 {
            let ts = T0 - 40 * DAY_MS + k * 20 * HOUR_MS + t as i64 * 300_000;
            events.extend(played(t, ts, EventOrigin::Manual, EventContext::Library, None, None));
            imported.extend(played(t, ts, EventOrigin::Import, EventContext::Library, None, None).map(|p| Play { origin: Some(EventOrigin::Import), ..p }));
        }
    }
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    assert_eq!(m.repeat_rest(), 24 * HOUR_MS);
    let m = build_of(&cat, TasteInput { events: imported, ..TasteInput::default() });
    assert_eq!(m.repeat_rest(), REPEAT_DEFAULT_MS, "imported plays");
    let versions = Catalog::new([info(0, &[1], Some(0)), info(1, &[1], Some(0)).with_song(tid(0)), info(2, &[2], Some(1))]);
    let m = build_of(&versions, TasteInput::default());
    assert_eq!((m.work_at(0), m.work_at(1), m.work_at(2)), (tid(0), tid(0), tid(2)));
}

/// A file come lately and never played is a new arrival: fully just
/// added, half a week later, none past the window, once played, without
/// a file, or with no time known.
#[test]
fn a_new_arrival_fades_and_ends_once_played() {
    let cat = Catalog::new([
        info(0, &[0], Some(0)).with_added(T0 - HOUR_MS),
        info(1, &[1], Some(1)).with_added(T0 - ARRIVAL_HALF_MS),
        info(2, &[2], Some(2)).with_added(T0 - ARRIVAL_WINDOW_MS - DAY_MS),
        info(3, &[3], Some(3)).with_added(T0 - HOUR_MS),
        info(4, &[4], Some(4)).with_added(T0 - HOUR_MS).with_local(false),
        info(5, &[5], Some(5)),
    ]);
    let events: Vec<Play> = played(3, T0 - 30 * 60_000, EventOrigin::Manual, EventContext::Library, None, None).into_iter().collect();
    let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
    assert!(m.arrival_at(0) > 0.99, "{}", m.arrival_at(0));
    assert!((m.arrival_at(1) - 0.5).abs() < 1e-3, "{}", m.arrival_at(1));
    assert_eq!(m.arrival_at(2), 0.0, "past the window");
    assert_eq!(m.arrival_at(3), 0.0, "played");
    assert_eq!(m.arrival_at(4), 0.0, "no file");
    assert_eq!(m.arrival_at(5), 0.0, "no time known");
}

/// A day's files are counted as they had come by the moment: more of
/// that day coming later (a replay's catalog holds them already) do not
/// make the morning's an import.
#[test]
fn a_days_import_counts_only_what_had_come() {
    let day = T0.div_euclid(DAY_MS) * DAY_MS;
    let now = day + 9 * HOUR_MS;
    let mut tracks: Vec<crate::taste::TrackInfo> = (0..50u128).map(|i| info(i, &[i as u32], None).with_added(day + HOUR_MS)).collect();
    tracks.extend((50..50 + ARRIVAL_BATCH as u128 + 1).map(|i| info(i, &[i as u32], None).with_added(day + 15 * HOUR_MS)));
    let cat = Catalog::new(tracks);
    let idx = VectorIndex::new();
    let m = UserModel::build(&cat, &TasteInput::default(), ModelInput { index: &idx, types: None, weights: None, graph: None }, now);
    assert!(m.arrival_at(0) > 0.9, "the morning's files are new arrivals");
    assert_eq!(m.arrival_at(60), 0.0, "not come yet");
    let later = UserModel::build(&cat, &TasteInput::default(), ModelInput { index: &idx, types: None, weights: None, graph: None }, day + 20 * HOUR_MS);
    assert_eq!(later.arrival_at(0), 0.0, "once the day brought an import, none of it");
}
