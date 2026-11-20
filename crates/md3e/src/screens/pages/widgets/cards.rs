use super::*;

/// "3 из 12" when a release is incomplete locally. Never for an album only
/// some songs of which are the user's: the rest is not theirs to lack.
pub fn progress_badge(r: &ReleaseCard) -> Option<String> {
    if r.mine.is_some() {
        return None;
    }
    match r.total_tracks {
        Some(t) if r.local_tracks < t => Some(i18n::pages::fmt_of(r.local_tracks, t)),
        _ => None,
    }
}

/// The badge on an album card: "стрим" when the library has none of its
/// files (it is liked or in a playlist from a service), else "3 из 12";
/// of an album only some songs of which are the user's, "стрим" while none
/// of those songs has a file, else nothing.
pub fn card_badge(r: &ReleaseCard) -> Option<String> {
    if r.mine.is_some() {
        (r.mine_local == 0).then(|| i18n::pages().stream.to_owned())
    } else if r.local_tracks == 0 {
        Some(i18n::pages().stream.to_owned())
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

/// The key of what the page of album `id` shows this visit: «Весь альбом»
/// (true) or «Мои треки».
fn whole_key(id: ReleaseId) -> Id {
    Id::new(("rel-view", id))
}

/// The key of the pass the page of album `id` was last drawn in.
fn whole_seen_key(id: ReleaseId) -> Id {
    Id::new(("rel-view-seen", id))
}

/// The page of album `id` shows «Весь альбом» (or «Мои треки») until
/// switched or left.
pub fn show_whole(ctx: &egui::Context, id: ReleaseId, whole: bool) {
    ctx.data_mut(|d| d.insert_temp(whole_key(id), whole));
}

/// What the page of album `id` shows this visit when switched on it
/// ([`show_whole`]); `None`: as the album opens (its own choice, else the
/// setting: every place the same). Called once per pass the page is drawn
/// in: a visit after a gap starts again from how the album opens.
pub fn visit_whole(ctx: &egui::Context, id: ReleaseId) -> Option<bool> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let seen = d.get_temp::<u64>(whole_seen_key(id));
        d.insert_temp(whole_seen_key(id), pass);
        if seen.is_none_or(|s| pass > s + 2) {
            d.remove::<bool>(whole_key(id));
        }
        d.get_temp::<bool>(whole_key(id))
    })
}

/// The like of an album: into (or out of) «Любимые альбомы».
pub fn album_like_label(liked: bool) -> &'static str {
    super::super::like_label(liked)
}

//// What the menu of an album does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseAct {
    Play,
    Shuffle,
    /// «Радио по альбому».
    Radio,
    Next,
    Queue,
    Playlist,
    Like,
    Artist,
    Share,
    /// «Слушать весь альбом» of an album only some songs of which are the
    /// user's: once, nothing added.
    PlayWhole,
    /// «Добавить в библиотеку»: the whole album, nothing downloads.
    AddWhole,
    /// «Оставить только мои треки»: the album added whole goes back to the
    /// songs chosen one by one.
    KeepMine,
    /// «Скачать недостающие»: the tracks it lacks to «Хочу скачать» (of an
    /// album only some songs of which are the user's, those songs).
    Download,
    /// «Скачать весь альбом» of an album only some songs of which are the
    /// user's.
    DownloadWhole,
    /// «Найти качество лучше»: a better copy of its files that can get one.
    Better,
    /// «Обновить обложку»: its art picked again, from its files and online.
    RefreshCover,
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
    let partial = r.mine.is_some();
    if !on_page {
        rows.push(Row::Item(A::Play, MenuItem::new(i18n::common().play).icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new(i18n::common().shuffle).icon(Icon::Shuffle)));
    }
    if partial {
        rows.push(Row::Item(A::PlayWhole, MenuItem::new(i18n::menus().play_whole_album).icon(Icon::Album)));
    }
    rows.push(Row::Item(A::Radio, MenuItem::new(i18n::menus().radio_by_album).icon(Icon::Radio)));
    rows.push(Row::Item(A::Next, MenuItem::new(i18n::menus().play_next).icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new(i18n::menus().to_queue).icon(Icon::QueueMusic)));
    rows.push(Row::Divider);
    if !on_page {
        rows.push(Row::Item(A::Like, MenuItem::new(album_like_label(r.liked)).icon(Icon::Favorite).selected(r.liked)));
    }
    rows.push(Row::Item(A::Playlist, MenuItem::new(i18n::menus().to_playlist).icon(Icon::PlaylistAdd)));
    if partial {
        rows.push(Row::Item(A::AddWhole, MenuItem::new(i18n::menus().add_album).icon(Icon::LibraryMusic)));
    } else if r.whole && !on_page {
        rows.push(Row::Item(A::KeepMine, MenuItem::new(i18n::menus().keep_mine).icon(Icon::LibraryMusic)));
    }
    rows.push(Row::Divider);
    if !r.artists.linked().is_empty() {
        rows.push(Row::Item(A::Artist, MenuItem::new(i18n::menus().to_artist).icon(Icon::Person)));
    }
    rows.push(Row::Item(A::Share, MenuItem::new(i18n::menus().share).icon(Icon::Share)));
    rows.push(Row::Item(A::RefreshCover, MenuItem::new(i18n::menus().refresh_cover).icon(Icon::Refresh)));
    // An album only some songs of which are the user's lacks only those
    // without a file; the whole of it is a download of its own.
    let download = !on_page && (progress_badge(r).is_some() || r.mine.is_some_and(|n| r.mine_local < n));
    let better = r.better && r.local_tracks > 0;
    if download || better || partial {
        rows.push(Row::Divider);
    }
    if download {
        rows.push(Row::Item(A::Download, MenuItem::new(i18n::menus().download_missing).icon(Icon::Download)));
    }
    if partial {
        rows.push(Row::Item(A::DownloadWhole, MenuItem::new(i18n::menus().download_whole_album).icon(Icon::Download)));
    }
    if better {
        rows.push(Row::Item(A::Better, MenuItem::new(i18n::menus().find_better).icon(Icon::Search)));
    }
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Trash, MenuItem::new(i18n::menus().delete_album).icon(Icon::Delete)));
    if r.local_tracks > 0 {
        rows.push(Row::Item(A::Purge, MenuItem::new(i18n::menus().delete_album_forever).icon(Icon::Delete)));
    }
    rows
}

/// Does an album action (the page opens «В плейлист…» with its tracks).
pub fn release_apply(ctx: &egui::Context, act: ReleaseAct, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    match act {
        ReleaseAct::Play => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
        ReleaseAct::Shuffle => out.push(Command::PlayRelease { id: r.id, start: None, shuffle: true }),
        ReleaseAct::Radio => out.push(Command::StartRadio { seed: nikgob_view::RadioSeed::Album(r.id) }),
        ReleaseAct::Next => out.push(Command::PlayReleaseNext(r.id)),
        ReleaseAct::Queue => out.push(Command::EnqueueRelease(r.id)),
        ReleaseAct::Like => out.push(Command::Like(LikeTarget::Release(r.id), !r.liked)),
        // Several artists: a chooser.
        ReleaseAct::Artist => return artist_links::go_to(ctx, artist_links::library(&r.artists)),
        ReleaseAct::Share => out.push(Command::Share(nikgob_view::ShareTarget::Release(r.id))),
        ReleaseAct::PlayWhole => out.push(Command::PlayReleaseWhole { id: r.id, start: None, shuffle: false }),
        ReleaseAct::AddWhole => out.push(Command::AddWholeAlbum { id: r.id, on: true }),
        ReleaseAct::KeepMine => out.push(Command::AddWholeAlbum { id: r.id, on: false }),
        ReleaseAct::Download => out.push(Command::DownloadRelease(r.id)),
        ReleaseAct::DownloadWhole => out.push(Command::DownloadWholeRelease(r.id)),
        ReleaseAct::Better => out.push(Command::FindBetterAlbum(r.id)),
        ReleaseAct::RefreshCover => out.push(Command::RefreshReleaseCover(r.id)),
        ReleaseAct::Trash => super::super::request_release_delete(ctx, r.id, &r.title, false),
        ReleaseAct::Purge => super::super::request_release_delete(ctx, r.id, &r.title, true),
        ReleaseAct::Playlist => {
            let anchor = ctx.input(|i| i.pointer.latest_pos()).map(|p| egui::Rect::from_center_size(p, egui::vec2(1.0, 1.0)));
            super::super::request_release_to_playlist(ctx, r.id, anchor);
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
    let here = super::super::here(&resp.ctx);
    let rows = || super::super::retain_rows(release_rows(r, false), &|a: ReleaseAct| a != ReleaseAct::Artist || !here.artists(&r.artists).is_empty());
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
    let mut subtitle = i18n::tracks_count(u64::from(p.track_count));
    if let Some(s) = p.service {
        subtitle = format!("{} · {subtitle}", s.display_name());
    }
    let o = MediaCard::new(p.display_name())
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

/// What the menu of a playlist does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistAct {
    Play,
    Shuffle,
    /// «Радио по плейлисту».
    Radio,
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
/// it does; «Выключить синхронизацию» (the link stays, the playlist stays
/// as it is, nothing syncs) stops it.
pub fn sync_menu_label(m: nikgob_view::PlaylistSyncMode) -> &'static str {
    use nikgob_view::PlaylistSyncMode as M;
    match m {
        M::TwoWay => i18n::menus().sync_two_way,
        M::Follow => i18n::menus().sync_follow,
        M::Once => i18n::menus().sync_off,
    }
}

/// The sync mode in the playlist page's line under the title.
pub fn sync_state_label(m: nikgob_view::PlaylistSyncMode) -> &'static str {
    use nikgob_view::PlaylistSyncMode as M;
    match m {
        M::TwoWay => i18n::pages().sync_state_two_way,
        M::Follow => i18n::pages().sync_state_follow,
        M::Once => i18n::pages().sync_state_off,
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
        rows.push(Row::Item(A::Play, MenuItem::new(i18n::common().play).icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new(i18n::common().shuffle).icon(Icon::Shuffle)));
    }
    rows.push(Row::Item(A::Radio, MenuItem::new(i18n::menus().radio_by_playlist).icon(Icon::Radio)));
    rows.push(Row::Item(A::Next, MenuItem::new(i18n::menus().play_next).icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new(i18n::menus().to_queue).icon(Icon::QueueMusic)));
    if o.editable {
        rows.push(Row::Divider);
        // The page edits the name and the cover; a card renames.
        let edit = if o.on_page { i18n::menus().edit_item } else { i18n::menus().rename_item };
        rows.push(Row::Item(A::Edit, MenuItem::new(edit).icon(Icon::Edit)));
    }
    if let Some((service, mode)) = o.sync {
        use nikgob_view::PlaylistSyncMode as M;
        rows.push(Row::Divider);
        rows.push(Row::Label(i18n::menus::fmt_sync_with(service)));
        // The two ways it can sync, the one on marked; while it is off
        // neither is, and a pick turns it on again.
        for m in [M::TwoWay, M::Follow] {
            rows.push(Row::Item(A::Sync(m), MenuItem::new(sync_menu_label(m)).icon(Icon::Sync).selected(m == mode)));
        }
        // Off is an action of its own while it syncs, not a third way.
        if mode != M::Once {
            rows.push(Row::Item(A::Sync(M::Once), MenuItem::new(sync_menu_label(M::Once)).icon(Icon::Block)));
        }
    }
    let link = o.sync.is_none() && o.can_link;
    if link || o.editable {
        rows.push(Row::Divider);
    }
    if link {
        rows.push(Row::Item(A::Link, MenuItem::new(i18n::menus().link_service).icon(Icon::Sync)));
    }
    if o.editable {
        rows.push(Row::Item(A::Delete, MenuItem::new(i18n::menus().delete_playlist).icon(Icon::Delete)));
    }
    rows
}

/// The command of a playlist action that needs no dialog (not «Изменить»,
/// «Связать с сервисом…» or «Удалить»).
pub fn playlist_command(act: PlaylistAct, id: PlaylistId) -> Option<Command> {
    Some(match act {
        PlaylistAct::Play => Command::PlayPlaylist { id, start: None, shuffle: false },
        PlaylistAct::Shuffle => Command::PlayPlaylist { id, start: None, shuffle: true },
        PlaylistAct::Radio => Command::StartRadio { seed: nikgob_view::RadioSeed::Playlist(id) },
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
pub fn role_colors(role: super::super::shelf::Role, s: &crate::ColorScheme) -> (Color32, Color32) {
    use super::super::shelf::Role;
    match role {
        Role::Primary => (s.primary_container, s.on_primary_container),
        Role::Secondary => (s.secondary_container, s.on_secondary_container),
        Role::Tertiary => (s.tertiary_container, s.on_tertiary_container),
        Role::Surface => (s.surface_container_highest, s.on_surface_variant),
    }
}

/// An artist's avatar: the photo in a circle, or the initials in the role
/// and shape the name picks ([`super::super::shelf::artist_look`]).
pub fn artist_picture(ui: &Ui, a: &ArtistCard, rect: Rect) {
    let s = Theme::get(ui.ctx()).scheme;
    let uri = a.image.as_ref().map(|c| c.uri());
    let (role, shape) = super::super::shelf::artist_look(&a.name);
    let (fill, on) = role_colors(role, &s);
    let shape = if uri.is_some() { MaterialShape::Circle } else { shape };
    crate::components::paint_shape_avatar(ui, rect, uri.as_deref(), &initials(&a.name), shape, fill, on);
}

/// The list row's leading avatar of an artist, as [`artist_picture`].
pub fn artist_leading(ui: &Ui, a: &ArtistCard) -> Leading {
    let s = Theme::get(ui.ctx()).scheme;
    let uri = a.image.as_ref().map(|c| c.uri());
    let (role, shape) = super::super::shelf::artist_look(&a.name);
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
        n => i18n::tracks_count(u64::from(n)),
    }
}

/// The line under an artist's name: its Latin name when its own is in a
/// script the user may not read, then «12 треков» in the library
/// («Ayako Saso · 12 треков»); empty when it has neither.
pub fn artist_caption(a: &ArtistCard) -> String {
    let tracks = artist_tracks(a);
    match &a.latin {
        Some(l) if tracks.is_empty() => l.clone(),
        Some(l) => format!("{l} · {tracks}"),
        None => tracks,
    }
}

/// What an artist is on a tile and in a search's top result, with its
/// Latin name: «Исполнитель · Ayako Saso», a role «Персонаж · Riko».
pub fn artist_kind(a: &ArtistCard) -> String {
    let kind = if a.character { i18n::pages().character } else { i18n::sound_facts().tag_artist };
    match &a.latin {
        Some(l) => format!("{kind} · {l}"),
        None => kind.to_owned(),
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
    // Under the name: its Latin name, and in the library the track count.
    let caption = artist_caption(a);
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
                    .tooltip(i18n::menus().play_mix)
                    .id_salt(("artist-play-btn", a.id)),
            )
            .clicked();
    }
    // ⋮ at the avatar's top right, as on the other cards.
    let more = super::super::cards::more_button(ui, id, hover, card_more_slot(img));
    super::super::cards::keep_more_open(ui.ctx(), id, more.as_ref());
    let menu = artist_menus(&resp, more.as_ref(), a, out);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if play {
        out.push(Command::PlayArtist { id: a.id, shuffle: false });
    }
    let more_clicked = more.as_ref().is_some_and(Response::clicked);
    (resp.clicked() && !play && !more_clicked).then_some(Route::Artist(a.id)).or(menu)
}

/// How tall [`artist_avatar`] is for `a` at `size` (the name, and the
/// line under it when it has one: [`artist_caption`]).
pub fn artist_avatar_height(a: &ArtistCard, size: f32) -> f32 {
    size + if artist_caption(a).is_empty() { 28.0 } else { 48.0 }
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
    Next,
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
        rows.push(Row::Item(A::Play, MenuItem::new(i18n::menus().play_mix).icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new(i18n::common().shuffle).icon(Icon::Shuffle)));
        rows.push(Row::Item(A::Radio, MenuItem::new(i18n::menus().radio_artist).icon(Icon::Radio)));
    }
    rows.push(Row::Item(A::Next, MenuItem::new(i18n::menus().play_next).icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new(i18n::menus().to_queue).icon(Icon::QueueMusic)));
    rows.push(Row::Divider);
    if !on_page {
        rows.push(Row::Item(A::Follow, MenuItem::new(if a.liked { i18n::menus().unfollow } else { i18n::menus().follow }).icon(Icon::Favorite)));
    }
    rows.push(Row::Item(A::Ban, super::super::ban_item(a.id, marks)));
    rows
}

/// Does an artist action.
pub fn artist_apply(act: ArtistAct, a: &ArtistCard, marks: &nikgob_view::DislikesView, out: &mut Vec<Command>) {
    out.push(match act {
        ArtistAct::Play => Command::PlayArtist { id: a.id, shuffle: false },
        ArtistAct::Shuffle => Command::PlayArtist { id: a.id, shuffle: true },
        ArtistAct::Radio => Command::StartRadio { seed: nikgob_view::RadioSeed::Artist(a.id) },
        ArtistAct::Next => Command::PlayArtistNext(a.id),
        ArtistAct::Queue => Command::EnqueueArtist(a.id),
        ArtistAct::Follow => Command::Like(LikeTarget::Artist(a.id), !a.liked),
        ArtistAct::Ban => super::super::ban_command(a.id, marks),
    });
}

/// Right-click menu of an artist (card or row): [`artist_rows`].
pub fn artist_menu(resp: &Response, a: &ArtistCard, out: &mut Vec<Command>) {
    artist_menus(resp, None, a, out);
}

/// The menu of an artist on a right click on `resp` and on its ⋮
/// (`more`): the same [`artist_rows`].
fn artist_menus(resp: &Response, more: Option<&Response>, a: &ArtistCard, out: &mut Vec<Command>) -> Option<Route> {
    let marks = super::super::marks(&resp.ctx);
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

