//! Настройки (md3e-player.md §3.8): grouped list rows; every change is sent
//! as a whole new `Prefs`.

use egui::{Sense, Ui};
use nikgob_shapes::paint as shape_paint;
use nikgob_view::{Ambient, BgShapes, Command, CoverShape, Prefs, RingStyle, Route, ThemeMode, ViewState};

use super::settings_help::{self, SettingId as Id};
use super::{PageState, connected};
use crate::screens::now_playing;
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Divider, FilterChip, Menu, MenuItem, Slider, Style, Switch, TextField},
};

fn group(ui: &mut Ui, title: &str) -> egui::Response {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(24.0);
    let header = ui.label(TypeRole::TitleSmall.style().rich(title, s.primary));
    ui.add_space(4.0);
    header
}

/// A 56/72 row: label (+ supporting) on the left, a control on the right.
/// In a narrow page the control goes under the text.
fn row(ui: &mut Ui, id: Id, label: &str, supporting: Option<&str>, control: impl FnOnce(&mut Ui)) {
    settings_help::item(ui, id, label, |ui| row_body(ui, label, supporting, control));
}

fn row_body(ui: &mut Ui, label: &str, supporting: Option<&str>, control: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    if ui.available_width() < 540.0 {
        ui.add_space(8.0);
        ui.label(TypeRole::BodyLarge.style().rich(label, s.on_surface));
        if let Some(t) = supporting {
            ui.label(TypeRole::BodyMedium.style().rich_para(t, s.on_surface_variant));
        }
        ui.add_space(4.0);
        control(ui);
        ui.add_space(8.0);
        return;
    }
    let h = if supporting.is_some() { 72.0 } else { 56.0 };
    let label_w = (ui.available_width() * 0.4).clamp(180.0, 320.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), h),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.vertical(|ui| {
                ui.set_width(label_w);
                // Centre the text block against the control.
                let block = if supporting.is_some() { 44.0 } else { 24.0 };
                ui.add_space(((h - block) * 0.5).max(0.0));
                ui.label(TypeRole::BodyLarge.style().rich(label, s.on_surface));
                if let Some(t) = supporting {
                    ui.label(TypeRole::BodyMedium.style().rich_para(t, s.on_surface_variant));
                }
            });
            ui.add_space(16.0);
            control(ui);
        },
    );
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
fn range_slider(
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

fn fmt_speed(kbps: u32) -> String {
    match kbps {
        0 => "без лимита".to_owned(),
        k if k < 1024 => format!("{k} КиБ/с"),
        k => format!("{:.1} МиБ/с", k as f32 / 1024.0),
    }
}

fn speed_slider(ui: &mut Ui, id: &str, kbps: u32) -> Option<u32> {
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
    let tab = pages.settings_tab.min(1);
    if let (Some(n), _) = connected(ui, egui::Id::new("set-tab"), &["Основные", "Внешний вид"], tab, &[]) {
        pages.settings_tab = n;
    }

    if pages.settings_tab == settings_help::TAB_APPEARANCE {
        appearance(ui, &mut p);
    } else {
        general(ui, &mut p, pages, out);
    }
    ui.add(Divider::new());
    settings_help::finish(ui, &p);

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
    row(ui, Id::Theme, "Тема", None, |ui| {
        let i = match p.theme {
            ThemeMode::Light => 0,
            ThemeMode::Dark => 1,
            ThemeMode::System => 2,
        };
        if let (Some(n), _) = connected(ui, egui::Id::new("set-theme"), &["Светлая", "Тёмная", "Системная"], i, &[]) {
            p.theme = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System][n];
        }
    });
    row(ui, Id::Contrast, "Контраст", None, |ui| {
        let labels = ["Обычный", "Средний", "Высокий"];
        let i = usize::from(p.contrast).min(2);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-contrast"), &labels, i, &[]) {
            p.contrast = n as u8;
        }
    });
    row(ui, Id::Scheme, "Схема цвета", Some("Палитра интерфейса из обложки трека"), |ui| {
        const VARIANTS: [&str; 7] =
            ["Content", "Tonal Spot", "Vibrant", "Expressive", "Fidelity", "Monochrome", "Neutral"];
        let cur = usize::from(p.scheme_variant).min(VARIANTS.len() - 1);
        let options: Vec<(String, bool)> =
            VARIANTS.iter().enumerate().map(|(i, v)| ((*v).to_owned(), i == cur)).collect();
        if let Some(n) = dropdown(ui, "set-variant", VARIANTS[cur], &options) {
            p.scheme_variant = n as u8;
        }
    });

    group(ui, "Сейчас играет");
    row(ui, Id::ProgressWave, "Волна на полосе прогресса", Some("Выключи — прогресс всегда ровная линия"), |ui| {
        ui.add(Switch::new(&mut p.progress_wave).id_salt("set-progress-wave"));
    });
    row(ui, Id::NpVolume, "Ползунок громкости", Some("Без него — кнопка звука, громкость колесом"), |ui| {
        ui.add(Switch::new(&mut p.np_volume).id_salt("set-np-volume"));
    });

    group(ui, "Обложка");
    row(ui, Id::CoverShape, "Форма обложки", Some("Пока играет музыка; на паузе — квадрат"), |ui| {
        let i = match p.cover_shape {
            CoverShape::Original => 0,
            CoverShape::Random => 2,
            _ => 1,
        };
        if let (Some(n), _) = connected(ui, egui::Id::new("set-cover-mode"), &["Квадрат", "Фигура", "Случайная"], i, &[]) {
            p.cover_shape = match n {
                0 => CoverShape::Original,
                2 => CoverShape::Random,
                _ => now_playing::random_pool(&p.random_shapes)[0],
            };
        }
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
    row(ui, Id::CoverSpin, "Вращать форму", Some("Контур медленно крутится, картинка стоит"), |ui| {
        ui.add(Switch::new(&mut p.cover_spin).id_salt("set-cover-spin"));
    });
    if p.cover_spin {
        row(ui, Id::CoverDir, "Направление формы", None, |ui| {
            if let (Some(n), _) = connected(ui, egui::Id::new("set-cover-dir"), &["По часовой", "Против"], usize::from(p.cover_ccw), &[]) {
                p.cover_ccw = n == 1;
            }
        });
    }
    row(ui, Id::CoverPulse, "Пульсация в такт", Some("Обложка чуть вздрагивает на долях"), |ui| {
        ui.add(Switch::new(&mut p.cover_pulse).id_salt("set-cover-pulse"));
    });

    group(ui, "Кольцо вокруг обложки");
    row(ui, Id::Ring, "Кольцо", None, |ui| {
        ui.add(Switch::new(&mut p.ring).id_salt("set-ring-on"));
    });
    if p.ring {
        row(ui, Id::RingStyle, "Вид кольца", Some("Круг из точек, точки по контуру формы или точки, вырастающие в бары под музыку (как в caelestia)"), |ui| {
            let styles = [RingStyle::Circle, RingStyle::Contour, RingStyle::Bars];
            let i = styles.iter().position(|r| *r == p.ring_style).unwrap_or(1);
            if let (Some(n), _) = connected(ui, egui::Id::new("set-ring-style"), &["Круг", "Контур", "Бары"], i, &[]) {
                p.ring_style = styles[n];
            }
        });
        // The bars stand on fixed angles, like a spectrum.
        if p.ring_style != RingStyle::Bars {
            row(ui, Id::RingSpin, "Кольцо крутится", Some("Пока играет музыка"), |ui| {
                ui.add(Switch::new(&mut p.spin_ring).id_salt("set-ring"));
            });
            if p.spin_ring {
                row(ui, Id::RingDir, "Направление кольца", None, |ui| {
                    if let (Some(n), _) = connected(ui, egui::Id::new("set-ring-dir"), &["По часовой", "Против"], usize::from(p.ring_ccw), &[]) {
                        p.ring_ccw = n == 1;
                    }
                });
            }
        }
        row(ui, Id::RingBeat, "Кольцо в такт музыке", Some("Прыгает на ударных и всплесках спектра"), |ui| {
            ui.add(Switch::new(&mut p.ring_beat).id_salt("set-ring-beat"));
        });
    }

    group(ui, "Фон");
    row(ui, Id::Ambient, "Ambient", Some("Мягкие облака в цветах темы, под звук"), |ui| {
        let i = match p.ambient {
            Ambient::Off => 0,
            Ambient::Glow => 1,
            Ambient::Reactive => 2,
        };
        if let (Some(n), _) = connected(ui, egui::Id::new("set-ambient"), &["Выкл", "Свечение", "Под звук"], i, &[]) {
            p.ambient = [Ambient::Off, Ambient::Glow, Ambient::Reactive][n];
        }
    });
    row(ui, Id::BgShapes, "Фигуры на фоне", Some(match p.bg_shapes {
        BgShapes::Off => "Фигуры Material за открытым треком, под музыку",
        BgShapes::Bounce => "Лежат внизу и подпрыгивают на долях",
        BgShapes::Float => "Парят по экрану и светятся в такт",
        BgShapes::Morph => "Плывут и превращаются в другие фигуры на ударах",
    }), |ui| {
        let modes = [BgShapes::Off, BgShapes::Bounce, BgShapes::Float, BgShapes::Morph];
        let i = modes.iter().position(|m| *m == p.bg_shapes).unwrap_or(0);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-bg-shapes"), &["Выкл", "Прыгают", "Парят", "Меняют форму"], i, &[]) {
            p.bg_shapes = modes[n];
        }
    });
    if p.bg_shapes != BgShapes::Off {
        row(ui, Id::BgCount, "Количество", Some(&format!("{} фигур", p.bg_shapes_count)), |ui| {
            if let Some(v) = range_slider(ui, "set-bg-count", p.bg_shapes_count as f32, (4.0, 32.0), 1.0, |v| format!("{v:.0}")) {
                p.bg_shapes_count = v as u32;
            }
        });
        let pct = |v: f32| format!("{:.0}%", v * 100.0);
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
        row(ui, Id::BgSpeed, "Скорость", Some("Дрейф, вращение и прыжки"), |ui| {
            if let Some(v) = range_slider(ui, "set-bg-speed", p.bg_shapes_speed, (0.25, 3.0), 0.05, pct) {
                p.bg_shapes_speed = v;
            }
        });
        row(ui, Id::BgReact, "Реакция на музыку", Some("0% — просто украшение"), |ui| {
            if let Some(v) = range_slider(ui, "set-bg-react", p.bg_shapes_react, (0.0, 2.0), 0.05, pct) {
                p.bg_shapes_react = v;
            }
        });
        row(ui, Id::BgColors, "Цвета", Some("Контейнеры темы, один акцент или яркие акценты"), |ui| {
            let i = usize::from(p.bg_shapes_colors).min(2);
            if let (Some(n), _) = connected(ui, egui::Id::new("set-bg-colors"), &["Тема", "Акцент", "Яркие"], i, &[]) {
                p.bg_shapes_colors = n as u8;
            }
        });
    }

    group(ui, "Текст песен");
    row(ui, Id::LyricsStyle, "Анимация", Some("Караоке — заливка по слогам, Фокус — строка целиком, Волна — буква за буквой"), |ui| {
        let i = usize::from(p.lyrics_style).min(2);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-lyrics-style"), &["Караоке", "Фокус", "Волна"], i, &[]) {
            p.lyrics_style = n as u8;
        }
    });
    if p.lyrics_style == 2 {
        row(ui, Id::WaveFallback, "Если нет разметки по слогам", Some("Для анимации «Волна»"), |ui| {
            let labels = ["Волна — прыгающие буквы", "Спокойный Фокус"];
            let i = usize::from(p.wave_syllables_only);
            let options = labels.iter().enumerate().map(|(n, label)| ((*label).to_owned(), n == i)).collect::<Vec<_>>();
            if let Some(n) = dropdown(ui, "set-wave-syllables", labels[i], &options) {
                p.wave_syllables_only = n == 1;
            }
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
    row(ui, Id::LyricsGap, "Индикатор паузы", Some("Кольцо между строками"), |ui| {
        let labels = ["Линия", "Волна", "Толстая линия", "Толстая волна"];
        let i = usize::from(p.lyrics_gap_style).min(3);
        let options = labels.iter().enumerate().map(|(n, label)| ((*label).to_owned(), n == i)).collect::<Vec<_>>();
        if let Some(n) = dropdown(ui, "set-lyrics-gap", labels[i], &options) {
            p.lyrics_gap_style = n as u8;
        }
    });
    if matches!(p.lyrics_gap_style, 1 | 3) {
        row(ui, Id::LyricsGapWaves, "Длина волны", Some("Чем больше гребней, тем короче волна"), |ui| {
            if let Some(v) = range_slider(ui, "set-lyrics-gap-waves", p.lyrics_gap_waves, (3.0, 16.0), 1.0, |v| format!("{v:.0} гребней")) {
                p.lyrics_gap_waves = v;
            }
        });
    }
    row(ui, Id::LyricsCountdown, "Обратный отсчёт 3-2-1 в конце паузы", None, |ui| {
        ui.add(Switch::new(&mut p.lyrics_countdown).id_salt("set-lyrics-countdown"));
    });

    group(ui, "Движение");
    row(ui, Id::NpMotion, "Открытие экрана", Some("Как он вырастает из плеера и сворачивается"), |ui| {
        let i = usize::from(p.np_motion).min(2);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-np-motion"), &["Плавно", "Живо", "Быстро"], i, &[]) {
            p.np_motion = n as u8;
        }
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

/// Behaviour: controls, lyrics sources, sound, library, import, torrents.
fn general(ui: &mut Ui, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    group(ui, "Управление");
    row(ui, Id::CoverClickPause, "Клик по обложке — пауза", Some("В открытом треке"), |ui| {
        ui.add(Switch::new(&mut p.cover_click_pause).id_salt("set-cover-click"));
    });
    row(ui, Id::DblClickShape, "Двойной клик — другая фигура", Some("Двойной клик в открытом треке меняет форму обложки"), |ui| {
        let labels = ["Выкл", "Только в режиме «Случайная»", "Всегда"];
        let i = usize::from(p.dbl_click_shape).min(2);
        let options = labels.iter().enumerate().map(|(n, label)| ((*label).to_owned(), n == i)).collect::<Vec<_>>();
        if let Some(n) = dropdown(ui, "set-dbl-click", labels[i], &options) {
            p.dbl_click_shape = n as u8;
        }
    });
    row(ui, Id::Keys, "Клавиши", Some("L — текст рядом с обложкой, Q — очередь, Esc — назад к обложке"), |_| {});

    group(ui, "Текст песен");
    row(ui, Id::SaveLyrics, "Сохранять найденный текст в файл трека", Some("Встраивает синхронизированный текст в теги, чтобы он был доступен без интернета"), |ui| {
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
    row(ui, Id::Gapless, "Без пауз между треками", Some("Gapless"), |ui| {
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
    row(ui, Id::Normalization, "Нормализация громкости", Some("ReplayGain / R128"), |ui| {
        ui.add(Switch::new(&mut p.normalization).id_salt("set-norm"));
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
    row(ui, Id::Trash, "Корзина", Some("Удалённые треки лежат 30 дней"), |ui| {
        if ui.add(Button::new("Открыть").style(Style::Outlined).id_salt("open-trash")).clicked() {
            let _ = open_folder(&trash);
        }
    });
    let userdata = p.library_dir.join(".musiclib").join("userdata");
    row(ui, Id::ExportData, "Экспорт данных", Some("Лайки и плейлисты в .musiclib/userdata рядом с музыкой"), |ui| {
        if ui.add(Button::new("Открыть папку").style(Style::Outlined).id_salt("open-userdata")).clicked() {
            let _ = open_folder(&userdata);
        }
    });

    group(ui, "Импорт и качество");
    row(ui, Id::OpusBitrate, "Битрейт Opus", Some(&format!("{} кбит/с", p.opus_bitrate)), |ui| {
        let vals = [128, 160, 192, 256];
        let labels = ["128", "160", "192", "256"];
        let i = vals.iter().position(|v| *v == p.opus_bitrate).unwrap_or(1);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-opus"), &labels, i, &[]) {
            p.opus_bitrate = vals[n];
        }
    });
    row(ui, Id::FakeFlac, "Проверять «фейковый» FLAC", Some("Срез спектра выдаёт перекодированный lossy"), |ui| {
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

    group(ui, "Торренты");
    if pages.indexer_url.is_empty() && !p.prowlarr_url.is_empty() {
        pages.indexer_url = p.prowlarr_url.clone();
    }
    ui.add_space(4.0);
    settings_help::item(ui, Id::Prowlarr, "Prowlarr", |ui| {
        TextField::new(&mut pages.indexer_url, "Адрес Prowlarr")
            .placeholder("https://…")
            .leading_icon(Icon::Link)
            .width(480.0)
            .id_salt("set-prowlarr-url")
            .show(ui);
        ui.add_space(8.0);
        TextField::new(&mut pages.indexer_key, "API-ключ")
            .password(true)
            .supporting(if p.prowlarr_configured {
                "Ключ сохранён. Введи новый, чтобы заменить."
            } else {
                "Хранится в settings.toml, в коде его нет."
            })
            .width(480.0)
            .id_salt("set-prowlarr-key")
            .show(ui);
        ui.add_space(8.0);
        if ui
            .add(
                Button::new("Сохранить")
                    .style(Style::Tonal)
                    .enabled(!pages.indexer_url.trim().is_empty() && !pages.indexer_key.trim().is_empty())
                    .id_salt("set-prowlarr-save"),
            )
            .clicked()
        {
            out.push(Command::SetIndexer {
                url: pages.indexer_url.trim().to_owned(),
                api_key: std::mem::take(&mut pages.indexer_key).trim().to_owned(),
            });
        }
    });
    row(ui, Id::NoSeed, "Не раздавать", Some("Скачанное сразу перестаёт раздаваться"), |ui| {
        ui.add(Switch::new(&mut p.no_seed).id_salt("set-noseed"));
    });
    row(ui, Id::SeedLimit, "Лимит раздачи", Some(&format!("{} ГБ", p.seed_limit_gb)), |ui| {
        if let Some(v) = range_slider(ui, "set-seed", p.seed_limit_gb as f32, (0.0, 500.0), 5.0, |v| format!("{v:.0} ГБ")) {
            p.seed_limit_gb = v as u32;
        }
    });
    row(ui, Id::SeedDays, "Срок раздачи", Some(&format!("чистка через {} дн.", p.seed_days)), |ui| {
        if let Some(v) = range_slider(ui, "set-seed-days", p.seed_days as f32, (1.0, 90.0), 1.0, |v| format!("{v:.0} дн.")) {
            p.seed_days = v as u32;
        }
    });
    row(
        ui,
        Id::SeedRatio,
        "Ratio",
        Some(&if p.seed_ratio < 0.05 { "без лимита".to_owned() } else { format!("до {:.1}", p.seed_ratio) }),
        |ui| {
            let label = |v: f32| if v < 0.05 { "без лимита".to_owned() } else { format!("{v:.1}") };
            if let Some(v) = range_slider(ui, "set-ratio", p.seed_ratio, (0.0, 5.0), 0.5, label) {
                p.seed_ratio = v;
            }
        },
    );
    row(ui, Id::DownloadLimit, "Скорость загрузки", Some(&fmt_speed(p.download_limit_kbps)), |ui| {
        if let Some(v) = speed_slider(ui, "set-dl-limit", p.download_limit_kbps) {
            p.download_limit_kbps = v;
        }
    });
    row(ui, Id::UploadLimit, "Скорость отдачи", Some(&fmt_speed(p.upload_limit_kbps)), |ui| {
        if let Some(v) = speed_slider(ui, "set-ul-limit", p.upload_limit_kbps) {
            p.upload_limit_kbps = v;
        }
    });

    group(ui, "Аккаунты");
    row(ui, Id::Spotify, "Spotify", Some("Этап 3"), |ui| {
        ui.add(Button::new("Войти").style(Style::Outlined).enabled(false).id_salt("acc-sp"));
    });
    row(ui, Id::YouTube, "YouTube Music", Some("Этап 3"), |ui| {
        ui.add(Button::new("Войти").style(Style::Outlined).enabled(false).id_salt("acc-yt"));
    });

    group(ui, "Узлы и бэкап");
    row(ui, Id::Nodes, "Raspberry Pi, сервер, телефон", Some("Этап 6"), |_| {});
}

fn open_folder(path: &std::path::Path) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(path);
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}
