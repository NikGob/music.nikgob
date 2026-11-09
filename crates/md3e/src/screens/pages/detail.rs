//! Альбом / Исполнитель / Плейлист (md3e-player.md §3.3–3.4).

use std::path::PathBuf;

use egui::{Context, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    ArtistId, Command, Cover, LIKED_PLAYLIST, LikeTarget, MixKey, PlaylistCoverMode, PlaylistDetail, PlaylistId, ReleaseId,
    ReleaseLine, ReleaseType, Route, TrackId, ViewState,
};

use super::{PageState, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, ButtonRow, Dialog, DialogAction, FilterChip, FlexibleAppBar, IconButton, ListItem,
        LoadingIndicator, Member, MenuItem, Size, SplitButton, Style, TextField, app_bar,
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
pub(super) fn with_bar<R>(
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
    artist: Option<&nikgob_view::ArtistCard>,
    add_text: impl FnOnce(&mut Ui),
) {
    header_with(
        ui,
        size,
        |ui, rect| match artist {
            // The same picture as in Библиотека: the photo, or the
            // initials in the artist's own role and shape.
            Some(a) => widgets::artist_picture(ui, a, rect),
            None => common::cover(ui, cover, rect, shape::EXTRA_LARGE),
        },
        add_text,
    );
}

/// [`header`] with the picture `paint` draws in its square.
fn header_with(ui: &mut Ui, size: f32, paint: impl FnOnce(&mut Ui, Rect), add_text: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        paint(ui, rect);
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
    // The album was deleted (or its last track): nothing left to show, go
    // back like ← does.
    // Once: the page may still be drawn while the transition runs.
    let left = Id::new(("release-left-empty", id));
    if r.tracks.is_empty() {
        if !ui.ctx().data(|d| d.get_temp::<bool>(left).unwrap_or(false)) {
            ui.ctx().data_mut(|d| d.insert_temp(left, true));
            pages.go_back = true;
        }
    } else {
        ui.ctx().data_mut(|d| d.remove::<bool>(left));
    }
    // One line per place the album counts; the counts are theirs.
    let n = if r.lines.is_empty() { r.tracks.len() } else { r.lines.len() } as u32;
    let lacking = n.saturating_sub(c.local_tracks);
    with_bar(ui, &c.title, pages, |ui, pages| {
        let mut nav = None;
        let size = cover_size(ui);
        header(ui, size, c.cover.as_ref(), None, |ui| {
            // «АЛЬБОМ · 17 МАЯ 2019» when the day is known, else the year.
            let when = match (r.released, c.year) {
                (Some(d), _) => Some(common::fmt_release_day(d).to_uppercase()),
                (None, Some(y)) => Some(y.to_string()),
                (None, None) => None,
            };
            let line = match when {
                Some(w) => format!("{} · {w}", type_label(c.release_type)),
                None => type_label(c.release_type).to_owned(),
            };
            ui.label(TypeRole::LabelLarge.style().rich(line, s.primary));
            // How much is here, in the same line as its size: «стрим» (no
            // files: it plays from a service), «14 треков · 13 скачано · 1
            // нет» — counted over the lines below.
            let mut line = format!("{} · {n} {}", c.artist, common::plural(u64::from(n), "трек", "трека", "треков"));
            if c.local_tracks > 0 && lacking > 0 {
                line = format!("{line} · {} скачано · {lacking} нет", c.local_tracks);
            }
            line = format!("{line} · {}", common::fmt_total(r.total_duration));
            if c.local_tracks == 0 {
                line.push_str(" · стрим");
            }
            // Each artist of the album is a link to its page.
            let (links, routes) = crate::screens::artist_links::library_spans(&c.artists, &line);
            let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
            if let Some(k) = crate::components::link_paragraph(ui, "rel-artists", &line, &links, format).clicked {
                nav = routes.get(k).cloned();
            }
            // The label and the formats, quietly: nothing to press.
            let facts: Vec<&str> = r.label.iter().chain(&r.quality).map(String::as_str).collect();
            if !facts.is_empty() {
                ui.label(TypeRole::BodyMedium.style().rich(facts.join(" · "), s.on_surface_variant));
            }
        });
        ui.add_space(16.0);

        // Actions: standard button group, "Слушать" is the only filled button.
        let missing = lacking > 0;
        let query = format!("{} {}", c.artist, c.title);
        // The album's «Хочу скачать» item, if it is on its way.
        let wish = widgets::WishIndex::of(ui.ctx(), &state.downloads.wishlist).of_release(id);
        let dl_label = match wish {
            Some(nikgob_view::WishlistStatus::Downloading) => "Качается",
            Some(nikgob_view::WishlistStatus::New | nikgob_view::WishlistStatus::Searching | nikgob_view::WishlistStatus::Confirm)
                if !state.prefs.prowlarr_configured =>
            {
                "Ждёт торренты"
            }
            Some(nikgob_view::WishlistStatus::New | nikgob_view::WishlistStatus::Searching | nikgob_view::WishlistStatus::Confirm) => "В очереди",
            Some(nikgob_view::WishlistStatus::NoTorrent | nikgob_view::WishlistStatus::Failed) => "Не нашлось",
            // A narrow page keeps the row on one line (the menu says more).
            _ if ui.available_width() < 640.0 => "Скачать",
            _ => "Скачать недостающие",
        };
        // «Скачать недостающие · 1»: as many as the list shows without a file.
        let dl_label = if matches!(dl_label, "Скачать недостающие" | "Скачать") {
            format!("{dl_label} · {lacking}")
        } else {
            dl_label.to_owned()
        };
        let mut members: Vec<Member> = vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            widgets::shuffle_member(ui),
        ];
        if missing {
            members.push(
                SplitButton::new("rel-download", &dl_label)
                    .icon(Icon::Download)
                    .size(Size::M)
                    .style(Style::Tonal)
                    .item(MenuItem::new("Все недостающие").icon(Icon::Download))
                    .item(MenuItem::new("Выбрать раздачу…").icon(Icon::Search))
                    .item(MenuItem::new("Заменить на FLAC…").icon(Icon::Search))
                    .into(),
            );
        }
        members.push(
            IconButton::new(Icon::Favorite).size(Size::M).style(Style::Tonal).selected(r.liked).tooltip(widgets::album_like_label(r.liked)).into(),
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
        // «Скачать недостающие»: one album item of «Хочу скачать» (it picks
        // the release and the files itself); once on its way, the button
        // opens it. By hand: the torrent search view.
        if missing && resp[2].clicked() {
            match wish {
                Some(nikgob_view::WishlistStatus::Waiting | nikgob_view::WishlistStatus::Dismissed) | None => {
                    out.push(Command::DownloadRelease(id));
                }
                Some(_) => nav = Some(Route::Downloads(nikgob_view::DownloadsTab::Wishlist)),
            }
        }
        match row.picked {
            Some((2, 0)) => out.push(Command::DownloadRelease(id)),
            Some((2, 1)) => out.push(Command::SearchTorrents(query.clone())),
            Some((2, 2)) => out.push(Command::SearchTorrents(format!("{query} FLAC"))),
            _ => {}
        }
        if resp[2 + off].clicked() {
            out.push(Command::Like(LikeTarget::Release(id), !r.liked));
        }
        let ids: Vec<_> = r.tracks.iter().map(|t| t.id).collect();
        let more = &resp[3 + off];
        // The same actions as the album's right-click menu
        // ([`widgets::release_rows`]), less the buttons left of it.
        let (menu, acts) = widgets::menu_of("rel-more", widgets::release_rows(c, true));
        match menu.attach(more).and_then(|i| acts.get(i).copied()) {
            Some(widgets::ReleaseAct::Playlist) => {
                pages.pending_add = Some(ids.clone());
                pages.pending_add_anchor = Some(more.rect);
            }
            Some(a) => {
                if let Some(r) = widgets::release_apply(ui.ctx(), a, c, out) {
                    nav = Some(r);
                }
            }
            None => {}
        }
        ui.add_space(16.0);
        // «Не скачано · 1»: only the lines without a file (the album has
        // some files; with none every line is one).
        let only_key = Id::new(("rel-only-missing", id));
        let mut only = ui.ctx().data(|d| d.get_temp::<bool>(only_key).unwrap_or(false));
        if c.local_tracks > 0 && lacking > 0 {
            let chip = FilterChip::new(format!("Не скачано · {lacking}"), only).id_salt("rel-only-missing");
            if ui.add(chip).clicked() {
                only = !only;
                ui.ctx().data_mut(|d| d.insert_temp(only_key, only));
            }
            ui.add_space(8.0);
        } else {
            only = false;
        }
        let lines: Vec<ReleaseLine> = if r.lines.is_empty() {
            (0..r.tracks.len()).map(ReleaseLine::Track).collect()
        } else {
            r.lines.clone()
        };
        let lines: Vec<ReleaseLine> = if only {
            lines
                .into_iter()
                .filter(|l| match *l {
                    ReleaseLine::Track(i) => !r.tracks[i].has_local,
                    ReleaseLine::Hole { .. } => true,
                })
                .collect()
        } else {
            lines
        };
        // The songs it lacks (from a service's tracklist): quieter, «стрим»;
        // a place no track has: «Трек 7 · нет файла».
        let opts = TrackListOpts { mark_missing: true, lines: Some(&lines), ..Default::default() };
        widgets::track_list_ext(ui, "release", &r.tracks, Lead::Number, &PlayFrom::Release(id), opts, state, pages, out)
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
            Some(&a.card),
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
                Button::new("Радио").icon(Icon::Radio).size(Size::M).style(Style::Tonal).into(),
                Button::new(if a.liked { "В подписках" } else { "Подписаться" })
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
        // An endless queue: the artist's tracks you like most, then what
        // sounds and is listened to like them.
        if r[2].clone().on_hover_text("Его треки и похожее — очередь дописывается сама").clicked() {
            out.push(Command::StartRadio { seed: nikgob_view::RadioSeed::Artist(id) });
        }
        if r[3].clicked() {
            out.push(Command::Like(LikeTarget::Artist(id), !a.liked));
        }
        // The same actions as the artist's right-click menu
        // ([`widgets::artist_rows`]), less the buttons left of it.
        let (menu, acts) = widgets::menu_of("artist-more", widgets::artist_rows(&a.card, true, &state.dislikes));
        if let Some(act) = menu.attach(&r[4]).and_then(|i| acts.get(i).copied()) {
            widgets::artist_apply(act, &a.card, &state.dislikes, out);
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
        widgets::edition_offers(ui, state, Some(&a.card.name), out);
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
        nav = discography(ui, &a.discography, &state.downloads.wishlist, out).or(nav);
        nav
    })
}

/// «Нет в библиотеке»: the artist's albums in the services the library has
/// no file of, as plain album cards. They play from the stream; ⬇ on hover
/// (or «Скачать» in the menu) likes the album, which puts it on the
/// wishlist. Hidden while there is nothing to show.
fn discography(
    ui: &mut Ui,
    d: &nikgob_view::DiscographyView,
    wishlist: &nikgob_view::WishlistView,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    if d.albums.is_empty() {
        if d.loading {
            widgets::section_header(ui, "Нет в библиотеке", false);
            ui.add(LoadingIndicator::new().size(32.0));
        }
        return None;
    }
    widgets::section_header(ui, "Нет в библиотеке", false);
    ui.label(TypeRole::BodyMedium.style().rich_para(
        "Есть в Spotify или YouTube Music, а у тебя нет — играют из стрима.",
        s.on_surface_variant,
    ));
    ui.add_space(8.0);
    // Albums, then singles and EPs, then compilations (newest first each).
    let kinds: [(&str, fn(ReleaseType) -> bool); 3] = [
        ("Альбомы", |t| t == ReleaseType::Album),
        ("Синглы и EP", |t| matches!(t, ReleaseType::Single | ReleaseType::Ep)),
        ("Сборники и другое", |t| !matches!(t, ReleaseType::Album | ReleaseType::Single | ReleaseType::Ep)),
    ];
    let groups: Vec<(&str, Vec<&nikgob_view::StreamAlbumCard>)> = kinds
        .iter()
        .map(|(title, is)| (*title, d.albums.iter().filter(|a| is(a.release_type)).collect::<Vec<_>>()))
        .filter(|(_, list)| !list.is_empty())
        .collect();
    let mut nav = None;
    let titled = groups.len() > 1;
    for (title, list) in groups {
        if titled {
            ui.label(TypeRole::TitleSmall.style().rich(title, s.on_surface_variant));
            ui.add_space(4.0);
        }
        nav = widgets::grid(ui, &list, widgets::CARD_W, |_, w| crate::components::MediaCard::height(w), |ui, a, w| {
            super::streams::album_card(ui, a, w, "discography", wishlist, out)
        })
        .or(nav);
        ui.add_space(8.0);
    }
    nav
}

/// State of the playlist edit dialog.
#[derive(Clone, Default)]
struct EditState {
    open: bool,
    shown: bool,
    name: String,
    cover: Option<PathBuf>,
    /// Cover mode picked in the dialog (`None` = as it was).
    mode: Option<PlaylistCoverMode>,
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
    let (mut open, mut shown, mut name, mut cover, mut mode) = (st.open, st.shown, st.name, st.cover, st.mode);
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
                    // Smart: a collage with 4+ albums, else the most common
                    // album cover, else a gradient.
                    let cur = mode.unwrap_or(p.card.cover_mode);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for (m, label, tip) in [
                            (PlaylistCoverMode::Smart, "Авто", "Коллаж, если в плейлисте 4+ альбома, иначе самая частая обложка"),
                            (PlaylistCoverMode::Collage, "Коллаж", "Обложки первых четырёх альбомов, 2×2"),
                        ] {
                            if ui
                                .add(FilterChip::new(label, cur == m && cover.is_none()).id_salt(("pl-cover-mode", label)))
                                .on_hover_text(tip)
                                .clicked()
                            {
                                mode = Some(m);
                                cover = None;
                            }
                        }
                    });
                    ui.add_space(8.0);
                    if ui
                        .add(
                            Button::new("Своя картинка…")
                                .icon(Icon::Edit)
                                .style(Style::Tonal)
                                .selected(cover.is_some() || (mode.is_none() && cur == PlaylistCoverMode::Custom))
                                .id_salt("pick-cover"),
                        )
                        .clicked()
                        && let Some(f) = rfd::FileDialog::new()
                            .set_title("Обложка плейлиста")
                            .add_filter("Изображения", &["jpg", "jpeg", "png", "webp"])
                            .pick_file()
                    {
                        cover = Some(f);
                        mode = Some(PlaylistCoverMode::Custom);
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
        } else if let Some(m) = mode.take()
            && m != PlaylistCoverMode::Custom
            && m != p.card.cover_mode
        {
            out.push(Command::SetPlaylistCoverMode(p.card.id, m));
        }
        open = false;
    }
    if !open {
        shown = false;
        mode = None;
    }
    put(ctx, key, EditState { open, shown, name, cover, mode });
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
            if let Some(l) = &p.sync {
                ui.add_space(8.0);
                let mode = widgets::sync_state_label(l.mode);
                // Which service it follows and how, in words (the modes are
                // in ⋮), then a small status: running, failed, or when.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let when = if l.syncing {
                        "синхронизируется…".to_owned()
                    } else {
                        l.synced.as_ref().map_or_else(String::new, |t| format!("синхронизирован {t}"))
                    };
                    let mut line = format!("{} · {mode}", l.remote.service.display_name());
                    if !when.is_empty() && l.error.is_none() {
                        line = format!("{line} · {when}");
                    }
                    ui.label(TypeRole::BodyMedium.style().rich(line, s.on_surface_variant))
                        .on_hover_text("Связан с плейлистом сервиса; как синхронизировать — в ⋮");
                    if l.syncing {
                        ui.add(LoadingIndicator::new().size(20.0));
                    }
                });
                if let Some(e) = &l.error {
                    ui.label(TypeRole::BodySmall.style().rich(e, s.on_surface_variant));
                }
            }
        });
        ui.add_space(16.0);
        let mut members: Vec<Member> = vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            widgets::shuffle_member(ui),
        ];
        if !liked {
            members.push(Button::new("Добавить треки").icon(Icon::Add).size(Size::M).style(Style::Outlined).into());
        }
        members.push(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into());
        let row = ButtonRow::new("pl-actions", Size::M).show(ui, members);
        let r = &row.responses;
        if r[0].clicked() {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: false });
        }
        if r[1].clicked() {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: true });
        }
        if !liked && r[2].clicked() {
            put(ui.ctx(), add_key, AddState { open: true, ..get(ui.ctx(), add_key) });
        }
        // The same actions as the playlist's right-click menu
        // ([`widgets::playlist_rows`]), less «Слушать» and «Перемешать»
        // (the buttons left of it).
        {
            let more = &r[r.len() - 1];
            let of = widgets::PlaylistMenuOf {
                on_page: true,
                editable: !liked,
                sync: p.sync.as_ref().map(|l| (l.remote.service.display_name(), l.mode)),
                can_link: !liked && p.sync.is_none(),
            };
            let (menu, acts) = widgets::menu_of("pl-more", widgets::playlist_rows(of));
            match menu.attach(more).and_then(|i| acts.get(i).copied()) {
                Some(widgets::PlaylistAct::Edit) => put(
                    ui.ctx(),
                    edit_key,
                    EditState { open: true, shown: false, name: p.card.name.clone(), cover: None, mode: None },
                ),
                Some(widgets::PlaylistAct::Delete) => pages.confirm_delete = Some(id),
                Some(a) => out.extend(widgets::playlist_command(a, id)),
                None => {}
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
        let opts = TrackListOpts { playlist: Some(id), reorder: !liked, ..Default::default() };
        widgets::track_list_ext(ui, "playlist", &p.tracks, Lead::Cover, &PlayFrom::Playlist(id), opts, state, pages, out)
    });
    if !liked {
        edit_dialog(&ctx, p, out);
        add_dialog(&ctx, p, state, out);
    }
    nav
}

/// A mix's page ([`Route::Mix`]), laid out like a playlist's: the mosaic
/// of the card, «МИКС», its artists, how many songs and how long, then
/// «Слушать», «Перемешать» and the card's ⋮, and its songs, a click on one
/// playing the mix from it. A mix no longer offered (the day's lineup
/// changed, it was left out) says so.
pub fn mix(
    ui: &mut Ui,
    key: MixKey,
    world: Option<&str>,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    // A world's mix is of the world on its page: one left since (through
    // the history) is loaded again.
    let asked = Id::new(("mix-world-asked", world));
    let lanes = match world {
        None => Some(state.home.lanes.as_slice()),
        Some(w) => state.world.as_ref().filter(|v| v.world.key == w).map(|v| v.mixes.as_slice()),
    };
    let Some(lanes) = lanes else {
        let was_asked = ui.ctx().data(|d| d.get_temp::<bool>(asked).unwrap_or(false));
        if !was_asked && let Some(w) = world {
            ui.ctx().data_mut(|d| d.insert_temp(asked, true));
            out.push(Command::LoadWorld(w.to_owned()));
        }
        // Asked, and the world is gone.
        if !(was_asked && state.world.is_none()) {
            return with_bar(ui, "", pages, |ui, _| {
                ui.add(LoadingIndicator::new());
                None
            });
        }
        return mix_gone(ui, key, world, state, pages);
    };
    ui.ctx().data_mut(|d| d.remove::<bool>(asked));
    let Some(l) = lanes.iter().find(|l| l.key == key) else {
        return mix_gone(ui, key, world, state, pages);
    };
    let local_only = state.prefs.mixes_downloaded_only;
    let with_new = world.is_none() && state.prefs.mixes_with_new && !local_only;
    let can_blend = world.is_none() && l.blend_base.is_some_and(|b| state.home.blend_profiles.iter().any(|p| p.key != b));
    let nav = with_bar(ui, &l.title, pages, |ui, pages| {
        let mut nav = None;
        let size = cover_size(ui);
        // The card's mosaic, «С новым» with a new song's cover fourth.
        let mut covers: Vec<String> = l.covers.iter().map(Cover::uri).collect();
        if with_new && let Some(c) = &l.new_cover {
            covers.truncate(3);
            if covers.len() == 3 {
                covers.push(c.uri());
            }
        }
        let n = l.tracks.len() as u64;
        let total: std::time::Duration = l.tracks.iter().filter_map(|t| t.duration).sum();
        header_with(
            ui,
            size,
            |ui, rect| super::cards::mosaic(ui, rect, &covers, shape::EXTRA_LARGE, 2.0, 1.0),
            |ui| {
                ui.label(TypeRole::LabelLarge.style().rich("МИКС", s.primary));
                // A name too long for the bar's one line is said whole here.
                let bar = TypeRole::DisplaySmall.style().job(&l.title, s.on_surface);
                let bar_w = crate::typography::layout(ui.painter(), bar).size().x;
                if bar_w > ui.ctx().content_rect().width() - size - 160.0 {
                    ui.label(TypeRole::HeadlineSmall.style().rich(&l.title, s.on_surface));
                }
                let tail = format!("{n} {} · {}", common::plural(n, "трек", "трека", "треков"), common::fmt_total(total));
                let width = ui.available_width();
                let role = TypeRole::TitleMedium.style();
                let fits = |t: &str| {
                    let mut job = egui::text::LayoutJob::single_section(format!("{t} · {tail}"), role.text_format(s.on_surface));
                    job.wrap.max_width = width;
                    crate::typography::layout(ui.painter(), job).rows.len() <= 2
                };
                let new: &[String] = if with_new { &l.new_artists } else { &[] };
                let artists = super::cards::names_line(&fits, &l.artists, new);
                let line = if artists.is_empty() { tail } else { format!("{artists} · {tail}") };
                ui.label(role.rich(line, s.on_surface_variant));
            },
        );
        ui.add_space(16.0);
        let members: Vec<Member> = vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            widgets::shuffle_member(ui),
            IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into(),
        ];
        let row = ButtonRow::new("mix-actions", Size::M).show(ui, members);
        let r = &row.responses;
        if r[0].clicked() {
            out.push(Command::PlayMix { id: l.id() });
        }
        if r[1].clicked() {
            super::cards::mix_do(ui.ctx(), l, super::cards::MixItem::Shuffle, world, out);
        }
        let items = super::cards::mix_items(l, can_blend, false);
        if let Some(k) = super::cards::mix_menu(Id::new("mix-more"), l, &items).attach(&r[2]) {
            nav = super::cards::mix_do(ui.ctx(), l, items[k], world, out);
            // Left out: back to where it was offered.
            if items[k] == super::cards::MixItem::Hide {
                pages.go_back = true;
            }
        }
        ui.add_space(16.0);
        if l.tracks.is_empty() {
            let body = if local_only {
                "Включено «Только скачанное», а скачанных треков для него нет."
            } else {
                "Его песни ещё не нашлись."
            };
            widgets::empty_state(ui, Icon::QueueMusic, "В миксе нет треков", body, None);
            return nav;
        }
        let ids = l.tracks.iter().map(|t| t.id).collect();
        let from = PlayFrom::Tracks(ids, l.context());
        widgets::track_list_ext(ui, "mix", &l.tracks, Lead::Cover, &from, TrackListOpts::default(), state, pages, out).or(nav)
    });
    if world.is_none() {
        super::home::blend_sheet(ui.ctx(), state, out);
    }
    nav
}

/// The page of a mix no longer offered: why, and the way to the mixes.
fn mix_gone(ui: &mut Ui, key: MixKey, world: Option<&str>, state: &ViewState, pages: &mut PageState) -> Option<Route> {
    let hidden = state.prefs.home_hidden_mixes.iter().find(|h| h.key == key);
    let title = hidden.map_or("", |h| h.title.as_str());
    with_bar(ui, title, pages, |ui, _| {
        let (body, action, to) = match (world, hidden) {
            (Some(w), _) => ("На странице мира теперь другие миксы.", "К миру", Route::World(w.to_owned())),
            (None, Some(_)) => (
                "Ты попросил его не предлагать. Вернуть можно в Настройках → «Рекомендации».",
                "На главную",
                Route::Home,
            ),
            (None, None) => ("Миксы на главной обновились.", "На главную", Route::Home),
        };
        widgets::empty_state(ui, Icon::QueueMusic, "Этого микса больше нет", body, Some(action)).then_some(to)
    })
}
