//! Загрузки → «Хочу скачать»: stream tracks on
//! their way to a file (likes, «Скачать», imports). Filter chips by status
//! (their counts are the only numbers said), the grouping with «Скачать
//! все» and «Искать снова» on its line, the list grouped by album or by
//! artist. A row says where its track is in words (no
//! coloured pills) and offers the one thing that helps now: «Скачать»,
//! «Проверить», or for a track nothing was found for «Искать снова ▾» (with
//! no source on, «Сохранить с YouTube ▾») with the ways to bring a file in
//! by hand. Dismissed items can come back. The list draws only the
//! rows on screen, so thousands of imported likes stay smooth.

use std::collections::HashMap;

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    Command, Route, Transfer, TransferPhase, ViewState, WishlistAttach, WishlistGroup, WishlistId, WishlistRow,
    WishlistScope, WishlistSelect, WishlistStatus as St,
};

use super::{Ask, PageState, connected, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, Card, FilterChip, IconButton, Leading, ListItem, LoadingIndicator, Menu, MenuItem, Radii, Size,
        SplitButton, Style, paint,
    },
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
        (Self::NoTorrent, "Не нашлось"),
        (Self::Done, "Готово"),
        (Self::Dismissed, "Не качать"),
    ];

    /// Without a download source nothing searches: what is queued waits
    /// with the rest («Ждут»), and «Ищу» stays away.
    fn takes(self, st: St, torrents: bool) -> bool {
        match self {
            Self::All => !matches!(st, St::Done | St::Dismissed),
            Self::Waiting => st == St::Waiting || (!torrents && matches!(st, St::New | St::Searching)),
            Self::Searching => torrents && matches!(st, St::New | St::Searching),
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

/// How many items each chip takes, counted once over the items: the chips
/// and the buttons say the same numbers.
struct Counts([usize; Filter::CHIPS.len()]);

impl Counts {
    fn of(items: &[WishlistRow], torrents: bool) -> Self {
        let mut n = [0; Filter::CHIPS.len()];
        for r in items {
            for (i, (f, _)) in Filter::CHIPS.iter().enumerate() {
                n[i] += usize::from(f.takes(r.status, torrents));
            }
        }
        Self(n)
    }

    fn get(&self, f: Filter) -> usize {
        Filter::CHIPS.iter().position(|(c, _)| *c == f).map_or(0, |i| self.0[i])
    }
}

/// The status of an item in words, for its supporting line.
fn status_text(r: &WishlistRow, torrents: bool) -> String {
    // A file the library has, looked for in better quality: the runtime
    // words its state («MP3 128 · ищу lossless»).
    if let Some(u) = &r.upgrade {
        return u.clone();
    }
    match r.status {
        St::Waiting | St::New | St::Searching if !torrents => "ждёт источник скачивания".into(),
        St::Waiting => "ждёт · скачается по кнопке".into(),
        St::New | St::Searching => "ищу…".into(),
        St::Confirm if r.confirm_files => "файлы ждут подтверждения".into(),
        St::Confirm => "нужно выбрать, что скачать".into(),
        // The same transfer its row in «Активные» shows.
        St::Downloading => r.transfer.unwrap_or_default().status(r.source.as_deref()),
        St::NoTorrent | St::Failed if r.has_copy => match &r.next_try {
            Some(next) => format!("есть копия с YouTube · ищу лучше, {next}"),
            None => "есть копия с YouTube · ищу лучше".into(),
        },
        St::NoTorrent | St::Failed => match &r.next_try {
            Some(next) => format!("не нашёл · {next}"),
            None => "не нашёл".into(),
        },
        St::Done if r.missing > 0 => format!("скачано не всё · без файла {} тр.", r.missing),
        St::Done => "скачано".into(),
        St::Dismissed => "не качаю".into(),
    }
}

/// Which grouping the tab shows.
fn by_artist(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("wish-by-artist"))).unwrap_or(false)
}

/// One line of the list: a group's head or an item (and the head it is
/// under).
enum Line<'a> {
    Head { g: &'a WishlistGroup, waiting: u32, open: Vec<WishlistId> },
    Item(&'a WishlistRow, Under),
}

impl Line<'_> {
    fn height(&self) -> f32 {
        match self {
            Line::Head { .. } => 56.0,
            Line::Item(..) => crate::components::list::TWO_LINE,
        }
    }
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let w = &state.downloads.wishlist;
    let torrents = w.can_search;
    if !w.loaded {
        ui.add(LoadingIndicator::new());
        return None;
    }
    if w.items.is_empty() {
        widgets::empty_state(
            ui,
            Icon::Download,
            "Пока ничего",
            "",
            None,
        );
        return None;
    }

    // Without any download source nothing downloads by itself: say it first.
    if !torrents {
        Card::filled().id_salt("wish-no-torrents").show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(TypeRole::TitleSmall.style().rich("Все источники выключены", s.on_surface));
            ui.label(TypeRole::BodyMedium.style().rich_para(
                "Всё отсюда скачается, когда включишь источник в «Откуда качать». А пока трек можно сохранить с YouTube — качество пониже, найденное потом заменит его.",
                s.on_surface_variant,
            ));
            ui.add_space(8.0);
            if ui.add(Button::new("Откуда качать").icon(Icon::Settings).style(Style::Tonal).id_salt("wish-setup")).clicked() {
                pages.open_setting(super::SettingId::DownloadSource(nikgob_view::SourceKey::Torrents));
            }
        });
        ui.add_space(12.0);
    }

    let by_id: HashMap<WishlistId, &WishlistRow> = w.items.iter().map(|r| (r.id, r)).collect();
    // One count for the chips and the buttons.
    let counts = Counts::of(&w.items, torrents);
    // What «Скачать все» starts.
    let waiting = if torrents { counts.get(Filter::Waiting) } else { 0 };
    // A source added since: what was not found is searched for in all of
    // them again.
    let missed = if torrents { counts.get(Filter::NoTorrent) } else { 0 };

    // Filter chips (the ones with something in them) and the grouping.
    let mut f: Filter = ui.ctx().data(|d| d.get_temp(Filter::key())).unwrap_or_default();
    if f != Filter::All && counts.get(f) == 0 {
        f = Filter::All;
    }
    let artist_view = by_artist(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (chip, label) in Filter::CHIPS {
            let n = counts.get(chip);
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
    // The grouping, and on the same line what starts the waiting items or
    // searches again for the missed ones.
    if grouped || waiting > 0 || missed > 0 {
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            if grouped
                && let (Some(i), _) =
                    connected(ui, Id::new("wish-group"), &["По альбомам", "По исполнителям"], usize::from(artist_view), &[])
            {
                ui.ctx().data_mut(|d| d.insert_temp(Id::new("wish-by-artist"), i == 1));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if waiting > 0
                    && ui
                        .add(
                            Button::new(format!("Скачать все ({waiting})"))
                                .icon(Icon::Download)
                                .style(Style::Filled)
                                .id_salt("wish-all"),
                        )
                        .clicked()
                {
                    out.push(Command::WishlistDownload(WishlistSelect::AllWaiting));
                }
                if missed > 0
                    && ui
                        .add(
                            Button::new(format!("Искать снова ({missed})"))
                                .icon(Icon::Refresh)
                                .style(if waiting > 0 { Style::Tonal } else { Style::Filled })
                                .id_salt("wish-retry-all"),
                        )
                        .clicked()
                {
                    out.push(Command::WishlistRetryAll);
                }
            });
        });
    }
    ui.add_space(8.0);

    // The lines: group heads and their items (only what the chip takes).
    let mut lines: Vec<Line<'_>> = Vec::new();
    if grouped {
        let groups = if artist_view { &w.by_artist } else { &w.by_album };
        let mut shown = std::collections::HashSet::new();
        for g in groups {
            let rows: Vec<&WishlistRow> =
                g.items.iter().filter_map(|id| by_id.get(id).copied()).filter(|r| f.takes(r.status, torrents)).collect();
            if rows.is_empty() {
                continue;
            }
            shown.extend(rows.iter().map(|r| r.id));
            let waiting = rows.iter().filter(|r| r.status == St::Waiting).count() as u32;
            let open = rows.iter().map(|r| r.id).collect();
            lines.push(Line::Head { g, waiting, open });
            let under = if artist_view { Under::Artist } else { Under::Album };
            lines.extend(rows.into_iter().map(|r| Line::Item(r, under)));
        }
        // Items no group took (no album or artist known).
        let rest: Vec<&WishlistRow> =
            w.items.iter().filter(|r| f.takes(r.status, torrents) && !shown.contains(&r.id)).collect();
        if !rest.is_empty() {
            lines.push(Line::Head { g: &LOOSE, waiting: rest.iter().filter(|r| r.status == St::Waiting).count() as u32, open: rest.iter().map(|r| r.id).collect() });
            lines.extend(rest.into_iter().map(|r| Line::Item(r, Under::Nothing)));
        }
    } else {
        lines.extend(w.items.iter().filter(|r| f.takes(r.status, torrents)).map(|r| Line::Item(r, Under::Nothing)));
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
            Line::Item(row, under) => item(&mut child, row, *under, torrents, pages, out),
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
            // The rows' side padding: their ⋮ and this one in one column.
            ui.add_space(16.0);
            let more = ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("wish-group-more", salt)));
            let select = || match (artist_view, g.release, g.artist) {
                (false, Some(r), _) => WishlistSelect::Album(r),
                (true, _, Some(a)) => WishlistSelect::Artist(a),
                _ => WishlistSelect::Items(open.to_vec()),
            };
            // Nothing waiting: nothing to download from here.
            let mut menu = Menu::new(("wish-group-menu", salt)).align_right(true);
            if waiting > 0 {
                menu = menu.item(MenuItem::new(format!("Скачать все ({waiting})")).icon(Icon::Download)).divider();
            }
            let picked = menu.item(MenuItem::new("Не качать ничего отсюда").icon(Icon::Block)).attach(&more);
            match picked.map(|k| if waiting > 0 { k } else { k + 1 }) {
                Some(0) => out.push(Command::WishlistDownload(select())),
                Some(1) => out.push(Command::WishlistDismiss(open.to_vec())),
                _ => {}
            }
            // One waiting item has its own button: no second one here.
            if waiting >= 2
                && ui
                    .add(
                        Button::new(format!("Скачать все ({waiting})"))
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

/// The head a row is drawn under: what it says need not be said again.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Under {
    Album,
    Artist,
    Nothing,
}

/// One item: cover, title, what it is (less what its head says) and where
/// it is, the one action that helps now, ⋮ for the rest. Double click
/// plays it.
fn item(ui: &mut Ui, r: &WishlistRow, under: Under, torrents: bool, pages: &mut PageState, out: &mut Vec<Command>) {
    let id = r.id;
    let album = r.album.as_deref().filter(|a| !a.is_empty());
    let what = match (under, r.scope, album) {
        (Under::Album, WishlistScope::Album, _) => "весь альбом".to_owned(),
        (Under::Album, ..) => String::new(),
        (Under::Artist, WishlistScope::Album, _) => "альбом".to_owned(),
        (Under::Artist, _, Some(a)) => a.to_owned(),
        (Under::Artist, _, None) => String::new(),
        (Under::Nothing, WishlistScope::Album, _) => format!("альбом · {}", r.artist),
        (Under::Nothing, _, Some(a)) => format!("{} · {a}", r.artist),
        (Under::Nothing, _, None) => r.artist.clone(),
    };
    let status = status_text(r, torrents);
    let line = if what.is_empty() { status } else { format!("{what} · {status}") };
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
                    St::Downloading => progress(ui, r.transfer.unwrap_or_default()),
                    St::NoTorrent | St::Failed => {
                        // With a source on: search again (a new one may have
                        // it). Without one, YouTube's audio is the way out.
                        // The rest — the user's own .torrent / magnet / file —
                        // in the menu.
                        let can_save = r.can_save_from_youtube && !r.has_copy;
                        let save = !torrents && can_save;
                        let (main, other) = if save {
                            ("Сохранить с YouTube", MenuItem::new("Искать снова").icon(Icon::Refresh))
                        } else {
                            ("Искать снова", MenuItem::new("Сохранить с YouTube").icon(Icon::Download).enabled(can_save))
                        };
                        let o = SplitButton::new(("wish-split", id), main)
                            .size(Size::S)
                            .style(Style::Tonal)
                            .item(MenuItem::new("Свой .torrent…").icon(Icon::AttachFile))
                            .item(MenuItem::new("Magnet-ссылка…").icon(Icon::Link))
                            .item(MenuItem::new("Свой файл…").icon(Icon::UploadFile))
                            .item(MenuItem::new("Папка с альбомом…").icon(Icon::Folder))
                            .item(other)
                            .item(MenuItem::new("Не качать").icon(Icon::Block))
                            .trigger_tooltip("Другие способы")
                            .show(ui);
                        let (on_main, on_other) = if save {
                            (Command::WishlistSaveFromYoutube(id), Command::WishlistRetry(id))
                        } else {
                            (Command::WishlistRetry(id), Command::WishlistSaveFromYoutube(id))
                        };
                        if o.main.clicked() {
                            out.push(on_main);
                        }
                        match o.picked {
                            Some(4) => out.push(on_other),
                            p => split = p,
                        }
                    }
                    St::Dismissed => {
                        let back = ui.add(Button::new("Вернуть").style(Style::Text).size(Size::S).id_salt(("wish-restore", id)));
                        if back.clicked() {
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
        // The split button's menu: .torrent, magnet, file, folder, no.
        attach_pick(i + 3, r, pages, out);
    }
    let menu = |key: &str| {
        let (menu, acts) = widgets::menu_of((key, id), wish_rows(r));
        (menu.align_right(true), acts)
    };
    let mut picked = more.as_ref().and_then(|m| {
        let (menu, acts) = menu("wish-menu");
        menu.attach(m).and_then(|k| acts.get(k).copied())
    });
    if r.status != St::Dismissed && !Menu::context_idle(&o.response, ("wish-ctx", id)) {
        let (menu, acts) = menu("wish-ctx");
        picked = picked.or_else(|| menu.context_menu(&o.response).and_then(|k| acts.get(k).copied()));
    }
    match picked {
        Some(WishAct::Download) => out.push(Command::WishlistDownload(WishlistSelect::Items(vec![id]))),
        Some(WishAct::Retry) => out.push(Command::WishlistRetry(id)),
        Some(WishAct::Album) => {
            if let Some(rel) = r.release {
                out.push(Command::DownloadRelease(rel));
            }
        }
        Some(WishAct::Youtube) => out.push(Command::WishlistSaveFromYoutube(id)),
        Some(WishAct::Torrent) => attach_pick(3, r, pages, out),
        Some(WishAct::Magnet) => attach_pick(4, r, pages, out),
        Some(WishAct::File) => attach_pick(5, r, pages, out),
        Some(WishAct::Folder) => attach_pick(6, r, pages, out),
        Some(WishAct::Dismiss) => out.push(Command::WishlistDismiss(vec![id])),
        None => {}
    }
}

/// What a wishlist row's ⋮ does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WishAct {
    Download,
    Retry,
    Album,
    Youtube,
    Torrent,
    Magnet,
    File,
    Folder,
    Dismiss,
}

/// A wishlist row's ⋮: what its status allows (nothing greyed: what
/// cannot work now is not there) │ the hand-made ways in │ «Не качать».
fn wish_rows(r: &WishlistRow) -> Vec<widgets::Row<WishAct>> {
    use WishAct as A;
    use widgets::Row;
    let mut rows = Vec::new();
    if matches!(r.status, St::Waiting | St::NoTorrent | St::Failed) {
        rows.push(Row::Item(A::Download, MenuItem::new("Скачать").icon(Icon::Download)));
    }
    if r.status != St::Downloading {
        rows.push(Row::Item(A::Retry, MenuItem::new("Искать снова").icon(Icon::Refresh)));
    }
    if r.scope == WishlistScope::Track && r.release.is_some() {
        rows.push(Row::Item(A::Album, MenuItem::new("Скачать альбом").icon(Icon::Album)));
    }
    rows.push(Row::Divider);
    if r.can_save_from_youtube {
        rows.push(Row::Item(A::Youtube, MenuItem::new("Сохранить с YouTube").icon(Icon::Download)));
    }
    rows.push(Row::Item(A::Torrent, MenuItem::new("Свой .torrent…").icon(Icon::AttachFile)));
    rows.push(Row::Item(A::Magnet, MenuItem::new("Magnet-ссылка…").icon(Icon::Link)));
    rows.push(Row::Item(A::File, MenuItem::new("Свой файл…").icon(Icon::UploadFile)));
    rows.push(Row::Item(A::Folder, MenuItem::new("Папка с альбомом…").icon(Icon::Folder)));
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Dismiss, MenuItem::new("Не качать").icon(Icon::Block)));
    rows
}

/// The hand-made ways in: 3 .torrent, 4 magnet, 5 file, 6 folder, 8
/// dismiss (the split button's menu maps onto these from 0).
fn attach_pick(i: usize, r: &WishlistRow, pages: &mut PageState, out: &mut Vec<Command>) {
    let id = r.id;
    match i {
        3 => super::dialogs::file(
            |d| d.set_title("Свой .torrent для этого трека").add_filter("Торрент", &["torrent"]),
            move |f| Command::WishlistAttach { item: id, what: WishlistAttach::TorrentFile(f) },
            out,
        ),
        4 => pages.open_ask(Ask::WishMagnet(id)),
        5 => super::dialogs::files(
            |d| {
                d.set_title("Файл этого трека")
                    .add_filter("Музыка", &["flac", "wav", "alac", "aiff", "mp3", "m4a", "ogg", "opus"])
            },
            move |files| Command::WishlistAttach { item: id, what: WishlistAttach::Files(files) },
            out,
        ),
        6 => super::dialogs::folder(
            |d| d.set_title("Папка с альбомом"),
            move |d| Command::WishlistAttach { item: id, what: WishlistAttach::Folder(d) },
            out,
        ),
        7 => out.push(Command::WishlistRetry(id)),
        8 => out.push(Command::WishlistDismiss(vec![id])),
        _ => {}
    }
}

/// «Сохранить с YouTube» while nothing else can download it.
fn save_button(ui: &mut Ui, id: WishlistId, out: &mut Vec<Command>) {
    if ui
        .add(Button::new("Сохранить с YouTube").style(Style::Tonal).size(Size::S).id_salt(("wish-yt", id)))
        .on_hover_text("Аудио как есть (Opus ~160 кбит/с, качество пониже); найденное потом заменит его")
        .clicked()
    {
        out.push(Command::WishlistSaveFromYoutube(id));
    }
}

/// A thin bar of the item's download: filled as far as its bytes (the
/// percent its line says), none while it waits in a peer's queue, a moving
/// piece while how much is not known yet.
fn progress(ui: &mut Ui, t: Transfer) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(96.0, 24.0), Sense::hover());
    if matches!(t.phase, TransferPhase::Queued(_)) {
        return;
    }
    let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    paint::container(ui.painter(), bar, Radii::all(shape::FULL), s.secondary_container, Stroke::NONE);
    match t.percent() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::WishlistOrigin;

    fn row(n: u128, status: St, scope: WishlistScope) -> WishlistRow {
        WishlistRow {
            id: WishlistId(nikgob_core::model::Uuid::from_u128(n)),
            title: format!("t{n}"),
            artist: "a".into(),
            album: None,
            track: None,
            release: None,
            artist_id: None,
            status,
            scope,
            origin: WishlistOrigin::Like,
            note: None,
            next_try: None,
            attempts: 0,
            can_save_from_youtube: false,
            cover: None,
            transfer: None,
            confirm_files: false,
            missing: 0,
            has_copy: false,
            source: None,
            upgrade: None,
        }
    }

    /// «Все» is the sum of the chips of the statuses on their way, whatever
    /// is a track and whatever an album: one count, no second number.
    #[test]
    fn the_chips_add_up_to_all() {
        let items = vec![
            row(1, St::Downloading, WishlistScope::Track),
            row(2, St::Downloading, WishlistScope::Track),
            row(3, St::Downloading, WishlistScope::Track),
            row(4, St::Searching, WishlistScope::Track),
            row(5, St::New, WishlistScope::Track),
            row(6, St::Searching, WishlistScope::Album),
            row(7, St::Waiting, WishlistScope::Track),
            row(8, St::Done, WishlistScope::Track),
            row(9, St::Dismissed, WishlistScope::Track),
            row(10, St::NoTorrent, WishlistScope::Track),
            row(11, St::Confirm, WishlistScope::Album),
        ];
        for torrents in [true, false] {
            let c = Counts::of(&items, torrents);
            let parts = [Filter::Waiting, Filter::Searching, Filter::Downloading, Filter::Confirm, Filter::NoTorrent];
            assert_eq!(parts.iter().map(|f| c.get(*f)).sum::<usize>(), c.get(Filter::All), "torrents: {torrents}");
            assert_eq!(c.get(Filter::All), 9);
            assert_eq!((c.get(Filter::Done), c.get(Filter::Dismissed)), (1, 1));
        }
        let c = Counts::of(&items, true);
        assert_eq!((c.get(Filter::Downloading), c.get(Filter::Searching), c.get(Filter::Waiting)), (3, 3, 1));
        let c = Counts::of(&items, false);
        assert_eq!((c.get(Filter::Searching), c.get(Filter::Waiting)), (0, 4), "without a source nothing searches");
    }

    /// A downloading item says what its transfer says: the same percent as
    /// «Активные», a peer's queue as a queue.
    #[test]
    fn a_downloading_line_is_its_transfer() {
        let mut r = row(1, St::Downloading, WishlistScope::Track);
        r.source = Some("mugilaw".into());
        assert_eq!(status_text(&r, true), "качается · mugilaw");
        r.transfer = Some(Transfer { phase: TransferPhase::Going, done: 37, total: Some(100), bps: 0 });
        assert_eq!(status_text(&r, true), "качается · mugilaw · 37%");
        r.transfer = Some(Transfer { phase: TransferPhase::Queued(Some(3)), ..Transfer::default() });
        assert_eq!(status_text(&r, true), "в очереди у mugilaw · 3-й");
    }
}
