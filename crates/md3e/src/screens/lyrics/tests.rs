    use super::*;

    fn line(start: f32, end: Option<f32>, text: &str) -> LyricLine {
        LyricLine { start: Duration::from_secs_f32(start), end: end.map(Duration::from_secs_f32), text: text.into(), ..Default::default() }
    }

    fn interludes(lines: &[LyricLine]) -> Vec<(f32, f32, bool)> {
        display_lines(lines).iter().filter_map(|item| match *item {
            DisplayLine::Interlude { start, end, duet } => Some((start.as_secs_f32(), end.as_secs_f32(), duet)),
            _ => None,
        }).collect()
    }

    /// A preset's tile, still or moving, paints its sample inside the tile:
    /// every shape and its clip are on the tile after the scene is scaled
    /// down to it, and the sung line spans most of it.
    #[test]
    fn a_tile_sample_is_scaled_into_its_tile() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        Theme::default().install(&ctx);
        let tile = Rect::from_min_size(pos2(200.0, 100.0), vec2(112.0, 72.0));
        for (k, prefs) in [Prefs::first_start(), Prefs { lyrics_style: 2, ..Prefs::first_start() }].into_iter().enumerate() {
            for still in [Some(1.25), None] {
                let mut clips = Vec::new();
                let mut bounds = Rect::NOTHING;
                for f in 0..3 {
                    let input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                        time: Some(1.0 + f64::from(f) * 0.3),
                        ..Default::default()
                    };
                    let out = ctx.run_ui(input, |ui| {
                        let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(tile));
                        tui.set_clip_rect(ui.clip_rect().intersect(tile));
                        tile_preview(&tui, tile, &prefs, still);
                    });
                    let mut out = out;
                    out.textures_delta.clear();
                    clips = out.shapes.iter().filter(|c| !matches!(c.shape, egui::Shape::Noop)).map(|c| c.clip_rect).collect();
                    bounds = out.shapes.iter().filter(|c| matches!(c.shape, egui::Shape::Text(_))).fold(Rect::NOTHING, |b, c| b.union(c.shape.visual_bounding_rect()));
                }
                assert!(!clips.is_empty(), "{k} {still:?}: nothing painted");
                for c in &clips {
                    assert!((c.min - tile.min).length() < 1.0 && (c.max - tile.max).length() < 1.0, "{k} {still:?}: clip {c:?} is not the tile {tile:?}");
                }
                assert!(bounds.width() > tile.width() * 0.6 && bounds.width() < tile.width() * 1.05, "{k} {still:?}: text {bounds:?}");
            }
        }
    }

    /// «Волна» with the calm fallback showed a word-timed text as karaoke:
    /// word times are real, so the letters wave over them; only a text with
    /// line times alone becomes bright.
    #[test]
    fn the_wave_goes_over_word_timing_and_only_line_timing_falls_back() {
        let p = Prefs { lyrics_style: 2, wave_syllables_only: true, ..Prefs::default() };
        let mut worded = line(1.0, Some(2.0), "Calling through");
        worded.words = vec![
            nikgob_view::LyricWord { start: Duration::from_secs(1), end: Duration::from_millis(1500), text: "Calling ".into() },
            nikgob_view::LyricWord { start: Duration::from_millis(1500), end: Duration::from_secs(2), text: "through".into() },
        ];
        let lines_only = line(1.0, Some(2.0), "Calling through");
        assert!(style_for(&p, std::slice::from_ref(&worded)).style == Style::Wave);
        assert!(style_for(&p, std::slice::from_ref(&lines_only)).style == Style::Focus);
        let mut syllables = worded.clone();
        syllables.words[0].text = "Call".into();
        assert!(style_for(&p, &[syllables]).style == Style::Wave);
        let p = Prefs { wave_syllables_only: false, ..p };
        assert!(style_for(&p, std::slice::from_ref(&worded)).style == Style::Wave);
        assert!(style_for(&p, std::slice::from_ref(&lines_only)).style == Style::Wave);
        let p = Prefs { lyrics_style: 1, ..p };
        assert!(style_for(&p, std::slice::from_ref(&worded)).style == Style::Focus);
        let p = Prefs { lyrics_style: 0, ..p };
        assert!(style_for(&p, &[worded]).style == Style::Karaoke);
    }

    /// KOMA, an empty stamp (a wait), then a line wrapping in two rows: the
    /// folded wait sat on the long line's top and took its pointer too.
    #[test]
    fn only_one_item_takes_the_pointer_and_a_folded_wait_none() {
        let reach = 9.0;
        // Folded: the wait has the long line's top and no room.
        let folded = [Some((0.0, 34.0, 1.0)), Some((52.0, 42.0, 0.0)), Some((52.0, 68.0, 1.0))];
        for y in [44.0, 52.0, 60.0, 80.0, 128.0] {
            assert_eq!(hit_target(&folded, y, reach), Some(2), "y={y}");
        }
        assert_eq!(hit_target(&folded, 20.0, reach), Some(0));
        assert_eq!(hit_target(&folded, 140.0, reach), None);
        // Half unfolded: the wait owns its half slot, the line moved down
        // by half of the wait's room (slot and gap).
        let half = [Some((0.0, 34.0, 1.0)), Some((52.0, 21.0, 0.5)), Some((82.0, 68.0, 1.0))];
        assert_eq!(hit_target(&half, 60.0, reach), Some(1));
        assert_eq!(hit_target(&half, 90.0, reach), Some(2));
        // Between them both reach: the nearer slot wins, never both.
        assert_eq!(hit_target(&half, 76.0, reach), Some(1));
        assert_eq!(hit_target(&half, 79.0, reach), Some(2));
        // A background voice hugging its line: the one the pointer is in.
        let hug = [Some((0.0, 40.0, 1.0)), Some((30.0, 30.0, 1.0))];
        assert_eq!(hit_target(&hug, 20.0, reach), Some(0));
        assert_eq!(hit_target(&hug, 50.0, reach), Some(1));
        assert_eq!(hit_target(&hug, 32.0, reach), Some(0), "both contain it: the nearer centre");
        assert_eq!(hit_target(&hug, 36.0, reach), Some(1));
        // Items not drawn take nothing.
        assert_eq!(hit_target(&[None, Some((100.0, 20.0, 1.0))], 10.0, reach), None);
    }

    fn word(start: f32, end: f32, text: &str) -> nikgob_view::LyricWord {
        nikgob_view::LyricWord { start: Duration::from_secs_f32(start), end: Duration::from_secs_f32(end), text: text.into() }
    }

    /// The timing of `l` laid out as `g` for `look`, at the default settings
    /// and 26 px.
    fn timing(l: &LyricLine, g: &Galley, look: Look) -> Timing {
        Timing::of(l, g, look, Pace::of(&Prefs::default()), 26.0).unwrap()
    }

    /// The units of a timed line as written, with their times.
    fn units_of(painter: &egui::Painter, l: &LyricLine) -> Vec<(String, f32, f32)> {
        let g = fixed_layout(painter, &l.text, 26.0);
        let t = Timing::new(l, &g).unwrap();
        let mut out: Vec<(String, f32, f32)> = t.units.iter().map(|u| (String::new(), u.start, u.end)).collect();
        for (gl, u) in g.rows.iter().flat_map(|r| &r.row.glyphs).zip(&t.glyph_units) {
            if let Some(u) = u {
                out[*u].0.push(gl.chr);
            }
        }
        out
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn written_units_split_hyphenated_words_as_written() {
        let text = "Я делаю do-dodge back — уклоняюсь от атаки";
        let units = written_units(text.chars());
        let mut words: Vec<String> = Vec::new();
        for (c, u) in text.chars().zip(&units) {
            if let Some(u) = *u {
                if words.len() <= u {
                    words.push(String::new());
                }
                words[u].push(c);
            }
        }
        assert_eq!(words, ["Я", "делаю", "do-", "dodge", "back", "—", "уклоняюсь", "от", "атаки"]);
        // A leading or doubled hyphen, or a dash between words, stays put.
        let parts = |t: &str| written_units(t.chars()).into_iter().flatten().max().map_or(0, |n| n + 1);
        assert_eq!(parts("-то"), 1);
        assert_eq!(parts("кто-то"), 2);
        assert_eq!(parts("a--"), 1);
        assert_eq!(parts("1-2"), 2);
    }

    /// Word timing keeps «do-dodge» one timed word: its parts share the
    /// word's time by their letters, weighted as everywhere: «do-» is
    /// d 1 + o 2.5 + «-» 0.25, «dodge» 1 + 2.5 + 1 + 1 + 2.5.
    #[test]
    fn words_split_at_the_hyphen_share_the_word_time_by_letters() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "Я делаю do-dodge back");
            l.words = vec![word(1.0, 1.2, "Я "), word(1.2, 2.0, "делаю "), word(2.0, 2.8, "do-dodge "), word(3.0, 3.5, "back")];
            let u = units_of(painter, &l);
            assert_eq!(u.iter().map(|u| u.0.as_str()).collect::<Vec<_>>(), ["Я", "делаю", "do-", "dodge", "back"]);
            let cut = 2.0 + 0.8 * 3.75 / 11.75;
            assert!(close(u[2].1, 2.0) && close(u[2].2, cut), "{:?}", u[2]);
            assert!(close(u[3].1, cut) && close(u[3].2, 2.8), "{:?}", u[3]);
            assert!(close(u[4].1, 3.0) && close(u[4].2, 3.5));
        });
    }

    /// Syllables light with their unit: «dodge» at its first syllable,
    /// never «dod» then «ge».
    #[test]
    fn syllables_of_one_unit_light_together() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "do-dodge back");
            l.words = vec![word(1.0, 1.4, "do-"), word(1.5, 1.8, "dod"), word(1.9, 2.4, "ge "), word(2.5, 3.0, "back")];
            let u = units_of(painter, &l);
            let want = [("do-", 1.0, 1.4), ("dodge", 1.5, 2.4), ("back", 2.5, 3.0)];
            assert_eq!(u.len(), want.len());
            for (got, want) in u.iter().zip(want) {
                assert!(got.0 == want.0 && close(got.1, want.1) && close(got.2, want.2), "{got:?}");
            }
            // Lit as one: the whole unit's light comes from its start.
            let pace = Pace::of(&Prefs::default());
            let (karaoke, focus) = (pace.rise(Style::Karaoke), pace.rise(Style::Focus));
            let lit = |rise: f32, pos: f32| light_up(pos - 1.5, rise);
            assert_eq!(lit(focus, 1.4), 0.0);
            assert!(lit(focus, 1.6) > 0.5);
            assert!(lit(focus, 1.9) > 0.99);
            // Karaoke fills a word quicker than focus brightens it.
            assert!(lit(karaoke, 1.55) > lit(focus, 1.55));
            assert!(lit(karaoke, 1.7) > 0.99);
            for rise in [karaoke, focus, 0.0, 0.5] {
                let mut was = 0.0;
                for ms in 1450..1900 {
                    let v = lit(rise, ms as f32 / 1000.0);
                    assert!(v >= was && v <= 1.0);
                    was = v;
                }
            }
        });
    }

    /// Line times only: «Спокойный Фокус» makes the line bright as a
    /// whole under every animation and every unit; otherwise the words are
    /// paced from the line, split as written. Real times keep the look.
    #[test]
    fn every_look_and_unit_follows_the_line_only_fallback() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_SYLLABLES, LYRICS_BY_WORDS};
        let lines_only = [line(1.0, Some(3.0), "do-dodge back")];
        let mut worded = lines_only[0].clone();
        worded.words = vec![word(1.0, 2.0, "do-dodge "), word(2.0, 3.0, "back")];
        let mut syllabled = lines_only[0].clone();
        syllabled.words = vec![word(1.0, 1.4, "do-"), word(1.4, 1.7, "dod"), word(1.7, 2.0, "ge "), word(2.0, 3.0, "back")];
        assert_eq!(LyricsSync::of(std::slice::from_ref(&syllabled)), Some(LyricsSync::Syllable));
        for (anim, style) in [(0, Style::Karaoke), (1, Style::Focus), (2, Style::Wave)] {
            for (unit, by) in [(LYRICS_BY_LETTERS, By::Letters), (LYRICS_BY_SYLLABLES, By::Syllables), (LYRICS_BY_WORDS, By::Words)] {
                let calm = Prefs { lyrics_style: anim, lyrics_unit: unit, wave_syllables_only: true, ..Prefs::default() };
                let case = format!("style {anim}, unit {unit}");
                assert_eq!(style_for(&calm, &lines_only), CALM, "{case}");
                let paced = Prefs { wave_syllables_only: false, ..calm.clone() };
                assert_eq!(style_for(&paced, &lines_only), Look { style, by }, "{case}");
                for timed in [&worded, &syllabled] {
                    assert_eq!(style_for(&calm, std::slice::from_ref(timed)), Look { style, by }, "{case}");
                    assert_eq!(style_for(&paced, std::slice::from_ref(timed)), Look { style, by }, "{case}");
                }
            }
        }
        // The old fourth animation reads as the wave by words here too.
        let old = Prefs { lyrics_style: 3, wave_syllables_only: true, ..Prefs::default() };
        assert_eq!(style_for(&old, &lines_only), CALM);
        assert_eq!(style_for(&old, std::slice::from_ref(&worded)), Look { style: Style::Wave, by: By::Words });
        // No text is not a guessed one.
        assert_eq!(style_for(&Prefs::default(), &[]), Look::of(&Prefs::default()));
        with_painter(|painter| {
            let u = units_of(painter, &paced_words(&lines_only[0], None));
            assert_eq!(u.iter().map(|u| u.0.as_str()).collect::<Vec<_>>(), ["do-", "dodge", "back"]);
            // 8 + 4 letters over the line's two seconds; inside «do-dodge»
            // its parts by their weighted letters (3.75 and 8).
            assert!(close(u[1].1, 1.0 + 2.0 * 8.0 / 12.0 * 3.75 / 11.75), "{:?}", u[1]);
            assert!(close(u[2].1, 1.0 + 2.0 * 8.0 / 12.0), "{:?}", u[2]);
        });
    }

    /// The unit applies to each animation (by syllables unless chosen
    /// otherwise); the old fourth animation (`lyrics_style` 3, «По
    /// словам») reads as the wave by words.
    #[test]
    fn the_unit_goes_with_every_animation_and_the_old_words_style_is_the_wave_by_words() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_SYLLABLES, LYRICS_BY_WORDS};
        for (anim, style) in [(0, Style::Karaoke), (1, Style::Focus), (2, Style::Wave)] {
            for (unit, by) in [(LYRICS_BY_LETTERS, By::Letters), (LYRICS_BY_SYLLABLES, By::Syllables), (LYRICS_BY_WORDS, By::Words)] {
                let p = Prefs { lyrics_style: anim, lyrics_unit: unit, ..Prefs::default() };
                assert_eq!(Look::of(&p), Look { style, by });
            }
        }
        assert_eq!(Look::of(&Prefs::default()).by, By::Syllables);
        let old = Prefs { lyrics_style: 3, lyrics_by_words: false, lyrics_unit: LYRICS_BY_LETTERS, ..Prefs::default() };
        assert_eq!(Look::of(&old), Look { style: Style::Wave, by: By::Words });
        let mut moved = old.clone();
        moved.upgrade_lyrics_style();
        assert_eq!((moved.lyrics_style, moved.lyrics_by_words, moved.lyrics_unit), (2, true, LYRICS_BY_WORDS));
        assert_eq!(Look::of(&moved), Look::of(&old));
        // Prefs from before syllables: letters (the old default) move to
        // syllables, words chosen stay.
        for (by_words, unit) in [(false, LYRICS_BY_SYLLABLES), (true, LYRICS_BY_WORDS)] {
            let mut p = Prefs { lyrics_by_words: by_words, lyrics_unit: LYRICS_BY_LETTERS, ..Prefs::default() };
            p.upgrade_lyrics(3);
            assert_eq!(p.lyrics_unit, unit);
            // Chosen since, it stays.
            let mut q = Prefs { lyrics_unit: LYRICS_BY_LETTERS, ..p };
            q.upgrade_lyrics(nikgob_view::LYRICS_REV);
            assert_eq!(q.lyrics_unit, LYRICS_BY_LETTERS);
        }
    }

    /// A wave letter or word jumps at its onset: still before it, off the
    /// line at once, over the top within a tenth of a second, then down onto
    /// its rest height on the spring; the speed setting scales the time.
    #[test]
    fn a_wave_piece_jumps_on_its_onset_and_settles() {
        assert_eq!(jump_height(-0.1, 1.0), 0.0);
        assert_eq!(jump_height(-ONSET_LEAD, 1.0), 0.0);
        // Up at once: a fifth of the way within 10 ms of the onset lead.
        assert!(jump_height(-ONSET_LEAD + 0.01, 1.0) > 0.2);
        let peak = |speed: f32| {
            (0..400).map(|ms| (ms, jump_height(ms as f32 / 1000.0 - ONSET_LEAD, speed)))
                .fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a })
        };
        let (at, top) = peak(1.0);
        assert!(at <= 100 && top > 1.4 && top < 1.8, "{at} ms, {top}");
        // At rest after its settle time, whatever the speed.
        for speed in [0.5, 1.0, 2.0] {
            let rest = jump_height(JUMP_SETTLE / speed - ONSET_LEAD, speed);
            assert!((rest - 1.0).abs() < 0.005, "{speed}: {rest}");
            assert!((jump_height(5.0, speed) - 1.0).abs() < 1e-4);
        }
        // Twice the speed: the same jump in half the time.
        assert!((peak(2.0).1 - top).abs() < 0.02 && peak(2.0).0 * 2 <= at + 4);
        assert!(peak(0.5).0 > at);
    }

    /// The speed settings: defaults light a word in about 60 ms and the
    /// karaoke edge is about a letter wide; the extremes are a quick ease
    /// (never a hard cut) and slow.
    #[test]
    fn the_light_speed_runs_from_smooth_to_at_once() {
        let p = Prefs::default();
        assert!((p.lyrics_jump_speed - 1.0).abs() < 1e-6);
        let pace = Pace::of(&p);
        assert!(pace.rise(Style::Wave) > 0.04 && pace.rise(Style::Wave) < 0.08);
        assert!(pace.rise(Style::Focus) > pace.rise(Style::Karaoke));
        assert!((pace.edge(26.0) - 26.0 * 0.575).abs() < 0.5);
        // The fastest is a quick ease, never a switch: no frame at 240 a
        // second takes it more than a fifth of the way.
        let at_once = Pace::of(&Prefs { lyrics_light_speed: 1.0, ..p.clone() });
        assert_eq!(at_once.rise(Style::Karaoke), LIGHT_FLOOR);
        assert!(at_once.edge(26.0) > 2.0 && at_once.edge(26.0) < pace.edge(26.0));
        assert!(at_once.rise(Style::Wave) <= pace.rise(Style::Wave));
        let mut was = 0.0;
        for i in 0..=120 {
            let v = light_up(-ONSET_LEAD + i as f32 / 240.0, at_once.rise(Style::Wave));
            assert!(v >= was && v - was < 0.2, "{i}: {was} → {v}");
            was = v;
        }
        assert!(light_up(-ONSET_LEAD + LIGHT_FLOOR, at_once.rise(Style::Wave)) > 0.85 && was > 0.999);
        let smooth = Pace::of(&Prefs { lyrics_light_speed: 0.0, ..p.clone() });
        assert!(smooth.rise(Style::Wave) >= 0.5 && smooth.edge(26.0) > 2.0 * pace.edge(26.0));
        assert!(light_up(0.1, smooth.rise(Style::Wave)) < light_up(0.1, pace.rise(Style::Wave)));
        // Out-of-range values are held to the slider's range.
        let wild = Pace::of(&Prefs { lyrics_jump_speed: 9.0, lyrics_light_speed: -1.0, ..p });
        assert_eq!((wild.jump, wild.light), (2.0, 0.0));
    }

    /// Every look paints a word-timed line (current, fading out, and not
    /// current) without panicking, by letters and by words, also wrapped.
    #[test]
    fn every_look_paints_a_timed_line() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "Я делаю do-dodge back");
            l.words = vec![word(1.0, 1.2, "Я "), word(1.2, 2.0, "делаю "), word(2.0, 2.4, "do-"), word(2.4, 2.8, "dod"), word(2.8, 3.0, "ge "), word(3.0, 3.5, "back")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), g.size());
            for style in [Style::Karaoke, Style::Focus, Style::Wave] {
                for by in [By::Letters, By::Syllables, By::Words] {
                    let look = Look { style, by };
                    let t = timing(&l, &g, look);
                    let prefs = Prefs { lyrics_glow: true, ..Prefs::default() };
                    for (at, current, focus) in [(0.5, true, 1.0), (2.5, true, 1.0), (2.9, false, 0.4), (5.0, false, 0.0)] {
                        timed_line(painter, &g, rect, &t, Duration::from_secs_f32(at), focus, if current { 1.0 } else { focus }, 26.0,
                            look, Color32::GRAY, Color32::WHITE, &prefs, None);
                    }
                }
            }
        });
    }

    /// Soft edges: the text is whole in the middle of the view and thins
    /// over a band at either edge, to nothing at the edge itself, by height
    /// alone and with no step.
    #[test]
    fn soft_edges_fade_over_the_band_only() {
        let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(600.0, 500.0));
        let size = 26.0;
        let soft = Soft::of(&Prefs::default());
        let fade = Fade::of(rect, soft);
        let band = soft.band(rect);
        assert!((40.0..=200.0).contains(&band), "{band}");
        // Whole from the bands' inner ends on.
        assert_eq!(fade.at(rect.top() + band), 1.0);
        assert_eq!(fade.at(rect.center().y), 1.0);
        assert_eq!(fade.at(rect.bottom() - band), 1.0);
        assert!(fade.clear(rect.top() + band + 0.5, rect.bottom() - band - 0.5));
        assert!(!fade.clear(rect.top() + band - 1.0, rect.center().y));
        assert!(!fade.clear(rect.center().y, rect.bottom() - band + 1.0));
        // Nothing at the edges and past them.
        for y in [rect.top(), rect.bottom(), rect.top() - 30.0, rect.bottom() + 30.0] {
            assert_eq!(fade.at(y), 0.0, "{y}");
        }
        // Rising all the way in from an edge with no step, the same from
        // the top and from the bottom.
        let mut last = 0.0;
        for i in 0..=(band as i32 * 4) {
            let d = i as f32 * 0.25;
            let a = fade.at(rect.top() + d);
            assert!(a >= last - 1e-6 && a - last < 0.02, "{d}: {last} to {a}");
            assert!((a - fade.at(rect.bottom() - d)).abs() < 1e-3, "{d}");
            last = a;
        }
        // A line comes in gradually over the whole band: faint a quarter
        // of the way in, about half there halfway, mostly there at three
        // quarters (not whole until just before the edge and then gone).
        assert!(fade.at(rect.top() + band * 0.25) < 0.3);
        let half_in = fade.at(rect.top() + band * 0.5);
        assert!((0.3..0.7).contains(&half_in), "{half_in}");
        assert!(fade.at(rect.top() + band * 0.75) > 0.7);
        // The blur: none in the middle and in the inner half of the band,
        // growing to the edge, by the letters' size and the strength.
        let sized = Fade::of(rect, soft.text(40.0, 30.0));
        assert_eq!(sized.blur_at(rect.center().y), 0.0);
        assert_eq!(sized.blur_at(rect.top() + band * 0.5), 0.0);
        assert!(sized.blur_at(rect.top() + band * 0.1) > sized.blur_at(rect.top() + band * 0.3));
        assert!(sized.blur_at(rect.top()) > 2.0 && sized.blur_at(rect.top()) < 8.0, "{}", sized.blur_at(rect.top()));
        assert_eq!(Fade::of(rect, Soft { strength: 0.0, ..soft.text(40.0, 30.0) }).blur_at(rect.top()), 0.0);
        // The band takes at least about a line, whatever the depth.
        assert!(Soft { depth: 0.08, ..soft }.text(80.0, 30.0).band(rect) >= 100.0);
        // A deeper band reaches further in; a gentler fade keeps the text
        // brighter at the same place.
        let near = rect.top() + 45.0;
        let of = |soft: Soft| Fade::of(rect, soft);
        assert!(of(Soft { depth: 0.3, ..soft }).at(near) < of(Soft { depth: 0.1, ..soft }).at(near));
        let mid = rect.top() + band * 0.5;
        assert!(of(Soft { fade: 0.0, ..soft }).at(mid) > of(Soft { fade: 1.0, ..soft }).at(mid));
        // A short view keeps its middle whole.
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 160.0));
        let short = Fade::of(small, Soft { depth: 0.1, ..soft });
        assert_eq!(short.at(80.0), 1.0);
        assert!(short.clear(60.0, 90.0));
        // The current line rests at 40 % of the whole view, clear of the
        // top band even when the band is deep, never below the middle.
        let (half, ink) = (17.0, size * 0.4);
        assert_eq!(rest_at(rect, 0.4, None, half, ink), 200.0);
        assert_eq!(rest_at(rect, 0.4, Some(soft), half, ink), 200.0);
        let deepest = Soft { depth: 0.4, ..soft };
        let rest = rest_at(rect, 0.4, Some(deepest), half, ink);
        assert!(rest > 200.0 && rest <= 250.0);
        assert!(of(deepest).clear(rect.top() + rest - half, rect.top() + rest + half));
    }

    /// A line near the edge gets copies of its glyphs spread around them
    /// (its blur); one in the middle and one in the inner half of a band
    /// keep their glyphs alone, and a strength of nothing blurs nothing.
    #[test]
    fn soft_edges_blur_only_right_at_the_edge() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let g = fixed_layout(ui.painter(), "Строка номер", 30.0);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 600.0));
            let soft = Soft::of(&Prefs::default()).text(60.0, 30.0);
            let fade = Fade::of(rect, soft);
            let count = |g: &Galley| g.rows.iter().map(|r| r.row.visuals.mesh.vertices.len()).sum::<usize>();
            let base = count(&g);
            assert!(base > 0);
            let at = |y: f32, fade: Fade| count(&faded(&g, pos2(0.0, y), Some(Color32::WHITE), fade));
            assert_eq!(at(285.0, fade), base, "the middle");
            assert_eq!(at(fade.band * 0.6, fade), base, "the inner half of the band");
            // At the edge: the copies (some cut at the edge itself).
            assert!(at(2.0, fade) > base * 5, "at the edge");
            assert_eq!(at(12.0, Fade::of(rect, Soft { strength: 0.0, ..soft })), base);
        });
        output.textures_delta.clear();
    }

    /// A line crossing an edge of the view is cut there by the mesh itself:
    /// no vertex lies past the edge, the vertices on it are clear, so the
    /// opacity the GPU blends between vertices is nothing right at the
    /// clip, at the top and at the bottom, sharp or blurred. Inside, the
    /// line is still drawn.
    #[test]
    fn a_line_crossing_an_edge_fades_to_nothing_on_it() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let g = fixed_layout(ui.painter(), "Строка номер", 30.0);
            let h = g.size().y;
            let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(600.0, 500.0));
            for strength in [0.0, 1.5] {
                let fade = Fade::of(rect, Soft { strength, ..Soft::of(&Prefs::default()).text(60.0, 30.0) });
                // Half above the top edge, half below the bottom one.
                for (y, edge) in [(rect.top() - h * 0.5, rect.top()), (rect.bottom() - h * 0.5, rect.bottom())] {
                    let out = faded(&g, pos2(0.0, y), Some(Color32::WHITE), fade);
                    let mut on_edge = 0;
                    let mut seen = false;
                    for r in &out.rows {
                        let m = &r.row.visuals.mesh;
                        for t in m.indices.chunks_exact(3) {
                            let ys: Vec<f32> = t.iter().map(|&i| y + r.pos.y + m.vertices[i as usize].pos.y).collect();
                            assert!(ys.iter().all(|&v| v >= rect.top() - 1e-3 && v <= rect.bottom() + 1e-3), "{ys:?} past the view");
                        }
                        for v in &m.vertices {
                            let at = y + r.pos.y + v.pos.y;
                            if (at - edge).abs() < 1e-3 {
                                on_edge += 1;
                                assert_eq!(v.color.a(), 0, "a vertex on the edge is clear");
                            } else if v.color.a() > 0 {
                                seen = true;
                            }
                        }
                    }
                    assert!(on_edge > 0, "the glyphs are cut on the edge {edge} (strength {strength})");
                    assert!(seen, "the part inside is drawn");
                }
            }
        });
        output.textures_delta.clear();
    }

    /// With the soft edges on every line in the view is still drawn, with
    /// its own glyphs (sharp, no picture of it), and a line in the top
    /// band thins towards the edge.
    #[test]
    fn soft_edges_keep_every_line_in_the_view() {
        let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0));
        // Each line drawn whole inside the view: its text, its top, and
        // the least and the most opaque of its glyph vertices.
        let drawn = |soft: bool| {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            crate::typography::install_fonts(&mut fonts);
            ctx.set_fonts(fonts);
            let mut state = ViewState::default();
            state.prefs = Prefs { lyrics_soft_edges: soft, lyrics_soft_depth: 0.3, lyrics_glow: false, ..Prefs::default() };
            state.player.status = Status::Paused;
            state.player.position = Duration::from_secs(40);
            state.lyrics.lines = (0..20).map(|i| line(i as f32 * 4.0, None, &format!("Строка номер {i}"))).collect();
            let mut lines: Vec<(String, f32, u8, u8)> = Vec::new();
            for frame in 0..16 {
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(view),
                    time: Some(frame as f64 * 0.25), ..Default::default()
                }, |ui| {
                    show(ui, &state, 0.0, false, &mut Vec::new());
                });
                output.textures_delta.clear();
                lines.clear();
                for shape in &output.shapes {
                    let egui::Shape::Text(text) = &shape.shape else { continue };
                    if !text.galley.job.text.starts_with("Строка номер") {
                        continue;
                    }
                    // Where the line is laid out (its mesh may be cut at an
                    // edge, or spread by a blur, with the soft edges).
                    let ink = text.galley.rect.translate(text.pos.to_vec2());
                    if ink.top() < view.top() + 2.0 || ink.bottom() > view.bottom() - 2.0 {
                        continue;
                    }
                    let mut alphas = Vec::new();
                    for row in &text.galley.rows {
                        let v = &row.row.visuals;
                        assert_eq!(v.mesh.texture_id, egui::TextureId::default(), "drawn from the font's own glyphs");
                        alphas.extend(v.mesh.vertices[v.glyph_vertex_range.clone()].iter().map(|v| text.override_text_color.unwrap_or(v.color).a()));
                    }
                    let (least, most) = (alphas.iter().copied().min().unwrap_or(0), alphas.iter().copied().max().unwrap_or(0));
                    lines.push((text.galley.job.text.clone(), ink.top(), least, most));
                }
            }
            lines.sort_by(|a, b| a.1.total_cmp(&b.1));
            lines
        };
        let (plain, soft) = (drawn(false), drawn(true));
        assert!(plain.len() >= 8, "{plain:?}");
        // The same lines are there with the soft edges as without.
        let texts = |lines: &[(String, f32, u8, u8)]| lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&soft), texts(&plain));
        assert!(soft.iter().all(|l| l.3 > 0), "{soft:?}");
        // The topmost one lies in the band: thinner at its top than at its
        // bottom. One in the middle is whole.
        let top = &soft[0];
        assert!(top.2 < top.3, "{top:?}");
        let most = soft.iter().map(|l| l.3).max().unwrap();
        assert!(top.3 < most, "{top:?} against {most}");
    }

    /// The mega view's control has its own row under the lyrics, with air
    /// between: no line or hover lies under it.
    #[test]
    fn the_control_has_a_row_of_its_own_under_the_lyrics() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0));
        let (lyrics, row) = control_row(area);
        let row = row.unwrap();
        assert_eq!(row.bottom(), area.bottom());
        assert_eq!(row.height(), Size::Xs.height());
        assert!(lyrics.bottom() + CONTROL_GAP <= row.top() + 1e-3);
        assert!(!lyrics.intersects(row));
    }


    /// The lead moves what is painted, within ±0.5 s and never before 0;
    /// the default lights a little before the voice.
    #[test]
    fn the_lead_moves_the_painted_position_only_within_its_range() {
        let at = |s: f32| Duration::from_secs_f32(s);
        let led_s = |pos: f32, lead: f32| led(at(pos), lead).as_secs_f32();
        assert!(close(led_s(10.0, 0.1), 10.1));
        assert!(close(led_s(10.0, -0.25), 9.75));
        assert!(close(led_s(10.0, 3.0), 10.5));
        assert!(close(led_s(10.0, -3.0), 9.5));
        assert_eq!(led(at(0.2), -0.5), Duration::ZERO);
        assert!((Prefs::default().lyrics_lead - nikgob_view::LYRICS_LEAD).abs() < 1e-6);
        // A line starting at 10 s is current 0.1 s before the voice.
        let lines = [line(5.0, Some(9.0), "раз"), line(10.0, Some(12.0), "два")];
        let items = display_lines(&lines);
        let cur = |pos| current_item(&lines, &items, pos).map(|i| items[i]);
        assert_eq!(cur(led(at(9.95), 0.1)), Some(DisplayLine::Line(1)));
        assert_ne!(cur(led(at(9.95), 0.0)), Some(DisplayLine::Line(1)));
    }

    #[test]
    fn intro_waits_for_the_first_main_voice_and_uses_its_side() {
        let mut first = line(2.5, None, "первый");
        first.duet = true;
        assert_eq!(interludes(&[first]), [(0.0, 2.5, true)]);
        assert!(interludes(&[line(2.49, None, "первый")]).is_empty());
    }

    #[test]
    fn gaps_use_known_end_or_speaking_pace() {
        assert_eq!(interludes(&[line(0.0, Some(2.0), "a"), line(6.0, None, "b")]), [(2.0, 6.0, false)]);
        assert_eq!(interludes(&[line(0.0, None, "a"), line(6.0, None, "b")]), [(0.8, 6.0, false)]);
        assert!(interludes(&[line(0.0, Some(2.0), "a"), line(5.99, None, "b")]).is_empty());
    }

    #[test]
    fn explicit_empty_stamps_replace_computed_gaps_and_intros() {
        let lines = [line(0.0, Some(1.0), "a"), line(2.0, None, ""), line(3.0, None, " "), line(10.0, None, "b")];
        assert_eq!(interludes(&lines), [(2.0, 10.0, false)]);
        assert_eq!(display_lines(&lines).len(), 3);
        assert_eq!(interludes(&[line(0.0, None, ""), line(3.0, None, "a")]), [(0.0, 3.0, false)]);
    }

    #[test]
    fn background_singing_cancels_waiting_even_if_it_started_before_the_gap() {
        let mut bg = line(1.0, Some(4.0), "эхо");
        bg.background = true;
        let lines = [line(0.0, Some(2.0), "a"), bg, line(10.0, None, "b")];
        assert!(interludes(&lines).is_empty());
        assert_eq!(display_lines(&lines), [DisplayLine::Line(0), DisplayLine::Line(1), DisplayLine::Line(2)]);
        let mut lines = lines;
        lines[1].start = Duration::from_secs(3);
        lines[1].end = None;
        assert!(interludes(&lines).is_empty());
    }

    #[test]
    fn short_explicit_breaks_and_trailing_empty_stamps_have_no_slot() {
        assert!(interludes(&[line(0.0, None, "a"), line(1.0, None, ""), line(2.49, None, "b"), line(3.0, None, "")]).is_empty());
        assert!(display_lines(&[]).is_empty());
        assert!(display_lines(&[line(0.0, None, "")]).is_empty());
    }

    #[test]
    fn interludes_own_the_scroll_anchor_until_singing_starts() {
        let lines = [line(3.0, Some(4.0), "a"), line(10.0, None, "b")];
        let items = display_lines(&lines);
        assert_eq!(current_item(&lines, &items, Duration::ZERO), Some(0));
        assert_eq!(current_item(&lines, &items, Duration::from_secs(3)), Some(1));
        assert_eq!(current_item(&lines, &items, Duration::from_secs(4)), Some(2));
        assert_eq!(current_item(&lines, &items, Duration::from_secs(10)), Some(3));
    }

    #[test]
    fn clicking_waiting_skips_to_one_second_before_singing() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs(1);
        state.lyrics.lines = vec![line(3.0, None, "Свет из окна")];
        // Without soft edges the line rests at the anchor of the part above
        // the control's row.
        state.prefs.lyrics_soft_edges = false;
        let mut target = pos2(0.0, 0.0);
        let mut commands = Vec::new();
        for frame in 0..3 {
            let events = if frame == 0 { Vec::new() } else {
                vec![egui::Event::PointerMoved(target), egui::Event::PointerButton {
                    pos: target, button: egui::PointerButton::Primary, pressed: frame == 1, modifiers: egui::Modifiers::NONE,
                }]
            };
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0))),
                time: Some(frame as f64 * 0.05), events, ..Default::default()
            }, |ui| {
                // Short lines are centred in mega mode: aim at the middle.
                target = pos2(ui.cursor().left() + ui.available_width() * 0.5,
                    ui.cursor().top() + (ui.available_height() - Size::Xs.height() - CONTROL_GAP) * MEGA.anchor);
                show(ui, &state, 0.0, true, &mut commands);
            });
            output.textures_delta.clear();
        }
        assert!(commands.iter().any(|c| matches!(c, Command::Seek(t) if *t == Duration::from_secs(2))));
    }

    /// A tap on a touch screen: the pointer is gone in the frame the press
    /// ends, so nothing is hovered then. The tap still seeks (it was lost:
    /// the view stayed where it was and went back to the current line).
    #[test]
    fn a_tap_seeks_though_the_pointer_is_gone_when_it_ends() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs(1);
        state.lyrics.lines = vec![line(3.0, None, "Свет из окна")];
        state.prefs.lyrics_soft_edges = false;
        let mut target = pos2(0.0, 0.0);
        let mut commands = Vec::new();
        for frame in 0..3 {
            let button = |pressed| egui::Event::PointerButton {
                pos: target, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            };
            let events = match frame {
                0 => Vec::new(),
                1 => vec![egui::Event::PointerMoved(target), button(true)],
                _ => vec![button(false), egui::Event::PointerGone],
            };
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0))),
                time: Some(frame as f64 * 0.05), events, ..Default::default()
            }, |ui| {
                target = pos2(ui.cursor().left() + ui.available_width() * 0.5,
                    ui.cursor().top() + (ui.available_height() - Size::Xs.height() - CONTROL_GAP) * MEGA.anchor);
                show(ui, &state, 0.0, true, &mut commands);
            });
            output.textures_delta.clear();
        }
        assert!(commands.iter().any(|c| matches!(c, Command::Seek(t) if *t == Duration::from_secs(2))), "{commands:?}");
    }

    /// Scrolled down by the wheel, then a click on a line far below: the
    /// song seeks to that line (the view stayed or went back up instead).
    #[test]
    fn a_click_after_a_wheel_scroll_seeks_to_the_line_under_it() {
        let seek_after = |scrolled: bool| -> f32 {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            crate::typography::install_fonts(&mut fonts);
            ctx.set_fonts(fonts);
            let mut state = ViewState::default();
            state.player.status = Status::Paused;
            state.player.position = Duration::from_millis(1500);
            state.lyrics.lines = (0..20).map(|i| line(1.0 + 4.0 * i as f32, Some(4.0 + 4.0 * i as f32), &format!("Строка номер {i}"))).collect();
            state.prefs.lyrics_soft_edges = false;
            let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0));
            let (over, below) = (pos2(320.0, 350.0), pos2(200.0, 560.0));
            let mut commands = Vec::new();
            for frame in 0..40 {
                let button = |pressed| egui::Event::PointerButton {
                    pos: below, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
                };
                let wheel = egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -120.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::NONE,
                };
                let events = match frame {
                    1 => vec![egui::Event::PointerMoved(over)],
                    2..=4 if scrolled => vec![wheel],
                    30 => vec![egui::Event::PointerMoved(below), button(true)],
                    31 => vec![button(false)],
                    _ => Vec::new(),
                };
                let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), time: Some(frame as f64 / 60.0), events, ..Default::default() }, |ui| {
                    show(ui, &state, 0.0, true, &mut commands);
                });
                output.textures_delta.clear();
            }
            let seeks: Vec<f32> = commands.iter().filter_map(|c| if let Command::Seek(t) = c { Some(t.as_secs_f32()) } else { None }).collect();
            assert_eq!(seeks.len(), 1, "{commands:?}");
            // A line's start.
            assert!(((seeks[0] - 1.0) / 4.0).fract().abs() < 1e-3, "{seeks:?}");
            seeks[0]
        };
        let (still, scrolled) = (seek_after(false), seek_after(true));
        // The same point on the screen holds a line further down once the
        // text was scrolled up under it.
        assert!(scrolled >= still + 8.0, "{still} {scrolled}");
    }

    /// Where a click seeks: a line's start; a wait's end less a second.
    #[test]
    fn a_click_seeks_to_its_line_or_just_before_the_singing() {
        let lines = [line(3.0, Some(4.0), "a"), line(10.0, None, "b")];
        let items = display_lines(&lines);
        let at = |item: DisplayLine| seek_to(&lines, item);
        assert_eq!(at(DisplayLine::Line(1)), Duration::from_secs(10));
        assert_eq!(at(items[0]), Duration::from_secs(2), "the intro's wait");
        assert_eq!(at(DisplayLine::Interlude { start: Duration::ZERO, end: Duration::from_millis(500), duet: false }), Duration::ZERO);
    }

    /// Scrolled down by hand to a line far below and clicked: the line is
    /// the current one from the click on, while the seek has not come back
    /// (the position still the old one, above or below), while a lead behind
    /// the sound paints a little before it (as long as it takes, paused
    /// too), and at most 3 s when nothing plays. Reaching the line lets go;
    /// another text drops it.
    #[test]
    fn a_clicked_line_is_current_until_its_seek_comes_back() {
        let lines = [line(3.0, Some(4.0), "a"), line(10.0, Some(12.0), "b"), line(60.0, None, "c")];
        let items = display_lines(&lines);
        let of = |item: DisplayLine| items.iter().position(|it| *it == item).unwrap();
        let (a, b, c) = (of(DisplayLine::Line(0)), of(DisplayLine::Line(1)), of(DisplayLine::Line(2)));
        let by = |secs: f32| current_item(&lines, &items, Duration::from_secs_f32(secs));
        let at = |secs: f32| Duration::from_secs_f32(secs);
        let none = Duration::ZERO;
        let jump = Jump { item: DisplayLine::Line(2), to: at(60.0), at: 100.0 };
        // Before the seek comes back the position is still at «b».
        assert_eq!(by(10.5), Some(b));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(10.5), at(10.5), none, 100.02), (Some(c), Some(jump)));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(10.6), at(10.6), none, 102.9), (Some(c), Some(jump)));
        // Back: the position says it, the jump is done.
        assert_eq!(with_jump(Some(jump), &lines, &items, by(60.0), at(60.0), none, 100.1), (Some(c), None));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(60.3), at(60.3), none, 100.1), (Some(c), None));
        // Nothing plays: let go after 3 s.
        assert_eq!(with_jump(Some(jump), &lines, &items, by(10.6), at(10.6), none, 103.1), (Some(b), None));
        // A lead 0.4 s behind the sound paints at 59.7 s: still «c», paused
        // for as long as it takes; without such a lead 59.7 is not «c».
        let back = Duration::from_millis(400);
        assert_eq!(with_jump(Some(jump), &lines, &items, by(59.7), at(59.7), back, 200.0), (Some(c), Some(jump)));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(59.7), at(59.7), none, 200.0), (by(59.7), None));
        // Up: clicked «a» while «c» plays, far past it.
        let up = Jump { item: DisplayLine::Line(0), to: at(3.0), at: 50.0 };
        assert_eq!(with_jump(Some(up), &lines, &items, by(61.0), at(61.0), none, 50.05), (Some(a), Some(up)));
        assert_eq!(with_jump(Some(up), &lines, &items, by(3.0), at(3.0), none, 50.2), (Some(a), None));
        // Another text (the song changed): dropped at once.
        let other = [line(1.0, None, "x"), line(2.0, None, "y"), line(5.0, None, "z")];
        let other_items = display_lines(&other);
        let by_other = current_item(&other, &other_items, at(0.5));
        assert_eq!(with_jump(Some(jump), &other, &other_items, by_other, at(0.5), none, 100.05), (by_other, None));
        assert_eq!(with_jump(None, &lines, &items, by(10.5), at(10.5), none, 1.0), (Some(b), None));
    }

    /// Scrolled by hand, the text stays where the user left it as the song
    /// moves to its next lines (it jumped a line at a time under the
    /// pointer, and a click took the line that had moved there); the hold
    /// ends and it eases back; a click ends it at once.
    #[test]
    fn a_scrolled_text_stays_put_while_the_song_moves_on() {
        let range = (-5000.0, 5000.0);
        // The wheel down 300: the text moves up, 300 past the current line.
        let m = Manual::default().step(100.0, -300.0, 10.0, 4.0, range);
        assert_eq!((m.offset, m.until, m.from), (300.0, 14.0, 100.0));
        assert!(m.on(10.0));
        // The next line becomes current (follow moves by 60): the text
        // stays at 400 all the same.
        let m = m.step(160.0, 0.0, 11.0, 4.0, range);
        assert_eq!(160.0 + m.shift(11.0), 400.0);
        let m = m.step(230.0, 0.0, 12.0, 4.0, range);
        assert_eq!(230.0 + m.shift(12.0), 400.0);
        // Scrolled on from there.
        let m = m.step(230.0, -50.0, 12.5, 4.0, range);
        assert_eq!(230.0 + m.shift(12.5), 450.0);
        assert_eq!(m.until, 16.5);
        // The hold is over: back to following, eased over half a second.
        let m = m.step(300.0, 0.0, 16.75, 4.0, range);
        assert!(!m.on(16.75));
        assert!(m.shift(16.75) > 0.0 && m.shift(16.75) < m.offset);
        assert_eq!(m.shift(17.0), 0.0);
        // The text's ends hold it; an empty range never panics.
        assert_eq!(Manual::default().step(0.0, -9000.0, 1.0, 4.0, (-50.0, 80.0)).offset, 80.0);
        assert_eq!(Manual::default().step(0.0, 9000.0, 1.0, 4.0, (-50.0, 80.0)).offset, -50.0);
        assert_eq!(Manual::default().step(0.0, 0.0, 1.0, 4.0, (30.0, 0.0)).offset, 0.0);
        // A click puts it back to following at once.
        let clicked = Manual::default();
        assert!(!clicked.on(12.0));
        assert_eq!(clicked.shift(12.0), 0.0);
    }

    fn with_painter(test: impl FnOnce(&egui::Painter)) {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut test = Some(test);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            test.take().unwrap()(ui.painter());
        });
        output.textures_delta.clear();
    }

    fn three_rows(painter: &egui::Painter, text: &str) -> Arc<Breaks> {
        (200..900).step_by(10).find_map(|width| {
            let b = fixed_breaks(painter, egui::Id::new("test-breaks"), text, width as f32, 44.0);
            (b.forced.split('\n').count() == 3).then_some(b)
        }).expect("fixture must wrap into three rows")
    }

    /// A swapped text with two more lines above the one being sung, while
    /// the view is moving to it: moved with that line, the text stays where
    /// it is drawn and keeps its speed, and only the rest of the way is
    /// animated.
    #[test]
    fn a_swapped_text_stays_where_it_is_drawn_and_keeps_its_speed() {
        use nikgob_motion::Animated;
        let old_tops = [0.0, 60.0, 120.0, 180.0];
        let old_slots = [10, 11, 12, 13];
        // The view moves from line 1 to line 2 (scroll = top − anchor 100).
        let mut scroll = Animated::new(60.0_f32 - 100.0, Spring::new(1.0, 300.0));
        scroll.set_target(120.0 - 100.0, 0.0);
        let now = 0.08;
        let drawn = old_tops[2] - scroll.value(now);
        let speed = scroll.velocity(now);
        // Two new lines come in before it; its slot stays with it.
        let new_tops = [0.0, 60.0, 120.0, 180.0, 240.0, 300.0];
        let new_slots = [20, 21, 10, 11, 12, 13];
        let moved = held_moved(Some((old_slots[2], old_tops[2])), &new_slots, &new_tops).unwrap();
        assert_eq!(moved, 120.0);
        scroll.shift(moved);
        assert!((new_tops[4] - scroll.value(now) - drawn).abs() < 1e-3);
        assert!((scroll.velocity(now) - speed).abs() < 1e-2);
        // It goes on to the line's place in the new text, on the same spring.
        scroll.set_target(new_tops[4] - 100.0, now);
        assert!((new_tops[4] - scroll.value(now) - drawn).abs() < 1e-3);
        assert!((scroll.velocity(now) - speed).abs() < 1e-2);
        assert_eq!(scroll.value(5.0), new_tops[4] - 100.0);
        // A line not in the new text moves nothing.
        assert_eq!(held_moved(Some((99, 0.0)), &new_slots, &new_tops), None);
        assert_eq!(held_moved(None, &new_slots, &new_tops), None);
    }

    /// At whole distances a line rests as bright as it always did; between
    /// them (the current line changing) its brightness moves without a step.
    #[test]
    fn resting_brightness_moves_without_steps_as_the_current_line_changes() {
        for style in [Style::Karaoke, Style::Focus, Style::Wave] {
            for dist in -12i32..=12 {
                let far = dist.unsigned_abs().min(8) as f32;
                let old = match style {
                    Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
                    _ => (if dist < 0 { 0.5 } else { 0.42 }) * 0.86f32.powf((far - 1.0).max(0.0)),
                };
                assert!((rest_brightness(style, dist as f32) - old).abs() < 1e-6, "{style:?} {dist}");
            }
            let mut was = rest_brightness(style, -10.0);
            for k in 1..=2000 {
                let now = rest_brightness(style, -10.0 + k as f32 * 0.01);
                assert!((now - was).abs() < 0.006, "{style:?} at {}", -10.0 + k as f32 * 0.01);
                was = now;
            }
        }
    }

    /// A line growing from one resting size to the other: its height and
    /// rows move a little every frame (never by a rounded row height's
    /// whole pixel at once), and it starts and ends on the resting layouts.
    #[test]
    fn a_growing_line_moves_evenly_and_meets_its_resting_layouts() {
        with_painter(|painter| {
            let b = three_rows(painter, "и слова, что не сказаны вслух, звучат громче всего остального, пока город считает шаги до утра");
            let rest = (32.0 * 1.1, 44.0 * 1.1);
            let (from, to) = (fixed_layout(painter, &b.forced, rest.0), fixed_layout(painter, &b.forced, rest.1));
            let per_step = (to.rect.height() - from.rect.height()) / 200.0;
            let mut was = from.clone();
            for step in 1..200 {
                let t = step as f32 / 200.0;
                let g = moving_layout(painter, &b.forced, rest, t);
                assert_eq!(g.rows.len(), 3);
                assert!((g.rect.height() - was.rect.height() - per_step).abs() < 1e-3, "t={t}");
                for r in 0..3 {
                    let moved = g.rows[r].pos.y - was.rows[r].pos.y;
                    let even = (to.rows[r].pos.y - from.rows[r].pos.y) / 200.0;
                    assert!((moved - even).abs() < 1e-3, "t={t} row {r}");
                }
                was = g;
            }
            let near = |t: f32| moving_layout(painter, &b.forced, rest, t);
            for (g, rest) in [(near(1e-4), &from), (near(1.0 - 1e-4), &to)] {
                assert!((g.rect.height() - rest.rect.height()).abs() < 0.01);
                assert!((g.rect.width() - rest.rect.width()).abs() < 0.05);
            }
        });
    }

    #[test]
    fn forced_rows_stay_identical_across_all_animated_sizes_and_invalidate_on_width() {
        with_painter(|painter| {
            for text in [
                "и слова, что не сказаны вслух, звучат громче всего остального, пока город считает шаги до утра",
                "The words we never spoke are louder than the city and the light that waits outside our window tonight",
            ] {
                let b = three_rows(painter, text);
                let rows: Vec<_> = b.forced.split('\n').map(str::to_owned).collect();
                for step in 0..=120 {
                    let g = fixed_layout(painter, &b.forced, 32.0 + step as f32 * 0.1);
                    assert_eq!(g.rows.len(), 3);
                    assert_eq!(g.rows.iter().map(|r| r.row.glyphs.iter().map(|gl| gl.chr).collect::<String>()).collect::<Vec<_>>(), rows);
                    assert_eq!(glyph_count(&g), text.chars().count());
                }
                let wide = fixed_breaks(painter, egui::Id::new("test-breaks"), text, 4000.0, 44.0);
                assert_eq!(wide.forced, text);
                let changed = fixed_breaks(painter, egui::Id::new("test-breaks"), "Другой текст", 4000.0, 44.0);
                assert_eq!(changed.forced, "Другой текст");
                let scaled = fixed_breaks(painter, egui::Id::new("test-breaks"), "Другой текст", 4000.0, 44.0 * 1.4);
                assert!(scaled.height > changed.height);
            }
        });
    }

    #[test]
    fn word_synced_three_row_line_maps_every_glyph_to_the_same_letter() {
        with_painter(|painter| {
            let text = "Свет из окна ложится на пол пока город считает наши шаги до утра и слова звучат громче всего";
            let mut l = line(0.0, Some(20.0), text);
            l.words = text.split_whitespace().enumerate().map(|(i, word)| nikgob_view::LyricWord {
                start: Duration::from_secs(i as u64), end: Duration::from_secs(i as u64 + 1), text: format!("{word} "),
            }).collect();
            let b = three_rows(painter, text);
            let mut expected = Vec::new();
            for (i, word) in l.words.iter().enumerate() {
                let n = word.text.chars().filter(|c| !c.is_whitespace()).count();
                for _ in 0..n {
                    expected.push(Some(i));
                }
            }
            for size in [32.0, 38.0, 44.0] {
                let g = fixed_layout(painter, &b.forced, size);
                let timing = Timing::new(&l, &g).unwrap();
                assert_eq!(g.rows.len(), 3);
                assert_eq!(timing.glyphs.len(), glyph_count(&g));
                let mut letters = expected.iter();
                for (gl, time) in g.rows.iter().flat_map(|r| &r.row.glyphs).zip(&timing.glyphs) {
                    if gl.chr.is_whitespace() {
                        assert_eq!(*time, None);
                    } else {
                        assert_eq!(Some(time), letters.next());
                    }
                }
                assert!(letters.next().is_none());
                let line = Rect::from_min_size(pos2(0.0, 0.0), g.size());
                let pace = Pace::of(&Prefs::default());
                karaoke(painter, &g, line, 80.0, pace.edge(size), Color32::GRAY, Color32::WHITE);
                // Words a second each, one after another: one front
                // through the three rows.
                let look = Look { style: Style::Karaoke, by: By::Letters };
                let sung = Timing::of(&l, &g, look, pace, size).unwrap();
                assert_eq!(sung.front.runs.len(), 1);
                assert!(sung.front.of.iter().zip(&sung.glyphs).all(|(run, s)| run.is_some() == s.is_some()));
                timed_line(painter, &g, line, &sung, Duration::from_secs(4), 1.0, 1.0, size, look, Color32::GRAY, Color32::WHITE, &Prefs::default(), None);
            }
        });
    }

    #[test]
    fn vowels_weigh_more_than_consonants_and_kana_evenly() {
        for v in ['а', 'Я', 'ё', 'o', 'É', 'y', 'ω'] {
            assert_eq!(letter_weight(v), 2.5, "{v}");
        }
        for c in ['д', 'Ж', 'й', 'k', 'S', '7'] {
            assert_eq!(letter_weight(c), 1.0, "{c}");
        }
        for m in ['-', '\'', ',', 'ь', 'Ъ'] {
            assert!(letter_weight(m) < 1.0, "{m}");
        }
        // Kana, hangul and hanzi are syllables already: even shares.
        assert!(['か', 'ナ', '한', '中'].iter().all(|c| letter_weight(*c) == 1.0));
    }

    /// A word with a held last vowel: the consonants go by quickly and the
    /// vowels take the time, the word still starting and ending on its own
    /// times, each letter right after the one before.
    #[test]
    fn letters_share_their_words_time_by_weight() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "здесь домой");
            l.words = vec![word(1.0, 1.6, "здесь "), word(2.0, 3.0, "домой")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let times: Vec<(f32, f32)> = t.letter_times.iter().flatten().copied().collect();
            assert_eq!(times.len(), 10);
            let (a, b) = times.split_at(5);
            for (word, (start, end)) in [(a, (1.0, 1.6)), (b, (2.0, 3.0))] {
                assert_eq!(word[0].0, start);
                assert_eq!(word[4].1, end);
                for pair in word.windows(2) {
                    assert!(close(pair[0].1, pair[1].0), "{pair:?}");
                }
            }
            // «домой»: 1 + 2.5 + 1 + 2.5 + 1 = 8 shares of a second.
            let len = |(a, b): (f32, f32)| b - a;
            assert!(close(len(b[0]), 1.0 / 8.0) && close(len(b[1]), 2.5 / 8.0) && close(len(b[4]), 1.0 / 8.0));
            assert!(len(b[3]) > 2.0 * len(b[2]));
            // The karaoke front by letters, on the times exactly, crosses
            // the «д» by its share, not by a fifth.
            let k = Timing::build(&l, &g, Look { style: Style::Karaoke, by: By::Letters }, 0.0, Ease::EXACT).unwrap();
            assert_eq!(k.front.runs.len(), 2, "a pause parts the words");
            let at = |s: f32| k.front.runs[1].way(f64::from(s));
            let x = |i: usize| t.xs[i];
            let d = 6; // «д» after «здесь» and the space
            assert!((at(2.0 + 0.5 / 8.0) - (x(d).0 + x(d).1) / 2.0).abs() < 0.5);
            assert!((at(2.0 + 1.0 / 8.0) - x(d).1).abs() < 0.5);
            assert!((at(2.0 + 2.25 / 8.0) - (x(d + 1).0 + x(d + 1).1) / 2.0).abs() < 0.5);
            let mut was = 0.0;
            for ms in 1000..3000 {
                let h = at(ms as f32 / 1000.0);
                assert!(h >= was - 1e-3, "{ms}");
                was = h;
            }
        });
    }

    #[test]
    fn glued_syllables_share_a_word_and_the_front_waits_in_gaps() {
        with_painter(|painter| {
            let mut l = line(2.0, Some(8.0), "Hello world");
            l.words = [(2.0, 3.0, "Hel"), (4.0, 6.0, "lo "), (7.0, 8.0, "world")].map(|(s, e, text)| nikgob_view::LyricWord {
                start: Duration::from_secs_f32(s), end: Duration::from_secs_f32(e), text: text.into(),
            }).to_vec();
            let g = fixed_layout(painter, &l.text, 44.0);
            let t = Timing::of(&l, &g, Look { style: Style::Karaoke, by: By::Letters }, Pace::of(&Prefs::default()), 44.0).unwrap();
            assert_eq!(t.words, [0, 2]);
            assert_eq!(t.syllables.iter().map(|s| s.word).collect::<Vec<_>>(), [0, 0, 1]);
            // A second of silence parts the pieces, each with a front of
            // its own: in a gap the piece sung is lit whole and the next
            // not begun, no letter left half lit (also inside a word).
            let front = &t.front;
            assert_eq!(front.runs.len(), 3);
            let heads = |p: f64| [front.head(0, p), front.head(1, p), front.head(2, p)];
            let (none, all) = (f32::NEG_INFINITY, f32::INFINITY);
            assert_eq!(heads(1.0), [none; 3]);
            assert_eq!(heads(3.6), [all, none, none]);
            assert_eq!(heads(6.6), [all, all, none]);
            assert_eq!(heads(9.0), [all; 3]);
            // Inside «lo» its front is in it.
            let h = front.head(1, 5.0);
            assert!(h > t.syllables[1].from && h < t.syllables[1].to + front.edge, "{h}");
            // It comes in and goes out with no pop: just sung, the edge has
            // only begun on the first letter; about to rest, it is past the
            // last.
            let first = front.head(0, 2.02);
            assert!(first > t.syllables[0].from && first < t.syllables[0].from + 0.3 * front.edge, "{first}");
            let (_, rest) = front.runs[0].times();
            let last = front.head(0, rest - 0.004);
            assert!(last > t.syllables[0].to + 0.95 * front.edge, "{last}");
        });
    }

    #[test]
    fn line_synced_lines_are_paced_by_letters_inside_the_gap() {
        // No end: a speaking pace, never past the next line.
        let l = line(2.0, None, "ab cdef");
        let p = paced_words(&l, Some(Duration::from_secs(10)));
        assert_eq!(p.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["ab ", "cdef"]);
        let span = speaking_secs("ab cdef", 8.0);
        assert_eq!(p.words[0].start, Duration::from_secs(2));
        assert!((p.words[0].end.as_secs_f32() - (2.0 + span / 3.0)).abs() < 1e-3, "2 of 6 letters");
        assert!((p.words[1].end.as_secs_f32() - (2.0 + span)).abs() < 1e-3);
        // With an end the whole line is used.
        let p = paced_words(&line(2.0, Some(8.0), "ab cdef"), None);
        assert!((p.words[1].end.as_secs_f32() - 8.0).abs() < 1e-3);
    }

    #[test]
    fn paused_waiting_does_not_poll_after_its_springs_settle() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs(1);
        state.lyrics.lines = vec![line(3.0, None, "Свет из окна")];
        let mut delay = Duration::ZERO;
        for i in 0..20 {
            let mut output = ctx.run_ui(egui::RawInput { time: Some(i as f64 * 0.5), ..Default::default() }, |ui| {
                show(ui, &state, 0.8, true, &mut Vec::new());

            });
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            output.textures_delta.clear();
        }
        assert!(delay > Duration::from_secs(1), "paused waiting kept requesting frames: {delay:?}");
    }

    #[test]
    fn close_lines_do_not_panic_and_keep_the_pace_inside_the_gap() {
        // Lines 0.5 s apart (and duplicate stamps) used to panic in `clamp`.
        assert!((speaking_secs("вторая быстро", 0.5) - 0.46).abs() < 1e-4);
        assert_eq!(speaking_secs("a", 0.0), 0.3);
        assert_eq!(speaking_secs("a", 6.0), 0.8);
        assert!((speaking_secs("слова звучат громче всего", 6.0) - (0.4 + 22.0 * 0.066)).abs() < 1e-4);
    }

    #[test]
    fn manual_scroll_holds_then_returns_smoothly_and_can_restart_mid_return() {
        assert_eq!(returning_offset(100.0, 4.0, 3.0), 100.0);
        assert_eq!(returning_offset(100.0, 4.0, 4.0), 100.0);
        assert_eq!(returning_offset(100.0, 4.0, 4.25), 50.0);
        assert_eq!(returning_offset(100.0, 4.0, 4.5), 0.0);
        let scrolled = returning_offset(100.0, 4.0, 4.25) + 10.0;
        assert_eq!(returning_offset(scrolled, 8.25, 4.25), 60.0);
    }

    #[test]
    fn wave_crests_round_and_clamp_without_folding_thick_strokes() {
        assert_eq!(wave_geometry(2.0, 13.0, 3.0, 0.0, 1.0), (3.0, 0.0));
        assert_eq!(wave_geometry(6.49, 13.0, 3.0, 0.0, 1.0).0, 6.0);
        assert_eq!(wave_geometry(6.5, 13.0, 3.0, 0.0, 1.0).0, 7.0);
        assert_eq!(wave_geometry(20.0, 13.0, 3.0, 0.0, 1.0).0, 16.0);
        // The default thin ring (6 crests) gets a wave about half its stroke.
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0, 1.0).1 >= 1.5);
        // «Высота волн» raises the cap with it (up to folding).
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0, 2.0).1 > wave_geometry(6.0, 13.0, 3.0, 100.0, 1.0).1 * 1.5);
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0, 0.5).1 < wave_geometry(6.0, 13.0, 3.0, 100.0, 1.0).1);
        for radius in [13.0, 16.0, 24.0] {
            for stroke in [3.0, 6.0, 9.0] {
                for count in 3..=16 {
                    let (waves, amp) = wave_geometry(count as f32, radius, stroke, 100.0, 1.0);
                    assert!(amp <= std::f32::consts::TAU * radius / waves * 0.12);
                    assert!(wave_geometry(count as f32, radius, stroke, 100.0, 2.0).1 <= std::f32::consts::TAU * radius / waves * 0.24);
                    // Exact polar curvature at a crest and a trough stays
                    // under the stroke's half-width limit (no folding).
                    for (r, d2) in [(radius + amp, -amp * waves * waves), (radius - amp, amp * waves * waves)] {
                        let curvature: f32 = (r * r - r * d2) / (r * r * r);
                        assert!(curvature.abs() <= 2.0 / stroke + 1e-4, "R={radius} s={stroke} n={waves} A={amp}");
                    }
                    assert!(((waves * std::f32::consts::TAU).sin()).abs() < 1e-4);
                }
            }
        }
    }

    /// The meshes a wait paints at `pos` of a ten-second wait, and its texts.
    fn wait_shapes(ctx: &egui::Context, prefs: &Prefs, pos: f32, appear: f32) -> (Vec<Arc<egui::epaint::Mesh>>, usize) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            waiting(ui.painter(), Rect::from_min_size(pos2(20.0, 20.0), vec2(56.0, 42.0)), 1.0,
                Duration::ZERO, Duration::from_secs(10), Duration::from_secs_f32(pos), appear, false, 1.0, 1.0, prefs);
        });
        output.textures_delta.clear();
        let meshes = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => Some(mesh.clone()), _ => None,
        }).collect();
        let texts = output.shapes.iter().filter(|shape| matches!(shape.shape, egui::Shape::Text(_))).count();
        (meshes, texts)
    }

    #[test]
    fn waiting_strokes_are_opaque_and_countdown_respects_reduced_motion() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        for reduced_motion in [false, true] {
            let mut theme = Theme::get(&ctx);
            theme.reduced_motion = reduced_motion;
            theme.install(&ctx);
            for form in 0..4 {
                let prefs = Prefs { lyrics_gap_style: form, lyrics_countdown: false, ..Prefs::default() };
                // Mid-wait: the track and the indicator, each one opaque
                // mesh (a solid core inside a clear rim), as wide as the form.
                let (meshes, _) = wait_shapes(&ctx, &prefs, 5.0, 1.0);
                assert_eq!(meshes.len(), 2);
                for mesh in &meshes {
                    assert!(mesh.vertices.iter().all(|v| v.color.a() == 255 || v.color.a() == 0));
                    let core = mesh.vertices[1].pos.distance(mesh.vertices[2].pos);
                    assert!((core - if form >= 2 { 5.0 } else { 2.0 }).abs() < 0.05, "{form}: {core}");
                }
                // Leaving: one closed ring (no ends) that fades as one.
                let (meshes, _) = wait_shapes(&ctx, &prefs, 9.8, 1.0);
                assert_eq!(meshes.len(), 1);
                assert!(meshes[0].vertices.iter().all(|v| v.color.a() < 255));
                assert_eq!(meshes[0].indices.len(), meshes[0].vertices.len() / 4 * 18, "a ring has no caps");
                assert!(wait_shapes(&ctx, &prefs, 10.0, 1.0).0.is_empty());
                // Before it has come, nothing.
                assert!(wait_shapes(&ctx, &prefs, 5.0, 0.0).0.is_empty());
            }
            let prefs = Prefs { lyrics_countdown: true, ..Prefs::default() };
            for remaining in [2.95, 2.5, 1.5, 0.5] {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    waiting(ui.painter(), Rect::from_min_size(pos2(20.0, 20.0), vec2(56.0, 42.0)), 1.0,
                        Duration::ZERO, Duration::from_secs(10), Duration::from_secs_f32(10.0 - remaining),
                        1.0, false, 0.0, 1.0, &prefs);
                });
                output.textures_delta.clear();
                let texts: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text), _ => None,
                }).collect();
                assert_eq!(texts.len(), 1);
                let text = texts[0];
                assert_eq!(text.galley.job.text, format!("{:.0}", remaining.ceil()));
                let size = text.galley.job.sections[0].format.font_id.size;
                if reduced_motion || remaining != 2.95 { assert_eq!(size, 28.0); }
                else { assert!(size > 28.0); }
                if remaining <= 2.5 {
                    assert!(!output.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::Mesh(_))));
                }
            }
        }
    }

    /// A stroke is one mesh: an open one has a half disc on each end built
    /// on the end's own vertices, a ring none; no triangle is drawn twice.
    #[test]
    fn ribbons_are_one_mesh_with_round_ends_or_closed() {
        let pts: Vec<egui::Pos2> = (0..10).map(|i| pos2(i as f32 * 3.0, (i as f32 * 0.5).sin() * 4.0)).collect();
        let egui::Shape::Mesh(open) = ribbon(&pts, 6.0, false, Color32::WHITE, 1.0) else { panic!() };
        let steps = ((3.5f32 * std::f32::consts::PI * 0.5).ceil() as usize).clamp(4, 24);
        assert_eq!(open.vertices.len(), 4 * pts.len() + 2 * (1 + 2 * (steps - 1)));
        assert_eq!(open.indices.len(), 18 * (pts.len() - 1) + 2 * 9 * steps);
        let mut seen = std::collections::HashSet::new();
        for t in open.indices.chunks(3) {
            let mut t = [t[0], t[1], t[2]];
            t.sort_unstable();
            assert!(seen.insert(t), "{t:?} twice");
        }
        // The caps reach half the width past the ends.
        let reach = open.vertices.iter().filter(|v| v.color.a() == 255).map(|v| v.pos.x).fold(f32::MIN, f32::max);
        assert!((reach - (27.0 + 2.5)).abs() < 0.6, "{reach}");
        let ring: Vec<egui::Pos2> = (0..24).map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 24.0;
            pos2(a.cos() * 20.0, a.sin() * 20.0)
        }).collect();
        let egui::Shape::Mesh(closed) = ribbon(&ring, 4.0, true, Color32::WHITE, 1.0) else { panic!() };
        assert_eq!((closed.vertices.len(), closed.indices.len()), (4 * 24, 18 * 24));
        // Too thin for its rim: fainter, not thinner than a pixel.
        let egui::Shape::Mesh(thin) = ribbon(&pts, 0.5, false, Color32::WHITE, 1.0) else { panic!() };
        assert!(thin.vertices.iter().all(|v| v.color.a() < 255));
        assert!(matches!(ribbon(&pts[..1], 4.0, false, Color32::WHITE, 1.0), egui::Shape::Noop));
    }

    /// The wait's room follows the indicator's size and its waves' height.
    #[test]
    fn the_wait_room_grows_with_the_ring() {
        let p = Prefs::default();
        assert!((p.lyrics_gap_size - 1.0).abs() < 1e-6 && (p.lyrics_gap_height - 1.0).abs() < 1e-6);
        assert_eq!(wait_height(&p), 34.0);
        assert_eq!(wait_height(&Prefs { lyrics_gap_style: 2, ..p.clone() }), 42.0);
        assert!(wait_height(&Prefs { lyrics_gap_size: 1.5, ..p.clone() }) > 50.0);
        assert!(wait_height(&Prefs { lyrics_gap_height: 2.0, ..p.clone() }) > 34.0);
        // A flat ring has no waves to make room for.
        assert_eq!(wait_height(&Prefs { lyrics_gap_style: 0, lyrics_gap_height: 2.0, ..p.clone() }), 34.0);
        // It grows in on its spring, from nothing.
        assert_eq!(ring_in(0.0), 0.0);
        assert!(ring_in(0.1) > 0.3 && ring_in(1.0) > 0.99);
    }

    /// Every glyph is drawn once and whole: the pieces a front splits it
    /// into tile its quad and its texture exactly, a uniform glyph stays
    /// one quad, the light only falls along the line, and a lift moves the
    /// whole glyph.
    #[test]
    fn inked_glyphs_are_whole_and_lit_along_the_line() {
        with_painter(|painter| {
            let g = fixed_layout(painter, "Слышишь этот ритм", 26.0);
            let quads: usize = g.rows.iter().flat_map(|r| &r.row.glyphs).filter(|gl| !gl.uv_rect.is_nothing()).count();
            let plain = inked(&g, |_| Ink::solid(Color32::WHITE, 0.0));
            assert_eq!(plain.rows[0].row.visuals.mesh.vertices.len(), 4 * quads);
            let lifted = inked(&g, |_| Ink::solid(Color32::WHITE, 3.0));
            for (a, b) in plain.rows[0].row.visuals.mesh.vertices.iter().zip(&lifted.rows[0].row.visuals.mesh.vertices) {
                assert!((a.pos.y - b.pos.y - 3.0).abs() < 1e-4 && a.pos.x == b.pos.x && a.uv == b.uv);
            }
            let front = Ink { lift: 0.0, base: Color32::BLACK, lit: Color32::WHITE, head: 70.0, feather: 9.0, rise: 0.0, grow: 0.0 };
            let lit = inked(&g, |_| front);
            let v = &lit.rows[0].row.visuals.mesh.vertices;
            assert!(v.len() > 4 * quads, "the front splits the glyphs it crosses");
            let src = &g.rows[0].row.visuals.mesh.vertices;
            let mut at = 0;
            for gl in g.rows[0].row.glyphs.iter().filter(|gl| !gl.uv_rect.is_nothing()) {
                let q = &src[gl.first_vertex as usize..gl.first_vertex as usize + 4];
                // This glyph's pieces: from its left edge to its right edge,
                // each starting where the one before ended.
                assert_eq!(v[at].pos, q[0].pos);
                let mut x = q[0].pos.x;
                loop {
                    assert_eq!(v[at].pos.x, x);
                    x = v[at + 1].pos.x;
                    assert!(x > v[at].pos.x);
                    // Brightness never rises to the right.
                    assert!(v[at + 1].color.r() <= v[at].color.r());
                    at += 4;
                    if x >= q[1].pos.x {
                        break;
                    }
                }
                assert_eq!(v[at - 3].pos, q[1].pos);
                assert_eq!(v[at - 3].uv, q[1].uv);
            }
            assert_eq!(at, v.len());
            // Lit before the front, unlit past it.
            assert_eq!(v[0].color, Color32::WHITE);
            assert_eq!(v[v.len() - 1].color, Color32::BLACK);
            // A hard front (no feather) stays hard inside the glyph it cuts.
            let hard = inked(&g, |_| Ink { feather: 0.0, ..front });
            assert!(hard.rows[0].row.visuals.mesh.vertices.iter().all(|v| v.color == Color32::WHITE || v.color == Color32::BLACK));
        });
    }

    /// Karaoke's sung words float up whole by the light at their middle (a
    /// lit glyph by all of its rise, one ahead of the front not at all, none
    /// leaning), and a swelling glyph grows about the middle of its foot.
    #[test]
    fn sung_glyphs_float_up_whole_and_a_swell_grows_from_the_foot() {
        with_painter(|painter| {
            let g = fixed_layout(painter, "Слышишь этот ритм", 26.0);
            let plain = inked(&g, |_| Ink::solid(Color32::WHITE, 0.0));
            let front = Ink { lift: 0.0, base: Color32::BLACK, lit: Color32::WHITE, head: 70.0, feather: 9.0, rise: 3.0, grow: 0.0 };
            let floated = inked(&g, |_| front);
            let pv = &plain.rows[0].row.visuals.mesh.vertices;
            let fv = &floated.rows[0].row.visuals.mesh.vertices;
            let drawn: Vec<_> = g.rows[0]
                .row
                .glyphs
                .iter()
                .zip(&floated.rows[0].row.glyphs)
                .zip(&plain.rows[0].row.glyphs)
                .filter(|((gl, _), _)| !gl.uv_rect.is_nothing())
                .map(|((_, f), p)| (f, p))
                .collect();
            let (mut lit, mut ahead) = (0, 0);
            for (i, (f, p)) in drawn.iter().enumerate() {
                let start = f.first_vertex as usize;
                let end = drawn.get(i + 1).map_or(fv.len(), |(n, _)| n.first_vertex as usize);
                let up = pv[p.first_vertex as usize].pos.y - fv[start].pos.y;
                // Every piece of the glyph at one height.
                for v in &fv[start..end] {
                    let top = (v.pos.y - fv[start].pos.y).abs() < 1e-3;
                    let bottom = (v.pos.y - fv[start + 2].pos.y).abs() < 1e-3;
                    assert!(top || bottom, "glyph {i} leans");
                }
                if p.pos.x + p.advance_width < front.head - front.feather - 2.0 {
                    assert!((up - 3.0).abs() < 1e-3, "glyph {i} lit: {up}");
                    lit += 1;
                } else if p.pos.x > front.head + 2.0 {
                    assert!(up.abs() < 1e-3, "glyph {i} ahead: {up}");
                    ahead += 1;
                } else {
                    assert!((-1e-3..=3.001).contains(&up), "glyph {i} under the front: {up}");
                }
            }
            assert!(lit > 0 && ahead > 0);
            // Grown by half: the foot's middle stays, the glyph is half as
            // wide and tall again.
            let grown = inked(&g, |_| Ink { grow: 0.5, ..Ink::solid(Color32::WHITE, 0.0) });
            let gv = &grown.rows[0].row.visuals.mesh.vertices;
            let (a, b) = (&pv[0..4], &gv[0..4]);
            assert!(((a[2].pos.x + a[3].pos.x) - (b[2].pos.x + b[3].pos.x)).abs() < 1e-3);
            assert!((a[2].pos.y - b[2].pos.y).abs() < 1e-3);
            assert!(((b[3].pos.x - b[2].pos.x) - 1.5 * (a[3].pos.x - a[2].pos.x)).abs() < 1e-3);
            assert!(((b[2].pos.y - b[0].pos.y) - 1.5 * (a[2].pos.y - a[0].pos.y)).abs() < 1e-3);
        });
    }

    /// A wave piece comes down from its crest on a spring: no step in a
    /// frame, a little past where it rests and back, settled within its
    /// fall at any jump speed.
    #[test]
    fn a_wave_piece_lands_on_a_bounce_and_settles() {
        assert_eq!(land(-0.1, 1.0), 0.0);
        assert_eq!(land(0.0, 1.0), 0.0);
        let (mut top, mut was) = (0.0f32, 0.0f32);
        for f in 1..=240 {
            let v = land(f as f32 / 240.0, 1.0);
            assert!((v - was).abs() < 0.08, "frame {f}: {was} → {v}");
            top = top.max(v);
            was = v;
        }
        assert!(top > 1.03 && top < 1.15, "a little past its rest: {top}");
        for speed in [0.5, 1.0, 2.0] {
            assert!((land(WAVE_FALL / speed, speed) - 1.0).abs() < 0.01, "{speed}");
        }
    }

    /// A timing slip (a piece timed before one written ahead of it) waits
    /// for that one: letters and words light in the order they are written.
    #[test]
    fn onsets_follow_the_written_order() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "ab cd ef");
            l.words = vec![word(1.0, 1.5, "ab "), word(0.8, 1.2, "cd "), word(2.0, 2.5, "ef")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let onsets: Vec<f32> = t.lit.iter().flatten().copied().collect();
            assert_eq!(onsets.len(), 6);
            assert!(onsets.windows(2).all(|w| w[0] <= w[1]), "{onsets:?}");
            // «cd», timed before «ab» is over, waits for it.
            assert!(close(onsets[2], 1.5) && close(onsets[3], 1.5) && close(onsets[4], 2.0), "{onsets:?}");
            assert!(t.units.windows(2).all(|w| w[0].start <= w[1].start && w[1].start <= w[1].end));
            // In order, they are the letters' own starts.
            let mut l = l;
            l.words[1] = word(1.5, 2.0, "cd ");
            let t = Timing::new(&l, &fixed_layout(painter, &l.text, 26.0)).unwrap();
            let own: Vec<f32> = t.letter_times.iter().flatten().map(|(a, _)| *a).collect();
            let lit: Vec<f32> = t.lit.iter().flatten().copied().collect();
            assert!(lit.len() == own.len() && lit.iter().zip(&own).all(|(a, b)| close(*a, *b)), "{lit:?} against {own:?}");
        });
    }

    /// The timed spans the front of `t` keeps time by under `by`: each
    /// written word by words, else each timed piece, as (start, end, from,
    /// to, its first glyph).
    fn spans(t: &Timing, by: By) -> Vec<(f32, f32, f32, f32, usize)> {
        if by != By::Words {
            return t.syllables.iter().filter(|s| s.letters > 0).map(|s| (s.start, s.end, s.from, s.to, s.glyphs.0)).collect();
        }
        let mut out: Vec<(f32, f32, f32, f32, usize)> = Vec::new();
        for (k, u) in t.glyph_units.iter().enumerate() {
            let Some(u) = *u else { continue };
            if out.len() <= u {
                out.push((t.units[u].start, t.units[u].end, t.xs[k].0, t.xs[k].1, k));
            }
            out[u].3 = t.xs[k].1;
        }
        out
    }

    /// The report: karaoke went ragged, a long syllable dragging and a short
    /// one flying by, every piece at its own speed. At every step, over
    /// Cyrillic, Latin and Japanese, and at any «Плавность»: each run's
    /// front crosses it in one motion (never back, no jump from one frame
    /// to the next at any frame rate), is in no piece before that is sung
    /// and through each within the tolerance, comes in on the first letter,
    /// and leaves the run lit whole.
    #[test]
    fn the_karaoke_front_crosses_a_run_in_one_motion_at_every_step() {
        let timed = |text: &str, pieces: &[(&str, f32, f32)]| {
            let mut l = line(pieces[0].1, Some(pieces[pieces.len() - 1].2 + 1.0), text);
            l.words = pieces.iter().map(|(t, a, b)| word(*a, *b, t)).collect();
            assert_eq!(l.words.iter().map(|w| w.text.as_str()).collect::<String>(), text);
            l
        };
        let lines = [
            timed("космоцветок домой", &[("ко", 1.0, 1.2), ("смо", 1.2, 1.5), ("цве", 1.55, 2.6), ("ток ", 2.6, 2.9), ("до", 4.0, 4.3), ("мой", 4.3, 5.0)]),
            timed("Somewhere close to me", &[("Some", 1.0, 1.18), ("where ", 1.18, 1.5), ("close ", 1.5, 2.4), ("to ", 2.45, 2.6), ("me", 2.6, 3.4)]),
            timed("夜空に光る星", &[("夜", 1.0, 1.3), ("空", 1.3, 1.5), ("に", 1.5, 1.6), ("光", 1.6, 2.4), ("る", 2.4, 2.55), ("星", 2.55, 3.6)]),
        ];
        with_painter(|painter| {
            for l in &lines {
                let g = fixed_layout(painter, &l.text, 26.0);
                for by in [By::Letters, By::Syllables, By::Words] {
                    for smooth in [0.0, 0.5, 1.0] {
                        let case = format!("«{}», {by:?}, smooth {smooth}", l.text);
                        let look = Look { style: Style::Karaoke, by };
                        let pace = Pace::of(&Prefs { lyrics_smooth: smooth, ..Prefs::default() });
                        let t = Timing::of(l, &g, look, pace, 26.0).unwrap();
                        let (front, ease) = (&t.front, Ease::of(smooth));
                        let edge = front.edge;
                        assert!((edge - pace.edge(26.0)).abs() < 1e-4 && edge > 2.0);
                        // Every letter goes with a run, the runs in order.
                        let runs: Vec<usize> = t.glyphs.iter().zip(&front.of).filter(|(s, _)| s.is_some()).map(|(_, r)| r.expect("a run")).collect();
                        assert!(runs.windows(2).all(|w| w[0] <= w[1]) && runs.last() == Some(&(front.runs.len() - 1)), "{case}");
                        for (r, run) in front.runs.iter().enumerate() {
                            let own = || t.xs.iter().zip(&front.of).filter(move |(_, o)| **o == Some(r)).map(|(x, _)| *x);
                            let (from, to) = (own().map(|x| x.0).fold(f32::MAX, f32::min), own().map(|x| x.1).fold(0.0, f32::max));
                            let (start, rest) = run.times();
                            assert_eq!((front.head(r, start), front.head(r, rest)), (f32::NEG_INFINITY, f32::INFINITY), "{case}");
                            for fps in [60.0, 142.0] {
                                let (mut was, mut seen) = (f32::NEG_INFINITY, 0);
                                let mut frame = 0;
                                loop {
                                    let p = start - 0.1 + f64::from(frame) / fps;
                                    frame += 1;
                                    let h = front.head(r, p);
                                    if h.is_finite() {
                                        if was.is_finite() {
                                            assert!(h >= was - 1e-3, "{case}, run {r} at {p:.3}: runs back");
                                            assert!(h - was < 900.0 / fps as f32, "{case}, run {r} at {p:.3}: jumps by {}", h - was);
                                        } else {
                                            assert!(seen == 0 && h < from + 900.0 / fps as f32, "{case}, run {r}: comes in at {h}, not {from}");
                                        }
                                        seen += 1;
                                    } else if was.is_finite() {
                                        assert!(h == f32::INFINITY && was > to + edge - 900.0 / fps as f32, "{case}, run {r}: leaves at {was}, not {}", to + edge);
                                    }
                                    was = h;
                                    if p > rest + 0.1 {
                                        break;
                                    }
                                }
                                assert!(seen > 5 && was == f32::INFINITY, "{case}, run {r}: {seen} frames");
                            }
                        }
                        // The middle of the edge is the front: in no piece
                        // before it is sung, through it within the
                        // tolerance after.
                        for (start, end, from, to, first) in spans(&t, by) {
                            let run = &front.runs[front.of[first].expect("a run")];
                            let before = run.at(f64::from(start));
                            assert!(before <= from + 0.05, "{case}: at {before} when the piece at {from} starts");
                            let after = run.at(f64::from(end + ease.behind()));
                            assert!(after >= to - 0.05, "{case}: at {after}, the piece to {to} ended {} s ago", ease.behind());
                        }
                    }
                }
            }
            // A pause parts the runs: in it the words sung are lit whole,
            // their space too, and the next has not begun, at any step.
            let l = &lines[0];
            let g = fixed_layout(painter, &l.text, 26.0);
            for by in [By::Letters, By::Syllables, By::Words] {
                let t = timing(l, &g, Look { style: Style::Karaoke, by });
                assert_eq!(t.front.runs.len(), 2, "{by:?}");
                for p in [3.4, 3.7, 4.0] {
                    assert_eq!((t.front.head(0, p), t.front.head(1, p)), (f32::INFINITY, f32::NEG_INFINITY), "{by:?} at {p}");
                }
                assert!(t.front.of[..11].iter().all(|r| *r == Some(0)) && t.front.of[11].is_none() && t.front.of[12..].iter().all(|r| *r == Some(1)));
            }
        });
    }

    /// «Плавность» evens the front out: the quick syllables of a line are
    /// crossed at paces closer to each other the smoother it is, a short
    /// piece between calmer ones no longer flying by; the note held long
    /// still takes its own long time, and warms up while it is held.
    #[test]
    fn smoothness_evens_the_front_out_and_a_held_note_stays_long() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(5.0), "тататам там тарарам");
            l.words = [("та", 1.0, 1.1), ("та", 1.1, 1.36), ("там ", 1.36, 1.46), ("там ", 1.5, 3.1), ("та", 3.1, 3.18), ("ра", 3.18, 3.42), ("рам", 3.42, 3.6)]
                .map(|(t, a, b)| word(a, b, t))
                .to_vec();
            let g = fixed_layout(painter, &l.text, 26.0);
            let front = |smooth: f32| {
                let pace = Pace::of(&Prefs { lyrics_smooth: smooth, ..Prefs::default() });
                Timing::of(&l, &g, Look { style: Style::Karaoke, by: By::Syllables }, pace, 26.0).unwrap()
            };
            // The fastest the front goes from frame to frame, and how long
            // it takes over the held «там».
            let measure = |t: &Timing| {
                let run = &t.front.runs[0];
                let fastest = (0..800).map(|f| (run.at(0.9 + f64::from(f + 1) / 240.0) - run.at(0.9 + f64::from(f) / 240.0)) * 240.0).fold(0.0f32, f32::max);
                let held = &t.syllables[3];
                let cross = |x: f32| (0..4000).map(|ms| 0.9 + f64::from(ms) / 1000.0).find(|p| run.at(*p) >= x).expect("crossed");
                (fastest, (cross(held.to - 1.0) - cross(held.from + 1.0)) as f32)
            };
            let (exact, calm, calmest) = (measure(&front(0.0)), measure(&front(0.5)), measure(&front(1.0)));
            assert_eq!(front(0.5).front.runs.len(), 1);
            assert!(calm.0 < 0.8 * exact.0 && calmest.0 < calm.0, "{exact:?} {calm:?} {calmest:?}");
            for held in [exact.1, calm.1, calmest.1] {
                assert!(held > 1.2 && held < 1.75, "the held note takes {held} s");
            }
            // 1.6 s over three letters is a held note; «рам» is not.
            assert_eq!(held_note(1.6, 3), 1.0);
            assert_eq!(held_note(0.18, 3), 0.0);
            assert!(held_note(0.9, 3) > 0.0 && held_note(0.9, 3) < 1.0 && held_note(2.0, 12) < 0.35);
            let mut was = 0.0;
            for ms in 1400..3600 {
                let p = ms as f32 / 1000.0;
                let (up, glow) = (held_in(1.5, p), held_in(1.5, p) * held_envelope(3.1, p));
                assert!(up >= was && glow <= up && (up - was) < 0.01, "{ms}");
                was = up;
            }
            assert!(held_in(1.5, 1.5) == 0.0 && held_in(1.5, 1.51 + HELD_IN) == 1.0 && held_envelope(3.1, 3.11 + SETTLE) == 0.0);
        });
    }

    /// Smoothed light and jumps: the light still only rises, starts half a
    /// window early and is whole a window after; at once becomes a short
    /// even ramp, not a step; a jump spreads into a lower, longer rise.
    #[test]
    fn smoothing_spreads_the_light_and_the_jump_evenly() {
        let w = 0.2;
        for rise in [0.0, 0.06, 0.3] {
            let mut was = 0.0;
            for ms in -400..900 {
                let t = ms as f32 / 1000.0;
                let v = light_over(t, rise, w);
                assert!(v >= was - 1e-5 && v <= 1.0, "{rise} {ms}");
                assert!(v - was < 0.02, "{rise} {ms}: a step");
                // The integral against a fine numeric average.
                let n = 400;
                let avg: f32 = (0..n).map(|i| light_up(t - w * 0.5 + w * (i as f32 + 0.5) / n as f32, rise)).sum::<f32>() / n as f32;
                assert!((v - avg).abs() < 0.01, "{rise} {ms}: {v} vs {avg}");
                was = v;
            }
            assert_eq!(light_over(-ONSET_LEAD - w * 0.5 - 0.001, rise, w), 0.0);
            assert!(light_over(w * 0.5 + rise * 3.0 + 0.1, rise, w) > 0.99);
        }
        assert_eq!(light_over(0.1, 0.06, 0.0), light_up(0.1, 0.06));
        // The jump: smooth, lower peak, the same rest.
        let mut was = 0.0;
        let mut top = 0.0f32;
        for ms in -300..800 {
            let v = jump_over(ms as f32 / 1000.0, 1.0, w);
            assert!((v - was).abs() < 0.08, "{ms}");
            top = top.max(v);
            was = v;
        }
        assert!(top > 1.05 && top < (0..400).map(|ms| jump_height(ms as f32 / 1000.0, 1.0)).fold(0.0, f32::max));
        assert!((jump_over(1.0, 1.0, w) - 1.0).abs() < 1e-3);
        assert_eq!(jump_over(0.05, 1.0, 0.0), jump_height(0.05, 1.0));
        assert_eq!(jump_over(-w, 1.0, w), 0.0);
    }

    /// Each glyph's colour as the lyrics view paints line `l` (alone, paused
    /// at `at` s) under `prefs`, once its springs have settled, and the lit
    /// colour.
    fn painted_glyphs(prefs: &Prefs, l: &LyricLine, at: f32) -> (Vec<Option<Color32>>, Color32) {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.prefs = Prefs { lyrics_soft_edges: false, ..prefs.clone() };
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs_f32(at);
        state.lyrics.lines = vec![l.clone()];
        let mut colours = Vec::new();
        for frame in 0..12 {
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0))),
                time: Some(frame as f64 * 0.25), ..Default::default()
            }, |ui| {
                show(ui, &state, 0.0, false, &mut Vec::new());
            });
            output.textures_delta.clear();
            colours.clear();
            for shape in &output.shapes {
                let egui::Shape::Text(text) = &shape.shape else { continue };
                if text.galley.job.text != l.text {
                    continue;
                }
                for row in &text.galley.rows {
                    for gl in &row.row.glyphs {
                        colours.push((!gl.uv_rect.is_nothing()).then(|| {
                            text.override_text_color.unwrap_or(row.row.visuals.mesh.vertices[gl.first_vertex as usize].color)
                        }));
                    }
                }
            }
        }
        (colours, Theme::get(&ctx).scheme.on_surface)
    }

    /// The report: «Подсвечивать: По словам» lit nothing under karaoke and
    /// only weakly under the wave. Every look, by letters and by words, over
    /// word, syllable and line timing (guessed or the calm fallback): the
    /// words already sung are fully lit, the ones to come are not (unless
    /// the look lights the line whole).
    #[test]
    fn sung_words_are_fully_lit_in_every_look() {
        let text = "Свет из окна ложится";
        let worded = {
            let mut l = line(1.0, Some(5.0), text);
            l.words = vec![word(1.0, 1.6, "Свет "), word(1.6, 2.0, "из "), word(2.0, 3.0, "окна "), word(3.0, 5.0, "ложится")];
            l
        };
        let syllables = {
            let mut l = line(1.0, Some(5.0), text);
            l.words = [("Св", 1.0, 1.3), ("ет ", 1.3, 1.6), ("из ", 1.6, 2.0), ("ок", 2.0, 2.5), ("на ", 2.5, 3.0), ("ло", 3.0, 3.8), ("жит", 3.8, 4.4), ("ся", 4.4, 5.0)]
                .map(|(t, a, b)| word(a, b, t)).to_vec();
            l
        };
        let lines_only = line(1.0, Some(5.0), text);
        let close = |a: Color32, b: Color32| a.to_array().iter().zip(b.to_array()).all(|(x, y)| x.abs_diff(y) <= 3);
        for (timing, l, calm) in [("word", &worded, true), ("syllable", &syllables, true), ("guessed", &lines_only, false), ("calm", &lines_only, true)] {
            for style in 0..3u8 {
                for unit in [nikgob_view::LYRICS_BY_LETTERS, nikgob_view::LYRICS_BY_SYLLABLES, nikgob_view::LYRICS_BY_WORDS] {
                    let letters = unit == nikgob_view::LYRICS_BY_LETTERS;
                    for smooth in [0.0, 0.5, 1.0] {
                        let prefs = Prefs { lyrics_style: style, lyrics_unit: unit, wave_syllables_only: calm, lyrics_smooth: smooth, ..Prefs::default() };
                        let case = format!("{timing}, style {style}, unit {unit}, smooth {smooth}");
                        let (glyphs, lit) = painted_glyphs(&prefs, l, 2.4);
                        assert_eq!(glyphs.len(), text.chars().count(), "{case}: the line was not painted with its timing");
                        // Focus by syllables or words has no fill: the word
                        // sung and the word to come are one tone, the
                        // line's, and not the lit one.
                        if style == 1 && !letters && timing != "calm" {
                            // (A guessed «Свет» has only just been sung.)
                            if timing != "guessed" {
                                assert_eq!(glyphs[0], glyphs[13], "{case}: «Свет» and «ложится» differ");
                            }
                            assert!(!close(glyphs[0].unwrap(), lit), "{case}: the line is lit whole");
                            continue;
                        }
                        // «Свет» was sung over a second ago.
                        for (k, c) in glyphs[..4].iter().enumerate() {
                            assert!(close(c.unwrap(), lit), "{case}: letter {k} is {:?}, lit is {lit:?}", c.unwrap());
                        }
                        // «ложится» is still to come.
                        let whole = (style == 1 && letters) || timing == "calm";
                        for (k, c) in glyphs[13..].iter().enumerate() {
                            assert_eq!(close(c.unwrap(), lit), whole, "{case}: letter {} is {:?}", 13 + k, c.unwrap());
                        }
                    }
                }
            }
        }
    }

    /// The painted position holds against a small step back while playing
    /// (the clock correcting itself), and follows seeks and pauses.
    #[test]
    fn the_painted_position_never_steps_back_by_a_hair() {
        let ms = Duration::from_millis;
        assert_eq!(steady_pos(Some(ms(10_000)), ms(9_800), true), ms(10_000));
        assert_eq!(steady_pos(Some(ms(10_000)), ms(10_020), true), ms(10_020));
        assert_eq!(steady_pos(Some(ms(10_000)), ms(9_000), true), ms(9_000), "a seek");
        assert_eq!(steady_pos(Some(ms(10_000)), ms(9_800), false), ms(9_800), "paused");
        assert_eq!(steady_pos(None, ms(5), true), ms(5));
        let mut last = None;
        let mut was = Duration::ZERO;
        for t in [100, 200, 180, 300, 290, 400] {
            let p = steady_pos(last, ms(t), true);
            assert!(p >= was);
            (last, was) = (Some(p), p);
        }
    }

    /// A raised letter goes onto whole pixels only after its jump is over,
    /// and off them as its line goes, each without a step.
    #[test]
    fn a_lift_eases_onto_and_off_whole_pixels() {
        assert_eq!(crisp_lift(-0.1, 1.0), 0.0, "still moving");
        assert_eq!(crisp_lift(0.0, 1.0), 0.0);
        assert_eq!(crisp_lift(CRISP, 1.0), 1.0);
        assert_eq!(crisp_lift(5.0, 0.9), 0.0, "going down with its line");
        assert_eq!(crisp_lift(5.0, 0.5), 0.0);
        let mut was = 0.0;
        for i in 0..=100 {
            let c = crisp_lift(i as f32 * CRISP / 100.0, 1.0);
            assert!(c >= was && c - was < 0.05, "{i}: {was} → {c}");
            was = c;
        }
        let mut was = 1.0;
        for i in 0..=100 {
            let c = crisp_lift(5.0, 1.0 - i as f32 * 0.001);
            assert!(c <= was && was - c < 0.05, "{i}: {was} → {c}");
            was = c;
        }
    }

    /// The view's clock at 240 frames a second: the player's position read
    /// a little early or late in each frame and put right by 80 ms mid-way
    /// never makes it hop, stop or go back; it is with the player again
    /// within a second; a seek, a pause and the track's end are followed
    /// at once.
    #[test]
    fn the_clock_runs_evenly_and_comes_to_the_player_without_a_hop() {
        let dt = 1.0 / 240.0;
        // The player: 10 s at frame 0, running at the pace of the frames,
        // read up to 3 ms off, and 80 ms further on from frame 240.
        let heard = |f: usize| {
            let jitter = [0.003, -0.002, 0.001, -0.003, 0.002][f % 5];
            10.0 + f as f64 * dt + jitter + if f >= 240 { 0.08 } else { 0.0 }
        };
        let mut c = Clock::step(None, heard(0), 0.0, 1.0, None, true);
        assert_eq!(c.pos, heard(0), "a first frame is where the player is");
        let (mut least, mut most) = (f64::INFINITY, 0.0f64);
        for f in 1..720 {
            let next = Clock::step(Some(c), heard(f), f as f64 * dt, 1.0, None, true);
            let step = next.pos - c.pos;
            least = least.min(step);
            most = most.max(step);
            // Before the correction it runs at the frames' pace, the
            // reading's own unevenness all but gone.
            if f < 240 {
                assert!((step - dt).abs() < dt * 0.1, "frame {f}: {step}");
            }
            c = next;
        }
        assert!(least >= dt * CLOCK_SLOWEST - 1e-9, "never stops or goes back: {least}");
        assert!(most < dt * 2.0, "the correction is run up to, not hopped: {most}");
        assert!((c.pos - (10.0 + 719.0 * dt + 0.08)).abs() < 0.005, "with the player again: {}", c.pos);
        // The player behind by a little: the clock slows down for it.
        let slow = Clock::step(Some(c), c.pos + dt - 0.1, c.at + dt, 1.0, None, true);
        assert!(slow.pos > c.pos && slow.pos < c.pos + dt);
        // A seek either way, and a pause, at once.
        assert_eq!(Clock::step(Some(c), 95.0, c.at + dt, 1.0, None, true).pos, 95.0);
        assert_eq!(Clock::step(Some(c), 2.0, c.at + dt, 1.0, None, true).pos, 2.0);
        assert_eq!(Clock::step(Some(c), c.pos - 0.01, c.at + dt, 1.0, None, false).pos, c.pos - 0.01);
        // A stalled sound stands still; the track's end is not run past.
        assert_eq!(Clock::step(Some(c), c.pos, c.at + dt, 0.0, None, true).pos, c.pos);
        let end = c.pos + 0.001;
        assert_eq!(Clock::step(Some(c), end, c.at + dt, 1.0, Some(end), true).pos, end);
    }

    /// A text between whole pixels is handed to the painter on them with its
    /// rows moved by the rest, so it is drawn where it is, not on the pixel
    /// nearest; one on whole pixels is drawn as it is.
    #[test]
    fn a_moving_text_is_drawn_between_whole_pixels() {
        with_painter(|painter| {
            let g = fixed_layout(painter, "Свет из окна\nложится на пол", 26.0);
            let ppp = painter.pixels_per_point();
            let (at, same) = afloat(painter, pos2(40.0, 120.0), &g);
            assert_eq!(at, pos2(40.0, 120.0));
            assert!(Arc::ptr_eq(&same, &g), "at rest: the layout itself");
            // Gliding down a pixel in 240 steps: every step moves it.
            let mut was = at.y + g.rows[1].pos.y;
            for step in 1..=240 {
                let y = 120.0 + step as f32 / 240.0 / ppp;
                let (at, moved) = afloat(painter, pos2(40.0, y), &g);
                assert!(((at.y * ppp).round() - at.y * ppp).abs() < 1e-3, "the origin is on whole pixels");
                assert_eq!(moved.rows.len(), g.rows.len());
                // (Up to the hair within which it counts as at rest.)
                for (m, r) in moved.rows.iter().zip(&g.rows) {
                    assert!((at.y + m.pos.y - (y + r.pos.y)).abs() < 0.021 / ppp, "step {step}");
                    assert_eq!(m.pos.x, r.pos.x);
                }
                let drawn = at.y + moved.rows[1].pos.y;
                assert!(drawn >= was - 1e-4 && drawn - was < 0.05 / ppp, "step {step}: {was} → {drawn}");
                was = drawn;
            }
        });
    }

    /// A growing line is one layout scaled: each letter moves along the
    /// line evenly, a little every frame, with one small shift at most (off
    /// the small resting layout), where a layout at every whole size on the
    /// way shook the letters against each other.
    #[test]
    fn a_growing_lines_letters_keep_their_places() {
        with_painter(|painter| {
            let text = "Слышишь этот ритм, он ведёт меня";
            let rest = (20.0, 26.0);
            let xs = |g: &Galley| g.rows[0].row.glyphs.iter().map(|gl| gl.pos.x).collect::<Vec<_>>();
            let big = xs(&*fixed_layout(painter, text, rest.1));
            let mut was = xs(&*moving_layout(painter, text, rest, 1.0 / 240.0));
            let mut shifts = 0;
            for step in 2..240 {
                let t = step as f32 / 240.0;
                let now = xs(&*moving_layout(painter, text, rest, t));
                assert_eq!(now.len(), big.len());
                // What a letter's place grows by in one step, at most.
                let even = big.last().unwrap() * (rest.1 - rest.0) / rest.1 / 240.0 + 0.01;
                let off = now.iter().zip(&was).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                if off > even {
                    shifts += 1;
                    assert!(off < 1.5, "step {step}: {off}");
                }
                was = now;
            }
            assert!(shifts <= 1, "{shifts} shifts");
            // Above the small resting size it is the big layout, scaled.
            let size = rest.0 + (rest.1 - rest.0) * 0.5;
            for (x, b) in xs(&*moving_layout(painter, text, rest, 0.5)).iter().zip(&big) {
                assert!((x - b * size / rest.1).abs() < 1e-3);
            }
        });
    }

    /// The text follows the current line without swinging past its place,
    /// lines further away later; a wave piece's jump comes off the line
    /// over a few frames whatever «Плавность» is.
    #[test]
    fn the_scroll_settles_without_overshoot_and_a_jump_eases_off_the_line() {
        for far in 0..=8 {
            let spring = follow_spring(far as f32);
            let (mut was, mut fastest) = (1.0f32, 0.0f32);
            for f in 1..=480 {
                // What is left of the way, from all of it at rest.
                let (left, _) = spring.solve(1.0, 0.0, f as f32 / 240.0);
                assert!(left >= -1e-4 && left <= was + 1e-5, "far {far}, frame {f}: {was} → {left}");
                fastest = fastest.max(was - left);
                was = left;
            }
            assert!(fastest < 0.03, "far {far}: {fastest} of the way in a frame");
            assert!(was < 0.01, "far {far}: there within two seconds");
            if far > 0 {
                assert!(spring.stiffness <= follow_spring(far as f32 - 1.0).stiffness);
            }
        }
        assert_eq!(follow_spring(0.0), FOLLOW);
        let fastest = |window: f32| {
            (-60..240).map(|f| jump_over((f + 1) as f32 / 240.0, 1.0, window) - jump_over(f as f32 / 240.0, 1.0, window))
                .fold(0.0f32, f32::max)
        };
        assert!(fastest(JUMP_EASE) < fastest(0.0) * 0.7, "{} against {}", fastest(JUMP_EASE), fastest(0.0));
        assert!(fastest(JUMP_EASE) < 0.16);
    }

    /// The same song's text swapped (timing made, another source) keeps each
    /// line's slot, so its place, focus and wait go on; another song's text
    /// takes the slots by place.
    #[test]
    fn a_swapped_text_keeps_each_line_in_its_slot() {
        let plain = vec![line(1.0, Some(2.0), "Раз"), line(2.0, Some(3.0), "Два"), line(6.0, Some(7.0), "Раз"), line(7.0, Some(8.0), "Три")];
        let items = display_lines(&plain);
        let keys = item_keys(&plain, &items);
        assert_eq!(keys.len(), items.len());
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len(), "a repeated line is told apart");
        let first = carry_slots(&Slots::default(), &keys);
        assert_eq!(first.slots, (0..keys.len() as u64).collect::<Vec<_>>());
        // Made timing ends the second line sooner: a wait comes in before
        // the third line, the lines keep their slots and the wait gets one.
        let mut made = plain.clone();
        made[1].end = Some(Duration::from_secs_f32(2.0));
        let made_items = display_lines(&made);
        assert!(made_items.len() > items.len(), "{made_items:?}");
        let swapped = carry_slots(&first, &item_keys(&made, &made_items));
        for (i, item) in made_items.iter().enumerate() {
            if let DisplayLine::Line(j) = item {
                let was = items.iter().position(|it| *it == DisplayLine::Line(*j)).unwrap();
                assert_eq!(swapped.slots[i], first.slots[was], "line {j}");
            } else {
                assert!(!first.slots.contains(&swapped.slots[i]), "the new wait has a slot of its own");
            }
        }
        // Another song: by place.
        let other = vec![line(1.0, Some(2.0), "Иное"), line(2.0, Some(3.0), "Совсем")];
        let other_items = display_lines(&other);
        let next = carry_slots(&swapped, &item_keys(&other, &other_items));
        assert_eq!(next.slots[..], swapped.slots[..other_items.len()]);
    }

    /// While a click holds its line, the lyrics are painted where its seek
    /// will land, so the painted position meets the seek's without a jump.
    #[test]
    fn a_held_click_is_painted_where_its_seek_lands() {
        let ms = Duration::from_millis;
        let j = Jump { item: DisplayLine::Line(3), to: ms(20_000), at: 1.0 };
        assert_eq!(jump_pos(None, ms(5_000), Duration::ZERO, 0.2), ms(5_000));
        // A lead ahead: where the seek lands is its point plus the lead.
        assert_eq!(jump_pos(Some(j), ms(5_000), Duration::ZERO, 0.2), led(ms(20_000), 0.2));
        assert_eq!(jump_pos(Some(j), ms(40_000), Duration::ZERO, 0.0), ms(20_000));
        // A lead behind: the seek lands behind its point and is followed.
        let back = Duration::from_secs_f32(0.3);
        let landed = led(ms(20_000), -0.3);
        assert_eq!(jump_pos(Some(j), ms(5_000), back, -0.3), landed);
        assert_eq!(jump_pos(Some(j), landed, back, -0.3), landed);
        assert_eq!(jump_pos(Some(j), ms(19_850), back, -0.3), ms(19_850));
    }

    /// A line that is no longer current goes on being sung from where it
    /// was (no jump to the end when the next line comes in mid-word), and
    /// stays as it was when the song went back before it.
    #[test]
    fn a_line_left_goes_on_from_where_it_was_sung() {
        let ms = Duration::from_millis;
        assert_eq!(sung_pos(Some(ms(3_000)), ms(3_016)), ms(3_016), "on with the song");
        assert_eq!(sung_pos(Some(ms(3_000)), ms(1_000)), ms(3_000), "a seek back holds it");
        assert_eq!(sung_pos(Some(ms(3_000)), ms(9_000)), ms(3_000), "so does a seek on");
        assert_eq!(sung_pos(None, ms(1_000)), ms(1_000));
        // Frame by frame it keeps up with the song for as long as it fades.
        let mut last = Some(ms(3_000));
        for f in 1..60 {
            let p = sung_pos(last, ms(3_000 + 16 * f));
            assert_eq!(p, ms(3_000 + 16 * f));
            last = Some(p);
        }
        // Karaoke: the front where the line was left is where it goes on
        // from, instead of the whole line lit at once.
        let mut l = line(1.0, Some(4.0), "Свет из окна");
        l.words = vec![word(1.0, 2.0, "Свет "), word(2.0, 2.5, "из "), word(2.5, 4.0, "окна")];
        with_painter(|painter| {
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = timing(&l, &g, Look { style: Style::Karaoke, by: By::Syllables });
            let left = ms(3_000);
            let head = |at: Duration| t.front.head(0, at.as_secs_f64());
            let (at, next) = (head(left), head(sung_pos(Some(left), left + ms(16))));
            assert!(at.is_finite() && next >= at && next - at < 5.0, "{at} → {next}");
        });
    }

    /// The unit is what the karaoke front keeps time by. By syllables each
    /// piece fills evenly across its letters over its own time: a fast «ко»
    /// and «смо», a held «цве», then «ток». By letters its vowel takes
    /// longer; by words the whole word is crossed evenly.
    #[test]
    fn a_syllable_fills_evenly_over_its_own_time() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космоцветок");
            l.words = vec![word(1.0, 1.1, "ко"), word(1.1, 1.25, "смо"), word(1.25, 2.1, "цве"), word(2.1, 2.3, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            // On the times exactly.
            let exact = |by| Timing::build(&l, &g, Look { style: Style::Karaoke, by }, 0.0, Ease::EXACT).unwrap();
            let t = exact(By::Syllables);
            let at = |p: f32| t.front.runs[0].way(f64::from(p));
            let s = &t.syllables[2];
            // Halfway through its time the front is halfway across it.
            let mid = at((s.start + s.end) * 0.5);
            assert!((mid - (s.from + s.to) * 0.5).abs() < 0.5, "{mid} in {}..{}", s.from, s.to);
            assert!((at(s.start) - s.from).abs() < 0.05 && (at(s.end) - s.to).abs() < 0.05);
            // The held one runs slowest.
            let pace = |i: usize| (t.syllables[i].to - t.syllables[i].from) / (t.syllables[i].end - t.syllables[i].start);
            assert!(pace(2) < pace(0) && pace(2) < pace(1));
            // By letters the same piece is weighted (its vowel takes longer).
            let letters = exact(By::Letters);
            assert!((letters.front.runs[0].way(f64::from((s.start + s.end) * 0.5)) - mid).abs() > 0.5);
            // By words the front is halfway across the word halfway through
            // it, whatever its syllables do (they are well past that).
            let words = exact(By::Words);
            let (from, to) = (t.syllables[0].from, t.syllables[3].to);
            let half = words.front.runs[0].way(1.65);
            assert!((half - (from + to) * 0.5).abs() < 0.5 && (half - at(1.65)).abs() > 5.0, "{half} against {}", at(1.65));
        });
    }

    /// «Шаг подсветки»: karaoke follows its front at every step; under
    /// Focus by syllables each syllable lights whole as it starts, by words
    /// each word at its first syllable; the wave lights letter by letter,
    /// by words the word whole.
    #[test]
    fn the_unit_sets_how_the_light_steps() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космо цветок");
            l.words = vec![word(1.0, 1.4, "кос"), word(1.4, 1.8, "мо "), word(1.8, 2.4, "цве"), word(2.4, 2.8, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let of = |style, by| timing(&l, &g, Look { style, by });
            // Glyphs: к о с м о ␠ ц в е т о к.
            for by in [By::Letters, By::Syllables, By::Words] {
                let t = of(Style::Karaoke, by);
                assert!(t.lit.iter().all(Option::is_none), "the front");
                assert_eq!(t.front.runs.len(), 1);
            }
            let s = of(Style::Focus, By::Syllables).lit;
            assert_eq!(s[0], s[2], "«кос» lights whole");
            assert_eq!(s[3], s[4], "«мо» lights whole");
            assert!(s[0] == Some(1.0) && s[3] > s[2], "a syllable after the other");
            assert_eq!(s[5], None, "a space has none");
            let w = of(Style::Focus, By::Words).lit;
            assert_eq!(w[0], w[4], "«космо» lights whole");
            assert!(w[6] > w[4] && w[6] == w[11]);
            // Focus by letters lights the whole line: nothing goes along it.
            let calm = of(Style::Focus, By::Letters);
            assert!(calm.lit.iter().all(Option::is_none) && calm.front.runs.is_empty());
            for by in [By::Letters, By::Syllables] {
                let wave = of(Style::Wave, by).lit;
                assert!(wave[1] > wave[0] && wave[2] > wave[1] && wave[3] > wave[2], "{by:?}: letter by letter");
                // No letter before its piece is sung, none much after it.
                assert!(wave[0] == Some(1.0) && wave[6] >= Some(1.8) && wave[11] < Some(2.8 + 0.2), "{by:?}: {wave:?}");
            }
            let wave = of(Style::Wave, By::Words).lit;
            assert_eq!(wave[0], wave[4], "the word jumps whole");
        });
    }

    /// A line's timing is worked out once for its layout, look and settings
    /// and kept: the same one comes back frame after frame, another for
    /// another unit, «Плавность» or timing, and only a few are kept.
    #[test]
    fn a_lines_timing_is_kept_from_frame_to_frame() {
        with_painter(|painter| {
            let ctx = painter.ctx();
            let mut l = line(1.0, Some(3.0), "космо цветок");
            l.words = vec![word(1.0, 1.4, "кос"), word(1.4, 1.8, "мо "), word(1.8, 2.4, "цве"), word(2.4, 2.8, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let pace = Pace::of(&Prefs::default());
            let look = Look { style: Style::Karaoke, by: By::Syllables };
            let kept = timing_of(ctx, &l, &g, look, pace, 26.0).unwrap();
            assert!(Arc::ptr_eq(&kept, &timing_of(ctx, &l, &g, look, pace, 26.0).unwrap()));
            let other = |l: &LyricLine, look: Look, pace: Pace| !Arc::ptr_eq(&kept, &timing_of(ctx, l, &g, look, pace, 26.0).unwrap());
            assert!(other(&l, Look { by: By::Words, ..look }, pace));
            assert!(other(&l, look, Pace { smooth: 1.0, ..pace }));
            let mut retimed = l.clone();
            retimed.words[3].end = Duration::from_secs_f32(2.9);
            assert!(other(&retimed, look, pace));
            // A line that grows is laid out anew every frame: its timings
            // push the oldest out.
            for size in 10..40 {
                timing_of(ctx, &l, &fixed_layout(painter, &l.text, size as f32), look, pace, size as f32);
            }
            let count = ctx.data_mut(|d| d.get_temp_mut_or_default::<Timings>(egui::Id::new("lyrics-timings")).0.len());
            assert_eq!(count, TIMINGS);
            assert!(timing_of(ctx, &line(1.0, None, "no words"), &g, look, pace, 26.0).is_none());
        });
    }

    /// «Буквы» must not swallow a short word, at full «Плавность» either:
    /// its letters light one after another as the front goes through it.
    /// The view paints [`LYRICS_LEAD`] ahead of the player, so with the lead
    /// off 1.15 is early in «Свет » (1.0 to 1.35) and 1.33 near its end.
    #[test]
    fn letters_of_a_word_light_one_by_one() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_WORDS};
        let mut l = line(1.0, Some(3.0), "Свет окна");
        l.words = vec![word(1.0, 1.35, "Свет "), word(2.0, 3.0, "окна")];
        let close = |a: Color32, b: Color32| a.to_array().iter().zip(b.to_array()).all(|(x, y)| x.abs_diff(y) <= 3);
        let bright = |c: Color32| c.to_array().iter().map(|v| u32::from(*v)).sum::<u32>();
        let letters = Prefs {
            lyrics_style: 0,
            lyrics_unit: LYRICS_BY_LETTERS,
            lyrics_smooth: 1.0,
            lyrics_lead: 0.0,
            ..Prefs::default()
        };
        // By words the front crosses the word too: it never lights at once.
        let words = Prefs { lyrics_unit: LYRICS_BY_WORDS, ..letters.clone() };
        for (unit, prefs) in [("letters", &letters), ("words", &words)] {
            let (glyphs, lit) = painted_glyphs(prefs, &l, 1.15);
            assert!(glyphs.len() >= 5, "{:?}", glyphs.len());
            let (first, last, next) = (glyphs[0].expect("«С» is painted"), glyphs[3].expect("«т» is painted"), glyphs[5].expect("«о» is painted"));
            assert!(bright(first) > bright(last), "{unit}: «С» lights before «т»: {first:?} {last:?}");
            assert!(!close(last, lit) && last == next, "{unit}: «т» is not sung yet: {last:?}");
            let (glyphs, lit) = painted_glyphs(prefs, &l, 1.33);
            let (first, last) = (glyphs[0].unwrap(), glyphs[3].unwrap());
            assert!(close(first, lit), "{unit}: «С» is sung: {first:?}");
            assert!(!close(last, lit), "{unit}: «т» is still being sung: {last:?}");
            // Sung and in the pause: the word is lit whole, the next not.
            let (glyphs, lit) = painted_glyphs(prefs, &l, 1.9);
            assert!(glyphs[..4].iter().all(|c| close(c.unwrap(), lit)) && !close(glyphs[5].unwrap(), lit), "{unit}: in the pause");
        }
    }

    /// Focus by syllables or words: a piece stands out while it is sung and
    /// goes back to the line's tone after it, evenly; nothing fills.
    #[test]
    fn focus_lifts_the_sung_piece_and_lets_it_go() {
        let at = |p: f32| focus_pulse(p, 2.0, 3.0, 0.1, 0.0);
        assert_eq!(at(1.5), 0.0);
        assert!(at(2.5) > 0.99 && at(3.0) > 0.99);
        assert!(at(3.0 + FOCUS_FALL * 0.5) > 0.3 && at(3.0 + FOCUS_FALL * 0.5) < 0.7);
        assert!(at(3.0 + FOCUS_FALL + 0.01) < 1e-3);
        let mut was = 0.0;
        for ms in 1500..4000 {
            let v = at(ms as f32 / 1000.0);
            assert!((v - was).abs() < 0.05, "{ms}: a step");
            was = v;
        }
        // A piece sung in a blink still shows.
        assert!(focus_pulse(2.2, 2.0, 2.05, 0.1, 0.0) > 0.9);
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космо цветок");
            l.words = vec![word(1.0, 1.4, "кос"), word(1.4, 1.8, "мо "), word(1.8, 2.4, "цве"), word(2.4, 2.8, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let look = |by| Look { style: Style::Focus, by };
            // Glyphs: к о с м о ␠ ц в е т о к.
            assert_eq!(piece_end(&t, 0, look(By::Syllables)), Some(t.syllables[0].end));
            assert_eq!(piece_end(&t, 0, look(By::Words)), Some(t.syllables[1].end));
            assert_eq!(piece_end(&t, 5, look(By::Words)), None);
        });
    }

    /// By syllables under the wave a written word rises with each of its
    /// syllables, all its letters together, and is up in full once the
    /// last is sung.
    #[test]
    fn by_syllables_a_word_rises_with_its_syllables() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "нон-стоп ok");
            l.words = vec![word(1.0, 1.5, "нон-"), word(1.5, 2.5, "стоп "), word(3.0, 3.5, "ok")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::build(&l, &g, Look { style: Style::Wave, by: By::Syllables }, 0.0, Ease::EXACT).unwrap();
            assert_eq!(t.words, [0, 2]);
            let at = |p: f32| t.word_jump(1, p, 1.0, 0.0);
            assert_eq!(at(0.5).0, 0.0);
            // Half up (at rest) between its syllables, then whole.
            assert!((at(1.45).0 - 0.5).abs() < 0.01, "{:?}", at(1.45));
            assert!((at(2.4).0 - 1.0).abs() < 0.01);
            assert_eq!(t.word_jump(0, 1.45, 1.0, 0.0), at(1.45));
            // Seconds since its last syllable, negative before it.
            assert!(at(1.45).1 < 0.0 && (at(2.0).1 - 0.5).abs() < 0.05);
            // The next word has not moved.
            assert_eq!(t.word_jump(2, 2.4, 1.0, 0.0).0, 0.0);
        });
    }

    /// Switching between the calm Focus and a timed look fades from the old
    /// one to the new one, the line never thinning much between them.
    #[test]
    fn a_change_of_look_fades_and_the_line_stays_whole() {
        let calm = Look { style: Style::Focus, by: By::Letters };
        let wave = Look { style: Style::Wave, by: By::Syllables };
        assert_eq!(look_mix(calm, calm, 0.0), 1.0);
        assert_eq!(look_mix(calm, wave, f32::INFINITY), 1.0);
        assert_eq!(look_mix(calm, wave, 0.0), 0.0);
        let mut last = -1.0;
        for ms in 0..400 {
            let m = look_mix(calm, wave, ms as f32 / 1000.0);
            assert!(m >= last && (0.0..=1.0).contains(&m));
            last = m;
        }
        assert_eq!(last, 1.0, "done within 0.4 s");
        assert_eq!(look_fades(0.0), (1.0, 0.0));
        assert_eq!(look_fades(1.0), (0.0, 1.0));
        for k in 0..=20 {
            let (out, inn) = look_fades(k as f32 / 20.0);
            assert!(inn + out - inn * out > 0.85, "{k}");
        }
        // In the view: a first frame shows its look, a change starts a fade.
        let ctx = egui::Context::default();
        let id = egui::Id::new("look");
        assert_eq!(look_change(&ctx, id, calm, 10.0), (None, 1.0));
        assert_eq!(look_change(&ctx, id, wave, 10.0), (Some(calm), 0.0));
        assert_eq!(look_change(&ctx, id, wave, 11.0), (None, 1.0));
    }

    /// A wait new to a swapped text opens from folded: its room's spring,
    /// started at 0, moves to its first target instead of starting there.
    #[test]
    fn a_new_wait_opens_from_folded() {
        let ctx = egui::Context::default();
        let (seeded, plain) = (egui::Id::new("seeded"), egui::Id::new("plain"));
        anim::fraction_starts_at(&ctx, seeded, 0.0);
        assert!(anim::fraction(&ctx, seeded, 1.0, WAIT_ROOM) < 0.01);
        assert_eq!(anim::fraction(&ctx, plain, 1.0, WAIT_ROOM), 1.0);
        // One that moves already is left as it is.
        anim::fraction_starts_at(&ctx, plain, 0.0);
        assert_eq!(anim::fraction(&ctx, plain, 1.0, WAIT_ROOM), 1.0);
    }

    /// Mega mode: another text moves the block of lines to its new middle
    /// on a spring instead of jumping there; a resize moves it at once.
    #[test]
    fn the_mega_block_glides_to_a_new_text_and_follows_a_resize() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.prefs.lyrics_soft_edges = false;
        let left_at = |state: &ViewState, time: f64, width: f32| -> f32 {
            let mut left = 0.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 700.0))),
                time: Some(time), ..Default::default()
            }, |ui| {
                show(ui, state, 0.0, true, &mut Vec::new());
                left = ui.ctx().data(|d| d.get_temp::<f32>(ui.id().with("lyrics-view").with("text-left"))).unwrap_or(f32::NAN);
            });
            output.textures_delta.clear();
            left
        };
        state.lyrics.lines = vec![line(1.0, None, "Да"), line(2.0, None, "Нет")];
        let a = left_at(&state, 0.0, 1000.0);
        assert_eq!(left_at(&state, 0.5, 1000.0), a);
        state.lyrics.lines = vec![line(1.0, None, "Строка подлиннее, чем те"), line(2.0, None, "И ещё одна длинная строка")];
        let b0 = left_at(&state, 1.0, 1000.0);
        let b1 = left_at(&state, 3.0, 1000.0);
        assert!((b0 - a).abs() < 1.0, "a new text starts where the block was: {a} → {b0}");
        assert!(b1 < a - 20.0, "then it moves to its own middle: {a} → {b1}");
        let c = left_at(&state, 3.1, 800.0);
        assert!((left_at(&state, 5.0, 800.0) - c).abs() < 0.5, "a resize at once");
    }
