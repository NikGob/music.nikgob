//! Building blocks shared by pages: section headers, horizontal card rows,
//! cards with their context menus, track lists, shape-cropped avatars.

use egui::{
    Color32, Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};
use nikgob_shapes::MaterialShape;
use std::collections::HashMap;

use nikgob_view::{
    ArtistCard, Command, LIKED_PLAYLIST, LikeTarget, PlaylistCard, PlaylistId, ReleaseCard, ReleaseId,
    ReleaseLine, Route, SourceKind, Status, TrackId, TrackRow, ViewState, WishlistScope, WishlistStatus, WishlistView,
};

use super::{Ask, PageState};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Equalizer, IconButton, Leading, ListItem, MediaCard, Menu, MenuItem, Radii, Size, Style,
        anim, paint,
    },
    screens::{artist_links, common},
    tokens::shape,
};

pub const CARD_W: f32 = 160.0;
pub const CARD_GAP: f32 = 12.0;
pub const SECTION_GAP: f32 = 32.0;

/// A quiet line of text with one button after it, centred on the line;
/// under it when the line is long for the width. `button`: its label, icon,
/// style and id. True when the button was clicked.
pub fn quiet_line(ui: &mut Ui, text: &str, button: Option<(&str, Option<Icon>, Style, &str)>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let text_w = ui
        .painter()
        .layout_no_wrap(text.to_owned(), TypeRole::BodyMedium.font_id(), s.on_surface_variant)
        .size()
        .x;
    // The button: its label, an icon and the padding.
    let button_w = button.map_or(0.0, |(label, icon, ..)| {
        let label_w = ui.painter().layout_no_wrap(label.to_owned(), TypeRole::LabelLarge.font_id(), s.primary).size().x;
        label_w + if icon.is_some() { 64.0 } else { 48.0 }
    });
    let mut clicked = false;
    let mut add_button = |ui: &mut Ui| {
        if let Some((label, icon, style, salt)) = button {
            let mut b = crate::components::Button::new(label).style(style).id_salt(salt);
            if let Some(i) = icon {
                b = b.icon(i);
            }
            clicked = ui.add(b).clicked();
        }
    };
    if text_w + button_w + 8.0 <= ui.available_width() {
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(TypeRole::BodyMedium.style().rich(text, s.on_surface_variant));
            add_button(ui);
        });
    } else {
        ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface_variant));
        add_button(ui);
    }
    clicked
}

/// «Перемешать» of a page's action row: a button, or in a narrow page an
/// icon button, so the row keeps to one line.
pub fn shuffle_member(ui: &Ui) -> crate::components::Member {
    if ui.available_width() < 640.0 {
        IconButton::new(Icon::Shuffle).size(Size::M).style(Style::Tonal).tooltip("Перемешать").into()
    } else {
        crate::components::Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Tonal).into()
    }
}

/// Title-large section header with an optional "Все →" text button.
/// Returns true when "Все" is clicked.
pub fn section_header(ui: &mut Ui, title: &str, all: bool) -> bool {
    section_header_with(ui, title, all.then_some(("Все", Icon::ArrowForward)))
}

/// [`section_header`] with its own text button on the right («Слушать
/// всё» with ▶). True when it is clicked.
pub fn section_header_with(ui: &mut Ui, title: &str, action: Option<(&str, Icon)>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    // A fixed-height row so the title and the 40 dp button share a centre
    // (`horizontal` would top-align the title in a shorter row).
    let row = egui::vec2(ui.available_width(), 40.0);
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
        if let Some((label, icon)) = action {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui
                    .add(
                        crate::components::Button::new(label)
                            .style(crate::components::Style::Text)
                            .icon(icon)
                            .id_salt(("all", title)),
                    )
                    .clicked();
            });
        }
    });
    ui.add_space(8.0);
    clicked
}

/// A horizontally scrolling row of cards.
pub fn card_row(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::horizontal()
        .id_salt(id)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = CARD_GAP;
                add(ui);
            });
        });
}

/// `text` in `role` and `color`, wrapped to `width` in at most `rows` rows
/// (an ellipsis after the last).
pub fn wrapped(ui: &Ui, text: &str, role: crate::TypeStyle, color: Color32, width: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), role.text_format(color));
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    crate::typography::layout(ui.painter(), job)
}

/// "3 из 12" when a release is incomplete locally.
pub fn progress_badge(r: &ReleaseCard) -> Option<String> {
    match r.total_tracks {
        Some(t) if r.local_tracks < t => Some(format!("{} из {}", r.local_tracks, t)),
        _ => None,
    }
}

/// The badge on an album card: "стрим" when the library has none of its
/// files (it is liked or in a playlist from a service), else "3 из 12".
pub fn card_badge(r: &ReleaseCard) -> Option<String> {
    if r.local_tracks == 0 {
        Some("стрим".to_owned())
    } else {
        progress_badge(r)
    }
}

pub fn release_subtitle(r: &ReleaseCard) -> String {
    match r.year {
        Some(y) => format!("{} · {}", r.artist, y),
        None => r.artist.clone(),
    }
}

/// An album card; navigates on click, plays on ▶; right click and its ⋮
/// open the menu.
pub fn release_card(
    ui: &mut Ui,
    r: &ReleaseCard,
    width: f32,
    row: &str,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let o = MediaCard::new(&r.title)
        .subtitle(release_subtitle(r))
        .cover_opt(r.cover.as_ref().map(|c| c.uri()))
        .badge_opt(card_badge(r))
        .menu_button(true)
        .width(width)
        .id_salt(("release-card", row, r.id))
        .show(ui);
    let menu = release_menus(&o.response, o.menu.as_ref(), r, out);
    if o.play_clicked {
        out.push(Command::PlayRelease {
            id: r.id,
            start: None,
            shuffle: false,
        });
        None
    } else if o.card_clicked {
        Some(Route::Release(r.id))
    } else {
        menu
    }
}

/// The like of an album: into (or out of) «Любимые альбомы».
pub fn album_like_label(liked: bool) -> &'static str {
    super::like_label(liked)
}

//// What the menu of an album does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseAct {
    Play,
    Shuffle,
    Next,
    Queue,
    Playlist,
    Like,
    Artist,
    Share,
    /// «Скачать недостающие»: the tracks it lacks to «Хочу скачать».
    Download,
    /// «Найти качество лучше»: a better copy of its files that can get one.
    Better,
    Trash,
    Purge,
}

/// The one list of an album's actions, for its right-click menu and ⋮
/// (card or row) and the ⋮ of its page: play │ the like, a playlist │ the
/// artist, share │ what it lacks │ the deletes. On the page (`on_page`)
/// «Слушать», «Перемешать», the like and the download are buttons beside
/// it and are left out; «…навсегда» only while it has files.
pub fn release_rows(r: &ReleaseCard, on_page: bool) -> Vec<Row<ReleaseAct>> {
    use ReleaseAct as A;
    let mut rows = Vec::new();
    if !on_page {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
    }
    rows.push(Row::Item(A::Next, MenuItem::new(super::PLAY_NEXT).icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic)));
    rows.push(Row::Divider);
    if !on_page {
        rows.push(Row::Item(A::Like, MenuItem::new(album_like_label(r.liked)).icon(Icon::Favorite).selected(r.liked)));
    }
    rows.push(Row::Item(A::Playlist, MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd)));
    rows.push(Row::Divider);
    if !r.artists.linked().is_empty() {
        rows.push(Row::Item(A::Artist, MenuItem::new("К исполнителю").icon(Icon::Person)));
    }
    rows.push(Row::Item(A::Share, MenuItem::new("Поделиться").icon(Icon::Share)));
    let download = !on_page && progress_badge(r).is_some();
    let better = r.better && r.local_tracks > 0;
    if download || better {
        rows.push(Row::Divider);
    }
    if download {
        rows.push(Row::Item(A::Download, MenuItem::new("Скачать недостающие").icon(Icon::Download)));
    }
    if better {
        rows.push(Row::Item(A::Better, MenuItem::new(super::FIND_BETTER).icon(Icon::Search)));
    }
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Trash, MenuItem::new("Удалить альбом").icon(Icon::Delete)));
    if r.local_tracks > 0 {
        rows.push(Row::Item(A::Purge, MenuItem::new("Удалить альбом навсегда").icon(Icon::Delete)));
    }
    rows
}

/// Does an album action (the page opens «В плейлист…» with its tracks).
pub fn release_apply(ctx: &egui::Context, act: ReleaseAct, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    match act {
        ReleaseAct::Play => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
        ReleaseAct::Shuffle => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: true }),
        ReleaseAct::Next => out.push(Command::PlayReleaseNext(r.id)),
        ReleaseAct::Queue => out.push(Command::EnqueueRelease(r.id)),
        ReleaseAct::Like => out.push(Command::Like(LikeTarget::Release(r.id), !r.liked)),
        // Several artists: a chooser.
        ReleaseAct::Artist => return artist_links::go_to(ctx, artist_links::library(&r.artists)),
        ReleaseAct::Share => out.push(Command::Share(nikgob_view::ShareTarget::Release(r.id))),
        ReleaseAct::Download => out.push(Command::DownloadRelease(r.id)),
        ReleaseAct::Better => out.push(Command::FindBetterAlbum(r.id)),
        ReleaseAct::Trash => super::request_release_delete(ctx, r.id, &r.title, false),
        ReleaseAct::Purge => super::request_release_delete(ctx, r.id, &r.title, true),
        ReleaseAct::Playlist => {
            let anchor = ctx.input(|i| i.pointer.latest_pos()).map(|p| egui::Rect::from_center_size(p, egui::vec2(1.0, 1.0)));
            super::request_release_to_playlist(ctx, r.id, anchor);
        }
    }
    None
}

/// Right-click menu of an album (card or row): [`release_rows`].
pub fn release_menu(resp: &Response, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    release_menus(resp, None, r, out)
}

/// The menu of an album on a right click on `resp` and on its ⋮ (`more`):
/// the same [`release_rows`].
pub fn release_menus(resp: &Response, more: Option<&Response>, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    // On its artist's page «К исполнителю» leads nowhere.
    let here = super::here(&resp.ctx);
    let rows = || super::retain_rows(release_rows(r, false), &|a: ReleaseAct| a != ReleaseAct::Artist || !here.artists(&r.artists).is_empty());
    let mut act = None;
    if let Some(m) = more {
        let (menu, acts) = menu_of(("release-more", m.id), rows());
        act = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied());
    }
    if !Menu::context_idle(resp, ("release-menu", resp.id)) {
        let (menu, acts) = menu_of(("release-menu", resp.id), rows());
        act = act.or(menu.context_menu(resp).and_then(|i| acts.get(i).copied()));
    }
    release_apply(&resp.ctx, act?, r, out)
}

/// A playlist card; with `pages` its right-click menu can rename and delete.
pub fn playlist_card_with(
    ui: &mut Ui,
    p: &PlaylistCard,
    width: f32,
    row: &str,
    pages: Option<&mut PageState>,
    out: &mut Vec<Command>,
) -> Option<Route> {
    // A playlist that follows a service says which, in words.
    let mut subtitle = format!(
        "{} {}",
        p.track_count,
        common::plural(u64::from(p.track_count), "трек", "трека", "треков")
    );
    if let Some(s) = p.service {
        subtitle = format!("{} · {subtitle}", s.display_name());
    }
    let o = MediaCard::new(&p.name)
        .subtitle(subtitle)
        .cover_opt(p.cover.as_ref().map(|c| c.uri()))
        .placeholder_icon(if p.pinned { Icon::Favorite } else { Icon::QueueMusic })
        .menu_button(true)
        .width(width)
        .id_salt(("playlist-card", row, p.id))
        .show(ui);
    playlist_menus(&o.response, o.menu.as_ref(), p, pages, out);
    if o.play_clicked {
        out.push(Command::PlayPlaylist {
            id: p.id,
            start: None,
            shuffle: false,
        });
        None
    } else if o.card_clicked {
        Some(Route::Playlist(p.id))
    } else {
        None
    }
}

/// One row of a menu built from actions: an item standing for `A`, a
/// divider, a group label (not an item: nothing to pick), or a row that
/// opens a submenu of rows (one level).
#[derive(Clone, Debug)]
pub enum Row<A> {
    Item(A, MenuItem),
    Divider,
    Label(String),
    Sub(MenuItem, Vec<Row<A>>),
}

impl<A> Row<A> {
    /// The same row for another kind of action (a submenu's rows too).
    pub fn map<B>(self, f: &mut impl FnMut(A) -> B) -> Row<B> {
        match self {
            Row::Item(a, item) => Row::Item(f(a), item),
            Row::Divider => Row::Divider,
            Row::Label(l) => Row::Label(l),
            Row::Sub(item, rows) => Row::Sub(item, rows.into_iter().map(|r| r.map(&mut *f)).collect()),
        }
    }
}

/// The actions of `rows` in item order (what a picked index stands for),
/// a submenu's in its place.
pub fn row_acts<A: Copy>(rows: &[Row<A>]) -> Vec<A> {
    let mut acts = Vec::new();
    for r in rows {
        match r {
            Row::Item(a, _) => acts.push(*a),
            Row::Divider | Row::Label(_) => {}
            Row::Sub(_, rows) => acts.extend(row_acts(rows)),
        }
    }
    acts
}

/// The items of `rows` in item order with their actions, a submenu's in
/// its place.
#[cfg(test)]
pub fn row_items<A: Copy>(rows: &[Row<A>]) -> Vec<(A, &MenuItem)> {
    let mut items = Vec::new();
    for r in rows {
        match r {
            Row::Item(a, item) => items.push((*a, item)),
            Row::Divider | Row::Label(_) => {}
            Row::Sub(_, rows) => items.extend(row_items(rows)),
        }
    }
    items
}

/// The rows of the submenu `label` of `rows`.
#[cfg(test)]
pub fn sub_rows<'a, A>(rows: &'a [Row<A>], label: &str) -> Option<&'a [Row<A>]> {
    rows.iter().find_map(|r| match r {
        Row::Sub(item, rows) if item.label == label => Some(rows.as_slice()),
        _ => None,
    })
}

/// The rows of the submenu `label` of `rows`, to add to.
pub fn sub_rows_mut<'a, A>(rows: &'a mut [Row<A>], label: &str) -> Option<&'a mut Vec<Row<A>>> {
    rows.iter_mut().find_map(|r| match r {
        Row::Sub(item, rows) if item.label == label => Some(rows),
        _ => None,
    })
}

/// `rows` as a menu shows them: a submenu with no item is left out, the
/// one item of a submenu stands in the menu in its place.
pub fn shown_rows<A>(rows: Vec<Row<A>>) -> Vec<Row<A>> {
    let mut shown = Vec::with_capacity(rows.len());
    for r in rows {
        match r {
            Row::Sub(parent, rows) => {
                let rows = shown_rows(rows);
                match rows.iter().filter(|r| matches!(r, Row::Item(..))).count() {
                    0 => {}
                    1 => shown.extend(rows.into_iter().filter(|r| matches!(r, Row::Item(..)))),
                    _ => shown.push(Row::Sub(parent, rows)),
                }
            }
            r => shown.push(r),
        }
    }
    shown
}

/// A menu of `rows` ([`shown_rows`]) and what each of its item indices
/// stands for. A divider at the start, at the end or after another is left
/// out.
pub fn menu_of<A: Copy>(key: impl egui::AsId, rows: Vec<Row<A>>) -> (Menu, Vec<A>) {
    let rows = shown_rows(rows);
    let acts = row_acts(&rows);
    (fill(Menu::new(key), rows), acts)
}

fn fill<A>(mut menu: Menu, rows: Vec<Row<A>>) -> Menu {
    let mut after_divider = true;
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        match r {
            Row::Item(_, item) => {
                menu = menu.item(item);
                after_divider = false;
            }
            Row::Sub(item, rows) => {
                menu = menu.item(item.submenu(fill(Menu::new("submenu"), rows)));
                after_divider = false;
            }
            Row::Label(text) => {
                menu = menu.group_label(text);
                after_divider = false;
            }
            Row::Divider => {
                if !after_divider && i + 1 < n {
                    menu = menu.divider();
                    after_divider = true;
                }
            }
        }
    }
    menu
}

/// What the menu of a playlist does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistAct {
    Play,
    Shuffle,
    Next,
    Queue,
    Edit,
    Sync(nikgob_view::PlaylistSyncMode),
    Link,
    Delete,
}

/// Where a playlist's menu is and what it may offer.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaylistMenuOf<'a> {
    /// The ⋮ of the playlist's page: «Слушать» and «Перемешать» are its
    /// buttons, so the menu leaves them out.
    pub on_page: bool,
    /// Can be renamed (edited) and deleted.
    pub editable: bool,
    /// Linked: the service's name and how it syncs.
    pub sync: Option<(&'a str, nikgob_view::PlaylistSyncMode)>,
    /// Not linked, and may be sent to a service.
    pub can_link: bool,
}

/// The sync mode as a choice of a linked playlist's menu: each says what
/// it does; «Не синхронизировать» (the link stays, nothing syncs) stops it.
pub fn sync_menu_label(m: nikgob_view::PlaylistSyncMode) -> &'static str {
    use nikgob_view::PlaylistSyncMode as M;
    match m {
        M::TwoWay => "Изменения в обе стороны",
        M::Follow => "Только получать изменения",
        M::Once => "Не синхронизировать",
    }
}

/// The sync mode in the playlist page's line under the title.
pub fn sync_state_label(m: nikgob_view::PlaylistSyncMode) -> &'static str {
    use nikgob_view::PlaylistSyncMode as M;
    match m {
        M::TwoWay => "изменения в обе стороны",
        M::Follow => "только получение изменений",
        M::Once => "не синхронизируется",
    }
}

/// The one list of a playlist's actions, for its right-click menu and ⋮
/// (card or row) and the ⋮ of its page: play, next, queue │ edit │ sync │
/// what cannot simply be taken back, last: the link to a service (asked
/// first) and the delete.
pub fn playlist_rows(o: PlaylistMenuOf<'_>) -> Vec<Row<PlaylistAct>> {
    use PlaylistAct as A;
    let mut rows = Vec::new();
    if !o.on_page {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
    }
    rows.push(Row::Item(A::Next, MenuItem::new(super::PLAY_NEXT).icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic)));
    if o.editable {
        rows.push(Row::Divider);
        // The page edits the name and the cover; a card renames.
        let edit = if o.on_page { "Изменить…" } else { "Переименовать…" };
        rows.push(Row::Item(A::Edit, MenuItem::new(edit).icon(Icon::Edit)));
    }
    if let Some((service, mode)) = o.sync {
        rows.push(Row::Divider);
        rows.push(Row::Label(format!("Синхронизация с {service}")));
        for &m in nikgob_view::PlaylistSyncMode::ALL {
            rows.push(Row::Item(A::Sync(m), MenuItem::new(sync_menu_label(m)).icon(Icon::Sync).selected(m == mode)));
        }
    }
    let link = o.sync.is_none() && o.can_link;
    if link || o.editable {
        rows.push(Row::Divider);
    }
    if link {
        rows.push(Row::Item(A::Link, MenuItem::new("Связать с сервисом…").icon(Icon::Sync)));
    }
    if o.editable {
        rows.push(Row::Item(A::Delete, MenuItem::new("Удалить плейлист").icon(Icon::Delete)));
    }
    rows
}

/// The command of a playlist action that needs no dialog (not «Изменить»,
/// «Связать с сервисом…» or «Удалить»).
pub fn playlist_command(act: PlaylistAct, id: PlaylistId) -> Option<Command> {
    Some(match act {
        PlaylistAct::Play => Command::PlayPlaylist { id, start: None, shuffle: false },
        PlaylistAct::Shuffle => Command::PlayPlaylist { id, start: None, shuffle: true },
        PlaylistAct::Next => Command::PlayPlaylistNext(id),
        PlaylistAct::Queue => Command::EnqueuePlaylist(id),
        PlaylistAct::Sync(mode) => Command::SetPlaylistSync { playlist: id, mode },
        PlaylistAct::Edit | PlaylistAct::Link | PlaylistAct::Delete => return None,
    })
}

/// «Связать с сервисом…» once it is confirmed: the playlist is made in a
/// service and synced both ways.
pub fn playlist_link_command(id: PlaylistId) -> Command {
    Command::SetPlaylistSync { playlist: id, mode: nikgob_view::PlaylistSyncMode::TwoWay }
}

/// Right-click menu of a playlist (card or row): [`playlist_rows`]; with
/// `pages` it can rename and delete (confirmed in a dialog). The liked
/// playlist can be neither renamed nor deleted.
pub fn playlist_menu_at(
    resp: &Response,
    p: &PlaylistCard,
    pages: Option<&mut PageState>,
    out: &mut Vec<Command>,
) {
    playlist_menus(resp, None, p, pages, out);
}

/// [`playlist_menu_at`] on a right click on `resp` and on its ⋮ (`more`).
pub fn playlist_menus(
    resp: &Response,
    more: Option<&Response>,
    p: &PlaylistCard,
    pages: Option<&mut PageState>,
    out: &mut Vec<Command>,
) {
    let editable = pages.is_some() && !p.pinned && p.id != LIKED_PLAYLIST;
    let of = PlaylistMenuOf { editable, ..Default::default() };
    let mut act = None;
    if let Some(m) = more {
        let (menu, acts) = menu_of(("playlist-more", m.id), playlist_rows(of));
        act = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied());
    }
    if !Menu::context_idle(resp, ("playlist-menu", resp.id)) {
        let (menu, acts) = menu_of(("playlist-menu", resp.id), playlist_rows(of));
        act = act.or(menu.context_menu(resp).and_then(|i| acts.get(i).copied()));
    }
    let Some(act) = act else { return };
    match act {
        PlaylistAct::Edit => {
            if let Some(pages) = pages {
                pages.ask_text = p.name.clone();
                pages.ask = Some(Ask::RenamePlaylist(p.id));
            }
        }
        PlaylistAct::Delete => {
            if let Some(pages) = pages {
                pages.confirm_delete = Some(p.id);
            }
        }
        a => out.extend(playlist_command(a, p.id)),
    }
}

// Shapes that artist avatars rotate through (formas only on media).
const AVATAR_SHAPES: [MaterialShape; 6] = [
    MaterialShape::Circle,
    MaterialShape::Cookie12Sided,
    MaterialShape::Clover8Leaf,
    MaterialShape::Cookie9Sided,
    MaterialShape::SoftBurst,
    MaterialShape::Flower,
];

/// Pick a shape from a stable hash so an artist keeps their shape.
pub fn avatar_shape(key: &impl std::hash::Hash) -> MaterialShape {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    let h = BuildHasherDefault::<DefaultHasher>::default().hash_one(key);
    AVATAR_SHAPES[(h % AVATAR_SHAPES.len() as u64) as usize]
}

/// An image (or initials on a tonal fill) cropped to a Material shape.
pub fn shape_image(ui: &Ui, uri: Option<&str>, initials: &str, rect: Rect, shape: MaterialShape) {
    let s = Theme::get(ui.ctx()).scheme;
    crate::components::paint_shape_avatar(ui, rect, uri, initials, shape, s.tertiary_container, s.on_tertiary_container);
}

/// The container and content colours of a scheme role.
pub fn role_colors(role: super::shelf::Role, s: &crate::ColorScheme) -> (Color32, Color32) {
    use super::shelf::Role;
    match role {
        Role::Primary => (s.primary_container, s.on_primary_container),
        Role::Secondary => (s.secondary_container, s.on_secondary_container),
        Role::Tertiary => (s.tertiary_container, s.on_tertiary_container),
        Role::Surface => (s.surface_container_highest, s.on_surface_variant),
    }
}

/// An artist's avatar: the photo in a circle, or the initials in the role
/// and shape the name picks ([`super::shelf::artist_look`]).
pub fn artist_picture(ui: &Ui, a: &ArtistCard, rect: Rect) {
    let s = Theme::get(ui.ctx()).scheme;
    let uri = a.image.as_ref().map(|c| c.uri());
    let (role, shape) = super::shelf::artist_look(&a.name);
    let (fill, on) = role_colors(role, &s);
    let shape = if uri.is_some() { MaterialShape::Circle } else { shape };
    crate::components::paint_shape_avatar(ui, rect, uri.as_deref(), &initials(&a.name), shape, fill, on);
}

/// The list row's leading avatar of an artist, as [`artist_picture`].
pub fn artist_leading(ui: &Ui, a: &ArtistCard) -> Leading {
    let s = Theme::get(ui.ctx()).scheme;
    let uri = a.image.as_ref().map(|c| c.uri());
    let (role, shape) = super::shelf::artist_look(&a.name);
    let (fill, on) = role_colors(role, &s);
    Leading::Shape {
        shape: if uri.is_some() { MaterialShape::Circle } else { shape },
        uri,
        initials: initials(&a.name),
        fill,
        on,
    }
}

/// «12 треков» under an artist; empty when not counted.
pub fn artist_tracks(a: &ArtistCard) -> String {
    match a.track_count {
        0 => String::new(),
        n => format!("{n} {}", common::plural(u64::from(n), "трек", "трека", "треков")),
    }
}

/// "Boards of Canada" → "BC": first letters of the first and last words,
/// skipping lowercase particles (of, the, и) when there are capitalised ones.
pub fn initials(name: &str) -> String {
    let words: Vec<char> = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .collect();
    let caps: Vec<char> = words.iter().copied().filter(|c| !c.is_lowercase()).collect();
    let pick = if caps.is_empty() { &words } else { &caps };
    let letters: Vec<char> = match pick.len() {
        0 => Vec::new(),
        1 => vec![pick[0]],
        n => vec![pick[0], pick[n - 1]],
    };
    letters.into_iter().flat_map(char::to_uppercase).collect()
}

#[cfg(test)]
mod tests {
    use super::initials;

    /// The grid takes the same room whether its rows are laid out or not.
    #[test]
    fn grid_rows_out_of_sight_keep_their_room() {
        let height_with = |screen_h: f32| {
            let ctx = egui::Context::default();
            let mut h = 0.0;
            for i in 0..3 {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, screen_h))),
                    time: Some(f64::from(i) * 0.05),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    let items: Vec<u32> = (0..203).collect();
                    let top = ui.cursor().top();
                    let mut drawn = 0;
                    super::grid(ui, &items, 160.0, |_, w| w + 56.0, |ui, _, w| {
                        drawn += 1;
                        ui.allocate_exact_size(egui::vec2(w, w + 56.0), egui::Sense::hover());
                        None
                    });
                    h = ui.cursor().top() - top;
                    let _ = drawn;
                });
                out.textures_delta.clear();
            }
            h
        };
        assert_eq!(height_with(400.0), height_with(100_000.0));
    }

    #[test]
    fn initials_skip_particles() {
        assert_eq!(initials("Boards of Canada"), "BC");
        assert_eq!(initials("Кино"), "К");
        assert_eq!(initials("Molchat Doma"), "MD");
        assert_eq!(initials("the xx"), "TX");
        assert_eq!(initials("Nick Cave & the Bad Seeds"), "NS");
        assert_eq!(initials(""), "");
    }
}

/// An artist avatar card with a ▶ (plays the mix) and a ⋮ on hover; ⋮ and
/// a right click open its menu. A click opens the artist.
pub fn artist_avatar(
    ui: &mut Ui,
    a: &ArtistCard,
    size: f32,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    // In the library the name has the track count under it.
    let caption = artist_tracks(a);
    let text_h = if caption.is_empty() { 28.0 } else { 48.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size + text_h), Sense::click());
    // A row holds every artist of the library: the ones scrolled out of
    // sight (with room for the hover growth) cost nothing.
    if !ui.is_rect_visible(rect.expand(4.0)) {
        return None;
    }
    let img = Rect::from_min_size(rect.min, vec2(size, size));
    let id = ui.id().with(("artist-card", a.id));
    let menu_open = ui.ctx().data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
    let hover = resp.contains_pointer() || menu_open;
    let grow = anim::f32(
        ui.ctx(),
        ui.id().with(("artist-hover", a.id)),
        if hover { 4.0 } else { 0.0 },
        theme.motion.fast_spatial,
    );
    // A real photo is a circle; the initials keep their playful shape.
    artist_picture(ui, a, img.expand(grow * 0.5));
    // One line, cut with an ellipsis: a long name must not run into the
    // next row.
    let mut job = egui::text::LayoutJob::single_section(a.name.clone(), TypeRole::TitleSmall.style().text_format(s.on_surface));
    job.wrap = egui::text::TextWrapping::truncate_at_width(size.max(1.0));
    let galley = crate::typography::layout(ui.painter(), job);
    let name_rect = Rect::from_center_size(
        pos2(rect.center().x, img.bottom() + 6.0 + galley.size().y * 0.5),
        galley.size(),
    );
    ui.painter().galley(name_rect.min, galley, s.on_surface);
    if !caption.is_empty() {
        let mut job = egui::text::LayoutJob::single_section(caption, TypeRole::BodySmall.style().text_format(s.on_surface_variant));
        job.wrap = egui::text::TextWrapping::truncate_at_width(size.max(1.0));
        let g = crate::typography::layout(ui.painter(), job);
        let at = pos2(rect.center().x - g.size().x * 0.5, name_rect.bottom() + 2.0);
        ui.painter().galley(at, g, s.on_surface_variant);
    }

    // ▶ over the avatar, fading in with the pointer (fast effects).
    let k = anim::fraction(
        ui.ctx(),
        ui.id().with(("artist-play", a.id)),
        if hover { 1.0 } else { 0.0 },
        theme.motion.fast_effects,
    )
    .clamp(0.0, 1.0);
    let mut play = false;
    if k > 0.01 {
        let slot = Rect::from_center_size(img.center(), vec2(56.0, 56.0));
        // A child, not a scope: the overlay must not move the row's cursor.
        let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(("artist-play-slot", a.id)));
        child.set_opacity(k);
        play = child
            .add_enabled(
                k > 0.5,
                IconButton::new(Icon::PlayArrow)
                    .size(Size::M)
                    .style(Style::Filled)
                    .filled_icon(true)
                    .tooltip("Слушать микс")
                    .id_salt(("artist-play-btn", a.id)),
            )
            .clicked();
    }
    // ⋮ at the avatar's top right, as on the other cards.
    let more = super::cards::more_button(ui, id, hover, card_more_slot(img));
    super::cards::keep_more_open(ui.ctx(), id, more.as_ref());
    let menu = artist_menus(&resp, more.as_ref(), a, out);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if play {
        out.push(Command::PlayArtist { id: a.id, shuffle: false });
    }
    let more_clicked = more.as_ref().is_some_and(Response::clicked);
    (resp.clicked() && !play && !more_clicked).then_some(Route::Artist(a.id)).or(menu)
}

/// How tall [`artist_avatar`] is for `a` at `size` (the name, and in the
/// library the track count under it).
pub fn artist_avatar_height(a: &ArtistCard, size: f32) -> f32 {
    size + if a.track_count == 0 { 28.0 } else { 48.0 }
}

/// Where a round or square cover's ⋮ stands: inside its top right.
pub fn card_more_slot(cover: Rect) -> Rect {
    let m = Size::S.height();
    Rect::from_min_size(pos2(cover.right() - 4.0 - m, cover.top() + 4.0), vec2(m, m))
}

/// What the menu of an artist does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtistAct {
    Play,
    Shuffle,
    Radio,
    Queue,
    Follow,
    Ban,
}

/// The one list of an artist's actions, for its right-click menu (card or
/// row) and the ⋮ of its page. On the page (`on_page`) the mix, shuffle,
/// radio and «Подписаться» are buttons beside it and are left out.
pub fn artist_rows(a: &ArtistCard, on_page: bool, marks: &nikgob_view::DislikesView) -> Vec<Row<ArtistAct>> {
    use ArtistAct as A;
    let mut rows = Vec::new();
    if !on_page {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать микс").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
        rows.push(Row::Item(A::Radio, MenuItem::new("Радио исполнителя").icon(Icon::Radio)));
    }
    rows.push(Row::Item(A::Queue, MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic)));
    rows.push(Row::Divider);
    if !on_page {
        rows.push(Row::Item(A::Follow, MenuItem::new(if a.liked { "Отписаться" } else { "Подписаться" }).icon(Icon::Favorite)));
    }
    rows.push(Row::Item(A::Ban, super::ban_item(a.id, marks)));
    rows
}

/// Does an artist action.
pub fn artist_apply(act: ArtistAct, a: &ArtistCard, marks: &nikgob_view::DislikesView, out: &mut Vec<Command>) {
    out.push(match act {
        ArtistAct::Play => Command::PlayArtist { id: a.id, shuffle: false },
        ArtistAct::Shuffle => Command::PlayArtist { id: a.id, shuffle: true },
        ArtistAct::Radio => Command::StartRadio { seed: nikgob_view::RadioSeed::Artist(a.id) },
        ArtistAct::Queue => Command::EnqueueArtist(a.id),
        ArtistAct::Follow => Command::Like(LikeTarget::Artist(a.id), !a.liked),
        ArtistAct::Ban => super::ban_command(a.id, marks),
    });
}

/// Right-click menu of an artist (card or row): [`artist_rows`].
pub fn artist_menu(resp: &Response, a: &ArtistCard, out: &mut Vec<Command>) {
    artist_menus(resp, None, a, out);
}

/// The menu of an artist on a right click on `resp` and on its ⋮
/// (`more`): the same [`artist_rows`].
fn artist_menus(resp: &Response, more: Option<&Response>, a: &ArtistCard, out: &mut Vec<Command>) -> Option<Route> {
    let marks = super::marks(&resp.ctx);
    let mut act = None;
    if let Some(m) = more {
        let (menu, acts) = menu_of(("artist-more", m.id), artist_rows(a, false, &marks));
        act = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied());
    }
    if !Menu::context_idle(resp, ("artist-menu", resp.id)) {
        let (menu, acts) = menu_of(("artist-menu", resp.id), artist_rows(a, false, &marks));
        act = act.or(menu.context_menu(resp).and_then(|i| acts.get(i).copied()));
    }
    artist_apply(act?, a, &marks, out);
    None
}

/// Where a track list plays from.
#[derive(Clone, Debug)]
pub enum PlayFrom {
    Release(nikgob_view::ReleaseId),
    Playlist(nikgob_view::PlaylistId),
    Tracks(Vec<nikgob_view::TrackId>, String),
    /// The merged «Всё» search: a queue of library and stream tracks,
    /// started at `at` (the row's own place in it).
    Search {
        picks: Vec<nikgob_view::SearchPick>,
        at: usize,
        context: String,
    },
}

/// How the leading column looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lead {
    Number,
    Cover,
}

/// Extras of [`track_list_ext`].
#[derive(Clone, Copy, Default)]
pub struct TrackListOpts<'a> {
    /// The list is this playlist: ⋮ gets "Убрать из плейлиста".
    pub playlist: Option<PlaylistId>,
    /// Rows can be reordered by their drag handle (user playlists).
    pub reorder: bool,
    /// A track with a file says «у тебя» (among the services' rows of the
    /// merged search).
    pub mark_local: bool,
    /// A track without a file is drawn quieter (`on-surface-variant`) and
    /// one that streams says «стрим» (an album's songs the library lacks).
    pub mark_missing: bool,
    /// The lines to draw, in order (an album page: its tracks and the
    /// places no track has); none: every track.
    pub lines: Option<&'a [ReleaseLine]>,
}

/// Tracks of different artists on one release (a compilation).
fn is_compilation(tracks: &[TrackRow]) -> bool {
    tracks
        .first()
        .is_some_and(|f| tracks.iter().any(|t| t.artist != f.artist))
}

/// A track list. Double-click (or click on the number) plays from that
/// track; ⋮ opens the track menu. Returns a route to follow.
#[allow(clippy::too_many_arguments)]
pub fn track_list(
    ui: &mut Ui,
    id: &str,
    tracks: &[TrackRow],
    lead: Lead,
    from: &PlayFrom,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    track_list_ext(ui, id, tracks, lead, from, TrackListOpts::default(), state, pages, out)
}

/// [`track_list`] with playlist extras. Rows are 56 (one line) or 72 (two
/// lines: compilations and lists with covers); the playing row is
/// `secondary-container` with a mini equalizer instead of the number. A
/// long list (a big «Любимые») lays out only the rows on screen.
#[allow(clippy::too_many_arguments)]
pub fn track_list_ext(
    ui: &mut Ui,
    id: &str,
    tracks: &[TrackRow],
    lead: Lead,
    from: &PlayFrom,
    opts: TrackListOpts<'_>,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    let two_line = lead == Lead::Cover || is_compilation(tracks);
    let (mut dragging, mut dropped) = (None, None);
    let every: Vec<ReleaseLine>;
    let lines = match opts.lines {
        Some(l) => l,
        None => {
            every = (0..tracks.len()).map(ReleaseLine::Track).collect();
            &every
        }
    };
    let disc_of = |l: &ReleaseLine| match *l {
        ReleaseLine::Track(i) => tracks[i].disc_no.max(1),
        ReleaseLine::Hole { disc, .. } => disc.max(1),
    };
    // An album on several discs numbers each disc from 1: a "Диск N" line
    // above each, or the numbers would read as broken.
    let discs = lead == Lead::Number && lines.iter().any(|l| disc_of(l) != disc_of(&lines[0]));
    let wish = WishIndex::of(ui.ctx(), &state.downloads.wishlist);
    let album = match from {
        PlayFrom::Release(r) => Some(*r),
        _ => None,
    };
    let mut row = |ui: &mut Ui, j: usize, nav: &mut Option<Route>| -> Option<Response> {
        let i = match lines[j] {
            ReleaseLine::Track(i) => i,
            ReleaseLine::Hole { no, .. } => return Some(hole_row(ui, id, j, no, album, &wish, state, out)),
        };
        let t = &tracks[i];
        let o = track_row(ui, id, i, t, lead, two_line, opts, &wish, state, out);
        if let Some(r) = o.link.clone() {
            *nav = Some(r);
        }
        if let Some(h) = &o.handle {
            if h.dragged() {
                dragging = Some(i);
            }
            if h.drag_stopped() {
                dropped = Some(i);
            }
        }
        let is_cur = state.player.current.as_ref().map(|c| c.id) == Some(t.id);
        // Desktop convention: double-click plays; on the playing row it
        // pauses. The ▶ on the cover / number plays with one click.
        if o.response.double_clicked() || o.play_clicked {
            out.push(if is_cur { Command::TogglePlay } else { play_from(from, tracks, i) });
        }
        let menu_key = Id::new(("pl-track-menu", id, i));
        match opts.playlist {
            Some(pl) => {
                if let Some(m) = &o.more {
                    let (menu, acts) = playlist_track_menu(&m.ctx, menu_key, t, pl, state, &wish);
                    let act = menu.attach(m).and_then(|i| acts.get(i).copied());
                    *nav = playlist_track_pick(&m.ctx, act, t, pl, Some(m.rect), pages, state, out).or(nav.take());
                }
                let act = if Menu::context_idle(&o.response, menu_key.with("ctx")) {
                    None
                } else {
                    let (menu, acts) = playlist_track_menu(&o.response.ctx, menu_key.with("ctx"), t, pl, state, &wish);
                    menu.context_menu(&o.response).and_then(|i| acts.get(i).copied())
                };
                *nav = playlist_track_pick(&o.response.ctx, act, t, pl, None, pages, state, out).or(nav.take());

            }
            None => {
                if let Some(m) = &o.more {
                    *nav = pages.track_menu_attach(m, t, state, out).or(nav.take());
                }
                *nav = pages.track_context_menu(&o.response, t, state, out).or(nav.take());
            }
        }
        Some(o.response)
    };
    let rects: Vec<Rect> = if discs {
        let mut rects = Vec::with_capacity(lines.len());
        for j in 0..lines.len() {
            let disc = disc_of(&lines[j]);
            if j == 0 || disc_of(&lines[j - 1]) != disc {
                ui.add_space(if j == 0 { 4.0 } else { 16.0 });
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(TypeRole::TitleSmall.style().rich(format!("Диск {disc}"), s.on_surface_variant));
                });
                ui.add_space(4.0);
            }
            if let Some(r) = row(ui, j, &mut nav) {
                rects.push(r.rect);
            }
        }
        rects
    } else {
        let row_h = if two_line { crate::components::list::TWO_LINE } else { crate::components::list::ONE_LINE };
        virtual_rows(ui, (id, "rows"), lines.len(), row_h, |ui, j| {
            row(ui, j, &mut nav);
        })
    };
    if let Some(pl) = opts.playlist.filter(|_| opts.reorder)
        && let Some(i) = dragging.or(dropped)
    {
        reorder(ui, pl, tracks, &rects, i, dragging.is_some(), out);
    }
    nav
}

/// A place of the album no track has: its number, «Трек 7», quiet, «нет
/// файла»; ⬇ asks «Хочу скачать» for what the album lacks (or says how far
/// it is).
#[allow(clippy::too_many_arguments)]
fn hole_row(
    ui: &mut Ui,
    id: &str,
    j: usize,
    no: u32,
    album: Option<ReleaseId>,
    wish: &WishIndex,
    state: &ViewState,
    out: &mut Vec<Command>,
) -> Response {
    let title = if no > 0 { format!("Трек {no}") } else { "Трек без номера".to_owned() };
    let wished = album.and_then(|r| wish.of_release(r));
    let leading = Leading::Number(if no > 0 { no.to_string() } else { String::new() });
    ListItem::new(title)
        .leading(leading)
        .muted(true)
        .enabled(true)
        .trailing(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Under the ⋮ of the track rows: ⬇ keeps their column.
                ui.add_space(Size::Xs.height());
                match (wished, album) {
                    (Some(st), _) => {
                        if wish_mark(ui, st, state.prefs.prowlarr_configured, ("hole-wish", id, j)).clicked() {
                            ui.ctx().data_mut(|d| d.insert_temp(Id::new(OPEN_WISHLIST), true));
                        }
                    }
                    (None, Some(r)) => {
                        let dl = IconButton::new(Icon::Download)
                            .size(Size::Xs)
                            .tooltip("Скачать недостающие — в «Хочу скачать»")
                            .id_salt(("hole-dl", id, j));
                        if ui.add(dl).clicked() {
                            out.push(Command::DownloadRelease(r));
                        }
                    }
                    (None, None) => {}
                }
                let variant = Theme::get(ui.ctx()).scheme.on_surface_variant;
                ui.label(TypeRole::LabelSmall.style().rich("нет файла", variant));
            });
        })
        .id_salt(("hole", id, j))
        .show(ui)
        .response
}

/// What a drawn track row reported.
struct RowOut {
    response: Response,
    /// An artist name or the album of the supporting line was clicked: its
    /// page.
    link: Option<Route>,
    handle: Option<Response>,
    more: Option<Response>,
    play_clicked: bool,
}

/// One row of [`track_list_ext`].
#[allow(clippy::too_many_arguments)]
fn track_row(
    ui: &mut Ui,
    id: &str,
    i: usize,
    t: &TrackRow,
    lead: Lead,
    two_line: bool,
    opts: TrackListOpts<'_>,
    wish: &WishIndex,
    state: &ViewState,
    out: &mut Vec<Command>,
) -> RowOut {
    let s = Theme::get(ui.ctx()).scheme;
    let current = state.player.current.as_ref().map(|t| t.id);
    let playing = state.player.status == Status::Playing;
    let is_cur = current == Some(t.id);
    let leading = match lead {
        Lead::Number if is_cur => Leading::Number(String::new()),
        Lead::Number => Leading::Number(if t.track_no > 0 { t.track_no.to_string() } else { (i + 1).to_string() }),
        Lead::Cover => Leading::Image(t.cover.as_ref().map(|c| c.uri()), Icon::MusicNote),
    };
    let mut supporting = t.artist.clone();
    // Each artist of the line is a link to its page, and so is the album.
    let (mut links, mut routes) = artist_links::library_spans(&t.artists, &t.artist);
    if lead == Lead::Cover
        && let Some(a) = &t.album
    {
        let at = artist_links::append_source(&mut supporting, a);
        if let Some(r) = t.release_id {
            links.push(at);
            routes.push(Route::Release(r));
        }
    }
    // A liked YouTube clip without its song: said, as search says it.
    if t.clip {
        supporting = format!("{supporting} · клип");
    }
    if opts.mark_local && t.has_local {
        supporting = format!("{supporting} · у тебя");
    }
    let dur = t.duration.map(common::fmt_time).unwrap_or_default();
    // No file: it streams (a ⬇ to «Хочу скачать», or how far it is there),
    // or nothing plays it at all («нет файла»).
    let streams = matches!(t.source, Some(SourceKind::Spotify | SourceKind::Youtube));
    let wished = (!t.has_local).then(|| wish.of_track(Some(t.id), t.release_id)).flatten();
    let track_id = t.id;
    let (liked, has_local) = (t.liked, t.has_local);
    let mut menu_resp: Option<Response> = None;
    let mut item = ListItem::new(&t.title)
        .leading(leading)
        .selected(is_cur)
        .muted(!has_local && (!streams || opts.mark_missing))
        .hover_play(!is_cur)
        .enabled(true)
        .trailing(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let m = ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("track-more", id, i)));
                menu_resp = Some(m);
                if !has_local {
                    match wished {
                        Some(st) => {
                            if wish_mark(ui, st, state.prefs.prowlarr_configured, ("track-wish", id, i)).clicked() {
                                ui.ctx().data_mut(|d| d.insert_temp(Id::new(OPEN_WISHLIST), true));
                            }
                        }
                        None => {
                            if ui
                                .add(
                                    IconButton::new(Icon::Download)
                                        .size(Size::Xs)
                                        .tooltip("Скачать — в «Хочу скачать»")
                                        .id_salt(("track-dl", id, i)),
                                )
                                .clicked()
                            {
                                out.push(Command::DownloadTracks(vec![track_id]));
                            }
                        }
                    }
                }
                if ui
                    .add(
                        IconButton::new(Icon::Favorite)
                            .size(Size::Xs)
                            .selected(liked)
                            .tooltip(super::like_label(liked))
                            .id_salt(("track-like", id, i)),
                    )
                    .clicked()
                {
                    out.push(Command::Like(LikeTarget::Track(track_id), !liked));
                }
                let variant = Theme::get(ui.ctx()).scheme.on_surface_variant;
                ui.label(TypeRole::BodyMedium.style().rich(dur, variant));
                if !has_local && !streams {
                    ui.label(TypeRole::LabelSmall.style().rich("нет файла", variant));
                } else if !has_local && opts.mark_missing {
                    ui.label(TypeRole::LabelSmall.style().rich("стрим", variant));
                }
            });
        })
        .id_salt(("track", id, i, t.id));
    if two_line {
        item = item.supporting(supporting).supporting_links(links);
    }
    if opts.reorder {
        item = item.drag_handle(true);
    }
    let o = item.show(ui);
    if is_cur {
        let (center, backdrop) = match lead {
            Lead::Number => (pos2(o.rect.left() + 16.0 + 14.0, o.rect.center().y), None),
            Lead::Cover => {
                let c = pos2(o.rect.left() + 16.0 + 28.0, o.rect.center().y);
                (c, Some(Rect::from_center_size(c, vec2(56.0, 56.0))))
            }
        };
        if let Some(b) = backdrop {
            paint::container(ui.painter(), b, Radii::all(shape::SMALL), s.secondary_container.gamma_multiply(0.85), Stroke::NONE);
        }
        Equalizer::new(("eq", id, i), s.on_secondary_container)
            .playing(playing)
            .paint_at(ui, Rect::from_center_size(center, vec2(24.0, 24.0)));
    }
    let link = o.link_clicked.and_then(|k| routes.get(k).cloned());
    RowOut { response: o.response, link, handle: o.handle, more: menu_resp, play_clicked: o.play_clicked }
}

/// Lays out `n` rows of `row_h` one under another (the page's item
/// spacing between them) and draws only those that meet the viewport.
/// Returns where every row is.
pub fn virtual_rows(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    n: usize,
    row_h: f32,
    mut draw: impl FnMut(&mut Ui, usize),
) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let gap = ui.spacing().item_spacing.y;
    let step = row_h + gap;
    let width = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(width, n as f32 * step - gap), Sense::hover());
    let rect_of = |i: usize| Rect::from_min_size(pos2(area.left(), area.top() + i as f32 * step), vec2(width, row_h));
    let clip = ui.clip_rect();
    // A row above and below the viewport stay drawn (focus, menus, hover).
    let first = (((clip.top() - area.top()) / step).floor().max(0.0) as usize).saturating_sub(1);
    let last = ((((clip.bottom() - area.top()) / step).ceil().max(0.0)) as usize + 1).min(n);
    let base = Id::new(id);
    for i in first..last.max(first) {
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect_of(i)).id_salt(base.with(i)));
        draw(&mut child, i);
    }
    (0..n).map(rect_of).collect()
}

/// Where «Хочу скачать» is for a track: its own item, else its album's
/// (dismissed and finished items count as none). Built once a frame.
#[derive(Clone, Default)]
pub struct WishIndex {
    tracks: HashMap<TrackId, WishlistStatus>,
    releases: HashMap<ReleaseId, WishlistStatus>,
}

impl WishIndex {
    pub fn of(ctx: &egui::Context, w: &WishlistView) -> std::sync::Arc<Self> {
        let key = Id::new("md3e-wish-index");
        let pass = ctx.cumulative_pass_nr();
        if let Some((at, ix)) = ctx.data(|d| d.get_temp::<(u64, std::sync::Arc<Self>)>(key))
            && at == pass
        {
            return ix;
        }
        let ix = std::sync::Arc::new(Self::build(w));
        ctx.data_mut(|d| d.insert_temp(key, (pass, ix.clone())));
        ix
    }

    /// The index of `w` without the frame's cache (a menu built once).
    pub fn build(w: &WishlistView) -> Self {
        let mut ix = Self::default();
        for r in &w.items {
            if matches!(r.status, WishlistStatus::Done | WishlistStatus::Dismissed) {
                continue;
            }
            match (r.scope, r.track, r.release) {
                (WishlistScope::Track, Some(t), _) => {
                    ix.tracks.insert(t, r.status);
                }
                (WishlistScope::Album, _, Some(rel)) => {
                    ix.releases.insert(rel, r.status);
                }
                (_, Some(t), _) => {
                    ix.tracks.insert(t, r.status);
                }
                _ => {}
            }
        }
        ix
    }

    pub fn of_track(&self, track: Option<TrackId>, release: Option<ReleaseId>) -> Option<WishlistStatus> {
        track
            .and_then(|t| self.tracks.get(&t).copied())
            .or_else(|| release.and_then(|r| self.releases.get(&r).copied()))
    }

    pub fn of_release(&self, release: ReleaseId) -> Option<WishlistStatus> {
        self.releases.get(&release).copied()
    }
}

/// A key a row sets when its «Хочу скачать» mark is clicked; the shell
/// opens the tab.
pub const OPEN_WISHLIST: &str = "md3e-open-wishlist";

/// How far a track is in «Хочу скачать», said quietly: one small icon (a
/// tooltip names it); a click opens «Хочу скачать». Without torrents
/// nothing searches: what is not on its way yet waits for them.
pub fn wish_mark(ui: &mut Ui, st: WishlistStatus, torrents: bool, salt: impl std::hash::Hash + std::fmt::Debug) -> Response {
    use WishlistStatus as St;
    let (icon, tip) = match st {
        St::Waiting | St::New | St::Searching if !torrents => (Icon::HourglassEmpty, "Ждёт торренты — «Хочу скачать»"),
        St::Waiting => (Icon::HourglassEmpty, "Ждёт в «Хочу скачать»"),
        St::New | St::Searching => (Icon::Search, "Ищу, где скачать"),
        St::Confirm => (Icon::Info, "Нужно выбрать, что скачать — «Хочу скачать»"),
        St::Downloading => (Icon::CloudDownload, "Качается"),
        St::NoTorrent | St::Failed => (Icon::Error, "Не нашёл, где скачать — «Хочу скачать»"),
        St::Done | St::Dismissed => (Icon::Check, "Скачано"),
    };
    ui.add(IconButton::new(icon).size(Size::Xs).tooltip(tip).id_salt(Id::new(salt)))
}

/// ⋮ / right-click menu of a playlist row: the library track's
/// [`super::library_track_rows`], then «Убрать из плейлиста» last, with
/// «Разные песни» when the track has it.
pub(crate) fn playlist_track_rows(track: &TrackRow, playlist: PlaylistId, of: super::LibraryRowsOf<'_>) -> Vec<Row<super::TrackAct>> {
    let mut rows = super::library_track_rows(track, of);
    if !matches!(rows.last(), Some(Row::Item(super::TrackAct::Apart, _))) {
        rows.push(Row::Divider);
    }
    let remove = if playlist == LIKED_PLAYLIST { super::UNLIKE } else { "Убрать из плейлиста" };
    rows.push(Row::Item(super::TrackAct::Remove, MenuItem::new(remove).icon(Icon::Delete)));
    rows
}

fn playlist_track_menu(
    ctx: &egui::Context,
    key: Id,
    track: &TrackRow,
    playlist: PlaylistId,
    state: &ViewState,
    wish: &WishIndex,
) -> (Menu, Vec<super::TrackAct>) {
    let rows = playlist_track_rows(track, playlist, super::LibraryRowsOf::of(state, track, wish));
    let rows = super::with_speed_row(rows, &state.speeds, super::is_playing(state, track.id));
    let also = state.also_on.get(&track.id).map(|(r, _)| *r);
    let rows = super::here(ctx).trim(rows, track, also, Some);
    let rows = super::placed(rows, &state.prefs.track_menu, |a| a.menu_item(track.has_local));
    let (menu, acts) = menu_of(key, rows);
    (menu.align_right(true), acts)
}

fn playlist_track_pick(
    ctx: &egui::Context,
    act: Option<super::TrackAct>,
    track: &TrackRow,
    playlist: PlaylistId,
    anchor: Option<Rect>,
    pages: &mut PageState,
    state: &ViewState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    match act? {
        super::TrackAct::Remove => {
            out.push(Command::RemoveFromPlaylist(playlist, track.id));
            None
        }
        act => pages.track_apply(ctx, act, track, state, anchor, out),
    }
}

/// Drag-reorder feedback and the final move: an insertion line follows the
/// pointer; releasing the handle emits [`Command::MoveInPlaylist`].
fn reorder(
    ui: &Ui,
    playlist: PlaylistId,
    tracks: &[TrackRow],
    rows: &[Rect],
    from: usize,
    live: bool,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx();
    let s = Theme::get(ctx).scheme;
    let Some(pointer) = ctx.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos())) else {
        return;
    };
    // Insertion slot in the original order, then its index after removal.
    let slot = rows.iter().take_while(|r| r.center().y < pointer.y).count();
    let to = if slot > from { slot - 1 } else { slot }.min(tracks.len().saturating_sub(1));
    if live {
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        if to != from {
            let y = match slot {
                0 => rows[0].top(),
                n => rows[n - 1].bottom(),
            };
            let line = Rect::from_min_max(
                pos2(rows[0].left() + 16.0, y - 1.5),
                pos2(rows[0].right() - 16.0, y + 1.5),
            );
            paint::container(ui.painter(), line, Radii::all(1.5), s.primary, Stroke::NONE);
        }
        // Scroll while dragging near the edges of the viewport.
        let clip = ui.clip_rect();
        if pointer.y < clip.top() + 56.0 {
            ui.scroll_with_delta(vec2(0.0, 14.0));
        } else if pointer.y > clip.bottom() - 120.0 {
            ui.scroll_with_delta(vec2(0.0, -14.0));
        }
        ctx.request_repaint();
    } else if to != from {
        out.push(Command::MoveInPlaylist { playlist, track: tracks[from].id, to });
    }
}

fn play_from(from: &PlayFrom, tracks: &[TrackRow], i: usize) -> Command {
    match from {
        PlayFrom::Release(id) => Command::PlayRelease {
            id: *id,
            start: Some(tracks[i].id),
            shuffle: false,
        },
        PlayFrom::Playlist(id) => Command::PlayPlaylist {
            id: *id,
            start: Some(tracks[i].id),
            shuffle: false,
        },
        PlayFrom::Tracks(ids, ctx) => Command::PlayTracks {
            tracks: ids.clone(),
            start: ids.iter().position(|t| *t == tracks[i].id).unwrap_or(i),
            context: ctx.clone(),
        },
        PlayFrom::Search { picks, at, context } => Command::PlaySearch {
            tracks: picks.clone(),
            start: *at,
            context: context.clone(),
        },
    }
}

/// A centred empty-state block: icon, title, body, optional button.
pub fn empty_state(ui: &mut Ui, icon: Icon, title: &str, body: &str, action: Option<&str>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
        shape_image(ui, None, "", r, MaterialShape::Cookie9Sided);
        icon.widget(false, s.on_tertiary_container, 40.0)
            .paint_at(ui, Rect::from_center_size(r.center(), vec2(40.0, 40.0)));
        ui.add_space(16.0);
        ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
        // An empty `body`: the title alone, no band under it.
        if !body.is_empty() {
            ui.add_space(4.0);
            ui.label(TypeRole::BodyMedium.style().rich_para(body, s.on_surface_variant));
        }
        if let Some(a) = action {
            ui.add_space(16.0);
            clicked = ui
                .add(crate::components::Button::new(a).style(crate::components::Style::Filled).id_salt(("empty", title)))
                .clicked();
        }
    });
    clicked
}

/// Number of grid columns that fit `width` with `card` cards.
pub fn columns(width: f32, card: f32) -> usize {
    crate::fit::Fit::at_least(width, CARD_GAP, card).count
}

/// The side of the big cover in a page's header (an album, a playlist, a
/// mix, an artist, a world): 232 beside a wide page, 160 otherwise.
pub fn header_cover(width: f32) -> f32 {
    if width > 720.0 { 232.0 } else { 160.0 }
}

/// The grid of cards at least `card` wide over `width`: how many stand in a
/// row and how wide each one is (whole points, the row filled).
pub fn grid_fit(width: f32, card: f32) -> (usize, f32) {
    let fit = crate::fit::Fit::at_least(width, CARD_GAP, card);
    (fit.count, fit.size.floor())
}

/// Which items of a long list are laid out this frame: those that meet
/// the viewport and one more on each side (`now`), and those laid out last
/// frame, so one that leaves the view still settles its hover and press as
/// it always did. The others only take their room. While a menu is up
/// every item is laid out (the one it belongs to stays under it).
pub struct Shown {
    now: std::ops::Range<usize>,
    last: std::ops::Range<usize>,
    all: bool,
}

impl Shown {
    pub fn of(ctx: &egui::Context, key: Id, now: std::ops::Range<usize>) -> Self {
        let last = ctx.data_mut(|d| {
            let last = d.get_temp::<std::ops::Range<usize>>(key);
            d.insert_temp(key, now.clone());
            last.unwrap_or_default()
        });
        Self { now, last, all: Menu::shown_lately(ctx) }
    }

    pub fn contains(&self, i: usize) -> bool {
        self.all || self.now.contains(&i) || self.last.contains(&i)
    }
}

/// The items `0..n` whose spans `start + i * step .. + size` meet `lo..hi`,
/// one more on each side.
pub fn span_range(n: usize, start: f32, step: f32, size: f32, lo: f32, hi: f32) -> std::ops::Range<usize> {
    if n == 0 || step <= 0.0 {
        return 0..0;
    }
    let first = ((lo - start - size) / step).ceil().max(0.0) as usize;
    let last = (((hi - start) / step).floor().max(-1.0) + 1.0) as usize;
    first.saturating_sub(1)..(last + 1).min(n)
}

/// Lays cards in a wrapping grid; cards get stretched to fill the row.
/// `height` is how tall an item's card is at a width: rows out of sight
/// take that room without being laid out.
pub fn grid<T>(
    ui: &mut Ui,
    items: &[T],
    min_card: f32,
    height: impl Fn(&T, f32) -> f32,
    mut draw: impl FnMut(&mut Ui, &T, f32) -> Option<Route>,
) -> Option<Route> {
    let (cols, card) = grid_fit(ui.available_width(), min_card);
    let key = ui.next_auto_id().with(("md3e-grid", items.len()));
    let rows: Vec<&[T]> = items.chunks(cols).collect();
    let heights: Vec<f32> = rows
        .iter()
        .map(|row| row.iter().map(|it| height(it, card)).fold(0.0, f32::max))
        .collect();
    // Where each row would be, and which ones meet the viewport.
    let gap = ui.spacing().item_spacing.y + CARD_GAP;
    let clip = ui.clip_rect();
    let mut y = ui.cursor().top();
    let (mut first, mut last) = (usize::MAX, 0);
    for (i, h) in heights.iter().enumerate() {
        if y + h >= clip.top() && y <= clip.bottom() {
            first = first.min(i);
            last = i + 1;
        }
        y += h + gap;
    }
    let now = if first < last { first.saturating_sub(1)..(last + 1).min(rows.len()) } else { 0..0 };
    let shown = Shown::of(ui.ctx(), key, now);
    let mut nav = None;
    for (i, row) in rows.into_iter().enumerate() {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            if shown.contains(i) {
                for it in row {
                    nav = draw(ui, it, card).or(nav.take());
                }
            } else {
                let n = row.len() as f32;
                ui.allocate_space(vec2(n * card + (n - 1.0) * CARD_GAP, heights[i]));
            }
        });
        ui.add_space(CARD_GAP);
    }
    nav
}

/// [`card_row`] of `n` items `item_w` wide, of which only those in sight
/// (see [`Shown`]) are laid out by `draw`.
pub fn card_row_virtual(ui: &mut Ui, id: &str, n: usize, item_w: f32, mut draw: impl FnMut(&mut Ui, usize)) {
    card_row(ui, id, |ui| {
        let start = ui.cursor().left();
        let clip = ui.clip_rect();
        let now = span_range(n, start, item_w + CARD_GAP, item_w, clip.left(), clip.right());
        let shown = Shown::of(ui.ctx(), ui.id().with(("md3e-row", id)), now);
        let mut skipped = 0usize;
        for i in 0..n {
            if shown.contains(i) {
                if skipped > 0 {
                    let k = skipped as f32;
                    ui.allocate_space(vec2(k * item_w + (k - 1.0) * CARD_GAP, 0.0));
                    skipped = 0;
                }
                draw(ui, i);
            } else {
                skipped += 1;
            }
        }
        if skipped > 0 {
            let k = skipped as f32;
            ui.allocate_space(vec2(k * item_w + (k - 1.0) * CARD_GAP, 0.0));
        }
    });
}

/// "Похоже, это один альбом": editions of one album the runtime found,
/// with a yes and a no (nothing merges on its own). `artist` narrows them
/// to one artist's page.
pub fn edition_offers(ui: &mut Ui, state: &ViewState, artist: Option<&str>, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let offers = state.editions.iter().filter(|o| artist.is_none_or(|a| o.artist == a));
    for o in offers {
        crate::components::Card::filled().id_salt(("edition", o.id)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(TypeRole::TitleMedium.style().rich("Похоже, это один альбом", s.on_surface));
            ui.add_space(4.0);
            let names = std::iter::once(&o.keep).chain(&o.others).map(|t| format!("«{t}»")).collect::<Vec<_>>().join(" и ");
            let mut what = format!("{names} — издания одного альбома ({}).", o.artist);
            if o.same_songs > 0 {
                what.push_str(&format!(" Одинаковых песен: {} — останется по одной, файл получше будет играть.", o.same_songs));
            }
            if o.new_songs > 0 {
                what.push_str(&format!(" Ещё {} добавится в «{}».", o.new_songs, o.keep));
            }
            ui.label(TypeRole::BodyMedium.style().rich_para(what, s.on_surface_variant));
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 40.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .add(crate::components::Button::new("Объединить").style(Style::Filled).id_salt(("ed-merge", o.id)))
                        .on_hover_text(format!("Всё окажется в «{}»; файлы на диске не трогаются", o.keep))
                        .clicked()
                    {
                        out.push(Command::MergeEditions(o.id));
                    }
                    if ui
                        .add(crate::components::Button::new("Оставить раздельно").style(Style::Text).id_salt(("ed-keep", o.id)))
                        .on_hover_text("Больше не спрашивать про эти альбомы")
                        .clicked()
                    {
                        out.push(Command::KeepEditionsApart(o.id));
                    }
                },
            );
        });
        ui.add_space(12.0);
    }
}

#[cfg(test)]
mod menu_tests {
    use super::{
        ArtistAct, PlaylistAct, PlaylistMenuOf, ReleaseAct, Row, artist_rows, playlist_command, playlist_rows, release_rows,
        row_acts,
    };
    use nikgob_view::{ArtistLinks, Command, PlaylistId, PlaylistSyncMode, ReleaseCard, ReleaseId, ReleaseType};

    fn labels<A>(rows: &[Row<A>]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Item(_, i) => Some(i.label.clone()),
                Row::Divider | Row::Label(_) | Row::Sub(..) => None,
            })
            .collect()
    }

    /// The playlist's right-click menu and its page's ⋮ are one list: the
    /// page leaves out only «Слушать» and «Перемешать» (its buttons), and
    /// both can play the playlist next and queue it.
    #[test]
    fn a_playlist_offers_the_same_actions_by_right_click_and_by_more() {
        let card = row_acts(&playlist_rows(PlaylistMenuOf { editable: true, ..Default::default() }));
        let page = row_acts(&playlist_rows(PlaylistMenuOf { on_page: true, editable: true, ..Default::default() }));
        let card_less_buttons: Vec<PlaylistAct> =
            card.iter().copied().filter(|a| !matches!(a, PlaylistAct::Play | PlaylistAct::Shuffle)).collect();
        assert_eq!(card_less_buttons, page);
        for acts in [&card, &page] {
            assert!(acts.contains(&PlaylistAct::Next) && acts.contains(&PlaylistAct::Queue));
            assert_eq!(acts.last(), Some(&PlaylistAct::Delete));
        }
        // The liked playlist: no edit, no delete, still next and queue.
        let liked = row_acts(&playlist_rows(PlaylistMenuOf::default()));
        assert_eq!(liked, [PlaylistAct::Play, PlaylistAct::Shuffle, PlaylistAct::Next, PlaylistAct::Queue]);
        assert_eq!(&labels(&playlist_rows(PlaylistMenuOf::default()))[2..], ["Сыграть следующим", "В очередь"]);
        let id = PlaylistId::new();
        assert_eq!(playlist_command(PlaylistAct::Next, id), Some(Command::PlayPlaylistNext(id)));
        assert_eq!(playlist_command(PlaylistAct::Queue, id), Some(Command::EnqueuePlaylist(id)));
    }

    /// A linked playlist's modes say what they do, and one stops the sync.
    #[test]
    fn a_linked_playlist_can_stop_syncing() {
        let rows = playlist_rows(PlaylistMenuOf {
            on_page: true,
            editable: true,
            sync: Some(("YouTube Music", PlaylistSyncMode::TwoWay)),
            can_link: false,
        });
        let l = labels(&rows);
        for want in ["Изменения в обе стороны", "Только получать изменения", "Не синхронизировать"] {
            assert!(l.iter().any(|x| x == want), "{want} missing from {l:?}");
        }
        assert!(rows.iter().any(|r| matches!(r, Row::Label(t) if t == "Синхронизация с YouTube Music")));
        let acts = row_acts(&rows);
        assert!(!acts.contains(&PlaylistAct::Link));
        let id = PlaylistId::new();
        assert_eq!(
            playlist_command(PlaylistAct::Sync(PlaylistSyncMode::Once), id),
            Some(Command::SetPlaylistSync { playlist: id, mode: PlaylistSyncMode::Once })
        );
        // Not linked: the way to link it instead of the modes, last with
        // the delete after their divider; asked first, so no command yet.
        let rows = playlist_rows(PlaylistMenuOf { on_page: true, editable: true, can_link: true, ..Default::default() });
        let unlinked = row_acts(&rows);
        assert_eq!(&unlinked[unlinked.len() - 2..], [PlaylistAct::Link, PlaylistAct::Delete]);
        assert!(matches!(rows[rows.len() - 3], Row::Divider));
        assert!(!unlinked.iter().any(|a| matches!(a, PlaylistAct::Sync(_))));
        assert_eq!(playlist_command(PlaylistAct::Link, id), None);
        assert_eq!(super::playlist_link_command(id), Command::SetPlaylistSync { playlist: id, mode: PlaylistSyncMode::TwoWay });
    }

    /// The album's right-click menu and its page's ⋮ are one list in one
    /// order: the page leaves out its buttons (play, shuffle, the like).
    #[test]
    fn an_album_offers_the_same_actions_by_right_click_and_by_more() {
        let r = ReleaseCard {
            id: ReleaseId::new(),
            title: "Album".into(),
            artist: "Artist".into(),
            artists: ArtistLinks::default(),
            year: None,
            release_type: ReleaseType::Album,
            cover: None,
            local_tracks: 1,
            total_tracks: Some(1),
            liked: false,
            added_at: 0,
            better: false,
        };
        let card = row_acts(&release_rows(&r, false));
        let page = row_acts(&release_rows(&r, true));
        let shared = |acts: &[ReleaseAct]| -> Vec<ReleaseAct> {
            acts.iter()
                .copied()
                .filter(|a| !matches!(a, ReleaseAct::Play | ReleaseAct::Shuffle | ReleaseAct::Like))
                .collect()
        };
        assert_eq!(shared(&card), shared(&page));
        assert_eq!(&card[..6], [ReleaseAct::Play, ReleaseAct::Shuffle, ReleaseAct::Next, ReleaseAct::Queue, ReleaseAct::Like, ReleaseAct::Playlist]);
        assert_eq!(&page[..3], [ReleaseAct::Next, ReleaseAct::Queue, ReleaseAct::Playlist]);
        assert_eq!(&labels(&release_rows(&r, false))[2..6], ["Сыграть следующим", "В очередь", "Нравится", "В плейлист…"]);
        for acts in [&card, &page] {
            assert_eq!(&acts[acts.len() - 2..], [ReleaseAct::Trash, ReleaseAct::Purge]);
        }
        // Not all of it here: what it lacks can be downloaded from the card.
        let part = ReleaseCard { local_tracks: 3, total_tracks: Some(12), ..r.clone() };
        let acts = row_acts(&release_rows(&part, false));
        assert_eq!(&acts[acts.len() - 3..], [ReleaseAct::Download, ReleaseAct::Trash, ReleaseAct::Purge]);
        assert!(!row_acts(&release_rows(&part, true)).contains(&ReleaseAct::Download), "a button on its page");
        // A stream-only album has no files to delete for good, and an
        // album with no known artist no way to one.
        let stream = ReleaseCard { local_tracks: 0, ..part };
        let acts = row_acts(&release_rows(&stream, false));
        assert!(acts.contains(&ReleaseAct::Trash) && !acts.contains(&ReleaseAct::Purge) && !acts.contains(&ReleaseAct::Artist));
        // An album with a file that can get better: on its card and its page.
        let mp3 = ReleaseCard { better: true, ..r.clone() };
        for on_page in [false, true] {
            let acts = row_acts(&release_rows(&mp3, on_page));
            assert_eq!(&acts[acts.len() - 3..], [ReleaseAct::Better, ReleaseAct::Trash, ReleaseAct::Purge], "{on_page}");
        }
        assert!(!row_acts(&release_rows(&r, false)).contains(&ReleaseAct::Better));
        let gone = ReleaseCard { local_tracks: 0, ..mp3 };
        assert!(!row_acts(&release_rows(&gone, false)).contains(&ReleaseAct::Better));
    }

    /// The artist's right-click menu and its page's ⋮: one list, the page
    /// without its buttons (mix, shuffle, radio, «Подписаться»).
    #[test]
    fn an_artist_offers_the_same_actions_by_right_click_and_by_more() {
        let a = nikgob_view::ArtistCard {
            id: nikgob_view::ArtistId::new(),
            name: "Artist".into(),
            image: None,
            liked: false,
            track_count: 0,
        };
        let marks = nikgob_view::DislikesView::default();
        let card = row_acts(&artist_rows(&a, false, &marks));
        let page = row_acts(&artist_rows(&a, true, &marks));
        let shared: Vec<ArtistAct> = card
            .iter()
            .copied()
            .filter(|x| !matches!(x, ArtistAct::Play | ArtistAct::Shuffle | ArtistAct::Radio | ArtistAct::Follow))
            .collect();
        assert_eq!(shared, page);
        assert_eq!(page, [ArtistAct::Queue, ArtistAct::Ban]);
        assert_eq!(labels(&artist_rows(&a, true, &marks))[0], "В очередь");
    }

    fn track(has_local: bool) -> nikgob_view::TrackRow {
        nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            release_id: Some(ReleaseId::new()),
            artists: nikgob_view::Credit::single("Artist", Some(nikgob_view::ArtistId::new())),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local,
            liked: false,
            cover: None,
            clip: false,
        }
    }

    fn of(marks: &nikgob_view::DislikesView) -> crate::screens::pages::LibraryRowsOf<'_> {
        crate::screens::pages::LibraryRowsOf { marks, info: false, also: None, torrents: false, wished: None }
    }

    /// The track menus (a library row, a playlist row, a service's song)
    /// share one list: the same common actions in one order, then what
    /// only the place adds («Убрать из плейлиста», last).
    #[test]
    fn every_track_menu_has_the_common_actions_in_one_order() {
        use crate::screens::pages::{LibraryRowsOf, TrackAct as A, library_track_rows};
        let marks = nikgob_view::DislikesView::default();
        let t = track(true);
        let library = row_acts(&library_track_rows(&t, of(&marks)));
        let playlist = row_acts(&super::playlist_track_rows(&t, PlaylistId::new(), of(&marks)));
        assert_eq!(&playlist[..library.len()], &library[..]);
        assert_eq!(playlist.last(), Some(&A::Remove));
        let head = [A::Next, A::Queue, A::Radio, A::Playlist, A::Album, A::Artist, A::Share];
        assert_eq!(&library[..head.len()], head);
        assert_eq!(&library[library.len() - 2..], [A::Dislike, A::Ban]);
        // The service's song: the same head and tail around its downloads.
        let stream = row_acts(&crate::screens::pages::track_rows(crate::screens::pages::TrackMenuOf {
            playable: true,
            has_album: true,
            has_artists: true,
            has_file: false,
            also_on: None,
            info: false,
            fetch: Some(crate::components::MenuItem::new("Скачать")),
            better: false,
            pick_release: false,
            find_song: None,
            wrong_file: false,
            dislike: crate::components::MenuItem::new("Не нравится"),
            ban: Some(crate::components::MenuItem::new("Не рекомендовать исполнителя")),
        }));
        assert_eq!(&stream[..head.len()], head);
        assert_eq!(&stream[head.len()..], [A::Fetch, A::AlbumDownload, A::Dislike, A::Ban]);
        // The other album of the song sits right after «К альбому»; «Разные
        // песни» and «Убрать из плейлиста» close the menu in one group.
        let also = (ReleaseId::new(), "Other".to_owned());
        let with_also = LibraryRowsOf { also: Some(&also), ..of(&marks) };
        let rows = library_track_rows(&t, with_also);
        let acts = row_acts(&rows);
        assert_eq!(&acts[4..7], [A::Album, A::AlsoOn, A::Artist]);
        assert!(labels(&rows).iter().any(|l| l == "Есть и на альбоме «Other»"));
        let rows = super::playlist_track_rows(&t, PlaylistId::new(), with_also);
        assert!(matches!(&rows[rows.len() - 3..], [Row::Divider, Row::Item(A::Apart, _), Row::Item(A::Remove, _)]));
    }
}
