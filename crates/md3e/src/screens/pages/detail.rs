//! Альбом / Исполнитель / Плейлист.

use std::path::PathBuf;
use std::time::Duration;

use egui::{Context, Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    AlbumDate, ArtistId, Command, Cover, LIKED_PLAYLIST, LikeTarget, MixKey, PlaylistCoverMode, PlaylistDetail, PlaylistId,
    ReleaseCard, ReleaseDay, ReleaseDetail, ReleaseId, ReleaseLine, ReleaseType, Route, TrackId, ViewState,
};

use super::{PageState, widgets};
#[cfg(target_os = "android")]
use crate::no_rfd as rfd;
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, ButtonRow, ConnectedGroup, Dialog, DialogAction, FilterChip, FlexibleAppBar, IconButton, ListItem,
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
    widgets::header_cover(ui.available_width())
}

/// What the header of an album's page shows: the album, how its date is
/// written, how many places it counts and how long it is, its label and
/// formats.
pub struct ReleaseHeader<'a> {
    pub card: &'a ReleaseCard,
    pub released: Option<ReleaseDay>,
    pub album_date: AlbumDate,
    /// The places it counts (its lines, else its tracks).
    pub count: u32,
    /// Of them, the ones with a file.
    pub local: u32,
    /// «13 скачано · 1 нет» is said (not on «Весь альбом» of an album only
    /// some songs of which are the user's: the rest is not theirs to lack).
    pub counts: bool,
    pub total: Duration,
    /// The label and the formats.
    pub facts: Vec<&'a str>,
}

/// What [`release_header`] gives back.
pub struct HeaderOut {
    /// An artist clicked.
    pub nav: Option<Route>,
    /// Where the type and date line is.
    pub date: Rect,
}

/// The header of an album's page: its cover, «АЛЬБОМ · 2019» (as far as
/// `album_date` asks and the date is known), the artists, the count and
/// the length, the label and the formats. Drawn by the page and by the
/// settings' stage without the page open.
pub fn release_header(ui: &mut Ui, h: &ReleaseHeader<'_>) -> HeaderOut {
    let s = Theme::get(ui.ctx()).scheme;
    let c = h.card;
    let n = h.count;
    let lacking = n.saturating_sub(h.local);
    let mut nav = None;
    let mut date = Rect::NOTHING;
    let size = cover_size(ui);
    header(ui, size, c.cover.as_ref(), None, |ui| {
        // «АЛЬБОМ · 2019», «АЛЬБОМ · МАЙ 2019» or «АЛЬБОМ · 17 МАЯ 2019»
        // (Prefs::album_date), as far as the date is known.
        let when = common::fmt_album_date(h.released, c.year, h.album_date).map(|w| w.to_uppercase());
        let line = match when {
            Some(w) => format!("{} · {w}", type_label(c.release_type)),
            None => type_label(c.release_type).to_owned(),
        };
        date = ui.label(TypeRole::LabelLarge.style().rich(line, s.primary)).rect;
        // How much is here, in the same line as its size: «стрим» (no
        // files: it plays from a service), «14 треков · 13 скачано · 1
        // нет» — counted over the lines below.
        let mut line = format!("{} · {n} {}", c.artist, common::plural(u64::from(n), "трек", "трека", "треков"));
        if h.counts && h.local > 0 && lacking > 0 {
            line = format!("{line} · {} скачано · {lacking} нет", h.local);
        }
        line = format!("{line} · {}", common::fmt_total(h.total));
        if h.counts && h.local == 0 {
            line.push_str(" · стрим");
        }
        // Each artist of the album is a link to its page.
        let (links, routes) = crate::screens::artist_links::library_spans(&c.artists, &line);
        let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
        if let Some(k) = crate::components::link_paragraph(ui, "rel-artists", &line, &links, format).clicked {
            nav = routes.get(k).cloned();
        }
        // The label and the formats, quietly: nothing to press.
        if !h.facts.is_empty() {
            ui.label(TypeRole::BodyMedium.style().rich(h.facts.join(" · "), s.on_surface_variant));
        }
    });
    HeaderOut { nav, date }
}

/// An album's whole page without it being open: the large bar with its
/// title (never collapsed) over [`release_body`]. For the settings' stage.
pub fn release_on(ui: &mut Ui, r: &ReleaseDetail, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> HeaderOut {
    let left = ui.cursor().left();
    let top = ui.cursor().top();
    let width = ui.available_width();
    ui.add_space(app_bar::LARGE_HEIGHT);
    let drawn = release_body(ui, r, state, pages, out);
    let m = app_bar::flexible_metrics(0.0, false, true);
    let rect = Rect::from_min_size(pos2(left, top), vec2(width, m.height));
    let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("detail-bar-ui"));
    FlexibleAppBar::new(&r.card.title)
        .leading(Icon::ArrowBack, "Назад (Esc)")
        .id_salt("detail-bar")
        .show(&mut bar_ui, 0.0, |_| {});
    drawn
}

pub fn release(
    ui: &mut Ui,
    id: ReleaseId,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let Some(r) = state.release.as_ref().filter(|r| r.card.id == id) else {
        return with_bar(ui, "", pages, |ui, _| {
            ui.add(LoadingIndicator::new());
            None
        });
    };
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
    with_bar(ui, &r.card.title, pages, |ui, pages| release_body(ui, r, state, pages, out).nav)
}

/// An album's page under its bar: the header, the actions and the list.
fn release_body(ui: &mut Ui, r: &ReleaseDetail, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> HeaderOut {
    let id = r.card.id;
    let c = &r.card;
    // An album only some songs of which are the user's: «Мои треки» (its
    // lines are those songs) or «Весь альбом» (every place, the versions of
    // its songs the user did not choose as a group of their own), as it
    // opens or as switched here.
    let whole = r.partial && widgets::visit_whole(ui.ctx(), id).unwrap_or(r.open_whole);
    let mine = r.partial && !whole;
    let (lines, versions_at) = if mine { (r.mine_lines(), None) } else { r.all_lines() };
    let shown: Vec<usize> = lines
        .iter()
        .filter_map(|l| match *l {
            ReleaseLine::Track(i) => Some(i),
            ReleaseLine::Hole { .. } => None,
        })
        .collect();
    // One line per place shown; the counts are theirs.
    let n = lines.len() as u32;
    let local = shown.iter().filter(|&&i| r.tracks[i].has_local).count() as u32;
    let lacking = n.saturating_sub(local);
    let length: Duration = if mine { shown.iter().filter_map(|&i| r.tracks[i].duration).sum() } else { r.total_duration };
    let head = ReleaseHeader {
        card: c,
        released: r.released,
        album_date: state.prefs.album_date,
        count: n,
        local,
        counts: !(r.partial && whole),
        total: length,
        facts: r.label.iter().chain(&r.quality).map(String::as_str).collect(),
    };
    let drawn = release_header(ui, &head);
    let mut nav = drawn.nav;
    ui.add_space(16.0);

    // Actions: standard button group, "Слушать" is the only filled button.
    // «Весь альбом» of an album only some songs of which are the user's
    // lacks nothing; such an album can be added to the library whole.
    let missing = lacking > 0 && !(r.partial && whole);
    let add_whole = r.partial;
    let query = format!("{} {}", c.artist, c.title);
    let index = widgets::WishIndex::of(ui.ctx(), &state.downloads.wishlist);
    // The album's «Хочу скачать» item, if it is on its way; of the
    // user's songs of an album only some of which are theirs, the item
    // of one of them without a file.
    let wish = if mine {
        shown.iter().filter(|&&i| !r.tracks[i].has_local).find_map(|&i| index.of_track(Some(r.tracks[i].id), None))
    } else {
        index.of_release(id)
    };
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
    // By hand, through the indexer: only with a torrent source set up
    // (a search for the whole album: not of the user's songs alone).
    let torrents = state.prefs.prowlarr_configured && !mine;
    if add_whole {
        let label = if ui.available_width() < 640.0 { "Добавить" } else { widgets::ADD_ALBUM };
        members.push(Button::new(label).icon(Icon::LibraryMusic).size(Size::M).style(Style::Tonal).into());
    }
    if missing && torrents {
        members.push(
            SplitButton::new("rel-download", &dl_label)
                .icon(Icon::Download)
                .size(Size::M)
                .style(Style::Tonal)
                .item(MenuItem::new(super::PICK_RELEASE).icon(Icon::Search))
                .item(MenuItem::new("Заменить на FLAC…").icon(Icon::Search))
                .into(),
        );
    } else if missing {
        members.push(Button::new(&dl_label).icon(Icon::Download).size(Size::M).style(Style::Tonal).into());
    }
    members.push(
        IconButton::new(Icon::Favorite).size(Size::M).style(Style::Tonal).selected(r.liked).tooltip(widgets::album_like_label(r.liked)).into(),
    );
    members.push(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into());
    let row = ButtonRow::new("rel-actions", Size::M).show(ui, members);
    let resp = &row.responses;
    let play = |shuffle: bool| page_play(id, r.partial, whole, shuffle);
    if resp[0].clicked() {
        out.push(play(false));
    }
    if resp[1].clicked() {
        out.push(play(true));
    }
    let at = 2 + usize::from(add_whole);
    if add_whole && resp[2].clicked() {
        out.push(Command::AddWholeAlbum { id, on: true });
    }
    let off = usize::from(add_whole) + usize::from(missing);
    // «Скачать недостающие»: one album item of «Хочу скачать» (it picks
    // the release and the files itself), of the user's songs an item
    // each; once on its way, the button opens it. By hand: the torrent
    // search view.
    if missing && resp[at].clicked() {
        match wish {
            Some(nikgob_view::WishlistStatus::Waiting | nikgob_view::WishlistStatus::Dismissed) | None => {
                out.push(Command::DownloadRelease(id));
            }
            Some(_) => nav = Some(Route::Downloads(nikgob_view::DownloadsTab::Wishlist)),
        }
    }
    match row.picked {
        Some((i, 0)) if i == at => out.push(Command::SearchTorrents(query.clone())),
        Some((i, 1)) if i == at => out.push(Command::SearchTorrents(format!("{query} FLAC"))),
        _ => {}
    }
    if resp[2 + off].clicked() {
        out.push(Command::Like(LikeTarget::Release(id), !r.liked));
    }
    // «В плейлист…»: the songs the page shows.
    let ids: Vec<_> = shown.iter().map(|&i| r.tracks[i].id).collect();
    let more = &resp[3 + off];
    // The same actions as the album's right-click menu
    // ([`widgets::release_rows`]), less the buttons left of it; and
    // what the album opens on, and an album added whole back to the
    // user's songs.
    let mut rows = widgets::release_rows(c, true);
    // «Слушать» of «Весь альбом» already plays it whole.
    if whole {
        rows.retain(|x| !matches!(x, widgets::Row::Item(widgets::ReleaseAct::PlayWhole, _)));
    }
    if r.added_whole {
        let at = rows.iter().position(|x| matches!(x, widgets::Row::Item(widgets::ReleaseAct::Playlist, _))).map_or(rows.len(), |p| p + 1);
        rows.insert(at, widgets::Row::Item(widgets::ReleaseAct::KeepMine, MenuItem::new(widgets::KEEP_MINE).icon(Icon::LibraryMusic)));
    }
    let (menu, acts) = widgets::menu_of("rel-more", rows);
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
    // «Мои треки · 2 │ Весь альбом · 9».
    if r.partial {
        let all = r.all_lines().0.len();
        let labels = [format!("Мои треки · {}", r.mine_lines().len()), format!("Весь альбом · {all}")];
        let labels = [labels[0].as_str(), labels[1].as_str()];
        if let Some(i) = ConnectedGroup::new(("rel-view-group", id), &labels).show(ui, usize::from(whole)) {
            widgets::show_whole(ui.ctx(), id, i == 1);
        }
        ui.add_space(16.0);
    }
    // «Не скачано · 1»: only the lines without a file (the album has
    // some files; with none every line is one).
    let only_key = Id::new(("rel-only-missing", id));
    let mut only = ui.ctx().data(|d| d.get_temp::<bool>(only_key).unwrap_or(false));
    if local > 0 && missing {
        let chip = FilterChip::new(format!("Не скачано · {lacking}"), only).id_salt("rel-only-missing");
        if ui.add(chip).clicked() {
            only = !only;
            ui.ctx().data_mut(|d| d.insert_temp(only_key, only));
        }
        ui.add_space(8.0);
    } else {
        only = false;
    }
    let versions_at = versions_at.filter(|_| !only);
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
    // a place no track has: «Трек 7 · нет файла». «Весь альбом» of an
    // album only some songs of which are the user's: each song in or
    // out of their songs, a place without a track no download of its
    // own.
    let opts = TrackListOpts {
        mark_missing: true,
        lines: Some(&lines),
        album_marks: (r.partial && whole).then_some(r.marks.as_slice()),
        versions_at,
        versions_on: r.versions_on,
        no_hole_download: r.partial,
        ..Default::default()
    };
    let from = if r.partial { PlayFrom::ReleaseAs(id, whole) } else { PlayFrom::Release(id) };
    let nav = widgets::track_list_ext(ui, "release", &r.tracks, Lead::Number, &from, opts, state, pages, out).or(nav);
    HeaderOut { nav, date: drawn.date }
}

/// What «Слушать» (`shuffle`: «Перемешать») of an album's page plays: what
/// the page shows. Of an album only some songs of which are the user's
/// (`partial`), «Весь альбом» (`whole`) plays all of it, «Мои треки»
/// their songs.
fn page_play(id: nikgob_view::ReleaseId, partial: bool, whole: bool, shuffle: bool) -> Command {
    match (partial, whole) {
        (false, _) => Command::PlayRelease { id, start: None, shuffle },
        (true, true) => Command::PlayReleaseWhole { id, start: None, shuffle },
        (true, false) => Command::PlayReleaseMine { id, start: None, shuffle },
    }
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
                ui.label(TypeRole::LabelLarge.style().rich(if a.card.character { "ПЕРСОНАЖ" } else { "ИСПОЛНИТЕЛЬ" }, s.primary));
                // The name in Latin letters, when its own is in a script
                // the user may not read.
                if let Some(latin) = &a.card.latin {
                    ui.label(TypeRole::TitleMedium.style().rich(latin.as_str(), s.on_surface));
                }
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
            super::streams::album_card(ui, a, w, super::streams::DISCOGRAPHY, wishlist, out)
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
        // «Слушать», «Перемешать», «С дополнением» (new songs join the
        // playlist as it plays: a tonal toggle), «Добавить треки», ⋮ — on
        // one line: short of room «Добавить треки», then «Слушать», become
        // icons (the toggle never goes to a line of its own).
        let plus = state.prefs.plus_playlists.contains(&id);
        let (compact_add, compact_play) = action_room(ui, !liked);
        let mut members: Vec<Member> = vec![
            if compact_play {
                IconButton::new(Icon::PlayArrow).size(Size::M).style(Style::Filled).tooltip("Слушать").into()
            } else {
                Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into()
            },
            widgets::shuffle_member(ui),
            plus_toggle(plus).into(),
        ];
        if !liked {
            members.push(if compact_add {
                IconButton::new(Icon::Add).size(Size::M).style(Style::Outlined).tooltip("Добавить треки").into()
            } else {
                Button::new("Добавить треки").icon(Icon::Add).size(Size::M).style(Style::Outlined).into()
            });
        }
        members.push(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into());
        let row = ButtonRow::new(("pl-actions", id), Size::M).show(ui, members);
        let r = &row.responses;
        if r[0].clicked() {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: false });
        }
        if r[1].clicked() {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: true });
        }
        if r[2].clicked() {
            out.push(Command::SetPref(nikgob_view::PrefPatch::Plus { playlist: id, on: !plus }));
        }
        if !liked && r[3].clicked() {
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
                // Made in a service at once: asked first.
                Some(widgets::PlaylistAct::Link) => pages.confirm_link = Some(id),
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
                "",
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

/// «С дополнением» of a playlist's page: a
/// tonal icon toggle, on while new songs join the playlist as it plays.
fn plus_toggle(on: bool) -> IconButton {
    IconButton::new(Icon::AutoAwesome).size(Size::M).style(Style::Tonal).selected(on).tooltip("С дополнением")
}

/// Whether a playlist's actions fit on one line only with «Добавить
/// треки» (`add`: the playlist has it), then «Слушать» too, as icons:
/// (`add` as an icon, «Слушать» as an icon).
fn action_room(ui: &Ui, add: bool) -> (bool, bool) {
    let avail = ui.available_width();
    let gap = Size::M.group_gap();
    let icon = plus_toggle(false).natural_width();
    // «Перемешать» is an icon below 640 already ([`widgets::shuffle_member`]).
    let shuffle = if avail < 640.0 { icon } else { Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).natural_width(ui) };
    let play = Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).natural_width(ui);
    let add_w = Button::new("Добавить треки").icon(Icon::Add).size(Size::M).natural_width(ui);
    let n = if add { 5.0 } else { 4.0 };
    // «Слушать», «Перемешать», the toggle and ⋮ (and «Добавить треки»).
    let need = |play: f32, add_w: f32| play + shuffle + 2.0 * icon + if add { add_w } else { 0.0 } + gap * (n - 1.0);
    if need(play, add_w) <= avail {
        (false, false)
    } else if need(play, icon) <= avail {
        (true, false)
    } else {
        (true, true)
    }
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
        // «С новым»: the new songs it plays are its rows too.
        let new_rows: &[nikgob_view::StreamTrackRow] = if with_new { &l.new_tracks } else { &[] };
        let n = (l.tracks.len() + new_rows.len()) as u64;
        let total: std::time::Duration = l.tracks.iter().filter_map(|t| t.duration).chain(new_rows.iter().filter_map(|t| t.duration)).sum();
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
        let items = super::cards::mix_items(l, can_blend, false, true);
        if let Some(k) = super::cards::mix_menu(Id::new("mix-more"), l, &items).attach(&r[2]) {
            nav = super::cards::mix_do(ui.ctx(), l, items[k], world, out);
            // Left out: back to where it was offered.
            if items[k] == super::cards::MixItem::Hide {
                pages.go_back = true;
            }
        }
        ui.add_space(16.0);
        if l.tracks.is_empty() && new_rows.is_empty() {
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
        let nav = widgets::track_list_ext(ui, "mix", &l.tracks, Lead::Cover, &from, TrackListOpts::default(), state, pages, out).or(nav);
        if new_rows.is_empty() {
            return nav;
        }
        // The new songs under its own, in the order they play.
        let from = super::streams::StreamFrom::Rows(l.context());
        super::streams::track_rows(ui, "mix-new", new_rows, false, &from, state, pages, out).or(nav)
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
                "Ты попросил его не предлагать. Вернуть можно в Настройках → «Вид» → «Главная».",
                "На главную",
                Route::Home,
            ),
            (None, None) => ("Миксы на главной обновились.", "На главную", Route::Home),
        };
        widgets::empty_state(ui, Icon::QueueMusic, "Этого микса больше нет", body, Some(action)).then_some(to)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actions of a playlist's page shorten «Добавить треки», then
    /// «Слушать», to icons as the page narrows; «Любимые» (no «Добавить
    /// треки») keeps «Слушать» longer.
    #[test]
    fn the_actions_shorten_as_the_page_narrows() {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::Theme::default().install(&ctx);
        let at = |width: f32, add: bool| {
            let mut got = (false, false);
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 700.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                ui.allocate_ui(vec2(width, 100.0), |ui| got = action_room(ui, add));
            });
            out.textures_delta.clear();
            got
        };
        assert_eq!(at(900.0, true), (false, false));
        assert_eq!(at(450.0, true), (true, false));
        assert_eq!(at(340.0, true), (true, true));
        assert_eq!(at(340.0, false), (false, false));
        assert_eq!(at(300.0, false), (true, true));
    }

    /// «Слушать» and «Перемешать» of an album's page play the chip it
    /// shows: «Весь альбом» all of it, «Мои треки» the user's songs.
    #[test]
    fn the_page_plays_what_its_chip_shows() {
        let id = nikgob_view::ReleaseId::new();
        for shuffle in [false, true] {
            assert_eq!(page_play(id, true, true, shuffle), Command::PlayReleaseWhole { id, start: None, shuffle });
            assert_eq!(page_play(id, true, false, shuffle), Command::PlayReleaseMine { id, start: None, shuffle });
            assert_eq!(page_play(id, false, false, shuffle), Command::PlayRelease { id, start: None, shuffle });
        }
    }
}
