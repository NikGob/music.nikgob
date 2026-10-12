//! Настройки (md3e-player.md §3.8): grouped list rows; every change is sent
//! as a whole new `Prefs`.

use egui::Ui;
use nikgob_view::{Ambient, Command, CoverShape, Prefs, RingStyle, Route, ThemeMode, ViewState};

use super::{PageState, connected};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Divider, FilterChip, Menu, MenuItem, Slider, Style, Switch, TextField},
};

fn group(ui: &mut Ui, title: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(24.0);
    ui.label(TypeRole::TitleSmall.style().rich(title, s.primary));
    ui.add_space(4.0);
}

/// A 56/72 row: label (+ supporting) on the left, a control on the right.
/// In a narrow page the control goes under the text.
fn row(ui: &mut Ui, label: &str, supporting: Option<&str>, control: impl FnOnce(&mut Ui)) {
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

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut p: Prefs = state.prefs.clone();
    let before = p.clone();
    ui.set_max_width(ui.available_width().min(840.0));
    ui.add_space(8.0);
    ui.label(TypeRole::HeadlineMedium.style().rich("Настройки", s.on_surface));

    group(ui, "Внешний вид");
    row(ui, "Дизайн", Some("Nothing и Liquid Glass — скоро"), |ui| {
        // Only MD3E exists so far; the others are shown but off.
        connected(ui, egui::Id::new("set-design"), &["Material 3", "Nothing", "Glass"], 0, &[1, 2]);
    });
    row(ui, "Тема", None, |ui| {
        let i = match p.theme {
            ThemeMode::Light => 0,
            ThemeMode::Dark => 1,
            ThemeMode::System => 2,
        };
        if let (Some(n), _) = connected(ui, egui::Id::new("set-theme"), &["Светлая", "Тёмная", "Системная"], i, &[]) {
            p.theme = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System][n];
        }
    });
    row(ui, "Контраст", None, |ui| {
        let labels = ["Обычный", "Средний", "Высокий"];
        let i = usize::from(p.contrast).min(2);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-contrast"), &labels, i, &[]) {
            p.contrast = n as u8;
        }
    });
    row(ui, "Схема цвета", Some("Палитра интерфейса из обложки трека"), |ui| {
        const VARIANTS: [&str; 7] =
            ["Content", "Tonal Spot", "Vibrant", "Expressive", "Fidelity", "Monochrome", "Neutral"];
        let cur = usize::from(p.scheme_variant).min(VARIANTS.len() - 1);
        let options: Vec<(String, bool)> =
            VARIANTS.iter().enumerate().map(|(i, v)| ((*v).to_owned(), i == cur)).collect();
        if let Some(n) = dropdown(ui, "set-variant", VARIANTS[cur], &options) {
            p.scheme_variant = n as u8;
        }
    });
    row(ui, "Ambient", Some("Мягкие облака в цветах темы, под звук"), |ui| {
        let i = match p.ambient {
            Ambient::Off => 0,
            Ambient::Glow => 1,
            Ambient::Reactive => 2,
        };
        if let (Some(n), _) = connected(ui, egui::Id::new("set-ambient"), &["Выкл", "Свечение", "Под звук"], i, &[]) {
            p.ambient = [Ambient::Off, Ambient::Glow, Ambient::Reactive][n];
        }
    });
    row(ui, "Лимит кадров", Some(&format!("{} FPS", p.fps_limit)), |ui| {
        if let Some(v) = range_slider(ui, "set-fps", p.fps_limit as f32, (30.0, 144.0), 1.0, |v| format!("{v:.0}")) {
            p.fps_limit = v as u32;
        }
    });
    row(ui, "Меньше движения", Some("Без отскоков, вращения и волн"), |ui| {
        ui.add(Switch::new(&mut p.reduced_motion).id_salt("set-rm"));
    });

    group(ui, "Сейчас играет");
    row(ui, "Открытие экрана", Some("Как он вырастает из плеера и сворачивается"), |ui| {
        let i = usize::from(p.np_motion).min(2);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-np-motion"), &["Плавно", "Живо", "Быстро"], i, &[]) {
            p.np_motion = n as u8;
        }
    });
    row(ui, "Ползунок громкости", Some("Без него — кнопка звука, громкость колесом"), |ui| {
        ui.add(Switch::new(&mut p.np_volume).id_salt("set-np-volume"));
    });
    row(ui, "Форма обложки", Some("Пока играет музыка; на паузе — квадрат"), |ui| {
        let options: Vec<(String, bool)> =
            CoverShape::ALL.iter().map(|c| (c.label().to_owned(), *c == p.cover_shape)).collect();
        if let Some(n) = dropdown(ui, "set-cover-shape", p.cover_shape.label(), &options) {
            p.cover_shape = CoverShape::ALL[n];
        }
    });
    row(ui, "Вращать форму", Some("Контур медленно крутится, картинка стоит"), |ui| {
        ui.add(Switch::new(&mut p.cover_spin).id_salt("set-cover-spin"));
    });
    if p.cover_spin {
        row(ui, "Направление формы", None, |ui| {
            if let (Some(n), _) = connected(ui, egui::Id::new("set-cover-dir"), &["По часовой", "Против"], usize::from(p.cover_ccw), &[]) {
                p.cover_ccw = n == 1;
            }
        });
    }
    row(ui, "Пульсация в такт", Some("Обложка чуть вздрагивает на долях"), |ui| {
        ui.add(Switch::new(&mut p.cover_pulse).id_salt("set-cover-pulse"));
    });
    row(ui, "Кольцо вокруг обложки", None, |ui| {
        ui.add(Switch::new(&mut p.ring).id_salt("set-ring-on"));
    });
    if p.ring {
        row(ui, "Вид кольца", Some("Круг из точек, точки по контуру формы или бары"), |ui| {
            let styles = [RingStyle::Circle, RingStyle::Contour, RingStyle::Bars];
            let i = styles.iter().position(|r| *r == p.ring_style).unwrap_or(1);
            if let (Some(n), _) = connected(ui, egui::Id::new("set-ring-style"), &["Круг", "Контур", "Бары"], i, &[]) {
                p.ring_style = styles[n];
            }
        });
        row(ui, "Кольцо крутится", Some("Пока играет музыка"), |ui| {
            ui.add(Switch::new(&mut p.spin_ring).id_salt("set-ring"));
        });
        if p.spin_ring {
            row(ui, "Направление кольца", None, |ui| {
                if let (Some(n), _) = connected(ui, egui::Id::new("set-ring-dir"), &["По часовой", "Против"], usize::from(p.ring_ccw), &[]) {
                    p.ring_ccw = n == 1;
                }
            });
        }
        row(ui, "Кольцо в такт музыке", Some("Прыгает на ударных и всплесках спектра"), |ui| {
            ui.add(Switch::new(&mut p.ring_beat).id_salt("set-ring-beat"));
        });
    }

    group(ui, "Звук");
    row(ui, "Устройство вывода", Some("Переключается сразу, без паузы"), |ui| {
        const DEFAULT: &str = "Системное по умолчанию";
        let mut options = vec![(DEFAULT.to_owned(), p.output_device.is_none())];
        options.extend(p.output_devices.iter().map(|d| (d.clone(), p.output_device.as_deref() == Some(d.as_str()))));
        let cur = p.output_device.as_deref().unwrap_or(DEFAULT).to_owned();
        if let Some(n) = dropdown(ui, "set-device", &cur, &options) {
            p.output_device = if n == 0 { None } else { p.output_devices.get(n - 1).cloned() };
        }
    });
    row(ui, "Без пауз между треками", Some("Gapless"), |ui| {
        ui.add(Switch::new(&mut p.gapless).id_salt("set-gapless"));
    });
    row(
        ui,
        "Кроссфейд",
        Some(&if p.crossfade_s < 0.5 { "выключен".to_owned() } else { format!("{:.0} с", p.crossfade_s) }),
        |ui| {
            if let Some(v) = range_slider(ui, "set-xfade", p.crossfade_s, (0.0, 12.0), 1.0, |v| format!("{v:.0} с")) {
                p.crossfade_s = v;
            }
        },
    );
    row(ui, "Нормализация громкости", Some("ReplayGain / R128"), |ui| {
        ui.add(Switch::new(&mut p.normalization).id_salt("set-norm"));
    });

    group(ui, "Библиотека");
    row(ui, "Папка музыки", Some(&p.library_dir.display().to_string()), |ui| {
        if ui.add(Button::new("Открыть").style(Style::Outlined).id_salt("open-lib")).clicked() {
            let _ = open_folder(&p.library_dir);
        }
    });
    row(ui, "Inbox", Some(&p.inbox_dir.display().to_string()), |ui| {
        if ui.add(Button::new("Открыть").style(Style::Outlined).id_salt("open-inbox")).clicked() {
            let _ = open_folder(&p.inbox_dir);
        }
    });
    row(ui, "Правила имён", Some("Исполнитель/Год - Альбом/01 - Трек.opus"), |_| {});
    let trash = p.library_dir.parent().map_or_else(|| p.library_dir.join("Trash"), |d| d.join("Trash"));
    row(ui, "Корзина", Some("Удалённые треки лежат 30 дней"), |ui| {
        if ui.add(Button::new("Открыть").style(Style::Outlined).id_salt("open-trash")).clicked() {
            let _ = open_folder(&trash);
        }
    });
    let userdata = p.library_dir.join(".musiclib").join("userdata");
    row(ui, "Экспорт данных", Some("Лайки и плейлисты в .musiclib/userdata рядом с музыкой"), |ui| {
        if ui.add(Button::new("Открыть папку").style(Style::Outlined).id_salt("open-userdata")).clicked() {
            let _ = open_folder(&userdata);
        }
    });

    group(ui, "Импорт и качество");
    row(ui, "Битрейт Opus", Some(&format!("{} кбит/с", p.opus_bitrate)), |ui| {
        let vals = [128, 160, 192, 256];
        let labels = ["128", "160", "192", "256"];
        let i = vals.iter().position(|v| *v == p.opus_bitrate).unwrap_or(1);
        if let (Some(n), _) = connected(ui, egui::Id::new("set-opus"), &labels, i, &[]) {
            p.opus_bitrate = vals[n];
        }
    });
    row(ui, "Проверять «фейковый» FLAC", Some("Срез спектра выдаёт перекодированный lossy"), |ui| {
        ui.add(Switch::new(&mut p.check_fake_lossless).id_salt("set-fake"));
    });
    if !pages.metadata_synced {
        pages.metadata_contact = p.metadata_contact.clone();
        pages.metadata_synced = true;
    }
    ui.add_space(8.0);
    TextField::new(&mut pages.metadata_contact, "Контакт для MusicBrainz")
        .placeholder("почта или адрес сайта")
        .supporting("Попадает в User-Agent запросов: так просит MusicBrainz.")
        .leading_icon(Icon::Person)
        .width(480.0)
        .id_salt("set-mb-contact")
        .show(ui);
    ui.add_space(8.0);
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
    row(ui, "Не раздавать", Some("Скачанное сразу перестаёт раздаваться"), |ui| {
        ui.add(Switch::new(&mut p.no_seed).id_salt("set-noseed"));
    });
    row(ui, "Лимит раздачи", Some(&format!("{} ГБ", p.seed_limit_gb)), |ui| {
        if let Some(v) = range_slider(ui, "set-seed", p.seed_limit_gb as f32, (0.0, 500.0), 5.0, |v| format!("{v:.0} ГБ")) {
            p.seed_limit_gb = v as u32;
        }
    });
    row(ui, "Срок раздачи", Some(&format!("чистка через {} дн.", p.seed_days)), |ui| {
        if let Some(v) = range_slider(ui, "set-seed-days", p.seed_days as f32, (1.0, 90.0), 1.0, |v| format!("{v:.0} дн.")) {
            p.seed_days = v as u32;
        }
    });
    row(
        ui,
        "Ratio",
        Some(&if p.seed_ratio < 0.05 { "без лимита".to_owned() } else { format!("до {:.1}", p.seed_ratio) }),
        |ui| {
            let label = |v: f32| if v < 0.05 { "без лимита".to_owned() } else { format!("{v:.1}") };
            if let Some(v) = range_slider(ui, "set-ratio", p.seed_ratio, (0.0, 5.0), 0.5, label) {
                p.seed_ratio = v;
            }
        },
    );
    row(ui, "Скорость загрузки", Some(&fmt_speed(p.download_limit_kbps)), |ui| {
        if let Some(v) = speed_slider(ui, "set-dl-limit", p.download_limit_kbps) {
            p.download_limit_kbps = v;
        }
    });
    row(ui, "Скорость отдачи", Some(&fmt_speed(p.upload_limit_kbps)), |ui| {
        if let Some(v) = speed_slider(ui, "set-ul-limit", p.upload_limit_kbps) {
            p.upload_limit_kbps = v;
        }
    });

    group(ui, "Аккаунты");
    row(ui, "Spotify", Some("Этап 3"), |ui| {
        ui.add(Button::new("Войти").style(Style::Outlined).enabled(false).id_salt("acc-sp"));
    });
    row(ui, "YouTube Music", Some("Этап 3"), |ui| {
        ui.add(Button::new("Войти").style(Style::Outlined).enabled(false).id_salt("acc-yt"));
    });

    group(ui, "Узлы и бэкап");
    row(ui, "Raspberry Pi, сервер, телефон", Some("Этап 6"), |_| {});
    ui.add(Divider::new());

    if p != before {
        out.push(Command::SetPrefs(Box::new(p)));
    }
    None
}

fn open_folder(path: &std::path::Path) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(path);
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}
