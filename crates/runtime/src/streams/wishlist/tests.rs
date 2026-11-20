    use super::*;
    use super::retry::{down_text_at_in, fetch_down_text, offers_of_json, plan_retry, refusal_at, Retry, same_state, unix_now};
    use nikgob_core::indexer::CandidateHandle;

    /// An empty card searches again once; the recovery ends once the item
    /// is answered, on its way or gone — a later empty card starts a new
    /// one — and only an empty card of the very search again is «не нашёл».
    #[test]
    fn an_empty_card_recovers_once_per_time() {
        use WishlistStatus as St;
        let (a, b) = (WishlistId::new(), WishlistId::new());
        let mut u = Unstuck::default();
        let seen = |list: &[(WishlistId, St, bool)]| list.iter().map(|(i, s, ok)| (*i, (*s, *ok))).collect::<HashMap<_, _>>();
        assert!(u.empty(a), "the first empty card searches again");
        u.seen(&seen(&[(a, St::Searching, false)]));
        assert!(!u.empty(a), "its search came back empty: not found");
        assert!(u.0.is_empty(), "that recovery is over");
        // Recovered: answered with a real card, then empty again much later.
        assert!(u.empty(a));
        u.seen(&seen(&[(a, St::Confirm, true)]));
        assert!(u.0.is_empty());
        assert!(u.empty(a), "a later empty card searches again");
        // On its way, or gone: over too.
        u.seen(&seen(&[(a, St::Downloading, false)]));
        assert!(u.empty(b));
        u.seen(&seen(&[]));
        assert!(u.0.is_empty(), "gone items leave");
    }

    /// An item left on its way with nothing bringing it a file searches
    /// again; one something still moves, or one waiting for an answer of
    /// its own, stays.
    #[test]
    fn an_item_nothing_moves_searches_again() {
        use WishlistStatus as St;
        assert!(stalled(St::Downloading, None, false), "after a restart, its download gone");
        assert!(stalled(St::Confirm, Some(WishNote::FilesWait.code()), false), "its files no longer wait");
        assert!(
            stalled(St::Confirm, Some(nikgob_core::i18n::status::LEGACY_NOTE_FILES_WAIT), false),
            "the words older versions wrote read as the same"
        );
        assert!(!stalled(St::Downloading, None, true), "a torrent, a download or an import still runs");
        assert!(!stalled(St::Confirm, Some(WishNote::FilesWait.code()), true));
        assert!(
            !stalled(St::Confirm, Some(WishNote::NotSure.code()), false),
            "its own card asks"
        );
        for st in [St::New, St::Searching, St::Waiting, St::NoTorrent, St::Done, St::Dismissed] {
            assert!(!stalled(st, None, false), "{st:?}");
        }
    }

    /// «ищу дальше» only for an item that looks again.
    #[test]
    fn wrong_audio_line_promises_a_search_only_when_it_looks() {
        let w = |from, sounds_like, looks| {
            nikgob_core::i18n::status::fmt_wrong_audio_in(Locale::Ru, from, sounds_like, looks)
        };
        assert_eq!(w(Some("Soulseek"), Some("Song"), true), "В файле от Soulseek другая песня — «Song» · ищу дальше");
        assert_eq!(w(None, None, false), "В файле другая песня");
        assert!(!w(Some("RuTracker"), None, false).contains("ищу"));
        assert_eq!(
            nikgob_core::i18n::status::fmt_wrong_audio_in(Locale::En, Some("Soulseek"), Some("Song"), false),
            "Wrong song in the file from Soulseek — “Song”"
        );
    }

    /// One refusal is not yet a pattern; the second sets the source aside,
    /// for at least as long as it asked to wait; a download that works ends it.
    #[test]
    fn a_source_is_set_aside_after_refusing_again() {
        let mut r = Refusals::default();
        let now = Instant::now();
        let e = SourceError::Http { status: 403 };
        assert_eq!(r.refused(SourceKey::Tidal, &e, now), None);
        assert_eq!(r.aside(SourceKey::Tidal, now), None);
        let until = r.refused(SourceKey::Tidal, &e, now).unwrap();
        assert_eq!(until - now, ASIDE_FIRST);
        assert_eq!(r.aside(SourceKey::Tidal, now), Some(until));
        assert_eq!(r.aside(SourceKey::Qobuz, now), None, "one source only");
        let long = SourceError::RateLimited { retry_after: Some(Duration::from_secs(3 * 3600)) };
        assert_eq!(r.refused(SourceKey::Tidal, &long, now).unwrap() - now, Duration::from_secs(3 * 3600));
        assert_eq!(r.why(SourceKey::Tidal), Some(&long));
        assert!(r.worked(SourceKey::Tidal), "it was set aside");
        assert_eq!(r.aside(SourceKey::Tidal, now), None);
        assert!(!r.worked(SourceKey::Tidal));
    }

    const MIN: Duration = Duration::from_secs(60);

    /// The first retry: five minutes, unless every source that said no
    /// asked for longer; then not before the first of them is back.
    #[test]
    fn a_retry_waits_for_the_first_source_back() {
        let r = plan_retry(1, &[(SourceKey::Tidal, None)], &[]);
        assert_eq!(r, Retry { wait: LATER_FIRST, ask: vec![SourceKey::Tidal] });
        // Qobuz is out of its day's requests for 6 h, Tidal set aside 30 min:
        // Tidal's half hour, and only Tidal is named.
        let r = plan_retry(1, &[(SourceKey::Qobuz, Some(360 * MIN)), (SourceKey::Tidal, Some(30 * MIN))], &[]);
        assert_eq!(r, Retry { wait: 30 * MIN, ask: vec![SourceKey::Tidal] });
        // One of them can be asked any time: the backoff, doubling.
        let r = plan_retry(3, &[(SourceKey::Qobuz, Some(360 * MIN)), (SourceKey::Tidal, None)], &[]);
        assert_eq!(r, Retry { wait: 20 * MIN, ask: vec![SourceKey::Tidal] });
        // Never past a day, never more than the longest backoff otherwise.
        assert_eq!(plan_retry(1, &[(SourceKey::Qobuz, Some(72 * 60 * MIN))], &[]).wait, ASIDE_LONGEST);
        assert_eq!(plan_retry(30, &[(SourceKey::Tidal, None)], &[]).wait, LATER_LONGEST);
    }

    /// The sources that had nothing for the item are asked again with the
    /// retry once their answer is six hours old: Soulseek answered 5 h 50
    /// ago and the retry is in 20 min, torrents a minute ago.
    #[test]
    fn a_retry_asks_again_what_had_nothing_long_ago() {
        let others = [(SourceKey::Soulseek, 350 * MIN), (SourceKey::Torrents, MIN)];
        let r = plan_retry(3, &[(SourceKey::Tidal, None)], &others);
        assert_eq!(r.ask, [SourceKey::Tidal, SourceKey::Soulseek]);
    }

    /// The reasons in words a listener understands, the source named.
    #[test]
    fn why_a_source_said_no_in_plain_words() {
        fn w(loc: Locale, key: SourceKey, e: &SourceError, what: What) -> String {
            refusal_at(key, e, what, unix_now()).words_in(loc)
        }
        fn w_at(loc: Locale, key: SourceKey, e: &SourceError, what: What, now: u64) -> String {
            refusal_at(key, e, what, now).words_in(loc)
        }
        let t = What::Track;
        assert_eq!(w(Locale::Ru, SourceKey::Tidal, &SourceError::Http { status: 403 }, t), "Tidal сейчас не отдаёт этот трек");
        assert_eq!(
            w(Locale::Ru, SourceKey::Tidal, &SourceError::Parse("a DASH manifest".into()), t),
            "Tidal сейчас не отдаёт этот трек"
        );
        assert_eq!(w(Locale::Ru, SourceKey::Qobuz, &SourceError::Unreachable("dns error".into()), t), "сервер Qobuz недоступен");
        assert_eq!(w(Locale::Ru, SourceKey::Qobuz, &SourceError::Http { status: 522 }, t), "сервер Qobuz недоступен");
        assert_eq!(w(Locale::Ru, SourceKey::Khinsider, &SourceError::Blocked("a bot check".into()), t), "khinsider закрыт проверкой");
        assert_eq!(w(Locale::Ru, SourceKey::Soulseek, &SourceError::Blocked("login refused".into()), t), "Soulseek не пускает");
        assert_eq!(w(Locale::Ru, SourceKey::Torrents, &SourceError::Unreachable("x".into()), t), "сервер Prowlarr недоступен");
        let short = SourceError::RateLimited { retry_after: Some(MIN) };
        assert_eq!(w(Locale::Ru, SourceKey::Qobuz, &short, t), "Qobuz просит подождать");
        // 01:00 UTC: five hours is a wait it named, 23 hours its day's limit.
        let one_am = 86_400 * 20_000 + 3_600;
        let long = SourceError::RateLimited { retry_after: Some(300 * MIN) };
        assert!(w_at(Locale::Ru, SourceKey::Qobuz, &long, t, one_am).starts_with("Qobuz просит подождать до "));
        let day = SourceError::RateLimited { retry_after: Some(23 * 60 * MIN) };
        assert_eq!(w_at(Locale::Ru, SourceKey::Qobuz, &day, t, one_am), "у Qobuz лимит на сегодня");
        assert_eq!(w(Locale::Ru, SourceKey::Tidal, &SourceError::NothingToFetch, t), "Tidal находит, но не скачивает");
        assert_eq!(
            w(Locale::En, SourceKey::Qobuz, &SourceError::Unreachable("dns".into()), t),
            "the Qobuz server is down"
        );
    }

    /// «Источники» tell each state as it is: a bot check is not a wait;
    /// a day's limit says when it is tried again; a wait or an error a
    /// source sits out for says until when; a source whose search works
    /// but gives no file says so.
    #[test]
    fn a_source_state_in_words() {
        let one_am = 86_400 * 20_000 + 3_600;
        let cf = SourceError::Blocked(nikgob_sources::http::BOT_CHECK.into());
        assert_eq!(down_text_at_in(Locale::Ru, &cf, None, one_am), "Сайт закрыт проверкой — не могу зайти");
        assert_eq!(down_text_at_in(Locale::Ru, &cf, Some(180 * MIN), one_am), "Сайт закрыт проверкой — не могу зайти");
        let login = SourceError::Blocked("the server refused the login: 40".into());
        assert_eq!(down_text_at_in(Locale::Ru, &login, None, one_am), "Не пускает — попробую позже");
        assert!(down_text_at_in(Locale::Ru, &login, Some(180 * MIN), one_am).starts_with("Не пускает — попробую в "));
        let wait = SourceError::RateLimited { retry_after: None };
        let day = down_text_at_in(Locale::Ru, &wait, Some(23 * 60 * MIN), one_am);
        assert!(day.starts_with("Лимит на сегодня — попробую в "), "{day}");
        assert!(down_text_at_in(Locale::Ru, &wait, Some(120 * MIN), one_am).starts_with("Просит подождать — попробую в "));
        assert_eq!(down_text_at_in(Locale::Ru, &wait, Some(MIN), one_am), "Просит подождать — попробую позже", "a short wait is not timed");
        assert_eq!(down_text_at_in(Locale::Ru, &wait, None, one_am), "Просит подождать — попробую позже");
        let bad = SourceError::Http { status: 400 };
        assert_eq!(down_text_at_in(Locale::Ru, &bad, None, one_am), "Отвечает ошибкой 400");
        assert!(down_text_at_in(Locale::Ru, &bad, Some(60 * MIN), one_am).starts_with("Отвечает ошибкой 400 — попробую в "));
        assert_eq!(down_text_at_in(Locale::Ru, &SourceError::NothingToFetch, None, one_am), status_in(Locale::Ru).finds_only);
        assert_eq!(down_text_at_in(Locale::Ru, &SourceError::Unreachable("dns".into()), None, one_am), "Не отвечает");
    }

    /// «пропускаю до 02:07» while the source is set aside; once that time
    /// passed (no time left), the line says only what it does.
    #[test]
    fn the_time_a_source_is_skipped_until_goes_once_it_passed() {
        let e = SourceError::Http { status: 403 };
        let now = fetch_down_text(&e, Some(30 * MIN)).unwrap();
        assert!(now.starts_with("Находит, но не скачивает — пропускаю до "), "{now}");
        assert_eq!(fetch_down_text(&e, None).as_deref(), Some(status_in(Locale::Ru).finds_only));
        assert_eq!(fetch_down_text(&SourceError::Io("disk".into()), None), None);
        // Set aside in «Источники» only while it is.
        let mut r = Refusals::default();
        let t0 = Instant::now();
        r.refused(SourceKey::Tidal, &e, t0);
        let until = r.refused(SourceKey::Tidal, &e, t0).unwrap();
        assert!(r.aside(SourceKey::Tidal, t0).is_some());
        assert_eq!(r.aside(SourceKey::Tidal, until), None, "past its time: told without it");
    }

    /// A state is logged once, not at every search: how long is left does
    /// not make it another.
    #[test]
    fn the_same_state_is_not_logged_again() {
        let a = SourceError::RateLimited { retry_after: Some(MIN) };
        let b = SourceError::RateLimited { retry_after: Some(30 * MIN) };
        assert!(same_state(&a, &b));
        assert!(!same_state(&SourceError::Http { status: 400 }, &SourceError::Http { status: 429 }));
        assert!(!same_state(&a, &SourceError::Blocked("a bot check".into())));
    }

    /// One line says it all: what is here, why, when and whom it asks.
    #[test]
    fn the_line_of_a_wait() {
        let why = vec![
            Refusal::GivesNo { source: SourceKey::Tidal, what: What::Rest },
            Refusal::ServerDown { source: SourceKey::Qobuz },
        ];
        let line = StoredLine::Later {
            have: Some((1, 10)),
            why: why.clone(),
            at: AtWords { hm: None, mins: 30 },
            asked: vec![SourceKey::Tidal, SourceKey::Soulseek],
        };
        assert_eq!(
            line.words_in(Locale::Ru),
            "Есть 1 из 10 · Tidal сейчас не отдаёт остальное, сервер Qobuz недоступен · через 30 мин спрошу Tidal, Soulseek"
        );
        let line = StoredLine::Later {
            have: None,
            why: vec![Refusal::ServerDown { source: SourceKey::Qobuz }],
            at: AtWords { hm: Some("21:40".into()), mins: 5 },
            asked: vec![],
        };
        assert_eq!(
            line.words_in(Locale::Ru),
            "Сервер Qobuz недоступен · попробую снова в 21:40"
        );
        let line = StoredLine::Later {
            have: Some((1, 10)),
            why: vec![Refusal::ServerDown { source: SourceKey::Qobuz }],
            at: AtWords { hm: None, mins: 5 },
            asked: vec![SourceKey::Tidal],
        };
        assert_eq!(
            line.words_in(Locale::En),
            "1 of 10 here · the Qobuz server is down · in 5 min asking Tidal"
        );
    }

    #[test]
    fn offers_saved_before_the_sources_still_read() {
        let old = vec![Candidate {
            id: "1".into(),
            title: "Кино - Группа крови (1988) [FLAC]".into(),
            indexer_name: "RuTracker".into(),
            size_bytes: Some(400 << 20),
            seeders: Some(9),
            leechers: None,
            published: None,
            categories: vec![3040],
            info_url: None,
            info_hash: None,
            handle: CandidateHandle("{}".into()),
        }];
        let offers = offers_of_json(&serde_json::to_string(&old).unwrap());
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].source, SourceKey::Torrents);
        assert_eq!(offers[0].detail(), "FLAC · 400 МБ · 9 сид. · RuTracker");
        assert_eq!(candidate_of(&offers[0]).unwrap(), old[0]);
        let new = offers_of_json(&serde_json::to_string(&offers).unwrap());
        assert_eq!(new, offers);
    }

    /// An offer this build cannot read (a source it no longer has) leaves
    /// out only itself: the card keeps the others to choose from.
    #[test]
    fn an_unreadable_offer_leaves_the_others() {
        let old = Candidate {
            id: "1".into(),
            title: "Gorillaz - Plastic Beach (2010) [FLAC]".into(),
            indexer_name: "RuTracker".into(),
            size_bytes: Some(400 << 20),
            seeders: Some(9),
            leechers: None,
            published: None,
            categories: vec![3040],
            info_url: None,
            info_hash: None,
            handle: CandidateHandle("{}".into()),
        };
        let good = offer_of(old);
        let mut gone = serde_json::to_value(&good).unwrap();
        gone["source"] = serde_json::Value::String("a_source_long_gone".into());
        let json = serde_json::to_string(&vec![gone, serde_json::to_value(&good).unwrap()]).unwrap();
        assert_eq!(offers_of_json(&json), vec![good]);
        assert!(offers_of_json("not json").is_empty());
    }

    /// A cancelled «Скачать» leaves «Хочу скачать»; a like's, an import's
    /// or a better copy's item is turned down; a done one stays done.
    #[test]
    fn a_cancelled_click_leaves_the_wishlist() {
        use WishlistStatus as St;
        let item = |origin, status| WishlistItem { origin, ..WishlistItem::new(status, 1) };
        for st in [St::New, St::Searching, St::Downloading, St::Confirm, St::NoTorrent, St::Waiting] {
            assert!(cancel_drops(&item(WishlistOrigin::Manual, st)), "{st:?}");
            for origin in [WishlistOrigin::Like, WishlistOrigin::Import, WishlistOrigin::Upgrade] {
                assert!(!cancel_drops(&item(origin, st)), "{origin:?} {st:?}");
            }
        }
        assert!(!cancel_drops(&item(WishlistOrigin::Manual, St::Done)));
    }

    /// A better copy of an album asked for by hand is matched file by file:
    /// a torrent that has the album only as a disc image with its cue sheet
    /// is passed over for it (any other item takes it, the importer cuts
    /// it), and an image that came all the same is never shown as a card.
    #[test]
    fn a_hand_album_upgrade_passes_over_a_disc_image() {
        let mut hand = WishlistItem::new(WishlistStatus::Searching, 1);
        hand.origin = WishlistOrigin::Upgrade;
        hand.scope = WishlistScope::Album;
        hand.better_than = Some(500);
        assert!(album_better::hand_album(&hand));
        let mut other = hand.clone();
        other.origin = WishlistOrigin::Manual;
        other.better_than = None;
        let want = Want {
            artists: vec!["Кино".into()],
            album: Some("Группа крови".into()),
            title: None,
            track_no: 0,
            disc_no: 1,
            scope: WantScope::Album,
            ..Want::default()
        };
        let image: Vec<(usize, String, u64)> = vec![
            (0, "Кино - Группа крови/CDImage.flac".into(), 300),
            (1, "Кино - Группа крови/CDImage.cue".into(), 1),
            (2, "Кино - Группа крови/CDImage.log".into(), 1),
        ];
        let picked = pick_files(&image, &want).unwrap();
        assert_eq!(picked, vec![0, 1]);
        let ten = Want { album_tracks: 10, ..want.clone() };
        assert!(!picked_fits(&hand, &ten, &image, &picked));
        assert!(picked_fits(&other, &ten, &image, &picked));
        // Its length unknown: the names cannot tell, the sheets are read
        // once it came.
        assert!(picked_fits(&hand, &want, &image, &picked));
        let tracks: Vec<(usize, String, u64)> = vec![
            (0, "Кино - Группа крови/01 - Группа крови.flac".into(), 30),
            (1, "Кино - Группа крови/02 - Закрой за мной дверь.flac".into(), 30),
            (2, "Кино - Группа крови/Группа крови.cue".into(), 1),
        ];
        let picked = pick_files(&tracks, &want).unwrap();
        assert_eq!(picked, vec![0, 1, 2]);
        let two = Want { album_tracks: 2, ..want.clone() };
        assert!(picked_fits(&hand, &two, &tracks, &picked), "track files with their sheet are taken");
    }
