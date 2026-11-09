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

/// An album card; navigates on click, plays on ▶, right click opens the menu.
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
        .width(width)
        .id_salt(("release-card", row, r.id))
        .show(ui);
    let menu = release_menu(&o.response, r, out);
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
    if liked { "Убрать из любимого" } else { "В любимое" }
}

/// Right-click menu of an album (card or row).
pub fn release_menu(resp: &Response, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    if Menu::context_idle(resp, ("release-menu", resp.id)) {
        return None;
    }
    let picked = Menu::new(("release-menu", resp.id))
        .item(MenuItem::new("Слушать").icon(Icon::PlayArrow))
        .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В очередь").icon(Icon::QueueMusic))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(!r.artists.linked().is_empty()))
        .divider()
        .item(MenuItem::new(album_like_label(r.liked)).icon(Icon::Favorite).selected(r.liked))
        .item(MenuItem::new("Поделиться").icon(Icon::Share))
        .divider()
        .item(MenuItem::new("Удалить альбом").icon(Icon::Delete))
        .item(MenuItem::new("Удалить альбом навсегда").icon(Icon::Delete))
        .context_menu(resp);
    match picked? {
        0 => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
        1 => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: true }),
        2 => out.push(Command::PlayReleaseNext(r.id)),
        3 => out.push(Command::EnqueueRelease(r.id)),
        4 => return artist_links::go_to(&resp.ctx, artist_links::library(&r.artists)),
        5 => out.push(Command::Like(LikeTarget::Release(r.id), !r.liked)),
        6 => out.push(Command::Share(nikgob_view::ShareTarget::Release(r.id))),
        7 => super::request_release_delete(&resp.ctx, r.id, &r.title, false),
        8 => super::request_release_delete(&resp.ctx, r.id, &r.title, true),

        _ => {}
    }
    None
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
        .width(width)
        .id_salt(("playlist-card", row, p.id))
        .show(ui);
    playlist_menu_at(&o.response, p, pages, out);
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

//// One row of a menu built from actions: an item standing for `A`, a
/// divider, or a group label (not an item: nothing to pick).
#[derive(Clone, Debug)]
pub enum Row<A> {
    Item(A, MenuItem),
    Divider,
    Label(String),
}

/// The actions of `rows` in item order (what a picked index stands for).
pub fn row_acts<A: Copy>(rows: &[Row<A>]) -> Vec<A> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Item(a, _) => Some(*a),
            Row::Divider | Row::Label(_) => None,
        })
        .collect()
}

/// A menu of `rows` and what each of its item indices stands for. A
/// divider at the start, at the end or after another is left out.
pub fn menu_of<A: Copy>(key: impl egui::AsId, rows: Vec<Row<A>>) -> (Menu, Vec<A>) {
    let acts = row_acts(&rows);
    let mut menu = Menu::new(key);
    let mut after_divider = true;
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        match r {
            Row::Item(_, item) => {
                menu = menu.item(item);
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
    (menu, acts)
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

/// The one list of a playlist's actions, for its right-click menu (card or
/// row) and the ⋮ of its page: play, next, queue; edit; sync; delete.
pub fn playlist_rows(o: PlaylistMenuOf<'_>) -> Vec<Row<PlaylistAct>> {
    use PlaylistAct as A;
    let mut rows = Vec::new();
    if !o.on_page {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
    }
    rows.push(Row::Item(A::Next, MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new("В конец очереди").icon(Icon::QueueMusic)));
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
    } else if o.can_link {
        rows.push(Row::Item(A::Link, MenuItem::new("Связать с сервисом…").icon(Icon::Sync)));
    }
    if o.editable {
        rows.push(Row::Divider);
        rows.push(Row::Item(A::Delete, MenuItem::new("Удалить плейлист").icon(Icon::Delete)));
    }
    rows
}

/// The command of a playlist action that needs no dialog (not «Изменить»
/// or «Удалить»).
pub fn playlist_command(act: PlaylistAct, id: PlaylistId) -> Option<Command> {
    use nikgob_view::PlaylistSyncMode as M;
    Some(match act {
        PlaylistAct::Play => Command::PlayPlaylist { id, start: None, shuffle: false },
        PlaylistAct::Shuffle => Command::PlayPlaylist { id, start: None, shuffle: true },
        PlaylistAct::Next => Command::PlayPlaylistNext(id),
        PlaylistAct::Queue => Command::EnqueuePlaylist(id),
        PlaylistAct::Sync(mode) => Command::SetPlaylistSync { playlist: id, mode },
        // Not linked: it is made in a service, synced both ways.
        PlaylistAct::Link => Command::SetPlaylistSync { playlist: id, mode: M::TwoWay },
        PlaylistAct::Edit | PlaylistAct::Delete => return None,
    })
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
    if Menu::context_idle(resp, ("playlist-menu", resp.id)) {
        return;
    }
    let editable = pages.is_some() && !p.pinned && p.id != LIKED_PLAYLIST;
    let of = PlaylistMenuOf { editable, ..Default::default() };
    let (menu, acts) = menu_of(("playlist-menu", resp.id), playlist_rows(of));
    let Some(act) = menu.context_menu(resp).and_then(|i| acts.get(i).copied()) else { return };
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

/// An artist avatar card with a ▶ on hover (plays the mix) and a right-click
/// menu. A click opens the artist.
pub fn artist_avatar(
    ui: &mut Ui,
    a: &ArtistCard,
    size: f32,
    out: &mut Vec<Command>,
) -> Option<Route> {
    artist_impl(ui, a, size, false, out)
}

/// How tall [`artist_avatar`] is for `a` at `size` (the name, and in the
/// library the track count under it).
pub fn artist_avatar_height(a: &ArtistCard, size: f32) -> f32 {
    size + if a.track_count == 0 { 28.0 } else { 48.0 }
}

fn artist_impl(ui: &mut Ui, a: &ArtistCard, size: f32, radio: bool, out: &mut Vec<Command>) -> Option<Route> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    // In the library the name has the track count under it.
    let caption = if radio { String::new() } else { artist_tracks(a) };
    let text_h = if caption.is_empty() { 28.0 } else { 48.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size + text_h), Sense::click());
    // A row holds every artist of the library: the ones scrolled out of
    // sight (with room for the hover growth) cost nothing.
    if !ui.is_rect_visible(rect.expand(4.0)) {
        return None;
    }
    let img = Rect::from_min_size(rect.min, vec2(size, size));
    let hover = resp.contains_pointer();
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
    // On a mix the name is the way to the page: a link under the pointer.
    let name = radio.then(|| {
        ui.interact(name_rect.expand2(vec2(4.0, 2.0)), ui.id().with(("artist-name", a.id)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Открыть исполнителя")
    });
    let underline = name.as_ref().is_some_and(Response::hovered);
    ui.painter().galley(name_rect.min, galley, s.on_surface);
    if !caption.is_empty() {
        let mut job = egui::text::LayoutJob::single_section(caption, TypeRole::BodySmall.style().text_format(s.on_surface_variant));
        job.wrap = egui::text::TextWrapping::truncate_at_width(size.max(1.0));
        let g = crate::typography::layout(ui.painter(), job);
        let at = pos2(rect.center().x - g.size().x * 0.5, name_rect.bottom() + 2.0);
        ui.painter().galley(at, g, s.on_surface_variant);
    }
    if underline {
        let y = name_rect.bottom() - 1.0;
        ui.painter().line_segment([pos2(name_rect.left(), y), pos2(name_rect.right(), y)], Stroke::new(1.0, s.on_surface));
    }

    // ▶ over the avatar, fading in with the pointer (fast effects).
    let k = anim::fraction(
        ui.ctx(),
        ui.id().with(("artist-play", a.id)),
        if hover && !underline { 1.0 } else { 0.0 },
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
                    .tooltip(if radio { "Радио исполнителя" } else { "Слушать микс" })
                    .id_salt(("artist-play-btn", a.id)),
            )
            .clicked();
    }
    let name_clicked = name.as_ref().is_some_and(Response::clicked);
    let menu = artist_menu_with(&resp, a, radio, out);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if radio {
        if (play || resp.clicked()) && !name_clicked {
            out.push(Command::PlayMix { id: nikgob_view::MixId::Artist(a.id) });
        }
        return name_clicked.then_some(Route::Artist(a.id)).or(menu);
    }
    if play {
        out.push(Command::PlayArtist { id: a.id, shuffle: false });
    }
    (resp.clicked() && !play).then_some(Route::Artist(a.id)).or(menu)
}

/// Right-click menu of an artist (card or row): Слушать микс · Перемешать ·
/// Радио исполнителя · Подписаться.
pub fn artist_menu(resp: &Response, a: &ArtistCard, out: &mut Vec<Command>) {
    artist_menu_with(resp, a, false, out);
}

/// [`artist_menu`]; on a mix card (`radio`) the radio comes first and the
/// artist's page can be opened from it.
fn artist_menu_with(resp: &Response, a: &ArtistCard, radio: bool, out: &mut Vec<Command>) -> Option<Route> {
    if Menu::context_idle(resp, ("artist-menu", resp.id)) {
        return None;
    }
    let follow = MenuItem::new(if a.liked { "Отписаться" } else { "Подписаться" }).icon(Icon::Favorite);
    let radio_cmd = Command::StartRadio { seed: nikgob_view::RadioSeed::Artist(a.id) };
    let marks = super::marks(&resp.ctx);
    let ban = super::ban_item(Some(a.id), &marks);
    if radio {
        match Menu::new(("artist-menu", resp.id))
            .item(MenuItem::new("Радио исполнителя").icon(Icon::Radio))
            .item(MenuItem::new("Перемешать его треки").icon(Icon::Shuffle))
            .item(MenuItem::new("Открыть исполнителя").icon(Icon::Person))
            .divider()
            .item(follow)
            .item(ban)
            .context_menu(resp)?
        {
            0 => out.push(radio_cmd),
            1 => out.push(Command::PlayArtist { id: a.id, shuffle: true }),
            2 => return Some(Route::Artist(a.id)),
            3 => out.push(Command::Like(LikeTarget::Artist(a.id), !a.liked)),
            4 => out.push(super::ban_command(a.id, &marks)),
            _ => {}
        }
        return None;
    }
    match Menu::new(("artist-menu", resp.id))
        .item(MenuItem::new("Слушать микс").icon(Icon::PlayArrow))
        .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
        .item(MenuItem::new("Радио исполнителя").icon(Icon::Radio))
        .divider()
        .item(follow)
        .item(ban)
        .context_menu(resp)?
    {
        0 => out.push(Command::PlayArtist { id: a.id, shuffle: false }),
        1 => out.push(Command::PlayArtist { id: a.id, shuffle: true }),
        2 => out.push(radio_cmd),
        3 => out.push(Command::Like(LikeTarget::Artist(a.id), !a.liked)),
        4 => out.push(super::ban_command(a.id, &marks)),
        _ => {}
    }
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
                    let picked = playlist_track_menu(menu_key, t, pl, &state.dislikes).attach(m);
                    *nav = playlist_track_pick(&m.ctx, picked, t, pl, Some(m.rect), pages, &state.dislikes, out).or(nav.take());
                }
                let picked = if Menu::context_idle(&o.response, menu_key.with("ctx")) {
                    None
                } else {
                    playlist_track_menu(menu_key.with("ctx"), t, pl, &state.dislikes).context_menu(&o.response)
                };
                *nav = playlist_track_pick(&o.response.ctx, picked, t, pl, None, pages, &state.dislikes, out).or(nav.take());

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
                            .tooltip(if liked { "Убрать из любимого" } else { "В любимое" })
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
        let ix = std::sync::Arc::new(ix);
        ctx.data_mut(|d| d.insert_temp(key, (pass, ix.clone())));
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

/// ⋮ / right-click menu of a playlist row.
fn playlist_track_menu(key: Id, track: &TrackRow, playlist: PlaylistId, marks: &nikgob_view::DislikesView) -> Menu {
    let mut m = Menu::new(key)
        .align_right(true)
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .item(MenuItem::new(super::RADIO_BY_TRACK).icon(Icon::Radio))
        .item(MenuItem::new("К альбому").icon(Icon::Album).enabled(track.release_id.is_some()))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(!track.artists.linked().is_empty()))
        .item(MenuItem::new("Скачать альбом").icon(Icon::Download).enabled(track.release_id.is_some()));
    // A clip: its song by hand; a stream from YouTube: another version.
    if let Some(find) = crate::screens::find_song::label(track) {
        m = m.item(MenuItem::new(find).icon(Icon::MusicNote));
    }
    m.divider()
        .item(
            MenuItem::new(if playlist == LIKED_PLAYLIST {
                "Убрать из любимого"
            } else {
                "Убрать из плейлиста"
            })
            .icon(Icon::Delete),
        )
        .divider()
        .item(super::dislike_item(marks.track(track.id)))
        .item(super::ban_item(track.artist_id(), marks))
}

fn playlist_track_pick(
    ctx: &egui::Context,
    picked: Option<usize>,
    track: &TrackRow,
    playlist: PlaylistId,
    anchor: Option<Rect>,
    pages: &mut PageState,
    marks: &nikgob_view::DislikesView,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let find = crate::screens::find_song::label(track).is_some();
    // «Не нравится» and «Не рекомендовать исполнителя» close the menu.
    let remove = if find { 8 } else { 7 };
    match picked? {
        n if n == remove + 1 => out.push(super::dislike_command(track.id, marks)),
        n if n == remove + 2 => out.extend(track.artist_id().map(|a| super::ban_command(a, marks))),
        0 => out.push(Command::PlayNext(vec![track.id])),
        1 => out.push(Command::Enqueue(vec![track.id])),
        2 => {
            pages.pending_add = Some(vec![track.id]);
            pages.pending_add_anchor = anchor;
        }
        3 => out.push(super::radio_by_track(track.id)),
        4 => return track.release_id.map(Route::Release),
        // Several artists: a chooser.
        5 => return artist_links::go_to(ctx, artist_links::library(&track.artists)),
        6 => {
            if let Some(r) = track.release_id {
                out.push(Command::DownloadRelease(r));
            }
        }
        7 if find => out.push(Command::FindSong { track: track.id, query: None }),
        7 | 8 => out.push(Command::RemoveFromPlaylist(playlist, track.id)),
        _ => {}
    }
    None
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
        ui.add_space(4.0);
        ui.label(TypeRole::BodyMedium.style().rich_para(body, s.on_surface_variant));
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
    (((width + CARD_GAP) / (card + CARD_GAP)).floor() as usize).max(1)
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
    let width = ui.available_width();
    let cols = columns(width, min_card);
    let card = ((width - CARD_GAP * (cols - 1) as f32) / cols as f32).floor();
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
