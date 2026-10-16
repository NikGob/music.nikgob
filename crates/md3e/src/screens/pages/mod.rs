//! Content pages and the menus and dialogs they open.

mod detail;
mod downloads;
mod home;
mod library;
mod search;
mod settings;
pub mod widgets;

pub use search::search_results;

use egui::{Context, Rect, Response, Ui, UiBuilder, vec2};
use nikgob_view::{
    Command, LIKED_PLAYLIST, LibraryTab, Route, TorrentFiles, TrackId, TrackRow, ViewState,
};

use super::PaneTab;
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, DialogAction, FabMenu, FabMenuItem, LoadingIndicator, Menu, MenuItem, Radii,
        SearchBar, Size, SnackbarHost, SnackbarItem, Style, TextInputDialog,
        dialog::TextInputResult,
    },
    tokens::shape,
};

/// Which text-input dialog is open.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    NewPlaylist { then_add: Vec<TrackId> },
    RenamePlaylist(nikgob_view::PlaylistId),
    Magnet,
}

/// Page-level presentation state that outlives a frame.
#[derive(Default)]
pub struct PageState {
    /// A navigation requested from inside an overlay or menu.
    pub pending_nav: Option<Route>,
    /// One-shot scroll to the lyrics settings header.
    pub scroll_to_lyrics: bool,
    /// Latest bass level for the ambient glow (set by the host each frame).
    pub levels_bass: f32,
    /// Ask the shell to open a pane tab (e.g. "Источник и качество").
    pub open_pane: Option<PaneTab>,
    /// A page asked to go back (its ← button).
    pub go_back: bool,
    pub(crate) library_tab: LibraryTab,
    release_filter: usize,
    downloads_tab: usize,
    home_filter: usize,
    ask: Option<Ask>,
    ask_text: String,
    confirm_delete: Option<nikgob_view::PlaylistId>,
    /// Track waiting for the "Удалить из библиотеки" confirmation.
    confirm_trash: Option<(TrackId, String)>,
    /// "Очистить сейчас" confirmation on the seeding tab.
    confirm_clean: bool,
    /// Which torrent the files picker shows (result, job, file count) and the ticked files.
    files_key: Option<(Option<usize>, Option<u64>, usize)>,
    files_sel: std::collections::BTreeSet<usize>,
    /// The picker already answered; waiting for the runtime to close it.
    files_done: bool,
    indexer_url: String,
    indexer_key: String,
    acoustid_key: String,
    metadata_contact: String,
    metadata_synced: bool,
    track_window: (usize, usize),
    /// Tracks waiting for the "В плейлист" picker.
    pending_add: Option<Vec<TrackId>>,
    pending_add_anchor: Option<Rect>,
    /// (playlist name, tracks) to add once the new playlist appears.
    add_after_create: Option<(String, Vec<TrackId>)>,
}

impl PageState {
    pub fn request_lyrics_settings(&mut self) {
        self.scroll_to_lyrics = true;
        self.pending_nav = Some(Route::Settings);
    }

    /// The "Добавить" FAB menu (§2.1): the rail's FAB turns into a close button
    /// with pills below it. The rail already drew a plain FAB at `fab`; the
    /// menu takes its place so it can morph.
    pub fn add_menu(&mut self, ui: &mut Ui, fab: &Response, out: &mut Vec<Command>) {
        let s = Theme::get(ui.ctx()).scheme;
        ui.painter()
            .rect_filled(fab.rect.expand2(vec2(20.0, 20.0)), 0, s.surface);
        let extended = fab.rect.width() > 76.0;
        let items = [
            FabMenuItem::new(Icon::UploadFile, "Файлы…"),
            FabMenuItem::new(Icon::Folder, "Папку…"),
            FabMenuItem::new(Icon::AttachFile, ".torrent…"),
            FabMenuItem::new(Icon::Link, "Magnet-ссылку"),
            FabMenuItem::new(Icon::PlaylistAdd, "Новый плейлист"),
        ];
        let mut menu = FabMenu::new("add-fab-menu", Icon::Add, &items);
        if extended {
            menu = menu.label("Добавить");
        }
        let slot = Rect::from_min_size(fab.rect.min, vec2(260.0, 56.0));
        let mut cui = ui.new_child(UiBuilder::new().max_rect(slot).id_salt("add-fab-slot"));
        let picked = menu.show(&mut cui).clicked;
        match picked {
            Some(0) => {
                if let Some(files) = rfd::FileDialog::new()
                    .set_title("Добавить файлы")
                    .add_filter("Музыка", &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "alac", "aiff"])
                    .pick_files()
                {
                    out.push(Command::ImportFiles(files));
                }
            }
            Some(1) => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Добавить папку").pick_folder() {
                    out.push(Command::ImportFolder(dir));
                }
            }
            Some(2) => {
                if let Some(f) = rfd::FileDialog::new()
                    .set_title("Открыть .torrent")
                    .add_filter("Торрент", &["torrent"])
                    .pick_file()
                {
                    out.push(Command::AddTorrentFile(f));
                }
            }
            Some(3) => self.open_ask(Ask::Magnet),
            Some(4) => self.open_ask(Ask::NewPlaylist { then_add: Vec::new() }),
            _ => {}
        }
    }

    fn open_ask(&mut self, ask: Ask) {
        self.ask_text.clear();
        self.ask = Some(ask);
    }

    /// The track menu, keyed by its anchor: the ⋮ menu and the right-click
    /// menu of one row (or the same track in two lists) must not share an id,
    /// or each closes the other and both draw the same widgets.
    fn track_menu_items(anchor: egui::Id, track: &TrackRow) -> Menu {
        let mut m = Menu::new(("track-menu", anchor))
            .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
            .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
            .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
            .divider()
            .item(MenuItem::new("К альбому").icon(Icon::Album).enabled(track.release_id.is_some()))
            .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(track.artist_id.is_some()))
            .item(MenuItem::new("Источник и качество").icon(Icon::Info))
            .item(MenuItem::new("Редактировать теги…").icon(Icon::Edit).enabled(track.has_local))
            .divider()
            .item(MenuItem::new("Удалить из библиотеки").icon(Icon::Delete).enabled(track.has_local));
        m = m.align_right(true);
        m
    }

    fn track_menu_pick(
        &mut self,
        picked: Option<usize>,
        track: &TrackRow,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        match picked? {
            0 => out.push(Command::PlayNext(vec![track.id])),
            1 => out.push(Command::Enqueue(vec![track.id])),
            2 => {
                self.pending_add = Some(vec![track.id]);
            }
            3 => return track.release_id.map(Route::Release),
            4 => return track.artist_id.map(Route::Artist),
            5 => self.open_pane = Some(PaneTab::Info),
            6 => out.push(Command::OpenTagEditor(track.id)),
            7 => self.confirm_trash = Some((track.id, track.title.clone())),
            _ => {}
        }
        None
    }

    /// ⋮ on a track row.
    pub fn track_menu_attach(
        &mut self,
        anchor: &Response,
        track: &TrackRow,
        _state: &ViewState,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        let picked = Self::track_menu_items(anchor.id, track).attach(anchor);
        self.track_menu_pick(picked, track, out)
    }

    /// Right click on a track row.
    pub fn track_context_menu(
        &mut self,
        row: &Response,
        track: &TrackRow,
        _state: &ViewState,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        let picked = Self::track_menu_items(row.id.with("ctx"), track).context_menu(row);
        self.track_menu_pick(picked, track, out)
    }

    /// The ⋮ menu for the playing track (Now Playing header).
    pub fn track_menu(
        &mut self,
        _ui: &mut Ui,
        anchor: &Response,
        track: Option<&TrackRow>,
        out: &mut Vec<Command>,
    ) {
        if let Some(t) = track {
            let picked = Self::track_menu_items(anchor.id, t).attach(anchor);
            if let Some(r) = self.track_menu_pick(picked, t, out) {
                self.pending_nav = Some(r);
            }
        }
    }

    /// "В плейлист" picker anchored to a button.
    pub fn playlist_menu(
        &mut self,
        _ui: &mut Ui,
        anchor: &Response,
        tracks: Vec<TrackId>,
        state: &ViewState,
        out: &mut Vec<Command>,
    ) {
        if anchor.clicked() {
            self.pending_add = Some(tracks);
        }
        self.pending_add_anchor = Some(anchor.rect);
        let _ = (state, out);
    }

    /// The file picker of a torrent: checkboxes with sizes, all/none.
    fn files_dialog(&mut self, ctx: &Context, f: &TorrentFiles, out: &mut Vec<Command>) {
        let s = Theme::get(ctx).scheme;
        let key = (f.result, f.job, f.files.len());
        if self.files_key != Some(key) {
            self.files_key = Some(key);
            self.files_sel = f.files.iter().map(|(i, ..)| *i).collect();
        }
        let ready = !f.loading && f.error.is_none() && !f.files.is_empty();
        let chosen = |sel: &std::collections::BTreeSet<usize>| -> (usize, u64) {
            f.files
                .iter()
                .filter(|(i, ..)| sel.contains(i))
                .fold((0, 0), |(n, b), (_, _, size)| (n + 1, b + size))
        };
        let (n_sel, _) = chosen(&self.files_sel);
        let can_download = f.result.is_some() && ready && n_sel > 0;
        let mut open = true;
        let sel = &mut self.files_sel;
        let r = crate::components::Dialog::new("torrent-files", "Выбрать файлы")
            .supporting(if f.name.is_empty() { "Раздача" } else { f.name.as_str() })
            .max_width(560.0)
            .action("Отмена")
            .action_with(DialogAction::new("Скачать").enabled(can_download))
            .show_with(ctx, &mut open, |ui| {
                ui.add_space(16.0);
                if f.loading {
                    ui.horizontal(|ui| {
                        ui.add(LoadingIndicator::new().size(32.0));
                        ui.label(TypeRole::BodyMedium.style().rich("Читаю список файлов…", s.on_surface_variant));
                    });
                } else if let Some(e) = &f.error {
                    ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
                } else if f.files.is_empty() {
                    ui.label(TypeRole::BodyMedium.style().rich("В раздаче нет файлов.", s.on_surface_variant));
                } else {
                    let (n, bytes) = chosen(sel);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui.add(Button::new("Все").style(Style::Text).size(Size::Xs).id_salt("files-all")).clicked() {
                            *sel = f.files.iter().map(|(i, ..)| *i).collect();
                        }
                        if ui.add(Button::new("Ничего").style(Style::Text).size(Size::Xs).id_salt("files-none")).clicked() {
                            sel.clear();
                        }
                        ui.label(TypeRole::BodySmall.style().rich(
                            format!("Выбрано {n} из {} · {}", f.files.len(), fmt_size(bytes)),
                            s.on_surface_variant,
                        ));
                    });
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("files-scroll")
                        .max_height(280.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (i, path, size) in &f.files {
                                let mut on = sel.contains(i);
                                let text = format!("{path} · {}", fmt_size(*size));
                                if ui
                                    .checkbox(&mut on, TypeRole::BodyMedium.style().rich(text, s.on_surface))
                                    .changed()
                                {
                                    if on {
                                        sel.insert(*i);
                                    } else {
                                        sel.remove(i);
                                    }
                                }
                            }
                        });
                    if f.result.is_none() {
                        ui.add_space(8.0);
                        ui.label(TypeRole::BodySmall.style().rich_para(
                            "Выбор файлов для .torrent и magnet пока недоступен: список только для просмотра.",
                            s.on_surface_variant,
                        ));
                    }
                }
            });
        if r.action == Some(1)
            && let Some(result) = f.result
        {
            let files: Vec<usize> = self.files_sel.iter().copied().collect();
            out.push(Command::DownloadTorrent { result, files: Some(files) });
            out.push(Command::CloseTorrentFiles);
            self.files_done = true;
        } else if r.action.is_some() || r.dismissed || !open {
            out.push(Command::CloseTorrentFiles);
            self.files_done = true;
        }
    }

    /// Draws global overlays: the playlist picker, dialogs, snackbars.
    pub fn overlays(&mut self, ctx: &Context, state: &ViewState, area: Rect, out: &mut Vec<Command>) {
        // Playlist picker.
        if let Some(tracks) = self.pending_add.clone() {
            let mut open = true;
            let mut menu = Menu::new("playlist-picker")
                .item(MenuItem::new("Любимые").icon(Icon::Favorite));
            let user: Vec<_> = state.library.playlists.iter().filter(|p| !p.pinned).collect();
            for p in &user {
                menu = menu.item(MenuItem::new(&p.name).icon(Icon::QueueMusic));
            }
            menu = menu.divider().item(MenuItem::new("Новый плейлист…").icon(Icon::Add));
            let anchor = self
                .pending_add_anchor
                .unwrap_or_else(|| Rect::from_center_size(area.center(), egui::vec2(1.0, 1.0)));
            let picked = menu.popup(ctx, anchor, &mut open);
            match picked {
                Some(0) => out.push(Command::AddToPlaylist(LIKED_PLAYLIST, tracks)),
                Some(i) if i <= user.len() => {
                    out.push(Command::AddToPlaylist(user[i - 1].id, tracks));
                }
                Some(_) => self.open_ask(Ask::NewPlaylist { then_add: tracks }),
                None => {}
            }
            if picked.is_some() || !open {
                self.pending_add = None;
                self.pending_add_anchor = None;
            }
        }

        // Text input dialogs.
        if let Some(ask) = self.ask.clone() {
            let mut open = true;
            let dialog = match &ask {
                Ask::NewPlaylist { .. } => TextInputDialog::new("ask-new-playlist", "Новый плейлист", "Название")
                    .icon(Icon::PlaylistAdd)
                    .buttons("Создать", "Отмена")
                    .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned())),
                Ask::RenamePlaylist(_) => TextInputDialog::new("ask-rename", "Переименовать", "Название")
                    .buttons("Сохранить", "Отмена")
                    .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned())),
                Ask::Magnet => TextInputDialog::new("ask-magnet", "Magnet-ссылка", "magnet:?xt=…")
                    .icon(Icon::Link)
                    .supporting("Загрузка появится во вкладке «Загрузки».")
                    .buttons("Скачать", "Отмена")
                    .validate(|t| {
                        (!t.trim().starts_with("magnet:")).then(|| "Ссылка должна начинаться с magnet:".to_owned())
                    }),
            };
            match dialog.show(ctx, &mut open, &mut self.ask_text) {
                Some(TextInputResult::Confirmed) => {
                    let text = self.ask_text.trim().to_owned();
                    match ask {
                        Ask::NewPlaylist { then_add } => {
                            out.push(Command::CreatePlaylist(text.clone()));
                            if !then_add.is_empty() {
                                self.add_after_create = Some((text, then_add));
                            }
                        }
                        Ask::RenamePlaylist(id) => out.push(Command::RenamePlaylist(id, text)),
                        Ask::Magnet => out.push(Command::AddMagnet(text)),
                    }
                    self.ask = None;
                }
                Some(TextInputResult::Cancelled) => self.ask = None,
                None => {
                    if !open {
                        self.ask = None;
                    }
                }
            }
        }
        // Tracks waiting for a playlist we just created.
        if let Some((name, tracks)) = self.add_after_create.clone()
            && let Some(p) = state.library.playlists.iter().find(|p| p.name == name && !p.pinned)
        {
            out.push(Command::AddToPlaylist(p.id, tracks));
            self.add_after_create = None;
        }

        // Delete confirmation.
        if let Some(id) = self.confirm_delete {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-delete", "Удалить плейлист?")
                .icon(Icon::Delete)
                .supporting("Треки останутся в библиотеке.")
                .action("Отмена")
                .action("Удалить")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::DeletePlaylist(id));
                self.pending_nav = Some(Route::Library(LibraryTab::Playlists));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_delete = None;
            }
        }

        // Track deletion (to Trash, 30 days).
        if let Some((id, title)) = self.confirm_trash.clone() {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-trash", "Удалить из библиотеки?")
                .icon(Icon::Delete)
                .supporting(format!(
                    "«{title}» переедет в Корзину на 30 дней. Вернуть можно кнопкой «Отменить» в уведомлении."
                ))
                .action("Отмена")
                .action("Удалить")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::TrashTrack(id));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_trash = None;
            }
        }

        // Seeding cleanup.
        if self.confirm_clean {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-clean", "Очистить раздачу?")
                .icon(Icon::Delete)
                .supporting("Завершённые торренты перестанут раздаваться, их файлы будут удалены. Музыка в библиотеке останется.")
                .action("Отмена")
                .action("Очистить")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::CleanSeeding);
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_clean = false;
            }
        }

        // Files of a torrent ("Выбрать файлы…").
        match &state.downloads.files {
            Some(f) if !self.files_done => self.files_dialog(ctx, f, out),
            Some(_) => {}
            None => self.files_done = false,
        }

        // Snackbars.
        let items: Vec<SnackbarItem<'_>> = state
            .toasts
            .iter()
            .map(|t| {
                let i = SnackbarItem::new(t.id, &t.text);
                match &t.action {
                    Some((label, _)) => i.action(label),
                    None => i,
                }
            })
            .collect();
        let ev = SnackbarHost::new("toasts").show(ctx, area, 64.0 + 16.0 + 16.0, &items);
        if let Some(id) = ev.action_clicked
            && let Some(t) = state.toasts.iter().find(|t| t.id == id)
            && let Some((_, cmd)) = &t.action
        {
            out.push(cmd.clone());
        }
        for id in ev.dismissed {
            out.push(Command::DismissToast(id));
        }
    }
}

pub fn search_bar(ui: &mut Ui, rect: Rect, query: &mut String, focus: &mut bool, out: &mut Vec<Command>) {
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("search-bar"));
    let before = query.clone();
    let o = SearchBar::new("main-search")
        .placeholder("Поиск: библиотека, Spotify, YouTube…")
        .width(rect.width())
        .shortcut(true)
        .show(&mut cui, query);
    if std::mem::take(focus) {
        o.response.request_focus();
    }
    if *query != before {
        out.push(Command::Search(query.clone()));
    }
    // Enter looks for torrents: Prowlarr is slow, so never on typing.
    if o.submitted && !query.trim().is_empty() {
        out.push(Command::SearchTorrents(query.trim().to_owned()));
        search::set_filter(ui.ctx(), search::Filter::Torrents);
    }
}

/// The page for `route`, inside a vertical scroll area that leaves room
/// for the floating player. Returns a route to navigate to.
pub fn page(
    ui: &mut Ui,
    route: &Route,
    state: &ViewState,
    bottom_inset: f32,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let mut nav = None;
    let id = format!("{route:?}");
    egui::ScrollArea::vertical()
        .id_salt(("page-scroll", id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let r = match route {
                Route::Home => home::show(ui, state, pages, out),
                Route::Library(_) => library::show(ui, state, pages, out),
                Route::Release(id) => detail::release(ui, *id, state, pages, out),
                Route::Artist(id) => detail::artist(ui, *id, state, pages, out),
                Route::Playlist(id) => detail::playlist(ui, *id, state, pages, out),
                Route::Downloads(_) => downloads::show(ui, state, pages, out),
                Route::Settings => settings::show(ui, state, pages, out),
            };
            nav = r.or(nav.take());
            ui.add_space(bottom_inset);
        });
    nav
}

/// Human size: "412 МБ", "1.1 ГБ".
pub(super) fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{bytes} Б"),
        _ if v >= 100.0 => format!("{v:.0} {}", UNITS[i]),
        _ => format!("{v:.1} {}", UNITS[i]),
    }
}

/// A connected button group (single select) that can disable members and
/// reports every member's rect (for badges). Returns the newly picked index.
pub(super) fn connected(
    ui: &mut Ui,
    id: egui::Id,
    labels: &[&str],
    selected: usize,
    disabled: &[usize],
) -> (Option<usize>, Vec<Rect>) {
    let n = labels.len();
    let inner = Size::S.connected_inner();
    let mut picked = None;
    let mut rects = Vec::with_capacity(n);
    // One button tall, so the group sits on its parent's centre line.
    let row = egui::vec2(ui.available_width(), Size::S.height());
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, label) in labels.iter().enumerate() {
            let sel = i == selected;
            let off = disabled.contains(&i);
            let l = if i == 0 || sel { shape::FULL } else { inner };
            let r = if i + 1 == n || sel { shape::FULL } else { inner };
            let b = Button::new(*label)
                .size(Size::S)
                .style(Style::Tonal)
                .selected(sel)
                .enabled(!off)
                .radii(
                    Radii::sides(l, r),
                    Radii::sides(
                        if i == 0 { shape::FULL } else { inner * 0.5 },
                        if i + 1 == n { shape::FULL } else { inner * 0.5 },
                    ),
                )
                .id_salt((id, i));
            let resp = ui.add(b);
            rects.push(resp.rect);
            if resp.clicked() && !sel && !off {
                picked = Some(i);
            }
        }
    });
    (picked, rects)
}
