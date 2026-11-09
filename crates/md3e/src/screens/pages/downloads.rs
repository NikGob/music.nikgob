//! Загрузки (md3e-player.md §3.7): active jobs with wavy progress, the
//! "Подтверди" queue, seeding and history.

use egui::{Rect, Stroke, Ui, vec2};
use nikgob_core::model::SourceId;
use nikgob_view::{
    Command, Confirm, ConfirmPick, DownloadsTab, Job, Route, SeedRow, SharingView, SlskSharingView, UploadRow,
    UploadRowState, ViewState,
};

use super::{PageState, SettingId, connected, fmt_size, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, Card, ConnectedGroup, IconButton, LoadingIndicator, ListItem, Radii, Size, Style, Switch, badge,
        paint, segmented_list, wavy_progress,
    },
    screens::common::plural,
    tokens::shape,
};

/// The tabs, in order.
const TABS: [(DownloadsTab, &str); 5] = [
    (DownloadsTab::Active, "Активные"),
    (DownloadsTab::Confirm, "Подтверди"),
    (DownloadsTab::Wishlist, "Хочу скачать"),
    (DownloadsTab::Seeding, "Раздача"),
    (DownloadsTab::History, "История"),
];

/// Where `tab` is in the tab switch.
pub fn tab_index(tab: DownloadsTab) -> usize {
    TABS.iter().position(|(t, _)| *t == tab).unwrap_or(0)
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let d = &state.downloads;
    let mut nav = None;
    ui.add_space(8.0);
    ui.label(TypeRole::HeadlineMedium.style().rich("Загрузки", s.on_surface));
    ui.add_space(8.0);
    let labels = TABS.map(|(_, l)| l);
    let tab = pages.downloads_tab.min(TABS.len() - 1);
    let (picked, rects) = connected(ui, egui::Id::new("dl-tabs"), &labels, tab, &[]);
    if let Some(i) = picked {
        pages.downloads_tab = i;
        if TABS[i].0 == DownloadsTab::Wishlist {
            out.push(Command::LoadWishlist);
        }
    }
    if !d.confirm.is_empty()
        && let Some(r) = rects.get(tab_index(DownloadsTab::Confirm))
    {
        badge::count(ui.painter(), badge::large_anchor(*r), d.confirm.len() as u32);
    }
    ui.add_space(16.0);
    match TABS[tab].0 {
        DownloadsTab::Wishlist => nav = super::wishlist::show(ui, state, pages, out),
        DownloadsTab::Active => {
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
        DownloadsTab::Confirm => confirm(ui, state, out),
        DownloadsTab::Seeding => nav = seeding(ui, state, pages, out),
        DownloadsTab::History => {
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

/// "Подтверди": imports whose tags the matcher was not sure about. Each
/// card asks which found release the file is (or to keep the file's own
/// tags); the header says what this is about and can accept every
/// recommended match at once.
fn confirm(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let d = &state.downloads;
    if d.confirm.is_empty() {
        widgets::empty_state(ui, Icon::Check, "Всё подтверждено", "Сомнительные совпадения появятся здесь.", None);
        return;
    }
    Card::filled().id_salt("confirm-help").show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(TypeRole::TitleMedium.style().rich("Проверь, что это за треки", s.on_surface));
        ui.add_space(4.0);
        ui.label(
            TypeRole::BodyMedium.style().rich_para(
                "Для этих файлов нашлось несколько похожих релизов, и уверенности нет. Выбери правильный вариант — название, альбом, номер и обложка возьмутся из него. Первый вариант — теги, которые уже записаны в файле.",
                s.on_surface_variant,
            ),
        );
        ui.add_space(12.0);
        // A card ticked song by song is never answered from here.
        let recommended: Vec<(u64, usize)> = d
            .confirm
            .iter()
            .filter(|c| c.pick.is_none())
            .filter_map(|c| c.options.iter().position(|o| o.recommended).map(|i| (c.id, i)))
            .collect();
        if !recommended.is_empty()
            && ui
                .add(
                    Button::new(format!("Принять все рекомендованные ({})", recommended.len()))
                        .icon(Icon::Check)
                        .style(Style::Tonal)
                        .id_salt("cf-all"),
                )
                .clicked()
        {
            for (id, option) in recommended {
                out.push(Command::ConfirmChoice { id, option: Some(option) });
            }
        }
    });
    ui.add_space(12.0);

    for c in &d.confirm {
        if let Some(pick) = &c.pick {
            pick_card(ui, c, pick, out);
            ui.add_space(12.0);
            continue;
        }
        let key = egui::Id::new(("confirm-choice", c.id));
        let mut choice: usize = ui
            .ctx()
            .data(|m| m.get_temp(key))
            .unwrap_or_else(|| c.options.iter().position(|o| o.recommended).unwrap_or(0));
        Card::outlined().id_salt(("confirm", c.id)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(TypeRole::TitleMedium.style().rich(&c.title, s.on_surface));
            if !c.origin.is_empty() {
                ui.label(TypeRole::BodySmall.style().rich(&c.origin, s.on_surface_variant));
            }
            ui.add_space(12.0);
            ui.label(TypeRole::LabelLarge.style().rich(&c.question, s.on_surface_variant));
            ui.add_space(4.0);
            // A yes or no question: the songs it is about, nothing to choose.
            if c.yes_no {
                for o in &c.options {
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        let width = ui.available_width() - 8.0;
                        ui.vertical(|ui| {
                            ui.set_max_width(width);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add(egui::Label::new(TypeRole::TitleSmall.style().rich(&o.label, s.on_surface)).wrap());
                            ui.add(egui::Label::new(TypeRole::BodySmall.style().rich(&o.detail, s.on_surface_variant)).wrap());
                        });
                    });
                    ui.add_space(6.0);
                }
                choice = 0;
            }
            for (i, o) in c.options.iter().enumerate().filter(|_| !c.yes_no) {
                let bg = ui.painter().add(egui::Shape::Noop);
                let row = ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    let r = ui.radio(choice == i, "");
                    // A long release name wraps inside the card instead of
                    // widening it (and pushing the buttons out).
                    let width = ui.available_width() - 8.0;
                    ui.vertical(|ui| {
                        ui.set_max_width(width);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal_wrapped(|ui| {
                            ui.add(egui::Label::new(TypeRole::TitleSmall.style().rich(&o.label, s.on_surface)).wrap());
                            if o.recommended {
                                ui.add_space(6.0);
                                ui.label(TypeRole::LabelSmall.emphasized().rich("РЕКОМЕНДУЕМ", s.primary));
                            }
                        });
                        ui.add(egui::Label::new(TypeRole::BodySmall.style().rich(&o.detail, s.on_surface_variant)).wrap());
                    });
                    r
                });
                // The whole width of the card, not just the words.
                let r = row.response.rect;
                let area = egui::Rect::from_min_max(r.min, egui::pos2(ui.max_rect().right(), r.max.y)).expand2(vec2(4.0, 4.0));
                let hit = ui.interact(area, ui.id().with(("cf-row", c.id, i)), egui::Sense::click());
                if hit.clicked() || row.inner.clicked() {
                    choice = i;
                }
                let fill = if choice == i {
                    s.secondary_container.gamma_multiply(0.6)
                } else if hit.hovered() {
                    s.on_surface.gamma_multiply(0.06)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().set(bg, egui::Shape::rect_filled(area, 12, fill));
                ui.add_space(6.0);
            }
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(Button::new(c.accept_label.as_str()).style(Style::Filled).id_salt(("cf-ok", c.id)))
                    .clicked()
                {
                    out.push(Command::ConfirmChoice { id: c.id, option: Some(choice) });
                }
                // No second way out when the file's own tags are an option.
                if !c.skip_label.is_empty()
                    && ui
                        .add(Button::new(c.skip_label.as_str()).style(Style::Text).id_salt(("cf-skip", c.id)))
                        .clicked()
                {
                    out.push(Command::ConfirmChoice { id: c.id, option: None });
                }
            });
        });
        ui.ctx().data_mut(|m| m.insert_temp(key, choice));
        ui.add_space(12.0);
    }
}

/// The ticked songs that are still on the card, in the card's order: a
/// song that left it (turned into an MP3, kept) is ticked no more.
fn ticked_on_card(ticked: &[SourceId], sources: &[SourceId]) -> Vec<SourceId> {
    sources.iter().filter(|s| ticked.contains(s)).copied().collect()
}

/// A card whose songs are ticked one by one (the fake FLACs offered as
/// MP3s): a list of rows with a checkbox each, nothing ticked at first,
/// «Выбрать все» over them when there are several. Both buttons say how
/// many songs they act on, act on the ticked ones only and wait for a tick
/// (no song is changed, or kept for good, unless the user ticked it).
fn pick_card(ui: &mut Ui, c: &Confirm, pick: &ConfirmPick, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let key = egui::Id::new(("confirm-pick", c.id));
    let n = c.options.len().min(pick.sources.len());
    let sources = &pick.sources[..n];
    let stored: Vec<SourceId> = ui.ctx().data(|m| m.get_temp(key)).unwrap_or_default();
    let mut ticked = ticked_on_card(&stored, sources);
    // One row: a checkbox and its text, the whole row a target, filled
    // while ticked (with `highlight`). True when it was toggled.
    let row = |ui: &mut Ui, salt: (u64, usize), on: bool, mixed: bool, highlight: bool, text: &mut dyn FnMut(&mut Ui)| -> bool {
        let bg = ui.painter().add(egui::Shape::Noop);
        let mut flag = on;
        let line = ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());
            ui.add_space(4.0);
            let r = ui.add(crate::components::Checkbox::new(&mut flag).mixed(mixed).id_salt(("cf-pick-box", salt)));
            let width = ui.available_width() - 8.0;
            ui.vertical(|ui| {
                ui.set_max_width(width);
                ui.spacing_mut().item_spacing.y = 2.0;
                text(ui);
            });
            r
        });
        let r = line.response.rect;
        let area = egui::Rect::from_min_max(r.min, egui::pos2(ui.max_rect().right(), r.max.y)).expand2(vec2(4.0, 4.0));
        let hit = ui.interact(area, ui.id().with(("cf-pick-row", salt)), egui::Sense::click());
        let toggled = line.inner.changed() || hit.clicked();
        let fill = if highlight && on != toggled {
            s.secondary_container.gamma_multiply(0.6)
        } else if hit.hovered() {
            s.on_surface.gamma_multiply(0.06)
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().set(bg, egui::Shape::rect_filled(area, 12, fill));
        toggled
    };
    Card::outlined().id_salt(("confirm", c.id)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(TypeRole::TitleMedium.style().rich(&c.title, s.on_surface));
        if !c.origin.is_empty() {
            ui.label(TypeRole::BodySmall.style().rich(&c.origin, s.on_surface_variant));
        }
        ui.add_space(12.0);
        ui.add(egui::Label::new(TypeRole::LabelLarge.style().rich(&c.question, s.on_surface_variant)).wrap());
        ui.add_space(4.0);
        if n > 1 {
            let all = ticked.len() == n;
            let mixed = !ticked.is_empty() && !all;
            let mut words = |ui: &mut Ui| {
                ui.label(TypeRole::LabelLarge.style().rich("Выбрать все", s.on_surface));
            };
            if row(ui, (c.id, usize::MAX), all, mixed, false, &mut words) {
                ticked = if all { Vec::new() } else { sources.to_vec() };
            }
            ui.add_space(6.0);
        }
        for (i, (o, src)) in c.options.iter().zip(sources).enumerate() {
            let on = ticked.contains(src);
            let mut words = |ui: &mut Ui| {
                ui.add(egui::Label::new(TypeRole::TitleSmall.style().rich(&o.label, s.on_surface)).wrap());
                ui.add(egui::Label::new(TypeRole::BodySmall.style().rich(&o.detail, s.on_surface_variant)).wrap());
            };
            if row(ui, (c.id, i), on, false, true, &mut words) {
                if on {
                    ticked.retain(|t| t != src);
                } else {
                    ticked.push(*src);
                }
            }
            ui.add_space(6.0);
        }
        let ticked_now = ticked_on_card(&ticked, sources);
        let k = ticked_now.len();
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    Button::new(ConfirmPick::words(&pick.accept, k))
                        .style(Style::Filled)
                        .enabled(k > 0)
                        .id_salt(("cf-ok", c.id)),
                )
                .clicked()
                && k > 0
            {
                out.push(Command::ConfirmPicked { id: c.id, sources: ticked_now.clone(), accept: true });
                ticked.clear();
            }
            if ui
                .add(
                    Button::new(ConfirmPick::words(&pick.skip, k))
                        .style(Style::Text)
                        .enabled(k > 0)
                        .id_salt(("cf-skip", c.id)),
                )
                .clicked()
                && k > 0
            {
                out.push(Command::ConfirmPicked { id: c.id, sources: ticked_now.clone(), accept: false });
                ticked.clear();
            }
        });
    });
    ui.ctx().data_mut(|m| m.insert_temp(key, ticked));
}

/// "Раздача": how much the network is used and how much was given away,
/// then torrents and Soulseek apart, each with its policy (editable here or
/// one click from its Settings row) and what it gives away.
fn seeding(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    share_summary(ui, &state.downloads.sharing);
    ui.add_space(24.0);
    torrent_share(ui, state, pages);
    ui.add_space(24.0);
    slsk_share(ui, &state.downloads.sharing.slsk, pages, out);
    None
}

/// The top card: speeds now and what was given away, summed and split.
fn share_summary(ui: &mut Ui, sh: &SharingView) {
    let s = Theme::get(ui.ctx()).scheme;
    let split = |t: u64, k: u64, f: fn(u64) -> String| format!("торренты {} · Soulseek {}", f(t), f(k));
    let tiles = [
        ("Отдача", fmt_rate(sh.up_bps()), split(sh.torrent_up_bps, sh.slsk_up_bps, fmt_rate)),
        ("Загрузка", fmt_rate(sh.down_bps()), split(sh.torrent_down_bps, sh.slsk_down_bps, fmt_rate)),
        ("Отдано сегодня", fmt_size(sh.today()), split(sh.torrent_today, sh.slsk_today, fmt_size)),
        ("Отдано всего", fmt_size(sh.total()), split(sh.torrent_total, sh.slsk_total, fmt_size)),
    ];
    Card::filled().id_salt("share-summary").show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Four across, or two by two when narrow.
        let per_row = if ui.available_width() < 640.0 { 2 } else { 4 };
        for (r, row) in tiles.chunks(per_row).enumerate() {
            if r > 0 {
                ui.add_space(16.0);
            }
            ui.columns(per_row, |cols| {
                for (col, (caption, value, parts)) in cols.iter_mut().zip(row) {
                    col.spacing_mut().item_spacing.y = 2.0;
                    col.label(TypeRole::LabelLarge.style().rich(*caption, s.on_surface_variant));
                    col.label(TypeRole::HeadlineSmall.style().rich(value.as_str(), s.on_surface));
                    col.add(egui::Label::new(TypeRole::BodySmall.style().rich(parts.as_str(), s.on_surface_variant)).wrap());
                }
            });
        }
        if sh.seeding_disk_bytes > 0 {
            ui.add_space(12.0);
            ui.label(TypeRole::BodyMedium.style().rich(
                format!("Раздачи занимают на диске {}", fmt_size(sh.seeding_disk_bytes)),
                s.on_surface_variant,
            ));
        }
    });
}

/// A section title.
fn section(ui: &mut Ui, title: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
    ui.add_space(8.0);
}

/// A short line in a section (nothing there, or what is shared).
fn note(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(text, s.on_surface_variant)).wrap());
}

/// Policy rows: each opens its row in Settings.
fn policy_rows(ui: &mut Ui, salt: &str, rows: &[(&str, String, SettingId)], pages: &mut PageState) {
    let outs = segmented_list(ui, rows.len(), |i| {
        let (title, value, _) = &rows[i];
        ListItem::new(*title).trailing_text(value.as_str()).id_salt((salt, i))
    });
    for (o, (_, _, id)) in outs.iter().zip(rows) {
        if o.response.clicked() {
            pages.open_setting(*id);
        }
    }
}

/// «Торренты»: the policy, what the held torrents uploaded against the cap,
/// and a row per torrent.
fn torrent_share(ui: &mut Ui, state: &ViewState, pages: &mut PageState) {
    let s = Theme::get(ui.ctx()).scheme;
    let d = &state.downloads;
    let p = &state.prefs;
    section(ui, "Торренты");
    if p.no_seed {
        policy_rows(ui, "seed-off", &[("Раздача выключена", "включить".to_owned(), SettingId::NoSeed)], pages);
    } else {
        let limit = u64::from(p.seed_limit_gb) * (1 << 30);
        Card::filled().id_salt("seed-header").show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(TypeRole::TitleMedium.style().rich("Отдано раздачами", s.on_surface));
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
            ui.add_space(8.0);
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
        });
        ui.add_space(8.0);
        policy_rows(
            ui,
            "seed-policy",
            &[
                ("Раздавать до ratio", ratio_target(p.seed_ratio), SettingId::SeedRatio),
                ("Срок раздачи", format!("{} дн. после скачивания", p.seed_days), SettingId::SeedDays),
                ("Лимит на все раздачи", if limit > 0 { format!("{} ГБ", p.seed_limit_gb) } else { "без лимита".to_owned() }, SettingId::SeedLimit),
                ("Скорость отдачи", super::settings::fmt_speed(p.upload_limit_kbps), SettingId::UploadLimit),
            ],
            pages,
        );
    }
    ui.add_space(12.0);
    if d.seeding.is_empty() {
        note(ui, "Сейчас ничего не раздаётся. Скачанные торренты раздаются здесь, пока не сработает политика.");
        return;
    }
    let _ = segmented_list(ui, d.seeding.len(), |i| seed_row(&d.seeding[i], p.seed_ratio));
}

/// "до 2.0" / "без лимита" (the ratio slider's own words).
fn ratio_target(r: f32) -> String {
    if r < 0.05 { "без лимита".to_owned() } else { format!("{r:.1}") }
}

/// What one torrent does: speed and peers now, what it gave away against
/// the ratio it stops at, its size and when it is cleaned up.
fn seed_line(r: &SeedRow, seed_ratio: f32) -> String {
    let mut parts = Vec::new();
    if r.up_bps > 0 {
        parts.push(format!("отдаёт {}", fmt_rate(r.up_bps)));
    }
    parts.push(format!("{} {}", r.peers, plural(u64::from(r.peers), "пир", "пира", "пиров")));
    let ratio = if r.size_bytes > 0 { r.uploaded_bytes as f64 / r.size_bytes as f64 } else { 0.0 };
    let target = if seed_ratio < 0.05 { String::new() } else { format!(" из {seed_ratio:.1}") };
    parts.push(format!("отдано {} · ratio {ratio:.2}{target}", fmt_size(r.uploaded_bytes)));
    parts.push(fmt_size(r.size_bytes));
    if let Some(e) = &r.expires {
        parts.push(e.clone());
    }
    parts.join(" · ")
}

fn seed_row(r: &SeedRow, seed_ratio: f32) -> ListItem<'static> {
    ListItem::new(&r.title)
        .supporting(seed_line(r, seed_ratio))
        .leading_icon(Icon::CloudDownload)
        .id_salt(("seed", r.id))
}

/// «Soulseek»: what is shared, the switch, limit and slots, and the uploads.
fn slsk_share(ui: &mut Ui, k: &SlskSharingView, pages: &mut PageState, out: &mut Vec<Command>) {
    section(ui, "Soulseek");
    if !k.configured {
        policy_rows(ui, "slsk-none", &[("Soulseek не подключён", "настроить".to_owned(), SettingId::Soulseek)], pages);
        return;
    }
    // The switch and the slots are drawn inside the rows; what they pick
    // comes out after.
    let (share_on, slots_picked) = (std::cell::Cell::new(k.share), std::cell::Cell::new(None::<u32>));
    let labels = ["1", "2", "3", "4", "5"];
    let (share, slots, labels) = (&share_on, &slots_picked, &labels);
    let outs = segmented_list(ui, 3, |i| match i {
        0 => ListItem::new("Раздавать библиотеку").id_salt("share-slsk-on").trailing(move |ui| {
            let mut on = share.get();
            ui.add(Switch::new(&mut on).id_salt("share-slsk-switch"));
            share.set(on);
        }),
        1 => ListItem::new("Скорость отдачи")
            .trailing_text(super::settings::fmt_speed(k.upload_limit_kbps))
            .id_salt("share-slsk-speed"),
        _ => ListItem::new("Отдач одновременно").id_salt("share-slsk-slots").trailing(move |ui| {
            // Five buttons of 48 and their 2 dp gaps, at the row's end.
            let at = k.upload_slots.saturating_sub(1) as usize;
            ui.allocate_ui_with_layout(vec2(5.0 * 48.0 + 8.0, 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if let Some(i) = ConnectedGroup::new("share-slsk-slots-group", labels).equal_width(48.0).show(ui, at) {
                    slots.set(Some(i as u32 + 1));
                }
            });
        }),
    });
    if share.get() != k.share {
        out.push(Command::SetSoulseekShare { share: share.get(), upload_limit_kbps: k.upload_limit_kbps });
    }
    if let Some(n) = slots.get()
        && n != k.upload_slots
    {
        out.push(Command::SetSoulseekSlots(n));
    }
    if outs.get(1).is_some_and(|o| o.response.clicked()) {
        pages.open_setting(SettingId::SoulseekUpload);
    }
    ui.add_space(12.0);
    if !k.share {
        note(ui, "Библиотека не раздаётся: другие не могут у тебя ничего скачать.");
        return;
    }
    if !k.running {
        note(ui, "Soulseek не запущен: раздача начнётся, когда он подключится.");
        return;
    }
    note(ui, &shared_line(k));
    ui.add_space(12.0);
    let (now, done): (Vec<&UploadRow>, Vec<&UploadRow>) =
        k.uploads.iter().partition(|u| matches!(u.state, UploadRowState::Sending | UploadRowState::Queued));
    if now.is_empty() && done.is_empty() {
        note(ui, "Пока никто ничего не скачивал.");
        return;
    }
    for (title, rows, salt) in [("Отдаётся сейчас", &now, "up-now"), ("Отдано недавно", &done, "up-done")] {
        if rows.is_empty() {
            continue;
        }
        let s = Theme::get(ui.ctx()).scheme;
        ui.label(TypeRole::TitleSmall.style().rich(title, s.on_surface_variant));
        ui.add_space(4.0);
        let _ = segmented_list(ui, rows.len(), |i| {
            let u = rows[i];
            ListItem::new(&u.file).supporting(upload_line(u)).leading_icon(Icon::Person).id_salt((salt, u.id))
        });
        ui.add_space(12.0);
    }
}

/// What Soulseek shares, in one line.
fn shared_line(k: &SlskSharingView) -> String {
    let files = if k.files == 0 {
        "список файлов ещё собирается".to_owned()
    } else {
        format!(
            "{} {} в {} {}",
            k.files,
            plural(u64::from(k.files), "файл", "файла", "файлов"),
            k.folders,
            if k.folders % 10 == 1 && k.folders % 100 != 11 { "папке" } else { "папках" },
        )
    };
    if k.remote {
        format!("Раздаёт утилита nikgob-slsk на другом компьютере: {files}.")
    } else {
        format!("Раздаются папки Music и Downloads библиотеки — песни и их обложки: {files}.")
    }
}

/// Who takes the file and how far it is.
fn upload_line(u: &UploadRow) -> String {
    match u.state {
        UploadRowState::Queued => format!("{} · в очереди · {}", u.user, fmt_size(u.size)),
        UploadRowState::Sending => format!(
            "{} · {} из {} · {}",
            u.user,
            fmt_size(u.sent),
            fmt_size(u.size),
            fmt_rate(u.bytes_per_sec)
        ),
        UploadRowState::Done => format!("{} · {} · {}", u.user, fmt_size(u.size), fmt_rate(u.bytes_per_sec)),
        UploadRowState::Failed => format!("{} · прервано на {} из {}", u.user, fmt_size(u.sent), fmt_size(u.size)),
    }
}

/// Bytes per second as the page says it ("0 Б/с", "512 КБ/с", "1.2 МБ/с").
fn fmt_rate(bps: u64) -> String {
    format!("{}/с", fmt_size(bps))
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
        if active && j.controllable {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(IconButton::new(Icon::Close).size(Size::S).tooltip("Отменить").id_salt(("job-x", j.id)))
                    .clicked()
                {
                    out.push(Command::CancelJob(j.id));
                }
                if j.pausable
                    && ui
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

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(state: UploadRowState) -> UploadRow {
        UploadRow {
            id: 1,
            user: "peer".into(),
            file: "01 Song.flac".into(),
            size: 30 * 1024 * 1024,
            sent: 3 * 1024 * 1024,
            bytes_per_sec: 512 * 1024,
            state,
            ended_at: 0,
        }
    }

    /// A song that left its card (turned into an MP3, kept) is ticked no
    /// more; the ticked ones go in the card's order; the buttons count them.
    #[test]
    fn ticks_follow_the_card_and_the_buttons_count_them() {
        let ids: Vec<SourceId> = (1..=3u128).map(|n| SourceId(nikgob_core::model::Uuid::from_u128(n))).collect();
        let gone = SourceId(nikgob_core::model::Uuid::from_u128(9));
        assert_eq!(ticked_on_card(&[ids[2], gone, ids[0]], &ids), vec![ids[0], ids[2]]);
        assert_eq!(ticked_on_card(&[ids[1]], &ids[..1]), Vec::<SourceId>::new());
        assert!(ticked_on_card(&[], &ids).is_empty(), "nothing is ticked at first");
        let accept = ("Превратить".to_owned(), "в MP3".to_owned());
        let skip = ("Оставить".to_owned(), "FLAC".to_owned());
        assert_eq!(ConfirmPick::words(&accept, 3), "Превратить 3 в MP3");
        assert_eq!(ConfirmPick::words(&accept, 0), "Превратить в MP3");
        assert_eq!(ConfirmPick::words(&skip, 2), "Оставить 2 FLAC");
        assert_eq!(ConfirmPick::words(&("Убрать".to_owned(), String::new()), 4), "Убрать 4");
    }

    #[test]
    fn rates_read_per_second() {
        assert_eq!(fmt_rate(0), "0 Б/с");
        assert_eq!(fmt_rate(512 * 1024), "512 КБ/с");
        assert_eq!(fmt_rate(1536 * 1024), "1.5 МБ/с");
    }

    #[test]
    fn ratio_target_says_no_limit_at_zero() {
        assert_eq!(ratio_target(0.0), "без лимита");
        assert_eq!(ratio_target(2.0), "2.0");
    }

    #[test]
    fn seed_line_tells_speed_peers_ratio_and_cleanup() {
        let r = SeedRow {
            id: 1,
            title: "Album".into(),
            uploaded_bytes: 512 * 1024 * 1024,
            downloaded_bytes: 1024 * 1024 * 1024,
            expires: Some("чистка через 3 дн.".into()),
            up_bps: 100 * 1024,
            down_bps: 0,
            peers: 2,
            size_bytes: 1024 * 1024 * 1024,
        };
        assert_eq!(
            seed_line(&r, 2.0),
            "отдаёт 100 КБ/с · 2 пира · отдано 512 МБ · ratio 0.50 из 2.0 · 1.0 ГБ · чистка через 3 дн."
        );
        let idle = SeedRow { up_bps: 0, peers: 0, expires: None, size_bytes: 0, ..r };
        assert_eq!(seed_line(&idle, 0.0), "0 пиров · отдано 512 МБ · ratio 0.00 · 0 Б");
    }

    #[test]
    fn shared_line_counts_files_and_folders() {
        let k = SlskSharingView { files: 4812, folders: 391, ..Default::default() };
        assert_eq!(
            shared_line(&k),
            "Раздаются папки Music и Downloads библиотеки — песни и их обложки: 4812 файлов в 391 папке."
        );
        let k = SlskSharingView { files: 2, folders: 12, remote: true, ..Default::default() };
        assert_eq!(shared_line(&k), "Раздаёт утилита nikgob-slsk на другом компьютере: 2 файла в 12 папках.");
        let k = SlskSharingView::default();
        assert!(shared_line(&k).ends_with("список файлов ещё собирается."));
    }

    #[test]
    fn upload_lines_follow_the_state() {
        assert_eq!(upload_line(&upload(UploadRowState::Queued)), "peer · в очереди · 30.0 МБ");
        assert_eq!(upload_line(&upload(UploadRowState::Sending)), "peer · 3.0 МБ из 30.0 МБ · 512 КБ/с");
        assert_eq!(upload_line(&upload(UploadRowState::Done)), "peer · 30.0 МБ · 512 КБ/с");
        assert_eq!(upload_line(&upload(UploadRowState::Failed)), "peer · прервано на 3.0 МБ из 30.0 МБ");
    }
}
