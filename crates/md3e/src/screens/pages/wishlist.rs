//! Загрузки → «Хочу скачать» (docs/stage3.md §3.5, §3.7): liked stream
//! tracks on their way to a file. Grouped by album or by artist, with a
//! status chip per item, bulk «скачать» buttons, and for an item without a
//! torrent the ways to bring a file in by hand. Dismissed items stay folded
//! at the bottom.

use egui::{Color32, Id, Sense, Stroke, Ui, vec2};
use nikgob_view::{
    Command, Route, ViewState, WishlistAttach, WishlistGroup, WishlistId, WishlistRow, WishlistSelect,
    WishlistStatus as St,
};

use super::{Ask, PageState, connected, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Card, IconButton, LoadingIndicator, Menu, MenuItem, Radii, Size, Style, paint},
    screens::common,
    tokens::shape,
};

/// The chip text of a status.
pub fn status_label(st: St) -> &'static str {
    match st {
        St::Waiting => "ждёт",
        St::New | St::Searching => "ищем",
        St::Confirm => "подтверди",
        St::Downloading => "качается",
        St::Done => "готово",
        St::NoTorrent => "нет торрента",
        St::Failed => "не вышло",
        St::Dismissed => "отклонено",
    }
}

/// (container, content) of a status chip.
fn status_colors(ui: &Ui, st: St) -> (Color32, Color32) {
    let s = Theme::get(ui.ctx()).scheme;
    match st {
        St::Waiting => (s.surface_container_highest, s.on_surface_variant),
        St::New | St::Searching => (s.secondary_container, s.on_secondary_container),
        St::Confirm => (s.tertiary_container, s.on_tertiary_container),
        St::Downloading => (s.primary_container, s.on_primary_container),
        St::Done => (s.primary, s.on_primary),
        St::NoTorrent | St::Failed => (s.error_container, s.on_error_container),
        St::Dismissed => (s.surface_container_high, s.outline),
    }
}

fn status_icon(st: St) -> Icon {
    match st {
        St::Waiting => Icon::HourglassEmpty,
        St::New | St::Searching => Icon::Search,
        St::Confirm => Icon::Info,
        St::Downloading => Icon::Download,
        St::Done => Icon::Check,
        St::NoTorrent | St::Failed => Icon::Error,
        St::Dismissed => Icon::Block,
    }
}

/// A status chip: icon and label on the status's container.
pub fn status_chip(ui: &mut Ui, st: St) -> egui::Response {
    let (bg, fg) = status_colors(ui, st);
    let text = status_label(st);
    let g = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelMedium.font_id(), fg);
    let size = vec2(g.size().x + 34.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint::container(ui.painter(), rect, Radii::all(shape::SMALL), bg, Stroke::NONE);
    let ic = egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), vec2(16.0, 16.0));
    status_icon(st).widget(false, fg, 16.0).paint_at(ui, ic);
    ui.painter().galley(egui::pos2(rect.left() + 26.0, rect.center().y - g.size().y * 0.5), g, fg);
    resp
}

/// Which grouping the tab shows.
fn by_artist(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("wish-by-artist"))).unwrap_or(false)
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let w = &state.downloads.wishlist;
    if !w.loaded {
        ui.add(LoadingIndicator::new());
        return None;
    }
    let live: Vec<&WishlistRow> = w.items.iter().filter(|i| i.status != St::Dismissed && i.status != St::Done).collect();
    let dismissed: Vec<&WishlistRow> = w.items.iter().filter(|i| i.status == St::Dismissed).collect();
    if w.items.is_empty() {
        widgets::empty_state(
            ui,
            Icon::Favorite,
            "Пока ничего",
            "Лайкни трек из Spotify или YouTube Music — он появится здесь и скачается сам.",
            None,
        );
        return None;
    }

    // Header: counts and «скачать всё ждущее».
    let waiting = w.count(St::Waiting);
    Card::filled().id_salt("wish-header").show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(TypeRole::TitleMedium.style().rich("Хочу скачать", s.on_surface));
            ui.add_space(8.0);
            let n = live.len() as u64;
            ui.label(TypeRole::BodyMedium.style().rich(
                format!("{n} {}", common::plural(n, "трек", "трека", "треков")),
                s.on_surface_variant,
            ));
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for (st, n) in &w.counts {
                if *st == St::Dismissed || *n == 0 {
                    continue;
                }
                status_chip(ui, *st);
                ui.label(TypeRole::LabelLarge.style().rich(n.to_string(), s.on_surface));
                ui.add_space(8.0);
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(
                    Button::new(if waiting > 0 { format!("Скачать всё ждущее ({waiting})") } else { "Скачать всё ждущее".to_owned() })
                        .icon(Icon::Download)
                        .style(Style::Filled)
                        .enabled(waiting > 0)
                        .id_salt("wish-all"),
                )
                .clicked()
            {
                out.push(Command::WishlistDownload(WishlistSelect::AllWaiting));
            }
            ui.label(TypeRole::BodySmall.style().rich_para(
                format!("Одновременно качается до {}; новые лайки качаются сами.", state.prefs.auto_download_limit.max(1)),
                s.on_surface_variant,
            ));
        });
    });
    ui.add_space(12.0);

    let artist_view = by_artist(ui.ctx());
    ui.horizontal(|ui| {
        if let (Some(i), _) = connected(ui, Id::new("wish-group"), &["По альбомам", "По исполнителям"], usize::from(artist_view), &[]) {
            ui.ctx().data_mut(|d| d.insert_temp(Id::new("wish-by-artist"), i == 1));
        }
    });
    ui.add_space(8.0);

    let groups = if artist_view { &w.by_artist } else { &w.by_album };
    let mut shown: std::collections::HashSet<WishlistId> = std::collections::HashSet::new();
    for g in groups {
        let rows: Vec<&WishlistRow> = g.items.iter().filter_map(|id| live.iter().find(|r| r.id == *id).copied()).collect();
        if rows.is_empty() {
            continue;
        }
        shown.extend(rows.iter().map(|r| r.id));
        group(ui, g, &rows, artist_view, pages, out);
    }
    // Items no group took (no album or artist known).
    let rest: Vec<&WishlistRow> = live.iter().copied().filter(|r| !shown.contains(&r.id)).collect();
    if !rest.is_empty() {
        let g = WishlistGroup {
            title: "Отдельные треки".into(),
            subtitle: String::new(),
            release: None,
            artist: None,
            items: rest.iter().map(|r| r.id).collect(),
            waiting: rest.iter().filter(|r| r.status == St::Waiting).count() as u32,
        };
        group(ui, &g, &rest, artist_view, pages, out);
    }
    let done = w.items.iter().filter(|i| i.status == St::Done).count();
    if done > 0 {
        ui.label(TypeRole::BodySmall.style().rich(
            format!("Скачано и уже в библиотеке: {done}."),
            s.on_surface_variant,
        ));
        ui.add_space(8.0);
    }
    if !dismissed.is_empty() {
        dismissed_fold(ui, &dismissed);
    }
    None
}

/// One album (or artist): a header with its bulk button, then the items.
fn group(ui: &mut Ui, g: &WishlistGroup, rows: &[&WishlistRow], artist_view: bool, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(8.0);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(TypeRole::TitleMedium.style().rich(&g.title, s.on_surface));
        if !g.subtitle.is_empty() {
            ui.label(TypeRole::BodyMedium.style().rich(format!("· {}", g.subtitle), s.on_surface_variant));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, select) = if artist_view {
                ("Скачать всё артиста", g.artist.map(WishlistSelect::Artist))
            } else {
                ("Скачать альбом", g.release.map(WishlistSelect::Album))
            };
            let select = select.unwrap_or_else(|| WishlistSelect::Items(g.items.clone()));
            // Nothing waits here: no button (the items show what they do).
            if g.waiting > 0
                && ui
                .add(
                    Button::new(label)
                        .icon(Icon::Download)
                        .style(Style::Tonal)
                        .size(Size::S)
                        .id_salt(("wish-group-dl", &g.title, &g.subtitle)),
                )
                .clicked()
            {
                out.push(Command::WishlistDownload(select));
            }
            if g.waiting > 0 {
                ui.label(TypeRole::LabelMedium.style().rich(format!("ждут: {}", g.waiting), s.on_surface_variant));
            }
        });
    });
    for r in rows {
        item(ui, r, artist_view, pages, out);
    }
    ui.add_space(8.0);
}

/// One item: title, what we know, the status chip, its actions.
fn item(ui: &mut Ui, r: &WishlistRow, artist_view: bool, pages: &mut PageState, out: &mut Vec<Command>) {
    let mut line = Vec::new();
    if artist_view {
        line.extend(r.album.clone());
    } else {
        line.push(r.artist.clone());
    }
    line.extend(r.note.clone());
    line.extend(r.next_try.clone());
    let id = r.id;
    let mut more = None;
    let o = crate::components::ListItem::new(&r.title)
        .supporting(line.into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" · "))
        .leading(crate::components::Leading::Icon(if r.scope == nikgob_view::WishlistScope::Album { Icon::Album } else { Icon::MusicNote }))
        .trailing(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                more = Some(ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("wish-more", id))));
                match r.status {
                    St::Waiting => {
                        if ui.add(Button::new("Скачать").style(Style::Tonal).size(Size::S).id_salt(("wish-dl", id))).clicked() {
                            out.push(Command::WishlistDownload(WishlistSelect::Items(vec![id])));
                        }
                    }
                    St::NoTorrent | St::Failed => {
                        if ui.add(Button::new("Повторить").icon(Icon::Refresh).style(Style::Text).size(Size::S).id_salt(("wish-retry", id))).clicked() {
                            out.push(Command::WishlistRetry(id));
                        }
                        if r.can_save_from_youtube
                            && ui
                                .add(Button::new("Сохранить с YouTube").style(Style::Tonal).size(Size::S).id_salt(("wish-yt", id)))
                                .on_hover_text("Аудио как есть (Opus ~160 кбит/с, низкое качество); торрент позже заменит его")
                                .clicked()
                        {
                            out.push(Command::WishlistSaveFromYoutube(id));
                        }
                    }
                    St::Confirm => {
                        if ui.add(Button::new("Проверить").style(Style::Tonal).size(Size::S).id_salt(("wish-confirm", id))).clicked() {
                            pages.downloads_tab = super::downloads::tab_index(nikgob_view::DownloadsTab::Confirm);
                        }
                    }
                    St::New | St::Searching | St::Downloading | St::Done | St::Dismissed => {}
                }
                status_chip(ui, r.status);
            });
        })
        .id_salt(("wish-row", id))
        .show(ui);
    let menu = |key: &str| {
        Menu::new((key, id))
            .align_right(true)
            .item(MenuItem::new("Скачать сейчас").icon(Icon::Download).enabled(r.status == St::Waiting))
            .item(MenuItem::new("Искать торрент снова").icon(Icon::Refresh).enabled(r.status != St::Downloading))
            .divider()
            .item(MenuItem::new("Сохранить с YouTube").icon(Icon::GraphicEq).enabled(r.can_save_from_youtube))
            .item(MenuItem::new("Добавить .torrent…").icon(Icon::AttachFile))
            .item(MenuItem::new("Магнет…").icon(Icon::Link))
            .item(MenuItem::new("Файл FLAC…").icon(Icon::UploadFile))
            .item(MenuItem::new("Папка FLAC…").icon(Icon::Folder))
            .divider()
            .item(MenuItem::new("Отклонить").icon(Icon::Block))
    };
    let mut picked = more.as_ref().and_then(|m| menu("wish-menu").attach(m));
    picked = picked.or_else(|| menu("wish-ctx").context_menu(&o.response));
    match picked {
        Some(0) => out.push(Command::WishlistDownload(WishlistSelect::Items(vec![id]))),
        Some(1) => out.push(Command::WishlistRetry(id)),
        Some(2) => out.push(Command::WishlistSaveFromYoutube(id)),
        Some(3) => {
            if let Some(f) = rfd::FileDialog::new().set_title("Свой .torrent для этого трека").add_filter("Торрент", &["torrent"]).pick_file() {
                out.push(Command::WishlistAttach { item: id, what: WishlistAttach::TorrentFile(f) });
            }
        }
        Some(4) => pages.open_ask(Ask::WishMagnet(id)),
        Some(5) => {
            if let Some(files) = rfd::FileDialog::new()
                .set_title("Файл этого трека")
                .add_filter("Музыка", &["flac", "wav", "alac", "aiff", "mp3", "m4a", "ogg", "opus"])
                .pick_files()
            {
                out.push(Command::WishlistAttach { item: id, what: WishlistAttach::Files(files) });
            }
        }
        Some(6) => {
            if let Some(d) = rfd::FileDialog::new().set_title("Папка с FLAC этого альбома").pick_folder() {
                out.push(Command::WishlistAttach { item: id, what: WishlistAttach::Folder(d) });
            }
        }
        Some(7) => out.push(Command::WishlistDismiss(vec![id])),
        _ => {}
    }
}

/// «Отклонённые (N)»: folded; they are never offered again.
fn dismissed_fold(ui: &mut Ui, rows: &[&WishlistRow]) {
    let s = Theme::get(ui.ctx()).scheme;
    let key = Id::new("wish-dismissed-open");
    let open = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    ui.add_space(8.0);
    let head = ui
        .horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            (if open { Icon::ExpandLess } else { Icon::ExpandMore }).widget(false, s.on_surface_variant, 20.0).paint_at(ui, r);
            ui.label(TypeRole::LabelLarge.style().rich(format!("Отклонённые ({})", rows.len()), s.on_surface_variant));
        })
        .response;
    if ui.interact(head.rect, key.with("hit"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(key, !open));
    }
    if !open {
        return;
    }
    ui.label(TypeRole::BodySmall.style().rich("Их больше не предлагаем — ни при импорте, ни из лайков в сервисах.", s.on_surface_variant));
    ui.scope(|ui| {
        ui.multiply_opacity(0.7);
        for r in rows {
            crate::components::ListItem::new(&r.title)
                .supporting(r.artist.clone())
                .leading(crate::components::Leading::Icon(Icon::Block))
                .id_salt(("wish-dismissed", r.id))
                .show(ui);
        }
    });
}
