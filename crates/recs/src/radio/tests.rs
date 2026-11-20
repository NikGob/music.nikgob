use super::*;

/// The wave's queue stands after what played last, a library song or
/// a new one: new songs played are not ahead any more; nothing played,
/// all is ahead; a session the served songs do not hold has no known
/// queue.
#[test]
fn the_queue_stands_after_what_played_last() {
    use nikgob_core::streaming::Service;
    let t = |i: u128| Item::Track(crate::test_util::tid(i));
    let n = |k: &str| Item::Stream(RemoteId::new(Service::Youtube, k));
    let served: VecDeque<Item> = [t(1), n("a"), n("b"), t(2), n("c")].into_iter().collect();
    let tid = crate::test_util::tid;
    assert_eq!(queued_after(&served, Some(tid(1)), Some(&n("b"))), vec![&t(2), &n("c")], "a new song played after the library one");
    assert_eq!(queued_after(&served, Some(tid(1)), None), vec![&n("a"), &n("b"), &t(2), &n("c")]);
    assert_eq!(queued_after(&served, Some(tid(2)), Some(&n("a"))), vec![&n("c")], "the later of the two");
    let only_new: VecDeque<Item> = [n("a"), n("b"), n("c")].into_iter().collect();
    assert_eq!(queued_after(&only_new, None, Some(&n("a"))), vec![&n("b"), &n("c")]);
    assert_eq!(queued_after(&only_new, None, None).len(), 3, "nothing played: all ahead");
    assert!(queued_after(&served, Some(tid(9)), None).is_empty(), "a session the wave did not serve");
    // Told played as it goes: a new song's end moves the queue on.
    let mut radio = RadioState::new(RadioSeed::Wave(crate::wave::WaveSeed::plain(1.0)));
    radio.note_played(n("a"));
    assert_eq!(radio.played_last, Some(n("a")));
    radio.feedback(n("b"), Outcome::Complete);
    assert_eq!(radio.played_last, Some(n("b")));
}
use crate::index::VectorIndex;
use crate::pick::assert_diverse;
use crate::taste::{Catalog, Play, Taste, TasteInput, TrackInfo};
use nikgob_core::model::EventType;
use crate::test_util::*;
use nikgob_core::streaming::Service;

const NOW: i64 = 1_000 * crate::taste::DAY_MS;

/// Region A around +x, region B around −x, the seed in the middle; 20
/// artists per region, 3 tracks each.
fn two_regions() -> (Catalog, VectorIndex<TrackId>) {
    let mut tracks = vec![info(0, &[999], None)];
    let mut idx = VectorIndex::with_weights([1.0; DIM]);
    let mut rng = Rng::new(1);
    idx.insert(tid(0), point(0.0, 0.0));
    for i in 1..=120u128 {
        let region_a = i <= 60;
        let artist = ((i - 1) / 3) as u32;
        tracks.push(info(i, &[artist], Some(500 + i)));
        let x = if region_a { 4.0 } else { -4.0 };
        idx.insert(tid(i), point(x + rng.f32() - 0.5, rng.f32() * 2.0 - 1.0));
    }
    (Catalog::new(tracks), idx)
}

fn in_a(p: &Pick) -> bool {
    matches!(&p.item, Item::Track(id) if (1..=60).contains(&id.0.as_u128()))
}

/// A wave saved before skips were weighed (`session.json` as it was
/// then) goes on: a play the bar was dragged into is a scrub-skip as it
/// was, the β its rejections add fades to the β it was restored with,
/// and its picks of then are of an unknown artist (one artist: their run
/// is left at [`crate::wave::LEAVE_UNKNOWN`]).
#[test]
fn a_wave_saved_before_goes_on() {
    use crate::open_world::Scene;
    use crate::wave::{Arm, BETA_KEEP, Bandit, WaveSeed};
    let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(0.3)));
    let arms = vec![Arm { scene: Scene::Profile(1), alpha: 4.0, beta: 2.0, tracks: 20 }, Arm { scene: Scene::Profile(2), alpha: 2.0, beta: 4.0, tracks: 20 }];
    let mut b = Bandit::with_arms(arms, 3);
    b.choose(&[], &[]);
    let k = b.current.unwrap();
    for t in 0..6 {
        b.note_served(Item::Track(tid(100 + t)), k, &format!("artist {t}"));
    }
    radio.wave = Some(b);
    let mut s = Session::default();
    s.start(tid(7), NOW, nikgob_core::model::EventContext::Radio, None);
    s.seek(tid(7), Some(1_000), 30_000);
    s.end_at(tid(7), Ended::Late, 190_000, Some(200_000), None);
    radio.attune(s, None);
    let judged = |r: &RadioState| r.session.last().map(|p| judge(p.ended, p.position_ms, p.duration_ms, p.dragged()));
    assert_eq!(judged(&radio), Some(Verdict::Approved), "161 s of 200 heard");
    // As it was saved then.
    let mut json = serde_json::to_value(&radio).unwrap();
    let play = json["session"]["tracks"][0].as_object_mut().unwrap();
    play.remove("scrub");
    play.insert("scrubbed".into(), true.into());
    let wave = json["wave"].as_object_mut().unwrap();
    for key in ["base_beta", "memory_no", "run", "heard_in", "only_new"] {
        wave.remove(key);
    }
    wave.insert("rejects".into(), serde_json::json!([1, 1]));
    for served in wave.get_mut("served").unwrap().as_array_mut().unwrap() {
        let o = served.as_object_mut().unwrap();
        o.remove("artist");
        o.remove("liked");
    }
    let mut back: RadioState = serde_json::from_value(json).unwrap();
    assert_eq!(judged(&back), Some(Verdict::Rejected), "a scrub-skip, as then");
    let b = back.wave.as_mut().unwrap();
    let b0 = b.arms[k].beta;
    let items: Vec<Item> = (0..6).map(|t| Item::Track(tid(100 + t))).collect();
    b.feedback(&items[0], Verdict::Approved);
    for i in &items[1..] {
        b.feedback(i, Verdict::Rejected);
    }
    assert!(!b.settled(), "of unknown artists alone: five leave");
    assert!((b.arms[k].beta - (b0 + 5.0)).abs() < 1e-4);
    b.note_served(Item::Track(tid(200)), k, "x");
    b.feedback(&Item::Track(tid(200)), Verdict::Approved);
    assert!((b.arms[k].beta - (b0 + 5.0 * BETA_KEEP)).abs() < 1e-4, "fades to the β restored");
}

/// What an end says, and picks dropped unplayed are not served: the
/// radio goes on from what is queued.
#[test]
fn what_an_end_says_and_dropped_picks_are_forgotten() {
    assert_eq!(Outcome::Complete.verdict(), Verdict::Approved);
    assert_eq!(Outcome::skip(5_000).verdict(), Verdict::Rejected);
    assert_eq!(Outcome::skip(120_000).verdict(), Verdict::Neutral, "length unknown: past 30 s says nothing");
    let at = |s: u32, scrub| Outcome::Skip { position_ms: s, duration_ms: Some(200_000), scrub }.verdict();
    let to = |to_ms| Some(Scrub { to_ms, heard_ms: 2_000, blind: false });
    assert_eq!(at(45_000, None), Verdict::Rejected, "the first half");
    assert_eq!(at(120_000, None), Verdict::Neutral);
    assert_eq!(at(170_000, None), Verdict::Approved, "the outro");
    assert_eq!(at(60_000, to(50_000)), Verdict::Rejected, "a scrub-skip");
    assert_eq!(at(60_000, to(30_000)), Verdict::Neutral, "played on past the drag");
    assert_eq!(at(170_000, to(160_000)), Verdict::Neutral, "dragged to the outro: no approval");
    let (cat, idx) = two_regions();
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    let picks = radio.extend(&recs, 3, &RadioParams { new_share: 0.0, seed: 1 });
    let dropped: Vec<Item> = picks[1..].iter().map(|p| p.item.clone()).collect();
    radio.forget(&dropped);
    assert!(radio.served().all(|x| !dropped.contains(x)));
    assert_eq!(radio.served().last(), Some(&picks[0].item));
}

/// A served song takes its versions along (they stand for it as their
/// song), and a served version takes its song and the other versions:
/// none of them is picked again in the session.
#[test]
fn a_served_song_takes_its_versions_along() {
    let cat = Catalog::new(vec![
        info(0, &[1], None),
        info(1, &[1], None).with_song(tid(0)),
        info(2, &[1], None).with_song(tid(0)),
        info(3, &[2], None),
    ]);
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let idx = VectorIndex::new();
    let recs = Recs::new(&cat, &taste, &idx);
    let mut taken = HashSet::new();
    take_works(&recs, &HashSet::from([Item::Track(tid(0))]), &mut taken);
    assert!(taken.contains(&Item::Track(tid(1))) && taken.contains(&Item::Track(tid(2))));
    assert!(!taken.contains(&Item::Track(tid(3))));
    let mut taken = HashSet::new();
    take_works(&recs, &HashSet::from([Item::Track(tid(2))]), &mut taken);
    assert!(taken.contains(&Item::Track(tid(0))));
    let slot = recs.slot(1, Reason::Random, 1.0);
    assert!(slot.song.as_ref().is_some_and(|s| taken.contains(s)), "the other version stays out");
    let mut taken = HashSet::new();
    assert!(take_works(&recs, &HashSet::new(), &mut taken).is_empty());
    assert!(taken.is_empty());
}

/// A small library of songs that all have a version never runs dry: once
/// everything has played, what played long ago (and its versions) may
/// play again, as for songs without one; one batch never holds a song
/// and its version.
#[test]
fn a_library_of_versions_never_runs_dry() {
    use crate::model::{ModelInput, UserModel};
    let mut tracks = Vec::new();
    let mut idx = VectorIndex::with_weights([1.0; DIM]);
    for i in 0..24u128 {
        let t = info(i, &[(i / 2) as u32], None);
        tracks.push(if i % 2 == 1 { t.with_song(tid(i - 1)) } else { t });
        idx.insert(tid(i), point(i as f32 * 0.1, 0.0));
    }
    let cat = Catalog::new(tracks);
    let input = TasteInput::default();
    let taste = Taste::build(&cat, &input, NOW);
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, NOW);
    for with_model in [false, true] {
        let recs = Recs::new(&cat, &taste, &idx);
        let recs = if with_model { recs.with_model(&model) } else { recs };
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        // Each work's one recording in the session (the seed's too).
        let mut heard: HashMap<u128, u128> = HashMap::from([(0, 0)]);
        for round in 0..30 {
            let picks = radio.extend(&recs, 3, &RadioParams { new_share: 0.0, seed: round });
            assert_eq!(picks.len(), 3, "{with_model}: round {round}");
            let ids: Vec<u128> = picks
                .iter()
                .map(|p| match &p.item {
                    Item::Track(id) => id.0.as_u128(),
                    Item::Stream(_) => u128::MAX,
                })
                .collect();
            let works: HashSet<u128> = ids.iter().map(|i| i / 2).collect();
            assert_eq!(works.len(), 3, "{with_model}: a song and its version in one batch, round {round}");
            for id in ids {
                let first = *heard.entry(id / 2).or_insert(id);
                assert_eq!(first, id, "{with_model}: another version of a work the session played, round {round}");
            }
        }
    }
}

/// A like while a radio plays: its next extension starts with the best
/// of the liked track's neighbours, by the model or by sound alone.
#[test]
fn a_like_brings_its_neighbours_first() {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx) = two_regions();
    let input = TasteInput::default();
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, NOW);
    let taste = Taste::build(&cat, &input, NOW);
    let params = RadioParams { new_share: 0.0, seed: 3 };
    for with_model in [true, false] {
        let recs = Recs::new(&cat, &taste, &idx);
        let recs = if with_model { recs.with_model(&model) } else { recs };
        // A radio gone into A…
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(tid(9)), Outcome::Complete);
        radio.feedback(Item::Track(tid(12)), Outcome::Complete);
        assert!(in_a(&radio.clone().extend(&recs, 1, &params)[0]), "{with_model}");
        // …and a track of B liked: B's first.
        radio.liked(Item::Track(tid(100)), None);
        let picks = radio.extend(&recs, 3, &params);
        assert!(!in_a(&picks[0]), "{with_model}: {:?}", picks[0]);
    }
}

/// A pick taken out of the queue by hand pushes the radio away from
/// its sound.
#[test]
fn a_removed_pick_pushes() {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx) = two_regions();
    let input = TasteInput::default();
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, NOW);
    let taste = Taste::build(&cat, &input, NOW);
    let params = RadioParams { new_share: 0.0, seed: 3 };
    for with_model in [true, false] {
        let recs = Recs::new(&cat, &taste, &idx);
        let recs = if with_model { recs.with_model(&model) } else { recs };
        let in_a_of = |removed: &[u128]| {
            let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
            radio.feedback(Item::Track(tid(9)), Outcome::Complete);
            for t in removed {
                radio.removed(Item::Track(tid(*t)));
            }
            radio.extend(&recs, 10, &params).iter().filter(|p| in_a(p)).count()
        };
        let (plain, pushed) = (in_a_of(&[]), in_a_of(&[5, 7, 11]));
        assert!(pushed < plain, "{with_model}: {plain} → {pushed}");
    }
}

/// A radio picked by its seed ranks by the user model once the runtime
/// hands it the session: what the session rejected turns it.
#[test]
fn a_seed_radio_ranks_by_the_model() {
    use crate::model::{ModelInput, UserModel};
    use nikgob_core::model::{EventContext, EventOrigin};
    let (cat, idx) = two_regions();
    let input = TasteInput::default();
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, NOW);
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
    let seed = RadioSeed::Style { index: 0, centroid: point(0.0, 0.0).to_vec(), artists: Vec::new() };
    let in_a_of = |attuned: bool| {
        let mut radio = RadioState::new(seed.clone());
        if attuned {
            let mut s = Session::default();
            for (k, t) in [3u128, 6, 9].into_iter().enumerate() {
                s.start(tid(t), NOW - 600_000 + k as i64 * 20_000, EventContext::Radio, Some(EventOrigin::Mix));
                s.end(tid(t), Ended::Early);
            }
            radio.attune(s, None);
        }
        radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 2 }).iter().filter(|p| in_a(p)).count()
    };
    let (plain, turned) = (in_a_of(false), in_a_of(true));
    assert!(turned < plain, "{plain} → {turned}");
}

/// The kinds of songs of [`listener`]: first id, count, x of the sound.
const RAP: (u128, u128, f32) = (0, 120, 20.0);
const OST: (u128, u128, f32) = (1_000, 60, 0.0);
const JPOP: (u128, u128, f32) = (2_000, 60, 10.0);
const ROCK: (u128, u128, f32) = (3_000, 60, -10.0);

fn kind_of(id: u128) -> u128 {
    [RAP, OST, JPOP, ROCK].into_iter().find(|k| (k.0..k.0 + k.1).contains(&id)).expect("a kind").0
}

type Listener = (Catalog, VectorIndex<TrackId>, TasteInput, crate::songtype::SongTypes);

/// A synthetic listener: a big rap region played most (twenty artists),
/// a small region of soundtracks (three worlds, one composer each)
/// played a little, and for an omnivore Japanese pop and rock too, in
/// sessions of one kind each (the user's own choices).
pub(crate) fn listener(omnivore: bool) -> Listener {
    use crate::songtype::{Lang, SongType, SongTypes, Voice};
    let mut kinds = vec![(RAP, Some(Lang::Russian), 40usize), (OST, None, 8)];
    if omnivore {
        kinds.push((JPOP, Some(Lang::Japanese), 10));
        kinds.push((ROCK, Some(Lang::English), 10));
    }
    let mut rng = Rng::new(17);
    let mut tracks = Vec::new();
    let mut idx = VectorIndex::with_weights([1.0; DIM]);
    let mut types = SongTypes::default();
    for ((first, n, x), lang, _) in &kinds {
        for k in 0..*n {
            let id = first + k;
            let world = k / 20;
            let artist = if *first == OST.0 { (id / 20) as u32 } else { (id / 6) as u32 };
            tracks.push(info(id, &[artist], Some(id / 10)).with_world(&format!("w{first}.{world}")).with_duration(Some(200_000)));
            // Each world a little apart inside its kind.
            idx.insert(tid(id), point(x + world as f32 * 1.2 + rng.f32() * 0.6, rng.f32() * 0.6));
            let ty = match lang {
                Some(l) => SongType { voice: Voice::Sung, lang: Some(*l), ..SongType::default() },
                None => SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() },
            };
            types.by_track.insert(tid(id), ty);
        }
    }
    let mut events = Vec::new();
    let mut ts = crate::model::tests::T0 - 20 * crate::taste::DAY_MS;
    for ((first, n, _), _, sessions) in &kinds {
        for _ in 0..*sessions {
            for _ in 0..6 {
                let id = tid(first + rng.below(*n as usize) as u128);
                events.push(Play { context: nikgob_core::model::EventContext::Library, ..Play::new(id, ts, EventType::Start, Some(0)) });
                ts += 200_000;
                events.push(Play::new(id, ts, EventType::Complete, Some(200_000)));
            }
            ts += 4 * crate::taste::HOUR_MS;
        }
    }
    events.sort_by_key(|p| p.ts);
    (Catalog::new(tracks), idx, TasteInput { events, ..TasteInput::default() }, types)
}

/// Plays a model radio of `seed` for `batches` of ten, every pick heard
/// out (the first `skip` of the first batch skipped early): the picks.
fn play_model_radio(l: &Listener, seed: u128, batches: usize, skip: usize) -> Vec<u128> {
    play_model_radio_with(l, seed, batches, skip, |_| {})
}

/// [`play_model_radio`] with the radio's knobs set by `tune`.
fn play_model_radio_with(l: &Listener, seed: u128, batches: usize, skip: usize, tune: fn(&mut RadioState)) -> Vec<u128> {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx, input, types) = l;
    let now = crate::model::tests::T0;
    let model = UserModel::build(cat, input, ModelInput { index: idx, types: Some(types), weights: None, graph: None }, now);
    let recs = Recs::new(cat, &model.taste, idx).with_model(&model);
    let mut session = Session::default();
    let mut ts = now;
    session.start(tid(seed), ts, nikgob_core::model::EventContext::Radio, None);
    session.end(tid(seed), Ended::Complete);
    let mut radio = RadioState::new(RadioSeed::Track(tid(seed)));
    tune(&mut radio);
    let mut out = Vec::new();
    let mut skipped = 0;
    for b in 0..batches {
        radio.attune(session.clone(), None);
        for p in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 5 }) {
            let Item::Track(t) = p.item else { continue };
            ts += 200_000;
            session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
            let early = b == 0 && skipped < skip;
            session.end(t, if early { Ended::Early } else { Ended::Complete });
            radio.feedback(p.item.clone(), if early { Outcome::skip(5_000) } else { Outcome::Complete });
            skipped += usize::from(early);
            out.push(t.0.as_u128());
            if early && skipped == skip {
                break;
            }
        }
    }
    out
}

/// «Похоже на» a soundtrack, for a listener of mostly rap and for an
/// omnivore: thirty songs, all soundtracks near the seed, never three of
/// one world running, never a rap song however much rap is played.
#[test]
fn a_soundtrack_radio_stays_a_soundtrack() {
    for omnivore in [false, true] {
        let l = listener(omnivore);
        let picks = play_model_radio(&l, OST.0 + 3, 3, 0);
        assert!(picks.len() >= 25, "{picks:?}");
        assert!(picks.iter().all(|t| kind_of(*t) == OST.0), "omnivore {omnivore}: {picks:?}");
        let worlds: Vec<u128> = picks.iter().map(|t| (t - OST.0) / 20).collect();
        for w in worlds.windows(3) {
            assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
        }
    }
}

/// Two early skips in a soundtrack radio turn it to the soundtracks of
/// another world, not to the rap played most.
#[test]
fn skips_turn_a_radio_to_a_neighbouring_sound() {
    let l = listener(true);
    let picks = play_model_radio(&l, OST.0 + 3, 2, 2);
    let skipped_world = (picks[0] - OST.0) / 20;
    let after = &picks[2..];
    assert!(!after.is_empty());
    assert!(after.iter().all(|t| kind_of(*t) == OST.0), "{picks:?}");
    let other = after.iter().filter(|t| (**t - OST.0) / 20 != skipped_world).count();
    assert!(other * 2 >= after.len(), "mostly another world: {picks:?}");
}

/// Rap and rock of one language that sound apart (two profiles), and
/// soundtracks without words, each played in sessions of its own: the
/// first id, how many, x of the sound.
const RU_RAP: (u128, u128, f32) = (0, 60, 20.0);
const RU_ROCK: (u128, u128, f32) = (1_000, 120, -10.0);
const QUIET: (u128, u128, f32) = (2_000, 60, 45.0);

fn of_kind(id: u128) -> u128 {
    [RU_RAP, RU_ROCK, QUIET].into_iter().find(|k| (k.0..k.0 + k.1).contains(&id)).expect("a kind").0
}

fn one_language_two_sounds() -> Listener {
    use crate::songtype::{Lang, SongType, SongTypes, Voice};
    let kinds = [(RU_RAP, true, 15usize), (RU_ROCK, true, 20), (QUIET, false, 10)];
    let mut rng = Rng::new(23);
    let mut tracks = Vec::new();
    let mut idx = VectorIndex::with_weights([1.0; DIM]);
    let mut types = SongTypes::default();
    for ((first, n, x), sung, _) in kinds {
        for k in 0..n {
            let id = first + k;
            // An artist (and their world) every six tracks.
            let artist = (id / 6) as u32;
            tracks.push(info(id, &[artist], Some(id / 6)).with_world(&format!("w{artist}")).with_duration(Some(200_000)));
            // Apart on the first feature only (each is normalised).
            idx.insert(tid(id), point(x + rng.f32() * 2.0, 0.0));
            let ty = if sung {
                SongType { voice: Voice::Sung, lang: Some(Lang::Russian), ..SongType::default() }
            } else {
                SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() }
            };
            types.by_track.insert(tid(id), ty);
        }
    }
    let mut events = Vec::new();
    let mut ts = crate::model::tests::T0 - 20 * crate::taste::DAY_MS;
    for ((first, n, _), _, sessions) in kinds {
        for _ in 0..sessions {
            for _ in 0..6 {
                let id = tid(first + rng.below(n as usize) as u128);
                events.push(Play { context: nikgob_core::model::EventContext::Library, ..Play::new(id, ts, EventType::Start, Some(0)) });
                ts += 200_000;
                events.push(Play::new(id, ts, EventType::Complete, Some(200_000)));
            }
            ts += 4 * crate::taste::HOUR_MS;
        }
    }
    events.sort_by_key(|p| p.ts);
    (Catalog::new(tracks), idx, TasteInput { events, ..TasteInput::default() }, types)
}

/// The fixture makes the profiles it is about: rap and rock of one
/// language apart, soundtracks a third.
fn profiles_of(l: &Listener) -> Vec<u128> {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx, input, types) = l;
    let model = UserModel::build(cat, input, ModelInput { index: idx, types: Some(types), weights: None, graph: None }, crate::model::tests::T0);
    let mut kinds: Vec<u128> = (0..model.profiles.len())
        .map(|j| {
            let i = (0..model.len()).find(|&i| model.profile_at(i) == Some(j)).expect("a member");
            of_kind(model.id(i).0.as_u128())
        })
        .collect();
    kinds.sort_unstable();
    kinds
}

/// «Похоже на» a rap song stays rap while the rap profile has songs to
/// play, then goes on with the rock of the same language (the
/// neighbouring profile), never with the soundtracks it never follows.
#[test]
fn stays_in_profile_until_exhausted() {
    let l = one_language_two_sounds();
    assert_eq!(profiles_of(&l), [RU_RAP.0, RU_ROCK.0, QUIET.0]);
    let picks = play_model_radio(&l, RU_RAP.0 + 3, 9, 0);
    assert!(picks.len() >= 80, "it keeps playing: {}", picks.len());
    let kinds: Vec<u128> = picks.iter().map(|t| of_kind(*t)).collect();
    let first_other = kinds.iter().position(|k| *k != RU_RAP.0).expect("it goes on past the rap");
    assert!(first_other >= RU_RAP.1 as usize - 10, "rap first while it lasts: {first_other} {kinds:?}");
    let rap = kinds.iter().filter(|k| **k == RU_RAP.0).count();
    assert!(rap >= RU_RAP.1 as usize - 5, "the rap played through: {rap}");
    assert!(kinds[first_other..].iter().all(|k| *k == RU_ROCK.0), "then the rock: {kinds:?}");
    let worlds: Vec<u128> = picks.iter().map(|t| t / 6).collect();
    for w in worlds.windows(3) {
        assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
    }
}

/// Two early skips running take a rap radio out of the rap to the rock
/// of its language; without them it stays.
#[test]
fn widens_after_two_skips() {
    let l = one_language_two_sounds();
    let calm = play_model_radio(&l, RU_RAP.0 + 3, 2, 0);
    assert!(calm.iter().all(|t| of_kind(*t) == RU_RAP.0), "{calm:?}");
    let picks = play_model_radio(&l, RU_RAP.0 + 3, 2, 2);
    let after = &picks[2..];
    assert!(after.len() >= 8, "{picks:?}");
    let rock = after.iter().filter(|t| of_kind(**t) == RU_ROCK.0).count();
    assert!(rock * 2 >= after.len(), "mostly the neighbouring profile: {picks:?}");
    assert!(after.iter().all(|t| of_kind(*t) != QUIET.0), "{picks:?}");
}

/// A background session's radio of soundtracks without words keeps to
/// what suits the background and to the middle of the profile's energy.
#[test]
fn a_background_radio_stays_quiet() {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx, input, types) = one_language_two_sounds();
    let now = crate::model::tests::T0;
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
    // Half of the soundtracks suit the background.
    let suits: Vec<bool> = cat.tracks().iter().map(|t| of_kind(t.id.0.as_u128()) == QUIET.0 && t.id.0.as_u128() % 2 == 0).collect();
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model).with_background(&suits);
    let mut radio = RadioState::new(RadioSeed::Track(tid(QUIET.0 + 2)));
    let plain = radio.plan(&model);
    assert!(!plain.background);
    let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 4 });
    assert!(picks.iter().any(|p| matches!(p.item, Item::Track(t) if t.0.as_u128() % 2 == 1)), "not background: anything of the profile");
    let mut bg = plain;
    bg.background = true;
    bg.wordless = true;
    let mut taken = HashSet::new();
    let (picks, wide) = radio.by_model(&recs, &model, &bg, 10, &HashSet::new(), &Diversity::default(), &mut Rng::new(1), &mut taken, None, None);
    assert!(!wide, "a background radio never widens");
    assert_eq!(picks.len(), 10);
    for p in &picks {
        let Item::Track(t) = p.item else { panic!() };
        assert!(suits[cat.position(t).unwrap()], "{t:?} does not suit the background");
    }
}

/// «С новым» with candidates enough: a fifth of a model radio is new,
/// never more than five of the library running, for a one-kind
/// listener and an omnivore alike.
#[test]
fn a_radio_with_new_brings_its_share() {
    use crate::model::{ModelInput, UserModel};
    for omnivore in [false, true] {
        let (cat, idx, input, types) = listener(omnivore);
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, crate::model::tests::T0);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let mut radio = RadioState::new(RadioSeed::Track(tid(OST.0 + 3)));
        radio.add_new((0..60).map(|k| NewTrack { work: String::new(),
            id: RemoteId::new(Service::Youtube, format!("n{k}")),
            artist: format!("new artist {}", k % 20),
            album: None,
            vector: None,
        }));
        let mut picks = Vec::new();
        for _ in 0..4 {
            picks.extend(radio.extend(&recs, 10, &RadioParams { new_share: 0.2, seed: 2 }));
        }
        let new: Vec<bool> = picks.iter().map(|p| matches!(p.item, Item::Stream(_))).collect();
        let share = new.iter().filter(|n| **n).count() as f32 / new.len() as f32;
        assert!((0.15..=0.25).contains(&share), "omnivore {omnivore}: {share} {new:?}");
        let longest = new.split(|n| *n).map(<[bool]>::len).max().unwrap_or(0);
        assert!(longest <= 5, "{longest} of the library running: {new:?}");
        // New music that came late joins at once.
        let late = radio.take_new(&recs, 2, 10, 0);
        assert_eq!(late.len(), 2);
        assert!(late.iter().all(|p| !picks.contains(p)));
    }
}

/// A one-kind listener's radio of a rap song stays rap and spreads its
/// artists.

#[test]
fn a_one_kind_radio_spreads_its_artists() {
    let l = listener(false);
    let picks = play_model_radio(&l, RAP.0 + 7, 3, 0);
    assert!(picks.len() >= 25);
    assert!(picks.iter().all(|t| kind_of(*t) == RAP.0), "{picks:?}");
    let artists: Vec<u128> = picks.iter().map(|t| t / 6).collect();
    for w in artists.windows(2) {
        assert_ne!(w[0], w[1], "an artist twice running: {artists:?}");
    }
}


#[test]
fn early_skips_push_away_and_completes_pull() {
    let (cat, idx) = two_regions();
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let params = RadioParams { new_share: 0.0, seed: 3 };

    let mut free = RadioState::new(RadioSeed::Track(tid(0)));
    let picks = free.extend(&recs, 20, &params);
    let a = picks.iter().filter(|p| in_a(p)).count();
    assert!((5..=15).contains(&a), "both regions without lessons: {a}");

    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.feedback(Item::Track(tid(5)), Outcome::skip(10_000));
    let picks = radio.extend(&recs, 10, &params);
    let a = picks.iter().filter(|p| in_a(p)).count();
    assert!(a <= 1, "an early skip in A moves away from A: {a}");

    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.feedback(Item::Track(tid(5)), Outcome::Complete);
    let picks = radio.extend(&recs, 10, &params);
    let a = picks.iter().filter(|p| in_a(p)).count();
    assert!(a >= 8, "a complete in A pulls to A: {a}");

    // A late skip teaches nothing (it only counts as played).
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.feedback(Item::Track(tid(5)), Outcome::skip(120_000));
    let mut fresh = RadioState::new(RadioSeed::Track(tid(0)));
    fresh.recent.push(Item::Track(tid(5)));
    fresh.played_last = Some(Item::Track(tid(5)));
    fresh.session = radio.session.clone();
    assert_eq!(radio, fresh);
}

/// A radio leaves out a favourite played an hour ago while «Не
/// повторять сыгранное» (three hours) runs, brings it back under a
/// shorter one, but never once it played in the radio's session; twice
/// running, it plays first. A mix lets the hour-old one rest.
#[test]
fn a_radio_repeats_after_five_and_a_mix_rests() {
    let now = NOW;
    let cat = Catalog::new((0..30u128).map(|i| info(i, &[i as u32], None)));
    let idx = VectorIndex::new();
    let fav = tid(3);
    let input = TasteInput {
        events: vec![
            Play::new(fav, now - 2 * HOUR_MS, EventType::Start, None),
            Play::new(fav, now - HOUR_MS, EventType::Complete, None),
        ],
        likes: vec![fav],
        ..TasteInput::default()
    };
    let taste = Taste::build(&cat, &input, now);
    let recs = Recs::new(&cat, &taste, &idx);
    let params = RadioParams { new_share: 0.0, seed: 1 };
    let has = |picks: &[Pick]| picks.iter().any(|p| p.item == Item::Track(fav));

    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    assert!(!has(&radio.extend(&recs, 5, &params)), "played an hour ago: it rests three hours");

    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.set_no_repeat_ms(HOUR_MS / 2);
    assert!(has(&radio.extend(&recs, 5, &params)), "played an hour ago: back after half an hour");

    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.set_no_repeat_ms(HOUR_MS / 2);
    radio.feedback(Item::Track(fav), Outcome::Complete);
    assert!(!has(&radio.extend(&recs, 5, &params)), "one of the last five");
    for k in 20..25 {
        radio.feedback(Item::Track(tid(k)), Outcome::Complete);
    }
    assert!(!has(&radio.extend(&recs, 5, &params)), "played in the session: it rests even five others later");

    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.feedback(Item::Track(fav), Outcome::Complete);
    radio.feedback(Item::Track(fav), Outcome::Complete);
    assert_eq!(radio.extend(&recs, 5, &params)[0].item, Item::Track(fav), "twice running: once more, first");

    let mut mix = RadioState::new(RadioSeed::Daylist { part: crate::taste::DayPart::Evening, centre: [0.0; 3] });
    assert!(!has(&mix.extend(&recs, 5, &params)), "a mix lets it rest");
}

/// «После A»: the sound of A's liked core (region +x), none of A, and
/// no other world over a quarter.
#[test]
fn after_a_world_comes_its_sound_from_other_worlds() {
    let (cat, idx) = two_regions();
    let worlds = ["a", "b", "c", "d"];
    let world = |i: u128| if i <= 30 { "a" } else if i <= 60 { worlds[1 + (i % 3) as usize] } else { "e" };
    let cat = Catalog::new(cat.tracks().iter().map(|t| {
        let i = t.id.0.as_u128();
        if i == 0 { t.clone() } else { t.clone().with_world(&format!("{}{}", world(i), if i > 60 { i % 8 } else { 0 })) }
    }));
    let likes: Vec<TrackId> = (1..=10).map(tid).collect();
    let taste = Taste::build(&cat, &TasteInput { likes, ..Default::default() }, NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let a = crate::world_token("a0");
    let mix = recs.after_world(a, 20, 0.25, &RadioParams { new_share: 0.0, seed: 3 });
    assert_eq!(mix.len(), 20);
    let ids: Vec<u128> = mix.iter().map(|t| t.0.as_u128()).collect();
    assert!(ids.iter().all(|&i| i > 30 || i == 0), "none of A: {ids:?}");
    assert!(ids.iter().filter(|&&i| i <= 60).count() >= 12, "its sound first: {ids:?}");
    let mut per: HashMap<&str, usize> = HashMap::new();
    for &i in &ids {
        *per.entry(world(i)).or_default() += usize::from(i <= 60);
    }
    assert!(per.values().all(|&n| n <= 5), "a quarter at most: {per:?}");
}

/// A track's radio by the user model, on the listener of albums: the
/// album being played goes on in its order first.
#[test]
fn a_model_radio_continues_the_album() {
    use crate::model::{ModelInput, UserModel};
    use nikgob_core::model::EventContext;
    let (cat, idx, history) = crate::model::tests::album_listener(15, 4);
    let now = crate::model::tests::T0 + crate::taste::DAY_MS;
    let model = UserModel::build(&cat, &history, ModelInput { index: &idx, types: None, weights: None, graph: None }, now);
    let taste = Taste::build(&cat, &history, now);
    let recs = Recs::new(&cat, &taste, &idx).with_model(&model);
    let mut session = crate::session::Session::default();
    session.start(tid(42), now - 400_000, EventContext::Album, None);
    session.end(tid(42), crate::session::Ended::Complete);
    session.start(tid(43), now - 200_000, EventContext::Album, None);
    let mut radio = RadioState::new(RadioSeed::Track(tid(43)));
    radio.attune(session, None);
    let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 1 });
    let ids: Vec<u128> = picks.iter().map(|p| match p.item { Item::Track(t) => t.0.as_u128(), Item::Stream(_) => 0 }).collect();
    assert_eq!(&ids[..3], &[44, 45, 46], "{ids:?}");
    assert_eq!(picks[0].reason, Reason::NextOnAlbum);
    assert_eq!(picks.len(), 10);
    let unique: HashSet<u128> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 10);
}

/// Two early skips running in one sound: the model radio leaves it.
#[test]
fn two_early_skips_move_a_model_radio() {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx) = two_regions();
    let input = TasteInput::default();
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, NOW);
    let taste = Taste::build(&cat, &input, NOW);
    let recs = Recs::new(&cat, &taste, &idx).with_model(&model);
    let params = RadioParams { new_share: 0.0, seed: 3 };
    // A complete in A before the seed: the radio goes on in A…
    let mut liked = RadioState::new(RadioSeed::Track(tid(0)));
    liked.feedback(Item::Track(tid(9)), Outcome::Complete);
    let a = liked.extend(&recs, 10, &params).iter().filter(|p| in_a(p)).count();
    assert!(a >= 8, "a complete in A: {a}");
    // …unless two of A were skipped early since.
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.feedback(Item::Track(tid(9)), Outcome::Complete);
    radio.feedback(Item::Track(tid(5)), Outcome::skip(8_000));
    radio.feedback(Item::Track(tid(7)), Outcome::skip(6_000));
    let a = radio.extend(&recs, 10, &params).iter().filter(|p| in_a(p)).count();
    assert!(a <= 2, "two skips in A move away from A: {a}");
}

#[test]
fn radio_never_repeats_survives_serde_and_is_deterministic() {
    let (cat, idx) = two_regions();
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let params = RadioParams { new_share: 0.2, seed: 9 };
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.add_new((0..30).map(|k| NewTrack { work: String::new(),
        id: RemoteId::new(Service::Youtube, format!("v{k}")),
        artist: format!("new artist {k}"),
        album: None,
        vector: Some(point(0.5, 0.0)),
    }));
    let mut seen = HashSet::new();
    let mut all = Vec::new();
    for _ in 0..4 {
        let json = serde_json::to_string(&radio).unwrap();
        radio = serde_json::from_str(&json).unwrap();
        let picks = radio.extend(&recs, 10, &params);
        assert_eq!(picks.len(), 10);
        assert_eq!(picks.iter().filter(|p| matches!(p.item, Item::Stream(_))).count(), 2, "a fifth is new");
        for p in &picks {
            assert!(seen.insert(p.item.clone()), "{:?} twice", p.item);
            assert_ne!(p.item, Item::Track(tid(0)), "the seed is not repeated");
        }
        all.extend(picks);
    }
    assert_diverse(&recs, &all);
    // The same seed, the same radio.
    let mut again = RadioState::new(RadioSeed::Track(tid(0)));
    again.add_new((0..30).map(|k| NewTrack { work: String::new(),
        id: RemoteId::new(Service::Youtube, format!("v{k}")),
        artist: format!("new artist {k}"),
        album: None,
        vector: Some(point(0.5, 0.0)),
    }));
    let first: Vec<Pick> = (0..4).flat_map(|_| again.extend(&recs, 10, &params)).collect();
    assert_eq!(first, all);
}

#[test]
fn a_small_library_keeps_playing() {
    let cat = Catalog::new((0..8u128).map(|i| info(i, &[i as u32], None)));
    let idx = VectorIndex::new();
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    for _ in 0..20 {
        assert_eq!(radio.extend(&recs, 5, &RadioParams { new_share: 0.0, seed: 1 }).len(), 5);
    }
}

#[test]
fn artist_radio_is_mostly_the_artist() {
    let (cat, idx) = {
        // Artist 7 has 40 tracks; 60 others by 30 artists around them.
        let mut tracks: Vec<TrackInfo> = (0..40u128).map(|i| info(i, &[7], Some(900 + i / 4))).collect();
        tracks.extend((40..100u128).map(|i| info(i, &[100 + (i as u32 % 30)], None)));
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        for i in 0..100u128 {
            idx.insert(tid(i), point((i % 10) as f32, (i / 10) as f32));
        }
        (Catalog::new(tracks), idx)
    };
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let mut radio = RadioState::new(RadioSeed::Artist(aid(7)));
    let picks = radio.extend(&recs, 20, &RadioParams { new_share: 0.0, seed: 5 });
    let own = picks
        .iter()
        .filter(|p| matches!(&p.item, Item::Track(id) if id.0.as_u128() < 40))
        .count();
    assert_eq!(own, 14, "70% of 20");
}

#[test]
fn moods_take_their_tail_and_skip_dislikes() {
    // Energy grows with the track number (tempo, ZCR, centroid,
    // loudness together); half of the tracks liked.
    let n = 90u128;
    let cat = Catalog::new((0..n).map(|i| info(i, &[i as u32], None)));
    let mut idx = VectorIndex::new();
    let mut rng = Rng::new(4);
    for i in 0..n {
        let e = i as f32 / n as f32;
        let mut v = [0.0f32; DIM];
        for f in v.iter_mut() {
            *f = rng.f32() * 0.002;
        }
        for f in [0, 1, 2, 8] {
            v[f] += e;
        }
        idx.insert(tid(i), v);
    }
    let input = TasteInput {
        disliked_tracks: vec![tid(89)],
        ..Default::default()
    };
    let taste = Taste::build(&cat, &input, NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let pool = recs.mood_pool(Mood::Energetic);
    assert_eq!(pool.len(), 30, "a third (the disliked one left out)");
    assert!(pool.iter().all(|(id, _)| id.0.as_u128() >= 59 && id.0.as_u128() != 89));
    let mut energetic = RadioState::new(RadioSeed::Mood(Mood::Energetic));
    let picks = energetic.extend(&recs, 15, &RadioParams { new_share: 0.0, seed: 2 });
    assert_eq!(picks.len(), 15);
    for p in &picks {
        let Item::Track(id) = &p.item else { panic!() };
        assert!(id.0.as_u128() >= 59, "energetic tail: {}", id.0.as_u128());
        assert_eq!(p.reason, Reason::Mood(Mood::Energetic));
    }
    let mut calm = RadioState::new(RadioSeed::Mood(Mood::Calm));
    for p in calm.extend(&recs, 15, &RadioParams { new_share: 0.0, seed: 2 }) {
        let Item::Track(id) = &p.item else { panic!() };
        assert!(id.0.as_u128() < 30, "calm tail: {}", id.0.as_u128());
    }
}

#[test]
fn mood_scoring_prefers_affinity() {
    // A flat mood pool: the liked half wins.
    let n = 300u128;
    let cat = Catalog::new((0..n).map(|i| info(i, &[i as u32], None)));
    let mut idx = VectorIndex::new();
    let mut rng = Rng::new(8);
    for i in 0..n {
        idx.insert(tid(i), std::array::from_fn(|_| rng.f32()));
    }
    let likes: Vec<TrackId> = (0..n).filter(|i| i % 2 == 0).map(tid).collect();
    let taste = Taste::build(&cat, &TasteInput { likes, ..Default::default() }, NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let mut radio = RadioState::new(RadioSeed::Mood(Mood::Focus));
    let picks = radio.extend(&recs, 20, &RadioParams { new_share: 0.0, seed: 6 });
    let liked = picks.iter().filter(|p| matches!(&p.item, Item::Track(id) if id.0.as_u128() % 2 == 0)).count();
    assert!(liked >= 15, "{liked}");
}

/// The shape of a real autoplay that left its scene: a big cluster of
/// Japanese songs played most (a calm mix of it was the session), and a
/// Russian rap artist [`SEED_ARTIST`] of three tracks sounding like it,
/// one more artist the graph links to it, and a little other Russian
/// music far in sound. The user also goes on from Russian to Japanese.
/// Autoplay after a list of Japanese pop and then rock (one pick of
/// the user's, the queue going on) goes on by the list's trajectory:
/// both of its sounds and nothing it never had. A list the user's own
/// pick began at its last track goes on as that track's radio.
#[test]
fn autoplay_goes_on_by_the_trajectory() {
    use crate::model::{ModelInput, UserModel};
    use nikgob_core::model::{EventContext, EventOrigin};
    let (cat, idx, input, types) = listener(true);
    let now = crate::model::tests::T0;
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
    let list = [JPOP.0 + 1, JPOP.0 + 13, JPOP.0 + 25, ROCK.0 + 2, ROCK.0 + 14, ROCK.0 + 26];
    let autoplay = |last_by_hand: bool, skipped: Option<usize>| -> (Vec<u128>, usize) {
        let mut session = Session::default();
        let mut ts = now - 30 * 60_000;
        for (k, &t) in list.iter().enumerate() {
            let by_hand = k == 0 || (last_by_hand && k + 1 == list.len());
            session.start(tid(t), ts, EventContext::Playlist, Some(if by_hand { EventOrigin::Manual } else { EventOrigin::Queue }));
            ts += 200_000;
            if skipped == Some(k) {
                // Skipped past its half: not rejected, not played through.
                session.end_at(tid(t), Ended::Late, 120_000, Some(200_000), None);
            } else {
                session.end(tid(t), Ended::Complete);
            }
        }
        let mut radio = RadioState::new(RadioSeed::Track(tid(list[5]))).continuing();
        radio.mark_served(list.iter().map(|t| Item::Track(tid(*t))));
        radio.attune(session, None);
        let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 3 });
        let ids = picks.iter().filter_map(|p| match p.item {
            Item::Track(t) => Some(t.0.as_u128()),
            Item::Stream(_) => None,
        });
        (ids.collect(), radio.trajectory().len())
    };
    let (picks, traj) = autoplay(false, None);
    assert_eq!(traj, 6, "the whole list");
    let kinds: Vec<u128> = picks.iter().map(|t| kind_of(*t)).collect();
    assert!(kinds.iter().all(|k| *k == JPOP.0 || *k == ROCK.0), "{kinds:?}");
    assert!(kinds.contains(&JPOP.0) && kinds.contains(&ROCK.0), "{kinds:?}");
    let (alone, traj) = autoplay(true, None);
    assert_eq!(traj, 0, "the user's own pick began a list of one");
    assert!(alone.iter().filter(|t| kind_of(**t) == ROCK.0).count() >= 8, "{alone:?}");
    let (_, traj) = autoplay(false, Some(2));
    assert_eq!(traj, 5, "a track skipped at 60% is out of the trajectory");
}

/// A list's radio («Радио по альбому», «Радио по плейлисту») blends its
/// tracks from the start: a list of Japanese pop and rock brings both
/// and nothing it never had, without its own tracks over and over; a
/// list of one kind keeps to it; a long list is spread to
/// [`BLEND_MAX`] tracks; a list of one track is that track's radio.
#[test]
fn a_list_radio_blends_its_tracks() {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx, input, types) = listener(true);
    let now = crate::model::tests::T0;
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
    let play = |list: &[u128]| -> Vec<u128> {
        let ids: Vec<TrackId> = list.iter().map(|t| tid(*t)).collect();
        let mut radio = RadioState::of_list(&ids);
        let picks = radio.extend(&recs, 12, &RadioParams { new_share: 0.0, seed: 5 });
        picks
            .iter()
            .filter_map(|p| match p.item {
                Item::Track(t) => Some(t.0.as_u128()),
                Item::Stream(_) => None,
            })
            .collect()
    };
    let both = play(&[JPOP.0 + 1, JPOP.0 + 13, JPOP.0 + 25, ROCK.0 + 2, ROCK.0 + 14, ROCK.0 + 26]);
    let kinds: Vec<u128> = both.iter().map(|t| kind_of(*t)).collect();
    assert!(kinds.len() >= 10, "{both:?}");
    assert!(kinds.iter().all(|k| *k == JPOP.0 || *k == ROCK.0), "{kinds:?}");
    assert!(kinds.contains(&JPOP.0) && kinds.contains(&ROCK.0), "{kinds:?}");
    let rock = play(&[ROCK.0 + 2, ROCK.0 + 14, ROCK.0 + 26]);
    assert!(rock.iter().all(|t| kind_of(*t) == ROCK.0), "{rock:?}");
    let one = play(&[ROCK.0 + 2]);
    assert!(!one.is_empty() && !one.contains(&(ROCK.0 + 2)), "{one:?}");
    let long: Vec<TrackId> = (0..100).map(|k| tid(JPOP.0 + k % 40)).collect();
    let RadioSeed::Tracks(sample) = RadioState::of_list(&long).seed else { panic!() };
    assert_eq!(sample.len(), BLEND_MAX);
    assert_eq!(sample[0], tid(JPOP.0));
    assert!(sample.windows(2).all(|w| w[0] != w[1]));
}

const JP: (u128, u128, f32) = (2_000, 150, 10.0);
const SEED_ARTIST: u32 = 900;
const LINKED_ARTIST: u32 = 901;
const SEED_TRACKS: [u128; 3] = [100, 101, 102];
const LINKED_TRACKS: [u128; 5] = [110, 111, 112, 113, 114];
const OTHER_RU: (u128, u128) = (120, 10);

fn russian(id: u128) -> bool {
    (100..130).contains(&id) || id == 103
}

fn sparse_seed() -> (Listener, crate::graph::GraphInput) {
    use crate::songtype::{Lang, SongType, SongTypes, Voice};
    let mut rng = Rng::new(31);
    let mut tracks = Vec::new();
    let mut idx = VectorIndex::with_weights([1.0; DIM]);
    let mut types = SongTypes::default();
    let mut add = |id: u128, artist: u32, x: f32, lang: Lang, rng: &mut Rng| {
        tracks.push(info(id, &[artist], Some(id / 6)).with_world(&format!("w{artist}")).with_duration(Some(200_000)));
        idx.insert(tid(id), point(x + rng.f32() * 0.6, rng.f32() * 0.6));
        types.by_track.insert(tid(id), SongType { voice: Voice::Sung, lang: Some(lang), ..SongType::default() });
    };
    for k in 0..JP.1 {
        add(JP.0 + k, 2_000 + (k / 6) as u32, JP.2, Lang::Japanese, &mut rng);
    }
    for id in SEED_TRACKS {
        add(id, SEED_ARTIST, JP.2 + 0.3, Lang::Russian, &mut rng);
    }
    for id in LINKED_TRACKS {
        add(id, LINKED_ARTIST, -5.0, Lang::Russian, &mut rng);
    }
    for k in 0..OTHER_RU.1 {
        add(OTHER_RU.0 + k, 902 + (k / 5) as u32, -8.0, Lang::Russian, &mut rng);
    }
    let mut events = Vec::new();
    let mut ts = crate::model::tests::T0 - 20 * crate::taste::DAY_MS;
    let mut play = |id: u128, ts: &mut i64| {
        events.push(Play { context: nikgob_core::model::EventContext::Library, ..Play::new(tid(id), *ts, EventType::Start, Some(0)) });
        *ts += 200_000;
        events.push(Play::new(tid(id), *ts, EventType::Complete, Some(200_000)));
    };
    for _ in 0..30 {
        for _ in 0..6 {
            play(JP.0 + rng.below(JP.1 as usize) as u128, &mut ts);
        }
        ts += 4 * crate::taste::HOUR_MS;
    }
    // Russian, then on to Japanese: the lanes go for this user.
    for _ in 0..12 {
        for _ in 0..2 {
            play(OTHER_RU.0 + rng.below(OTHER_RU.1 as usize) as u128, &mut ts);
        }
        for _ in 0..3 {
            play(JP.0 + rng.below(JP.1 as usize) as u128, &mut ts);
        }
        ts += 4 * crate::taste::HOUR_MS;
    }
    events.sort_by_key(|p| p.ts);
    let graph = crate::graph::GraphInput {
        artists: vec![crate::graph::Edge {
            from: crate::test_util::aid(SEED_ARTIST),
            to: crate::test_util::aid(LINKED_ARTIST),
            rank: 0,
            of: 10,
            source: crate::graph::GraphSource::DeezerArtists,
        }],
        ..crate::graph::GraphInput::default()
    };
    ((Catalog::new(tracks), idx, TasteInput { events, ..TasteInput::default() }, types), graph)
}

/// Autoplay after a session of a calm Japanese mix and an album of the
/// seed's artist: `facts` for a seed the catalog lacks. Thirty picks.
fn sparse_autoplay(seed: u128, played: &[u128], facts: Option<SeedFacts>) -> Vec<u128> {
    sparse_autoplay_with(seed, played, facts, |_| {})
}

/// [`sparse_autoplay`] with the radio's knobs set by `tune`.
fn sparse_autoplay_with(seed: u128, played: &[u128], facts: Option<SeedFacts>, tune: fn(&mut RadioState)) -> Vec<u128> {
    use crate::model::{ModelInput, UserModel};
    let ((cat, idx, input, types), graph) = sparse_seed();
    let now = crate::model::tests::T0;
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: Some(&graph) }, now);
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
    let mut session = Session::default();
    let mut ts = now - 30 * 60_000;
    for k in 0..6 {
        session.start(tid(JP.0 + 7 * k), ts, nikgob_core::model::EventContext::HomeSection, None);
        ts += 200_000;
        session.end(tid(JP.0 + 7 * k), Ended::Complete);
    }
    for &t in played.iter().chain([&seed]) {
        session.start(tid(t), ts, nikgob_core::model::EventContext::Album, Some(nikgob_core::model::EventOrigin::Manual));
        ts += 120_000;
        session.end(tid(t), Ended::Complete);
    }
    let mut radio = RadioState::new(RadioSeed::Track(tid(seed))).continuing();
    if let Some(f) = facts {
        radio = radio.with_seed_facts(f);
    }
    radio.mark_served(played.iter().map(|t| Item::Track(tid(*t))));
    tune(&mut radio);
    let mut out = Vec::new();
    for _ in 0..3 {
        radio.attune(session.clone(), None);
        for p in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 9 }) {
            let Item::Track(t) = p.item else { continue };
            ts += 200_000;
            session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
            session.end(t, Ended::Complete);
            radio.feedback(p.item.clone(), Outcome::Complete);
            out.push(t.0.as_u128());
        }
    }
    out
}

/// The seed's artist has no tracks left and nothing of its scene sounds
/// like it: autoplay goes on with the artist the graph links to it,
/// then the rest of its language, not with the Japanese the session's
/// mix was of; Japanese only once the Russian is played through.
#[test]
fn a_sparse_seed_keeps_to_its_scene() {
    let picks = sparse_autoplay(102, &[100, 101], None);
    let ru = LINKED_TRACKS.len() + OTHER_RU.1 as usize;
    assert!(picks.len() >= 25, "{picks:?}");
    assert!(picks[..ru].iter().all(|t| russian(*t)), "the Russian first: {picks:?}");
    assert!(LINKED_TRACKS.iter().all(|t| picks[..8].contains(t)), "the linked artist first: {picks:?}");
}

/// The same seed as a stream the catalog lacks (played from its album's
/// page), known only by its artist and sound: its artist's own tracks
/// and its scene come first.
#[test]
fn a_seed_the_catalog_lacks_keeps_to_its_artists_scene() {
    let facts = SeedFacts { artists: vec![crate::test_util::aid(SEED_ARTIST)], vector: Some(point(JP.2 + 0.3, 0.3)), lane: None };
    let picks = sparse_autoplay(103, &[], Some(facts));
    assert!(picks.len() >= 25, "{picks:?}");
    let ru = SEED_TRACKS.len() + LINKED_TRACKS.len() + OTHER_RU.1 as usize;
    assert!(picks[..ru].iter().all(|t| russian(*t)), "the Russian first: {picks:?}");
    assert!(SEED_TRACKS.iter().all(|t| picks[..8].contains(t)), "its artist first: {picks:?}");
}

/// What the session played (and what played within «Не повторять
/// сыгранное») is not picked again while anything else is left; with
/// nothing else left it comes back.
#[test]
fn autoplay_leaves_out_what_played_lately() {
    use crate::model::{ModelInput, UserModel};
    let ((cat, idx, mut input, types), graph) = sparse_seed();
    let now = crate::model::tests::T0;
    // A Japanese session of twenty-four, the seed last: the history
    // the model is built from has it, the session keeps its last twenty.
    let played: Vec<u128> = (0..24).map(|k| JP.0 + 3 * k).collect();
    let mut ts = now - 90 * 60_000;
    for &t in &played {
        input.events.push(Play::new(tid(t), ts, EventType::Start, Some(0)));
        input.events.push(Play::new(tid(t), ts + 200_000, EventType::Complete, Some(200_000)));
        ts += 200_000;
    }
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: Some(&graph) }, now);
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
    let mut session = Session::default();
    let mut ts = now - 90 * 60_000;
    for &t in &played {
        session.start(tid(t), ts, nikgob_core::model::EventContext::Library, Some(nikgob_core::model::EventOrigin::Manual));
        ts += 200_000;
        session.end(tid(t), Ended::Complete);
    }
    let seed = *played.last().unwrap();
    let mut radio = RadioState::new(RadioSeed::Track(tid(seed))).continuing();
    let mut picks = Vec::new();
    for _ in 0..5 {
        radio.attune(session.clone(), None);
        for p in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 4 }) {
            let Item::Track(t) = p.item else { continue };
            ts += 200_000;
            session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
            session.end(t, Ended::Complete);
            radio.feedback(p.item.clone(), Outcome::Complete);
            picks.push(t.0.as_u128());
        }
    }
    assert!(picks.len() >= 45, "{picks:?}");
    assert!(picks.iter().all(|t| !played.contains(t)), "nothing of the session again: {picks:?}");

    // A library of four, all played in the session: they come back.
    let small = Catalog::new((0..4u128).map(|i| info(i, &[i as u32], None)));
    let taste = Taste::build(&small, &TasteInput::default(), NOW);
    let empty = VectorIndex::new();
    let recs = Recs::new(&small, &taste, &empty);
    let mut session = Session::default();
    for i in 0..4u128 {
        session.start(tid(i), NOW - 60_000 * (5 - i as i64), nikgob_core::model::EventContext::Library, None);
        session.end(tid(i), Ended::Complete);
    }
    let mut radio = RadioState::new(RadioSeed::Track(tid(3)));
    radio.attune(session, None);
    assert!(!radio.extend(&recs, 3, &RadioParams { new_share: 0.0, seed: 1 }).is_empty(), "a small library keeps playing");
}

fn stream(tag: &str, k: usize, vector: Option<Vector>) -> NewTrack {
    NewTrack { work: String::new(),
        id: RemoteId::new(Service::Youtube, format!("{tag}{k}")),
        artist: format!("{tag} artist {k}"),
        album: None,
        vector,
    }
}

/// New music of `picks` whose id starts with `tag`.
fn streams_of(picks: &[Pick], tag: &str) -> usize {
    picks.iter().filter(|p| matches!(&p.item, Item::Stream(r) if r.id.starts_with(tag))).count()
}

/// «Подмешивать случайное»: «Никогда» brings no new music that is not
/// known to sound like the seed, neither in a batch nor joining later,
/// while what sounds like it still comes; «Редко» one in ten; no limit,
/// the share of new music as before.
#[test]
fn explore_limits_the_blind_new_music() {
    let (cat, idx) = two_regions();
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let params = RadioParams { new_share: 0.2, seed: 9 };
    let mut never = RadioState::new(RadioSeed::Track(tid(0)));
    never.set_explore(Some(0.0));
    never.add_new((0..30).map(|k| stream("b", k, None)));
    for _ in 0..4 {
        let picks = never.extend(&recs, 10, &params);
        assert_eq!(picks.len(), 10, "the library fills in");
        assert_eq!(streams_of(&picks, "b"), 0, "{picks:?}");
    }
    assert!(never.take_new(&recs, 2, 10, 0).is_empty(), "nor later");
    never.add_new((0..30).map(|k| stream("n", k, Some(point(0.0, 0.0)))));
    let picks = never.extend(&recs, 10, &params);
    assert_eq!(streams_of(&picks, "n"), 2, "what sounds like the seed comes: {picks:?}");
    assert_eq!(streams_of(&picks, "b"), 0);

    let mut rare = RadioState::new(RadioSeed::Track(tid(0)));
    rare.set_explore(Some(0.1));
    rare.add_new((0..30).map(|k| stream("b", k, None)));
    let mut plain = RadioState::new(RadioSeed::Track(tid(0)));
    plain.add_new((0..30).map(|k| stream("b", k, None)));
    for _ in 0..4 {
        assert_eq!(streams_of(&rare.extend(&recs, 10, &params), "b"), 1);
        assert_eq!(streams_of(&plain.extend(&recs, 10, &params), "b"), 2);
    }
    // Joining later, the new music queued counts as blind.
    assert!(rare.take_new(&recs, 2, 10, 1).is_empty());
    assert_eq!(rare.take_new(&recs, 2, 10, 0).len(), 1);
    // The knob survives a restart.
    let json = serde_json::to_string(&rare).unwrap();
    let back: RadioState = serde_json::from_str(&json).unwrap();
    assert_eq!((back.explore, back.unheard), (Some(0.1), 0.0));
}

/// The same in a radio by the user model: «Никогда» no blind new music
/// at all, «Редко» one in ten at most, what sounds like the seed comes.
#[test]
fn explore_limits_a_model_radio_too() {
    use crate::model::{ModelInput, UserModel};
    let (cat, idx, input, types) = listener(true);
    let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, crate::model::tests::T0);
    let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
    let seed = OST.0 + 3;
    let params = RadioParams { new_share: 0.2, seed: 2 };
    for (explore, most) in [(Some(0.0), 0), (Some(0.1), 1), (None, 2)] {
        let mut radio = RadioState::new(RadioSeed::Track(tid(seed)));
        radio.set_explore(explore);
        radio.add_new((0..40).map(|k| stream("b", k, None)));
        let mut total = 0;
        for _ in 0..4 {
            let picks = radio.extend(&recs, 10, &params);
            assert_eq!(picks.len(), 10);
            let blind = streams_of(&picks, "b");
            assert!(blind <= most, "{explore:?}: {blind} blind");
            total += blind;
        }
        assert_eq!(total == 0, most == 0, "{explore:?}: {total} blind");
    }
    let mut radio = RadioState::new(RadioSeed::Track(tid(seed)));
    radio.set_explore(Some(0.0));
    radio.add_new((0..40).map(|k| stream("b", k, None)));
    radio.add_new((0..40).map(|k| stream("n", k, idx.get(tid(seed)).copied())));
    let picks: Vec<Pick> = (0..4).flat_map(|_| radio.extend(&recs, 10, &params)).collect();
    assert!(streams_of(&picks, "n") > 0, "what sounds like the seed comes");
    assert_eq!(streams_of(&picks, "b"), 0);
}

/// «Только новые»: a radio takes only what the user has not heard (a
/// dislike still kept out) while there is, then the usual picks, never
/// stopping; with new music that sounds like the seed, that stands in
/// for what was heard; with only what has a file, the usual picks.
#[test]
fn only_new_takes_the_unheard_then_falls_back() {
    let (cat, idx) = two_regions();
    let mut input = TasteInput { disliked_tracks: vec![tid(101)], ..TasteInput::default() };
    for i in 1..=100u128 {
        let ts = NOW - 10 * crate::taste::DAY_MS + i as i64 * 1_000_000;
        input.events.push(Play::new(tid(i), ts, EventType::Start, Some(0)));
        input.events.push(Play::new(tid(i), ts + 200_000, EventType::Complete, Some(200_000)));
    }
    let taste = Taste::build(&cat, &input, NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let unheard = |p: &Pick| match &p.item {
        Item::Track(t) => t.0.as_u128() > 100,
        Item::Stream(_) => true,
    };
    let count = |picks: &[Pick]| picks.iter().filter(|p| unheard(p)).count();
    let disliked = |picks: &[Pick]| picks.iter().any(|p| p.item == Item::Track(tid(101)));

    // Nineteen unheard (one disliked), then the heard ones.
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.set_unheard(1.0);
    let params = RadioParams { new_share: 0.0, seed: 3 };
    let batches: Vec<Vec<Pick>> = (0..3).map(|_| radio.extend(&recs, 10, &params)).collect();
    assert!(batches.iter().all(|b| b.len() == 10), "it goes on: {batches:?}");
    assert_eq!(batches.iter().map(|b| count(b)).collect::<Vec<_>>(), vec![10, 9, 0], "{batches:?}");
    assert!(!batches.iter().any(|b| disliked(b)));

    // New music that sounds like the seed stands in for what was heard;
    // none of the blind.
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.set_unheard(1.0);
    radio.set_explore(Some(0.0));
    radio.add_new((0..30).map(|k| stream("n", k, Some(point(0.0, 0.0)))));
    radio.add_new((0..30).map(|k| stream("b", k, None)));
    let params = RadioParams { new_share: 0.2, seed: 3 };
    let picks: Vec<Pick> = (0..4).flat_map(|_| radio.extend(&recs, 10, &params)).collect();
    assert_eq!(picks.len(), 40);
    assert_eq!(count(&picks), 40, "{picks:?}");
    assert_eq!(streams_of(&picks, "b"), 0);
    assert!(!disliked(&picks));

    // Only what has a file, and the unheard have none: no new music,
    // the usual picks.
    let local = Catalog::new(cat.tracks().iter().cloned().map(|mut t| {
        t.local = t.id.0.as_u128() <= 100;
        t
    }));
    let taste = Taste::build(&local, &input, NOW);
    let recs = Recs::new(&local, &taste, &idx).local_only(true);
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.set_unheard(1.0);
    radio.add_new((0..30).map(|k| stream("n", k, Some(point(0.0, 0.0)))));
    for _ in 0..3 {
        let picks = radio.extend(&recs, 10, &params);
        assert_eq!(picks.len(), 10, "it goes on");
        assert!(picks.iter().all(|p| !unheard(p)), "{picks:?}");
    }
}

/// «Больше новых»: at least half of the library's picks unheard while
/// there are.
#[test]
fn more_new_takes_half_unheard() {
    let (cat, idx) = two_regions();
    let mut input = TasteInput::default();
    for i in 1..=100u128 {
        let ts = NOW - 10 * crate::taste::DAY_MS + i as i64 * 1_000_000;
        input.events.push(Play::new(tid(i), ts, EventType::Start, Some(0)));
        input.events.push(Play::new(tid(i), ts + 200_000, EventType::Complete, Some(200_000)));
    }
    let taste = Taste::build(&cat, &input, NOW);
    let recs = Recs::new(&cat, &taste, &idx);
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.set_unheard(0.5);
    let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 3 });
    assert_eq!(picks.len(), 10);
    let fresh = picks.iter().filter(|p| matches!(&p.item, Item::Track(t) if t.0.as_u128() > 100)).count();
    assert!(fresh >= 5, "{fresh} unheard: {picks:?}");
}

/// The knobs keep a model radio's rules: with «Только новые» and
/// «Никогда» a soundtrack's radio still brings only soundtracks, never a
/// world thrice running, and no fewer unheard than usual.
#[test]
fn the_knobs_keep_a_model_radios_rules() {
    use crate::model::{ModelInput, UserModel};
    for omnivore in [false, true] {
        let l = listener(omnivore);
        let (cat, idx, input, types) = &l;
        let model = UserModel::build(cat, input, ModelInput { index: idx, types: Some(types), weights: None, graph: None }, crate::model::tests::T0);
        let recs = Recs::new(cat, &model.taste, idx).with_model(&model);
        let fresh = |picks: &[u128]| picks.iter().filter(|t| cat.position(tid(**t)).is_some_and(|i| !recs.heard(i))).count();
        // The same «Подмешивать случайное» on both sides: blind service
        // tracks are unheard too and would tip the count.
        let usual = play_model_radio_with(&l, OST.0 + 3, 3, 0, |r| r.set_explore(Some(0.0)));
        let only = play_model_radio_with(&l, OST.0 + 3, 3, 0, |r| {
            r.set_unheard(1.0);
            r.set_explore(Some(0.0));
        });
        assert!(only.len() >= 25, "{only:?}");
        assert!(only.iter().all(|t| kind_of(*t) == OST.0), "omnivore {omnivore}: {only:?}");
        let worlds: Vec<u128> = only.iter().map(|t| (t - OST.0) / 20).collect();
        for w in worlds.windows(3) {
            assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
        }
        assert!(fresh(&only) >= fresh(&usual), "omnivore {omnivore}: {} unheard of {}, usually {} of {}", fresh(&only), only.len(), fresh(&usual), usual.len());
    }
}

/// The knobs keep a sparse seed's scene: with «Только новые» and
/// «Никогда» its language still comes before another.
#[test]
fn the_knobs_keep_a_sparse_seeds_scene() {
    let picks = sparse_autoplay_with(102, &[100, 101], None, |r| {
        r.set_unheard(1.0);
        r.set_explore(Some(0.0));
    });
    let ru = LINKED_TRACKS.len() + OTHER_RU.1 as usize;
    assert!(picks.len() >= 25, "{picks:?}");
    assert!(picks[..ru].iter().all(|t| russian(*t)), "the Russian first: {picks:?}");
}

/// New songs of a radio are one song across their versions too: «Sora»
/// and «Sora (Off Vocal)» never both, in one extension or across them.
#[test]
fn a_radios_new_songs_keep_one_version() {
    use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamTrack};
    let cat = Catalog::new((0..6u128).map(|i| info(i, &[i as u32], None)));
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let empty = VectorIndex::new();
    let recs = Recs::new(&cat, &taste, &empty);
    let song = |k: usize, title: &str| {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, format!("s{k}")), title);
        t.artists.push(ArtistRef::new(None, "Singer"));
        NewTrack::from_stream(&t, None)
    };
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.add_new([song(0, "Sora"), song(1, "Sora (Off Vocal)"), song(2, "Umi"), song(3, "Umi (TV Size)"), song(4, "Kaze")]);
    let mut works: Vec<String> = Vec::new();
    for round in 0..4 {
        for p in radio.extend(&recs, 3, &RadioParams { new_share: 1.0, seed: round }) {
            if let Item::Stream(r) = &p.item {
                let n: usize = r.id[1..].parse().unwrap();
                let w = ["sora", "sora", "umi", "umi", "kaze"][n].to_owned();
                assert!(!works.contains(&w), "two versions of {w}: round {round}");
                works.push(w);
            }
        }
    }
    assert!(!works.is_empty());
}

/// New songs spliced into a radio's queue later ([`RadioState::take_new`])
/// keep one version of a song too: not another version of one served,
/// nor two versions at once.
#[test]
fn new_songs_spliced_later_keep_one_version() {
    use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamTrack};
    let cat = Catalog::new((0..6u128).map(|i| info(i, &[i as u32], None)));
    let taste = Taste::build(&cat, &TasteInput::default(), NOW);
    let empty = VectorIndex::new();
    let recs = Recs::new(&cat, &taste, &empty);
    let song = |k: usize, title: &str| {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, format!("s{k}")), title);
        t.artists.push(ArtistRef::new(None, "Singer"));
        NewTrack::from_stream(&t, None)
    };
    let id = |k: usize| Item::Stream(RemoteId::new(Service::Youtube, format!("s{k}")));
    let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
    radio.add_new([song(0, "Sora")]);
    let first = radio.take_new(&recs, 1, 4, 0);
    assert_eq!(first.iter().map(|p| p.item.clone()).collect::<Vec<_>>(), vec![id(0)]);
    // Its off vocal comes later, with another song in two versions.
    radio.add_new([song(1, "Sora (Off Vocal)"), song(2, "Umi"), song(3, "Umi (TV Size)")]);
    let later: Vec<Item> = radio.take_new(&recs, 3, 8, 1).into_iter().map(|p| p.item).collect();
    assert!(!later.contains(&id(1)), "a version of one served: {later:?}");
    assert!(!(later.contains(&id(2)) && later.contains(&id(3))), "two versions at once: {later:?}");
    assert!(!later.is_empty());
    let mut fresh = RadioState::new(RadioSeed::Track(tid(0)));
    fresh.add_new([song(0, "Sora"), song(1, "Sora (Off Vocal)"), song(2, "Umi"), song(3, "Umi (TV Size)")]);
    let served: HashSet<Item> = [id(0)].into_iter().collect();
    assert_eq!(fresh.new_taken_by_work(&served), [id(1), id(3)].into_iter().collect::<HashSet<_>>());
}
