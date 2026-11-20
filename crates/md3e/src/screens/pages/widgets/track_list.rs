use super::*;

/// Where a track list plays from.
#[derive(Clone, Debug)]
pub enum PlayFrom {
    Release(nikgob_view::ReleaseId),
    /// The page of an album only some songs of which are the user's:
    /// «Весь альбом» (true) or «Мои треки».
    ReleaseAs(nikgob_view::ReleaseId, bool),
    Playlist(nikgob_view::PlaylistId),
    Tracks(Vec<nikgob_view::TrackId>, nikgob_view::QueueOrigin),
    /// Library rows of the search: the row played is a search's pick
    /// ([`nikgob_view::Prefs::search_play`] says what follows it).
    SearchRows(nikgob_view::QueueOrigin),
    /// The merged «Всё» search: a queue of library and stream tracks,
    /// started at `at` (the row's own place in it).
    Search {
        picks: Vec<nikgob_view::SearchPick>,
        at: usize,
        context: nikgob_view::QueueOrigin,
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
    /// «Весь альбом» of an album only some songs of which are the user's:
    /// what each track is to them; a row puts its song in (+) or takes it
    /// out (✓) of their songs.
    pub album_marks: Option<&'a [AlbumMark]>,
    /// The line other versions of the album's songs start at: a «Другие
    /// версии» header above it.
    pub versions_at: Option<usize>,
    /// The switch of that header: whether they play with the album
    /// (`Command::AlbumVersions`).
    pub versions_on: bool,
    /// A place without a track has no ⬇ (its album is not the user's
    /// whole).
    pub no_hole_download: bool,
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
    let versions_at = opts.versions_at.filter(|&at| at < lines.len());
    let wish = WishIndex::of(ui.ctx(), &state.downloads.wishlist);
    let album = match from {
        PlayFrom::Release(r) | PlayFrom::ReleaseAs(r, _) => Some(*r),
        _ => None,
    };
    let mut row = |ui: &mut Ui, j: usize, nav: &mut Option<Route>| -> Option<Response> {
        let i = match lines[j] {
            ReleaseLine::Track(i) => i,
            ReleaseLine::Hole { no, .. } => {
                let album = album.filter(|_| !opts.no_hole_download);
                return Some(hole_row(ui, id, j, no, album, &wish, state, out));
            }
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
    let mut versions_toggle = None;
    let rects: Vec<Rect> = if discs || versions_at.is_some() {
        let mut rects = Vec::with_capacity(lines.len());
        for j in 0..lines.len() {
            let disc = disc_of(&lines[j]);
            let versions_head = versions_at == Some(j);
            let head = if versions_head {
                Some(i18n::pages().versions_head.to_owned())
            } else if discs && versions_at.is_none_or(|at| j < at) && (j == 0 || disc_of(&lines[j - 1]) != disc) {
                Some(i18n::pages::fmt_disc(disc))
            } else {
                None
            };
            if let Some(head) = head {
                ui.add_space(if j == 0 { 4.0 } else { 16.0 });
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(TypeRole::TitleSmall.style().rich(head, s.on_surface_variant));
                    // Whether they play with the album, on its row's right
                    // (under the ⋮ column of the tracks).
                    if versions_head && let Some(id) = album {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(16.0);
                            let mut on = opts.versions_on;
                            let sw = ui.add(crate::components::Switch::new(&mut on).id_salt(("rel-versions", id)));
                            if sw.changed() {
                                versions_toggle = Some((id, on));
                            }
                        });
                    }
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
    if let Some((id, on)) = versions_toggle {
        out.push(Command::AlbumVersions { id, on });
    }
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
    let title = if no > 0 { i18n::pages::fmt_track_no(no) } else { i18n::pages().track_no_number.to_owned() };
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
                            .tooltip(i18n::pages().dl_missing_tip)
                            .id_salt(("hole-dl", id, j));
                        if ui.add(dl).clicked() {
                            out.push(Command::DownloadRelease(r));
                        }
                    }
                    (None, None) => {}
                }
                let variant = Theme::get(ui.ctx()).scheme.on_surface_variant;
                ui.label(TypeRole::LabelSmall.style().rich(i18n::pages().no_file, variant));
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
        supporting = format!("{supporting} · {}", i18n::pages().clip);
    }
    if opts.mark_local && t.has_local {
        supporting = format!("{supporting} · {}", i18n::pages().you_have);
    }
    let dur = t.duration.map(common::fmt_time).unwrap_or_default();
    // No file: it streams (a ⬇ to «Хочу скачать», or how far it is there),
    // or nothing plays it at all («нет файла»).
    let streams = matches!(t.source, Some(SourceKind::Spotify | SourceKind::Youtube));
    let wished = (!t.has_local).then(|| wish.of_track(Some(t.id), t.release_id)).flatten();
    let track_id = t.id;
    // «Весь альбом»: the song is one of the user's (✓, a click takes it out)
    // or not (+, a click puts it in).
    let mark = opts.album_marks.and_then(|m| m.get(i).copied()).zip(t.release_id);
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
                let m = ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip(i18n::common().more).id_salt(("track-more", id, i)));
                menu_resp = Some(m);
                if let Some((mark, release)) = mark {
                    let mine = mark == AlbumMark::Mine;
                    let b = IconButton::new(if mine { Icon::Check } else { Icon::Add })
                        .size(Size::Xs)
                        .selected(mine)
                        .tooltip(if mine { i18n::pages().mine_remove } else { i18n::pages().mine_add })
                        .id_salt(("track-mine", id, i));
                    if ui.add(b).clicked() {
                        out.push(Command::AlbumTrackMine { release, track: track_id, mine: !mine });
                    }
                }
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
                                        .tooltip(i18n::pages().dl_tip)
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
                            .tooltip(super::super::like_label(liked))
                            .id_salt(("track-like", id, i)),
                    )
                    .clicked()
                {
                    out.push(Command::Like(LikeTarget::Track(track_id), !liked));
                }
                let variant = Theme::get(ui.ctx()).scheme.on_surface_variant;
                ui.label(TypeRole::BodyMedium.style().rich(dur, variant));
                if !has_local && !streams {
                    ui.label(TypeRole::LabelSmall.style().rich(i18n::pages().no_file, variant));
                } else if !has_local && opts.mark_missing {
                    ui.label(TypeRole::LabelSmall.style().rich(i18n::pages().stream, variant));
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
        St::Waiting | St::New | St::Searching if !torrents => (Icon::HourglassEmpty, i18n::pages().wish_wait_torrents),
        St::Waiting => (Icon::HourglassEmpty, i18n::pages().wish_wait),
        St::New | St::Searching => (Icon::Search, i18n::pages().wish_searching),
        St::Confirm => (Icon::Info, i18n::pages().wish_confirm),
        St::Downloading => (Icon::CloudDownload, i18n::pages().wish_downloading),
        St::NoTorrent | St::Failed => (Icon::Error, i18n::pages().wish_not_found),
        St::Done | St::Dismissed => (Icon::Check, i18n::pages().wish_done),
    };
    ui.add(IconButton::new(icon).size(Size::Xs).tooltip(tip).id_salt(Id::new(salt)))
}

/// ⋮ / right-click menu of a playlist row: the library track's
/// [`super::super::library_track_rows`], then «Убрать из плейлиста» last, with
/// «Разные песни» when the track has it.
pub(crate) fn playlist_track_rows(track: &TrackRow, playlist: PlaylistId, of: super::super::LibraryRowsOf<'_>) -> Vec<Row<super::super::TrackAct>> {
    let mut rows = super::super::library_track_rows(track, of);
    if !matches!(rows.last(), Some(Row::Item(super::super::TrackAct::Apart, _))) {
        rows.push(Row::Divider);
    }
    let remove = if playlist == LIKED_PLAYLIST { i18n::menus().unlike } else { i18n::menus().remove_from_playlist };
    rows.push(Row::Item(super::super::TrackAct::Remove, MenuItem::new(remove).icon(Icon::Delete)));
    rows
}

fn playlist_track_menu(
    ctx: &egui::Context,
    key: Id,
    track: &TrackRow,
    playlist: PlaylistId,
    state: &ViewState,
    wish: &WishIndex,
) -> (Menu, Vec<super::super::TrackAct>) {
    let rows = playlist_track_rows(track, playlist, super::super::LibraryRowsOf::of(state, track, wish));
    let rows = super::super::with_speed_row(rows, &state.speeds, super::super::is_playing(state, track.id));
    let also = state.also_on.get(&track.id).map(|(r, _)| *r);
    let rows = super::super::here(ctx).trim(rows, track, also, Some);
    let rows = super::super::placed(rows, &state.prefs.track_menu, |a| a.menu_item(track.has_local));
    let (menu, acts) = menu_of(key, rows);
    (menu.align_right(true), acts)
}

fn playlist_track_pick(
    ctx: &egui::Context,
    act: Option<super::super::TrackAct>,
    track: &TrackRow,
    playlist: PlaylistId,
    anchor: Option<Rect>,
    pages: &mut PageState,
    state: &ViewState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    match act? {
        super::super::TrackAct::Remove => {
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
        PlayFrom::ReleaseAs(id, true) => Command::PlayReleaseWhole {
            id: *id,
            start: Some(tracks[i].id),
            shuffle: false,
        },
        PlayFrom::ReleaseAs(id, false) => Command::PlayReleaseMine {
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
        PlayFrom::SearchRows(context) => Command::PlaySearch {
            tracks: tracks.iter().map(|t| nikgob_view::SearchPick::Library(t.id)).collect(),
            start: i,
            context: context.clone(),
        },
    }
}

