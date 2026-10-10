//! Альбом / Исполнитель / Плейлист (md3e-player.md §3.3–3.4).

use std::path::PathBuf;

use egui::{Context, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    ArtistId, Command, Cover, LIKED_PLAYLIST, LikeTarget, PlaylistDetail, PlaylistId, ReleaseId,
    ReleaseType, Route, TrackId, ViewState,
};

use super::{PageState, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        AssistChip, Button, ButtonRow, Dialog, DialogAction, FlexibleAppBar, IconButton, ListItem,
        LoadingIndicator, Member, Menu, MenuItem, Size, SplitButton, Style, TextField, app_bar,
    },
    screens::common,
    tokens::shape,
};
use widgets::{Lead, PlayFrom, TrackListOpts};

fn type_label(t: ReleaseType) -> &'static str {
    match t {
        ReleaseType::Album => "АЛЬБОМ",
        ReleaseType::Single => "СИНГЛ",
        ReleaseType::Ep => "EP",
        ReleaseType::Compilation => "СБОРНИК",
        _ => "РЕЛИЗ",
    }
}

/// Runs `body` under a large flexible app bar pinned to the top of the page
/// viewport: it collapses to the small 64 bar as the page scrolls, its
/// container turns `surface-container`, and ← goes back.
fn with_bar<R>(
    ui: &mut Ui,
    title: &str,
    pages: &mut PageState,
    body: impl FnOnce(&mut Ui, &mut PageState) -> R,
) -> R {
    let left = ui.cursor().left();
    let width = ui.available_width();
    let scroll = (ui.clip_rect().top() - ui.cursor().top()).max(0.0);
    ui.add_space(app_bar::LARGE_HEIGHT);
    let r = body(ui, pages);
    // The bar is drawn last so scrolled content passes underneath it.
    let m = app_bar::flexible_metrics(scroll, false, true);
    let rect = Rect::from_min_size(pos2(left, ui.clip_rect().top()), vec2(width, m.height));
    ui.interact(rect, ui.id().with("bar-block"), Sense::click());
    let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("detail-bar-ui"));
    let o = FlexibleAppBar::new(title)
        .leading(Icon::ArrowBack, "Назад (Esc)")
        .id_salt("detail-bar")
        .show(&mut bar_ui, scroll, |_| {});
    if o.leading_clicked {
        pages.go_back = true;
    }
    r
}

/// Header: big cover (radius 28, or a shape for artists) and the text
/// column to its right.
fn header(
    ui: &mut Ui,
    size: f32,
    cover: Option<&Cover>,
    shape: Option<(&str, nikgob_shapes::MaterialShape)>,
    add_text: impl FnOnce(&mut Ui),
) {
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        match shape {
            Some((initials, sh)) => widgets::shape_image(ui, cover.map(|c| c.uri()).as_deref(), initials, rect, sh),
            None => common::cover(ui, cover, rect, shape::EXTRA_LARGE),
        }
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.set_min_height(size);
            add_text(ui);
        });
    });
}

/// 232 beside wide pages, 160 otherwise.
fn cover_size(ui: &Ui) -> f32 {
    if ui.available_width() > 720.0 { 232.0 } else { 160.0 }
}

pub fn release(
    ui: &mut Ui,
    id: ReleaseId,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let Some(r) = state.release.as_ref().filter(|r| r.card.id == id) else {
        return with_bar(ui, "", pages, |ui, _| {
            ui.add(LoadingIndicator::new());
            None
        });
    };
    let c = &r.card;
    with_bar(ui, &c.title, pages, |ui, pages| {
        let mut nav = None;
        let size = cover_size(ui);
        header(ui, size, c.cover.as_ref(), None, |ui| {
            let line = match c.year {
                Some(y) => format!("{} · {y}", type_label(c.release_type)),
                None => type_label(c.release_type).to_owned(),
            };
            ui.label(TypeRole::LabelLarge.style().rich(line, s.primary));
            let n = r.tracks.len() as u64;
            let artist = ui.add(
                egui::Label::new(TypeRole::TitleMedium.style().rich(
                    format!(
                        "{} · {n} {} · {}",
                        c.artist,
                        common::plural(n, "трек", "трека", "треков"),
                        common::fmt_total(r.total_duration)
                    ),
                    s.on_surface_variant,
                ))
                .sense(Sense::click()),
            );
            if artist.clicked()
                && let Some(a) = c.artist_id
            {
                nav = Some(Route::Artist(a));
            }
            artist.on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if let Some(q) = &r.quality {
                    ui.add(AssistChip::new(q).icon(Icon::GraphicEq).id_salt("rel-q"));
                }
                if let Some(b) = widgets::progress_badge(c) {
                    ui.add(AssistChip::new(format!("{b} у тебя")).icon(Icon::Download).id_salt("rel-have"));
                }
            });
        });
        ui.add_space(16.0);

        // Actions: standard button group, "Слушать" is the only filled button.
        let missing = c.total_tracks.is_some_and(|t| c.local_tracks < t);
        let query = format!("{} {}", c.artist, c.title);
        let mut members: Vec<Member> = vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Tonal).into(),
        ];
        if missing {
            members.push(
                SplitButton::new("rel-download", "Скачать недостающие")
                    .icon(Icon::Download)
                    .size(Size::M)
                    .style(Style::Tonal)
                    .item(MenuItem::new("Все недостающие").icon(Icon::Download))
                    .item(MenuItem::new("Выбрать треки…").icon(Icon::Check))
                    .item(MenuItem::new("Заменить всё на лучшее качество").icon(Icon::GraphicEq))
                    .into(),
            );
        }
        members.push(
            IconButton::new(Icon::Favorite).size(Size::M).style(Style::Tonal).selected(r.liked).tooltip("Нравится").into(),
        );
        members.push(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into());
        let row = ButtonRow::new("rel-actions", Size::M).show(ui, members);
        let resp = &row.responses;
        let off = usize::from(missing);
        if resp[0].clicked() {
            out.push(Command::PlayRelease { id, start: None, shuffle: false });
        }
        if resp[1].clicked() {
            out.push(Command::PlayRelease { id, start: None, shuffle: true });
        }
        // The shell turns this into the torrent search view.
        if (missing && resp[2].clicked()) || row.picked.is_some() {
            out.push(Command::SearchTorrents(query));
        }
        if resp[2 + off].clicked() {
            out.push(Command::Like(LikeTarget::Release(id), !r.liked));
        }
        let ids: Vec<_> = r.tracks.iter().map(|t| t.id).collect();
        let more = &resp[3 + off];
        let picked = Menu::new("rel-more")
            .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
            .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
            .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
            .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(c.artist_id.is_some()))
            .attach(more);
        match picked {
            Some(0) => out.push(Command::PlayNext(ids.clone())),
            Some(1) => out.push(Command::Enqueue(ids.clone())),
            Some(2) => {
                pages.pending_add = Some(ids.clone());
                pages.pending_add_anchor = Some(more.rect);
            }
            Some(3) => nav = c.artist_id.map(Route::Artist),
            _ => {}
        }
        ui.add_space(16.0);
        widgets::track_list(ui, "release", &r.tracks, Lead::Number, &PlayFrom::Release(id), state, pages, out)
            .or(nav)
    })
}

pub fn artist(
    ui: &mut Ui,
    id: ArtistId,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let Some(a) = state.artist.as_ref().filter(|a| a.card.id == id) else {
        return with_bar(ui, "", pages, |ui, _| {
            ui.add(LoadingIndicator::new());
            None
        });
    };
    with_bar(ui, &a.card.name, pages, |ui, pages| {
        let mut nav = None;
        header(
            ui,
            160.0,
            a.card.image.as_ref(),
            Some((&widgets::initials(&a.card.name), nikgob_shapes::MaterialShape::Cookie12Sided)),
            |ui| {
                ui.label(TypeRole::LabelLarge.style().rich("ИСПОЛНИТЕЛЬ", s.primary));
                let n = (a.albums.len() + a.singles_eps.len()) as u64;
                ui.label(TypeRole::TitleMedium.style().rich(
                    format!("{n} {} в библиотеке", common::plural(n, "релиз", "релиза", "релизов")),
                    s.on_surface_variant,
                ));
            },
        );
        ui.add_space(16.0);
        let row = ButtonRow::new("artist-actions", Size::M).show(
            ui,
            vec![
                Button::new("Микс исполнителя").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
                IconButton::new(Icon::Shuffle).size(Size::M).style(Style::Tonal).tooltip("Перемешать").into(),
                Button::new(if a.liked { "Вы подписаны" } else { "Подписаться" })
                    .icon(Icon::Favorite)
                    .size(Size::M)
                    .style(Style::Outlined)
                    .selected(a.liked)
                    .into(),
                IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into(),
            ],
        );
        let r = &row.responses;
        if r[0].clicked() {
            out.push(Command::PlayArtist { id, shuffle: false });
        }
        if r[1].clicked() {
            out.push(Command::PlayArtist { id, shuffle: true });
        }
        if r[2].clicked() {
            out.push(Command::Like(LikeTarget::Artist(id), !a.liked));
        }
        // "В очередь всё" queues what the artist page knows: the popular tracks.
        let picked = Menu::new("artist-more")
            .item(MenuItem::new("В очередь всё").icon(Icon::QueueMusic).enabled(!a.top_tracks.is_empty()))
            .item(MenuItem::new(if a.liked { "Отписаться" } else { "Подписаться" }).icon(Icon::Favorite))
            .attach(&r[3]);
        match picked {
            Some(0) => out.push(Command::EnqueueArtist(a.card.id)),
            Some(1) => out.push(Command::Like(LikeTarget::Artist(id), !a.liked)),
            _ => {}
        }
        ui.add_space(widgets::SECTION_GAP);
        if !a.top_tracks.is_empty() {
            widgets::section_header(ui, "Популярное у тебя", false);
            let ids = a.top_tracks.iter().map(|t| t.id).collect();
            nav = widgets::track_list(
                ui,
                "artist-top",
                &a.top_tracks,
                Lead::Cover,
                &PlayFrom::Tracks(ids, a.card.name.clone()),
                state,
                pages,
                out,
            )
            .or(nav);
            ui.add_space(widgets::SECTION_GAP);
        }
        for (title, list) in [("Альбомы", &a.albums), ("Синглы и EP", &a.singles_eps)] {
            if list.is_empty() {
                continue;
            }
            widgets::section_header(ui, title, false);
            widgets::card_row(ui, title, |ui| {
                for r in list.iter() {
                    nav = widgets::release_card(ui, r, widgets::CARD_W, title, out).or(nav.take());
                }
            });
            ui.add_space(widgets::SECTION_GAP);
        }
        widgets::section_header(ui, "Нет в библиотеке", false);
        ui.label(TypeRole::BodyMedium.style().rich(
            "Дискография из MusicBrainz со стримингом и загрузкой появится на этапах 2–3.",
            s.on_surface_variant,
        ));
        nav
    })
}

/// State of the playlist edit dialog.
#[derive(Clone, Default)]
struct EditState {
    open: bool,
    shown: bool,
    name: String,
    cover: Option<PathBuf>,
}

/// State of the "Добавить треки" dialog.
#[derive(Clone, Default)]
struct AddState {
    open: bool,
    shown: bool,
    query: String,
    added: Vec<TrackId>,
}

fn get<T: Clone + Default + Send + Sync + 'static>(ctx: &Context, key: Id) -> T {
    ctx.data(|d| d.get_temp::<T>(key)).unwrap_or_default()
}

fn put<T: Clone + Send + Sync + 'static>(ctx: &Context, key: Id, v: T) {
    ctx.data_mut(|d| d.insert_temp(key, v));
}

/// Name and cover of a playlist.
fn edit_dialog(ctx: &Context, p: &PlaylistDetail, out: &mut Vec<Command>) {
    let key = Id::new(("pl-edit", p.card.id));
    let st: EditState = get(ctx, key);
    let s = Theme::get(ctx).scheme;
    let (mut open, mut shown, mut name, mut cover) = (st.open, st.shown, st.name, st.cover);
    let valid = !name.trim().is_empty();
    let mut submitted = false;
    let current = p.card.cover.clone();
    let res = Dialog::new(("pl-edit-dialog", p.card.id), "Изменить плейлист")
        .action("Отмена")
        .action_with(DialogAction::new("Сохранить").enabled(valid))
        .show_with(ctx, &mut open, |ui| {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
                let shown_cover = match &cover {
                    Some(path) => Some(Cover { path: path.clone(), seed: None, ambient: None }),
                    None => current.clone(),
                };
                common::cover(ui, shown_cover.as_ref(), r, shape::MEDIUM);
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    if ui
                        .add(Button::new("Выбрать обложку…").icon(Icon::Edit).style(Style::Tonal).id_salt("pick-cover"))
                        .clicked()
                        && let Some(f) = rfd::FileDialog::new()
                            .set_title("Обложка плейлиста")
                            .add_filter("Изображения", &["jpg", "jpeg", "png", "webp"])
                            .pick_file()
                    {
                        cover = Some(f);
                    }
                    if let Some(f) = cover.as_ref().and_then(|c| c.file_name()) {
                        ui.add_space(4.0);
                        ui.label(TypeRole::BodySmall.style().rich(f.to_string_lossy(), s.on_surface_variant));
                    }
                });
            });
            ui.add_space(24.0);
            submitted = TextField::new(&mut name, "Название")
                .outlined()
                .background(s.surface_container_high)
                .request_focus(!shown)
                .id_salt("pl-name")
                .show(ui)
                .submitted;
            shown = true;
        });
    let save = res.action == Some(1) || (submitted && valid && open);
    if save {
        if name.trim() != p.card.name {
            out.push(Command::RenamePlaylist(p.card.id, name.trim().to_owned()));
        }
        if let Some(path) = cover.take() {
            out.push(Command::SetPlaylistCover(p.card.id, path));
            out.push(Command::LoadPlaylist(p.card.id));
        }
        open = false;
    }
    if !open {
        shown = false;
    }
    put(ctx, key, EditState { open, shown, name, cover });
}

/// Search the library and add tracks to the playlist.
fn add_dialog(ctx: &Context, p: &PlaylistDetail, state: &ViewState, out: &mut Vec<Command>) {
    let key = Id::new(("pl-add", p.card.id));
    let st: AddState = get(ctx, key);
    let s = Theme::get(ctx).scheme;
    let (mut open, mut shown, mut query, mut added) = (st.open, st.shown, st.query, st.added);
    let mut search = None;
    Dialog::new(("pl-add-dialog", p.card.id), "Добавить треки")
        .action("Готово")
        .show_with(ctx, &mut open, |ui| {
            ui.add_space(24.0);
            let o = TextField::new(&mut query, "Поиск треков")
                .outlined()
                .leading_icon(Icon::Search)
                .background(s.surface_container_high)
                .request_focus(!shown)
                .id_salt("pl-add-q")
                .show(ui);
            shown = true;
            if o.response.changed() {
                search = Some(query.clone());
            }
            ui.add_space(8.0);
            let q = query.trim();
            if q.is_empty() {
                ui.label(TypeRole::BodyMedium.style().rich("Введи название трека или исполнителя.", s.on_surface_variant));
            } else if state.search.searching || state.search.query.trim() != q {
                ui.add(LoadingIndicator::new());
            } else if state.search.tracks.is_empty() {
                ui.label(TypeRole::BodyMedium.style().rich("Ничего не нашлось.", s.on_surface_variant));
            } else {
                egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    for t in state.search.tracks.iter().take(50) {
                        let have = added.contains(&t.id) || p.tracks.iter().any(|x| x.id == t.id);
                        let mut add = false;
                        ListItem::new(&t.title)
                            .supporting(match &t.album {
                                Some(a) => format!("{} · {a}", t.artist),
                                None => t.artist.clone(),
                            })
                            .leading(crate::components::Leading::Image(t.cover.as_ref().map(|c| c.uri()), Icon::MusicNote))
                            .trailing(|ui| {
                                add = ui
                                    .add(
                                        IconButton::new(if have { Icon::Check } else { Icon::Add })
                                            .size(Size::S)
                                            .style(Style::Tonal)
                                            .enabled(!have)
                                            .tooltip(if have { "Уже в плейлисте" } else { "Добавить" })
                                            .id_salt(("pl-add-btn", t.id)),
                                    )
                                    .clicked();
                            })
                            .id_salt(("pl-add-row", t.id))
                            .show(ui);
                        if add {
                            out.push(Command::AddToPlaylist(p.card.id, vec![t.id]));
                            out.push(Command::LoadPlaylist(p.card.id));
                            added.push(t.id);
                        }
                    }
                });
            }
        });
    if let Some(q) = search {
        out.push(Command::Search(q));
    }
    if !open {
        shown = false;
    }
    put(ctx, key, AddState { open, shown, query, added });
}

pub fn playlist(
    ui: &mut Ui,
    id: PlaylistId,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let Some(p) = state.playlist.as_ref().filter(|p| p.card.id == id) else {
        return with_bar(ui, "", pages, |ui, _| {
            ui.add(LoadingIndicator::new());
            None
        });
    };
    let liked = id == LIKED_PLAYLIST;
    let ctx = ui.ctx().clone();
    let (edit_key, add_key) = (Id::new(("pl-edit", id)), Id::new(("pl-add", id)));
    let nav = with_bar(ui, &p.card.name, pages, |ui, pages| {
        let size = cover_size(ui);
        header(ui, size, p.card.cover.as_ref(), None, |ui| {
            ui.label(TypeRole::LabelLarge.style().rich("ПЛЕЙЛИСТ", s.primary));
            let n = p.tracks.len() as u64;
            ui.label(TypeRole::TitleMedium.style().rich(
                format!(
                    "{n} {} · {}",
                    common::plural(n, "трек", "трека", "треков"),
                    common::fmt_total(p.total_duration)
                ),
                s.on_surface_variant,
            ));
        });
        ui.add_space(16.0);
        let mut members: Vec<Member> = vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Tonal).into(),
        ];
        if !liked {
            members.push(Button::new("Добавить треки").icon(Icon::Add).size(Size::M).style(Style::Outlined).into());
            members.push(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into());
        }
        let row = ButtonRow::new("pl-actions", Size::M).show(ui, members);
        let r = &row.responses;
        if r[0].clicked() {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: false });
        }
        if r[1].clicked() {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: true });
        }
        if !liked {
            if r[2].clicked() {
                put(ui.ctx(), add_key, AddState { open: true, ..get(ui.ctx(), add_key) });
            }
            let picked = Menu::new("pl-more")
                .item(MenuItem::new("Изменить…").icon(Icon::Edit))
                .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
                .divider()
                .item(MenuItem::new("Удалить плейлист").icon(Icon::Delete))
                .attach(&r[3]);
            match picked {
                Some(0) => put(
                    ui.ctx(),
                    edit_key,
                    EditState { open: true, shown: false, name: p.card.name.clone(), cover: None },
                ),
                Some(1) => out.push(Command::Enqueue(p.tracks.iter().map(|t| t.id).collect())),
                Some(2) => pages.confirm_delete = Some(id),
                _ => {}
            }
        }
        ui.add_space(16.0);
        if p.tracks.is_empty() {
            widgets::empty_state(
                ui,
                Icon::QueueMusic,
                "Плейлист пуст",
                "Добавляй треки через ⋮ → «В плейлист…» или правой кнопкой по треку.",
                None,
            );
            return None;
        }
        let opts = TrackListOpts { playlist: Some(id), reorder: !liked };
        widgets::track_list_ext(ui, "playlist", &p.tracks, Lead::Cover, &PlayFrom::Playlist(id), opts, state, pages, out)
    });
    if !liked {
        edit_dialog(&ctx, p, out);
        add_dialog(&ctx, p, state, out);
    }
    nav
}
