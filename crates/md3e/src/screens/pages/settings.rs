//! Настройки (md3e-player.md §3.8): grouped list rows; every change is sent
//! as a whole new `Prefs`.

use egui::{Sense, Ui};
use nikgob_shapes::paint as shape_paint;
use nikgob_view::{Ambient, BgShapes, Command, CoverShape, Prefs, RingStyle, Route, ViewState};

use super::settings_help::{self, SettingId as Id};
use super::{PageState, connected};
use crate::screens::{bg_shapes, common::plural, now_playing};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Divider, FilterChip, Menu, MenuItem, Slider, Style, Switch, TextField},
};

pub(super) fn group(ui: &mut Ui, title: &str) -> egui::Response {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(24.0);
    let header = ui.label(TypeRole::TitleSmall.style().rich(title, s.primary));
    ui.add_space(4.0);
    header
}

/// A 56/72 row: label (+ supporting) on the left, a control on the right.
/// In a narrow page the control goes under the text.
pub(super) fn row(ui: &mut Ui, id: Id, label: &str, supporting: Option<&str>, control: impl FnOnce(&mut Ui)) {
    settings_help::item(ui, id, label, |ui| row_body(ui, id, label, supporting, control));
}

/// The row's label with its ⓘ right after the last word.
fn row_label(ui: &mut Ui, id: Id, label: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(TypeRole::BodyLarge.style().rich(label, s.on_surface));
        settings_help::help_button(ui, id);
    });
}

/// The control, reported so the row's cards line up with it. Beside the
/// text its width is kept for the next frame's layout (under the text a
/// control may stretch, which says nothing about the room it needs).
fn row_control(ui: &mut Ui, id: Id, beside: bool, control: impl FnOnce(&mut Ui)) {
    let r = ui.scope(control).response.rect;
    settings_help::report_control(ui.ctx(), r);
    if beside {
        ui.ctx().data_mut(|d| d.insert_temp(control_width_key(id), r.width()));
    }
}

fn control_width_key(id: Id) -> egui::Id {
    egui::Id::new(("settings-control-w", id))
}

fn row_body(ui: &mut Ui, id: Id, label: &str, supporting: Option<&str>, control: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    let label_w = (ui.available_width() * 0.4).clamp(180.0, 320.0);
    // A control wider than the room beside the text goes under it, as in a
    // narrow page (three segments do not fit beside the text at 720 px).
    let control_w: f32 = ui.ctx().data(|d| d.get_temp(control_width_key(id))).unwrap_or(0.0);
    let needed = label_w + settings_help::BTN + 4.0 + 16.0 + control_w;
    if ui.available_width() < 540.0 || needed > ui.available_width() + 1.0 {
        ui.add_space(8.0);
        row_label(ui, id, label);
        if let Some(t) = supporting {
            ui.label(TypeRole::BodyMedium.style().rich_para(t, s.on_surface_variant));
        }
        ui.add_space(4.0);
        row_control(ui, id, false, control);
        ui.add_space(8.0);
        return;
    }
    let h = if supporting.is_some() { 72.0 } else { 56.0 };
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), h),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.vertical(|ui| {
                // Room for the ⓘ after the longest labels.
                ui.set_width(label_w + settings_help::BTN + 4.0);
                // Centre the text block against the control.
                let block = if supporting.is_some() { 44.0 } else { 24.0 };
                ui.add_space(((h - block) * 0.5).max(0.0));
                row_label(ui, id, label);
                if let Some(t) = supporting {
                    ui.label(TypeRole::BodyMedium.style().rich_para(t, s.on_surface_variant));
                }
            });
            ui.add_space(16.0);
            row_control(ui, id, true, control);
        },
    );
}

/// Segmented buttons for a choice row (labels and values from
/// [`Id::choices`]); hovering one previews it in the row's card.
pub(super) fn choice(ui: &mut Ui, id: Id, p: &mut Prefs) {
    let labels: Vec<&str> = id.choices().iter().map(|c| c.label).collect();
    let (pick, rects) = connected(ui, egui::Id::new(("set-choice", id)), &labels, id.chosen(p), &[]);
    settings_help::report_choices(ui.ctx(), rects);
    if let Some(n) = pick {
        id.choose(p, n);
    }
}

/// A menu chip for a choice row with long labels.
fn choice_menu(ui: &mut Ui, id: Id, p: &mut Prefs) {
    let cur = id.chosen(p);
    let options: Vec<(String, bool)> =
        id.choices().iter().enumerate().map(|(i, c)| (c.label.to_owned(), i == cur)).collect();
    let salt = format!("set-menu-{id:?}");
    let pick = dropdown(ui, &salt, id.choices()[cur].label, &options);
    // Hovering an item of the open menu explains it.
    if let Some(rects) = Menu::item_rects(ui.ctx(), (salt.as_str(), "menu")) {
        settings_help::report_menu(ui.ctx(), rects);
    }
    if let Some(n) = pick {
        id.choose(p, n);
    }
}

/// A chip that opens a menu of `options` (label, selected). Returns the pick.
fn dropdown(ui: &mut Ui, id: &str, current: &str, options: &[(String, bool)]) -> Option<usize> {
    let resp = ui.add(FilterChip::new(ellipsize(current, 30), false).trailing_icon(Icon::ExpandMore).id_salt(id));
    let mut menu = Menu::new((id, "menu"));
    for (label, selected) in options {
        menu = menu.item(MenuItem::new(ellipsize(label, 34)).selected(*selected));
    }
    menu.attach(&resp)
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

/// A 0..=1 slider over `lo..=hi` that sends whole steps of `step`.
pub(super) fn range_slider(
    ui: &mut Ui,
    id: &str,
    value: f32,
    (lo, hi): (f32, f32),
    step: f32,
    label: impl Fn(f32) -> String + 'static,
) -> Option<f32> {
    let frac = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let o = Slider::new(id, frac)
        .width(240.0)
        .track(8.0)
        .value_label(move |v| label(snap(lo + v * (hi - lo), step)))
        .show(ui);
    o.committed.or(o.dragging).map(|v| snap(lo + v * (hi - lo), step))
}

fn snap(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

/// KiB/s slider mapping: quadratic, so the low end is fine-grained.
const SPEED_MAX: f32 = 50_000.0;

fn speed_step(raw: f32) -> f32 {
    if raw < 1_000.0 {
        50.0
    } else if raw < 10_000.0 {
        250.0
    } else {
        1_000.0
    }
}

pub(super) fn fmt_speed(kbps: u32) -> String {
    match kbps {
        0 => "без лимита".to_owned(),
        k if k < 1024 => format!("{k} КиБ/с"),
        k => format!("{:.1} МиБ/с", k as f32 / 1024.0),
    }
}

pub(super) fn speed_slider(ui: &mut Ui, id: &str, kbps: u32) -> Option<u32> {
    let frac = (kbps as f32 / SPEED_MAX).sqrt().clamp(0.0, 1.0);
    let at = |v: f32| {
        let raw = v * v * SPEED_MAX;
        snap(raw, speed_step(raw)) as u32
    };
    let o = Slider::new(id, frac)
        .width(240.0)
        .track(8.0)
        .value_label(move |v| fmt_speed(at(v)))
        .show(ui);
    o.committed.or(o.dragging).map(at)
}

/// Tiles with every cover shape drawn, for picking one or a set. The
/// hovered tile turns its shape, as it would in the player. Returns the
/// clicked shape.
fn shape_grid(ui: &mut Ui, id: &str, selected: impl Fn(CoverShape) -> bool) -> Option<CoverShape> {
    let s = Theme::get(ui.ctx()).scheme;
    let time = ui.ctx().input(|i| i.time) as f32;
    let mut clicked = None;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for &c in CoverShape::shapes() {
            let Some(m) = now_playing::material(c) else { continue };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), Sense::hover());
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
    });
    ui.add_space(8.0);
    clicked
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut p: Prefs = state.prefs.clone();
    let before = p.clone();
    ui.set_max_width(ui.available_width().min(840.0));
    ui.add_space(8.0);
    ui.label(TypeRole::HeadlineMedium.style().rich("Настройки", s.on_surface));
    ui.add_space(16.0);
    if let Some(id) = pages.focus_setting.take() {
        pages.settings_tab = id.tab();
        settings_help::begin_focus(ui.ctx(), id);
    }
    let tab = pages.settings_tab.min(settings_help::TABS.len() - 1);
    if let (Some(n), _) = connected(ui, egui::Id::new("set-tab"), &settings_help::TABS, tab, &[]) {
        pages.settings_tab = n;
    }

    match pages.settings_tab {
        settings_help::TAB_APPEARANCE => appearance(ui, &mut p),
        settings_help::TAB_SOURCES => super::sources::show(ui, state, &mut p, pages, out),
        _ => general(ui, state, &mut p, pages, out),
    }
    ui.add(Divider::new());
    settings_help::finish(ui, &mut p);

    if p != before {
        out.push(Command::SetPrefs(Box::new(p)));
    }
    None
}

/// Everything about looks: theme, the player, the cover and its ring, the
/// background and the lyrics' animation.
fn appearance(ui: &mut Ui, p: &mut Prefs) {
    group(ui, "Тема и цвета");
    row(ui, Id::Design, "Дизайн", Some("Nothing и Liquid Glass — скоро"), |ui| {
        // Only MD3E exists so far; the others are shown but off.
        connected(ui, egui::Id::new("set-design"), &["Material 3", "Nothing", "Glass"], 0, &[1, 2]);
    });
    row(ui, Id::Theme, "Тема", None, |ui| choice(ui, Id::Theme, p));
    row(ui, Id::Contrast, "Контраст", None, |ui| choice(ui, Id::Contrast, p));
    row(ui, Id::Scheme, "Схема цвета", Some("Цвета приложения берутся из обложки"), |ui| {
        choice_menu(ui, Id::Scheme, p);
    });

    group(ui, "Сейчас играет");
    row(ui, Id::ProgressWave, "Волна на полосе прогресса", Some("Выключи — прогресс всегда ровная линия"), |ui| {
        ui.add(Switch::new(&mut p.progress_wave).id_salt("set-progress-wave"));
    });
    row(ui, Id::NpVolume, "Громкость", Some("Где регулируется звук в открытом треке"), |ui| choice(ui, Id::NpVolume, p));
    let pop = p.np_volume_pop && !p.np_volume;
    dimmed(ui, !pop, |ui| {
        row(ui, Id::NpVolumeHold, "Плашка исчезает через", Some("Для варианта «При изменении»"), |ui| {
            let hold = p.np_volume_pop_hold.clamp(0.3, 3.0);
            if let Some(v) = range_slider(ui, "set-np-volume-hold", hold, (0.3, 3.0), 0.1, |v| format!("{v:.1} с")) {
                p.np_volume_pop_hold = v;
                if !pop {
                    // Tuning the pill turns it on.
                    p.np_volume = false;
                    p.np_volume_pop = true;
                }
            }
        });
    });

    group(ui, "Обложка");
    row(ui, Id::CoverShape, "Форма обложки", Some("Пока играет музыка; на паузе — квадрат"), |ui| {
        choice(ui, Id::CoverShape, p);
    });
    match p.cover_shape {
        CoverShape::Original => {}
        CoverShape::Random => {
            let pool = now_playing::random_pool(&p.random_shapes);
            let all = pool.len() == CoverShape::shapes().len();
            row(
                ui,
                Id::ShapeRange,
                "Из каких фигур выбирать",
                Some(&format!(
                    "{} из {} — новая на каждом треке и по двойному клику",
                    pool.len(),
                    CoverShape::shapes().len()
                )),
                |ui| {
                    if !all && ui.add(Button::new("Все").style(Style::Outlined).id_salt("set-shapes-all")).clicked() {
                        p.random_shapes.clear();
                    }
                },
            );
            if let Some(c) = shape_grid(ui, "set-shapes-range", |c| pool.contains(&c)) {
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
        }
        current => {
            row(ui, Id::ShapePick, "Фигура", Some(current.label()), |_| {});
            if let Some(c) = shape_grid(ui, "set-shape-fixed", |c| c == current) {
                p.cover_shape = c;
            }
        }
    }
    row(ui, Id::CoverSpin, "Вращать форму", Some("Край поворачивается, картинка стоит"), |ui| {
        ui.add(Switch::new(&mut p.cover_spin).id_salt("set-cover-spin"));
    });
    if p.cover_spin {
        row(ui, Id::CoverDir, "Направление формы", None, |ui| choice(ui, Id::CoverDir, p));
    }
    row(ui, Id::CoverPulse, "Пульсация в такт", Some("Обложка вздрагивает в такт музыке"), |ui| {
        ui.add(Switch::new(&mut p.cover_pulse).id_salt("set-cover-pulse"));
    });

    group(ui, "Кольцо вокруг обложки");
    row(ui, Id::Ring, "Кольцо", None, |ui| {
        ui.add(Switch::new(&mut p.ring).id_salt("set-ring-on"));
    });
    if p.ring {
        row(ui, Id::RingStyle, "Вид кольца", Some("Как выглядят точки вокруг обложки"), |ui| {
            choice(ui, Id::RingStyle, p);
        });
        // The bars stand on fixed angles, like a spectrum.
        if p.ring_style != RingStyle::Bars {
            row(ui, Id::RingSpin, "Кольцо крутится", Some("Пока играет музыка"), |ui| {
                ui.add(Switch::new(&mut p.spin_ring).id_salt("set-ring"));
            });
            if p.spin_ring {
                row(ui, Id::RingDir, "Направление кольца", None, |ui| choice(ui, Id::RingDir, p));
            }
        }
        row(ui, Id::RingBeat, "Кольцо в такт музыке", Some("Точки подскакивают на ударных"), |ui| {
            ui.add(Switch::new(&mut p.ring_beat).id_salt("set-ring-beat"));
        });
    }

    background(ui, p);

    group(ui, "Текст песен");
    row(ui, Id::LyricsStyle, "Анимация", Some("Как выделяется строка, которую поют"), |ui| {
        choice(ui, Id::LyricsStyle, p);
    });
    if p.lyrics_style == 2 {
        row(ui, Id::WaveFallback, "Если нет разметки по слогам", Some("Для анимации «Волна»"), |ui| {
            choice_menu(ui, Id::WaveFallback, p);
        });
        row(ui, Id::WaveJump, "Высота прыжка букв", Some("0 — буквы только подсвечиваются"), |ui| {
            if let Some(v) = range_slider(ui, "set-lyrics-jump", p.lyrics_wave_jump, (0.0, 2.0), 0.1, |v| format!("{v:.1}")) {
                p.lyrics_wave_jump = v;
            }
        });
    }
    row(ui, Id::LyricsDim, "Приглушение остальных строк", Some("0% — без приглушения, 100% — сильное"), |ui| {
        if let Some(v) = range_slider(ui, "set-lyrics-dim", p.lyrics_dim, (0.0, 1.0), 0.05, |v| format!("{:.0}%", v * 100.0)) {
            p.lyrics_dim = v;
        }
    });
    row(ui, Id::LyricsScale, "Размер текста", None, |ui| {
        if let Some(v) = range_slider(ui, "set-lyrics-scale", p.lyrics_scale, (0.8, 1.4), 0.05, |v| format!("{:.0}%", v * 100.0)) {
            p.lyrics_scale = v;
        }
    });
    row(ui, Id::LyricsCascade, "Каскад строк", Some("Дальние строки следуют за прокруткой с задержкой"), |ui| {
        ui.add(Switch::new(&mut p.lyrics_cascade).id_salt("set-lyrics-cascade"));
    });
    row(ui, Id::LyricsGlow, "Свечение текста", Some("Лёгкое свечение спетой части"), |ui| {
        ui.add(Switch::new(&mut p.lyrics_glow).id_salt("set-lyrics-glow"));
    });
    row(ui, Id::LyricsGap, "Индикатор паузы", Some("Кружок, пока в песне нет слов"), |ui| {
        choice_menu(ui, Id::LyricsGap, p);
    });
    if matches!(p.lyrics_gap_style, 1 | 3) {
        row(ui, Id::LyricsGapWaves, "Длина волны", Some("Больше — волны мельче и чаще"), |ui| {
            if let Some(v) = range_slider(ui, "set-lyrics-gap-waves", p.lyrics_gap_waves, (3.0, 16.0), 1.0, |v| format!("{v:.0} гребней")) {
                p.lyrics_gap_waves = v;
            }
        });
    }
    row(ui, Id::LyricsCountdown, "Обратный отсчёт 3-2-1 в конце паузы", None, |ui| {
        ui.add(Switch::new(&mut p.lyrics_countdown).id_salt("set-lyrics-countdown"));
    });

    group(ui, "Навигация");
    row(
        ui,
        Id::RailCompact,
        "Боковая панель всегда компактная",
        Some("Только значки, без кнопки раскрытия"),
        |ui| {
            ui.add(Switch::new(&mut p.rail_compact).id_salt("set-rail-compact"));
        },
    );

    group(ui, "Движение");
    row(ui, Id::NpMotion, "Открытие экрана", Some("Как экран трека открывается и закрывается"), |ui| {
        choice(ui, Id::NpMotion, p);
    });
    row(ui, Id::ReducedMotion, "Меньше движения", Some("Без отскоков, вращения и волн"), |ui| {
        ui.add(Switch::new(&mut p.reduced_motion).id_salt("set-rm"));
    });
    row(ui, Id::FpsLimit, "Лимит кадров", Some(&format!("{} FPS", p.fps_limit)), |ui| {
        if let Some(v) = range_slider(ui, "set-fps", p.fps_limit as f32, (30.0, 144.0), 1.0, |v| format!("{v:.0}")) {
            p.fps_limit = v as u32;
        }
    });
}

/// Ambient clouds and the shapes behind Now Playing. The shapes' fields are
/// always shown: dimmed while they are off, and touching one turns them on.
fn background(ui: &mut Ui, p: &mut Prefs) {
    group(ui, "Фон");
    row(ui, Id::Ambient, "Ambient", Some("Цветные пятна за экраном трека"), |ui| choice(ui, Id::Ambient, p));
    let clouds_off = p.ambient == Ambient::Off;
    dimmed(ui, clouds_off, |ui| {
        row(ui, Id::AmbientStrength, "Яркость облаков", None, |ui| {
            if let Some(v) = range_slider(ui, "set-ambient-strength", p.ambient_strength, (0.25, 2.0), 0.05, pct) {
                p.ambient_strength = v;
                if clouds_off {
                    p.ambient = Ambient::Reactive;
                }
            }
        });
    });

    let off = p.bg_shapes == BgShapes::Off;
    row(ui, Id::BgShapes, "Фигуры на фоне", Some(match p.bg_shapes {
        BgShapes::Off => "Выключены — тронь параметр ниже, и они включатся",
        BgShapes::Bounce => "Лежат внизу и подпрыгивают на долях",
        BgShapes::Float => "Парят по экрану и светятся в такт",
        BgShapes::Morph => "Плывут и превращаются в другие фигуры на ударах",
    }), |ui| choice(ui, Id::BgShapes, p));
    let fields = |p: &Prefs| {
        (p.bg_shapes_count, p.bg_shapes_size, p.bg_shapes_opacity, p.bg_shapes_speed, p.bg_shapes_react, p.bg_shapes_colors, p.bg_shapes_hue)
    };
    let before = fields(p);
    dimmed(ui, off, |ui| {
        row(ui, Id::BgCount, "Количество", Some(&format!("{} фигур", p.bg_shapes_count)), |ui| {
            if let Some(v) = range_slider(ui, "set-bg-count", p.bg_shapes_count as f32, (4.0, 32.0), 1.0, |v| format!("{v:.0}")) {
                p.bg_shapes_count = v as u32;
            }
        });
        row(ui, Id::BgSize, "Размер", None, |ui| {
            if let Some(v) = range_slider(ui, "set-bg-size", p.bg_shapes_size, (0.5, 2.0), 0.05, pct) {
                p.bg_shapes_size = v;
            }
        });
        row(ui, Id::BgOpacity, "Яркость", None, |ui| {
            if let Some(v) = range_slider(ui, "set-bg-opacity", p.bg_shapes_opacity, (0.25, 2.0), 0.05, pct) {
                p.bg_shapes_opacity = v;
            }
        });
        row(ui, Id::BgSpeed, "Скорость", Some("Как быстро двигаются фигуры"), |ui| {
            if let Some(v) = range_slider(ui, "set-bg-speed", p.bg_shapes_speed, (0.25, 3.0), 0.05, pct) {
                p.bg_shapes_speed = v;
            }
        });
        row(ui, Id::BgReact, "Реакция на музыку", Some("0% — двигаются сами по себе"), |ui| {
            if let Some(v) = range_slider(ui, "set-bg-react", p.bg_shapes_react, (0.0, 2.0), 0.05, pct) {
                p.bg_shapes_react = v;
            }
        });
        row(ui, Id::BgColors, "Цвета", Some("Тема, акцент, яркие или свой оттенок"), |ui| {
            choice(ui, Id::BgColors, p);
        });
        let hue = (p.bg_shapes_colors == 3).then(|| bg_shapes::HUES[usize::from(p.bg_shapes_hue).min(7)].1);
        row(ui, Id::BgHue, "Свой цвет", Some(hue.unwrap_or("Клик по цвету включает режим «Свой»")), |ui| {
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

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

/// Rows drawn faded while what they tune is off (they still work).
pub(super) fn dimmed(ui: &mut Ui, dim: bool, add: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        if dim {
            ui.multiply_opacity(0.55);
        }
        add(ui);
    });
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
            // The help card names and shows the colour.
            let resp = ui
                .interact(rect, egui::Id::new(("set-bg-hue", i)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
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
    // Hovering a swatch previews it.
    settings_help::report_choices(ui.ctx(), rects);
    picked
}

/// Behaviour: controls, lyrics, sound, library, import and quality
/// (streaming and torrents live on «Источники»).
fn general(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    group(ui, "Запуск");
    row(ui, Id::Autostart, "Запускать вместе с Windows", Some("Когда входишь в систему"), |ui| {
        ui.add(Switch::new(&mut p.autostart).id_salt("set-autostart"));
    });
    dimmed(ui, !p.autostart, |ui| {
        row(ui, Id::AutostartHidden, "Сразу в трей", Some("Без окна: музыка ждёт в значке у часов"), |ui| {
            ui.add(Switch::new(&mut p.autostart_hidden).id_salt("set-autostart-hidden"));
        });
    });

    group(ui, "Управление");
    row(ui, Id::CoverClickPause, "Клик по обложке — пауза", Some("В открытом треке"), |ui| {
        ui.add(Switch::new(&mut p.cover_click_pause).id_salt("set-cover-click"));
    });
    row(ui, Id::DblClickShape, "Двойной клик — другая фигура", Some("Двойной клик в открытом треке меняет форму обложки"), |ui| {
        choice_menu(ui, Id::DblClickShape, p);
    });
    dimmed(ui, p.dbl_click_shape == 0, |ui| {
        let label = |v: f32| if v < 0.05 { "сразу".to_owned() } else { format!("{:.0} мс", v * 1000.0) };
        let wait = p.np_tap_wait.clamp(0.0, 0.4);
        let title = format!("Ожидание второго клика: {}", label(wait));
        row(ui, Id::TapWait, &title, Some("Меньше — пауза по клику срабатывает быстрее"), |ui| {
            if let Some(v) = range_slider(ui, "set-tap-wait", wait, (0.0, 0.4), 0.02, label) {
                p.np_tap_wait = v;
            }
        });
    });
    row(ui, Id::Keys, "Клавиши", Some("L — текст рядом с обложкой, Q — очередь, Esc — назад к обложке"), |_| {});

    group(ui, "Текст песен");
    row(ui, Id::SaveLyrics, "Сохранять найденный текст в файл трека", Some("Чтобы текст показывался и без интернета"), |ui| {
        ui.add(Switch::new(&mut p.save_found_lyrics).id_salt("set-save-lyrics"));
    });
    row(ui, Id::LyricsReturn, &format!("Возвращаться к текущей строке через {:.0} с", p.lyrics_return_s), Some("После ручной прокрутки текста"), |ui| {
        if let Some(v) = range_slider(ui, "set-lyrics-return", p.lyrics_return_s, (1.0, 15.0), 1.0, |v| format!("{v:.0} с")) {
            p.lyrics_return_s = v;
        }
    });

    group(ui, "Звук");
    row(ui, Id::OutputDevice, "Устройство вывода", Some("Переключается сразу, без паузы"), |ui| {
        const DEFAULT: &str = "Системное по умолчанию";
        let mut options = vec![(DEFAULT.to_owned(), p.output_device.is_none())];
        options.extend(p.output_devices.iter().map(|d| (d.clone(), p.output_device.as_deref() == Some(d.as_str()))));
        let cur = p.output_device.as_deref().unwrap_or(DEFAULT).to_owned();
        if let Some(n) = dropdown(ui, "set-device", &cur, &options) {
            p.output_device = if n == 0 { None } else { p.output_devices.get(n - 1).cloned() };
        }
    });
    row(ui, Id::Gapless, "Без пауз между треками", Some("Треки альбома идут без тишины между ними"), |ui| {
        ui.add(Switch::new(&mut p.gapless).id_salt("set-gapless"));
    });
    row(
        ui,
        Id::Crossfade,
        "Кроссфейд",
        Some(&if p.crossfade_s < 0.5 { "выключен".to_owned() } else { format!("{:.0} с", p.crossfade_s) }),
        |ui| {
            if let Some(v) = range_slider(ui, "set-xfade", p.crossfade_s, (0.0, 12.0), 1.0, |v| format!("{v:.0} с")) {
                p.crossfade_s = v;
            }
        },
    );
    row(ui, Id::Normalization, "Нормализация громкости", Some("Все треки примерно одинаково громкие"), |ui| {
        ui.add(Switch::new(&mut p.normalization).id_salt("set-norm"));
    });

    group(ui, "Рекомендации");
    row(ui, Id::RadioNewShare, "Новое в радио и миксах", Some("Сколько незнакомой музыки подмешивать"), |ui| {
        choice(ui, Id::RadioNewShare, p);
    });
    row(ui, Id::SoundAnalysis, "Анализ звука", Some(&analysis_status(&state.analysis)), |ui| {
        // Stream-only tracks are analysed (or not) on «Источники».
        if ui.add(Button::new("Анализ стримов").style(Style::Text).id_salt("set-stream-analysis-link")).clicked() {
            pages.open_setting(Id::StreamAnalysis);
        }
    });

    group(ui, "Библиотека");
    row(ui, Id::LibraryDir, "Папка музыки", Some(&p.library_dir.display().to_string()), |ui| {
        if ui.add(Button::new("Открыть").style(Style::Outlined).id_salt("open-lib")).clicked() {
            let _ = open_folder(&p.library_dir);
        }
    });
    let default_masters = p.library_dir.join(".musiclib").join("masters");
    let masters_dir = if p.masters_dir.as_os_str().is_empty() {
        default_masters
    } else {
        p.masters_dir.clone()
    };
    row(ui, Id::MastersDir, "Архив FLAC (оригиналы)", Some(&masters_dir.display().to_string()), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(Button::new("Перенести…").style(Style::Tonal).id_salt("move-masters"))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new()
                    .set_title("Выбери диск или папку для FLAC-оригиналов")
                    .pick_folder()
            {
                out.push(Command::MoveMastersDir(dir));
            }
            if ui
                .add(Button::new("Открыть").style(Style::Outlined).id_salt("open-masters"))
                .clicked()
            {
                let _ = open_folder(&masters_dir);
            }
        });
    });
    row(ui, Id::Inbox, "Inbox", Some(&p.inbox_dir.display().to_string()), |ui| {
        if ui.add(Button::new("Открыть").style(Style::Outlined).id_salt("open-inbox")).clicked() {
            let _ = open_folder(&p.inbox_dir);
        }
    });
    row(ui, Id::NamingRules, "Правила имён", Some("Исполнитель/Год - Альбом/01 - Трек.opus"), |_| {});
    let trash = p.library_dir.parent().map_or_else(|| p.library_dir.join("Trash"), |d| d.join("Trash"));
    row(
        ui,
        Id::Trash,
        "Корзина",
        Some(if p.use_trash {
            "Удалённое лежит 30 дней: Библиотека → Корзина"
        } else {
            "Выключена: удаление сразу стирает файлы"
        }),
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.add(Switch::new(&mut p.use_trash).id_salt("set-use-trash"));
                if ui.add(Button::new("Открыть папку").style(Style::Outlined).id_salt("open-trash")).clicked() {
                    let _ = open_folder(&trash);
                }
            });
        },
    );
    let userdata = p.library_dir.join(".musiclib").join("userdata");
    row(ui, Id::ExportData, "Экспорт данных", Some("Лайки и плейлисты в .musiclib/userdata рядом с музыкой"), |ui| {
        if ui.add(Button::new("Открыть папку").style(Style::Outlined).id_salt("open-userdata")).clicked() {
            let _ = open_folder(&userdata);
        }
    });

    group(ui, "Импорт и качество");
    row(ui, Id::OpusBitrate, "Битрейт Opus", Some(&format!("{} кбит/с", p.opus_bitrate)), |ui| {
        choice(ui, Id::OpusBitrate, p);
    });
    row(ui, Id::FakeFlac, "Проверять «фейковый» FLAC", Some("Находит FLAC, сделанные из MP3"), |ui| {
        ui.add(Switch::new(&mut p.check_fake_lossless).id_salt("set-fake"));
    });
    if !pages.metadata_synced {
        pages.metadata_contact = p.metadata_contact.clone();
        pages.metadata_synced = true;
    }
    ui.add_space(8.0);
    settings_help::item(ui, Id::MusicBrainz, "Контакт для MusicBrainz", |ui| {
        TextField::new(&mut pages.metadata_contact, "Контакт для MusicBrainz")
            .placeholder("почта или адрес сайта")
            .supporting("Попадает в User-Agent запросов: так просит MusicBrainz.")
            .leading_icon(Icon::Person)
            .width(480.0)
            .id_salt("set-mb-contact")
            .show(ui);
    });
    ui.add_space(8.0);
    settings_help::item(ui, Id::AcoustId, "Ключ AcoustID", |ui| {
        TextField::new(&mut pages.acoustid_key, "Ключ AcoustID")
            .password(true)
            .supporting(if p.acoustid_configured {
                "Ключ задан. Введи новый, чтобы заменить."
            } else {
                "Нужен для опознания треков по звуку. Хранится в settings.toml."
            })
            .width(480.0)
            .id_salt("set-acoustid")
            .show(ui);
    });
    ui.add_space(8.0);
    let contact_changed = pages.metadata_contact.trim() != p.metadata_contact.trim();
    if ui
        .add(
            Button::new("Сохранить")
                .style(Style::Tonal)
                .enabled(contact_changed || !pages.acoustid_key.trim().is_empty())
                .id_salt("set-metadata-save"),
        )
        .clicked()
    {
        let key = std::mem::take(&mut pages.acoustid_key).trim().to_owned();
        out.push(Command::SetMetadata {
            contact: pages.metadata_contact.trim().to_owned(),
            acoustid_key: (!key.is_empty()).then_some(key),
        });
    }

    group(ui, "Узлы и бэкап");
    row(ui, Id::Nodes, "Raspberry Pi, сервер, телефон", Some("Появится позже"), |_| {});
}

/// «1 240»: thousands apart with a no-break space, as Russian writes them.
pub(super) fn fmt_count(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{a0}');
        }
        out.push(c);
    }
    out
}

/// «1 240 из 3 000 треков · идёт анализ» / «… · готово» (the row is
/// «Анализ звука»), files and stream-only tracks together.
pub(super) fn analysis_status(a: &nikgob_view::AnalysisProgress) -> String {
    let (done, total) = (a.files_done + a.streams_done, a.files_total + a.streams_total);
    if total == 0 {
        return "Анализировать пока нечего".to_owned();
    }
    let state = if a.complete() { "готово" } else { "идёт анализ" };
    format!("{} из {} {} · {state}", fmt_count(done.min(total)), fmt_count(total), plural(u64::from(total), "трека", "треков", "треков"))
}

pub(super) fn open_folder(path: &std::path::Path) -> std::io::Result<()> {
    // A test must never launch a program on the user's desktop.
    if cfg!(test) {
        panic!("a test tried to launch an external program");
    }
    let _ = std::fs::create_dir_all(path);
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{analysis_status, fmt_count};
    use nikgob_view::AnalysisProgress;

    #[test]
    fn counts_and_analysis_read_as_russian() {
        assert_eq!(fmt_count(7), "7");
        assert_eq!(fmt_count(1240), "1\u{a0}240");
        assert_eq!(fmt_count(1_234_567), "1\u{a0}234\u{a0}567");
        let mut a = AnalysisProgress { files_done: 1000, files_total: 2600, streams_done: 240, streams_total: 400 };
        assert_eq!(analysis_status(&a), "1\u{a0}240 из 3\u{a0}000 треков · идёт анализ");
        a.files_done = 2600;
        a.streams_done = 400;
        assert_eq!(analysis_status(&a), "3\u{a0}000 из 3\u{a0}000 треков · готово");
        assert_eq!(analysis_status(&AnalysisProgress::default()), "Анализировать пока нечего");
    }
}
