//! Загрузки (md3e-player.md §3.7): active jobs with wavy progress, the
//! "Подтверди" queue, seeding and history.

use egui::{Rect, Stroke, Ui, vec2};
use nikgob_view::{Command, Job, Route, SeedRow, ViewState};

use super::{PageState, connected, fmt_size, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, Card, IconButton, LoadingIndicator, ListItem, Radii, Size, Style, badge, paint,
        wavy_progress,
    },
    tokens::shape,
};

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let d = &state.downloads;
    let mut nav = None;
    ui.add_space(8.0);
    ui.label(TypeRole::HeadlineMedium.style().rich("Загрузки", s.on_surface));
    ui.add_space(8.0);
    let labels = ["Активные", "Подтверди", "Раздача", "История"];
    let (picked, rects) = connected(ui, egui::Id::new("dl-tabs"), &labels, pages.downloads_tab, &[]);
    if let Some(i) = picked {
        pages.downloads_tab = i;
    }
    if !d.confirm.is_empty()
        && let Some(r) = rects.get(1)
    {
        badge::count(ui.painter(), badge::large_anchor(*r), d.confirm.len() as u32);
    }
    ui.add_space(16.0);
    match pages.downloads_tab {
        0 => {
            if d.active.is_empty() {
                widgets::empty_state(
                    ui,
                    Icon::Download,
                    "Ничего не качается",
                    "Импорт файлов и торренты будут видны здесь.",
                    None,
                );
            }
            for j in &d.active {
                job(ui, j, out, true);
            }
        }
        1 => {
            if d.confirm.is_empty() {
                widgets::empty_state(ui, Icon::Check, "Всё подтверждено", "Сомнительные совпадения появятся здесь.", None);
            }
            for c in &d.confirm {
                let key = egui::Id::new(("confirm-choice", c.id));
                let mut choice: usize = ui.ctx().data(|m| m.get_temp(key)).unwrap_or_else(|| {
                    c.options.iter().position(|o| o.recommended).unwrap_or(0)
                });
                Card::outlined().id_salt(("confirm", c.id)).show(ui, |ui| {
                    ui.label(TypeRole::TitleMedium.style().rich(&c.title, s.on_surface));
                    ui.label(TypeRole::BodySmall.style().rich(&c.origin, s.on_surface_variant));
                    ui.add_space(8.0);
                    for (i, o) in c.options.iter().enumerate() {
                        let text = if o.recommended {
                            format!("{} · {}   ← рекоменд.", o.label, o.detail)
                        } else {
                            format!("{} · {}", o.label, o.detail)
                        };
                        if ui.radio(choice == i, TypeRole::BodyMedium.style().rich(text, s.on_surface)).clicked() {
                            choice = i;
                        }
                    }
                    ui.add_space(8.0);
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(Button::new(c.accept_label.as_str()).style(Style::Filled).id_salt(("cf-ok", c.id))).clicked() {
                            out.push(Command::ConfirmChoice { id: c.id, option: Some(choice) });
                        }
                        if ui.add(Button::new(c.skip_label.as_str()).style(Style::Text).id_salt(("cf-skip", c.id))).clicked() {
                            out.push(Command::ConfirmChoice { id: c.id, option: None });
                        }
                    });
                });
                ui.ctx().data_mut(|m| m.insert_temp(key, choice));
                ui.add_space(12.0);
            }
        }
        2 => nav = seeding(ui, state, pages, out),
        _ => {
            if d.history.is_empty() {
                widgets::empty_state(ui, Icon::Check, "История пуста", "Завершённые импорты и загрузки.", None);
            }
            for j in &d.history {
                job(ui, j, out, false);
            }
        }
    }
    nav
}

/// "Раздача": uploaded vs the limit, the cleanup policy, one row per torrent.
fn seeding(ui: &mut Ui, state: &ViewState, pages: &mut PageState, _out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let d = &state.downloads;
    let p = &state.prefs;
    let mut nav = None;
    let limit = u64::from(p.seed_limit_gb) * (1 << 30);

    Card::filled().id_salt("seed-header").show(ui, |ui| {
        ui.set_width(ui.available_width());
        if p.no_seed {
            ui.label(TypeRole::TitleMedium.style().rich("Раздача выключена", s.on_surface));
            ui.label(TypeRole::BodySmall.style().rich(
                "Скачанное сразу перестаёт раздаваться (Настройки → Торренты).",
                s.on_surface_variant,
            ));
        } else {
            ui.horizontal(|ui| {
                ui.label(TypeRole::TitleMedium.style().rich("Отдано", s.on_surface));
                ui.label(TypeRole::TitleMedium.style().rich(fmt_size(d.seeded_bytes), s.primary));
                ui.label(TypeRole::BodyMedium.style().rich(
                    if limit > 0 { format!("из лимита {} ГБ", p.seed_limit_gb) } else { "без лимита".to_owned() },
                    s.on_surface_variant,
                ));
            });
            if limit > 0 {
                ui.add_space(4.0);
                let frac = (d.seeded_bytes as f32 / limit as f32).clamp(0.0, 1.0);
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
                paint::container(ui.painter(), rect, Radii::all(shape::FULL), s.outline_variant, Stroke::NONE);
                if frac > 0.0 {
                    let fill = Rect::from_min_size(rect.min, vec2((rect.width() * frac).max(8.0), 8.0));
                    paint::container(ui.painter(), fill, Radii::all(shape::FULL), s.primary, Stroke::NONE);
                }
            }
            ui.add_space(4.0);
            ui.label(TypeRole::BodySmall.style().rich(
                format!("Раздаю до {} дн., потом чистка. Раздач: {}.", p.seed_days, d.seeding.len()),
                s.on_surface_variant,
            ));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    Button::new("Очистить сейчас")
                        .style(Style::Tonal)
                        .icon(Icon::Delete)
                        .enabled(!d.seeding.is_empty())
                        .id_salt("seed-clean"),
                )
                .clicked()
            {
                pages.confirm_clean = true;
            }
            if ui
                .add(Button::new("Политика раздачи").style(Style::Text).icon(Icon::Settings).id_salt("seed-policy"))
                .clicked()
            {
                nav = Some(Route::Settings);
            }
        });
    });
    ui.add_space(12.0);

    if d.seeding.is_empty() {
        widgets::empty_state(ui, Icon::CloudDownload, "Ничего не раздаётся", "Скачанные торренты появятся здесь.", None);
    }
    for r in &d.seeding {
        seed_row(ui, r);
    }
    nav
}

fn seed_row(ui: &mut Ui, r: &SeedRow) {
    let ratio = if r.downloaded_bytes > 0 {
        format!("ratio {:.2}", r.uploaded_bytes as f64 / r.downloaded_bytes as f64)
    } else {
        "ratio —".to_owned()
    };
    let mut sub = format!("↑ {} · ↓ {} · {ratio}", fmt_size(r.uploaded_bytes), fmt_size(r.downloaded_bytes));
    if let Some(e) = &r.expires {
        sub.push_str(&format!(" · {e}"));
    }
    ListItem::new(&r.title)
        .supporting(sub)
        .leading_icon(Icon::CloudDownload)
        .id_salt(("seed", r.id))
        .show(ui);
}

fn job(ui: &mut Ui, j: &Job, out: &mut Vec<Command>, active: bool) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 120.0).max(200.0));
            ui.label(TypeRole::BodyLarge.style().rich(&j.title, s.on_surface));
            let mut sub = j.subtitle.clone();
            if let Some(r) = &j.rate {
                sub.push_str(&format!(" · {r}"));
            }
            if let Some(e) = &j.eta {
                sub.push_str(&format!(" · {e}"));
            }
            if let Some(err) = &j.error {
                ui.label(TypeRole::BodySmall.style().rich(err, s.error));
            } else {
                ui.label(TypeRole::BodySmall.style().rich(sub, s.on_surface_variant));
            }
            if active {
                let w = ui.available_width();
                match j.progress {
                    Some(p) => {
                        wavy_progress(ui, ("job", j.id), p, w);
                    }
                    None => {
                        ui.add(LoadingIndicator::new().size(32.0));
                    }
                }
            }
        });
        if active {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(IconButton::new(Icon::Close).size(Size::S).tooltip("Отменить").id_salt(("job-x", j.id)))
                    .clicked()
                {
                    out.push(Command::CancelJob(j.id));
                }
                if ui
                    .add(
                        IconButton::new(Icon::Pause)
                            .selected_icon(Icon::PlayArrow)
                            .selected(j.paused)
                            .size(Size::S)
                            .tooltip("Пауза")
                            .id_salt(("job-p", j.id)),
                    )
                    .clicked()
                {
                    out.push(Command::PauseJob(j.id, !j.paused));
                }
            });
        }
    });
    ui.add_space(12.0);
}
