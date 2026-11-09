//! «Вид»: the theme, the track screen (cover, ring, background, the bar
//! and volume, the controls), Главная, the library and album, the window.
//! The stage beside the rows shows the track screen (Главная and the album
//! for their groups).

use egui::{Rect, Sense, Ui, vec2};
use nikgob_shapes::paint as shape_paint;
use nikgob_view::presets::Area;
use nikgob_view::{self as v, Ambient, BgShapes, Command, CoverShape, HomeSectionSwitch, PrefPatch, Prefs, RingStyle};

use super::super::settings_help::{self, SettingId as Id};
use super::preset;
use super::rows::{self, Num, child, dimmed, group, item, menu, row, segments, sub, switch};
use crate::screens::{bg_shapes, now_playing};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, IconButton, Size, Style, Switch},
};

pub(super) fn show(ui: &mut Ui, p: &mut Prefs, out: &mut Vec<Command>) {
    group(ui, "Тема");
    segments(ui, Id::Theme, p);
    menu(ui, Id::Scheme, p);
    segments(ui, Id::Contrast, p);

    track_screen(ui, p);
    home(ui, p, out);

    group(ui, "Библиотека и альбом");
    segments(ui, Id::AlbumDate, p);
    segments(ui, Id::LikedAlbumsPlace, p);

    group(ui, "Окно");
    switch(ui, Id::RailCompact, &mut p.rail_compact);
    switch(ui, Id::ReducedMotion, &mut p.reduced_motion);
    switch(ui, Id::Tooltips, &mut p.tooltips);
    sub(ui, "Панель справа");
    switch(ui, Id::PaneQueue, &mut p.pane_queue);
    switch(ui, Id::PaneLyrics, &mut p.pane_lyrics);
    switch(ui, Id::PaneInfo, &mut p.pane_info);
}

/// «Экран трека», in «Подробно» under its subtitles: Обложка · Кольцо ·
/// Фон · Полоса и громкость · Управление.
fn track_screen(ui: &mut Ui, p: &mut Prefs) {
    group(ui, "Экран трека");
    // Строго · Живо · Максимум · Свои: while one is under the pointer the
    // rows show what it would give (and keep none of it).
    let mut preview = preset::row(ui, Area::Track, p);
    let p: &mut Prefs = preview.as_mut().unwrap_or(p);
    sub(ui, "Обложка");
    segments(ui, Id::CoverShape, p);
    shapes(ui, p);
    switch(ui, Id::CoverSpin, &mut p.cover_spin);
    let spin = p.cover_spin;
    child(ui, !spin, |ui| {
        segments(ui, Id::CoverDir, p);
        let num = Num::new(*v::COVER_SPIN_RANGE.start(), *v::COVER_SPIN_RANGE.end(), 1.0, 0, " с");
        if let Some(s) = rows::slider(ui, Id::CoverSpinSpeed, p.cover_spin_s, num) {
            p.cover_spin_s = s;
        }
    });
    switch(ui, Id::CoverPulse, &mut p.cover_pulse);
    let pulse = p.cover_pulse;
    child(ui, !pulse, |ui| {
        let num = Num::pct(*v::COVER_PULSE_RANGE.start(), *v::COVER_PULSE_RANGE.end(), 0.01);
        if let Some(s) = rows::slider(ui, Id::CoverPulseStrength, p.cover_pulse_strength, num) {
            p.cover_pulse_strength = s;
        }
    });
    // The square cover has no shape to change.
    dimmed(ui, p.cover_shape == CoverShape::Original, |ui| switch(ui, Id::CoverPauseMorph, &mut p.cover_pause_morph));

    sub(ui, "Кольцо");
    switch(ui, Id::Ring, &mut p.ring);
    let ring = p.ring;
    child(ui, !ring, |ui| {
        segments(ui, Id::RingStyle, p);
        // The bars stand on fixed angles, like a spectrum.
        let bars = p.ring_style == RingStyle::Bars;
        dimmed(ui, bars, |ui| switch(ui, Id::RingSpin, &mut p.spin_ring));
        let spins = p.spin_ring && !bars;
        child(ui, !spins, |ui| {
            segments(ui, Id::RingDir, p);
            let num = Num::new(*v::RING_SPIN_RANGE.start(), *v::RING_SPIN_RANGE.end(), 1.0, 0, " с");
            if let Some(s) = rows::slider(ui, Id::RingSpinSpeed, p.ring_spin_s, num) {
                p.ring_spin_s = s;
            }
        });
        switch(ui, Id::RingBeat, &mut p.ring_beat);
    });

    sub(ui, "Фон");
    background(ui, p);

    sub(ui, "Полоса и громкость");
    switch(ui, Id::ProgressWave, &mut p.progress_wave);
    segments(ui, Id::NpVolume, p);
    let pop = p.np_volume_pop && !p.np_volume;
    child(ui, !pop, |ui| {
        if let Some(v) = rows::slider(ui, Id::NpVolumeHold, p.np_volume_pop_hold, Num::new(0.3, 3.0, 0.1, 1, " с")) {
            p.np_volume_pop_hold = v;
            if !pop {
                // Tuning the pill turns it on.
                p.np_volume = false;
                p.np_volume_pop = true;
            }
        }
    });

    sub(ui, "Управление");
    switch(ui, Id::CoverClickPause, &mut p.cover_click_pause);
    menu(ui, Id::DblClickShape, p);
    let double = p.dbl_click_shape != 0;
    child(ui, !double, |ui| {
        if let Some(v) = rows::slider(ui, Id::TapWait, p.np_tap_wait, Num::new(0.0, 0.4, 0.02, 0, " мс").times(1000.0)) {
            p.np_tap_wait = v;
        }
    });
    segments(ui, Id::NpMotion, p);
}

/// The grid of shapes under «Форма обложки»: the random one's pool, or the
/// one shape; nothing for the square cover.
fn shapes(ui: &mut Ui, p: &mut Prefs) {
    match p.cover_shape {
        CoverShape::Original => {}
        CoverShape::Random => {
            let pool = now_playing::random_pool(&p.random_shapes);
            let all = pool.len() == CoverShape::shapes().len();
            item(ui, Id::ShapeRange, |ui| {
                let mut every = false;
                let picked = shape_grid(ui, Id::ShapeRange, |c| pool.contains(&c), |ui| {
                    every = !all && ui.add(Button::new("Все").style(Style::Outlined).id_salt("set-shapes-all")).clicked();
                });
                if every {
                    p.random_shapes.clear();
                } else if let Some(c) = picked {
                    let mut next: Vec<CoverShape> = pool.clone();
                    if let Some(i) = next.iter().position(|x| *x == c) {
                        // At least one shape stays.
                        if next.len() > 1 {
                            next.remove(i);
                        }
                    } else {
                        next.push(c);
                    }
                    p.random_shapes = if next.len() == CoverShape::shapes().len() {
                        Vec::new()
                    } else {
                        CoverShape::shapes().iter().copied().filter(|x| next.contains(x)).collect()
                    };
                }
            });
        }
        current => {
            item(ui, Id::ShapePick, |ui| {
                if let Some(c) = shape_grid(ui, Id::ShapePick, |c| c == current, |_| {}) {
                    p.cover_shape = c;
                }
            });
        }
    }
}

/// Ambient clouds and the shapes behind Now Playing. The fields of each
/// are always shown: faded while it is off, and touching one turns it on.
fn background(ui: &mut Ui, p: &mut Prefs) {
    segments(ui, Id::Ambient, p);
    let clouds_off = p.ambient == Ambient::Off;
    child(ui, clouds_off, |ui| {
        if let Some(v) = rows::slider(ui, Id::AmbientStrength, p.ambient_strength, Num::pct(0.25, 2.0, 0.05)) {
            p.ambient_strength = v;
            if clouds_off {
                p.ambient = Ambient::Reactive;
            }
        }
    });

    let off = p.bg_shapes == BgShapes::Off;
    menu(ui, Id::BgShapes, p);
    let fields = |p: &Prefs| {
        (p.bg_shapes_count, p.bg_shapes_size, p.bg_shapes_opacity, p.bg_shapes_speed, p.bg_shapes_react, p.bg_shapes_colors, p.bg_shapes_hue)
    };
    let before = fields(p);
    child(ui, off, |ui| {
        if let Some(v) = rows::slider(ui, Id::BgCount, p.bg_shapes_count as f32, Num::new(4.0, 32.0, 1.0, 0, "")) {
            p.bg_shapes_count = v as u32;
        }
        if let Some(v) = rows::slider(ui, Id::BgSize, p.bg_shapes_size, Num::pct(0.5, 2.0, 0.05)) {
            p.bg_shapes_size = v;
        }
        if let Some(v) = rows::slider(ui, Id::BgOpacity, p.bg_shapes_opacity, Num::pct(0.25, 2.0, 0.05)) {
            p.bg_shapes_opacity = v;
        }
        if let Some(v) = rows::slider(ui, Id::BgSpeed, p.bg_shapes_speed, Num::pct(0.25, 3.0, 0.05)) {
            p.bg_shapes_speed = v;
        }
        if let Some(v) = rows::slider(ui, Id::BgReact, p.bg_shapes_react, Num::pct(0.0, 2.0, 0.05)) {
            p.bg_shapes_react = v;
        }
        segments(ui, Id::BgColors, p);
        row(ui, Id::BgHue, |ui| {
            if let Some(i) = swatches(ui, p) {
                p.bg_shapes_hue = i;
                p.bg_shapes_colors = 3;
            }
        });
    });
    if off && fields(p) != before {
        p.bg_shapes = BgShapes::Float;
    }
}

/// «Главная»: what its mixes are and how they open.
fn home(ui: &mut Ui, p: &mut Prefs, out: &mut Vec<Command>) {
    group(ui, "Главная");
    home_sections(ui, p);
    switch(ui, Id::MixesProfiles, &mut p.mixes_profiles);
    switch(ui, Id::MixesEverything, &mut p.mixes_everything);
    switch(ui, Id::MixesMoods, &mut p.mixes_moods);
    switch(ui, Id::MixesOnRepeat, &mut p.mixes_on_repeat);
    menu(ui, Id::BackgroundCards, p);
    segments(ui, Id::ProfileNames, p);
    switch(ui, Id::MixClickOpensList, &mut p.mix_click_opens_list);
    if !p.home_hidden_mixes.is_empty() {
        rows::titled(ui, Id::HiddenMixes, |ui| {
            for m in &p.home_hidden_mixes {
                rows::mark_row(ui, egui::Id::new(("set-unhide-mix", m.key)), &m.title, None, "Вернуть", || {
                    out.push(Command::SetPref(PrefPatch::ShowMix(m.key)));
                });
            }
        });
    }
}

/// «Разделы главной»: the seven sections in their order, each with «Выше»
/// / «Ниже» and its switch. The lines are the row's options (the stage
/// outlines the section under the pointer).
fn home_sections(ui: &mut Ui, p: &mut Prefs) {
    let _ = item(ui, Id::HomeSections, |ui| {
        let s = Theme::get(ui.ctx()).scheme;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(TypeRole::BodyLarge.style().rich(Id::HomeSections.label(), s.on_surface));
            settings_help::help_button(ui, Id::HomeSections);
        });
        ui.add_space(4.0);
        let mut order = p.home_order();
        let label_w = rows::label_column(ui.available_width()) - 16.0;
        let n = order.len();
        let mut moved: Option<(usize, usize)> = None;
        let mut lines: Vec<Rect> = Vec::with_capacity(n);
        for (i, sec) in order.iter_mut().enumerate() {
            let line = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(16.0);
                    let (name, _) = ui.allocate_exact_size(vec2(label_w.max(120.0), 40.0), Sense::hover());
                    let color = if sec.on { s.on_surface } else { s.on_surface_variant };
                    ui.painter().text(name.left_center(), egui::Align2::LEFT_CENTER, sec.key.label(), TypeRole::BodyLarge.font_id(), color);
                    let up = IconButton::new(Icon::ExpandLess).size(Size::Xs).enabled(i > 0).tooltip("Выше");
                    if ui.add(up.id_salt(("set-home-up", sec.key))).clicked() && i > 0 {
                        moved = Some((i, i - 1));
                    }
                    let down = IconButton::new(Icon::ExpandMore).size(Size::Xs).enabled(i + 1 < n).tooltip("Ниже");
                    if ui.add(down.id_salt(("set-home-down", sec.key))).clicked() && i + 1 < n {
                        moved = Some((i, i + 1));
                    }
                    ui.add_space(8.0);
                    ui.add(Switch::new(&mut sec.on).id_salt(("set-home-on", sec.key)));
                })
                .response
                .rect;
            lines.push(line);
        }
        settings_help::report_choices(ui.ctx(), lines);
        if let Some((a, b)) = moved {
            order.swap(a, b);
        }
        let next = HomeSectionSwitch::complete(&order);
        if next != p.home_order() {
            p.home_sections = next;
        }
        ui.add_space(4.0);
    });
}

/// Tiles with every cover shape drawn, for picking one or a set, then
/// `extra` and the row's ⓘ on the tiles' line. The hovered tile turns its
/// shape, as it would in the player. Returns the clicked shape.
fn shape_grid(ui: &mut Ui, id: Id, selected: impl Fn(CoverShape) -> bool, extra: impl FnOnce(&mut Ui)) -> Option<CoverShape> {
    let s = Theme::get(ui.ctx()).scheme;
    let time = ui.ctx().input(|i| i.time) as f32;
    let mut clicked = None;
    ui.add_space(4.0);
    let mut tiles: Vec<Rect> = Vec::with_capacity(CoverShape::shapes().len());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for &c in CoverShape::shapes() {
            let Some(m) = now_playing::material(c) else {
                tiles.push(Rect::NOTHING);
                continue;
            };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), Sense::hover());
            tiles.push(rect);
            let resp = ui.interact(rect, egui::Id::new((id, c)), Sense::click()).on_hover_text(c.label());
            let on = selected(c);
            let painter = ui.painter();
            painter.rect_filled(rect, 16.0, if on { s.secondary_container } else { s.surface_container_high });
            if resp.hovered() {
                painter.rect_filled(rect, 16.0, s.on_surface.gamma_multiply(0.08));
                ui.ctx().request_repaint();
            }
            let fill = if on { s.primary } else { s.on_surface_variant.gamma_multiply(0.5) };
            let turn = if resp.hovered() { time * 0.9 } else { 0.0 };
            shape_paint::fill_shape(painter, now_playing::shape_cubics(m), rect.shrink(9.0), turn, fill);
            if resp.clicked() {
                clicked = Some(c);
            }
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            extra(ui);
            settings_help::help_button(ui, id);
        });
    });
    // The shapes are the row's options, in `CoverShape::shapes` order (the
    // stage shows the one under the pointer).
    settings_help::report_choices(ui.ctx(), tiles);
    ui.add_space(8.0);
    clicked
}

/// The hue swatches for the shapes, in the theme's tones. Returns a click.
fn swatches(ui: &mut Ui, p: &Prefs) -> Option<u8> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut picked = None;
    let mut rects = Vec::new();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for i in 0..bg_shapes::HUES.len() as u8 {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::hover());
            rects.push(rect);
            let resp = ui.interact(rect, egui::Id::new(("set-bg-hue", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let on = p.bg_shapes_colors == 3 && p.bg_shapes_hue == i;
            let color = bg_shapes::hue_color(&s, i);
            let painter = ui.painter();
            let r = if resp.hovered() || on { 12.0 } else { 10.0 };
            painter.circle_filled(rect.center(), r, color);
            if on {
                painter.circle_stroke(rect.center(), 13.5, egui::Stroke::new(2.0, s.on_surface));
            }
            if resp.clicked() {
                picked = Some(i);
            }
        }
    });
    // The swatches are the row's options (the ⓘ card, the stage).
    settings_help::report_choices(ui.ctx(), rects);
    picked
}
