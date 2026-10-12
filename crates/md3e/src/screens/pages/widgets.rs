//! Building blocks shared by pages: section headers, horizontal card rows,
//! cards with their context menus, track lists, shape-cropped avatars.

use egui::{
    Color32, Id, Mesh, Rect, Response, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2,
};
use nikgob_shapes::{MaterialShape, paint as shape_paint};
use nikgob_view::{
    ArtistCard, Command, LIKED_PLAYLIST, LikeTarget, PlaylistCard, PlaylistId, ReleaseCard,
    Route, SourceKind, Status, TrackRow, ViewState,
};

use super::{Ask, PageState};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Equalizer, IconButton, Leading, ListItem, MediaCard, Menu, MenuItem, Radii, Size, Style,
        anim, paint,
    },
    screens::common,
    tokens::shape,
};

pub const CARD_W: f32 = 160.0;
pub const CARD_GAP: f32 = 12.0;
pub const SECTION_GAP: f32 = 32.0;

/// Title-large section header with an optional "Все →" text button.
/// Returns true when "Все" is clicked.
pub fn section_header(ui: &mut Ui, title: &str, all: bool) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    // A fixed-height row so the title and the 40 dp button share a centre
    // (`horizontal` would top-align the title in a shorter row).
    let row = egui::vec2(ui.available_width(), 40.0);
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
        if all {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui
                    .add(
                        crate::components::Button::new("Все")
                            .style(crate::components::Style::Text)
                            .icon(Icon::ArrowForward)
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

/// "3/12" when a release is incomplete locally.
pub fn progress_badge(r: &ReleaseCard) -> Option<String> {
    match r.total_tracks {
        Some(t) if r.local_tracks < t => Some(format!("{}/{}", r.local_tracks, t)),
        _ => None,
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
        .badge_opt(progress_badge(r))
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

/// Right-click menu of an album (card or row).
pub fn release_menu(resp: &Response, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    let picked = Menu::new(("release-menu", resp.id))
        .item(MenuItem::new("Слушать").icon(Icon::PlayArrow))
        .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
        .item(MenuItem::new("В очередь").icon(Icon::QueueMusic))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(r.artist_id.is_some()))
        .divider()
        .item(
            MenuItem::new(if r.liked { "Убрать из избранного" } else { "В избранное" })
                .icon(Icon::Favorite),
        )
        .context_menu(resp);
    match picked? {
        0 => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
        1 => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: true }),
        2 => out.push(Command::EnqueueRelease(r.id)),
        3 => return r.artist_id.map(Route::Artist),
        4 => out.push(Command::Like(LikeTarget::Release(r.id), !r.liked)),
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
    let subtitle = format!(
        "{} {}",
        p.track_count,
        common::plural(u64::from(p.track_count), "трек", "трека", "треков")
    );
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

/// Right-click menu of a playlist (card or row): Слушать · Перемешать, and
/// with `pages` Переименовать · Удалить (confirmed in a dialog). The liked
/// playlist can be neither renamed nor deleted.
pub fn playlist_menu_at(
    resp: &Response,
    p: &PlaylistCard,
    pages: Option<&mut PageState>,
    out: &mut Vec<Command>,
) {
    let editable = pages.is_some() && !p.pinned && p.id != LIKED_PLAYLIST;
    let mut menu = Menu::new(("playlist-menu", resp.id))
        .item(MenuItem::new("Слушать").icon(Icon::PlayArrow))
        .item(MenuItem::new("Перемешать").icon(Icon::Shuffle));
    if editable {
        menu = menu
            .divider()
            .item(MenuItem::new("Переименовать").icon(Icon::Edit))
            .item(MenuItem::new("Удалить").icon(Icon::Delete));
    }
    match menu.context_menu(resp) {
        Some(0) => out.push(Command::PlayPlaylist { id: p.id, start: None, shuffle: false }),
        Some(1) => out.push(Command::PlayPlaylist { id: p.id, start: None, shuffle: true }),
        Some(2) => {
            if let Some(pages) = pages {
                pages.ask_text = p.name.clone();
                pages.ask = Some(Ask::RenamePlaylist(p.id));
            }
        }
        Some(3) => {
            if let Some(pages) = pages {
                pages.confirm_delete = Some(p.id);
            }
        }
        _ => {}
    }
}

/// Shapes that artist avatars rotate through (formas only on media).
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
    let cubics = shape.polygon().cubics();
    let opts = shape_paint::FillOptions::for_ppp(ui.ctx().pixels_per_point());
    let tex = uri.and_then(|u| {
        egui::Image::new(u.to_owned())
            .texture_options(crate::components::PHOTO_TEXTURE)
            .load_for_size(ui.ctx(), rect.size())
            .ok()
            .and_then(|p| p.texture_id())
    });
    let mesh: Mesh = match tex {
        Some(t) => shape_paint::texture_mesh(
            cubics,
            rect,
            0.0,
            t,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
            &opts,
        ),
        None => shape_paint::fill_mesh(cubics, rect, 0.0, s.tertiary_container, &opts),
    };
    ui.painter().add(Shape::mesh(mesh));
    if tex.is_none() {
        let size = (rect.height() * 0.32).max(12.0);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            initials,
            egui::FontId::new(size, crate::typography::family(500)),
            s.on_tertiary_container,
        );
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
/// menu.
pub fn artist_avatar(
    ui: &mut Ui,
    a: &ArtistCard,
    size: f32,
    out: &mut Vec<Command>,
) -> Option<Route> {
    artist_impl(ui, a, size, Some(out))
}

fn artist_impl(
    ui: &mut Ui,
    a: &ArtistCard,
    size: f32,
    out: Option<&mut Vec<Command>>,
) -> Option<Route> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size + 28.0), Sense::click());
    let img = Rect::from_min_size(rect.min, vec2(size, size));
    let hover = resp.contains_pointer();
    let grow = anim::f32(
        ui.ctx(),
        ui.id().with(("artist-hover", a.id)),
        if hover { 4.0 } else { 0.0 },
        theme.motion.fast_spatial,
    );
    shape_image(
        ui,
        a.image.as_ref().map(|c| c.uri()).as_deref(),
        &initials(&a.name),
        img.expand(grow * 0.5),
        // A real photo is a circle; the initials keep their playful shape.
        if a.image.is_some() { MaterialShape::Circle } else { avatar_shape(&a.id) },
    );
    let text_rect = Rect::from_min_max(pos2(rect.left(), img.bottom() + 6.0), rect.max);
    let galley = ui.painter().layout(
        a.name.clone(),
        TypeRole::TitleSmall.font_id(),
        s.on_surface,
        size,
    );
    ui.painter().galley(
        pos2(text_rect.center().x - galley.size().x * 0.5, text_rect.top()),
        galley,
        s.on_surface,
    );

    let mut play = false;
    if let Some(out) = out {
        // ▶ over the avatar, fading in with the pointer (fast effects).
        let k = anim::fraction(
            ui.ctx(),
            ui.id().with(("artist-play", a.id)),
            if hover { 1.0 } else { 0.0 },
            theme.motion.fast_effects,
        )
        .clamp(0.0, 1.0);
        if k > 0.01 {
            let slot = Rect::from_center_size(img.center(), vec2(56.0, 56.0));
            // A child, not a scope: the overlay must not move the row's cursor.
            let mut child =
                ui.new_child(UiBuilder::new().max_rect(slot).id_salt(("artist-play-slot", a.id)));
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
        if play {
            out.push(Command::PlayArtist { id: a.id, shuffle: false });
        }
        artist_menu(&resp, a, out);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    (resp.clicked() && !play).then_some(Route::Artist(a.id))
}

/// Right-click menu of an artist (card or row): Слушать микс · Перемешать ·
/// Подписаться.
pub fn artist_menu(resp: &Response, a: &ArtistCard, out: &mut Vec<Command>) {
    match Menu::new(("artist-menu", resp.id))
        .item(MenuItem::new("Слушать микс").icon(Icon::PlayArrow))
        .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
        .divider()
        .item(
            MenuItem::new(if a.liked { "Отписаться" } else { "Подписаться" })
                .icon(Icon::Favorite),
        )
        .context_menu(resp)
    {
        Some(0) => out.push(Command::PlayArtist { id: a.id, shuffle: false }),
        Some(1) => out.push(Command::PlayArtist { id: a.id, shuffle: true }),
        Some(2) => out.push(Command::Like(LikeTarget::Artist(a.id), !a.liked)),
        _ => {}
    }
}

/// Where a track list plays from.
#[derive(Clone, Debug)]
pub enum PlayFrom {
    Release(nikgob_view::ReleaseId),
    Playlist(nikgob_view::PlaylistId),
    Tracks(Vec<nikgob_view::TrackId>, String),
}

/// How the leading column looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lead {
    Number,
    Cover,
}

/// Extras of [`track_list_ext`].
#[derive(Clone, Copy, Default)]
pub struct TrackListOpts {
    /// The list is this playlist: ⋮ gets "Убрать из плейлиста".
    pub playlist: Option<PlaylistId>,
    /// Rows can be reordered by their drag handle (user playlists).
    pub reorder: bool,
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
/// `secondary-container` with a mini equalizer instead of the number.
#[allow(clippy::too_many_arguments)]
pub fn track_list_ext(
    ui: &mut Ui,
    id: &str,
    tracks: &[TrackRow],
    lead: Lead,
    from: &PlayFrom,
    opts: TrackListOpts,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    let current = state.player.current.as_ref().map(|t| t.id);
    let playing = state.player.status == Status::Playing;
    let two_line = lead == Lead::Cover || is_compilation(tracks);
    let mut rows: Vec<Rect> = Vec::with_capacity(tracks.len());
    let (mut dragging, mut dropped) = (None, None);
    for (i, t) in tracks.iter().enumerate() {
        let is_cur = current == Some(t.id);
        let leading = match lead {
            Lead::Number if is_cur => Leading::Number(String::new()),
            Lead::Number => Leading::Number(if t.track_no > 0 {
                t.track_no.to_string()
            } else {
                (i + 1).to_string()
            }),
            Lead::Cover => Leading::Image(t.cover.as_ref().map(|c| c.uri()), Icon::MusicNote),
        };
        let mut supporting = t.artist.clone();
        if lead == Lead::Cover
            && let Some(a) = &t.album
        {
            supporting = format!("{supporting} · {a}");
        }
        let dur = t.duration.map(common::fmt_time).unwrap_or_default();
        let status = (!t.has_local).then_some(match t.source {
            Some(SourceKind::Spotify | SourceKind::Youtube) => "стрим",
            _ => "нет файла",
        });
        let download = format!("{} {}", t.artist, t.album.as_deref().unwrap_or(&t.title));
        let track_id = t.id;
        let (liked, has_local) = (t.liked, t.has_local);
        let mut menu_resp: Option<Response> = None;
        let mut item = ListItem::new(&t.title)
            .leading(leading)
            .selected(is_cur)
            .muted(!has_local)
            .enabled(true)
            .trailing(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let m = ui.add(
                        IconButton::new(Icon::MoreVert)
                            .size(Size::Xs)
                            .tooltip("Ещё")
                            .id_salt(("track-more", id, i)),
                    );
                    menu_resp = Some(m);
                    if !has_local
                        && ui
                            .add(
                                IconButton::new(Icon::Download)
                                    .size(Size::Xs)
                                    .tooltip("Скачать")
                                    .id_salt(("track-dl", id, i)),
                            )
                            .clicked()
                    {
                        out.push(Command::SearchTorrents(download));
                    }
                    if ui
                        .add(
                            IconButton::new(Icon::Favorite)
                                .size(Size::Xs)
                                .selected(liked)
                                .tooltip("Нравится")
                                .id_salt(("track-like", id, i)),
                        )
                        .clicked()
                    {
                        out.push(Command::Like(LikeTarget::Track(track_id), !liked));
                    }
                    let variant = Theme::get(ui.ctx()).scheme.on_surface_variant;
                    ui.label(TypeRole::BodyMedium.style().rich(dur, variant));
                    if let Some(st) = status {
                        ui.label(TypeRole::LabelSmall.style().rich(st, variant));
                    }
                });
            })
            .id_salt(("track", id, i, t.id));
        if two_line {
            item = item.supporting(supporting);
        }
        if opts.reorder {
            item = item.drag_handle(true);
        }
        let o = item.show(ui);
        rows.push(o.rect);
        if is_cur {
            let (center, backdrop) = match lead {
                Lead::Number => (pos2(o.rect.left() + 16.0 + 14.0, o.rect.center().y), None),
                Lead::Cover => {
                    let c = pos2(o.rect.left() + 16.0 + 28.0, o.rect.center().y);
                    (c, Some(Rect::from_center_size(c, vec2(56.0, 56.0))))
                }
            };
            if let Some(b) = backdrop {
                paint::container(
                    ui.painter(),
                    b,
                    Radii::all(shape::SMALL),
                    s.secondary_container.gamma_multiply(0.85),
                    Stroke::NONE,
                );
            }
            Equalizer::new(("eq", id, i), s.on_secondary_container)
                .playing(playing)
                .paint_at(ui, Rect::from_center_size(center, vec2(24.0, 24.0)));
        }
        if let Some(h) = &o.handle {
            if h.dragged() {
                dragging = Some(i);
            }
            if h.drag_stopped() {
                dropped = Some(i);
            }
        }
        let resp = o.response;
        // Desktop convention: double-click plays; on the playing row it pauses.
        if resp.double_clicked() {
            out.push(if is_cur {
                Command::TogglePlay
            } else {
                play_from(from, tracks, i)
            });
        }
        let menu_key = Id::new(("pl-track-menu", id, i));
        match opts.playlist {
            Some(pl) => {
                if let Some(m) = &menu_resp {
                    let picked = playlist_track_menu(menu_key, t, pl).attach(m);
                    nav = playlist_track_pick(picked, t, pl, Some(m.rect), pages, out).or(nav);
                }
                let picked = playlist_track_menu(menu_key.with("ctx"), t, pl).context_menu(&resp);
                nav = playlist_track_pick(picked, t, pl, None, pages, out).or(nav);
            }
            None => {
                if let Some(m) = &menu_resp {
                    nav = pages.track_menu_attach(m, t, state, out).or(nav);
                }
                nav = pages.track_context_menu(&resp, t, state, out).or(nav);
            }
        }
    }
    if let Some(pl) = opts.playlist.filter(|_| opts.reorder)
        && let Some(i) = dragging.or(dropped)
    {
        reorder(ui, pl, tracks, &rows, i, dragging.is_some(), out);
    }
    nav
}

/// ⋮ / right-click menu of a playlist row.
fn playlist_track_menu(key: Id, track: &TrackRow, playlist: PlaylistId) -> Menu {
    Menu::new(key)
        .align_right(true)
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .item(MenuItem::new("К альбому").icon(Icon::Album).enabled(track.release_id.is_some()))
        .divider()
        .item(
            MenuItem::new(if playlist == LIKED_PLAYLIST {
                "Убрать из Любимых"
            } else {
                "Убрать из плейлиста"
            })
            .icon(Icon::Delete),
        )
}

fn playlist_track_pick(
    picked: Option<usize>,
    track: &TrackRow,
    playlist: PlaylistId,
    anchor: Option<Rect>,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    match picked? {
        0 => out.push(Command::PlayNext(vec![track.id])),
        1 => out.push(Command::Enqueue(vec![track.id])),
        2 => {
            pages.pending_add = Some(vec![track.id]);
            pages.pending_add_anchor = anchor;
        }
        3 => return track.release_id.map(Route::Release),
        4 => out.push(Command::RemoveFromPlaylist(playlist, track.id)),
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

/// Lays cards in a wrapping grid; cards get stretched to fill the row.
pub fn grid<T>(
    ui: &mut Ui,
    items: &[T],
    min_card: f32,
    mut draw: impl FnMut(&mut Ui, &T, f32) -> Option<Route>,
) -> Option<Route> {
    let width = ui.available_width();
    let cols = columns(width, min_card);
    let card = ((width - CARD_GAP * (cols - 1) as f32) / cols as f32).floor();
    let mut nav = None;
    for row in items.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for it in row {
                nav = draw(ui, it, card).or(nav.take());
            }
        });
        ui.add_space(CARD_GAP);
    }
    nav
}
