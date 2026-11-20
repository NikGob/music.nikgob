use super::*;

/// A synthetic dataset goes through serde and back unchanged, and its
/// parts rebuild the same catalog and history.
#[test]
fn a_dataset_round_trips() {
    let d = synthetic::user(synthetic::Kind::Cold, 1);
    let json = serde_json::to_string(&d).unwrap();
    let back: Dataset = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tracks, d.tracks);
    assert_eq!(back.events, d.events);
    let (a, b) = (d.parts(), back.parts());
    assert_eq!(a.catalog.len(), b.catalog.len());
    assert_eq!(a.index.len(), b.index.len());
    assert_eq!(a.input.events, b.input.events);
    let again = Dataset::of("again", &a.catalog, &a.index, &a.types, &a.input, Some(&a.graph));
    assert_eq!(again.tracks, d.tracks);
    assert_eq!(again.events, d.events);
}

/// The session before a moment is the one going on: its starts in
/// order, how each ended; nothing after a pause.
#[test]
fn the_session_before_a_moment() {
    let d = synthetic::user(synthetic::Kind::OneGenre, 2);
    let p = d.parts();
    let starts = Starts::of(&p.input.events);
    let k = (2..starts.list.len()).find(|&k| starts.list[k].3 == starts.list[k - 2].3).unwrap();
    let s = session_before(&p.input.events, starts.list[k].0);
    assert!(s.len() >= 2);
    assert_eq!(s.last().map(|x| x.track), Some(starts.list[k - 1].1));
    let end = p.input.events.iter().map(|e| e.ts).max().unwrap();
    assert!(session_before(&p.input.events, end + 2 * SESSION_GAP_MS).is_empty());
}

/// The evaluation of a small synthetic listener runs through and its
/// numbers are sane: the learnt model finds the next start far better
/// than chance, lists are made and measured, the picks of a listener
/// of one kind of music rate high by the hidden taste.
#[test]
fn the_evaluation_runs() {
    let d = synthetic::user(synthetic::Kind::OneGenre, 3);
    let p = d.parts();
    let r = evaluate(&d.name, &p, &Options { autoplay_points: 6, mix_days: 2, ..Options::default() });
    let next = r.next.as_ref().unwrap().get("listened", "learned");
    assert!(next.n > 100, "{next:?}");
    assert!(next.hit50 > 0.2, "{next:?}");
    assert!(r.autoplay.lists > 0 && r.mixes.lists > 0, "{}", r.table());
    assert!(r.mixes.truth.unwrap() > 0.6 && r.autoplay.truth.unwrap() > 0.6, "{}", r.table());
    eprintln!("{}", r.table());
}

/// A listener who brought a long history of an older taste from
/// another service and listens to other music here: the home's mixes
/// are not the import's old songs (a third of them at most over the
/// row; a mix keeps its quarter, a short one fills to its least length)
/// and they bring songs never heard.
#[test]
fn an_imported_history_does_not_take_the_mixes_over() {
    let d = synthetic::user(synthetic::Kind::Imported, 4);
    assert!(d.events.iter().filter(|e| e.origin == Some(EventOrigin::Import)).count() > 3_000, "a long imported history");
    let p = d.parts();
    let last = p.input.events.iter().filter(|e| e.origin != Some(EventOrigin::Import)).map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let home = home_mixes(&p, &model, &suits, 1);
    assert!(!home.is_empty());
    let recs = Recs::new(&p.catalog, &model.taste, &p.index).with_model(&model).with_background(&suits);
    let fit = |i: usize| suits.get(i).copied().unwrap_or(false);
    let b = Builder::new(&recs, &model, &|_| true, &fit, 1, DayPart::Day, Live::default(), MixSettings::default());
    let (mut old, mut never, mut all) = (0usize, 0usize, 0usize);
    for m in home.iter().filter(|m| m.key != MixKey::Liked) {
        old += m.tracks.iter().filter(|&&i| b.stale_at(i)).count();
        never += m.tracks.iter().filter(|&&i| never_heard(&model, i)).count();
        all += m.tracks.len();
    }
    assert!(old * 3 <= all, "{old} old songs of {all}");
    assert!(never > 0, "nothing never heard in {all} songs");
}

/// A listener of rap with three calm songs of one artist: «Спокойное»
/// is a niche, its wave plays mostly new calm songs of several artists
/// — none of the rap, not the rap its calm songs' radios hold, not the
/// songs of its own rapper — and the wave of any music stays the
/// user's share of new.
#[test]
fn a_calm_niche_of_three_songs_opens_to_the_world() {
    let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let pool = open_pool(&p, &model);
    let calm = wave(&p, &model, &suits, &pool, &WaveFilter { novelty: USER_NEW_SHARE, character: Character::Calm, ..WaveFilter::default() }, &Session::default(), WAVE_LEN);
    assert!(calm.niche, "fitting {} of {} artists", calm.fitting, calm.artists);
    let new: Vec<&crate::open_world::WavePick> = calm.picks.iter().filter(|p| p.new).collect();
    assert!(new.len() as f32 >= 0.7 * calm.picks.len() as f32, "{} new of {}", new.len(), calm.picks.len());
    let title = |p: &crate::open_world::WavePick| match &p.item {
        Item::Stream(r) => d.new.iter().find(|n| n.track.id == *r).map(|n| n.track.title.clone()).unwrap_or_default(),
        Item::Track(_) => String::new(),
    };
    assert!(new.iter().all(|p| title(p).starts_with("Drift")), "{:?}", new.iter().map(|p| title(p)).collect::<Vec<_>>());
    let artists: HashSet<&str> = calm.picks.iter().map(|p| p.artist.as_str()).collect();
    assert!(artists.len() >= 4, "{artists:?}");
    let any = wave(&p, &model, &suits, &pool, &WaveFilter::default(), &Session::default(), WAVE_LEN);
    assert!(!any.niche);
    let share = any.picks.iter().filter(|p| p.new).count() as f32 / any.picks.len() as f32;
    assert!(share <= USER_NEW_SHARE + 0.1, "{share}");
}

/// A session of «Моя волна» on a listener of several clusters: its new
/// music follows the novelty (none at «Знакомое», all it can at «Только
/// новое»), it weighs several clusters, and early skips of a fresh wave
/// as the session judges them move it to another, not before the second.
/// An open pool for a listener who has none: a new song of a new
/// artist beside each of `every`-th library tracks, linked to it.
fn linked_pool(model: &UserModel, every: usize) -> Vec<OpenCandidate> {
    use nikgob_core::streaming::{ArtistRef, RemoteId, Service};
    (0..model.len())
        .step_by(every)
        .enumerate()
        .map(|(k, i)| {
            let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("w{k}")), format!("Wave {k}"));
            track.artists.push(ArtistRef::new(None, format!("New {k}")));
            OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, embedding: None, lane: None, near: vec![i], weight: 0.8 }
        })
        .collect()
}

#[test]
fn a_wave_session_follows_novelty_and_skips() {
    let d = synthetic::user(synthetic::Kind::Omnivore, 5);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let pool = linked_pool(&model, 2);
    assert!(pool.len() > 40);
    let r = wave_session(&p, &model, &suits, &pool, &Session::default());
    let new = r.new.map(Option::unwrap);
    assert_eq!(new[0], 0.0, "«Знакомое» plays no new song: {new:?}");
    assert!(new[1] > 0.0 && new[1] < new[2] + 0.05 && new[2] < new[3] + 0.05, "{new:?}");
    assert!(new[3] >= 0.9, "«Только новое»: {new:?}");
    assert!(r.clusters >= 2, "{}", r.clusters);
    assert_eq!(r.moved, Some(true), "skips change the cluster: {r:?}");
    assert!(r.skips >= 2, "a skip stays: {r:?}");
    assert_eq!(r.verdicts.iter().sum::<usize>(), r.skips, "{r:?}");
}

/// A skip or two keep the wave's cluster past a block's end, the queue
/// replaced as the runtime does: the first block's last song skipped
/// while the next block's first is queued ahead of another cluster, that
/// song gives way, and what plays after one skip and after two is of
/// the skipped cluster; a third skip leaves it.
#[test]
fn skips_keep_the_cluster_past_a_block_boundary() {
    use crate::radio::Outcome;
    use crate::wave::WaveSeed;
    let d = synthetic::user(synthetic::Kind::Omnivore, 6);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let none = |_: &StreamTrack| Vec::new();
    let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &background, artist_tracks: &none, index: None, learned: None };
    let skip = |radio: &mut RadioState, item: &Item| radio.judged(item.clone(), Outcome::skip(8_000), Verdict::Rejected);
    let mut checked = 0;
    for seed in 0..16u64 {
        let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(crate::wave::NOVELTY_FAMILIAR)));
        radio.attune(Session::default(), Some(model.weights));
        // One at a time until the first block is picked whole.
        let mut block: Vec<Item> = Vec::new();
        while (block.is_empty() || radio.wave_bandit().is_some_and(|b| b.block_left() > 0)) && block.len() < 12 {
            let Some(pk) = radio.extend_wave(&ctx, &[], 1, seed).pop() else { break };
            block.push(pk.item);
        }
        let Some(a) = radio.wave_bandit().filter(|b| b.arms.len() >= 2 && block.len() >= 2).and_then(|b| b.scene_of(&block[0])) else { continue };
        // The next block's first song queued ahead.
        // The next block drawn of another cluster, as a draw may (the
        // prior leads the first cluster's way too far to wait for one):
        // its first song queued ahead while the block's last plays.
        if !radio.other_vibe(block.last()) {
            continue;
        }
        let Some(ahead) = radio.extend_wave(&ctx, &[], 1, seed).pop() else { continue };
        let scene = |radio: &RadioState, i: &Item| radio.wave_bandit().and_then(|b| b.scene_of(i));
        assert_ne!(scene(&radio, &ahead.item), Some(a), "seed {seed}: queued ahead of another cluster");
        checked += 1;
        let mut queue = vec![ahead.item];
        // The block's last song skipped as it plays: the song queued
        // ahead stays only when of its cluster, the tail again.
        skip(&mut radio, &block[block.len() - 1]);
        requeue(&mut radio, &ctx, &[], &mut queue, 3, seed);
        assert!(!queue.is_empty(), "seed {seed}");
        for item in &queue {
            assert_eq!(scene(&radio, item), Some(a), "seed {seed}: one skip, the skipped cluster still");
        }
        // The next skipped too: the whole tail, of the cluster still.
        let item = queue.remove(0);
        skip(&mut radio, &item);
        requeue(&mut radio, &ctx, &[], &mut queue, 3, seed);
        for item in &queue {
            assert_eq!(scene(&radio, item), Some(a), "seed {seed}: two skips, the skipped cluster still");
        }
        // A third leaves it: the next song is of another.
        let item = queue.remove(0);
        skip(&mut radio, &item);
        requeue(&mut radio, &ctx, &[], &mut queue, 3, seed);
        assert!(!queue.is_empty(), "seed {seed}");
        for item in &queue {
            assert_ne!(scene(&radio, item), Some(a), "seed {seed}: a third skip leaves");
        }
    }
    assert!(checked > 0, "a wave of two clusters or more was met");
}

/// A fresh wave of «Знакомое» with its first block picked whole, one at
/// a time, and the next block drawn of another cluster, its first song
/// queued ahead: the wave, the block, its cluster, the song ahead.
fn wave_at_a_boundary(ctx: &WaveContext<'_>, model: &UserModel, seed: u64) -> Option<(RadioState, Vec<Item>, crate::open_world::Scene, Item)> {
    let mut radio = RadioState::new(RadioSeed::Wave(crate::wave::WaveSeed::plain(crate::wave::NOVELTY_FAMILIAR)));
    radio.attune(Session::default(), Some(model.weights));
    let mut block: Vec<Item> = Vec::new();
    while (block.is_empty() || radio.wave_bandit().is_some_and(|b| b.block_left() > 0)) && block.len() < 12 {
        let Some(pk) = radio.extend_wave(ctx, &[], 1, seed).pop() else { break };
        block.push(pk.item);
    }
    let a = radio.wave_bandit().filter(|b| b.arms.len() >= 2 && block.len() >= 3).and_then(|b| b.scene_of(&block[0]))?;
    if !radio.other_vibe(block.last()) {
        return None;
    }
    let ahead = radio.extend_wave(ctx, &[], 1, seed).pop()?.item;
    Some((radio, block, a, ahead))
}

/// «Не сейчас» of a worn song in a settled vibe at its block's end, the
/// next block of another cluster queued ahead: the song ahead gives way
/// and the next to play is of the vibe still, after a skip before it
/// too, the queue replaced as the runtime does; nothing counts.
#[test]
fn a_worn_song_keeps_the_next_song_past_a_block_boundary() {
    use crate::radio::Outcome;
    let d = synthetic::user(synthetic::Kind::Omnivore, 6);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let none = |_: &StreamTrack| Vec::new();
    let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &background, artist_tracks: &none, index: None, learned: None };
    let scene = |radio: &RadioState, i: &Item| radio.wave_bandit().and_then(|b| b.scene_of(i));
    let mut checked = 0;
    for seed in 0..16u64 {
        for skip_first in [false, true] {
            let Some((mut radio, block, a, ahead)) = wave_at_a_boundary(&ctx, &model, seed) else { continue };
            assert_ne!(scene(&radio, &ahead), Some(a), "seed {seed}: queued ahead of another cluster");
            radio.judged(block[0].clone(), Outcome::Complete, Verdict::Approved);
            if skip_first {
                radio.judged(block[block.len() - 2].clone(), Outcome::skip(8_000), Verdict::Rejected);
            }
            let arms = radio.wave_bandit().map(|b| b.arms.clone());
            radio.judged(block[block.len() - 1].clone(), Outcome::skip(4_000), Verdict::NotNow);
            assert_eq!(radio.wave_bandit().map(|b| b.arms.clone()), arms, "seed {seed}: nothing counted");
            let mut queue = vec![ahead];
            requeue(&mut radio, &ctx, &[], &mut queue, 3, seed);
            assert_eq!(queue.first().and_then(|i| scene(&radio, i)), Some(a), "seed {seed}, a skip first {skip_first}: the next is of the vibe still");
            checked += 1;
        }
    }
    assert!(checked > 0, "a wave of two clusters or more was met");
}

/// Blocks of one cluster: a wave's picks come four to six of one
/// cluster at a time, and each library pick is of its cluster and of
/// the filter's character.
#[test]
fn a_wave_plays_blocks_of_its_filter() {
    use crate::radio::Outcome;
    use crate::wave::{BLOCK_MIN, WaveSeed};
    let d = synthetic::user(synthetic::Kind::Omnivore, 6);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let none = |_: &StreamTrack| Vec::new();
    let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &background, artist_tracks: &none, index: None, learned: None };
    for character in [Character::Any, Character::Calm] {
        let seed = WaveSeed { filter: WaveFilter { novelty: 0.0, character, scene: crate::open_world::Scene::All }, only: Vec::new(), part: None };
        let mut radio = RadioState::new(RadioSeed::Wave(seed.clone()));
        radio.attune(Session::default(), Some(model.weights));
        let mut runs: Vec<(Option<crate::open_world::Scene>, usize)> = Vec::new();
        let (mut fits, mut played) = (0usize, 0usize);
        let mut heard: Vec<TrackId> = Vec::new();
        let mut first_again: Option<usize> = None;
        for _ in 0..8 {
            let picks = radio.extend_wave(&ctx, &[], 3, 5);
            let scene = radio.wave_bandit().and_then(|b| b.current_scene());
            for pk in picks {
                let Item::Track(t) = pk.item else { panic!("«Знакомое» is the library") };
                let i = model.position(t).unwrap();
                if first_again.is_none() && heard.contains(&t) {
                    first_again = Some(heard.len());
                }
                heard.push(t);
                played += 1;
                let fit = crate::open_world::fits_filter(&ctx, &seed.filter, i);
                fits += usize::from(fit);
                if let (true, Some(crate::open_world::Scene::Profile(k))) = (fit, scene) {
                    assert_eq!(model.profile_at(i).map(|j| model.profiles.list[j].key), Some(k), "a pick of its block's cluster");
                }
                match runs.last_mut() {
                    Some((s, n)) if *s == scene => *n += 1,
                    _ => runs.push((scene, 1)),
                }
                radio.feedback(pk.item, Outcome::Complete);
            }
        }
        // Every pick is of the filter (its character never lets go), and
        // every rested track of it played before one plays again.
        assert_eq!(fits, played, "{character:?}: {fits} of {played} fit");
        assert!(played > 0);
        let rested = (0..model.len())
            .filter(|&i| crate::open_world::fits_filter(&ctx, &seed.filter, i))
            .filter(|&i| model.taste.last_played_at(i).is_none_or(|t| model.now() - t >= 3 * crate::taste::HOUR_MS))
            .count();
        if let Some(at) = first_again {
            assert_eq!(at, rested, "{character:?}: played again after {at} of {rested}");
        }
        // Every run of one cluster but the last is a whole block.
        // (A calm cluster of a few songs may run dry within its block.)
        if character == Character::Any && radio.wave_bandit().is_some_and(|b| b.arms.len() > 1) {
            for (_, n) in &runs[..runs.len() - 1] {
                assert!(*n >= usize::from(BLOCK_MIN), "{runs:?}");
            }
        }
    }
}

/// A filter run dry plays its songs again, those heard least lately
/// first, before anything else; its character never lets go; its scene
/// lets go only when even that is nothing, and the wave says so.
#[test]
fn a_dry_wave_plays_its_own_again_before_letting_its_scene_go() {
    use crate::radio::Outcome;
    use crate::wave::{REPEAT_GAP, WaveSeed};
    let d = synthetic::user(synthetic::Kind::Omnivore, 6);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let none = |_: &StreamTrack| Vec::new();
    let rested = |i: usize| model.taste.last_played_at(i).is_none_or(|t| model.now() - t >= 3 * crate::taste::HOUR_MS);
    let all = |_: usize| true;
    let full = WaveContext { model: &model, ok: &all, genre: None, background: &background, artist_tracks: &none, index: None, learned: None };
    let calm = WaveFilter { novelty: 0.0, character: Character::Calm, scene: crate::open_world::Scene::All };
    // Eight calm songs only may play.
    let eight: Vec<usize> = (0..model.len()).filter(|&i| rested(i) && crate::open_world::fits_filter(&full, &calm, i)).take(8).collect();
    assert_eq!(eight.len(), 8, "a calm side to the omnivore");
    let only_eight = |i: usize| eight.contains(&i) || !crate::open_world::fits_filter(&full, &calm, i);
    let ctx = WaveContext { model: &model, ok: &only_eight, genre: None, background: &background, artist_tracks: &none, index: None, learned: None };
    let seed = WaveSeed { filter: calm, only: Vec::new(), part: None };
    let mut radio = RadioState::new(RadioSeed::Wave(seed.clone()));
    radio.attune(Session::default(), Some(model.weights));
    let mut heard: Vec<usize> = Vec::new();
    for _ in 0..8 {
        // (The call that runs dry may come short: its own picks are
        // in the queue's gap.)
        let picks = radio.extend_wave(&ctx, &[], 3, 5);
        assert!(!picks.is_empty(), "never silent while its own can play again");
        for pk in picks {
            let Item::Track(t) = pk.item else { panic!() };
            let i = model.position(t).unwrap();
            assert!(eight.contains(&i), "only calm songs: {i}");
            heard.push(i);
            radio.feedback(pk.item, Outcome::Complete);
        }
    }
    assert!(heard.len() >= 16, "{heard:?}");
    let distinct: HashSet<usize> = heard[..8].iter().copied().collect();
    assert_eq!(distinct.len(), 8, "all eight before one again: {heard:?}");
    // The first played again is of those heard least lately, and none
    // within the queue's gap.
    assert!(heard[..8 - REPEAT_GAP].contains(&heard[8]), "{heard:?}");
    for (k, i) in heard.iter().enumerate() {
        assert!(!heard[k.saturating_sub(REPEAT_GAP)..k].contains(i), "{i} again within {REPEAT_GAP}: {heard:?}");
    }
    assert_eq!(radio.wave_seed().map(|s| s.filter), Some(calm));

    // A scene of two calm songs: they, then every scene, and the
    // wave's scene says «Все»; the character still holds.
    let k = model.profiles.list.iter().map(|p| p.key).find(|&k| {
        let scene = WaveFilter { scene: crate::open_world::Scene::Profile(k), ..calm };
        (0..model.len()).filter(|&i| rested(i) && crate::open_world::fits_filter(&full, &scene, i)).count() >= 2
    });
    let Some(k) = k else { panic!("a profile with calm songs") };
    let scene = WaveFilter { scene: crate::open_world::Scene::Profile(k), ..calm };
    let two: Vec<usize> = (0..model.len()).filter(|&i| rested(i) && crate::open_world::fits_filter(&full, &scene, i)).take(2).collect();
    let only_two = |i: usize| two.contains(&i) || !crate::open_world::fits_filter(&full, &scene, i);
    let ctx = WaveContext { model: &model, ok: &only_two, genre: None, background: &background, artist_tracks: &none, index: None, learned: None };
    let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed { filter: scene, only: Vec::new(), part: None }));
    radio.attune(Session::default(), Some(model.weights));
    let picks = radio.extend_wave(&ctx, &[], 3, 5);
    assert_eq!(picks.len(), 3);
    let at: Vec<usize> = picks.iter().map(|pk| match pk.item { Item::Track(t) => model.position(t).unwrap(), _ => panic!() }).collect();
    assert!(two.contains(&at[0]) && two.contains(&at[1]), "its scene first: {at:?}");
    assert!(!two.contains(&at[2]));
    assert!(at.iter().all(|&i| crate::open_world::fits_filter(&full, &calm, i)), "calm throughout");
    assert_eq!(radio.wave_seed().map(|s| s.filter), Some(calm), "the chip shows «Все»");
}

/// The calm niche of three songs is a card of the home (below a mix's
/// least length), in place of the calm side of everything: its three
/// songs and mostly new calm songs of several artists; its pool is its
/// three songs (the top-up's seeds).
#[test]
fn a_calm_niche_of_three_songs_is_a_card_of_the_home() {
    let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let (home, _) = home_with_niches(&p, &model, &suits, 1);
    let calm = home.iter().find(|m| m.key == MixKey::Calm).expect("a calm card");
    assert_eq!(calm.tracks.len(), 3, "its own songs only");
    assert_eq!(calm.pool, calm.tracks);
    let at = home.iter().position(|m| m.key == MixKey::Calm).unwrap();
    let new = home_new(&p, &model, &home);
    assert!(new[at].niche);
    let share = new[at].placed as f32 / (new[at].placed + 3) as f32;
    assert!(share >= 0.7, "{share}");
    assert!(new[at].artists >= 4, "{:?}", new[at]);
    // A card drawn short from a large pool is no niche: the pool tells.
    let mut big = home.iter().find(|m| m.key != MixKey::Calm && m.pool_size.0 >= 25 && m.pool_size.1 >= 4).expect("a large pool").clone();
    big.tracks.truncate(12);
    big.artists.truncate(2);
    assert!(!home_new(&p, &model, &[big])[0].niche);
}

/// The ends of novelty hold: «Знакомое» plays no new song even when
/// the library has few, «Только новое» no library song even when the
/// pool runs short — fewer songs then.
#[test]
fn the_ends_of_novelty_hold() {
    let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let pool = open_pool(&p, &model);
    let calm = |novelty: f32| WaveFilter { novelty, character: Character::Calm, ..WaveFilter::default() };
    let familiar = wave(&p, &model, &suits, &pool, &calm(0.0), &Session::default(), WAVE_LEN);
    assert!(!familiar.picks.is_empty() && familiar.picks.iter().all(|x| !x.new), "{:?}", familiar.picks);
    assert!(familiar.picks.len() < WAVE_LEN, "three calm songs only");
    let only = wave(&p, &model, &suits, &pool[..3], &calm(1.0), &Session::default(), WAVE_LEN);
    assert!(only.picks.iter().all(|x| x.new), "{:?}", only.picks);
    assert!(only.picks.len() <= 3);
}

/// A song without a preview a calm song's radio holds is no calm song
/// on that word alone (a new rapper's loud song); the lists of two of
/// the niche's songs, or its artist's own calm songs, are.
#[test]
fn one_radio_is_no_proof_of_a_calm_sound() {
    let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let calm: Vec<usize> = d.new.iter().filter(|n| n.track.title.starts_with("Drift")).flat_map(|n| n.near.iter().filter_map(|t| model.position(*t))).collect::<HashSet<_>>().into_iter().collect();
    assert_eq!(calm.len(), 3);
    let strict = crate::open_world::NicheSound::of(&model, &calm, true);
    assert!(!strict.fits(&model, None, None, &[calm[0]], &[]), "one radio, a new artist");
    assert!(strict.fits(&model, None, None, &[calm[0], calm[1]], &[]), "two of the niche's lists");
    let calm_energy: Vec<f32> = calm.iter().filter_map(|&i| model.energy_at(i)).collect();
    assert!(strict.fits(&model, None, None, &[calm[0]], &calm_energy), "its artist's calm songs");
    let loose = crate::open_world::NicheSound::of(&model, &calm, false);
    assert!(loose.fits(&model, None, None, &[calm[0]], &[]), "a niche of a kind takes the link");
}

/// A scene of a family of genres works through the wave: library songs
/// of the family, new songs linked to them or by its artists.
#[test]
fn a_wave_of_a_genre() {
    let mut d = synthetic::user(synthetic::Kind::NicheCalm, 5);
    // The rap's artists tagged hip hop.
    let rap_artists: HashSet<ArtistId> = d.tracks.iter().filter(|t| d.types.by_track.get(&t.id).is_some_and(|s| s.lang == Some(crate::songtype::Lang::Russian))).flat_map(|t| t.artists.clone()).collect();
    for a in &rap_artists {
        d.tags.push((*a, "hip hop".into(), 10.0));
    }
    let p = d.parts();
    let family = genres(&p).into_iter().flatten().next().expect("a family");
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let pool = open_pool(&p, &model);
    let w = wave(&p, &model, &suits, &pool, &WaveFilter { novelty: 0.5, scene: crate::open_world::Scene::Genre(family), ..WaveFilter::default() }, &Session::default(), WAVE_LEN);
    assert!(w.fitting > 50, "{}", w.fitting);
    assert!(w.picks.iter().any(|x| x.new) && w.picks.iter().any(|x| !x.new));
    let title = |r: &nikgob_core::streaming::RemoteId| d.new.iter().find(|n| n.track.id == *r).map(|n| n.track.title.clone()).unwrap_or_default();
    for x in &w.picks {
        match &x.item {
            Item::Track(t) => assert!(model.position(*t).and_then(|i| model.main_artist_at(i)).is_some_and(|a| rap_artists.contains(&a))),
            Item::Stream(r) => assert!(!title(r).starts_with("Drift"), "{}", title(r)),
        }
    }
}

/// How the wave ranks the library in a report (`NIKGOB_WAVE_MODEL`:
/// `on`, `off`, `both`; unset, as the constant has it): each way with
/// its name.
fn ranking_modes() -> Vec<(Option<bool>, &'static str)> {
    match std::env::var("NIKGOB_WAVE_MODEL").as_deref() {
        Ok("on") => vec![(Some(true), " [wave by the model]")],
        Ok("off") => vec![(Some(false), " [wave by its pull]")],
        Ok("both") => vec![(Some(false), " [wave by its pull]"), (Some(true), " [wave by the model]")],
        _ => vec![(None, "")],
    }
}

/// The parts of the wave turned off for a report
/// (`NIKGOB_DISCOVER_OFF=works,rest,probe,sequence,rejected,arrivals`):
/// each step measured on its own.
fn parts_off() {
    use crate::open_world::Part;
    let off: Vec<Part> = std::env::var("NIKGOB_DISCOVER_OFF")
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| match p.trim() {
            "works" => Some(Part::Works),
            "rest" => Some(Part::Rest),
            "probe" => Some(Part::Probe),
            "sequence" => Some(Part::Sequence),
            "rejected" => Some(Part::Rejected),
            "arrivals" => Some(Part::Arrivals),
            _ => None,
        })
        .collect();
    if !off.is_empty() {
        println!("parts off: {off:?}");
    }
    crate::open_world::set_off(&off);
}

/// Every synthetic listener, measured (`cargo test -p nikgob-recs
/// --release eval::tests::synthetic_report -- --ignored --nocapture`).
#[test]
#[ignore = "a report, minutes"]
fn synthetic_report() {
    // `NIKGOB_EVAL_QUICK`: the lists only (no replay, no big listener);
    // `NIKGOB_EVAL_NEXT`: the replay only.
    let quick = std::env::var_os("NIKGOB_EVAL_QUICK").is_some();
    let next_only = std::env::var_os("NIKGOB_EVAL_NEXT").is_some();
    parts_off();
    for kind in synthetic::Kind::ALL {
        if (quick || next_only) && kind == synthetic::Kind::Big {
            continue;
        }
        let d = synthetic::user(kind, 7);
        let p = d.parts();
        for (k, (mode, label)) in ranking_modes().into_iter().enumerate() {
            crate::open_world::set_model_ranking(mode);
            let opts = if kind == synthetic::Kind::Big {
                Options { next: false, autoplay_points: 0, mix_days: 0, ..Options::default() }
            } else {
                Options { next: !quick && k == 0, autoplay_points: if next_only { 0 } else { 40 }, mix_days: if next_only { 0 } else { 8 }, ..Options::default() }
            };
            println!("{}", evaluate(&format!("{}{label}", d.name), &p, &opts).table());
        }
    }
    crate::open_world::set_model_ranking(None);
}

/// A real library's dataset (`NIKGOB_RECS_DATASET=path.json`, written
/// by the runtime from a copy), measured.
#[test]
#[ignore = "needs NIKGOB_RECS_DATASET"]
fn dataset_report() {
    let Some(path) = std::env::var_os("NIKGOB_RECS_DATASET") else {
        eprintln!("NIKGOB_RECS_DATASET is not set");
        return;
    };
    let d: Dataset = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let p = d.parts();
    parts_off();
    for (k, (mode, label)) in ranking_modes().into_iter().enumerate() {
        crate::open_world::set_model_ranking(mode);
        // `NIKGOB_WAVE_TRACE`: sessions of the wave picked one by one.
        if std::env::var_os("NIKGOB_WAVE_TRACE").is_some() {
            for novelty in [crate::wave::NOVELTY_FAMILIAR, crate::wave::NOVELTY_BALANCE, crate::wave::NOVELTY_MORE] {
                println!("{label}\n{}", wave_trace(&p, novelty, 60));
            }
        }
        println!("{}", evaluate(&format!("{}{label}", d.name), &p, &Options { next: k == 0, ..Options::default() }).table());
    }
    crate::open_world::set_model_ranking(None);
}

/// The wave replayed against the history: its picks go by as asked,
/// a song the history skips is skipped (and the bandit learns it), the
/// trace adds them up; at «Знакомое» it plays no new song.
#[test]
fn a_wave_replays_against_the_history() {
    let d = synthetic::user(synthetic::Kind::Skipper, 6);
    let p = d.parts();
    let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
    let model = p.model_at(last + 1);
    let suits = p.suits();
    let pool = linked_pool(&model, 3);
    let skipped = |i: usize| i % 2 == 0;
    let steps = wave_replay(&p, &model, &suits, &pool, &Session::default(), crate::wave::NOVELTY_BALANCE, 30, &skipped);
    assert_eq!(steps.len(), 30);
    let (all, new, skip) = wave_counts(&steps);
    assert!(new > 0 && new < all, "{new} new of {all}");
    assert_eq!(skip, steps.iter().filter(|s| s.track.is_some_and(|i| i % 2 == 0)).count());
    let familiar = wave_replay(&p, &model, &suits, &pool, &Session::default(), crate::wave::NOVELTY_FAMILIAR, 12, &skipped);
    assert!(familiar.iter().all(|s| s.track.is_some()));
    let r = evaluate(&d.name, &p, &Options { next: false, autoplay_points: 4, mix_days: 0, ..Options::default() });
    assert!(r.wave_familiar.lists > 0, "{}", r.table());
    assert_eq!(r.wave_familiar_mix.0, 0.0, "{}", r.table());
    assert!(wave_trace(&p, crate::wave::NOVELTY_BALANCE, 20).contains("= "));
    assert!(steps.iter().filter(|s| s.track.is_none()).all(|s| !s.skipped), "a new song's end is unknown");
    // One identity of an artist for library and new songs; nobody's
    // song is an artist of its own.
    for s in &steps {
        match s.track {
            Some(i) => assert_eq!(s.artist, ArtistKey::of_track(&model, i).label(|| format!("track:{i}"))),
            None => assert!(s.artist.starts_with("name:") || s.artist.starts_with("new:") || !s.artist.is_empty(), "{}", s.artist),
        }
    }
}

/// The kept taste of a wave: what is listened to counts, an artist
/// again less, a skipped pick nothing, a pick near the skipped sound
/// costs, a new song counts its fixed share.
#[test]
fn a_waves_kept_taste() {
    use nikgob_core::streaming::{RemoteId, Service};
    let lib = |i: usize, artist: &str, skipped: bool| WaveStep { item: Item::Track(crate::test_util::tid(i as u128 + 1)), track: Some(i), artist: artist.to_owned(), skipped };
    let new = WaveStep { item: Item::Stream(RemoteId::new(Service::Youtube, "n")), track: None, artist: "n".to_owned(), skipped: false };
    let steps = vec![lib(0, "a", false), lib(1, "a", false), lib(2, "b", true), lib(3, "c", false), new];
    let far = |x: f32| Some([x; DIM]);
    let sounds = vec![far(10.0), far(20.0), far(0.0), far(0.0), None];
    let relevance = |i: usize| if i < 2 { 1.0 } else { 0.5 };
    let q = kept_taste(&steps, &sounds, 1.0, &relevance);
    let want = (1.0 + Q_ARTIST + 0.0 + (0.5 - Q_BACK) + Q_NEW) / 5.0;
    assert!((q - want).abs() < 1e-5, "{q} {want}");
    assert_eq!(kept_taste(&[], &[], 1.0, &relevance), 0.0);
}

/// What steers a replayed wave at a moment is only what ended by it: an
/// early skip counts from the moment it ended, not from its start — a
/// play started before the moment and skipped after it is unknown yet.
#[test]
fn the_wave_replay_knows_no_future_skip() {
    let d = synthetic::user(synthetic::Kind::Skipper, 6);
    let p = d.parts();
    let starts = Starts::of(&p.input.events);
    let k = (0..starts.list.len())
        .find(|&k| starts.list[k].2 == Ended::Early && starts.ends[k] > starts.list[k].0 + 1 && p.catalog.position(starts.list[k].1).is_some())
        .expect("an early skip that ends after its start");
    let (start, track, _, _) = starts.list[k];
    let end = starts.ends[k];
    let i = p.catalog.position(track).unwrap();
    let before = skips_before(&starts, &p, start);
    // Started, not ended yet: its skip is not known.
    assert_eq!(skips_before(&starts, &p, start + 1)[i], before[i]);
    assert_eq!(skips_before(&starts, &p, end - 1)[i], before[i]);
    let at = skips_before(&starts, &p, end);
    assert_eq!(at[i], (before[i].0 + 1, before[i].1 + 1), "known once it ended");
    assert_eq!(skips_before(&starts, &p, starts.list[0].0).iter().map(|x| x.0).sum::<u32>(), 0);
}
