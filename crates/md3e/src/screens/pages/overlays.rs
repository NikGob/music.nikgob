//! The overlays every page shares: the playlist picker, the text-input
//! dialogs, the delete confirmations, the files of a torrent and the
//! snackbars ([`PageState::overlays`]).

use egui::{Context, Rect};
use nikgob_core::i18n;
use nikgob_view::{Command, LIKED_PLAYLIST, LibraryTab, Route, TorrentFiles, TrackId};

use super::{
    Ask, Doomed, PageState, StreamAdd, apart_request_key, delete_request_key, dialogs, fmt_size, picker_request_key, sound, widgets,
};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, DialogAction, LoadingIndicator, Menu, MenuItem, Size, SnackbarHost, SnackbarItem, Style, TextInputDialog,
        dialog::TextInputResult,
    },
};

impl PageState {
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
        let r = crate::components::Dialog::new("torrent-files", i18n::search().pick_files_title)
            .supporting(if f.name.is_empty() { i18n::search().torrent_word } else { f.name.as_str() })
            .max_width(560.0)
            .action(i18n::common().cancel)
            .action_with(DialogAction::new(i18n::menus().download).enabled(can_download))
            .show_with(ctx, &mut open, |ui| {
                ui.add_space(16.0);
                if f.loading {
                    ui.horizontal(|ui| {
                        ui.add(LoadingIndicator::new().size(32.0));
                        ui.label(TypeRole::BodyMedium.style().rich(i18n::search().reading_files, s.on_surface_variant));
                    });
                } else if let Some(e) = &f.error {
                    ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
                } else if f.files.is_empty() {
                    ui.label(TypeRole::BodyMedium.style().rich(i18n::search().no_files, s.on_surface_variant));
                } else {
                    let (n, bytes) = chosen(sel);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui.add(Button::new(i18n::common().all).style(Style::Text).size(Size::Xs).id_salt("files-all")).clicked() {
                            *sel = f.files.iter().map(|(i, ..)| *i).collect();
                        }
                        if ui.add(Button::new(i18n::search().select_none).style(Style::Text).size(Size::Xs).id_salt("files-none")).clicked() {
                            sel.clear();
                        }
                        ui.label(TypeRole::BodySmall.style().rich(
                            i18n::search::fmt_selected_files(n, f.files.len(), fmt_size(bytes)),
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
                            i18n::search().files_readonly_note,
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
    /// `snack`: where the snackbars sit, `(area, bottom inset)`.
    pub fn overlays(&mut self, ctx: &Context, state: &ViewState, area: Rect, snack: (Rect, f32), out: &mut Vec<Command>) {
        dialogs::drain(ctx, out);
        sound::overlay(ctx, state, out);
        crate::screens::info_window::overlay(ctx, state, out);
        // Playlist picker (for the player's and the queue's menus too).
        if let Some((tracks, anchor)) = ctx.data_mut(|d| d.remove_temp::<(Vec<TrackId>, Option<Rect>)>(picker_request_key())) {
            self.pending_add = Some(tracks);
            self.pending_add_anchor = anchor;
        }
        if let Some(tracks) = self.pending_add.clone() {
            let mut open = true;
            let mut menu = Menu::new("playlist-picker")
                .item(MenuItem::new(i18n::pages().liked_short).icon(Icon::Favorite));
            let user: Vec<_> = state.library.playlists.iter().filter(|p| !p.pinned).collect();
            for p in &user {
                menu = menu.item(MenuItem::new(&p.name).icon(Icon::QueueMusic));
            }
            menu = menu.divider().item(MenuItem::new(i18n::pages().new_playlist_dots).icon(Icon::Add));
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


        // The same picker for stream tracks or an album. «Любимые» only for
        // one stream track (a like of each of many would also download
        // each) or a library album (its tracks are here already).
        if let Some((album, anchor)) =
            ctx.data_mut(|d| d.remove_temp::<Option<(nikgob_view::RemoteId, Option<Rect>)>>(egui::Id::new("stream-album-playlist-request"))).flatten()
        {
            self.pending_add_stream = Some(StreamAdd::Album(album));
            self.pending_add_anchor = anchor;
        }
        if let Some((album, anchor)) =
            ctx.data_mut(|d| d.remove_temp::<Option<(nikgob_view::ReleaseId, Option<Rect>)>>(egui::Id::new("release-playlist-request"))).flatten()
        {
            self.pending_add_stream = Some(StreamAdd::Release(album));
            self.pending_add_anchor = anchor;
        }
        if let Some(add) = self.pending_add_stream.clone() {
            let mut open = true;
            let liked = matches!(&add, StreamAdd::Tracks(t) if t.len() == 1) || matches!(add, StreamAdd::Release(_));
            let mut menu = Menu::new("playlist-picker-stream");
            if liked {
                menu = menu.item(MenuItem::new(i18n::pages().liked_short).icon(Icon::Favorite));
            }
            let user: Vec<_> = state.library.playlists.iter().filter(|p| !p.pinned).collect();
            for p in &user {
                menu = menu.item(MenuItem::new(&p.name).icon(Icon::QueueMusic));
            }
            menu = menu.divider().item(MenuItem::new(i18n::pages().new_playlist_dots).icon(Icon::Add));
            let anchor = self
                .pending_add_anchor
                .unwrap_or_else(|| Rect::from_center_size(area.center(), egui::vec2(1.0, 1.0)));
            let picked = menu.popup(ctx, anchor, &mut open).map(|i| if liked { i } else { i + 1 });
            match (picked, add) {
                (Some(0), StreamAdd::Tracks(tracks)) => {
                    out.extend(tracks.into_iter().map(|track| Command::LikeStream { track, on: true }))
                }
                (Some(0), StreamAdd::Release(album)) => out.push(Command::AddAlbumToPlaylist(LIKED_PLAYLIST, album)),
                (Some(0), StreamAdd::Album(_)) => {}
                (Some(i), add) if i <= user.len() => out.push(add.into_playlist(user[i - 1].id)),
                (Some(_), add) => self.open_ask(Ask::NewPlaylistStream(add)),
                (None, _) => {}
            }
            if picked.is_some() || !open {
                self.pending_add_stream = None;
                self.pending_add_anchor = None;
            }
        }

        // Text input dialogs.
        if let Some(ask) = self.ask.clone() {
            let mut open = true;
            let dialog = match &ask {
                Ask::NewPlaylist { .. } | Ask::NewPlaylistStream(_) => {
                    TextInputDialog::new("ask-new-playlist", i18n::pages().ask_title_new_playlist, i18n::pages().ask_name_label)
                        .icon(Icon::PlaylistAdd)
                        .buttons(i18n::pages().create_btn, i18n::common().cancel)
                        .validate(|t| t.trim().is_empty().then(|| i18n::pages().enter_name.to_owned()))
                }
                Ask::RenamePlaylist(_) => {
                    TextInputDialog::new("ask-rename", i18n::pages().ask_title_rename, i18n::pages().ask_name_label)
                        .buttons(i18n::common().save, i18n::common().cancel)
                        .validate(|t| t.trim().is_empty().then(|| i18n::pages().enter_name.to_owned()))
                }
                Ask::WishMagnet(_) => {
                    TextInputDialog::new("ask-wish-magnet", i18n::pages().magnet_title, i18n::pages().magnet_placeholder)
                        .icon(Icon::Link)
                        .supporting(i18n::pages().magnet_as_track)
                        .buttons(i18n::menus().download, i18n::common().cancel)
                        .validate(|t| {
                            (!t.trim().starts_with("magnet:")).then(|| i18n::pages().magnet_must_start.to_owned())
                        })
                }
                Ask::Magnet => {
                    TextInputDialog::new("ask-magnet", i18n::pages().magnet_title, i18n::pages().magnet_placeholder)
                        .icon(Icon::Link)
                        .buttons(i18n::menus().download, i18n::common().cancel)
                        .validate(|t| {
                            (!t.trim().starts_with("magnet:")).then(|| i18n::pages().magnet_must_start.to_owned())
                        })
                }
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
                        Ask::NewPlaylistStream(add) => {
                            out.push(Command::CreatePlaylist(text.clone()));
                            self.stream_after_create = Some((text, add));
                        }
                        Ask::WishMagnet(item) => out.push(Command::WishlistAttach {
                            item,
                            what: nikgob_view::WishlistAttach::Magnet(text),
                        }),
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
        if let Some((name, add)) = self.stream_after_create.clone()
            && let Some(p) = state.library.playlists.iter().find(|p| p.name == name && !p.pinned)
        {
            out.push(add.into_playlist(p.id));
            self.stream_after_create = None;
        }

        // Delete confirmation.
        if let Some(id) = self.confirm_delete {
            let mut open = true;
            // A playlist that follows a service: that one stays as it is.
            let service = state
                .playlist
                .as_ref()
                .filter(|p| p.card.id == id)
                .and_then(|p| p.sync.as_ref())
                .map(|l| l.remote.service)
                // From a card's menu: what the card knows.
                .or_else(|| state.library.playlists.iter().find(|p| p.id == id).and_then(|p| p.service))
                .map(|s| s.display_name());
            let text = match service {
                Some(s) => i18n::pages::fill_service(i18n::pages().playlist_delete_kept, s),
                None => i18n::pages().playlist_delete_plain.to_owned(),
            };
            let r = crate::components::Dialog::new("confirm-delete", i18n::pages().delete_playlist_q)
                .icon(Icon::Delete)
                .supporting(text)
                .action(i18n::common().cancel)
                .action(i18n::pages().delete_btn)
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::DeletePlaylist(id));
                self.pending_nav = Some(Route::Library(LibraryTab::Playlists));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_delete = None;
            }
        }

        // «Связать с сервисом…»: a playlist is made in the service at once.
        if let Some(id) = self.confirm_link {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-link", i18n::pages().link_service_q)
                .icon(Icon::Sync)
                .supporting(i18n::pages().link_service_body)
                .action(i18n::common().cancel)
                .action(i18n::pages().link_btn)
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(widgets::playlist_link_command(id));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_link = None;
            }
        }
        // A deletion asked from a menu without the pages at hand (a card's,
        // the player's).
        if let Some((what, title, forever)) = ctx.data_mut(|d| d.remove_temp::<(Doomed, String, bool)>(delete_request_key())) {
            if forever {
                self.confirm_purge = Some((what, title));
            } else {
                self.confirm_trash = Some((what, title));
            }
        }
        // «Разные песни»: for good, so asked.
        if let Some(apart) = ctx.data_mut(|d| d.remove_temp::<(TrackId, String, String)>(apart_request_key())) {
            self.confirm_apart = Some(apart);
        }
        if let Some((track, title, album)) = self.confirm_apart.clone() {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-apart", i18n::pages().apart_q)
                .icon(Icon::Close)
                .supporting(i18n::menus::fmt_apart_body(&title, &album))
                .action(i18n::common().cancel)
                .action(i18n::menus().apart)
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::RecordingApart(track));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_apart = None;
            }
        }
        // Deletion: into the Trash (30 days), or for good when it is off.
        if let Some((what, title)) = self.confirm_trash.clone() {
            let mut open = true;
            let trash = state.prefs.use_trash;
            let r = crate::components::Dialog::new(
                "confirm-trash",
                if trash { i18n::pages().trash_q } else { i18n::pages().purge_q },
            )
            .icon(Icon::Delete)
            .supporting(if trash {
                i18n::pages::fill(i18n::pages().trash_note_30, &title)
            } else {
                i18n::pages::fill(i18n::pages().trash_note_off, &title)
            })
            .action(i18n::common().cancel)
            .action(if trash { i18n::pages().delete_btn } else { i18n::pages().purge_forever_field })
            .show(ctx, &mut open);
            if r.action == Some(1) {
                match what {
                    Doomed::Tracks(ids) => out.extend(ids.into_iter().map(Command::TrashTrack)),
                    Doomed::Release(id) => out.push(Command::TrashRelease(id)),
                }
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_trash = None;
            }
        }
        if let Some((what, title)) = self.confirm_purge.clone() {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-purge", i18n::pages().purge_q)
                .icon(Icon::Delete)
                .supporting(i18n::pages::fill(i18n::pages().purge_note, &title))
                .action(i18n::common().cancel)
                .action(i18n::pages().purge_forever_field)
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(match what {
                    Doomed::Tracks(ids) => Command::DeleteForever(ids),
                    Doomed::Release(id) => Command::DeleteReleaseForever(id),
                });
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_purge = None;
            }
        }
        if self.confirm_empty_trash {
            let mut open = true;
            let n = state.library.trash.len() as u64;
            let r = crate::components::Dialog::new("confirm-empty-trash", i18n::pages().empty_trash_q)
                .icon(Icon::Delete)
                .supporting(i18n::pages::fill_count(i18n::pages().empty_trash_forever, &i18n::tracks_count(n)))
                .action(i18n::common().cancel)
                .action(i18n::pages().clear_trash_btn)
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::EmptyTrash);
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_empty_trash = false;
            }
        }

        // Seeding cleanup.
        if self.confirm_clean {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-clean", i18n::pages().clean_seeding_q)
                .icon(Icon::Delete)
                .supporting(i18n::pages().clean_seeding_body)
                .action(i18n::common().cancel)
                .action(i18n::pages().clear_trash_btn)
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

        // A row's «Хочу скачать» mark was clicked.
        if ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(widgets::OPEN_WISHLIST))).unwrap_or(false) {
            self.pending_nav = Some(Route::Downloads(nikgob_view::DownloadsTab::Wishlist));
        }

        // Snackbars: «Уведомления на экране» seconds (0 = until closed).
        let duration = match state.prefs.toast_s {
            0 => f64::INFINITY,
            s => f64::from(s),
        };
        let items: Vec<SnackbarItem<'_>> = state
            .toasts
            .iter()
            .map(|t| {
                let i = SnackbarItem::new(t.id, &t.text).kind(match t.kind {
                    nikgob_view::ToastKind::Info => crate::components::SnackbarKind::Info,
                    nikgob_view::ToastKind::Success => crate::components::SnackbarKind::Success,
                    nikgob_view::ToastKind::Working => crate::components::SnackbarKind::Working,
                    nikgob_view::ToastKind::Error => crate::components::SnackbarKind::Error,
                });
                let i = if t.long { i.longer(duration) } else { i }.again(t.again);
                match (&t.action, &t.open) {
                    (Some((label, _)), _) | (None, Some((label, _))) => i.action(label),
                    (None, None) => i,
                }
            })
            .collect();
        // One at a time (M3): a new one replaces the one up, an undo stays.
        let ev = SnackbarHost::new("toasts")
            .replacing(true)
            .max_visible(1)
            .show_close(true)
            .duration(duration)
            .show(ctx, snack.0, snack.1, &items);
        if let Some(id) = ev.action_clicked
            && let Some(t) = state.toasts.iter().find(|t| t.id == id)
        {
            match (&t.action, &t.open) {
                (Some((_, cmd)), _) => out.push(cmd.clone()),
                (None, Some((_, link))) => self.follow(link.clone()),
                (None, None) => {}
            }
        }
        for id in ev.dismissed {
            out.push(Command::DismissToast(id));
        }
    }
}
