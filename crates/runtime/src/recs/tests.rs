use super::home::distinct_leads;
use super::worker::{pick_tracks, PickTrack};

    use super::*;

    /// A home mix of library tracks and songs of the services (the
    /// daylist's new ones) gives both, in its order, for its download and
    /// its queue; a song without its details is left out.
    #[test]
    fn a_mixed_home_mix_gives_its_songs_of_the_services_too() {
        use nikgob_core::streaming::Service;
        let (a, b) = (TrackId::new(), TrackId::new());
        let new = StreamTrack::new(RemoteId::new(Service::Youtube, "new"), "New");
        let gone = RemoteId::new(Service::Youtube, "gone");
        let pick = |item| Pick { item, reason: Reason::Random };
        let picks = vec![pick(Item::Track(a)), pick(Item::Stream(new.id.clone())), pick(Item::Stream(gone)), pick(Item::Track(b))];
        let streams = HashMap::from([(new.id.clone(), new.clone())]);
        assert_eq!(pick_tracks(&picks, &streams), vec![PickTrack::Library(a), PickTrack::Stream(&new), PickTrack::Library(b)]);
        assert!(pick_tracks(&[], &streams).is_empty());
    }

    /// An off vocal and a TV size of a song stand for the plain one; a
    /// remix, another artist's song of the same title and a lone version
    /// stand for themselves.
    #[test]
    fn versions_of_a_work_stand_for_the_plain_one() {
        let (a, b) = (ArtistId::new(), ArtistId::new());
        let ids: Vec<TrackId> = (0..7).map(|_| TrackId::new()).collect();
        let rows = [
            (ids[0], "Song", Some(a)),
            (ids[1], "Song (Off Vocal)", Some(a)),
            (ids[2], "Song (TV Size)", Some(a)),
            (ids[3], "Song (Remix)", Some(a)),
            (ids[4], "Song (Instrumental)", Some(b)),
            (ids[5], "Other (Karaoke)", Some(a)),
            (ids[6], "Another (Instrumental)", Some(a)),
        ];
        let heads = work_heads(rows.iter().map(|r| (r.0, r.1, r.2)));
        assert_eq!(heads.get(&ids[1]), Some(&ids[0]));
        assert_eq!(heads.get(&ids[2]), Some(&ids[0]));
        assert_eq!(heads.len(), 2, "{heads:?}");
    }

    /// A changed model vector changes the song types' fingerprint even at
    /// the same scale, and so does having one at all.
    #[test]
    fn every_coordinate_of_a_vector_changes_the_fingerprint() {
        let mut a = nikgob_recs::Compact { scale: 0.01, q: [3; nikgob_recs::embedding::COMPACT_DIM] };
        let h = embedding_fingerprint(7, Some(&a));
        assert_eq!(h, embedding_fingerprint(7, Some(&a)));
        a.q[200] = 4;
        assert_ne!(h, embedding_fingerprint(7, Some(&a)), "same scale, another vector");
        assert_ne!(embedding_fingerprint(7, None), embedding_fingerprint(7, Some(&a)));
    }

    /// A copy of an off vocal on a second album, the off vocal and the plain
    /// song are one song; when the work's head has a copy standing for its
    /// song, the whole work takes that copy.
    #[test]
    fn song_copies_and_work_versions_compose() {
        let mut ids: Vec<TrackId> = (0..6).map(|_| TrackId::new()).collect();
        ids.sort();
        let (plain, off1, off2) = (ids[3], ids[1], ids[2]);
        // off2 is off1 on another album; both are versions of plain.
        let works = HashMap::from([(off1, plain), (off2, plain)]);
        let keys = compose_songs([(plain, plain), (off1, off1), (off2, off1)].into_iter(), &works);
        assert_eq!(keys.get(&plain), Some(&plain));
        assert_eq!(keys.get(&off1), Some(&plain));
        assert_eq!(keys.get(&off2), Some(&plain), "{keys:?}");
        // The head plain2 is a copy of plain1 (the one with the file).
        let (plain1, plain2, off) = (ids[5], ids[0], ids[4]);
        let works = HashMap::from([(off, plain2)]);
        let keys = compose_songs([(plain1, plain1), (plain2, plain1), (off, off)].into_iter(), &works);
        assert!([plain1, plain2, off].iter().all(|t| keys.get(t) == Some(&plain1)), "{keys:?}");
        // Nothing joined: each its own.
        let alone = compose_songs([(ids[0], ids[0])].into_iter(), &HashMap::new());
        assert_eq!(alone.get(&ids[0]), Some(&ids[0]));
    }

    /// An artist no tag tells gets the family their songs' styles lead
    /// with, as the tagged artists' songs taught it; a tagged artist of no
    /// family is left to the tags; nothing taught, nothing told.
    #[test]
    fn untagged_artists_take_the_family_of_their_styles() {
        let (a, b, c) = (ArtistId::new(), ArtistId::new(), ArtistId::new());
        let tracks: Vec<TrackId> = (0..4).map(|_| TrackId::new()).collect();
        let catalog = Catalog::new([
            TrackInfo::new(tracks[0], vec![a], None),
            TrackInfo::new(tracks[1], vec![a], None),
            TrackInfo::new(tracks[2], vec![b], None),
            TrackInfo::new(tracks[3], vec![c], None),
        ]);
        let ambient = nikgob_recs::styles::STYLES.iter().position(|l| *l == "Electronic---Ambient").unwrap() as u16;
        let styles = nikgob_recs::Styles { top: [(ambient, 0.8), (u16::MAX, 0.0), (u16::MAX, 0.0), (u16::MAX, 0.0)], sure: 0.8 };
        let mut index = VectorIndex::new();
        for (k, t) in tracks.iter().enumerate() {
            index.insert(*t, [k as f32; nikgob_recs::DIM]);
            index.set_sketch(*t, nikgob_recs::SoundSketch::default().with_styles(Some(&styles)));
        }
        // c's tags make it of family 7: its song teaches that the style is.
        let families = HashMap::from([(c, 7u8)]);
        let fams = style_families(&catalog, &index, &|x| x == b || x == c, &families);
        assert_eq!(fams.get(&a).copied(), Some(7));
        assert!(!fams.contains_key(&b) && !fams.contains_key(&c));
        assert!(style_families(&catalog, &index, &|x| x == b, &HashMap::new()).is_empty());
    }

    /// «Твой микс» and the daylist kept for the day are picked again once
    /// a later pass found new music (not under «Только скачанное», where
    /// they take none).
    #[test]
    fn a_later_pass_redraws_the_days_picks() {
        assert!(new_kept(500, 500, false));
        assert!(new_kept(600, 500, false));
        assert!(!new_kept(0, 500, false), "kept before any pass");
        assert!(!new_kept(400, 500, false));
        assert!(new_kept(0, 500, true));
        assert!(new_kept(0, 0, false), "no pass yet");
    }

    /// The page of a mix lists the new songs «Слушать» plays, in their
    /// order, and only those that find a place between loved ones.
    #[test]
    fn a_mix_page_lists_its_new_songs() {
        use nikgob_core::streaming::Service;
        let new: Vec<StreamTrack> = (0..3).map(|k| StreamTrack::new(RemoteId::new(Service::Youtube, format!("v{k}")), format!("New {k}"))).collect();
        let lane = lanes::LaneHome {
            key: v::MixKey::Calm,
            title: "Спокойное".into(),
            short: Vec::new(),
            labels: lanes::PerLocale::default(),
            side_flavor: None,
            tracks: (0..12).map(|_| TrackId::new()).collect(),
            // Two gaps between loved ones only: two of the three play.
            anchors: (0..12).map(|k| !matches!(k, 0 | 1 | 4 | 5 | 6 | 7 | 10 | 11)).collect(),
            artists: Vec::new(),
            covers: Vec::new(),
            new,
            open: false,
            niche: Vec::new(),
            blend_base: None,
            why: None,
        };
        let home = RecHome {
            day: String::new(),
            epoch: String::new(),
            daily: Vec::new(),
            daily_streams: HashMap::new(),
            daily_covers: Vec::new(),
            history: true,
            moods: Vec::new(),
            styles: Vec::new(),
            daylist_key: String::new(),
            daylist: None,
            tiles: Vec::new(),
            lanes: vec![lane],
            blend_profiles: Vec::new(),
            blend_profile_titles: Vec::new(),
            top: Vec::new(),
        };
        let parts = HomeParts::of_in(Locale::Ru, Some(&home), Vec::new(), Vec::new(), false, false);
        let played: Vec<RemoteId> = home.lanes[0]
            .picks(true)
            .into_iter()
            .filter_map(|p| match p.item {
                Item::Stream(r) => Some(r),
                Item::Track(_) => None,
            })
            .collect();
        assert_eq!(played.len(), 2, "{played:?}");
        let rows: Vec<RemoteId> = parts.lanes[0].new_rows.iter().map(|t| t.id.clone()).collect();
        assert_eq!(rows, played);
    }

    /// Cards of one row lead with different pictures while they have them.
    #[test]
    fn the_cards_of_a_row_lead_with_different_covers() {
        let mut covers = vec![vec![1, 2, 3], vec![1, 2, 3], vec![2, 1], vec![1], Vec::new(), vec![3, 4]];
        distinct_leads(&mut covers);
        assert_eq!(covers, [vec![1, 2, 3], vec![2, 1, 3], vec![2, 1], vec![1], vec![], vec![3, 4]]);
    }


    #[test]
    fn style_titles_name_their_artists() {
        let n = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(style_title(&n(&["mzlff", "MORGENSHTERN"]), 7, Locale::Ru), "mzlff, MORGENSHTERN и другие");
        assert_eq!(style_title(&n(&["mzlff", "MORGENSHTERN"]), 7, Locale::En), "mzlff, MORGENSHTERN & more");
        assert_eq!(style_title(&n(&["A", "B"]), 2, Locale::Ru), "A и B");
        assert_eq!(style_title(&n(&["A", "B"]), 2, Locale::En), "A & B");
        assert_eq!(style_title(&n(&["A"]), 1, Locale::Ru), "A");
    }

    #[test]
    fn calm_and_lively_lanes_preserve_generated_titles() {
        let calm_flavor = Some(nikgob_core::i18n::SideMixFlavor::Russian);
        let lively_flavor = Some(nikgob_core::i18n::SideMixFlavor::Instrumental);
        let calm_mt = lane_mix_title(v::MixKey::Calm, "Русские песни потише", calm_flavor, None);
        let lively_mt = lane_mix_title(v::MixKey::Lively, "Бодрое без слов", lively_flavor, None);
        assert_eq!(calm_mt, Some(v::MixTitle::Side { calm: true, flavor: nikgob_core::i18n::SideMixFlavor::Russian }));
        assert_eq!(lively_mt, Some(v::MixTitle::Side { calm: false, flavor: nikgob_core::i18n::SideMixFlavor::Instrumental }));

        let card = v::LaneCard {
            key: v::MixKey::Calm,
            title: "Русские песни потише".into(),
            mix_title: calm_mt,
            short_titles: Vec::new(),
            artists: Vec::new(),
            new_artists: Vec::new(),
            covers: Vec::new(),
            new_cover: None,
            tracks: Vec::new(),
            new_tracks: Vec::new(),
            blend_base: None,
            queue: None,
            why: None,
        };
        assert_eq!(card.display_title_in(Locale::En), "Calm Russian Songs");
        assert_ne!(card.display_title_in(Locale::En), "Calm Mix");
        assert_eq!(
            card.origin(),
            v::QueueOrigin::Mix(v::MixTitle::Side { calm: true, flavor: nikgob_core::i18n::SideMixFlavor::Russian })
        );
        assert_eq!(
            lively_mt.as_ref().map(|m| m.title_in(Locale::En)).as_deref(),
            Some("Upbeat Instrumental")
        );
    }

    /// «ATM 4» of two artists' discographies, a tile twice: each row keeps
    /// the first.
    #[test]
    fn home_rows_show_each_item_once() {
        use nikgob_core::streaming::Service;
        let album = |id: &str, artist: &str| v::StreamAlbumCard {
            remote: RemoteId::new(Service::Deezer, id),
            title: "ATM 4".into(),
            artist: artist.into(),
            artists: v::StreamArtistLinks::default(),
            year: Some(2026),
            release_type: nikgob_core::model::ReleaseType::Album,
            cover_url: None,
            total_tracks: None,
            release: None,
            have: v::AlbumHave::default(),
        };
        let card = |id, title: &str| v::MixCard {
            id,
            title: title.into(),
            mix_title: None,
            kicker: String::new(),
            subtitle: String::new(),
            covers: Vec::new(),
        };
        let mut home = v::HomeView {
            new_releases: vec![album("1", "Mzlff"), album("2", "Mzlff"), album("1", "Guest")],
            tiles: vec![
                card(v::MixId::Tile(v::TileKind::OnRepeat), "На повторе"),
                card(v::MixId::Tile(v::TileKind::OnRepeat), "На повторе"),
                card(v::MixId::Tile(v::TileKind::Lottery), "Лотерея"),
            ],
            style_mixes: vec![card(v::MixId::Style(0), "A и B"), card(v::MixId::Style(0), "A и B")],
            ..v::HomeView::default()
        };
        dedupe_rows(&mut home);
        let ids: Vec<(&str, &str)> = home.new_releases.iter().map(|a| (a.remote.id.as_str(), a.artist.as_str())).collect();
        assert_eq!(ids, [("1", "Mzlff"), ("2", "Mzlff")]);
        assert_eq!(home.tiles.len(), 2);
        assert_eq!(home.style_mixes.len(), 1);
    }

    #[test]
    fn moods_map_both_ways() {
        for m in Mood::ALL {
            assert_eq!(from_view_mood(to_view_mood(m)), m);
            for loc in Locale::ALL {
                assert_eq!(to_view_mood(m).title_in(loc), m.title_in(loc));
            }
        }
    }
