    use nikgob_core::model::{EventOrigin, EventType, ReleaseId, UnixMs};

    use super::*;
    use super::naming::{blend_short_in, blend_title_in, side_title_in};
    use std::collections::HashSet;
    use crate::pick::Reason;
    use crate::rng::Rng;
    use crate::VectorIndex;
    use crate::analysis::DIM;
    use crate::model::ModelInput;
    use crate::songtype::{SongType, SongTypes, Voice};
    use crate::taste::{Catalog, DAY_MS, HOUR_MS, Play, TasteInput, TrackInfo};
    use crate::test_util::*;

    const T0: UnixMs = 20_000 * DAY_MS;

    /// A listener of Russian and Japanese songs and of soundtracks without
    /// words, 110 tracks of each kind in eight worlds, a favourite 60 % of
    /// each kind played over `days` days (the rest never), each track's
    /// sound its kind's place plus an energy of its own; `extra` plays more.
    struct Lib {
        catalog: Catalog,
        index: VectorIndex<TrackId>,
        types: SongTypes,
        input: TasteInput,
    }

    fn lib(days: i64, extra: &[(UnixMs, usize)]) -> Lib {
        let kinds = [(Some(Lang::Russian), 0.5f32), (Some(Lang::Japanese), 0.3), (None, 0.2)];
        let per = 110usize;
        let mut rng = Rng::new(9);
        let (mut tracks, mut types) = (Vec::new(), SongTypes::default());
        let mut index = VectorIndex::new();
        for (k, (lang, _)) in kinds.iter().enumerate() {
            for t in 0..per {
                let i = (k * per + t) as u128;
                let world = format!("kind {k} world {}", t % 8);
                let artist = (k * 100 + t % 14) as u32;
                let release = ReleaseId(uuid::Uuid::from_u128(1_000 + (k * per + t / 9) as u128));
                let ms = 150_000 + (t as u32 % 7) * 20_000;
                tracks.push(TrackInfo::new(tid(i), vec![aid(artist)], Some(release)).with_world(&world).with_duration(Some(ms)));
                let mut v = [0.0f32; DIM];
                for (d, x) in v.iter_mut().enumerate() {
                    *x = rng.f32() * 0.4 + if d % 3 == k { 2.0 } else { 0.0 };
                }
                // An energy of its own on the axes the moods read.
                let e = rng.f32() * 3.0;
                for d in crate::mood::ENERGY_FEATURES {
                    v[d] += e;
                }
                index.insert(tid(i), v);
                let ty = match lang {
                    Some(l) => SongType { voice: Voice::Sung, lang: Some(*l), ..SongType::default() },
                    None => SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() },
                };
                types.by_track.insert(tid(i), ty);
            }
        }
        let mut events = Vec::new();
        let mut ts = T0 - days * DAY_MS;
        let fav = per * 3 / 5;
        for _ in 0..days {
            for s in 0..2 {
                let mut x = rng.f32();
                let k = kinds
                    .iter()
                    .position(|(_, w)| {
                        x -= w;
                        x <= 0.0
                    })
                    .unwrap_or(0);
                let mut at = ts + (9 + s * 8) * HOUR_MS;
                for _ in 0..10 {
                    let i = k * per + rng.below(fav);
                    events.push(Play { origin: Some(EventOrigin::Manual), ..Play::new(tid(i as u128), at, EventType::Start, Some(0)) });
                    at += 180_000;
                    events.push(Play::new(tid(i as u128), at, EventType::Complete, Some(180_000)));
                }
            }
            ts += DAY_MS;
        }
        for &(at, i) in extra {
            events.push(Play { origin: Some(EventOrigin::Manual), ..Play::new(tid(i as u128), at, EventType::Start, Some(0)) });
            events.push(Play::new(tid(i as u128), at + 180_000, EventType::Complete, Some(180_000)));
        }
        events.sort_by_key(|p| p.ts);
        Lib { catalog: Catalog::new(tracks), index, types, input: TasteInput { events, ..TasteInput::default() } }
    }

    fn model(l: &Lib, now: UnixMs) -> UserModel {
        UserModel::build(&l.catalog, &l.input, ModelInput { index: &l.index, types: Some(&l.types), weights: None, graph: None }, now)
    }

    fn with<T>(l: &Lib, m: &UserModel, part: DayPart, f: impl FnOnce(&Builder<'_>) -> T) -> T {
        in_epoch(l, m, part, m.now().div_euclid(DAY_MS) as u64, f)
    }

    fn in_epoch<T>(l: &Lib, m: &UserModel, part: DayPart, epoch: u64, f: impl FnOnce(&Builder<'_>) -> T) -> T {
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
        let no = |_: usize| false;
        let b = Builder::new(&recs, m, &|_| true, &no, epoch, part, Live::default(), Settings::default());
        f(&b)
    }

    fn energy(m: &UserModel, tracks: &[usize]) -> Vec<f32> {
        let raw: Vec<f32> = tracks.iter().map(|&i| m.energy_at(i).unwrap()).collect();
        crate::listening::pct_rank(&raw)
    }

    fn mean(x: &[f32]) -> f32 {
        x.iter().sum::<f32>() / x.len() as f32
    }

    fn profile(m: &UserModel, lane: LaneKey) -> u32 {
        m.profiles.list.iter().find(|p| p.lane == lane).unwrap().key
    }

    /// At night a mix settles: its first third is livelier than its last;
    /// by day it rises to its second half. The songs are the same.
    #[test]
    fn arc_settles_at_night() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let ru = MixKey::Profile(profile(&m, LaneKey::Sung(Lang::Russian)));
        let night = with(&l, &m, DayPart::Night, |b| b.mix(ru, &Kept::default())).unwrap();
        let e = energy(&m, &night.tracks);
        let third = e.len() / 3;
        assert!(mean(&e[..third]) > mean(&e[e.len() - third..]) + 0.25, "{e:?}");
        let day = with(&l, &m, DayPart::Day, |b| b.mix(ru, &Kept::default())).unwrap();
        let e = energy(&m, &day.tracks);
        assert!(mean(&e[e.len() / 2..e.len() * 4 / 5]) > mean(&e[..third / 2 + 1]) + 0.15, "{e:?}");
        let set = |t: &[usize]| t.iter().copied().collect::<HashSet<usize>>();
        assert_eq!(set(&night.tracks), set(&day.tracks), "the same songs, another order");
    }

    /// A song never played stands only between two loved familiar ones,
    /// never in the first two places, never two running.
    #[test]
    fn new_is_sandwiched() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let (mixes, _) = with(&l, &m, DayPart::Day, |b| b.lineup(&Kept::default()));
        let mut seen = 0;
        for mix in &mixes {
            for (k, &i) in mix.tracks.iter().enumerate() {
                if m.tracks[i].starts > 0 {
                    continue;
                }
                seen += 1;
                assert!(k >= 2 && k + 1 < mix.tracks.len(), "{}: unheard at {k}", mix.title);
                assert!(mix.anchors[k - 1] && mix.anchors[k + 1], "{}: unheard at {k} not between loved familiar songs", mix.title);
            }
            let unheard = mix.tracks.iter().filter(|&&i| m.tracks[i].starts == 0).count();
            assert!(unheard as f32 <= (UNHEARD_SHARE * mix.tracks.len() as f32).ceil() + 1.0, "{}: {unheard}", mix.title);
        }
        assert!(seen > 0, "the mixes bring something never played");
    }

    /// «Преимущественно новые песни»: the mixes take more songs never
    /// played, still each between two loved familiar ones.
    #[test]
    fn more_new_songs_when_asked() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let count = |settings: Settings| {
            let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(&m);
            let no = |_: usize| false;
            let b = Builder::new(&recs, &m, &|_| true, &no, m.now().div_euclid(DAY_MS) as u64, DayPart::Day, Live::default(), settings);
            let (mixes, _) = b.lineup(&Kept::default());
            let (mut unheard, mut all) = (0usize, 0usize);
            for mix in &mixes {
                for (k, &i) in mix.tracks.iter().enumerate() {
                    all += 1;
                    if m.tracks[i].starts > 0 {
                        continue;
                    }
                    unheard += 1;
                    assert!(k >= 2 && k + 1 < mix.tracks.len(), "{}: unheard at {k}", mix.title);
                    assert!(mix.anchors[k - 1] && mix.anchors[k + 1], "{}: unheard at {k} not between loved ones", mix.title);
                }
            }
            (unheard, all)
        };
        let (usual, _) = count(Settings::default());
        let (more, all) = count(Settings { unheard: UNHEARD_MOST, ..Settings::default() });
        assert!(more > usual, "{more} never played asked for, {usual} as usual");
        assert!(more as f32 >= UNHEARD_SHARE * all as f32, "{more} of {all}");
    }

    /// Files come lately and never played are the first of the never
    /// played a mix takes: of the songs never played the mixes bring, more
    /// are new arrivals than without their time known.
    #[test]
    fn new_arrivals_lead_the_never_played() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        // Every other song never played came in the last days.
        let unplayed: Vec<TrackId> = (0..m.len()).filter(|&i| m.tracks[i].starts == 0).map(|i| m.id(i)).collect();
        let arrived: HashSet<TrackId> = unplayed.iter().step_by(2).copied().collect();
        let tracks: Vec<TrackInfo> =
            l.catalog.tracks().iter().enumerate().map(|(k, t)| if arrived.contains(&t.id) { t.clone().with_added(T0 - HOUR_MS - (k % 10) as i64 * DAY_MS) } else { t.clone() }).collect();
        let fresh = Lib { catalog: Catalog::new(tracks), index: l.index.clone(), types: l.types.clone(), input: l.input.clone() };
        let fm = model(&fresh, T0);
        assert!(arrived.iter().filter_map(|id| fm.position(*id)).any(|i| fm.arrival_at(i) > 0.0));
        let share = |l: &Lib, m: &UserModel| {
            let (mixes, _) = with(l, m, DayPart::Day, |b| b.lineup(&Kept::default()));
            let picked: Vec<TrackId> = mixes.iter().flat_map(|x| x.tracks.iter().map(|&i| m.id(i))).filter(|id| unplayed.contains(id)).collect();
            (picked.iter().filter(|id| arrived.contains(id)).count(), picked.len())
        };
        let (before, of_before) = share(&l, &m);
        let (after, of_after) = share(&fresh, &fm);
        assert!(of_after > 0 && after * of_before > before * of_after, "{after} of {of_after} vs {before} of {of_before}");
    }

    /// Later in the epoch, after more plays, the lineup, its order and the
    /// songs of each mix in theirs are the same.
    #[test]
    fn stable_within_epoch() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let (first, lineup) = in_epoch(&l, &m, DayPart::Morning, 5, |b| b.lineup(&Kept::default()));
        let mut kept = Kept { lineup, ..Kept::default() };
        kept.keep(&first, &m);
        // Ten more songs, another session.
        let plays: Vec<(UnixMs, usize)> = (0..10).map(|k| (T0 + 9 * HOUR_MS + k * 200_000, 3 * k as usize)).collect();
        let l2 = lib(40, &plays);
        let m2 = model(&l2, T0 + 10 * HOUR_MS);
        let (again, keys) = in_epoch(&l2, &m2, DayPart::Morning, 5, |b| b.lineup(&kept));
        assert_eq!(keys, kept.lineup);
        let order = |x: &[Mix]| x.iter().map(|m| m.key).collect::<Vec<_>>();
        assert_eq!(order(&again), order(&first), "the cards in their order");
        for (a, b) in first.iter().zip(&again) {
            assert_eq!(a.tracks, b.tracks, "{}", a.title);
        }
    }

    /// The next epoch (a start of the app, another part of the day) draws
    /// another lineup and other songs: a mix that stays shares a third or
    /// so of its songs and shows other covers first.
    #[test]
    fn the_next_epoch_rotates() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let (now, lineup) = in_epoch(&l, &m, DayPart::Morning, 11, |b| b.lineup(&Kept::default()));
        let mut kept = Kept { lineup: lineup.clone(), ..Kept::default() };
        kept.keep(&now, &m);
        let next = kept.clone().next_epoch(true);
        assert_eq!(next.shown, std::slice::from_ref(&lineup));
        let (then, keys) = in_epoch(&l, &m, DayPart::Morning, 12, |b| b.lineup(&next));
        assert_ne!(keys, lineup, "another lineup");
        let mut compared = 0;
        for a in &now {
            let b = in_epoch(&l, &m, DayPart::Morning, 12, |b| b.mix(a.key, &next)).unwrap();
            let set: HashSet<usize> = a.tracks.iter().copied().collect();
            let shared = b.tracks.iter().filter(|i| set.contains(i)).count() as f32 / a.tracks.len().max(b.tracks.len()) as f32;
            assert!((0.05..=0.55).contains(&shared), "{}: {shared}", a.title);
            let firsts = |t: &[usize]| t.iter().take(6).copied().collect::<HashSet<usize>>();
            assert!(firsts(&a.tracks) != firsts(&b.tracks), "{}", a.title);
            compared += 1;
        }
        assert!(compared >= 3, "{compared}");
        let _ = then;
    }

    /// «Другой состав» draws another mix: much of it new, the same kind.
    #[test]
    fn another_draw() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let ru = MixKey::Profile(profile(&m, LaneKey::Sung(Lang::Russian)));
        let a = with(&l, &m, DayPart::Day, |b| b.mix(ru, &Kept::default())).unwrap();
        let mut kept = Kept::default();
        kept.keep(std::slice::from_ref(&a), &m);
        kept.salts.push((ru, 7));
        let b = with(&l, &m, DayPart::Day, |b| b.mix(ru, &kept)).unwrap();
        let set: HashSet<usize> = a.tracks.iter().copied().collect();
        let shared = b.tracks.iter().filter(|i| set.contains(i)).count() as f32 / b.tracks.len() as f32;
        assert!(shared < 0.7, "{shared}");
        assert!(b.tracks.iter().all(|&i| m.profile_at(i) == a.tracks.first().and_then(|&f| m.profile_at(f))));
    }

    /// Five moments of three profiles, drawn from «Всё вперемешку», the
    /// profiles and both sides; no two alike, none of one world, every rule
    /// kept, nothing too long or too short.
    #[test]
    fn the_lineup_of_three_profiles() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let (mixes, keys) = with(&l, &m, DayPart::Day, |b| b.lineup(&Kept::default()));
        let titles: Vec<&str> = mixes.iter().map(|x| x.title.as_str()).collect();
        let shares: Vec<(LaneKey, f32, bool)> = m.profiles.list.iter().zip(&m.profile_share).map(|(p, s)| (p.lane, *s, p.visible)).collect();
        assert_eq!(mixes.len(), MOMENTS, "{titles:?} {shares:?}");
        assert_eq!(keys.len(), MOMENTS);
        let unique: HashSet<&str> = titles.iter().copied().collect();
        assert_eq!(unique.len(), titles.len(), "{titles:?}");
        let recs = Recs::new(&l.catalog, &m.taste, &l.index);
        for mix in &mixes {
            assert!((MIX_LEN - 4..=MIX_LEN).contains(&mix.tracks.len()), "{}: {}", mix.title, mix.tracks.len());
            let picks: Vec<crate::Pick> = mix.tracks.iter().map(|&i| crate::Pick { item: crate::Item::Track(m.id(i)), reason: Reason::Favourite }).collect();
            crate::pick::assert_diverse(&recs, &picks);
            let mut worlds: HashMap<u64, usize> = HashMap::new();
            for &i in &mix.tracks {
                *worlds.entry(l.catalog.tracks()[i].world.unwrap()).or_default() += 1;
            }
            let cap = (mix.tracks.len() as f32 * WORLD_SHARE).ceil() as usize;
            assert!(worlds.len() >= 4 && worlds.values().all(|c| *c <= cap), "{}: {worlds:?}", mix.title);
        }
        // «Не предлагать этот микс»: the epoch's lineup goes on without it,
        // another in its place.
        let gone = keys[1];
        let kept = Kept { lineup: keys.clone(), ..Kept::default() };
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(&m);
        let no = |_: usize| false;
        let settings = Settings { hidden: vec![gone], ..Settings::default() };
        let b = Builder::new(&recs, &m, &|_| true, &no, m.now().div_euclid(DAY_MS) as u64, DayPart::Day, Live::default(), settings);
        let (after, after_keys) = b.lineup(&kept);
        assert!(!after_keys.contains(&gone) && after.iter().all(|x| x.key != gone), "{after_keys:?}");
        assert_eq!(after.len(), mixes.len(), "{after_keys:?}");
        assert_eq!(after_keys[..3], [keys[0], keys[2], keys[3]], "the rest in their places");
    }

    /// The listener of [`lib`] with an old collection brought from a
    /// service: the 40 % of each kind never played here, each played five
    /// times three years ago there and all liked at once yesterday; and the
    /// first forty favourites of each kind liked by hand over the last
    /// weeks. The old songs, by their numbers.
    fn with_old_collection(mut l: Lib) -> (Lib, Vec<usize>) {
        let per = 110usize;
        let fav = per * 3 / 5;
        let old: Vec<usize> = (0..3).flat_map(|k| (fav..per).map(move |t| k * per + t)).collect();
        for &i in &old {
            for n in 0..5i64 {
                let at = T0 - (3 * 365 + n * 7) * DAY_MS + i as i64 * 1_000;
                l.input.events.push(Play { origin: Some(EventOrigin::Import), ..Play::new(tid(i as u128), at, EventType::Start, Some(0)) });
                l.input.events.push(Play { origin: Some(EventOrigin::Import), ..Play::new(tid(i as u128), at + 180_000, EventType::Complete, Some(180_000)) });
            }
            l.input.likes.push(tid(i as u128));
            l.input.like_times.push((tid(i as u128), T0 - DAY_MS + i as i64));
        }
        for k in 0..3 {
            for t in 0..40usize {
                let i = (k * per + t) as u128;
                l.input.likes.push(tid(i));
                l.input.like_times.push((tid(i), T0 - (k * 40 + t) as i64 * 4 * HOUR_MS - 2 * DAY_MS));
            }
        }
        l.input.events.sort_by_key(|p| p.ts);
        (l, old)
    }

    /// A collection brought from a service — liked all at once, last
    /// played years ago — does not take the mixes over: its songs count
    /// for a third of what is heard now, a mix takes a quarter of them at
    /// most («Любимые» half), the mixes of the day a third among them all.
    #[test]
    fn an_old_imported_collection_is_a_minority() {
        let (l, old) = with_old_collection(lib(40, &[]));
        let m = model(&l, T0);
        let old: HashSet<usize> = old.iter().filter_map(|&i| m.position(tid(i as u128))).collect();
        assert_eq!(old.len(), 132);
        with(&l, &m, DayPart::Day, |b| {
            for &i in &old {
                assert!(b.stale_at(i), "{i}: liked in a bulk and played years ago is an old song");
                assert!(b.fresh_at(i) <= STALE_FLOOR + 0.01, "{i}: {}", b.fresh_at(i));
            }
            let heard_now = (0..m.len()).filter(|i| !old.contains(i) && m.tracks[*i].starts > 0);
            for i in heard_now {
                assert!(!b.stale_at(i) && b.fresh_at(i) > 0.8, "{i}: {}", b.fresh_at(i));
            }
            let mut mixes = b.lineup(&Kept::default()).0;
            mixes.extend(b.pinned(&Kept::default()));
            mixes.extend(b.mix(MixKey::Liked, &Kept::default()));
            assert!(mixes.iter().any(|x| x.key == MixKey::Liked), "the likes still make a mix");
            for mix in &mixes {
                let n = mix.tracks.iter().filter(|i| old.contains(i)).count();
                let (share, len) = if mix.key == MixKey::Liked { (LIKED_STALE_SHARE, LIKED_LEN) } else { (STALE_SHARE, MIX_LEN) };
                assert!(n as f32 <= (share * len as f32).ceil(), "{}: {n} old songs of {}", mix.title, mix.tracks.len());
            }
            let daily = b.daily(&Kept::default()).0;
            assert!(daily.len() >= 4, "{}", daily.len());
            let all: usize = daily.iter().map(|x| x.tracks.len()).sum();
            let of_old: usize = daily.iter().map(|x| x.tracks.iter().filter(|i| old.contains(i)).count()).sum();
            assert!(of_old * 3 <= all, "the mixes of the day: {of_old} old songs of {all}");
        });
        // With nothing heard at all and no like set by hand, the likes are
        // what there is: none of them is old, each counts whole.
        let brought: HashSet<TrackId> = old.iter().map(|&i| m.id(i)).collect();
        let mut only_likes = l;
        only_likes.input.events.clear();
        only_likes.input.likes.retain(|t| brought.contains(t));
        only_likes.input.like_times.retain(|(t, _)| brought.contains(t));
        let m = model(&only_likes, T0);
        with(&only_likes, &m, DayPart::Day, |b| {
            let liked: Vec<usize> = (0..m.len()).filter(|&i| m.tracks[i].liked(T0)).collect();
            assert_eq!(liked.len(), 132);
            let bulk = liked.iter().filter(|&&i| old.contains(&i));
            for &i in bulk {
                assert!(!b.stale_at(i) && b.fresh_at(i) == 1.0, "{i}");
            }
        });
    }

    /// Everything loved of a kind is old (heard half a year before what is
    /// heard now) and the kind has songs never played: the old songs keep
    /// their quarter, the never played take the room (each still between
    /// two loved ones, the mix shorter if need be); «Любимые» are the likes
    /// whatever their age.
    #[test]
    fn old_songs_keep_their_quarter_when_all_loved_is_old() {
        let per = 110usize;
        let fav = per * 3 / 5;
        let mut extra = Vec::new();
        // Russian favourites heard now, Japanese ones half a year ago.
        for d in 0..20usize {
            for k in 0..6usize {
                let x = (d * 6 + k) % fav;
                extra.push((T0 - (d as i64 + 1) * DAY_MS + k as i64 * 600_000, x));
                extra.push((T0 - (200 + d as i64) * DAY_MS + k as i64 * 600_000, per + x));
            }
        }
        let l = lib(0, &extra);
        let m = model(&l, T0);
        let ja = MixKey::Kind(lane_code(LaneKey::Sung(Lang::Japanese)));
        let quarter = (STALE_SHARE * MIX_LEN as f32).ceil() as usize;
        with(&l, &m, DayPart::Day, |b| {
            let old: HashSet<usize> = (per..per + fav).filter_map(|i| m.position(tid(i as u128))).collect();
            assert!(old.iter().all(|&i| b.stale_at(i)), "half a year before what is heard now is old");
            let pool = b.pool(ja).expect("a pool of the kind");
            let (unheard, stale) = b.unheard_and_stale(&pool, MIX_LEN, 0, false);
            assert_eq!(stale, quarter, "the quarter holds");
            assert!(unheard > (UNHEARD_SHARE * MIX_LEN as f32).ceil() as usize, "the never played take the room: {unheard}");
            let (_, liked) = b.unheard_and_stale(&pool, MIX_LEN, 0, true);
            assert_eq!(liked, MIX_LEN, "«Любимые» take their old likes");
            if let Some(mix) = b.mix(ja, &Kept::default()) {
                let n = mix.tracks.iter().filter(|i| old.contains(i)).count();
                assert!(n <= quarter.max(MIN_SONGS), "{n} old songs of {}", mix.tracks.len());
            }
        });
    }

    /// A fresh import of another service's history (plays of the last
    /// days) does not take the taste profiles' shares over beside the
    /// user's own plays: what is listened to here leads.
    #[test]
    fn a_fresh_import_does_not_lead_the_shares() {
        let per = 110usize;
        let mut l = lib(40, &[]);
        for k in 0..240usize {
            let at = T0 - 10 * DAY_MS + k as i64 * 2_000_000;
            let i = (per + k % (per * 3 / 5)) as u128;
            l.input.events.push(Play { origin: Some(EventOrigin::Import), ..Play::new(tid(i), at, EventType::Start, Some(0)) });
            l.input.events.push(Play { origin: Some(EventOrigin::Import), ..Play::new(tid(i), at + 180_000, EventType::Complete, Some(180_000)) });
        }
        l.input.events.sort_by_key(|p| p.ts);
        let m = model(&l, T0);
        let share = |lane: LaneKey| -> f32 { m.profiles.list.iter().zip(&m.profile_share).filter(|(p, _)| p.lane == lane).map(|(_, s)| *s).sum() };
        let (ru, ja) = (share(LaneKey::Sung(Lang::Russian)), share(LaneKey::Sung(Lang::Japanese)));
        assert!(ru > ja, "the own listening leads: ru {ru}, ja {ja}");
    }

    /// The day's mixes: one «Микс дня» of every profile listened to now,
    /// then the named mixes, one at least for each taste profile, five at
    /// most, no song in two of the named ones, a share of songs never
    /// played; the same all day whatever is played meanwhile and whatever
    /// the part of the day, other songs the next day.
    #[test]
    fn the_mixes_of_the_day_are_distinct_and_the_days_own() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let day = 77u64;
        let (first, keys) = in_epoch(&l, &m, DayPart::Morning, day, |b| b.daily(&Kept::default()));
        assert_eq!(first[0].key, MixKey::Daily(0));
        assert_eq!(first[0].title, "Микс дня");
        assert!(first[0].profiles.len() >= 2, "«Микс дня» is of every profile: {:?}", first[0].profiles);
        let named = &first[1..];
        assert!((3..=DAILY_MIXES).contains(&named.len()), "{}", named.len());
        let mut seen: HashSet<usize> = HashSet::new();
        let mut profiles: HashSet<usize> = HashSet::new();
        for (k, mix) in first.iter().enumerate() {
            assert_eq!(mix.key, keys[k]);
            assert!((MIN_SONGS..=MIX_LEN).contains(&mix.tracks.len()), "{}: {}", mix.title, mix.tracks.len());
            let unheard = mix.tracks.iter().filter(|&&i| m.tracks[i].starts == 0).count();
            assert!(unheard as f32 <= (DAILY_UNHEARD * MIX_LEN as f32).ceil(), "{}: {unheard} never played", mix.title);
        }
        for mix in named {
            assert!(mix.key.is_cluster() && !MixKey::Daily(0).is_cluster());
            // Its profile's name until its makeup names it: never a number.
            assert!(!mix.title.is_empty() && !mix.title.chars().any(|c| c.is_ascii_digit()), "{}", mix.title);
            assert!(!mix.title.starts_with("Микс дня"), "one mix of the day: {}", mix.title);
            assert!(mix.profiles.len() <= 1, "{}: of {:?}", mix.title, mix.profiles);
            profiles.extend(mix.profiles.iter().copied());
            for &i in &mix.tracks {
                assert!(seen.insert(i), "{}: a song of another named mix", mix.title);
            }
        }
        assert!(profiles.len() >= 3, "each profile has its named mix: {profiles:?}");
        // Another part of the same day, nothing kept: the same.
        let (evening, _) = in_epoch(&l, &m, DayPart::Evening, day, |b| b.daily(&Kept::default()));
        let songs = |x: &[Mix]| x.iter().map(|m| (m.key, m.tracks.clone())).collect::<Vec<_>>();
        assert_eq!(songs(&evening), songs(&first));
        // Ten more songs played, the model built again: the day's mixes as
        // they were kept.
        let mut kept = Kept { lineup: keys.clone(), ..Kept::default() };
        kept.keep(&first, &m);
        let plays: Vec<(UnixMs, usize)> = (0..10).map(|k| (T0 + 9 * HOUR_MS + k * 200_000, 3 * k as usize)).collect();
        let l2 = lib(40, &plays);
        let m2 = model(&l2, T0 + 10 * HOUR_MS);
        let (again, keys2) = in_epoch(&l2, &m2, DayPart::Day, day, |b| b.daily(&kept));
        assert_eq!(keys2, keys);
        let ids = |m: &UserModel, x: &[Mix]| x.iter().map(|mix| mix.tracks.iter().map(|&i| m.id(i)).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!(ids(&m2, &again), ids(&m, &first));
        // The next day draws again.
        let (next, _) = in_epoch(&l, &m, DayPart::Morning, day + 1, |b| b.daily(&Kept::default()));
        assert_eq!(next.len(), first.len());
        assert!(next.iter().zip(&first).any(|(a, b)| a.tracks != b.tracks), "another day, other songs");
    }

    /// The named mixes are called by what they are made of: the Russian
    /// songs' artists are tagged as rappers, so their mix is «Хип-хоп
    /// микс»; nobody tagged the others, so they go by an artist, a mood or
    /// their kind. No two cards alike, none of a name the row has, none
    /// numbered, and «Микс дня» stays what it is.
    #[test]
    fn the_named_mixes_are_named_by_their_makeup() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        in_epoch(&l, &m, DayPart::Morning, 77, |b| {
            let (mut mixes, _) = b.daily(&Kept::default());
            let n = mixes.len();
            let kind = |a: ArtistId| (a.0.as_u128() / 100) as usize;
            let tags = |a: ArtistId| if kind(a) == 0 { vec![("hip hop".to_owned(), 5.0), ("russian".to_owned(), 2.0)] } else { Vec::new() };
            let artist = |a: ArtistId| Some(format!("{} {}", ["Ru", "Ja", "Ost"][kind(a).min(2)], (b'a' + (a.0.as_u128() % 100) as u8) as char));
            let year = |_: usize| None;
            let song = |i: usize| Some(format!("Song {i}"));
            let makeups = b.makeups(&mixes, &tags, &artist, &year, &song);
            assert_eq!(makeups.len(), n);
            for (mix, made) in mixes.iter().zip(&makeups) {
                assert_eq!(made.tags.len(), mix.tracks.len());
                assert!(!made.artists.is_empty() && made.kind.is_some());
            }
            let taken = ["Любимые".to_owned(), "Японские песни".to_owned()];
            b.name_daily(&mut mixes, &makeups, &taken, None);
            assert_eq!(mixes.len(), n, "every mix has something to be named by");
            assert_eq!(mixes[0].title, "Микс дня");
            let titles: Vec<&str> = mixes.iter().map(|x| x.title.as_str()).collect();
            assert!(titles.contains(&"Хип-хоп микс"), "{titles:?}");
            let unique: HashSet<&str> = titles.iter().copied().collect();
            assert_eq!(unique.len(), titles.len(), "{titles:?}");
            for t in &titles {
                assert!(!t.trim().is_empty() && !t.chars().any(|c| c.is_ascii_digit()), "{titles:?}");
                assert!(!taken.iter().any(|x| x == t), "{titles:?}");
            }
        });
    }

    /// The mix of the part of the day is there once anything was played,
    /// called by what it is for, the epoch's, and can be left out. The mix
    /// of the liked songs is not beside it («Любимые» is the playlist's
    /// tile), though it can still be drawn.
    #[test]
    fn the_part_of_the_day_has_its_mix() {
        let mut l = lib(40, &[]);
        let m = model(&l, T0);
        let morning = MixKey::Part(DayPart::Morning.index() as u8);
        let pinned = with(&l, &m, DayPart::Morning, |b| b.pinned(&Kept::default()));
        assert_eq!(pinned.iter().map(|x| x.key).collect::<Vec<_>>(), [morning]);
        assert_eq!(pinned[0].title, "Доброе утро");
        assert!(pinned[0].tracks.iter().all(|&i| m.tracks[i].starts > 0), "of what is loved");
        // Every third song liked by hand over the last weeks.
        for (n, i) in (0..330u128).step_by(3).enumerate() {
            l.input.likes.push(tid(i));
            l.input.like_times.push((tid(i), T0 - n as i64 * 5 * HOUR_MS));
        }
        let m = model(&l, T0);
        let (pinned, liked) = with(&l, &m, DayPart::Morning, |b| (b.pinned(&Kept::default()), b.mix(MixKey::Liked, &Kept::default())));
        assert_eq!(pinned.iter().map(|x| x.key).collect::<Vec<_>>(), [morning], "the liked songs' mix is not in the row");
        let liked = liked.expect("the liked songs still make a mix");
        assert_eq!(liked.title, "Микс любимых");
        assert!((MIN_SONGS..=LIKED_LEN).contains(&liked.tracks.len()), "{}", liked.tracks.len());
        assert!(liked.tracks.iter().all(|&i| m.tracks[i].liked(T0)), "liked songs alone");
        // Kept for the epoch: the same songs in their order.
        let mut kept = Kept::default();
        kept.keep(&pinned, &m);
        let again = with(&l, &m, DayPart::Morning, |b| b.pinned(&kept));
        assert_eq!(again.iter().map(|x| (x.key, x.tracks.clone())).collect::<Vec<_>>(), pinned.iter().map(|x| (x.key, x.tracks.clone())).collect::<Vec<_>>());
        // «Не предлагать этот микс».
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(&m);
        let no = |_: usize| false;
        let settings = Settings { hidden: vec![morning], ..Settings::default() };
        let b = Builder::new(&recs, &m, &|_| true, &no, 1, DayPart::Morning, Live::default(), settings);
        assert!(b.pinned(&Kept::default()).is_empty());
        // A part's mix is not taken for «Микс дня».
        let parts: Vec<&str> = DayPart::ALL.iter().map(|p| part_title_in(p.index() as u8, Locale::Ru)).collect();
        for want in ["Доброе утро", "День", "Вечер", "Ночь"] {
            assert!(parts.contains(&want), "{parts:?}");
        }
        let parts_en: Vec<&str> = DayPart::ALL.iter().map(|p| part_title_in(p.index() as u8, Locale::En)).collect();
        assert_eq!(parts_en, ["Good Morning", "Daytime", "Evening", "Night"]);
        assert!(parts.iter().all(|t| !t.to_lowercase().contains("микс")), "{parts:?}");
        assert_ne!(MixKey::Liked.token(), MixKey::Daily(1).token());
        assert_ne!(MixKey::Daily(0).token(), MixKey::Daily(1).token());
        assert_ne!(MixKey::Genre(1).token(), MixKey::Daily(1).token());
        assert_ne!(MixKey::Genre(1).token(), MixKey::Genre(2).token());
    }

    /// [`in_epoch`] with the genres of the artists known.
    fn tagged<T>(l: &Lib, m: &UserModel, part: DayPart, epoch: u64, tags: &dyn Fn(ArtistId) -> Vec<(String, f32)>, f: impl FnOnce(&Builder<'_>) -> T) -> T {
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
        let no = |_: usize| false;
        let b = Builder::new(&recs, m, &|_| true, &no, epoch, part, Live::default(), Settings::default()).with_genres(tags);
        f(&b)
    }

    /// The kind of [`lib`] an artist is of: 0 the Russian songs, 1 the
    /// Japanese ones, 2 the soundtracks.
    fn artist_kind(a: ArtistId) -> usize {
        (a.0.as_u128() / 100) as usize
    }

    fn track_kind(l: &Lib, i: usize) -> usize {
        artist_kind(l.catalog.tracks()[i].artists[0])
    }

    /// The artists of the Russian songs are rappers, those of the Japanese
    /// ones sing J-pop; nobody tagged the soundtracks'.
    fn two_genres(a: ArtistId) -> Vec<(String, f32)> {
        match artist_kind(a) {
            0 => vec![("hip hop".to_owned(), 5.0), ("russian".to_owned(), 2.0)],
            1 => vec![("j-pop".to_owned(), 4.0), ("pop".to_owned(), 1.0)],
            _ => Vec::new(),
        }
    }

    /// A library whose artists are tagged with two genres has a mix of
    /// each, built of that genre's songs alone and called by it, right
    /// after «Микс дня»; no song is in two of them, most of each is
    /// familiar; another day draws other songs.
    #[test]
    fn genre_mixes_are_built_by_genre() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let (mixes, keys) = tagged(&l, &m, DayPart::Morning, 77, &two_genres, |b| b.daily(&Kept::default()));
        assert_eq!(keys, mixes.iter().map(|x| x.key).collect::<Vec<_>>());
        assert_eq!((mixes[0].key, mixes[0].title.as_str()), (MixKey::Daily(0), "Микс дня"));
        let genres: Vec<&Mix> = mixes.iter().filter(|x| x.key.is_genre()).collect();
        let mut titles: Vec<&str> = genres.iter().map(|x| x.title.as_str()).collect();
        titles.sort_unstable();
        assert_eq!(titles, ["J-pop микс", "Хип-хоп микс"], "nobody tagged the soundtracks: no mix of theirs");
        assert!(mixes[1].key.is_genre() && mixes[2].key.is_genre(), "after «Микс дня»");
        assert!(mixes[3..].iter().all(|x| x.key.is_cluster()), "before the named mixes");
        let mut seen: HashSet<usize> = HashSet::new();
        for mix in &genres {
            let family = crate::mix_names::family_id_in(&mix.title, Locale::Ru);
            assert_eq!(Some(mix.key), family.map(MixKey::Genre), "{}", mix.title);
            assert_eq!(mix.genre, family, "{}", mix.title);
            assert!((MIN_SONGS..=MIX_LEN).contains(&mix.tracks.len()), "{}: {}", mix.title, mix.tracks.len());
            let kind = usize::from(mix.title != "Хип-хоп микс");
            assert!(mix.tracks.iter().all(|&i| track_kind(&l, i) == kind), "{}: a song of another genre", mix.title);
            let unheard = mix.tracks.iter().filter(|&&i| m.tracks[i].starts == 0).count();
            assert!(unheard as f32 <= (DAILY_UNHEARD * MIX_LEN as f32).ceil(), "{}: {unheard} never played", mix.title);
            assert!(unheard * 2 < mix.tracks.len(), "{}: mostly familiar, {unheard} never played", mix.title);
            for &i in &mix.tracks {
                assert!(seen.insert(i), "{}: a song of another genre mix", mix.title);
            }
        }
        let (next, _) = tagged(&l, &m, DayPart::Morning, 78, &two_genres, |b| b.daily(&Kept::default()));
        for mix in &genres {
            let other = next.iter().find(|x| x.key == mix.key).expect("the same genres the next day");
            assert_ne!(other.tracks, mix.tracks, "{}: another day, other songs", mix.title);
        }
    }

    /// A genre the listener hardly listens to has no mix, however many of
    /// its songs the library holds.
    #[test]
    fn a_genre_needs_songs_listened_to_now() {
        // Thirty Russian songs played thrice over the last days, four
        // Japanese ones.
        let plays: Vec<(UnixMs, usize)> = (0..3i64)
            .flat_map(|r| {
                let ru = (0..30usize).map(move |t| (T0 - (r * 40 + t as i64 + 1) * HOUR_MS, t));
                let ja = (0..4usize).map(move |t| (T0 - (r * 40 + t as i64 + 32) * HOUR_MS, 110 + t));
                ru.chain(ja)
            })
            .collect();
        let l = lib(0, &plays);
        let m = model(&l, T0);
        let (mixes, _) = tagged(&l, &m, DayPart::Day, 77, &two_genres, |b| b.daily(&Kept::default()));
        let genres: Vec<&str> = mixes.iter().filter(|x| x.key.is_genre()).map(|x| x.title.as_str()).collect();
        assert_eq!(genres, ["Хип-хоп микс"], "four songs heard are no J-pop mix");
    }

    /// What a build has for the row: the day's mixes with their lineup,
    /// the mix of the part of the day and the moments.
    fn drawn(b: &Builder<'_>) -> (Vec<Mix>, Vec<MixKey>, Option<Mix>, Vec<Mix>) {
        let (daily, keys) = b.daily(&Kept::default());
        (daily, keys, b.pinned(&Kept::default()).into_iter().next(), b.lineup(&Kept::default()).0)
    }

    /// With no tags at all there are no genre mixes and nothing else
    /// changes: the row is «Микс дня», the part's mix, a named mix or two
    /// of the clusters and the moments, none without a name.
    #[test]
    fn without_tags_the_row_is_the_clusters() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let plain = in_epoch(&l, &m, DayPart::Morning, 77, drawn);
        let no_tags = |_: ArtistId| Vec::new();
        let none = tagged(&l, &m, DayPart::Morning, 77, &no_tags, drawn);
        // The cards and their songs (a score sums in no fixed order).
        let songs = |d: &(Vec<Mix>, Vec<MixKey>, Option<Mix>, Vec<Mix>)| -> Vec<(MixKey, String, Vec<usize>)> {
            d.0.iter().chain(&d.2).chain(&d.3).map(|m| (m.key, m.title.clone(), m.tracks.clone())).collect()
        };
        assert_eq!(songs(&none), songs(&plain), "tags that say nothing change nothing");
        assert_eq!(none.1, plain.1);
        let (daily, keys, part, moments) = none;
        assert!(keys.iter().all(|k| !k.is_genre()), "{keys:?}");
        let cards = row(daily, part, moments);
        assert_eq!(cards[0].key, MixKey::Daily(0));
        assert!(cards.len() <= ROW_MIXES, "{}", cards.len());
        let named = cards.iter().filter(|c| c.key.is_cluster()).count();
        assert!(named <= ROW_NAMED, "{named} named mixes");
        for c in &cards {
            assert!(!c.title.trim().is_empty(), "{:?} has no name", c.key);
            assert!(!c.key.is_genre() && c.genre.is_none(), "{}", c.title);
        }
        assert!(row(Vec::new(), None, Vec::new()).is_empty());
    }

    /// The row of a library with two genres: «Микс дня», the part's mix,
    /// the genre mixes, the named mixes, the moments, ten at most; no two
    /// cards of one name, none sharing more than [`OVERLAP`] of its songs
    /// with a card before it; the named mixes and the profiles that are a
    /// genre's songs again are not there, nor is the liked songs' mix.
    #[test]
    fn the_row_has_no_look_alikes() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let (daily, part, moments) = tagged(&l, &m, DayPart::Morning, 77, &two_genres, |b| {
            let (mut daily, _, part, moments) = drawn(b);
            let artist = |a: ArtistId| Some(format!("Artist {}", a.0.as_u128()));
            let year = |_: usize| None;
            let song = |i: usize| Some(format!("Song {i}"));
            let makeups = b.makeups(&daily, &two_genres, &artist, &year, &song);
            let taken: Vec<String> = part.iter().chain(&moments).map(|x| x.title.clone()).collect();
            b.name_daily(&mut daily, &makeups, &taken, None);
            (daily, part, moments)
        });
        // A cluster of rappers is «Хип-хоп микс» again: only the
        // soundtracks' clusters are left.
        for c in daily.iter().filter(|c| c.key.is_cluster()) {
            assert!(c.tracks.iter().all(|&i| track_kind(&l, i) == 2), "«{}» is a genre's mix again", c.title);
        }
        let cards = row(daily, part, moments);
        let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
        assert!(cards.len() <= ROW_MIXES, "{titles:?}");
        assert_eq!(cards[0].key, MixKey::Daily(0), "{titles:?}");
        assert!(titles.contains(&"Хип-хоп микс") && titles.contains(&"J-pop микс"), "{titles:?}");
        let rank = |k: MixKey| match k {
            MixKey::Daily(0) => 0,
            MixKey::Part(_) => 1,
            MixKey::Genre(_) => 2,
            MixKey::Daily(_) => 3,
            _ => 4,
        };
        assert!(cards.windows(2).all(|w| rank(w[0].key) <= rank(w[1].key)), "{titles:?}");
        assert!(cards.iter().filter(|c| c.key.is_cluster()).count() <= ROW_NAMED, "{titles:?}");
        let side = |x: &Mix| matches!(x.key, MixKey::Calm | MixKey::Lively);
        for (k, b) in cards.iter().enumerate() {
            assert!(!b.title.trim().is_empty(), "{titles:?}");
            assert!(b.key != MixKey::Liked && b.title != "Микс любимых", "{titles:?}");
            for a in &cards[..k] {
                assert_ne!(a.title.trim().to_lowercase(), b.title.trim().to_lowercase(), "{titles:?}");
                let set: HashSet<usize> = a.tracks.iter().copied().collect();
                let shared = b.tracks.iter().filter(|i| set.contains(i)).count();
                assert!(shared as f32 <= OVERLAP * a.tracks.len().min(b.tracks.len()) as f32, "«{}» and «{}» share {shared} songs", a.title, b.title);
                assert!(!(b.kind.is_some() && a.kind == b.kind && side(a) != side(b)), "«{}» and «{}» are of one kind", a.title, b.title);
            }
        }
        // The Russian songs are the rappers', the Japanese ones J-pop: the
        // profiles' own mixes would be the genres' again.
        for c in cards.iter().filter(|c| matches!(c.key, MixKey::Profile(_))) {
            assert!(c.tracks.iter().all(|&i| track_kind(&l, i) == 2), "«{}» is a genre's mix again: {titles:?}", c.title);
        }
    }

    /// A card for [`row`]: `tracks` are its songs.
    fn card(key: MixKey, title: &str, tracks: std::ops::Range<usize>) -> Mix {
        let genre = if let MixKey::Genre(f) = key { Some(f) } else { None };
        Mix { key, title: title.to_owned(), short: Vec::new(), tracks: tracks.collect(), anchors: Vec::new(), artists: Vec::new(), profiles: Vec::new(), score: 0.0, kind: None, genre, pool_size: (0, 0), pool: Vec::new() }
    }

    /// The row's order and its ten cards; the named mixes fill it, two of
    /// them, one beside three genre mixes; of two cards alike — by the
    /// name, the songs, the genre, the kind of song — the first stays; a
    /// blend always does.
    #[test]
    fn the_row_keeps_ten_cards_and_the_first_of_two_alike() {
        use MixKey::{Calm, Daily, Genre, Lively, Part, Profile};
        let n = 20usize;
        let at = |k: usize| k * n..(k + 1) * n;
        let daily = |genres: u8| -> Vec<Mix> {
            let mut d = vec![card(Daily(0), "Микс дня", at(0))];
            d.extend((1..=genres).map(|g| card(Genre(g), crate::mix_names::family_title_in(g, Locale::Ru).unwrap(), at(usize::from(g)))));
            d.extend((1..=5u8).map(|c| card(Daily(c), &format!("Микс: артист {c}"), at(10 + usize::from(c)))));
            d
        };
        let part = || Some(card(Part(2), part_title_in(2, Locale::Ru), at(9)));
        let moments = || -> Vec<Mix> { (1..=6u32).map(|p| card(Profile(p), &format!("Профиль {p}"), at(20 + p as usize))).collect() };
        let keys = |r: &[Mix]| r.iter().map(|m| m.key).collect::<Vec<_>>();

        let full = row(daily(4), part(), moments());
        assert_eq!(full.len(), ROW_MIXES);
        assert_eq!(keys(&full), [Daily(0), Part(2), Genre(1), Genre(2), Genre(3), Genre(4), Daily(1), Profile(1), Profile(2), Profile(3)]);
        let two = row(daily(2), part(), moments());
        assert_eq!(keys(&two), [Daily(0), Part(2), Genre(1), Genre(2), Daily(1), Daily(2), Profile(1), Profile(2), Profile(3), Profile(4)]);
        let none = row(daily(0), None, moments());
        assert_eq!(keys(&none), [Daily(0), Daily(1), Daily(2), Profile(1), Profile(2), Profile(3), Profile(4), Profile(5), Profile(6)]);

        // The name, the songs, the genre.
        let mut m = moments();
        m[0].title = " микс ДНЯ".into();
        m[1].tracks = (n..n + 14).chain(900..906).collect();
        m[2].tracks = (2 * n..2 * n + 13).chain(910..917).collect();
        m[3].genre = Some(1);
        m[4].genre = Some(9);
        let r = row(daily(2), part(), m);
        assert_eq!(keys(&r), [Daily(0), Part(2), Genre(1), Genre(2), Daily(1), Daily(2), Profile(3), Profile(5), Profile(6)]);

        // The kind of song: a profile's mix and a side that keeps to its
        // language are one; two profiles of a kind, or both sides, are not.
        let ru = Some(Kind::Sung(Lang::Russian));
        let kinded = |key: MixKey, title: &str, k: usize, kind: Option<Kind>| Mix { kind, ..card(key, title, at(k)) };
        let m = vec![
            kinded(Profile(1), "Русские песни", 30, ru),
            kinded(Lively, "Русские песни погромче", 31, ru),
            kinded(Profile(2), "Русские песни, потише", 32, ru),
            kinded(Calm, "Спокойное", 33, None),
        ];
        assert_eq!(keys(&row(Vec::new(), None, m)), [Profile(1), Profile(2), Calm]);
        let m = vec![kinded(Calm, "Русские песни потише", 30, ru), kinded(Lively, "Русские песни погромче", 31, ru), kinded(Profile(1), "Русские песни", 32, ru)];
        assert_eq!(keys(&row(Vec::new(), None, m)), [Calm, Lively]);
        // A named mix called by its kind stands for it (even in English).
        let r = row(vec![kinded(Daily(1), "Russian Songs", 1, ru)], None, vec![kinded(Lively, "Upbeat Russian Songs", 31, ru)]);
        assert_eq!(keys(&r), [Daily(1)]);
        // A blend is the user's own.
        let blend = MixKey::Blend([1, 2, 0]);
        let r = row(vec![card(Daily(0), "Микс дня", at(0))], None, vec![card(blend, "Микс дня", at(0))]);
        assert_eq!(keys(&r), [Daily(0), blend]);
    }

    /// More cards asked for at the row's end: the row as it was, then
    /// what it left out for want of room (the named mixes past their
    /// share, the cards past ten), none alike.
    #[test]
    fn more_cards_come_after_the_row_as_it_was() {
        use MixKey::{Daily, Genre, Part, Profile};
        let n = 20usize;
        let at = |k: usize| k * n..(k + 1) * n;
        let daily = || -> Vec<Mix> {
            let mut d = vec![card(Daily(0), "Микс дня", at(0))];
            d.extend((1..=4u8).map(|g| card(Genre(g), crate::mix_names::family_title_in(g, Locale::Ru).unwrap(), at(usize::from(g)))));
            d.extend((1..=5u8).map(|c| card(Daily(c), &format!("Микс: артист {c}"), at(10 + usize::from(c)))));
            d
        };
        let part = || Some(card(Part(2), part_title_in(2, Locale::Ru), at(9)));
        let moments = || -> Vec<Mix> {
            let mut m: Vec<Mix> = (1..=6u32).map(|p| card(Profile(p), &format!("Профиль {p}"), at(20 + p as usize))).collect();
            // One alike to a card of the row: it never comes.
            m[5].title = "Микс дня".into();
            m
        };
        let keys = |r: &[Mix]| r.iter().map(|m| m.key).collect::<Vec<_>>();
        let base = row(daily(), part(), moments());
        assert_eq!(keys(&row_upto(daily(), part(), moments(), 0)), keys(&base));
        let more = row_upto(daily(), part(), moments(), 4);
        assert_eq!(keys(&more[..ROW_MIXES]), keys(&base), "the row stays as it was");
        assert_eq!(keys(&more[ROW_MIXES..]), [Daily(2), Daily(3), Daily(4), Daily(5)]);
        // All it has: the rest of the moments, not the look-alike.
        let all = row_upto(daily(), part(), moments(), 100);
        assert_eq!(keys(&all[ROW_MIXES..]), [Daily(2), Daily(3), Daily(4), Daily(5), Profile(4), Profile(5)]);
    }

    /// The day's genre mixes stay the day's: after more plays, at another
    /// part of the day and with the tags gone they are the same songs in
    /// their order; «Другой состав» draws other songs of the same genre.
    #[test]
    fn the_days_genre_mixes_stay_and_redraw() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let day = 77u64;
        let (first, keys) = tagged(&l, &m, DayPart::Morning, day, &two_genres, |b| b.daily(&Kept::default()));
        assert_eq!(keys.iter().filter(|k| k.is_genre()).count(), 2, "{keys:?}");
        let mut kept = Kept { lineup: keys.clone(), ..Kept::default() };
        kept.keep(&first, &m);
        let plays: Vec<(UnixMs, usize)> = (0..10).map(|k| (T0 + 9 * HOUR_MS + k * 200_000, 3 * k as usize)).collect();
        let l2 = lib(40, &plays);
        let m2 = model(&l2, T0 + 10 * HOUR_MS);
        let ids = |m: &UserModel, x: &[Mix]| x.iter().map(|mix| (mix.key, mix.tracks.iter().map(|&i| m.id(i)).collect::<Vec<_>>())).collect::<Vec<_>>();
        let (again, keys2) = tagged(&l2, &m2, DayPart::Evening, day, &two_genres, |b| b.daily(&kept));
        assert_eq!(keys2, keys);
        assert_eq!(ids(&m2, &again), ids(&m, &first));
        let (untagged, _) = in_epoch(&l2, &m2, DayPart::Evening, day, |b| b.daily(&kept));
        assert_eq!(ids(&m2, &untagged), ids(&m, &first), "kept as drawn, whatever the tags say now");
        // «Другой состав» of one of them.
        let was = first.iter().find(|x| x.key.is_genre()).unwrap();
        kept.salts.push((was.key, 7));
        let now = tagged(&l, &m, DayPart::Morning, day, &two_genres, |b| b.mix(was.key, &kept)).expect("drawn again");
        assert_eq!(now.title, was.title);
        let set: HashSet<usize> = was.tracks.iter().copied().collect();
        let shared = now.tracks.iter().filter(|i| set.contains(i)).count() as f32 / now.tracks.len() as f32;
        assert!(shared < 0.8, "{}: {shared} of it as before", now.title);
        let kind = track_kind(&l, was.tracks[0]);
        assert!(now.tracks.iter().all(|&i| track_kind(&l, i) == kind), "{}: of the same genre", now.title);
        // The others stay as they were.
        let (rest, _) = tagged(&l, &m, DayPart::Morning, day, &two_genres, |b| b.daily(&kept));
        for (a, b) in first.iter().zip(&rest).filter(|(a, _)| a.key != was.key) {
            assert_eq!(a.tracks, b.tracks, "{}", a.title);
        }
    }

    #[test]
    fn keys_and_names() {
        assert_eq!(MixKey::blend(&[3, 3, 5]), Some(MixKey::Blend([3, 5, 0])));
        let names = |n: &[&str]| n.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(blend_title_in(names(&["Русские песни", "Саундтреки без слов", "Японские песни"]), Locale::Ru), "Русские и японские песни + Саундтреки без слов");
        assert_eq!(blend_title_in(names(&["Японские песни", "Саундтреки без слов"]), Locale::Ru), "Японские песни + Саундтреки без слов");
        assert_eq!(
            blend_title_in(names(&["Russian Songs", "Instrumental Soundtracks", "Japanese Songs"]), Locale::En),
            "Russian & Japanese Songs + Instrumental Soundtracks"
        );
        // Shorter forms for a narrow card, of the kinds alone.
        let (ru, ja) = (Kind::Sung(Lang::Russian), Kind::Sung(Lang::Japanese));
        assert_eq!(
            blend_short_in(&[ru, Kind::InstrumentalSoundtracks, ja], Locale::Ru),
            ["Русские и японские песни + без слов", "Русские и японские + без слов"]
        );
        assert_eq!(
            blend_short_in(&[ru, Kind::InstrumentalSoundtracks, ja], Locale::En),
            ["Russian & Japanese Songs + instrumental", "Russian & Japanese + instrumental"]
        );
        assert_eq!(blend_short_in(&[ja, Kind::InstrumentalSoundtracks], Locale::Ru), ["Японские песни + без слов", "Японские + без слов"]);
        assert_eq!(blend_short_in(&[ru, ja], Locale::Ru), ["Русские и японские песни"]);
        assert_eq!(blend_short_in(&[ru, ja], Locale::En), ["Russian & Japanese Songs"]);
        assert_eq!(blend_short_in(&[Kind::Instrumental, Kind::Soundtracks], Locale::Ru), ["Без слов + саундтреки"]);
        assert_eq!(blend_short_in(&[Kind::Instrumental, Kind::Soundtracks], Locale::En), ["Instrumental + soundtracks"]);
        assert!(blend_short_in(&[ru, ru], Locale::Ru).is_empty(), "two of one kind: their names tell them apart");
        assert_eq!(MixKey::blend(&[3]), None);
        assert_ne!(MixKey::Calm.token(), MixKey::Lively.token());
        for l in [LaneKey::Instrumental, LaneKey::Unknown, LaneKey::Sung(Lang::Korean), LaneKey::Sung(Lang::Other)] {
            assert_eq!(lane_of_code(lane_code(l)), l);
        }
        let ru_lanes = || std::iter::repeat_n(LaneKey::Sung(Lang::Russian), 8).chain([LaneKey::Instrumental, LaneKey::Unknown]);
        assert_eq!(side_title_in(true, ru_lanes(), Locale::Ru), "Русские песни потише");
        assert_eq!(side_title_in(true, ru_lanes(), Locale::En), "Calm Russian Songs");
        let mixed = [LaneKey::Sung(Lang::Russian), LaneKey::Sung(Lang::Japanese), LaneKey::Sung(Lang::English), LaneKey::Instrumental];
        assert_eq!(side_title_in(false, mixed.into_iter().cycle().take(20), Locale::Ru), "Песни погромче");
        assert_eq!(side_title_in(false, mixed.into_iter().cycle().take(20), Locale::En), "Upbeat Songs");
        assert_eq!(side_title_in(true, std::iter::repeat_n(LaneKey::Instrumental, 7).chain([LaneKey::Sung(Lang::Russian); 3]), Locale::Ru), "Спокойное без слов");
        assert_eq!(side_title_in(true, std::iter::repeat_n(LaneKey::Instrumental, 7).chain([LaneKey::Sung(Lang::Russian); 3]), Locale::En), "Calm Instrumental");
        assert_eq!(side_title_in(false, [LaneKey::Instrumental, LaneKey::Sung(Lang::Russian)].into_iter().cycle().take(10), Locale::Ru), "Погромче");
        assert_eq!(side_title_in(false, [LaneKey::Instrumental, LaneKey::Sung(Lang::Russian)].into_iter().cycle().take(10), Locale::En), "Upbeat");
    }

    #[test]
    fn english_builder_renders_mixes_and_daily_names() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(&m);
        let no = |_: usize| false;
        let settings = Settings { locale: Locale::En, ..Settings::default() };
        let b = Builder::new(&recs, &m, &|_| true, &no, 77, DayPart::Morning, Live::default(), settings).with_genres(&two_genres);
        let (mut mixes, _) = b.daily(&Kept::default());
        assert_eq!(mixes[0].title, "Daily Mix");
        let genre_titles: Vec<&str> = mixes.iter().filter(|x| x.key.is_genre()).map(|x| x.title.as_str()).collect();
        assert!(genre_titles.contains(&"Hip-Hop Mix") && genre_titles.contains(&"J-Pop Mix"), "{genre_titles:?}");
        let artist = |a: ArtistId| Some(format!("Artist {}", a.0.as_u128()));
        let makeups = b.makeups(&mixes, &two_genres, &artist, &|_| None, &|i| Some(format!("Song {i}")));
        let taken = ["Liked Songs".to_owned(), "On Repeat".to_owned()];
        b.name_daily(&mut mixes, &makeups, &taken, None);
        assert_eq!(mixes[0].title, "Daily Mix");
        let pinned = b.pinned(&Kept::default());
        assert_eq!(pinned[0].title, "Good Morning");
        let (mut moments, _) = b.lineup(&Kept::default());
        b.named(&mut moments, &artist);
        assert!(!moments.is_empty());
        assert!(moments.iter().all(|x| !x.title.trim().is_empty()));
    }

    /// The soundtracks' part of the profiles' shares (the soundtracks are
    /// the catalog's places from 220 on in [`lib`]).
    fn soundtracks_share(m: &UserModel) -> f32 {
        (0..m.profiles.len())
            .map(|j| {
                let of: Vec<usize> = (0..m.len()).filter(|&i| m.profile_at(i) == Some(j)).collect();
                let st = of.iter().filter(|&&i| i >= 220).count() as f32 / of.len().max(1) as f32;
                m.profile_share.get(j).copied().unwrap_or(0.0) * st
            })
            .sum()
    }

    /// The voice assistant playing soundtracks on the speaker three nights
    /// running, eight hours each untouched after one command, hardly moves
    /// what the computer's mixes are made of — the same plays at the
    /// computer would; the user's sessions and background threshold stay
    /// as they were.
    #[test]
    fn the_speakers_background_does_not_move_the_computers_mixes() {
        use crate::context::{PlayActor, PlayDevice};
        let with_nights = |actor: PlayActor, device: PlayDevice| -> Lib {
            let mut l = lib(40, &[]);
            for night in 0..3i64 {
                let mut at = T0 - (night + 2) * DAY_MS + 21 * HOUR_MS;
                for k in 0..160usize {
                    let i = (220 + (k * 7 + night as usize * 13) % 110) as u128;
                    let origin = if k == 0 { EventOrigin::Manual } else { EventOrigin::Radio };
                    l.input.events.push(Play { origin: Some(origin), actor, device, ..Play::new(tid(i), at, EventType::Start, Some(0)) });
                    at += 179_000;
                    l.input.events.push(Play { actor, device, ..Play::new(tid(i), at, EventType::Complete, Some(179_000)) });
                    at += 1_000;
                }
            }
            l.input.events.sort_by_key(|p| p.ts);
            l
        };
        let base = lib(40, &[]);
        let m0 = model(&base, T0);
        let speaker = with_nights(PlayActor::Assistant, PlayDevice::Ambient);
        let m1 = model(&speaker, T0);
        let desk = with_nights(PlayActor::Owner, PlayDevice::Personal);
        let m2 = model(&desk, T0);
        let (s0, s1, s2) = (soundtracks_share(&m0), soundtracks_share(&m1), soundtracks_share(&m2));
        assert!((s1 - s0).abs() < 0.05, "the speaker moved the shares: {s0} → {s1}");
        assert!(s2 - s0 > 3.0 * (s1 - s0).abs().max(0.02), "at the computer the same plays would: {s0} → {s2} (speaker {s1})");
        assert_eq!(m1.listening.sessions.len(), m0.listening.sessions.len(), "the user's own sessions");
        assert_eq!((m1.listening.threshold, m1.listening.has_background), (m0.listening.threshold, m0.listening.has_background));
        // «Микс дня» of the computer keeps its makeup.
        let daily = |l: &Lib, m: &UserModel| {
            let mix = with(l, m, DayPart::Evening, |b| b.mix(MixKey::Daily(0), &Kept::default())).expect("«Микс дня»");
            mix.tracks.iter().filter(|&&i| i >= 220).count() as f32 / mix.tracks.len() as f32
        };
        let (d0, d1) = (daily(&base, &m0), daily(&speaker, &m1));
        assert!((d1 - d0).abs() <= 0.15, "soundtracks in «Микс дня»: {d0} → {d1}");
        // The songs the speaker played on by itself are neither counted as
        // played nor on repeat at the computer (the three asked for by
        // voice are).
        let played: Vec<usize> =
            (220..330).filter(|&i| speaker.input.events.iter().any(|p| p.actor == PlayActor::Assistant && p.track == tid(i as u128))).collect();
        assert!(played.len() > 50, "{}", played.len());
        let counted = played.iter().filter(|&&i| m1.taste.plays_at(i) > m0.taste.plays_at(i) || m1.tracks[i].starts > m0.tracks[i].starts).count();
        assert!(counted <= 3, "{counted} counted as played");
        let more: u32 = played.iter().map(|&i| m1.taste.recent_completes_at(i) - m0.taste.recent_completes_at(i)).sum();
        assert!(more <= 3, "{more} more on repeat");
        // Nor do they end the library's cold start sooner than the
        // computer's own plays would.
        assert!(m1.cold() >= 0.5 * m0.cold() && m1.cold() >= m2.cold(), "{} {} {}", m0.cold(), m1.cold(), m2.cold());
    }

    /// A week of habits: quiet Russian songs on weekday mornings (the first
    /// 100), the other 20 of them on Sunday afternoons only; lively
    /// Japanese songs (120–160) on Saturday mornings; loud soundtracks
    /// (160–310) every evening. Eight weeks of it.
    fn week_lib() -> Lib {
        let mut rng = Rng::new(21);
        let sizes = [120usize, 40, 150];
        let energy = [(0.0f32, 0.5f32), (2.5, 3.0), (2.0, 2.5)];
        let (mut tracks, mut types) = (Vec::new(), SongTypes::default());
        let mut index = VectorIndex::new();
        let mut first = 0usize;
        for k in 0..3usize {
            for t in 0..sizes[k] {
                let i = (first + t) as u128;
                let release = ReleaseId(uuid::Uuid::from_u128(5_000 + (first + t / 10) as u128));
                tracks.push(TrackInfo::new(tid(i), vec![aid((k * 100 + t % 20) as u32)], Some(release)).with_world(&format!("week {k} {}", t % 10)).with_duration(Some(180_000)));
                let mut v = [0.0f32; DIM];
                for (d, x) in v.iter_mut().enumerate() {
                    *x = rng.f32() * 0.4 + if d % 3 == k { 2.0 } else { 0.0 };
                }
                let e = energy[k].0 + rng.f32() * (energy[k].1 - energy[k].0);
                for d in crate::mood::ENERGY_FEATURES {
                    v[d] += e;
                }
                index.insert(tid(i), v);
                let ty = match k {
                    0 => SongType { voice: Voice::Sung, lang: Some(Lang::Russian), ..SongType::default() },
                    1 => SongType { voice: Voice::Sung, lang: Some(Lang::Japanese), ..SongType::default() },
                    _ => SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() },
                };
                types.by_track.insert(tid(i), ty);
            }
            first += sizes[k];
        }
        let mut events = Vec::new();
        let mut session = |at: UnixMs, songs: Vec<usize>| {
            let mut ts = at;
            for i in songs {
                events.push(Play { origin: Some(EventOrigin::Manual), ..Play::new(tid(i as u128), ts, EventType::Start, Some(0)) });
                events.push(Play::new(tid(i as u128), ts + 180_000, EventType::Complete, Some(180_000)));
                ts += 181_000;
            }
        };
        for d in 1..=56i64 {
            let day = T0 - d * DAY_MS;
            let n = d as usize;
            match (day.div_euclid(DAY_MS) + 3).rem_euclid(7) {
                0..=4 => session(day + 8 * HOUR_MS, (0..8).map(|j| (n * 8 + j) % 100).collect()),
                5 => session(day + 9 * HOUR_MS + 30 * 60_000, (0..8).map(|j| 120 + (n * 8 + j) % 40).collect()),
                _ => session(day + 14 * HOUR_MS, (0..6).map(|j| 100 + (n * 6 + j) % 20).collect()),
            }
            session(day + 21 * HOUR_MS, (0..8).map(|j| 160 + (n * 8 + j) % 150).collect());
        }
        events.sort_by_key(|p| p.ts);
        Lib { catalog: Catalog::new(tracks), index, types, input: TasteInput { events, ..TasteInput::default() } }
    }

    /// On a weekday morning the mix of the morning is «Доброе утро», right
    /// after «Микс дня», of the quiet morning songs — and of the quiet ones
    /// never heard in the morning before the loud songs of the evening; on
    /// a Saturday morning the lively songs of Saturdays lead it and the
    /// wave's prior. The user's own habits, no rule of what a morning is.
    #[test]
    fn morning_songs_rise_in_the_morning() {
        use crate::open_world::{Scene, WaveContext};
        use crate::wave::{Bandit, WaveSeed};
        let l = week_lib();
        // T0 is a Friday: the Saturday after it at 09:40, the Tuesday after
        // at 08:30.
        let tuesday = T0 + 4 * DAY_MS + 8 * HOUR_MS + 30 * 60_000;
        let saturday = T0 + DAY_MS + 9 * HOUR_MS + 40 * 60_000;
        let (m_tue, m_sat) = (model(&l, tuesday), model(&l, saturday));
        assert_eq!(m_tue.slot().weekday, 1);
        assert_eq!(m_sat.slot().weekday, 5);
        let kind = |i: usize| if i < 120 { 0 } else if i < 160 { 1 } else { 2 };
        let pool_tue = with(&l, &m_tue, DayPart::Morning, |b| b.part_tracks(DayPart::Morning));
        let top: Vec<usize> = pool_tue.iter().copied().take(40).collect();
        assert!(top.iter().filter(|&&i| kind(i) == 0).count() as f32 >= 0.7 * top.len() as f32, "{top:?}");
        // The quiet songs of Sunday afternoons, never heard in the morning,
        // stand before every loud evening song.
        let at = |i: usize| pool_tue.iter().position(|&x| x == i);
        let unheard_quiet: Vec<usize> = (100..120).filter_map(at).collect();
        assert!(!unheard_quiet.is_empty(), "the quiet profile brings its songs");
        let first_loud = (160..310).filter_map(at).min().unwrap_or(usize::MAX);
        assert!(unheard_quiet.iter().all(|&p| p < first_loud), "{unheard_quiet:?} vs {first_loud}");
        // The card: its name, its place, its songs.
        let pinned = with(&l, &m_tue, DayPart::Morning, |b| b.pinned(&Kept::default()));
        let morning = pinned.first().expect("the morning's mix");
        assert_eq!(morning.title, "Доброе утро");
        assert!(morning.tracks.iter().filter(|&&i| kind(i) == 0).count() as f32 >= 0.7 * morning.tracks.len() as f32);
        let r = row(vec![card(MixKey::Daily(0), "Микс дня", 400..420)], Some(morning.clone()), Vec::new());
        assert_eq!(r.iter().map(|m| m.key).collect::<Vec<_>>(), [MixKey::Daily(0), morning.key]);
        // Saturday: the lively songs lead the morning.
        let pool_sat = with(&l, &m_sat, DayPart::Morning, |b| b.part_tracks(DayPart::Morning));
        let lively = |pool: &[usize]| pool.iter().take(40).filter(|&&i| kind(i) == 1).count();
        assert!(lively(&pool_sat) >= 20, "Saturday: {}", lively(&pool_sat));
        assert!(lively(&pool_tue) <= 5, "Tuesday: {}", lively(&pool_tue));
        // …and the wave's prior on Saturday morning.
        let none = |_: &nikgob_core::streaming::StreamTrack| Vec::new();
        let ok = |_: usize| true;
        let bg = |_: usize| false;
        let prior = |m: &UserModel| {
            let ctx = WaveContext { model: m, ok: &ok, genre: None, background: &bg, artist_tracks: &none, index: None, learned: None };
            let key = profile(m, LaneKey::Sung(Lang::Japanese));
            Bandit::arms_of(&ctx, &WaveSeed::plain(0.0)).iter().find(|a| a.scene == Scene::Profile(key)).map_or(0.0, |a| a.mean())
        };
        assert!(prior(&m_sat) > prior(&m_tue) + 0.2, "{} vs {}", prior(&m_sat), prior(&m_tue));
    }