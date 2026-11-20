    use super::*;

    #[test]
    fn glow_pulses_then_fades_out() {
        assert_eq!(glow_at(-0.1), 0.0);
        assert!(glow_at(0.0) < 0.05);
        assert!(glow_at(0.2) > 0.5);
        // A dip between the two pulses.
        assert!(glow_at(0.375) < glow_at(0.75));
        assert!(glow_at(1.4) < 0.2);
        assert_eq!(glow_at(FOCUS_S as f32), 0.0);
    }

    #[test]
    fn group_tint_comes_up_holds_and_eases_out() {
        assert_eq!(group_tint_at(-0.1, false), 0.0);
        assert_eq!(group_tint_at(0.1, false), 0.0, "waits for the page to land");
        assert!(group_tint_at(0.3, false) > 0.5);
        assert!(group_tint_at(1.0, false) > 0.99);
        assert!(group_tint_at(2.2, false) < 0.2);
        assert_eq!(group_tint_at(GROUP_S as f32, false), 0.0);
        let mut was = 1.0;
        for ms in 1200..2400 {
            let v = group_tint_at(ms as f32 / 1000.0, false);
            assert!(v <= was);
            was = v;
        }
        // Reduced motion: there at once, then only the fade.
        assert_eq!(group_tint_at(0.0, true), 1.0);
        assert_eq!(group_tint_at(1.0, true), 1.0);
        assert!(group_tint_at(2.2, true) < 0.2);
    }

    /// Every row knows its tab, level, name and ⓘ text.
    #[test]
    fn every_row_has_a_tab_a_level_a_name_and_help() {
        for &id in crate::screens::sweep::ALL {
            assert!(id.tab() < TABS.len(), "{id:?}");
            assert!(!id.label().is_empty(), "{id:?}");
            assert!(!id.help().is_empty(), "{id:?}");
            // A name says the thing, never its value.
            assert!(!id.label().contains(':') && !id.label().ends_with(" с"), "{id:?}: {}", id.label());
        }
    }

    /// «Просто» has whether a thing is there and which kind of it;
    /// «Подробно» adds how strong, how fast and how long, the rare cases
    /// and the technical rows.
    #[test]
    fn simple_has_the_thing_and_deep_its_tuning() {
        use SettingId as S;
        for id in [
            // The animation, its step, each effect's switch.
            S::LyricsStyle, S::LyricsUnit, S::LyricsScale, S::LyricsLead, S::LyricsCascade, S::LyricsGlow, S::LyricsSoftEdges,
            S::LyricsGap, S::LyricsCountdown, S::LyricsWordsPlaying, S::LyricsLinesAuto, S::LyricsStreams,
            // Each part of the track screen.
            S::Contrast, S::CoverShape, S::CoverSpin, S::CoverPulse, S::Ring, S::Ambient, S::BgShapes,
            S::ProgressWave, S::NpVolume, S::HiddenMixes,
            S::SpotifyPlayback, S::ImportSpotify, S::DownloadToasts, S::Inbox, S::Trash,
        ] {
            assert_eq!(id.level(), Level::Simple, "{id:?}");
        }
        for id in [
            S::LyricsSmooth, S::LightSpeed, S::WaveJump, S::JumpSpeed, S::LyricsLift, S::LyricsDim, S::LyricsSoftDepth,
            S::LyricsSoftStrength, S::LyricsSoftFade, S::LyricsGapWaves, S::LyricsGapHeight, S::LyricsGapSize, S::LyricsReturn, S::WaveFallback,
            S::LyricsModels, S::CoverDir, S::CoverSpinSpeed, S::CoverPulseStrength, S::RingStyle,
            S::AmbientStrength, S::BgCount, S::NpVolumeHold, S::TapWait, S::ToastTime, S::StreamCache,
            S::SeedRatio, S::FpsLimit,
        ] {
            assert_eq!(id.level(), Level::Deep, "{id:?}");
        }
        // A row that hangs on another shows no earlier than it does.
        for (child, parent) in [
            (S::RingStyle, S::Ring),
            (S::AutostartHidden, S::Autostart),
            (S::LyricsSoftDepth, S::LyricsSoftEdges),
            (S::LyricsGapSize, S::LyricsGap),
            (S::LyricsStreamKeep, S::LyricsStreams),
            (S::KeepTimbre, S::TrackSpeed),
            (S::AutoplayWorld, S::Autoplay),
            (S::CoverSpinSpeed, S::CoverSpin),
            (S::CoverPulseStrength, S::CoverPulse),
            (S::AmbientStrength, S::Ambient),
            (S::BgCount, S::BgShapes),
            (S::NpVolumeHold, S::NpVolume),
        ] {
            assert!(child.level() >= parent.level(), "{child:?} under {parent:?}");
        }
    }

    /// Links from other screens land on the tab that has the row now.
    #[test]
    fn linked_rows_know_their_new_tab() {
        use SettingId as S;
        for (id, tab) in [
            (S::LyricsStyle, TAB_LYRICS),
            (S::LyricsUi, TAB_SYSTEM),
            (S::LyricsModels, TAB_LYRICS),
            (S::BgHue, TAB_LOOK),
            (S::MixesProfiles, TAB_LOOK),
            (S::HiddenMixes, TAB_LOOK),
            (S::LikedAlbumsPlace, TAB_LOOK),
            (S::CoverClickPause, TAB_LOOK),
            (S::Prowlarr, TAB_DOWNLOADS),
            (S::NoSeed, TAB_DOWNLOADS),
            (S::SeedRatio, TAB_DOWNLOADS),
            (S::UploadLimit, TAB_DOWNLOADS),
            (S::Soulseek, TAB_DOWNLOADS),
            (S::SoulseekUpload, TAB_DOWNLOADS),
            (S::DownloadSource(v::SourceKey::Torrents), TAB_DOWNLOADS),
            (S::LikeDownload, TAB_DOWNLOADS),
            (S::Spotify, TAB_SERVICES),
            (S::SpotifyPlayback, TAB_SERVICES),
            (S::YouTube, TAB_SERVICES),
            (S::PlaylistSync, TAB_SERVICES),
            (S::StreamAnalysis, TAB_SYSTEM),
            (S::MusicBrainz, TAB_SYSTEM),
            (S::Autoplay, TAB_MUSIC),
            (S::SearchPlay, TAB_MUSIC),
            (S::RadioSource, TAB_MUSIC),
            (S::NewShareBy(v::RadioKind::Mix), TAB_MUSIC),
            (S::DislikeButton, TAB_MUSIC),
            (S::Marks, TAB_MUSIC),
            (S::ListShuffle(v::ListKind::OwnPlaylist), TAB_MUSIC),
            (S::AlbumOpens, TAB_MUSIC),
            (S::AlbumVersions, TAB_MUSIC),
            (S::PlayerButton(v::PlayerButton::Sound), TAB_LOOK),
            (S::NavRoot, TAB_LOOK),
            (S::ApiEnabled, TAB_NET),
            (S::ApiTokens, TAB_NET),
            (S::MeshKeepMasters, TAB_NET),
            (S::MeshDevices, TAB_NET),
        ] {
            assert_eq!(id.tab(), tab, "{id:?}");
        }
        assert!(stage_tab(TAB_LYRICS) && stage_tab(TAB_LOOK) && !stage_tab(TAB_SYSTEM));
    }

    /// A link to a developer row in a handout build, or to its group, goes
    /// to a row users see; every other row is its own neighbour.
    #[test]
    fn no_link_ends_on_a_developer_row() {
        for &id in crate::screens::sweep::ALL {
            let n = id.user_neighbour();
            assert_ne!(n.level(), Level::Dev, "{id:?} → {n:?}");
            if id.level() != Level::Dev {
                assert_eq!(n, id);
            }
        }
        assert_eq!(SettingId::LyricsUi.user_neighbour(), SettingId::LyricsStyle);
        assert_eq!(SettingId::LyricsGpu.user_neighbour(), SettingId::LyricsModels);
    }

    /// «Ненастоящий FLAC» reads and writes the three old prefs, and nothing
    /// starts out making MP3 by itself.
    #[test]
    fn fake_flac_is_one_choice_over_three_prefs() {
        let id = SettingId::FakeFlac;
        for p in [Prefs::default(), Prefs::first_start()] {
            assert_ne!(id.chosen(&p), 3);
        }
        let mut p = Prefs::default();
        id.choose(&mut p, 3);
        assert!(p.check_fake_lossless && p.convert_fake_lossless && p.fake_flac_auto);
        id.choose(&mut p, 0);
        assert!(!p.check_fake_lossless);
        assert_eq!(id.chosen(&p), 0);
        id.choose(&mut p, 1);
        assert!(p.check_fake_lossless && !p.convert_fake_lossless);
        id.choose(&mut p, 2);
        assert!(p.check_fake_lossless && p.convert_fake_lossless && !p.fake_flac_auto);
    }

    #[test]
    fn every_choice_round_trips_through_prefs() {
        use SettingId as S;
        for id in [
            S::Language, S::Theme, S::Contrast, S::Scheme, S::CoverShape, S::CoverDir, S::AlbumDate, S::RingStyle, S::Ambient,
            S::BgShapes, S::BgColors, S::LyricsStyle, S::LyricsUnit, S::WaveFallback, S::LyricsGap, S::NpMotion, S::DblClickShape,
            S::OpusBitrate, S::NpVolume, S::LikeDownload, S::AlbumLike, S::SingleLike, S::LikedAlbumsPlace, S::PlaylistSync, S::RadioNewShare, S::RadioSource,
            S::NewShareBy(v::RadioKind::Track), S::NewShareBy(v::RadioKind::Autoplay), S::ProfileNames, S::BackgroundCards,
            S::RadioNoRepeat, S::RadioExplore, S::RadioUnheard, S::SoundNeural, S::SoundDevice, S::MixCardsWave, S::LyricsGpu, S::LyricsUi, S::ToastTime, S::LyricsStreamKeep,
            S::UpgradeEvery, S::UpgradeBar, S::UpgradeAuto, S::FakeFlac, S::ListShuffle(v::ListKind::Mix),
            S::ListShuffle(v::ListKind::Album), S::AlbumOpens, S::SearchPlay,
        ] {
            assert!(id.choices().len() >= 2, "{id:?}");
            for i in 0..id.choices().len() {
                let mut p = Prefs::default();
                id.choose(&mut p, i);
                assert_eq!(id.chosen(&p), i, "{id:?} option {i}");
                assert!(!id.choices()[i].help.is_empty());
            }
        }
    }

    #[test]
    fn language_setting_uses_prefs_only_and_exposes_native_endonyms() {
        use nikgob_core::i18n::{LangPref, Locale};
        let id = SettingId::Language;
        assert_eq!(id.tab(), TAB_LOOK);
        assert_eq!(id.level(), Level::Simple);
        assert_eq!(id.label_in(Locale::Ru), "Язык");
        assert_eq!(id.label_in(Locale::En), "Language");
        let ru_labels: Vec<&str> = id.choices_in(Locale::Ru).iter().map(|c| c.label).collect();
        let en_labels: Vec<&str> = id.choices_in(Locale::En).iter().map(|c| c.label).collect();
        assert_eq!(ru_labels, ["Системный", "Русский", "English (beta)"]);
        assert_eq!(en_labels, ["System", "Русский", "English (beta)"]);

        let before_global = nikgob_core::i18n::locale();
        for (i, pref) in LangPref::ALL.iter().enumerate() {
            let mut p = Prefs::default();
            id.choose(&mut p, i);
            assert_eq!(p.language, *pref);
            assert_eq!(id.chosen(&p), i);
        }
        assert_eq!(nikgob_core::i18n::locale(), before_global);
    }

    /// A preset row's options are its presets: each one picked reads back,
    /// values of the user's own read as «Свои» (the button after them), and
    /// its card's table names the values as the rows do.
    #[test]
    fn presets_are_choices_and_their_cards_read() {
        use v::presets::{Area, Field, FieldValue};
        for area in Area::ALL.into_iter().filter(|a| !a.names().is_empty()) {
            let id = SettingId::Preset(area);
            assert_eq!(id.choices().len(), area.names().len(), "{area:?}");
            for (i, c) in id.choices().iter().enumerate() {
                assert_eq!(c.label, area.names()[i]);
                let mut p = Prefs::default();
                id.choose(&mut p, i);
                assert_eq!(id.chosen(&p), i, "{area:?} {i}");
            }
            assert_eq!(id.level(), Level::Simple);
            assert_eq!(preset_title(area), id.label());
            assert!(matches!(base_body(id), Body::Table));
        }
        let mut own = Prefs::first_start();
        own.fps_limit = 75;
        assert_eq!(SettingId::Preset(Area::Load).chosen(&own), 3, "«Свои»");
        assert_eq!(field_text_in(Locale::Ru, Field::SeedRatio, &FieldValue::F32(0.0)), "без предела");
        assert_eq!(field_text_in(Locale::Ru, Field::SeedLimitGb, &FieldValue::U32(0)), "без лимита");
        assert_eq!(field_text_in(Locale::Ru, Field::SeedRatio, &FieldValue::F32(3.5)), "3,5");
        assert_eq!(field_text_in(Locale::Ru, Field::Ambient, &FieldValue::Ambient(v::Ambient::Reactive)), "Под звук");
        assert_eq!(field_text_in(Locale::Ru, Field::CoverShape, &FieldValue::Shape(v::CoverShape::Original)), "Квадрат");
        assert_eq!(field_text_in(Locale::Ru, Field::LyricsStyle, &FieldValue::U8(2)), "Волна");
        assert_eq!(field_text_in(Locale::Ru, Field::LyricsUnit, &FieldValue::U8(nikgob_view::LYRICS_BY_SYLLABLES)), "Слоги");
        assert_eq!(field_text_in(Locale::Ru, Field::LyricsLift, &FieldValue::F32(0.0)), "0 %");
        assert_eq!(field_text_in(Locale::Ru, Field::LyricsLightSpeed, &FieldValue::F32(0.55)), "55 %");
    }

    /// The new knobs of the track screen know their stage part, and the
    /// scenes acted out come from the rows that need them.
    #[test]
    fn rows_say_what_the_stage_shows() {
        use SettingId as S;
        let p = Prefs::default();
        for id in [S::CoverSpinSpeed, S::CoverPulseStrength] {
            assert_eq!((id.scene(), id.part()), (Some(Scene::Track), Some(Part::Cover)), "{id:?}");
        }
        assert_eq!((S::RingStyle.scene(), S::RingStyle.part()), (Some(Scene::Track), Some(Part::Ring)));
        assert_eq!((S::Ring.level(), S::RingStyle.level()), (Level::Simple, Level::Deep));
        assert_eq!(S::HomeSections.scene(), Some(Scene::Home));
        assert_eq!(S::MixesProfiles.part(), Some(Part::Home(Section::Mixes)));
        assert_eq!(S::AlbumDate.scene(), Some(Scene::Album));
        assert_eq!(S::Theme.scene(), None, "the theme shows on any screen");
        assert_eq!(S::RailCompact.part(), None);
        let random = Prefs { cover_shape: v::CoverShape::Random, ..p.clone() };
        assert_eq!(S::CoverShape.script(&random), Script::RandomShape);
        assert_eq!(S::ShapeRange.script(&random), Script::RandomShape);
        let one = Prefs { cover_shape: v::CoverShape::shapes()[3], ..random.clone() };
        assert_eq!(S::ShapeRange.script(&one), Script::None, "a shape of the pool under the pointer stands still");
        assert_eq!(S::LyricsCascade.script(&p), Script::LyricsStep);
        assert_eq!(S::LyricsSoftDepth.script(&p), Script::EdgesScroll);
        assert_eq!(S::LyricsCountdown.script(&p), Script::Countdown);
        assert_eq!(S::LyricsGapSize.script(&p), Script::Interlude);
        assert_eq!(S::NpMotion.script(&p), Script::Open);
        // The stage shows a knob in the look that has it.
        let karaoke = Prefs { lyrics_style: 0, lyrics_gap_style: 0, ..p.clone() };
        assert_eq!(S::JumpSpeed.staged(&karaoke).lyrics_style, 2);
        assert_eq!(S::LyricsGapWaves.staged(&karaoke).lyrics_gap_style, 1);
        assert_eq!(S::LyricsDim.staged(&karaoke), karaoke, "a row the look has is shown as it is");
        assert_eq!(S::Preset(v::presets::Area::Track).parts().len(), 4);
        assert!(S::Theme.parts().is_empty(), "the whole screen: nothing outlined");
        // Pulse and the ring default on. Those rows pulse only while the
        // switch is on; off, the stage stays calm. A shape row never pulses.
        let calm = Prefs { cover_pulse: false, ring: false, ..Prefs::default() };
        assert!(!S::CoverShape.needs_beat(&calm) && !S::RingStyle.needs_beat(&calm), "a shape stays calm");
        assert!(!S::CoverShape.needs_beat(&Prefs::default()), "a shape stays calm even when pulse is on");
        assert!(!S::CoverPulse.needs_beat(&calm), "pulse off: calm");
        assert!(S::CoverPulse.needs_beat(&Prefs { cover_pulse: true, ..calm.clone() }));
        assert!(!S::CoverPulseStrength.needs_beat(&calm));
        assert!(S::CoverPulseStrength.needs_beat(&Prefs { cover_pulse: true, ..calm.clone() }));
        assert!(S::Ring.needs_beat(&Prefs { ring: true, ..calm.clone() }));
        assert!(!S::Ring.needs_beat(&calm));
        assert!(!S::RingStyle.needs_beat(&Prefs { ring: true, ..calm.clone() }), "a look stays calm");
        assert!(S::Ambient.needs_beat(&Prefs { ambient: v::Ambient::Reactive, ..calm.clone() }));
        assert!(!S::Ambient.needs_beat(&Prefs { ambient: v::Ambient::Glow, ..calm }));
    }

    /// The card opens next to its own ⓘ, not under the row. A group title's
    /// row is the whole list; anchoring there used to drop the card at the
    /// window's corner.
    #[test]
    fn cards_open_next_to_their_button() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0));
        let size = vec2(300.0, 200.0);
        let button = Rect::from_min_size(pos2(120.0, 140.0), Vec2::splat(22.0));
        assert_eq!(place(button, size, screen), pos2(button.left(), button.bottom() + GAP), "just under the ⓘ");
        // Near the bottom: above the button, still on its left edge.
        let low = Rect::from_min_size(pos2(120.0, 700.0), Vec2::splat(22.0));
        let p = place(low, size, screen);
        assert!(p.y + size.y <= low.top() + 0.01, "flips above, {p:?}");
        assert!((p.x - low.left()).abs() < 0.01, "stays on the ⓘ, {p:?}");
        // No room to the right: the card grows left, still touching the ⓘ.
        let edge = Rect::from_min_size(pos2(1100.0, 140.0), Vec2::splat(22.0));
        let p = place(edge, size, screen);
        assert!(p.x + size.x <= screen.right() - 8.0 + 0.01, "on screen, {p:?}");
        assert!((p.x + size.x - edge.right()).abs() < 0.01, "flips left onto the ⓘ, {p:?}");
        assert!((p.y - (edge.bottom() + GAP)).abs() < 0.01, "still just under it, {p:?}");
        // A ⓘ in the bottom-right corner flips above and left, next to itself.
        let corner = Rect::from_min_size(pos2(1160.0, 760.0), Vec2::splat(22.0));
        let p = place(corner, size, screen);
        assert!(p.y + size.y <= corner.top() + 0.01, "above the corner ⓘ, {p:?}");
        assert!((p.x + size.x - corner.right()).abs() < 0.01, "left onto the corner ⓘ, {p:?}");
        assert!(p.x >= screen.left() && p.y >= screen.top());
    }
