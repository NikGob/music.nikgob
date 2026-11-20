use super::*;

/// A looping sample for the settings' hover card: the line being sung
/// between two resting ones, painted by the real line painter under `prefs`
/// (animation, glow, dimming, size, letter jump).
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    preview_at(ui, rect, prefs, None);
}

/// The text of a sample: the resting lines above and below the sung one,
/// and the sung one by its syllables.
pub(super) struct SampleText {
    pub(super) above: &'static str,
    pub(super) below: &'static str,
    pub(super) sung: &'static [&'static str],
}

/// The ⓘ card's sample.
pub(super) const CARD_TEXT: SampleText = SampleText { above: "Где-то рядом", below: "Он ведёт меня", sung: &["Слы", "шишь ", "э", "тот ", "ритм"] };
/// A preset tile's: short lines, so the sung one can be large in a small
/// tile.
pub(super) const TILE_TEXT: SampleText = SampleText { above: "Где-то рядом", below: "Он ведёт", sung: &["Слы", "шишь ", "ритм"] };

/// [`preview`] standing at `still` seconds into its loop, or running with
/// the clock.
pub(crate) fn preview_at(ui: &Ui, rect: Rect, prefs: &Prefs, still: Option<f64>) {
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let scale = prefs.lyrics_scale.clamp(0.8, 1.4);
    let (big, small) = (PANE.big * 0.8 * scale, PANE.small * 0.8 * scale);
    // Fit the three lines into the sample (a small card shrinks the text;
    // the size setting still shows, relative to the others): fitted at the
    // largest size.
    let (w, h) = sample_extent(&painter, &CARD_TEXT, big / scale * 1.4, small / scale * 1.4);
    let fit = ((rect.width() - 2.0 * 10.0) / w).min((rect.height() - 2.0 * 6.0) / h).min(1.0);
    sample(ui.ctx(), &painter, rect, prefs, still, &CARD_TEXT, big * fit, small * fit, 6.0 * fit, 10.0);
}

/// A preset's tile (`rect`, small): the sample drawn as the lyrics view
/// draws it, at the view's own size, and the whole scene scaled down to the
/// tile — the jump, the lift, the glow and the gaps shrink with the text,
/// so the motion reads as it does on the real screen. Standing at `still`
/// seconds into its loop (a tile at rest), or running with the clock. The
/// size setting is not a preset's and is left out.
pub(crate) fn tile_preview(ui: &Ui, rect: Rect, prefs: &Prefs, still: Option<f64>) {
    let ctx = ui.ctx();
    let probe = ui.painter();
    let (big, small, gap) = (PANE.big, PANE.small, 8.0);
    let (w, h) = sample_extent(probe, &TILE_TEXT, big, small);
    // The sung line spans the tile but for a margin; the three rows fit.
    const MARGIN: f32 = 10.0;
    let k = ((rect.width() - 2.0 * MARGIN) / w).min((rect.height() - 8.0) / (h + 2.0 * gap)).clamp(0.05, 1.0);
    let t = egui::emath::TSTransform::new(rect.center().to_vec2() * (1.0 - k), k);
    let virt = t.inverse().mul_rect(rect);
    let clip = t.inverse().mul_rect(ui.clip_rect().intersect(rect));
    let layer = ui.layer_id();
    let first = ctx.graphics(|g| g.get(layer).map_or(egui::layers::ShapeIdx(0), |l| l.next_idx()));
    // Set, not intersected with the tile's clip: that one is on screen,
    // this one in the scene's points (scaled down with it).
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(clip);
    sample(ctx, &painter, virt, prefs, still, &TILE_TEXT, big, small, gap, MARGIN / k);
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        let end = list.next_idx();
        list.transform_range(first, end, t);
    });
}

/// How wide the widest line of `text` is and how tall its three rows
/// stand (with the wave's jump above), at `big` for the sung line and
/// `small` for the others.
pub(super) fn sample_extent(painter: &egui::Painter, text: &SampleText, big: f32, small: f32) -> (f32, f32) {
    let sung: String = text.sung.concat();
    let rows = [(text.above, small), (sung.as_str(), big), (text.below, small)];
    let w = rows.iter().map(|(t, z)| fixed_layout(painter, t, *z).size().x).fold(0.0, f32::max);
    let h: f32 = rows.iter().map(|(t, z)| fixed_layout(painter, t, *z).size().y).sum();
    (w, h + 2.0 * 6.0 + big * 0.25)
}

/// The looping sample over `rect`: `text`'s sung line at `big` between its
/// resting lines at `small`, `gap` apart, `pad` in from the left, painted by
/// the real line painter under `prefs`.
#[allow(clippy::too_many_arguments)]
pub(super) fn sample(
    ctx: &egui::Context,
    painter: &egui::Painter,
    rect: Rect,
    prefs: &Prefs,
    still: Option<f64>,
    text: &SampleText,
    big: f32,
    small: f32,
    gap: f32,
    pad: f32,
) {
    let s = Theme::get(ctx).scheme;
    let look = Look::of(prefs);
    // Sung by ~2.9 s, held lit, then eased back to unsung before the loop
    // starts over (a snap back read as a glitch).
    const LOOP: f64 = 3.7;
    const BACK: f32 = 3.2;
    let pos = Duration::from_secs_f64(still.unwrap_or_else(|| anim::now(ctx)).rem_euclid(LOOP));
    let mut at = 0.3;
    let words = text
        .sung
        .iter()
        .map(|w| {
            let d = 0.16 * w.trim().chars().count() as f32;
            let word = nikgob_view::LyricWord {
                start: Duration::from_secs_f32(at),
                end: Duration::from_secs_f32(at + d),
                text: (*w).to_owned(),
            };
            at += d + 0.05;
            word
        })
        .collect::<Vec<_>>();
    let line = LyricLine { text: words.iter().map(|w| w.text.as_str()).collect(), words, ..Default::default() };
    let rest = 1.0 - (1.0 - 0.42) * prefs.lyrics_dim.clamp(0.0, 1.0);
    let dim = s.on_surface.gamma_multiply(rest);
    let lit = s.on_surface;
    let g = fixed_layout(painter, &line.text, big);
    // On whole pixels, as a line at rest in the view is.
    let ppp = painter.pixels_per_point();
    let top = ((rect.center().y - g.size().y * 0.5 + big * 0.1) * ppp).round() / ppp;
    let cur = Rect::from_min_size(pos2(rect.left() + pad, top), g.size());
    for (t, y) in [(text.above, cur.top() - gap), (text.below, cur.bottom() + gap)] {
        let sg = fixed_layout(painter, t, small);
        let top = if y < cur.top() { y - sg.size().y } else { y };
        painter.galley_with_override_text_color(pos2(cur.left(), top), sg, dim);
    }
    let base = s.on_surface.gamma_multiply(rest.min(UNSUNG));
    if let Some(timing) = timing_of(ctx, &line, &g, look, Pace::of(prefs), big) {
        // Past the end the line is drawn as no longer current, its light
        // (and the wave's lift) easing out with `focus`.
        let back = ((pos.as_secs_f32() - BACK) / 0.4).clamp(0.0, 1.0);
        let back = back * back * (3.0 - 2.0 * back);
        let (current, focus) = if back > 0.0 { (false, 1.0 - back) } else { (true, 1.0) };
        let color = crate::lerp_color(base, lit, focus);
        timed_line(painter, &g, cur, &timing, pos, focus, if current { 1.0 } else { focus }, big, look, base, color, prefs, None);
    }
}

/// The pause indicator between lines for the settings' cards: a gap on
/// loop, with the countdown at its end.
pub(crate) fn gap_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    // The ring (with its wave and the countdown's pop) fits the sample with
    // ~10 px around it, centred; fitted at the largest size, so the size
    // setting still shows.
    let base = if prefs.lyrics_gap_style >= 2 { 16.0 } else { 13.0 };
    let reach = base + 8.0;
    let k = ((rect.width().min(rect.height()) - 20.0) / (2.0 * reach * 1.5)).clamp(0.4, 1.4) * prefs.lyrics_gap_size.clamp(0.8, 1.5);
    let left = rect.center().x - 2.0 * k - base * k;
    let line = Rect::from_min_size(pos2(left, rect.center().y - 21.0 * k), vec2(56.0 * k, 42.0 * k));
    // A seven-second gap: four of the ring (its style is what is compared),
    // then the countdown when it is on.
    let pos = Duration::from_secs_f64(anim::now(ui.ctx()).rem_euclid(7.6));
    let bass = super::super::pages::settings_help::beat(anim::now(ui.ctx()) as f32) * 0.5;
    waiting(&painter, line, k, Duration::ZERO, Duration::from_secs(7), pos, ring_in(pos.as_secs_f32()), true, bass, 1.0, prefs);
}

/// The countdown in context for its setting's card: a sung line, the wait
/// under it (ring, then 3-2-1 when on) and the next line, which lights up
/// right when the count ends.
pub(crate) fn countdown_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const PAD: f32 = 12.0;
    // A 1.4 s ring, the count, then the next line sung for a moment.
    const GAP: f32 = 4.4;
    const LOOP: f64 = 6.2;
    let t = anim::now(ui.ctx()).rem_euclid(LOOP) as f32;
    let rest = 1.0 - (1.0 - 0.42) * prefs.lyrics_dim.clamp(0.0, 1.0);
    let dim = s.on_surface.gamma_multiply(rest);
    let small = PANE.small * 0.75;
    let k = 0.8 * prefs.lyrics_gap_size.clamp(0.8, 1.5);
    let row_h = 42.0 * k;
    let prev = fixed_layout(&painter, "Слышишь этот ритм", small);
    let next = fixed_layout(&painter, "Он ведёт меня", small);
    let total = prev.size().y + row_h + next.size().y + 8.0;
    let top = rect.center().y - total * 0.5;
    painter.galley_with_override_text_color(pos2(rect.left() + PAD, top), prev.clone(), dim);
    let line = Rect::from_min_size(pos2(rect.left() + PAD, top + prev.size().y + 4.0), vec2(56.0 * k, row_h));
    let bass = super::super::pages::settings_help::beat(anim::now(ui.ctx()) as f32) * 0.5;
    let pos = Duration::from_secs_f32(t);
    waiting(&painter, line, k, Duration::ZERO, Duration::from_secs_f32(GAP), pos, ring_in(t), true, bass, 1.0, prefs);
    // The next line: dim while waiting, sung once the wait is over.
    let on = ((t - GAP) / 0.18).clamp(0.0, 1.0);
    let color = crate::lerp_color(dim, s.on_surface, on);
    painter.galley_with_override_text_color(pos2(rect.left() + PAD, line.bottom() + 4.0), next, color);
}

/// The soft edges for their settings' card: lines rolling up through the
/// sample as the lyrics scroll, thinning over its top and bottom bands as
/// in the view (when off, cut by the card as before).
pub(crate) fn edges_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const TEXTS: [&str; 6] = ["Где-то рядом", "Слышишь этот ритм", "Он ведёт меня", "Сквозь огни домов", "До самого утра", "И дальше вперёд"];
    // A line every 1.6 s, each move eased out over half a second.
    const STEP: f64 = 1.6;
    let size = (rect.height() / 6.0).clamp(12.0, 20.0);
    // The lines rest on whole pixels, a whole number of them apart, and
    // roll between them ([`afloat`]).
    let ppp = painter.pixels_per_point();
    let whole = |v: f32| (v * ppp).round() / ppp;
    let pitch = whole(size * 1.8);
    let t = anim::now(ui.ctx()) / STEP;
    let n = t.floor() as i64;
    let x = ((t.fract() * STEP) / 0.5).min(1.0) as f32;
    let ease = 1.0 - (1.0 - x).powi(3);
    // Over what is seen of the card, as in the view.
    let seen = painter.clip_rect();
    let seen = if seen.is_positive() { seen } else { rect };
    let fade = prefs.lyrics_soft_edges.then(|| Fade::of(seen, Soft::of(prefs).text(pitch, size)));
    for k in -4i64..=5 {
        let g = fixed_layout(&painter, TEXTS[(n + k).rem_euclid(TEXTS.len() as i64) as usize], size);
        let top = whole(rect.center().y - g.size().y * 0.5) + (k as f32 + 1.0 - ease) * pitch;
        let alpha = if fade.is_some() {
            1.0
        } else {
            let band = rect.height() * 0.12;
            ((top + g.size().y - rect.top()) / band).min((rect.bottom() - top) / band).clamp(0.0, 1.0)
        };
        let rest = if k == 0 { 1.0 } else { 0.45 };
        fade_galley(&painter, pos2(rect.left() + 12.0, top), &g, s.on_surface.gamma_multiply(rest * alpha), fade);
    }
}

/// `lines` drawn as the lyrics view draws the current text, smaller (the
/// whole text behind the lyrics control), following the playing position;
/// a click seeks.
pub(crate) fn made_preview(ui: &mut Ui, size: egui::Vec2, state: &ViewState, lines: &[LyricLine], out: &mut Vec<Command>) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let look = style_for(&state.prefs, lines);
    // «Размер текста» too, as in the views.
    let scale = state.prefs.lyrics_scale.clamp(0.8, 1.4);
    let mut sizes = PANE;
    sizes.big = 22.0 * scale;
    sizes.small = 17.0 * scale;
    sizes.sub = 13.0 * scale;
    sizes.gap = 14.0 * scale;
    paint(ui, rect, &resp, state, lines, look, 0.0, sizes, false, &Stage::REAL, out);
}
