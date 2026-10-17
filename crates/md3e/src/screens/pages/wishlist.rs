//! Загрузки → «Хочу скачать» (docs/stage3.md §3.5, §3.7): stream tracks on
//! their way to a file (likes, «Скачать», imports). One summary line with
//! «Скачать все», filter chips by status, the list grouped by album or by
//! artist. A row says where its track is in words (no coloured pills) and
//! offers the one thing that helps now: «Скачать», «Проверить», or for a
//! track without a torrent «Сохранить с YouTube ▾» with the ways to bring a
//! file in by hand. Dismissed items can come back. The list draws only the
//! rows on screen, so thousands of imported likes stay smooth.

use std::collections::HashMap;

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    Command, Route, ViewState, WishlistAttach, WishlistGroup, WishlistId, WishlistRow, WishlistScope, WishlistSelect,
    WishlistStatus as St,
};

use super::{Ask, PageState, connected, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, Card, FilterChip, IconButton, Leading, ListItem, LoadingIndicator, Menu, MenuItem, Radii, Size,
        SplitButton, Style, paint,
    },
    screens::common,
    tokens::shape,
};

/// What the filter chips pick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    /// Everything not done and not dismissed.
    #[default]
    All,
    Waiting,
    Searching,
    Downloading,
    Confirm,
    NoTorrent,
    Done,
    Dismissed,
}

impl Filter {
    const CHIPS: [(Self, &'static str); 8] = [
        (Self::All, "Все"),
        (Self::Waiting, "Ждут"),
        (Self::Searching, "Ищу"),
        (Self::Downloading, "Качаются"),
        (Self::Confirm, "Проверить"),
        (Self::NoTorrent, "Нет раздачи"),
        (Self::Done, "Готово"),
        (Self::Dismissed, "Не качать"),
    ];

    fn takes(self, st: St) -> bool {
        match self {
            Self::All => !matches!(st, St::Done | St::Dismissed),
            Self::Waiting => st == St::Waiting,
            Self::Searching => matches!(st, St::New | St::Searching),
            Self::Downloading => st == St::Downloading,
            Self::Confirm => st == St::Confirm,
            Self::NoTorrent => matches!(st, St::NoTorrent | St::Failed),
            Self::Done => st == St::Done,
            Self::Dismissed => st == St::Dismissed,
        }
    }

    fn key() -> Id {
        Id::new("wish-filter")
    }
}

/// The status of an item in words, for its supporting line.
fn status_text(r: &WishlistRow, torrents: bool) -> String {
    match r.status {
        St::Waiting | St::New | St::Searching if !torrents => "ждёт торренты".into(),
        St::Waiting => "ждёт · скачается по кнопке".into(),
        St::New | St::Searching => "ищу раздачу…".into(),
        St::Confirm => "нужно выбрать раздачу".into(),
        St::Downloading => match r.progress {
            Some(p) => format!("качается · {p}%"),
            None => "качается".into(),
        },
        St::NoTorrent | St::Failed => match &r.next_try {
            Some(next) => format!("не нашёл раздачу · {next}"),
            None => "не нашёл раздачу".into(),
        },
        St::Done => "скачано".into(),
        St::Dismissed => "не качаю".into(),
    }
}

/// Which grouping the tab shows.
fn by_artist(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("wish-by-artist"))).unwrap_or(false)
}

/// One line of the list: a group's head or an item.
enum Line<'a> {
    Head { g: &'a WishlistGroup, waiting: u32, open: Vec<WishlistId> },
    Item(&'a WishlistRow),
}

impl Line<'_> {
    fn height(&self) -> f32 {
        match self {
            Line::Head { .. } => 56.0,
            Line::Item(_) => crate::components::list::TWO_LINE,
        }
    }
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let w = &state.downloads.wishlist;
    let torrents = state.prefs.prowlarr_configured;
    if !w.loaded {
        ui.add(LoadingIndicator::new());
        return None;
    }
    if w.items.is_empty() {
        widgets::empty_state(
            ui,
            Icon::Download,
            "Пока ничего",
            "Нажми «Скачать» у трека или альбома из Spotify и YouTube Music — он появится здесь и скачается сам.",
            None,
        );
        return None;
    }

    // Without Prowlarr nothing downloads by itself: say it first.
    if !torrents {
        Card::filled().id_salt("wish-no-torrents").show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(TypeRole::TitleSmall.style().rich("Торренты не настроены", s.on_surface));
            ui.label(TypeRole::BodyMedium.style().rich_para(
                "Всё отсюда скачается, когда подключишь Prowlarr. А пока трек можно сохранить с YouTube — качество пониже, торрент потом заменит его.",
                s.on_surface_variant,
            ));
            ui.add_space(8.0);
            if ui.add(Button::new("Настроить торренты").icon(Icon::Settings).style(Style::Tonal).id_salt("wish-setup")).clicked() {
                pages.open_setting(super::SettingId::Prowlarr);
            }
        });
        ui.add_space(12.0);
    }

    let by_id: HashMap<WishlistId, &WishlistRow> = w.items.iter().map(|r| (r.id, r)).collect();
    let count = |f: Filter| w.items.iter().filter(|r| f.takes(r.status)).count();
    let live = count(Filter::All) as u64;
    let waiting = w.count(St::Waiting);

    // Summary and «Скачать все».
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 48.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let mut line = format!("{live} {}", common::plural(live, "трек", "трека", "треков"));
        if waiting > 0 {
            line = format!("{line} · {waiting} {}", common::plural(u64::from(waiting), "ждёт", "ждут", "ждут"));
        }
        ui.label(TypeRole::TitleMedium.style().rich(line, s.on_surface));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if waiting > 0 {
                let b = ui
                    .add(
                        Button::new(format!("Скачать все ({waiting})"))
                            .icon(Icon::Download)
                            .style(Style::Filled)
                            .enabled(torrents)
                            .id_salt("wish-all"),
                    )
                    .on_disabled_hover_text("Сначала подключи торренты");
                if b.clicked() {
                    out.push(Command::WishlistDownload(WishlistSelect::AllWaiting));
                }
            }
        });
    });
    if torrents {
        let auto = if state.prefs.like_downloads { "; новые лайки качаются сами" } else { "" };
        ui.label(TypeRole::BodySmall.style().rich(
            format!("Одновременно качается до {}{auto}.", state.prefs.auto_download_limit.max(1)),
            s.on_surface_variant,
        ));
    }
    ui.add_space(8.0);

    // Filter chips (the ones with something in them) and the grouping.
    let mut f: Filter = ui.ctx().data(|d| d.get_temp(Filter::key())).unwrap_or_default();
    if f != Filter::All && count(f) == 0 {
        f = Filter::All;
    }
    let artist_view = by_artist(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (chip, label) in Filter::CHIPS {
            let n = count(chip);
            if n == 0 && chip != Filter::All {
                continue;
            }
            if ui.add(FilterChip::new(format!("{label} {n}"), f == chip).id_salt(("wish-chip", label))).clicked() {
                f = chip;
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(Filter::key(), f));
    let grouped = !matches!(f, Filter::Done | Filter::Dismissed);
    if grouped {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if let (Some(i), _) = connected(ui, Id::new("wish-group"), &["По альбомам", "По исполнителям"], usize::from(artist_view), &[]) {
                ui.ctx().data_mut(|d| d.insert_temp(Id::new("wish-by-artist"), i == 1));
            }
        });
    }
    ui.add_space(8.0);

    // The lines: group heads and their items (only what the chip takes).
    let mut lines: Vec<Line<'_>> = Vec::new();
    if grouped {
        let groups = if artist_view { &w.by_artist } else { &w.by_album };
        let mut shown = std::collections::HashSet::new();
        for g in groups {
            let rows: Vec<&WishlistRow> = g.items.iter().filter_map(|id| by_id.get(id).copied()).filter(|r| f.takes(r.status)).collect();
            if rows.is_empty() {
                continue;
            }
            shown.extend(rows.iter().map(|r| r.id));
            let waiting = rows.iter().filter(|r| r.status == St::Waiting).count() as u32;
            let open = rows.iter().map(|r| r.id).collect();
            lines.push(Line::Head { g, waiting, open });
            lines.extend(rows.into_iter().map(Line::Item));
        }
        // Items no group took (no album or artist known).
        let rest: Vec<&WishlistRow> = w.items.iter().filter(|r| f.takes(r.status) && !shown.contains(&r.id)).collect();
        if !rest.is_empty() {
            lines.push(Line::Head { g: &LOOSE, waiting: rest.iter().filter(|r| r.status == St::Waiting).count() as u32, open: rest.iter().map(|r| r.id).collect() });
            lines.extend(rest.into_iter().map(Line::Item));
        }
    } else {
        lines.extend(w.items.iter().filter(|r| f.takes(r.status)).map(Line::Item));
    }
    if f == Filter::Dismissed {
        ui.label(TypeRole::BodySmall.style().rich(
            "Их больше не предлагаю — ни при импорте, ни из лайков в сервисах. «Вернуть» — и снова качаются.",
            s.on_surface_variant,
        ));
        ui.add_space(4.0);
    }

    // Laid out at their heights; drawn only where they meet the viewport.
    let gap = 4.0;
    let mut tops = Vec::with_capacity(lines.len());
    let mut y = 0.0;
    for l in &lines {
        tops.push(y);
        y += l.height() + gap;
    }
    let width = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(width, y.max(0.0)), Sense::hover());
    let clip = ui.clip_rect();
    for (i, l) in lines.iter().enumerate() {
        let r = Rect::from_min_size(pos2(area.left(), area.top() + tops[i]), vec2(width, l.height()));
        if r.bottom() < clip.top() - 80.0 || r.top() > clip.bottom() + 80.0 {
            continue;
        }
        let mut child = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("wish-line", i)));
        match l {
            Line::Head { g, waiting, open } => {
                group_head(&mut child, g, if torrents { *waiting } else { 0 }, open, artist_view, out);
            }
            Line::Item(row) => item(&mut child, row, torrents, pages, out),
        }
    }
    None
}

/// The group of items without an album or artist.
static LOOSE: WishlistGroup = WishlistGroup {
    title: String::new(),
    subtitle: String::new(),
    release: None,
    artist: None,
    items: Vec::new(),
    waiting: 0,
};

/// An album's (or artist's) head: its name and, when several wait (and
/// torrents can fetch them), «Скачать N»; ⋮: the whole group, or none of it.
fn group_head(ui: &mut Ui, g: &WishlistGroup, waiting: u32, open: &[WishlistId], artist_view: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let salt = (&g.title, &g.subtitle, open.len());
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 56.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let title = if g.title.is_empty() { "Отдельные треки" } else { g.title.as_str() };
        ui.label(TypeRole::TitleMedium.style().rich(title, s.on_surface));
        if !g.subtitle.is_empty() {
            ui.label(TypeRole::BodyMedium.style().rich(format!("· {}", g.subtitle), s.on_surface_variant));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let more = ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("wish-group-more", salt)));
            let select = || match (artist_view, g.release, g.artist) {
                (false, Some(r), _) => WishlistSelect::Album(r),
                (true, _, Some(a)) => WishlistSelect::Artist(a),
                _ => WishlistSelect::Items(open.to_vec()),
            };
            let picked = Menu::new(("wish-group-menu", salt))
                .align_right(true)
                .item(MenuItem::new("Скачать все").icon(Icon::Download).enabled(waiting > 0))
                .item(MenuItem::new("Не качать ничего отсюда").icon(Icon::Block))
                .attach(&more);
            match picked {
                Some(0) => out.push(Command::WishlistDownload(select())),
                Some(1) => out.push(Command::WishlistDismiss(open.to_vec())),
                _ => {}
            }
            // One waiting item has its own button: no second one here.
            if waiting >= 2
                && ui
                    .add(
                        Button::new(format!("Скачать {waiting}"))
                            .icon(Icon::Download)
                            .style(Style::Tonal)
                            .size(Size::S)
                            .id_salt(("wish-group-dl", salt)),
                    )
                    .clicked()
            {
                out.push(Command::WishlistDownload(select()));
            }
        });
    });
}

/// One item: cover, title, what it is and where it is, the one action that
/// helps now, ⋮ for the rest. Double click plays it.
fn item(ui: &mut Ui, r: &WishlistRow, torrents: bool, pages: &mut PageState, out: &mut Vec<Command>) {
    let id = r.id;
    let what = match (r.scope, &r.album) {
        (WishlistScope::Album, _) => format!("альбом · {}", r.artist),
        (_, Some(a)) if !a.is_empty() => format!("{} · {a}", r.artist),
        _ => r.artist.clone(),
    };
    let line = format!("{what} · {}", status_text(r, torrents));
    let mut more = None;
    let mut split = None;
    let o = ListItem::new(&r.title)
        .supporting(line)
        .leading(Leading::Image(
            r.cover.as_ref().map(|c| c.uri()),
            if r.scope == WishlistScope::Album { Icon::Album } else { Icon::MusicNote },
        ))
        .muted(r.status == St::Dismissed)
        .trailing(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if r.status != St::Dismissed {
                    more = Some(ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("wish-more", id))));
                }
                match r.status {
                    St::Waiting if torrents => {
                        if ui.add(Button::new("Скачать").style(Style::Tonal).size(Size::S).id_salt(("wish-dl", id))).clicked() {
                            out.push(Command::WishlistDownload(WishlistSelect::Items(vec![id])));
                        }
                    }
                    St::Waiting | St::New | St::Searching if !torrents && r.can_save_from_youtube => {
                        save_button(ui, id, out);
                    }
                    St::Confirm => {
                        if ui.add(Button::new("Проверить").style(Style::Tonal).size(Size::S).id_salt(("wish-confirm", id))).clicked() {
                            pages.downloads_tab = super::downloads::tab_index(nikgob_view::DownloadsTab::Confirm);
                        }
                    }
                    St::Downloading => progress(ui, r.progress),
                    St::NoTorrent | St::Failed => {
                        // The way out without a torrent: YouTube's audio, or
                        // the user's own .torrent / magnet / file.
                        let main = if r.can_save_from_youtube { "Сохранить с YouTube" } else { "Искать снова" };
                        let o = SplitButton::new(("wish-split", id), main)
                            .size(Size::S)
                            .style(Style::Tonal)
                            .item(MenuItem::new("Свой .torrent…").icon(Icon::AttachFile))
                            .item(MenuItem::new("Magnet-ссылка…").icon(Icon::Link))
                            .item(MenuItem::new("Свой файл…").icon(Icon::UploadFile))
                            .item(MenuItem::new("Папка с альбомом…").icon(Icon::Folder))
                            .item(MenuItem::new("Искать снова").icon(Icon::Refresh))
                            .item(MenuItem::new("Не качать").icon(Icon::Block))
                            .trigger_tooltip("Другие способы")
                            .show(ui);
                        if o.main.clicked() {
                            out.push(if r.can_save_from_youtube {
                                Command::WishlistSaveFromYoutube(id)
                            } else {
                                Command::WishlistRetry(id)
                            });
                        }
                        split = o.picked;
                    }
                    St::Dismissed => {
                        if ui.add(Button::new("Вернуть").style(Style::Text).size(Size::S).id_salt(("wish-restore", id))).clicked() {
                            out.push(Command::WishlistRestore(vec![id]));
                        }
                    }
                    _ => {}
                }
            });
        })
        .id_salt(("wish-row", id))
        .show(ui);
    if o.response.double_clicked() {
        match (r.track, r.release) {
            (Some(t), _) if r.scope == WishlistScope::Track => out.push(Command::PlayTracks {
                tracks: vec![t],
                start: 0,
                context: "«Хочу скачать»".into(),
            }),
            (_, Some(rel)) => out.push(Command::PlayRelease { id: rel, start: None, shuffle: false }),
            (Some(t), None) => out.push(Command::PlayTracks { tracks: vec![t], start: 0, context: "«Хочу скачать»".into() }),
            _ => {}
        }
    }
    if let Some(i) = split {
        // The split button's menu: .torrent, magnet, file, folder, again, no.
        attach_pick(i + 3, r, pages, out);
    }
    let menu = |key: &str| {
        Menu::new((key, id))
            .align_right(true)
            .item(MenuItem::new("Скачать сейчас").icon(Icon::Download).enabled(matches!(r.status, St::Waiting | St::NoTorrent | St::Failed)))
            .item(MenuItem::new("Искать снова").icon(Icon::Refresh).enabled(r.status != St::Downloading))
            .divider()
            .item(MenuItem::new("Сохранить с YouTube").icon(Icon::Download).enabled(r.can_save_from_youtube))
            .item(MenuItem::new("Свой .torrent…").icon(Icon::AttachFile))
            .item(MenuItem::new("Magnet-ссылка…").icon(Icon::Link))
            .item(MenuItem::new("Свой файл…").icon(Icon::UploadFile))
            .item(MenuItem::new("Папка с альбомом…").icon(Icon::Folder))
            .divider()
            .item(MenuItem::new("Не качать").icon(Icon::Block))
    };
    let mut picked = more.as_ref().and_then(|m| menu("wish-menu").attach(m));
    if r.status != St::Dismissed {
        picked = picked.or_else(|| menu("wish-ctx").context_menu(&o.response));
    }
    match picked {
        Some(0) => out.push(Command::WishlistDownload(WishlistSelect::Items(vec![id]))),
        Some(1) => out.push(Command::WishlistRetry(id)),
        Some(2) => out.push(Command::WishlistSaveFromYoutube(id)),
        Some(7) => out.push(Command::WishlistDismiss(vec![id])),
        Some(i) => attach_pick(i, r, pages, out),
        None => {}
    }
}

/// The hand-made ways in, by their place in the ⋮ menu: 3 .torrent, 4
/// magnet, 5 file, 6 folder; the split button's menu maps onto these and
/// adds 7 search again and 8 dismiss.
fn attach_pick(i: usize, r: &WishlistRow, pages: &mut PageState, out: &mut Vec<Command>) {
    let id = r.id;
    match i {
        3 => {
            if let Some(f) = rfd::FileDialog::new().set_title("Свой .torrent для этого трека").add_filter("Торрент", &["torrent"]).pick_file() {
                out.push(Command::WishlistAttach { item: id, what: WishlistAttach::TorrentFile(f) });
            }
        }
        4 => pages.open_ask(Ask::WishMagnet(id)),
        5 => {
            if let Some(files) = rfd::FileDialog::new()
                .set_title("Файл этого трека")
                .add_filter("Музыка", &["flac", "wav", "alac", "aiff", "mp3", "m4a", "ogg", "opus"])
                .pick_files()
            {
                out.push(Command::WishlistAttach { item: id, what: WishlistAttach::Files(files) });
            }
        }
        6 => {
            if let Some(d) = rfd::FileDialog::new().set_title("Папка с альбомом").pick_folder() {
                out.push(Command::WishlistAttach { item: id, what: WishlistAttach::Folder(d) });
            }
        }
        7 => out.push(Command::WishlistRetry(id)),
        8 => out.push(Command::WishlistDismiss(vec![id])),
        _ => {}
    }
}

/// «Сохранить с YouTube» while nothing else can download it.
fn save_button(ui: &mut Ui, id: WishlistId, out: &mut Vec<Command>) {
    if ui
        .add(Button::new("Сохранить с YouTube").style(Style::Tonal).size(Size::S).id_salt(("wish-yt", id)))
        .on_hover_text("Аудио как есть (Opus ~160 кбит/с, качество пониже); торрент потом заменит его")
        .clicked()
    {
        out.push(Command::WishlistSaveFromYoutube(id));
    }
}

/// A thin bar with the percent, for an item that downloads.
fn progress(ui: &mut Ui, p: Option<u8>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(96.0, 24.0), Sense::hover());
    let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    paint::container(ui.painter(), bar, Radii::all(shape::FULL), s.secondary_container, Stroke::NONE);
    match p {
        Some(p) => {
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * f32::from(p.min(100)) / 100.0, bar.height()));
            if fill.width() > 0.5 {
                paint::container(ui.painter(), fill, Radii::all(shape::FULL), s.primary, Stroke::NONE);
            }
        }
        None => {
            // Not known yet: a short piece moving along.
            let t = ui.ctx().input(|i| i.time) as f32;
            let x = (t * 0.6).fract() * (bar.width() - 24.0);
            let fill = Rect::from_min_size(pos2(bar.left() + x, bar.top()), vec2(24.0, bar.height()));
            paint::container(ui.painter(), fill, Radii::all(shape::FULL), s.primary, Stroke::NONE);
            ui.ctx().request_repaint();
        }
    }
}
