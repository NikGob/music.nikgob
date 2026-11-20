    use super::*;
    use crate::test_util::tid;

    fn arm(k: u32, alpha: f32, beta: f32) -> Arm {
        Arm { scene: Scene::Profile(k), alpha, beta, tracks: 20 }
    }

    /// Three clusters, the first played most.
    fn bandit() -> Bandit {
        Bandit::with_arms(vec![arm(1, 4.6, 1.4), arm(2, 2.0, 4.0), arm(3, 1.4, 4.6)], 7)
    }

    #[test]
    fn beta_draws_stay_in_range_and_follow_the_mean() {
        let mut rng = Rng::new(3);
        let n = 4000;
        let mut sum = 0.0f32;
        for _ in 0..n {
            let x = beta(&mut rng, 4.0, 1.0);
            assert!((0.0..=1.0).contains(&x));
            sum += x;
        }
        let mean = sum / n as f32;
        assert!((mean - 0.8).abs() < 0.03, "{mean}");
        let small = (0..n).map(|_| beta(&mut rng, 0.5, 0.5)).sum::<f32>() / n as f32;
        assert!((small - 0.5).abs() < 0.04, "{small}");
    }

    /// The prior leads: the cluster played most at this time is chosen
    /// most often, but the others get their turns.
    #[test]
    fn the_prior_leads_the_choice() {
        let mut counts = [0usize; 3];
        for s in 0..400 {
            let mut b = bandit();
            b.seed = s;
            assert!(b.choose(&[], &[]));
            counts[b.current.unwrap()] += 1;
        }
        assert!(counts[0] > counts[1] && counts[1] > counts[2], "{counts:?}");
        assert!(counts[2] > 0, "every cluster has a chance: {counts:?}");
        // The session's cluster wins with its bonus.
        let mut with = 0;
        for s in 0..400 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[0.0, 0.0, 6.0], &[]);
            with += usize::from(b.current == Some(2));
        }
        assert!(with > counts[2] * 3, "{with} vs {}", counts[2]);
    }

    /// Once the mood is found a block is 4–6 songs of one cluster; then
    /// another draw.
    #[test]
    fn blocks_are_four_to_six() {
        for s in 0..50 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let len = b.block_left();
            assert!((BLOCK_MIN..=BLOCK_MAX).contains(&len), "{len}");
            for t in 0..u128::from(len) {
                b.note_served(Item::Track(tid(t)), k, "");
            }
            assert_eq!(b.block_left(), 0);
        }
    }

    /// Songs `from`… of arm `k`, one by each of `artists`, served.
    fn serve(b: &mut Bandit, k: usize, from: u128, artists: &[&str]) -> Vec<Item> {
        artists
            .iter()
            .enumerate()
            .map(|(t, a)| {
                let i = Item::Track(tid(from + t as u128));
                b.note_served(i.clone(), k, a);
                i
            })
            .collect()
    }

    /// The wave's next block, drawn now, is of another cluster than `k`.
    fn has_left(b: &mut Bandit, k: usize) -> bool {
        b.block_left = 0;
        b.choose(&[], &[]);
        b.current != Some(k)
    }

    /// Heard out grows α of its cluster; a skip grows β; two skips stay
    /// in the vibe (its block goes on, playing fewer new songs), whether
    /// the vibe was settled or not.
    #[test]
    fn two_skips_stay_in_the_vibe() {
        for s in 0..40 {
            for settled in [false, true] {
                let mut b = bandit();
                b.seed = s;
                b.choose(&[], &[]);
                let k = b.current.unwrap();
                let (a0, b0) = (b.arms[k].alpha, b.arms[k].beta);
                let items = serve(&mut b, k, 100, &["a", "b", "c"]);
                if settled {
                    b.feedback(&items[0], Verdict::Approved);
                    assert_eq!(b.arms[k].alpha, a0 + 1.0);
                }
                let left_before = b.block_left();
                b.feedback(&items[1], Verdict::Rejected);
                b.feedback(&items[2], Verdict::Rejected);
                assert!((b.arms[k].beta - (b0 + 2.0)).abs() < 1e-4);
                assert_eq!(b.skips_running(), 2);
                assert_eq!(b.block_left(), left_before, "the block goes on");
                assert!(b.settled());
                assert_eq!(b.novelty_of(0.6), 0.3, "with fewer new songs");
                assert!(!has_left(&mut b, k));
            }
        }
    }

    /// One skip never leaves the cluster, however close the others are.
    #[test]
    fn one_skip_never_leaves() {
        for s in 0..100 {
            let mut b = Bandit::with_arms(vec![arm(1, 2.0, 2.0), arm(2, 2.0, 2.0)], s);
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let left_before = b.block_left();
            b.note_served(Item::Track(tid(1)), k, "");
            b.feedback(&Item::Track(tid(1)), Verdict::Rejected);
            assert_eq!(b.arms[k].beta, 3.0);
            assert_eq!(b.block_left(), left_before - 1);
            assert!(b.settled());
            assert!(!has_left(&mut b, k), "the next block is of it too");
        }
    }

    /// A vibe not settled yet (nothing heard out in it) is left at the third
    /// skip running.
    #[test]
    fn three_skips_leave_an_unsettled_vibe() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["a", "a", "a"]);
            b.feedback(&items[0], Verdict::Rejected);
            b.feedback(&items[1], Verdict::Rejected);
            assert!(b.settled());
            b.feedback(&items[2], Verdict::Rejected);
            assert_eq!(b.block_left(), 0, "the block ends");
            assert!(has_left(&mut b, k), "the next block is of another cluster");
        }
    }

    /// A settled vibe (a song of it heard out last) is left at the fourth
    /// skip running, of two artists at least: four of one artist turn from
    /// the artist, a fifth of another leaves.
    #[test]
    fn a_settled_vibe_takes_four_skips_of_two_artists() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "a", "b", "a", "b"]);
            b.feedback(&items[0], Verdict::Approved);
            for i in &items[1..4] {
                b.feedback(i, Verdict::Rejected);
            }
            assert!(b.settled(), "three stay");
            b.feedback(&items[4], Verdict::Rejected);
            assert!(has_left(&mut b, k), "the fourth leaves");

            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "a", "a", "a", "a", "b"]);
            b.feedback(&items[0], Verdict::Approved);
            for i in &items[1..5] {
                b.feedback(i, Verdict::Rejected);
            }
            assert!(b.settled(), "four of one artist stay");
            b.feedback(&items[5], Verdict::Rejected);
            assert!(has_left(&mut b, k), "another artist's leaves");
        }
    }

    /// A song heard out, of any cluster, ends the run of skips.
    #[test]
    fn a_song_heard_out_ends_the_run() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["a", "b", "c", "d"]);
            let other = serve(&mut b, (k + 1) % 3, 200, &["e"]);
            b.feedback(&items[0], Verdict::Rejected);
            b.feedback(&items[1], Verdict::Rejected);
            b.feedback(&other[0], Verdict::Approved);
            assert_eq!(b.skips_running(), 0);
            b.feedback(&items[2], Verdict::Rejected);
            b.feedback(&items[3], Verdict::Rejected);
            assert_eq!(b.skips_running(), 2);
            assert!(!has_left(&mut b, k), "two running, not four");
        }
    }

    /// «Не сейчас» of a loved song: in a settled vibe the song is worn, not
    /// the vibe (nothing counted: no β, no run, no memory); in a vibe not
    /// settled it weighs one and a half, two leave.
    #[test]
    fn not_now_counts_by_the_vibe() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "a", "b", "c", "d", "e"]);
            b.feedback(&items[0], Verdict::Approved);
            let before = b.arms.clone();
            let memory = b.memory.clone();
            for i in &items[1..] {
                b.feedback(i, Verdict::NotNow);
            }
            assert_eq!(b.arms, before);
            assert_eq!(b.memory, memory);
            assert_eq!(b.skips_running(), 0);
            assert!(b.settled());

            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let b0 = b.arms[k].beta;
            let items = serve(&mut b, k, 100, &["a", "b"]);
            b.feedback(&items[0], Verdict::NotNow);
            assert!((b.arms[k].beta - (b0 + NOT_NOW_WEIGHT)).abs() < 1e-4);
            assert!(b.settled());
            b.feedback(&items[1], Verdict::NotNow);
            assert!(has_left(&mut b, k));
        }
    }

    /// A song that did not land (unfamiliar) weighs half a skip: five stay,
    /// the sixth leaves a vibe not settled.
    #[test]
    fn a_song_that_did_not_land_weighs_half() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let b0 = b.arms[k].beta;
            let items = serve(&mut b, k, 100, &["a", "b", "c", "d", "e", "f"]);
            for i in &items[..5] {
                b.feedback(i, Verdict::Missed);
            }
            assert!((b.arms[k].beta - (b0 + 5.0 * MISSED_WEIGHT)).abs() < 1e-4);
            assert!(b.settled(), "five stay");
            b.feedback(&items[5], Verdict::Missed);
            assert!(has_left(&mut b, k), "the sixth leaves");
        }
    }

    /// One mood of an evening does not weigh on the days after: a wave
    /// writes two rejections at most into the memory of a cluster; and what
    /// its rejections added to β fades with each song heard out.
    #[test]
    fn rejections_weigh_little_past_the_mood() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let base = b.arms[k].beta;
        let items = serve(&mut b, k, 100, &["a", "b", "c", "d", "e", "f"]);
        for i in &items {
            b.feedback(i, Verdict::Rejected);
        }
        assert!((b.arms[k].beta - (base + 6.0)).abs() < 1e-4);
        let key = b.memory_key(b.arms[k].scene);
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP));
        let other = serve(&mut b, (k + 1) % 3, 200, &["g", "h"]);
        b.feedback(&other[0], Verdict::Approved);
        assert!((b.arms[k].beta - (base + 6.0 * BETA_KEEP)).abs() < 1e-4);
        b.feedback(&other[1], Verdict::Approved);
        assert!((b.arms[k].beta - (base + 6.0 * BETA_KEEP * BETA_KEEP)).abs() < 1e-4);
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP), "heard out elsewhere");
    }

    /// The memory's cap holds for a cluster through the wave whatever its
    /// arms: left out by a filter and back, it is the same cluster.
    #[test]
    fn the_memory_cap_holds_through_a_filter_and_back() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let scene = b.arms[0].scene;
        let key = b.memory_key(scene);
        for i in serve(&mut b, 0, 100, &["a", "b", "c"]) {
            b.feedback(&i, Verdict::Rejected);
        }
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP));
        // A filter without it, then one with it again (at another place).
        let a = b.arms.remove(0);
        b.arms.push(a);
        let k = b.arms.iter().position(|a| a.scene == scene).unwrap();
        b.run = None;
        for i in serve(&mut b, k, 200, &["d", "e"]) {
            b.feedback(&i, Verdict::Rejected);
        }
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP));
    }

    /// The songs of unknown artists are one artist, «unknown», in a settled
    /// run: a known artist and unknown ones are two (left at the fourth
    /// skip), two known ones are two, and unknown ones alone are one, left
    /// only at the fifth skip; the next block is drawn for real each time.
    #[test]
    fn unknown_artists_are_one_artist() {
        let run = |s: u64, artists: &[&str]| -> Vec<bool> {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let mut all = vec!["x"];
            all.extend_from_slice(artists);
            let items = serve(&mut b, k, 100, &all);
            b.feedback(&items[0], Verdict::Approved);
            items[1..]
                .iter()
                .map(|i| {
                    b.feedback(i, Verdict::Rejected);
                    let mut next = b.clone();
                    has_left(&mut next, k)
                })
                .collect()
        };
        for s in 0..20 {
            assert_eq!(run(s, &["a", "", "", ""]), [false, false, false, true], "a known artist and unknown ones");
            assert_eq!(run(s, &["", "", "", "a"]), [false, false, false, true], "unknown ones, then a known artist");
            assert_eq!(run(s, &["a", "b", "a", "b"]), [false, false, false, true], "two known artists");
            assert_eq!(run(s, &["", "", "", "", ""]), [false, false, false, false, true], "unknown ones alone");
            assert_eq!(run(s, &["a", "a", "a", "a", "a"]), [false; 5], "one known artist alone");
        }
    }

    /// After a skip, the radio's next song queued stays only when it is of
    /// the cluster held (a block of another picked ahead is replaced); two
    /// skips running replace it whatever it is.
    #[test]
    fn the_next_song_stays_only_of_the_cluster_held() {
        for s in 1..30 {
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            assert!(b.choose(&[], &[0, 2]));
            b.note_served(Item::Track(tid(200)), 1, "");
            let ahead = Item::Track(tid(200));
            assert!(b.keep_next(Some(&ahead)), "nothing held");
            b.feedback(&a[a.len() - 1], Verdict::Rejected);
            assert!(!b.keep_next(Some(&ahead)), "of another cluster: replaced");
            assert!(b.keep_next(Some(&a[0])), "of the cluster held: kept");
            assert!(b.keep_next(None), "the user's own: kept");
            b.feedback(&a[a.len() - 2], Verdict::Rejected);
            assert!(!b.keep_next(Some(&a[0])), "two running: the whole tail");
        }
    }

    /// Where the wave plays new music alone a song that did not land is a
    /// whole rejection: three leave a vibe not settled.
    #[test]
    fn of_new_music_alone_a_miss_is_a_rejection() {
        let mut b = bandit();
        b.only_new = true;
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let items = serve(&mut b, k, 100, &["a", "b", "c"]);
        for i in &items {
            b.feedback(i, Verdict::Missed);
        }
        assert!(has_left(&mut b, k));
    }

    /// A like approves at once and once: it ends the run and settles the
    /// vibe, and the song's end, skipped or heard out, adds nothing.
    #[test]
    fn a_like_approves_at_once_and_once() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let items = serve(&mut b, k, 100, &["a", "b", "c", "d"]);
        b.feedback(&items[0], Verdict::Rejected);
        b.feedback(&items[1], Verdict::Rejected);
        let a0 = b.arms[k].alpha;
        b.liked(&items[2]);
        assert_eq!(b.arms[k].alpha, a0 + 1.0);
        assert_eq!(b.skips_running(), 0);
        assert_eq!(b.heard_in, Some(k));
        let after = b.clone();
        b.feedback(&items[2], Verdict::Rejected);
        assert_eq!(b, after, "liked, then skipped: nothing more");
        b.feedback(&items[2], Verdict::Approved);
        b.liked(&items[2]);
        assert_eq!(b, after, "nor heard out, nor liked again");
        // Settled: «not now» counts nothing, a skip one.
        b.feedback(&items[3], Verdict::NotNow);
        assert_eq!(b.skips_running(), 0);
    }

    /// What came while picks were on their way is told to their state in
    /// the order it came: a like after two skips ends their run there too.
    #[test]
    fn a_like_during_an_extension_keeps_its_order() {
        let mut live = bandit();
        live.choose(&[], &[]);
        let k = live.current.unwrap();
        let items = serve(&mut live, k, 100, &["a", "b", "c"]);
        let mut picked = live.clone();
        let mut late = live.clone();
        live.feedback(&items[0], Verdict::Rejected);
        live.feedback(&items[1], Verdict::Rejected);
        live.liked(&items[2]);
        live.feedback(&items[2], Verdict::Approved);
        // The marks as they came, on the state the picks came with.
        picked.feedback(&items[0], Verdict::Rejected);
        picked.feedback(&items[1], Verdict::Rejected);
        picked.liked(&items[2]);
        picked.feedback(&items[2], Verdict::Approved);
        assert_eq!(picked, live);
        assert_eq!(picked.skips_running(), 0);
        assert!(picked.held().is_none());
        // The ends first and the likes after: the like would count twice.
        late.feedback(&items[0], Verdict::Rejected);
        late.feedback(&items[1], Verdict::Rejected);
        late.feedback(&items[2], Verdict::Approved);
        late.liked(&items[2]);
        assert!(late.arms[k].alpha > live.arms[k].alpha);
    }

    /// A song heard past its half ends the run, of its cluster or another:
    /// skip, skip, past the half, skip does not leave; nothing else moves.
    #[test]
    fn a_song_heard_past_its_half_ends_the_run() {
        for other in [false, true] {
            let mut b = bandit();
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["a", "b", "c", "d"]);
            let half = if other { serve(&mut b, (k + 1) % 3, 200, &["e"]).remove(0) } else { items[2].clone() };
            b.feedback(&items[0], Verdict::Rejected);
            b.feedback(&items[1], Verdict::Rejected);
            let (arms, memory) = (b.arms.clone(), b.memory.clone());
            b.feedback(&half, Verdict::Neutral);
            assert_eq!((b.arms.clone(), b.memory.clone()), (arms, memory));
            assert_eq!(b.skips_running(), 0);
            assert_eq!(b.held(), None);
            b.feedback(&items[3], Verdict::Rejected);
            assert_eq!(b.skips_running(), 1);
            assert!(b.settled(), "not three running");
        }
    }

    /// «Не сейчас» of a loved song in a settled vibe, after a skip or not:
    /// nothing counted and nothing held, but the next song stays in the
    /// cluster, once: the next pick, or a song queued ahead of it, of
    /// another cluster gives way; after a song of it is picked, or the next
    /// ends, the draws are free again.
    #[test]
    fn a_worn_song_keeps_the_next_song_once() {
        for skip_first in [false, true] {
            let mut b = bandit();
            let a = played_block(&mut b, 0, 100);
            b.feedback(&a[0], Verdict::Approved);
            if skip_first {
                b.feedback(&a[1], Verdict::Rejected);
                assert_eq!(b.held(), Some(0));
            }
            // The next block, of another cluster, picked ahead.
            assert!(b.choose(&[], &[0, 2]));
            b.note_served(Item::Track(tid(200)), 1, "");
            let ahead = Item::Track(tid(200));
            let before = (b.arms.clone(), b.memory.clone());
            b.feedback(a.last().unwrap(), Verdict::NotNow);
            assert_eq!((b.arms.clone(), b.memory.clone()), before, "nothing counted");
            assert_eq!(b.held(), None, "nothing held");
            assert!(!b.keep_next(Some(&ahead)), "the song ahead of another cluster gives way");
            assert!(b.keep_next(Some(&a[2])));
            // The tail picked again: the next pick is of the cluster.
            b.forget(std::slice::from_ref(&ahead));
            assert_eq!(b.kept(), Some(0));
            assert!(b.choose(&[], &[]));
            assert_eq!(b.current, Some(0));
            b.note_served(Item::Track(tid(300)), 0, "");
            assert_eq!(b.kept(), None, "once");
        }
        // The next song ends before any pick: free again too.
        let mut b = bandit();
        let a = played_block(&mut b, 0, 100);
        b.feedback(&a[0], Verdict::Approved);
        b.feedback(&a[1], Verdict::NotNow);
        assert_eq!(b.kept(), Some(0));
        b.feedback(&a[2], Verdict::Neutral);
        assert_eq!(b.kept(), None);
    }

    /// «↻ Другой вайб»: another cluster at once, and nothing counted
    /// against the cluster or the song playing; so after a skip or two too.
    #[test]
    fn another_vibe_counts_no_skip() {
        for s in 0..40 {
            for skips in 0..3 {
                let mut b = bandit();
                b.seed = s;
                b.choose(&[], &[]);
                let k = b.current.unwrap();
                let items = serve(&mut b, k, 100, &["a", "b", "c"]);
                for i in &items[..skips] {
                    b.feedback(i, Verdict::Rejected);
                }
                let (before, memory) = (b.arms.clone(), b.memory.clone());
                assert!(b.other_vibe(items.last()));
                assert_ne!(b.current, Some(k));
                assert_eq!(b.arms, before, "no α or β moved");
                assert_eq!(b.memory, memory, "nothing remembered");
                assert!(b.settled());
                assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()));
            }
        }
        let mut one = Bandit::with_arms(vec![arm(1, 2.0, 2.0)], 1);
        one.choose(&[], &[]);
        assert!(!one.other_vibe(None), "nowhere else to go");
    }

    /// A full block of `k` picked: the queue's buffer then picks ahead.
    fn played_block(b: &mut Bandit, k: usize, from: u128) -> Vec<Item> {
        b.avoid = None;
        let others: Vec<usize> = (0..b.arms.len()).filter(|&j| j != k).collect();
        assert!(b.choose(&[], &others));
        let items: Vec<Item> = (0..u128::from(b.block_left())).map(|t| Item::Track(tid(from + t))).collect();
        for i in &items {
            b.note_served(i.clone(), k, "");
        }
        items
    }

    /// Three skips of the songs playing leave their cluster though the
    /// next block was picked ahead already: of another cluster, it goes on
    /// and the one after avoids theirs; of the same, it ends.
    #[test]
    fn skips_leave_the_playing_cluster_past_a_block_boundary() {
        for s in 1..30 {
            // The next block of another cluster.
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            // One song of the next block (of the second cluster) queued.
            assert!(b.choose(&[], &[0, 2]));
            b.note_served(Item::Track(tid(200)), 1, "");
            let left = b.block_left();
            for i in &a[a.len() - 3..] {
                b.feedback(i, Verdict::Rejected);
            }
            assert_eq!(b.block_left(), left, "the block ahead goes on");
            b.block_left = 0;
            b.choose(&[], &[]);
            assert_ne!(b.current, Some(0), "the next block leaves the skipped cluster");

            // The next block of the same cluster again.
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            played_block(&mut b, 0, 200);
            for i in &a[a.len() - 3..] {
                b.feedback(i, Verdict::Rejected);
            }
            assert_eq!(b.block_left(), 0, "the block ahead of the skipped cluster ends");
            b.choose(&[], &[]);
            assert_ne!(b.current, Some(0));
        }
    }

    /// «↻ Другой вайб» leaves the cluster of the song playing, not that of
    /// the block picked ahead of it (nor that one, with three clusters).
    #[test]
    fn another_vibe_leaves_the_playing_cluster() {
        for s in 1..40 {
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            played_block(&mut b, 1, 200);
            let before = b.arms.clone();
            assert!(b.other_vibe(a.last()));
            assert_eq!(b.current, Some(2), "neither the playing nor the ahead");
            assert_eq!(b.arms, before);
            // Two clusters: the one ahead is the other.
            let mut two = Bandit::with_arms(vec![arm(1, 4.0, 1.0), arm(2, 1.0, 4.0)], s);
            let a = played_block(&mut two, 0, 100);
            played_block(&mut two, 1, 200);
            assert!(two.other_vibe(a.last()));
            assert_eq!(two.current, Some(1));
        }
    }

    /// What plays again: each song by its last pick, past the queue's gap
    /// (longer for new music), heard least lately first.
    #[test]
    fn repeatable_is_past_the_gap_oldest_first() {
        let mut served: Vec<(String, bool)> = (0..10).map(|k| (format!("t{k}"), false)).collect();
        served.push(("t0".into(), false));
        served.push(("n0".into(), true));
        let again = repeatable(&served);
        // 12 served: the last 6 (t6…t9, t0, n0) wait; t0 was picked again.
        assert_eq!(again, vec!["t1", "t2", "t3", "t4", "t5"]);
        let mut long: Vec<(String, bool)> = vec![("n0".into(), true)];
        long.extend((0..REPEAT_GAP_NEW).map(|k| (format!("t{k}"), false)));
        assert_eq!(repeatable(&long)[0], "n0");
    }

    /// Picks dropped unplayed give the block its places back; a removed
    /// pick grows its cluster's β, and runs no skips.
    #[test]
    fn forgotten_picks_return_to_the_block() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let left = b.block_left();
        b.note_served(Item::Track(tid(1)), k, "");
        b.note_served(Item::Track(tid(2)), k, "");
        b.forget(&[Item::Track(tid(2))]);
        assert_eq!(b.block_left(), left - 1);
        let beta0 = b.arms[k].beta;
        b.removed(&Item::Track(tid(1)));
        assert_eq!(b.arms[k].beta, beta0 + 1.0);
        assert_eq!(b.skips_running(), 0);
        // A song the bandit never served says nothing.
        b.feedback(&Item::Track(tid(9)), Verdict::Rejected);
        assert_eq!(b.arms[k].beta, beta0 + 1.0);
    }

    /// A card of the row is its wave.
    #[test]
    fn cards_are_presets() {
        use crate::mixes::MixKey as K;
        assert_eq!(preset_of(K::Calm, 0.3).unwrap().filter.character, Character::Calm);
        assert_eq!(preset_of(K::Genre(3), 0.3).unwrap().filter.scene, Scene::Genre(3));
        assert_eq!(preset_of(K::Profile(9), 0.6).unwrap().filter, WaveFilter { novelty: 0.6, character: Character::Any, scene: Scene::Profile(9) });
        let bg = preset_of(K::Background { profile: 4, card: 0, cards: 2 }, 0.0).unwrap();
        assert_eq!((bg.filter.character, bg.filter.scene), (Character::Background, Scene::Profile(4)));
        assert_eq!(preset_of(K::Blend([1, 2, 0]), 0.3).unwrap().only, vec![1, 2]);
        assert_eq!(preset_of(K::Part(3), 0.3).unwrap().part, Some(DayPart::Night));
        assert_eq!(preset_of(K::Daily(0), 0.3).unwrap(), WaveSeed::plain(0.3));
    }

    /// The bandit goes through serde with the queue.
    #[test]
    fn the_bandit_is_saved() {
        let mut b = bandit();
        b.choose(&[], &[]);
        b.note_served(Item::Track(tid(1)), 0, "");
        let json = serde_json::to_string(&b).unwrap();
        let back: Bandit = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    /// A wave starts with full blocks (its prior and its session say the
    /// mood); a block left after its rejections loses the mood: blocks of
    /// one or two songs, going on while heard out, until two songs running
    /// are heard out.
    #[test]
    fn short_blocks_until_the_mood_is_found_again() {
        let mut b = bandit();
        assert!(b.choose(&[], &[]));
        assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()), "{}", b.block_left());
        let k = b.current.unwrap();
        let bad: Vec<Item> = (0..3).map(|t| Item::Track(tid(400 + t))).collect();
        for i in &bad {
            b.note_served(i.clone(), k, "");
        }
        for i in &bad {
            b.feedback(i, Verdict::Rejected);
        }
        assert!(!b.settled());
        b.choose(&[], &[]);
        assert!((1..=PROBE_MAX).contains(&b.block_left()));
        let k = b.current.unwrap();
        let items: Vec<Item> = (0..2).map(|t| Item::Track(tid(300 + t))).collect();
        for i in &items {
            b.note_served(i.clone(), k, "");
        }
        b.feedback(&items[0], Verdict::Approved);
        assert!(!b.settled());
        assert!(b.block_left() >= 1, "a cluster heard out keeps its block going");
        b.feedback(&items[1], Verdict::Approved);
        assert!(b.settled());
        assert!(b.block_left() >= BLOCK_MIN, "found, the block grows to a full one");
        b.block_left = 0;
        b.choose(&[], &[]);
        assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()));
    }

    /// Two new songs rejected in a block halve the next block's share of
    /// new music (not «Только новое»); the block after is whole again; a
    /// library song rejected counts nothing towards it.
    #[test]
    fn rejected_new_songs_halve_the_next_block() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let new = |n: &str| Item::Stream(RemoteId::new(nikgob_core::streaming::Service::Youtube, n));
        let items = [new("a"), new("b"), Item::Track(tid(9))];
        for i in &items {
            b.note_served(i.clone(), k, "");
        }
        b.feedback(&items[2], Verdict::Rejected);
        b.feedback(&items[0], Verdict::Rejected);
        b.block_left = 0;
        b.choose(&[], &[]);
        assert_eq!(b.novelty_of(0.6), 0.6, "one new song rejected: whole");
        let k = b.current.unwrap();
        let more = [new("c"), new("d")];
        for i in &more {
            b.note_served(i.clone(), k, "");
        }
        for i in &more {
            b.feedback(i, Verdict::Rejected);
        }
        b.block_left = 0;
        b.choose(&[], &[]);
        assert_eq!(b.novelty_of(0.6), 0.3);
        assert_eq!(b.novelty_of(NOVELTY_ONLY_NEW), NOVELTY_ONLY_NEW);
        b.block_left = 0;
        b.choose(&[], &[]);
        assert_eq!(b.novelty_of(0.6), 0.6, "the block after is whole again");
    }

    /// The memory keeps what each cluster did in each part of the day,
    /// fades by the day, weighs a few songs at most, and keeps its size.
    #[test]
    fn the_memory_fades_and_stays_small() {
        let mut m = BanditMemory::default();
        for _ in 0..4 {
            m.learn("any.p1", 2, 100, true);
        }
        m.learn("any.p1", 2, 100, false);
        assert_eq!(m.prior("any.p1", 2, 100), (4.0, 1.0));
        assert_eq!(m.prior("any.p1", 1, 100), (0.0, 0.0), "another part of the day");
        let (ok, no) = m.prior("any.p1", 2, 102);
        assert!((ok - 4.0 * MEMORY_DECAY * MEMORY_DECAY).abs() < 1e-4 && (no - MEMORY_DECAY * MEMORY_DECAY).abs() < 1e-4);
        for _ in 0..40 {
            m.learn("any.p2", 0, 100, true);
        }
        let (ok, no) = m.prior("any.p2", 0, 100);
        assert!((ok + no - MEMORY_WEIGHT).abs() < 1e-4);
        for k in 0..(MEMORY_CAP + 20) {
            m.learn(&format!("any.p{k}"), 3, 200 + k as i64, true);
        }
        assert_eq!(m.entries.len(), MEMORY_CAP);
        let back: BanditMemory = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    /// In a room any wave is ordered for a steady energy (no loud cold
    /// head after a quiet tail); at a computer only a calm or a background
    /// one is, and a lively one nowhere.
    #[test]
    fn a_room_keeps_any_wave_steady() {
        use crate::context::PlayDevice::{Ambient, Personal};
        assert!(kept_steady(Character::Any, Ambient));
        assert!(!kept_steady(Character::Any, Personal));
        for c in [Character::Calm, Character::Background] {
            assert!(kept_steady(c, Personal) && kept_steady(c, Ambient), "{c:?}");
        }
        for c in [Character::Lively, Character::Dark] {
            assert!(!kept_steady(c, Ambient), "{c:?}");
        }
        // The step a steady order avoids: a loud cold head after a quiet tail.
        use crate::sketch::{Edge, EdgeShape};
        let edge = |loudness: f32, shape: EdgeShape| Edge { loudness, shape, silence: 0.0, chroma: [15, 1, 8, 1, 10, 9, 1, 13, 1, 8, 1, 3] };
        assert!(edge_cost(&edge(-24.0, EdgeShape::Fade), &edge(-13.0, EdgeShape::Cold)) > edge_cost(&edge(-24.0, EdgeShape::Fade), &edge(-24.0, EdgeShape::Fade)));
    }

    /// Edges: a key on the far side of the circle of fifths costs more
    /// than its neighbour or its relative minor; a loud cold head right
    /// after a quiet tail costs, a fade-in does not.
    #[test]
    fn steps_by_their_edges() {
        use crate::sketch::{Edge, EdgeShape};
        // C major's pitch classes, and rotated to other keys.
        let c: [u8; 12] = [15, 1, 8, 1, 10, 9, 1, 13, 1, 8, 1, 3];
        let key = |shift: usize, loudness: f32, shape: EdgeShape| Edge { loudness, shape, silence: 0.0, chroma: std::array::from_fn(|i| c[(i + 12 - shift) % 12]) };
        let tail = key(0, -12.0, EdgeShape::Fade);
        let same = edge_cost(&tail, &key(0, -12.0, EdgeShape::Cold));
        let fifth = edge_cost(&tail, &key(7, -12.0, EdgeShape::Cold));
        let tritone = edge_cost(&tail, &key(6, -12.0, EdgeShape::Cold));
        assert!(same < 1e-6, "{same}");
        assert!(fifth < tritone, "{fifth} {tritone}");
        let attack = edge_cost(&key(0, -24.0, EdgeShape::Fade), &key(0, -8.0, EdgeShape::Cold));
        let eased = edge_cost(&key(0, -24.0, EdgeShape::Fade), &key(0, -8.0, EdgeShape::Fade));
        assert!(attack > eased + 0.3, "{attack} {eased}");
        assert_eq!(fifths(crate::sketch::Key { root: 9, minor: true }), fifths(crate::sketch::Key { root: 0, minor: false }), "A minor is C major's relative");
    }

    /// The permutations of a batch are all weighed, the first one first.
    #[test]
    fn every_order_of_a_batch() {
        let mut p = vec![0, 1, 2];
        let mut seen = vec![p.clone()];
        while next_permutation(&mut p) {
            seen.push(p.clone());
        }
        assert_eq!(seen.len(), 6);
        assert_eq!(seen[5], vec![2, 1, 0]);
    }

    /// The bandit at a cold start (a report): six clusters, one the mood
    /// of the moment (heard out 4 in 5), the rest 1 in 4, the prior saying
    /// nothing; how many songs until three are heard out running: blocks of
    /// 4 to 6 throughout; short blocks once the mood is lost (as it plays);
    /// short blocks from the start too; as it plays with the memory of the
    /// mood from the day before.
    #[test]
    #[ignore = "a report"]
    fn cold_start_report() {
        let runs = 400u64;
        let mut totals = [0.0f32; 4];
        for (policy, total) in totals.iter_mut().enumerate() {
            for run in 0..runs {
                let arms: Vec<Arm> = (0..6).map(|k| arm(k, 2.0, 2.0)).collect();
                let mut b = Bandit::with_arms(arms, 1 + run);
                b.lost = policy == 2;
                if policy == 3 {
                    // The day before heard the mood out (the memory's songs
                    // on its prior, at most MEMORY_WEIGHT).
                    b.arms[3].alpha += 4.0;
                }
                let mut rng = Rng::new(9_000 + run);
                let (mut steps, mut run_ok, mut t) = (0u32, 0u32, 0u128);
                while run_ok < 3 && steps < 200 {
                    if policy == 0 {
                        b.lost = false;
                    }
                    if b.block_left() == 0 || b.current.is_none() {
                        b.choose(&[], &[]);
                    }
                    let k = b.current.unwrap();
                    let item = Item::Track(tid(10_000 + t));
                    t += 1;
                    b.note_served(item.clone(), k, "");
                    let p = if k == 3 { 0.8 } else { 0.25 };
                    let ok = rng.f32() < p;
                    b.feedback(&item, if ok { Verdict::Approved } else { Verdict::Rejected });
                    run_ok = if ok { run_ok + 1 } else { 0 };
                    steps += 1;
                }
                *total += steps as f32;
            }
        }
        let mean = |x: f32| x / runs as f32;
        eprintln!(
            "songs to three heard out running at a cold start: full blocks {:.1}, short once lost {:.1}, short from the start {:.1}, short once lost with memory {:.1}",
            mean(totals[0]),
            mean(totals[1]),
            mean(totals[2]),
            mean(totals[3])
        );
        assert!(totals[1] <= totals[0]);
    }
