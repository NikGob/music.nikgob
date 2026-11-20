//! Tests of the screens hub (`crate::screens::tests`).
    use super::*;

    fn context() -> Context {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        ctx
    }

    fn frame(ctx: &Context, ui: &mut Md3eUi, state: &mut ViewState, time: f64, width: f32, key: Option<Key>) -> egui::FullOutput {
        let events = key.into_iter().map(|key| egui::Event::Key {
            key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE,
        }).collect();
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 900.0))),
            time: Some(time), events, ..Default::default()
        }, |root| ui.frame(root, state, &mut commands));
        output.textures_delta.clear();
        for command in commands {
            match command {
                Command::SetPrefs(prefs) => state.prefs = *prefs,
                Command::SetPref(patch) => patch.apply(&mut state.prefs),
                _ => {}
            }
        }
        output
    }

    #[test]
    fn lyrics_link_closes_overlays_and_scrolls_to_its_row_in_all_window_sizes() {
        for width in [380.0, 800.0, 1440.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = ViewState::default();
            ui.set_now_playing(&ctx, true);
            ui.pane = Some(PaneTab::Lyrics);
            frame(&ctx, &mut ui, &mut state, 0.0, width, None);
            ui.pages.open_setting(pages::SettingId::LyricsStyle);
            frame(&ctx, &mut ui, &mut state, 1.0, width, None);
            assert_eq!(ui.route(), &Route::Settings);
            assert!(!ui.now_playing);
            assert_eq!(ui.pane, None);
            assert!(ui.pages.focus_setting.is_some());
            frame(&ctx, &mut ui, &mut state, 2.0, width, None);
            assert!(ui.pages.focus_setting.is_none());
            assert_eq!(ui.route(), &Route::Settings);
            frame(&ctx, &mut ui, &mut state, 2.5, width, None);
            let output = frame(&ctx, &mut ui, &mut state, 2.6, width, None);
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "Анимация" && (150.0..750.0).contains(&text.pos.y)
            )), "lyrics animation row was not scrolled into view at width {width}");
            if width == 380.0 {
                frame(&ctx, &mut ui, &mut state, 3.0, width, Some(Key::Escape));
                assert_eq!(ui.route(), &Route::Home);
            }
            // Opening from lyrics while Settings is already the current route
            // must still request another scroll.
            ui.navigate(&ctx, Route::Settings, &mut Vec::new());
            ui.pages.open_setting(pages::SettingId::LyricsStyle);
            frame(&ctx, &mut ui, &mut state, 4.0, width, None);
            frame(&ctx, &mut ui, &mut state, 5.0, width, None);
            assert_eq!(ui.route(), &Route::Settings);
            assert!(ui.pages.focus_setting.is_none());
        }
    }

    /// «Настройки текста…» lands on the group of the lyrics settings and
    /// tints all of it (not one row, not the groups around it), then the
    /// tint goes.
    #[test]
    fn lyrics_settings_link_tints_its_whole_group_then_lets_go() {
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut state = ViewState::default();
        frame(&ctx, &mut ui, &mut state, 0.0, 1280.0, None);
        ui.pages.open_setting_group(lyrics_ui::SETTINGS);
        for t in [1.0, 1.05, 1.1, 1.2, 1.4] {
            frame(&ctx, &mut ui, &mut state, t, 1280.0, None);
        }
        let tint = Theme::get(&ctx).scheme.primary;
        let tinted = |output: &egui::FullOutput| -> Vec<Rect> {
            output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(r) if r.fill.a() > 0 && r.fill.r().abs_diff((tint.r() as f32 * r.fill.a() as f32 / 255.0) as u8) <= 2
                    && r.fill.g().abs_diff((tint.g() as f32 * r.fill.a() as f32 / 255.0) as u8) <= 2
                    && r.rect.height() > 120.0 => Some(r.rect),
                _ => None,
            }).collect()
        };
        let text_at = |output: &egui::FullOutput, label: &str| output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos),
            _ => None,
        });
        let output = frame(&ctx, &mut ui, &mut state, 1.6, 1280.0, None);
        assert_eq!(ui.route(), &Route::Settings);
        let rects = tinted(&output);
        assert_eq!(rects.len(), 1, "one tinted group: {rects:?}");
        // «Где делать подсветку» is a developer's row now: the link lands
        // on the lyrics' own look.
        for label in ["Вид текста", "Анимация", "Размер текста"] {
            let at = text_at(&output, label).unwrap_or_else(|| panic!("{label} not drawn"));
            assert!(rects[0].contains(at + vec2(4.0, 4.0)), "{label} outside the tint");
            assert!((0.0..900.0).contains(&at.y), "{label} not scrolled into view");
        }
        for label in ["Поиск текста", "Звук", "Для разработчика"] {
            if let Some(at) = text_at(&output, label) {
                assert!(!rects[0].contains(at + vec2(4.0, 4.0)), "{label} inside the tint");
            }
        }
        frame(&ctx, &mut ui, &mut state, 4.0, 1280.0, None);
        let output = frame(&ctx, &mut ui, &mut state, 4.1, 1280.0, None);
        assert!(tinted(&output).is_empty());
        // A row link still glows its row only.
        ui.pages.open_setting(pages::SettingId::LyricsUi);
        assert!(!ui.pages.focus_group);
    }

    #[test]
    fn back_and_forward_walk_the_pages_like_a_browser() {
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut out = Vec::new();
        let album = Route::Release(nikgob_view::ReleaseId::new());
        ui.navigate(&ctx, Route::Library(nikgob_view::LibraryTab::All), &mut out);
        ui.navigate(&ctx, album.clone(), &mut out);
        ui.navigate(&ctx, Route::Settings, &mut out);
        // Back across a top-level switch, which used to forget the album.
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
        // The page's own ← from there is the same step back.
        assert!(ui.back(&ctx, &mut out));
        assert_eq!(ui.route(), &Route::Library(nikgob_view::LibraryTab::All));
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.route(), &Route::Settings);
        assert!(!ui.history_forward(&ctx, &mut out), "nothing ahead");
        // Going somewhere new drops what was ahead.
        ui.history_back(&ctx, &mut out);
        ui.navigate(&ctx, Route::Home, &mut out);
        assert!(!ui.history_forward(&ctx, &mut out));
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
    }

    /// «Разделы открываются с начала» (on by default): «Библиотека» of the
    /// rail opens «Всё» from its top, not the tab left there, with no step
    /// back to that tab; off, the tab stays.
    #[test]
    fn the_rail_opens_the_library_at_its_start() {
        use nikgob_view::LibraryTab;
        let ctx = context();
        let mut out = Vec::new();
        let mut state = ViewState::default();
        assert!(state.prefs.nav_root);
        for on in [true, false] {
            state.prefs.nav_root = on;
            let mut ui = Md3eUi::new();
            ui.navigate(&ctx, Route::Library(LibraryTab::All), &mut out);
            ui.pages.library_tab = LibraryTab::Tracks;
            ui.note_tab_switch();
            ui.navigate(&ctx, Route::Home, &mut out);
            let library = Route::Library(LibraryTab::All);
            ui.navigate(&ctx, library.clone(), &mut out);
            ui.to_section_root(&library, &state, true);
            let tab = if on { LibraryTab::All } else { LibraryTab::Tracks };
            assert_eq!(ui.pages.library_tab, tab, "on {on}");
            ui.note_tab_switch();
            // Back is Главная, not the library on «Треки».
            assert!(ui.history_back(&ctx, &mut out));
            assert_eq!(ui.route(), &Route::Home, "on {on}");
        }
    }

    #[test]
    fn history_brings_a_page_back_on_its_tab_and_where_it_was_scrolled() {
        use nikgob_view::LibraryTab;
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut out = Vec::new();
        ui.navigate(&ctx, Route::Library(LibraryTab::All), &mut out);
        ui.note_tab_switch();
        // «Альбомы», scrolled down, then an album from there.
        ui.pages.library_tab = LibraryTab::Albums;
        ui.note_tab_switch();
        assert_eq!(ui.pages.scroll_target(), Some(0.0), "a new tab starts at the top");
        ui.pages.scroll_now = 640.0;
        ui.note_tab_switch();
        let album = Route::Release(nikgob_view::ReleaseId::new());
        ui.navigate(&ctx, album.clone(), &mut out);
        assert_eq!(ui.pages.scroll_target(), Some(0.0), "an album opens at its top");
        ui.pages.scroll_now = 200.0;

        // Back: the albums tab, scrolled where it was.
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.route(), &Route::Library(LibraryTab::All));
        assert_eq!(ui.pages.library_tab, LibraryTab::Albums);
        assert_eq!(ui.pages.scroll_target(), Some(640.0));
        ui.note_tab_switch();
        // Back again: the tab before it; forward twice: the album as it was.
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.pages.library_tab, LibraryTab::All);
        ui.note_tab_switch();
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.pages.library_tab, LibraryTab::Albums);
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
        assert_eq!(ui.pages.scroll_target(), Some(200.0));
    }

    #[test]
    fn slots_toggle_restore_and_escape_in_narrow_and_wide_windows() {
        for width in [380.0, 800.0, 1440.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = ViewState::default();
            ui.set_now_playing(&ctx, true);
            frame(&ctx, &mut ui, &mut state, 0.0, width, None);
            // Lyrics is available even when this track has no text.
            for (time, key, slot) in [
                (1.0, Key::L, NpSlot::Lyrics),
                (2.0, Key::L, NpSlot::None),
                (3.0, Key::Q, NpSlot::Queue),
                (4.0, Key::Q, NpSlot::None),
                (5.0, Key::Q, NpSlot::Queue),
                (6.0, Key::L, NpSlot::Lyrics),
            ] {
                frame(&ctx, &mut ui, &mut state, time, width, Some(key));
                assert_eq!(ui.chrome.np_slot, slot);
                assert_eq!(state.prefs.np_slot, slot);
            }
            for (i, slot) in [NpSlot::None, NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info].into_iter().enumerate() {
                state.prefs.np_slot = slot;
                state.prefs.reduced_motion = i % 2 == 0;
                ui.set_now_playing(&ctx, true);
                let t = 10.0 + i as f64 * 5.0;
                frame(&ctx, &mut ui, &mut state, t, width, None);
                // The track's info is never restored: prefs that kept it
                // open nothing.
                let shown = if slot == NpSlot::Info { NpSlot::None } else { slot };
                assert_eq!(ui.chrome.np_slot, shown);
                frame(&ctx, &mut ui, &mut state, t + 1.0, width, None);
                frame(&ctx, &mut ui, &mut state, t + 2.0, width, Some(Key::Escape));
                assert_eq!(state.prefs.np_slot, NpSlot::None);
                assert_eq!(ui.now_playing, shown != NpSlot::None);
                if shown != NpSlot::None {
                    frame(&ctx, &mut ui, &mut state, t + 3.0, width, Some(Key::Escape));
                    assert!(!ui.now_playing);
                }
            }
        }
    }

    /// The queue's button and Ctrl+Q open Now Playing on the queue, not on
    /// the slot the prefs kept; in a compact window its own page
    /// (Settings) gives way to the player.
    #[test]
    fn the_queue_opens_on_the_queue_from_anywhere() {
        for width in [380.0, 800.0, 1440.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = ViewState::default();
            state.prefs.np_slot = NpSlot::Lyrics;
            let mut out = Vec::new();
            frame(&ctx, &mut ui, &mut state, 0.0, width, None);
            ui.toggle_queue(&ctx, false, &mut out);
            for t in 1..4 {
                frame(&ctx, &mut ui, &mut state, f64::from(t), width, None);
                assert_eq!(ui.chrome.np_slot, NpSlot::Queue, "{width}");
            }
            assert_eq!(state.prefs.np_slot, NpSlot::Queue);
            if width > 400.0 {
                assert!(ui.now_playing);
            }
            // Closed again, then opened on the queue while the prefs keep
            // the lyrics.
            ui.set_now_playing(&ctx, false);
            state.prefs.np_slot = NpSlot::Lyrics;
            ui.chrome.np_slot = NpSlot::Lyrics;
            frame(&ctx, &mut ui, &mut state, 5.0, width, None);
            ui.open_queue(&ctx, &mut out);
            frame(&ctx, &mut ui, &mut state, 6.0, width, None);
            assert_eq!(ui.chrome.np_slot, NpSlot::Queue, "{width}");
            // From Settings.
            ui.set_now_playing(&ctx, false);
            ui.navigate(&ctx, Route::Settings, &mut out);
            frame(&ctx, &mut ui, &mut state, 7.0, width, None);
            ui.toggle_queue(&ctx, false, &mut out);
            frame(&ctx, &mut ui, &mut state, 8.0, width, None);
            assert_eq!(ui.chrome.np_slot, NpSlot::Queue, "{width}");
            if width < 400.0 {
                assert_eq!(ui.route(), &Route::Home, "the compact player");
            } else {
                assert!(ui.now_playing);
            }
        }
    }

    /// «Источник и качество» of Now Playing's ⋮: Esc puts it away and the
    /// player stays; it is never saved, so closing the player with it open
    /// and opening it again shows what was kept before, not the info.
    #[test]
    fn track_info_is_a_look_up_that_is_never_kept() {
        for width in [380.0, 800.0, 1440.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = ViewState::default();
            state.prefs.np_slot = NpSlot::Lyrics;
            ui.set_now_playing(&ctx, true);
            frame(&ctx, &mut ui, &mut state, 0.0, width, None);
            assert_eq!(ui.chrome.np_slot, NpSlot::Lyrics);
            ui.chrome.np_slot = NpSlot::Info;
            frame(&ctx, &mut ui, &mut state, 1.0, width, None);
            assert_eq!(ui.chrome.np_slot, NpSlot::Info);
            assert_eq!(state.prefs.np_slot, NpSlot::Lyrics, "the info is not saved");
            ui.set_now_playing(&ctx, false);
            frame(&ctx, &mut ui, &mut state, 2.0, width, None);
            ui.set_now_playing(&ctx, true);
            frame(&ctx, &mut ui, &mut state, 3.0, width, None);
            assert_eq!(ui.chrome.np_slot, NpSlot::Lyrics, "what was kept comes back, not the info");

            ui.chrome.np_slot = NpSlot::Info;
            frame(&ctx, &mut ui, &mut state, 4.0, width, None);
            frame(&ctx, &mut ui, &mut state, 5.0, width, Some(Key::Escape));
            assert_eq!(ui.chrome.np_slot, NpSlot::None);
            assert_eq!(state.prefs.np_slot, NpSlot::None);
            assert!(ui.now_playing, "Esc put the info away, not the player");
        }
    }

    #[test]
    fn info_menu_selects_slot_and_bar_menu_keeps_the_shell_pane() {
        let mut chrome = chrome::Chrome::default();
        let track = nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local: true, liked: false, cover: None, clip: false, in_library: true, temporary: false,
        };
        let mut out = Vec::new();
        let ctx = egui::Context::default();
        let mut state = ViewState::default();
        state.player.current = Some(track.clone());
        state.prefs.pane_info = true;
        let info = crate::screens::pages::TrackAct::Info;
        chrome.apply_track(&ctx, info, &track, &state, Rect::NOTHING, chrome::Where::Np, &mut out);
        assert_eq!(chrome.np_slot, NpSlot::Info);
        assert_eq!(chrome.open_pane, None);
        chrome.apply_track(&ctx, info, &track, &state, Rect::NOTHING, chrome::Where::Bar, &mut out);
        assert_eq!(chrome.open_pane, Some(PaneTab::Info));
        assert_eq!(chrome.np_slot, NpSlot::Info);
        assert!(out.is_empty(), "{out:?}");

        // Another track (a queue row): its own window, never the pane.
        chrome.open_pane = None;
        let other = nikgob_view::TrackRow { id: nikgob_view::TrackId::new(), ..track.clone() };
        chrome.apply_track(&ctx, info, &other, &state, Rect::NOTHING, chrome::Where::Bar, &mut out);
        assert_eq!(chrome.open_pane, None);
        assert_eq!(out, vec![Command::TrackInfo(Some(other.id))]);
        assert_eq!(info_window::shown_track(&ctx), Some(other.id));
        // The playing one with the pane off: the window too.
        out.clear();
        state.prefs.pane_info = false;
        chrome.apply_track(&ctx, info, &track, &state, Rect::NOTHING, chrome::Where::Bar, &mut out);
        assert_eq!((chrome.open_pane, out), (None, vec![Command::TrackInfo(Some(track.id))]));
    }

    #[test]
    fn content_wheel_scrolls_and_the_rest_changes_volume() {
        for width in [380.0, 1440.0] {
            for slot in [NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info] {
                let ctx = context();
                let mut ui = Md3eUi::new();
                let mut state = ViewState::default();
                state.prefs.np_slot = slot;
                ui.set_now_playing(&ctx, true);
                frame(&ctx, &mut ui, &mut state, 0.0, width, None);
                // The info is opened from the ⋮ menu, never restored.
                ui.chrome.np_slot = slot;
                frame(&ctx, &mut ui, &mut state, 1.0, width, None);
                // The content's place is known a frame after it has opened.
                frame(&ctx, &mut ui, &mut state, 1.8, width, None);
                frame(&ctx, &mut ui, &mut state, 1.9, width, None);
                for (i, pos) in [pos2(width * 0.75, 400.0), pos2(20.0, 40.0)].into_iter().enumerate() {
                    let mut commands = Vec::new();
                    let mut output = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 900.0))),
                        time: Some(2.0 + i as f64),
                        events: vec![egui::Event::PointerMoved(pos), egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, -1.0),
                            phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE,
                        }], ..Default::default()
                    }, |root| ui.frame(root, &state, &mut commands));
                    output.textures_delta.clear();
                    assert_eq!(commands.iter().any(|c| matches!(c, Command::SetVolume(_))), i == 1);
                }
            }
        }
    }

    /// The settings' stage: drawn where the page asks for it, with the
    /// placeholder playing; it sends nothing, leaves the real Now Playing's
    /// springs alone, and wants levels only while it is up.
    #[test]
    fn the_stage_is_a_picture_of_its_own() {
        use stage::{Scene, Script, StageRequest};
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut state = ViewState::default();
        state.prefs.np_volume = false;
        state.prefs.np_volume_pop = true;
        state.prefs.dbl_click_shape = 2;
        state.prefs.cover_click_pause = true;
        ui.navigate(&ctx, Route::Settings, &mut Vec::new());
        frame(&ctx, &mut ui, &mut state, 0.0, 1280.0, None);
        frame(&ctx, &mut ui, &mut state, 1.0, 1280.0, None);
        assert!(!ui.wants_levels());
        let rect = Rect::from_min_size(pos2(640.0, 160.0), vec2(560.0, 350.0));
        let run = |ui: &mut Md3eUi, state: &ViewState, t: f64, req: Option<StageRequest>| {
            if let Some(r) = req {
                stage::request(&ctx, r);
            }
            let mut commands = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
                time: Some(t),
                ..Default::default()
            }, |root| ui.frame(root, state, &mut commands));
            output.textures_delta.clear();
            (commands, output)
        };
        let shows = [
            (Scene::Track, Script::None),
            (Scene::Track, Script::DoubleTap),
            (Scene::Track, Script::PausePlay),
            (Scene::Track, Script::Volume),
            (Scene::Lyrics, Script::LyricsReturn),
            (Scene::Lyrics, Script::Interlude),
            (Scene::Home, Script::None),
            (Scene::Album, Script::None),
            (Scene::Track, Script::Open),
        ];
        let mut t = 2.0;
        let mut title_seen = false;
        for (scene, script) in shows {
            for _ in 0..12 {
                t += 0.25;
                let mut req = StageRequest::new(rect, scene, state.prefs.clone());
                req.script = script;
                req.parts = vec![stage::Part::Cover];
                req.outline = 1.0;
                let (commands, output) = run(&mut ui, &state, t, Some(req));
                assert!(ui.wants_levels(), "{scene:?}: levels while the stage is up");
                for c in &commands {
                    assert!(!matches!(c,
                        Command::TogglePlay | Command::Seek(_) | Command::SetVolume(_) | Command::ToggleMute
                        | Command::Next | Command::Previous | Command::Like(..) | Command::Dislike { .. }
                        | Command::SetPref(nikgob_view::PrefPatch::CoverShape(_))
                    ), "{scene:?} {script:?}: the stage sent {c:?}");
                }
                if scene == Scene::Track && script == Script::None {
                    title_seen |= output.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Text(text) if text.galley.text() == "Название песни" && rect.contains(text.pos)));
                }
            }
        }
        assert!(title_seen, "the placeholder plays on the stage, inside its rect");
        // The real screen's springs were never touched; the copy has its own.
        assert!(ctx.data(|d| d.get_temp::<Animated<f32>>(egui::Id::new("np-mega"))).is_none());
        assert!(ctx.data(|d| d.get_temp::<(NpSlot, Animated<[f32; 4]>)>(egui::Id::new("np-content-fade"))).is_none());
        let copy = stage::Stage::copy(1.0, false, (0.0, Vec::new()));
        assert!(ctx.data(|d| d.get_temp::<Animated<f32>>(copy.id("np-mega"))).is_some());
        // No request: no stage, no levels.
        let (_, output) = run(&mut ui, &state, t + 1.0, None);
        assert!(!ui.wants_levels());
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "Название песни")));
    }

    /// The stage stands right of the rows in a 1280 window (the rows at
    /// most 600 wide), as a strip over them at 800, and not at all at 500.
    #[test]
    fn the_stage_stands_right_wide_on_top_narrow_and_not_in_mini_mode() {
        for width in [1280.0, 800.0, 500.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = sweep::survey_state();
            state.prefs.settings_deep = true;
            ui.pages.open_setting(pages::SettingId::Theme);
            for i in 0..10 {
                frame(&ctx, &mut ui, &mut state, f64::from(i) * 0.2, width, None);
            }
            let rows = pages::settings_help::drawn_rows(&ctx).1;
            assert!(!rows.is_empty(), "{width}: the settings rows");
            // The rows' own extent (the clip is the whole scroll area).
            let column = rows.iter().fold(rows[0].rect, |r, row| r.union(row.rect));
            match ui.stage.rect {
                Some(s) if width > 1000.0 => {
                    assert!(column.right() < s.left(), "{width}: the stage on the right of {column:?}: {s:?}");
                    assert!(column.width() <= 600.5, "{width}: rows {}", column.width());
                    assert!(s.width() > 300.0);
                }
                Some(s) if width > 600.0 => {
                    assert!(column.top() >= s.bottom(), "{width}: a strip over the rows {column:?}: {s:?}");
                    assert!(s.width() > s.height() * 1.5, "{width}: a strip {s:?}");
                }
                None => assert!(width < 600.0, "{width}: no stage"),
                Some(s) => panic!("{width}: a stage in mini mode {s:?}"),
            }
        }
    }

    /// One frame with a pointer press or release at `pos`.
    fn pointer(ctx: &Context, ui: &mut Md3eUi, state: &ViewState, time: f64, pos: egui::Pos2, pressed: bool) {
        let events = vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
        ];
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1263.0, 900.0))),
            time: Some(time), events, ..Default::default()
        }, |root| ui.frame(root, state, &mut commands));
        output.textures_delta.clear();
    }

    /// The point a click on the moving Now Playing container lands on.
    fn container_point(ui: &Md3eUi, ctx: &Context, time: f64) -> egui::Pos2 {
        let k = ui.np_anim.value(time).clamp(0.0, 1.0);
        let full = ctx.content_rect();
        ui.bar_rect.center().lerp(full.center(), k)
    }

    #[test]
    fn now_playing_turns_around_mid_flight_without_a_cooldown() {
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut state = ViewState::default();
        state.player.current = Some(nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local: true, liked: false, cover: None, clip: false, in_library: true, temporary: false,
        });
        for i in 0..3 {
            frame(&ctx, &mut ui, &mut state, i as f64 * 0.016, 1263.0, None);
        }
        // Closing: a click on the shrinking container reopens it at once.
        ui.set_now_playing(&ctx, true);
        frame(&ctx, &mut ui, &mut state, 2.0, 1263.0, None);
        ui.set_now_playing(&ctx, false);
        let t = 2.05;
        frame(&ctx, &mut ui, &mut state, t, 1263.0, None);
        let k = ui.np_anim.value(t);
        assert!(k > 0.05 && k < 0.95, "mid-close, k = {k}");
        let p = container_point(&ui, &ctx, t);
        pointer(&ctx, &mut ui, &state, t + 0.01, p, true);
        pointer(&ctx, &mut ui, &state, t + 0.02, p, false);
        assert!(ui.now_playing, "a click mid-close reopens");
        // Retargeted from where it was (with its speed), not restarted from 0.
        assert_eq!(ui.np_anim.target(), 1.0);
        assert!(ui.np_anim.value(t + 0.03) > 0.05);

        // Opening: a click on the growing container closes it again, even
        // when the press and the release straddle the moment the content
        // becomes clickable.
        ui.set_now_playing(&ctx, false);
        frame(&ctx, &mut ui, &mut state, 5.0, 1263.0, None);
        ui.set_now_playing(&ctx, true);
        let t = 5.02;
        frame(&ctx, &mut ui, &mut state, t, 1263.0, None);
        let p = container_point(&ui, &ctx, t);
        pointer(&ctx, &mut ui, &state, t + 0.01, p, true);
        pointer(&ctx, &mut ui, &state, t + 0.4, p, false);
        assert!(!ui.now_playing, "a click mid-open closes");
    }
