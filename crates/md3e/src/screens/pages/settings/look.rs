//! «Вид»: the theme, the track screen (cover, ring, background, the bar
//! and volume, the controls), Главная, the library and album, the window.
//! The stage beside the rows shows the track screen (Главная and the album
//! for their groups).

use egui::{Rect, Sense, Ui, vec2};
use nikgob_core::i18n;
use nikgob_shapes::paint as shape_paint;
use nikgob_view::presets::Area;
use nikgob_view::{self as v, Ambient, BgShapes, Command, CoverShape, HomeSectionSwitch, PrefPatch, Prefs};

use super::super::settings_help::{self, SettingId as Id};
use super::preset;
use super::rows::{self, Num, child, dimmed, group, item, menu, row, segments, sub, switch};
use crate::screens::{bg_shapes, now_playing};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Dialog, IconButton, Size, Style, Switch},
};

pub(super) fn show(ui: &mut Ui, p: &mut Prefs, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    group(ui, st.g_theme);
    menu(ui, Id::Language, p);
    segments(ui, Id::Theme, p);
    menu(ui, Id::Scheme, p);
    segments(ui, Id::Contrast, p);

    track_screen(ui, p);
    home(ui, p, out);

    group(ui, st.g_library_album);
    segments(ui, Id::AlbumDate, p);
    segments(ui, Id::LikedAlbumsPlace, p);

    group(ui, st.g_window);
    // Taken when let go: the window grows or shrinks under the pointer.
    let scale = Num::pct(*v::UI_SCALE_RANGE.start(), *v::UI_SCALE_RANGE.end(), v::UI_SCALE_STEP);
    if let Some(s) = rows::slider_on_release(ui, Id::UiScale, v::ui_scale(p.ui_scale), scale) {
        p.ui_scale = v::ui_scale(s);
    }
    switch(ui, Id::RailCompact, &mut p.rail_compact);
    switch(ui, Id::NavRoot, &mut p.nav_root);
    switch(ui, Id::ReducedMotion, &mut p.reduced_motion);
    switch(ui, Id::Tooltips, &mut p.tooltips);
    track_menu(ui, p);
    sub(ui, st.g_right_pane);
    switch(ui, Id::PaneLyrics, &mut p.pane_lyrics);
    switch(ui, Id::PaneInfo, &mut p.pane_info);
}

/// «Экран трека», in «Подробно» under its subtitles: Обложка · Кольцо ·
/// Фон · Полоса и громкость · Управление · Кнопки.
fn track_screen(ui: &mut Ui, p: &mut Prefs) {
    let st = i18n::settings_tabs();
    group(ui, st.g_track_screen);
    // Строго · Живо · Максимум · Свои: while one is under the pointer the
    // rows show what it would give (and keep none of it).
    let mut preview = preset::row(ui, Area::Track, p);
    let p: &mut Prefs = preview.as_mut().unwrap_or(p);
    sub(ui, st.g_cover);
    segments(ui, Id::CoverShape, p);
    shapes(ui, p);
    switch(ui, Id::CoverSpin, &mut p.cover_spin);
    let spin = p.cover_spin;
    child(ui, !spin, |ui| {
        segments(ui, Id::CoverDir, p);
        let num = Num::new(*v::COVER_SPIN_RANGE.start(), *v::COVER_SPIN_RANGE.end(), 1.0, 0, st.suffix_sec);
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

    sub(ui, st.g_ring);
    // Only a shaped cover has a ring.
    let square = p.cover_shape == CoverShape::Original;
    dimmed(ui, square, |ui| switch(ui, Id::Ring, &mut p.ring));
    let ring = p.ring && !square;
    child(ui, !ring, |ui| segments(ui, Id::RingStyle, p));

    sub(ui, st.g_background);
    background(ui, p);

    sub(ui, st.g_progress_volume);
    switch(ui, Id::ProgressWave, &mut p.progress_wave);
    segments(ui, Id::NpVolume, p);
    let pop = p.np_volume_pop && !p.np_volume;
    child(ui, !pop, |ui| {
        if let Some(v) = rows::slider(ui, Id::NpVolumeHold, p.np_volume_pop_hold, Num::new(0.3, 3.0, 0.1, 1, st.suffix_sec)) {
            p.np_volume_pop_hold = v;
            if !pop {
                // Tuning the pill turns it on.
                p.np_volume = false;
                p.np_volume_pop = true;
            }
        }
    });

    sub(ui, st.g_controls);
    switch(ui, Id::CoverClickPause, &mut p.cover_click_pause);
    menu(ui, Id::DblClickShape, p);
    let double = p.dbl_click_shape != 0;
    child(ui, !double, |ui| {
        if let Some(v) = rows::slider(ui, Id::TapWait, p.np_tap_wait, Num::new(0.0, 0.4, 0.02, 0, st.suffix_ms).times(1000.0)) {
            p.np_tap_wait = v;
        }
    });
    segments(ui, Id::NpMotion, p);

    // Play, previous and next always stay; the keys work whatever is off.
    sub(ui, st.g_buttons);
    for b in v::PlayerButton::ALL {
        switch(ui, Id::PlayerButton(b), p.player_buttons.of_mut(b));
    }
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
                    every = !all && ui.add(Button::new(i18n::common().all).style(Style::Outlined).id_salt("set-shapes-all")).clicked();
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
    group(ui, i18n::nav().home);
    home_sections(ui, p);
    switch(ui, Id::MixesProfiles, &mut p.mixes_profiles);
    switch(ui, Id::MixesEverything, &mut p.mixes_everything);
    switch(ui, Id::MixesMoods, &mut p.mixes_moods);
    switch(ui, Id::MixesOnRepeat, &mut p.mixes_on_repeat);
    menu(ui, Id::BackgroundCards, p);
    segments(ui, Id::ProfileNames, p);
    if !p.home_hidden_mixes.is_empty() {
        rows::titled(ui, Id::HiddenMixes, |ui| {
            for m in &p.home_hidden_mixes {
                rows::mark_row(ui, egui::Id::new(("set-unhide-mix", m.key)), &m.title, None, i18n::settings_tabs().restore, || {
                    out.push(Command::SetPref(PrefPatch::ShowMix(m.key)));
                });
            }
        });
    }
}

/// The places of «Меню трека», as [`v::TrackMenuPlace::ALL`] has them.
pub(super) fn track_menu_places() -> [&'static str; 3] {
    i18n::settings_tabs::track_menu_places_in(i18n::locale())
}
/// The column of the items' names in the list of «Меню трека».
const TRACK_MENU_NAME_W: f32 = 280.0;

/// «Меню трека»: a row that opens the list of the menu's items, each in
/// the menu, in «Файл» or hidden, and «Сбросить» under it.
fn track_menu(ui: &mut Ui, p: &mut Prefs) {
    let st = i18n::settings_tabs();
    let open_id = egui::Id::new("set-track-menu-open");
    row(ui, Id::TrackMenu, |ui| {
        if ui.add(Button::new(st.configure).style(Style::Outlined).size(Size::S).id_salt("set-track-menu")).clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(open_id, true));
        }
    });
    if !ui.ctx().data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false) {
        return;
    }
    let s = Theme::get(ui.ctx()).scheme;
    let mut open = true;
    let r = Dialog::new("set-track-menu-list", Id::TrackMenu.label()).max_width(640.0).action(i18n::common().close).show_with(ui.ctx(), &mut open, |ui| {
        egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
            for item in v::TrackMenuItem::ALL {
                ui.horizontal(|ui| {
                    let (name, _) = ui.allocate_exact_size(vec2(TRACK_MENU_NAME_W, 40.0), Sense::hover());
                    ui.painter().text(name.left_center(), egui::Align2::LEFT_CENTER, item.label(), TypeRole::BodyLarge.font_id(), s.on_surface);
                    let now = p.track_menu_place(item);
                    let at = v::TrackMenuPlace::ALL.iter().position(|x| *x == now).unwrap_or(0);
                    let (picked, _) = super::super::connected(ui, egui::Id::new(("set-track-menu-place", item)), &track_menu_places(), at, &[]);
                    if let Some(i) = picked {
                        p.set_track_menu_place(item, v::TrackMenuPlace::ALL[i.min(v::TrackMenuPlace::ALL.len() - 1)]);
                    }
                });
            }
        });
        ui.add_space(8.0);
        let reset = Button::new(i18n::common().reset).style(Style::Outlined).size(Size::S).enabled(!p.track_menu.is_empty());
        if ui.add(reset.id_salt("set-track-menu-reset")).clicked() {
            p.track_menu.clear();
        }
    });
    if r.action.is_some() || r.dismissed || !open {
        ui.ctx().data_mut(|d| d.remove::<bool>(open_id));
    }
}

/// «Разделы главной»: the seven sections in their order, each with «Выше»
/// / «Ниже» and its switch. While the row is shown the stage fits the
/// whole of Главная, so a toggle is visible.
fn home_sections(ui: &mut Ui, p: &mut Prefs) {
    let st = i18n::settings_tabs();
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
        // The arrows start on the line of the other rows' controls: the
        // name's column, the gap after it and the 16 before a control,
        // less this line's own 16 in and 4 between.
        let full = ui.available_width();
        let label_w = (rows::label_column(full) + ui.spacing().item_spacing.x - 4.0).min(full - 170.0);
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
                    let up = IconButton::new(Icon::ExpandLess).size(Size::Xs).enabled(i > 0).tooltip(st.up);
                    if ui.add(up.id_salt(("set-home-up", sec.key))).clicked() && i > 0 {
                        moved = Some((i, i - 1));
                    }
                    let down = IconButton::new(Icon::ExpandMore).size(Size::Xs).enabled(i + 1 < n).tooltip(st.down);
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
            // The tile with its hover state layer, as one opaque colour.
            let base = if on { s.secondary_container } else { s.surface_container_high };
            let tile = if resp.hovered() { crate::lerp_color(base, s.on_surface, 0.08) } else { base };
            painter.rect_filled(rect, 16.0, tile);
            if resp.hovered() {
                ui.ctx().request_repaint();
            }
            // Opaque too: the many-petal shapes' thin points overlap in the
            // fill's mesh, and a see-through colour showed every overlap as
            // a dark dot along their edges.
            let fill = if on { s.primary } else { crate::lerp_color(tile, s.on_surface_variant, 0.5) };
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
