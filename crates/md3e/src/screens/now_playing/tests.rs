    #[test]
    fn the_album_gives_way_before_the_artists() {
        let w = super::LineWidths { artist: 120.0, first: 60.0, sep: 12.0, album: 200.0 };
        assert_eq!(super::split_line(400.0, w), (120.0, Some(212.0)), "both whole");
        assert_eq!(super::split_line(200.0, w), (120.0, Some(80.0)), "the album truncates");
        assert_eq!(super::split_line(150.0, w), (90.0, Some(60.0)), "then the artists");
        assert_eq!(super::split_line(80.0, w), (60.0, None), "the first artist stays, the album goes");
        assert_eq!(super::split_line(40.0, w), (40.0, None));
        let alone = super::LineWidths { album: 0.0, ..w };
        assert_eq!(super::split_line(100.0, alone), (100.0, None));
    }
    use super::*;

    /// The ring shows only while the music plays and only around a shaped
    /// cover: never on a pause, never around the square one.
    #[test]
    fn the_ring_shows_only_playing_around_a_shape() {
        for shape in [CoverShape::Cookie12, CoverShape::Circle, CoverShape::Random] {
            assert!(ring_shown(true, shape, true), "{shape:?}");
            assert!(!ring_shown(true, shape, false), "{shape:?} paused");
            assert!(!ring_shown(false, shape, true), "{shape:?} off");
            assert!(ring_on(true, shape));
        }
        assert!(!ring_shown(true, CoverShape::Original, true), "the square cover");
        assert!(!ring_on(true, CoverShape::Original));
    }

    /// A click on empty space puts the track's info away and leaves the
    /// player open; with the lyrics, the queue or nothing beside the cover
    /// it closes the player (mini mode has nothing to close to).
    #[test]
    fn a_click_beside_the_track_info_closes_the_info_not_the_player() {
        for embedded in [false, true] {
            assert_eq!(outside_click(NpSlot::Info, embedded), Outside::CloseInfo);
        }
        for slot in [NpSlot::None, NpSlot::Lyrics, NpSlot::Queue] {
            assert_eq!(outside_click(slot, false), Outside::ClosePlayer);
            assert_eq!(outside_click(slot, true), Outside::Nothing);
        }
    }

    #[test]
    fn the_circle_ring_rounds_the_square_inside_the_circles_box() {
        let rect = Rect::from_center_size(pos2(300.0, 300.0), vec2(400.0, 400.0));
        let (gap, circle) = (14.0, 214.0);
        let dir = |i: usize| vec2((i as f32 * TAU / BINS as f32).sin(), -(i as f32 * TAU / BINS as f32).cos());
        // Around a shape: the circle itself.
        let shape = Outline::new(Some(MaterialShape::Cookie12Sided), 1.0, rect, 0.4);
        assert!(circle_path(&shape.radii(), circle, gap).iter().all(|r| (r - circle).abs() < 1.0));
        // Around the square (and half-way to it): a gap off every corner, and
        // never farther out at the sides than the circle.
        for k in [0.0, 0.5] {
            let o = Outline::new(Some(MaterialShape::Cookie12Sided), k, rect, 0.0);
            let radii = o.radii();
            let path = circle_path(&radii, circle, gap);
            for i in 0..BINS {
                assert!(path[i] >= radii[i] + gap - 0.01);
                let p = dir(i) * path[i];
                assert!(p.x.abs() <= circle + 1.0 && p.y.abs() <= circle + 1.0, "{k} {i}: {p:?}");
            }
        }
        let square = circle_path(&Outline::new(None, 1.0, rect, 0.0).radii(), circle, gap);
        assert!(square[45] > circle + 50.0, "the corners go round, not under, the dots");
        // The dots keep the circle's speed: a quarter turn moves them a
        // quarter of the circle's dots, however long the path.
        let ctx = egui::Context::default();
        let key = egui::Id::new("t");
        assert_eq!(dot_phase(&ctx, key, 6.0, 72.0 / TAU), 0.0);
        let u = dot_phase(&ctx, key, 6.0 + 0.25 * TAU / 72.0, 72.0 / TAU);
        assert!((u - 0.25).abs() < 1e-3);
        // Across the wrap of the angle, no jump.
        let u = dot_phase(&ctx, key, 0.01, 72.0 / TAU);
        let expect = (0.25 + (0.01 + TAU - 6.0 - 0.25 * TAU / 72.0) * 72.0 / TAU).rem_euclid(1.0);
        assert!((u - expect).abs() < 1e-3, "{u} vs {expect}");
    }

    #[test]
    fn only_the_shape_itself_is_the_cover() {
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 200.0));
        for shape in [MaterialShape::Cookie9Sided, MaterialShape::Clover4Leaf, MaterialShape::Sunny] {
            let o = Outline::new(Some(shape), 1.0, rect, 0.0);
            assert!(o.contains(rect.center()), "{shape:?} centre");
            // The square's corners are outside every rounded shape.
            for c in [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()] {
                let inward = c + (rect.center() - c) * 0.06;
                assert!(!o.contains(inward), "{shape:?} corner {inward:?}");
            }
            assert!(!o.contains(pos2(400.0, 200.0)));
        }
    }

    #[test]
    fn a_pause_lands_on_the_square_by_the_smallest_turn() {
        let quarter = TAU / 4.0;
        for n in [1u32, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16] {
            let lobe = TAU / n as f32;
            // A turn of a quarter's lobes at most: what the square's own
            // four-fold look and the shape's lobes leave.
            let most = (lobe * 0.5).min(quarter * 0.5);
            for i in 0..720 {
                let angle = i as f32 / 720.0 * TAU;
                let (base, twist) = square_pose(angle, n);
                assert!(twist.abs() <= most + 1e-4, "n={n} angle={angle} twist={twist}");
                // The base is the upright square.
                let q = base / quarter;
                assert!((q - q.round()).abs() < 1e-3, "n={n} base={base}");
                // At the morph's start the shape stands where it played.
                let lobes = (base + twist - angle) / lobe;
                assert!((lobes - lobes.round()).abs() < 1e-3, "n={n} angle={angle}: {base} + {twist}");
            }
        }
        // A pentagon is a fifth of a turn from a quarter at most 9°.
        assert!(square_pose(0.3, 5).1.abs() <= TAU / 40.0 + 1e-4);
    }

    /// Plays (the shape, spinning) to `angle`, then pauses (the morph
    /// 1 → 0 over `frames`, the spin braking on), then resumes (0 → 1),
    /// then plays on with the spin's ramp growing: the angles and morphs
    /// of each part.
    #[allow(clippy::type_complexity)]
    fn pause_and_resume(n: u32, angle: f32, frames: usize) -> (Vec<(f32, f32)>, Vec<f32>, Vec<f32>) {
        let mut t = CoverTurn::default();
        let speed = 0.02;
        let mut spin = 0.0f32;
        t.step(n, spin, 1.0, true, 1.0);
        while t.angle < angle {
            spin = (spin + speed).rem_euclid(TAU);
            t.step(n, spin, 1.0, true, 1.0);
        }
        let mut pause = Vec::new();
        for k in 0..=frames {
            // The braking spin still steps, and the spring overshoots 0.
            spin = (spin + speed * (1.0 - k as f32 / frames as f32)).rem_euclid(TAU);
            let m = 1.0 - k as f32 / frames as f32 * 1.05;
            pause.push((t.step(n, spin, m, false, 1.0), m));
        }
        let mut resume = Vec::new();
        for k in 0..=frames {
            spin = (spin + speed * k as f32 / frames as f32).rem_euclid(TAU);
            resume.push(t.step(n, spin, k as f32 / frames as f32 * 1.04, true, 0.0));
        }
        let mut after = Vec::new();
        for k in 0..60 {
            spin = (spin + speed).rem_euclid(TAU);
            after.push(t.step(n, spin, 1.0, true, k as f32 / 60.0));
        }
        (pause, resume, after)
    }

    /// `d` within half of `turn` either way.
    fn near(d: f32, turn: f32) -> f32 {
        (d + turn * 0.5).rem_euclid(turn) - turn * 0.5
    }

    #[test]
    fn a_morph_turns_the_outline_at_most_half_a_lobe() {
        let quarter = TAU / 4.0;
        for n in [1u32, 2, 4, 5, 6, 8, 9, 12, 16] {
            let lobe = TAU / n as f32;
            for i in 0..48 {
                let angle = 0.05 + i as f32 / 48.0 * (TAU - 0.1);
                let (pause, resume, after) = pause_and_resume(n, angle, 30);
                // The pause: how far the outline turns, frame by frame (a
                // whole turn looks the same; the square a quarter turn).
                let mut turned = 0.0f32;
                let (mut lo, mut hi) = (0.0f32, 0.0f32);
                for w in pause.windows(2) {
                    let ((a, _), (b, m)) = (w[0], w[1]);
                    let d = near(b - a, if m <= 1e-3 { quarter } else { TAU });
                    assert!(d.abs() < 0.1, "n={n} angle={angle}: {a} -> {b} at {m}");
                    turned += d;
                    lo = lo.min(turned);
                    hi = hi.max(turned);
                }
                assert!(hi - lo <= lobe * 0.5 + 1e-3, "n={n} angle={angle}: {lo}..{hi}");
                // On the square: upright.
                assert!(near(pause.last().unwrap().0, quarter).abs() < 1e-4, "n={n} angle={angle}: {pause:?}");
                // The resume: the shape grows out of it upright, no turn.
                assert!(resume.iter().all(|a| a.abs() < 1e-5), "n={n} angle={angle}: {resume:?}");
                // Then it turns, from rest: the speed grows.
                let steps: Vec<f32> = after.windows(2).map(|w| near(w[1] - w[0], TAU)).collect();
                assert!(steps[0] < 0.002, "n={n}: {steps:?}");
                assert!(steps.windows(2).all(|w| w[1] + 1e-5 >= w[0]), "n={n}: {steps:?}");
                assert!(*steps.last().unwrap() > 0.015, "n={n}: {steps:?}");
            }
        }
    }

    #[test]
    fn a_resume_in_the_middle_of_a_pause_goes_back_the_same_way() {
        let mut t = CoverTurn::default();
        t.step(5, 0.0, 1.0, true, 1.0);
        t.step(5, 0.5, 1.0, true, 1.0);
        let start = t.angle;
        // Half way to the square, then back to the shape.
        let mut seen = Vec::new();
        for k in 0..=10 {
            seen.push(t.step(5, 0.5, 1.0 - k as f32 * 0.05, false, 1.0));
        }
        for k in 0..=10 {
            seen.push(t.step(5, 0.5, 0.5 + k as f32 * 0.05, true, 0.0));
        }
        let end = *seen.last().unwrap();
        // The same pose as it left (by whole lobes), never a jump.
        assert!(near(end - start, TAU / 5.0).abs() < 1e-3, "{start} -> {end}");
        assert!(seen.windows(2).all(|w| near(w[1] - w[0], TAU).abs() < 0.05), "{seen:?}");
    }

    #[test]
    fn the_square_meets_the_shape_at_its_nearest_lobes() {
        // How far the outline turns inside the morph itself: the mean turn
        // of its matched points around the centre, by their share of the
        // outline. The square's corners go to the nearest lobes, so it is
        // under half a lobe (and an eighth of a turn) for every shape.
        let mut shapes: Vec<MaterialShape> = CoverShape::shapes().iter().filter_map(|c| material(*c)).collect();
        shapes.sort();
        shapes.dedup();
        for shape in shapes {
            let lobe = TAU / symmetry(Some(shape)) as f32;
            let m = cover_morph(shape);
            let polar = |p: Point| (p.y - 0.5).atan2(p.x - 0.5);
            let (mut turn, mut weight) = (0.0f32, 0.0f32);
            for (a, b) in m.morph_match() {
                let d = (polar(b.anchor0()) - polar(a.anchor0()) + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
                let w = (a.anchor1() - a.anchor0()).distance() + (b.anchor1() - b.anchor0()).distance();
                turn += d * w;
                weight += w;
            }
            let turn = turn / weight.max(1e-6);
            assert!(turn.abs() <= (lobe * 0.5).min(TAU / 8.0) + 1e-3, "{shape:?}: {}°", turn.to_degrees());
        }
    }

    #[test]
    fn bars_follow_a_loud_track_and_rest_in_silence() {
        let mut c = Cava::default();
        // A loud track: every band high, one peak in the bass. The bars
        // still tell the peak apart instead of all sitting at the top.
        let mut bands = vec![0.8; 64];
        bands[4] = 1.0;
        let mut t = Vec::new();
        for _ in 0..120 {
            t = cava_targets(&mut c, &bands, true, 0.05, 0.05);
        }
        // Near the top on both sides, the same: no seam at 12 o'clock.
        assert!(t[1] > 0.9 && t[BAR_COUNT - 1] > 0.9, "{t:?}");
        assert_eq!(t[1], t[BAR_COUNT - 1]);
        assert!(t[BAR_COUNT / 2] < 0.5, "{t:?}");
        // The highs at the bottom.
        let mut high = vec![0.2; 64];
        high[63] = 1.0;
        let mut c = Cava::default();
        let t = cava_targets(&mut c, &high, true, 0.05, 0.05);
        assert!(t[BAR_COUNT / 2] > 0.9 && t[0] < 0.2, "{t:?}");
        // Silence and pause: nothing.
        assert!(cava_targets(&mut c, &[0.01; 64], true, 0.05, 0.05).iter().all(|v| *v == 0.0));
        assert!(cava_targets(&mut c, &bands, false, 0.05, 0.05).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn every_pair_of_shapes_morphs() {
        // `Morph::new` panics on polygons it cannot match; the cover swaps
        // between any two cover shapes, the background between its pool.
        let mut shapes: Vec<MaterialShape> = CoverShape::shapes().iter().filter_map(|c| material(*c)).collect();
        shapes.extend(super::super::bg_shapes::pool());
        shapes.sort();
        shapes.dedup();
        for &a in &shapes {
            for &b in &shapes {
                if a != b {
                    assert!(!pair_morph(a, b).as_cubics(0.5).is_empty(), "{a:?} -> {b:?}");
                }
            }
        }
    }

    #[test]
    fn random_range_and_reroll() {
        let ctx = egui::Context::default();
        let range = [CoverShape::Heart, CoverShape::Gem];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let shape = cover_shape(&ctx, CoverShape::Random, &range, None).expect("a shape");
            assert!(matches!(shape, MaterialShape::Heart | MaterialShape::Gem));
            // Stays put until a reroll, then never the same twice.
            assert_eq!(cover_shape(&ctx, CoverShape::Random, &range, None), Some(shape));
            reroll_shape(&ctx);
            let next = cover_shape(&ctx, CoverShape::Random, &range, None).expect("a shape");
            assert_ne!(next, shape);
            seen.insert(next);
        }
        assert_eq!(seen.len(), 2);
        assert_eq!(random_pool(&[]).len(), CoverShape::shapes().len());
    }

    #[test]
    fn shapes_look_the_same_after_a_lobe() {
        // The orders in `symmetry` against the real outlines: turning the
        // playing shape by one lobe must not change it.
        let rect = Rect::from_center_size(pos2(0.0, 0.0), vec2(200.0, 200.0));
        for choice in CoverShape::shapes() {
            let shape = material(*choice);
            let lobe = TAU / symmetry(shape) as f32;
            let (a, b) = (Outline::new(shape, 1.0, rect, 0.0).radii(), Outline::new(shape, 1.0, rect, lobe).radii());
            let worst = a.iter().zip(&b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(worst < 3.0, "{shape:?}: radii differ by {worst}");
        }
    }

    #[test]
    fn switching_slots_crossfades_without_moving_the_cover() {
        let ctx = egui::Context::default();
        for reduced in [false, true] {
            Theme::new(crate::ColorScheme::baseline(true), true).with_reduced_motion(reduced).install(&ctx);
            for (time, slot, expected) in [
                (0.0, NpSlot::Lyrics, Some([1.0, 0.0, 0.0, 0.0])),
                (1.0, NpSlot::Queue, Some([1.0, 0.0, 0.0, 0.0])),
                (1.05, NpSlot::Queue, None),
                (2.0, NpSlot::Queue, Some([0.0, 1.0, 0.0, 0.0])),
                (3.0, NpSlot::None, Some([0.0, 1.0, 0.0, 0.0])),
                (4.0, NpSlot::Info, Some([0.0, 0.0, 1.0, 0.0])),
            ] {
                let mut output = ctx.run_ui(egui::RawInput { time: Some(time + if reduced { 10.0 } else { 0.0 }), ..Default::default() }, |ui| {
                    let alpha = slot_opacities(ui.ctx(), slot, &Stage::REAL);
                    if let Some(expected) = expected { assert_eq!(alpha, expected); }
                    else { assert!(alpha[0] > 0.0 && alpha[1] > 0.0); }
                    assert!((alpha.iter().sum::<f32>() - 1.0).abs() < 0.001);
                    if slot != NpSlot::None {
                        let k = anim::fraction(ui.ctx(), egui::Id::new("np-mega"), 1.0, Theme::get(ui.ctx()).motion.default_spatial);
                        assert_eq!(k, 1.0);
                    }
                });
                output.textures_delta.clear();
            }
            ctx.data_mut(|d| d.remove::<(NpSlot, Animated<[f32; 4]>)>(egui::Id::new("np-content-fade")));
        }
    }

    #[test]
    fn mega_columns_and_thumbnail_leave_lyrics_and_controls_room() {
        for width in [360.0, 800.0, 899.0, 900.0, 1280.0, 1800.0] {
            let body = Rect::from_min_size(pos2(24.0, 80.0), vec2(width, 780.0));
            let layout = mega_layout(body);
            assert!(body.contains_rect(layout.cover));
            assert!(body.contains_rect(layout.info));
            assert!(body.contains_rect(layout.content));
            assert!(layout.content.is_positive());
            assert!(!layout.info.intersects(layout.content));
            if layout.wide {
                // Halves: the cover column is centred in the left half, the
                // content starts right of the centre line.
                let mid = body.center().x;
                assert!(layout.info.right() < mid && layout.content.left() > mid);
                let col_mid = layout.info.center().x;
                assert!((col_mid - (body.left() + mid) * 0.5).abs() < (body.width() * 0.05).clamp(24.0, 96.0), "{width}");
                assert!(layout.cover.width() <= 440.0);
            } else {
                assert_eq!(layout.cover.size(), vec2(56.0, 56.0));
                assert_eq!(layout.info.bottom(), body.bottom());
            }
            assert_eq!(lerp_rect(body, layout.cover, 0.0), body);
            assert_eq!(lerp_rect(body, layout.cover, 1.0), layout.cover);
            // The lyrics start just under the header (16 px above the body)
            // or the small cover's row, never touching either.
            let lyrics_top = layout.content.top() - layout.lyrics_lift;
            let above = if layout.wide { body.top() - 16.0 } else { layout.cover.bottom() };
            assert_eq!(lyrics_top, above + LYRICS_TOP_GAP, "{width}");
        }
    }
