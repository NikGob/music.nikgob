//! Главная (md3e-player.md §3.1, docs/stage4.md §4–§5, §12.1): greeting,
//! filter chips, the bento board (the daylist, «Твой микс» and the smaller
//! mixes), then feed rows: moods, the albums to continue, style and artist
//! mixes, new music from the services and the library's own rows.

use egui::{Color32, Rect, Ui, vec2};
use nikgob_shapes::MaterialShape;
use nikgob_view::{
    AnalysisProgress, Command, ContinueCard, HomeView, LibraryTab, MixCard, MixId, Mood, RecTarget, Route, Service,
    StreamPage, StreamTrackRow, TrackId, ViewState, WishlistStatus,
};

use super::{PageState, bento, widgets};
use crate::{
    ColorScheme, Icon, Theme, TypeRole,
    components::{CoverStatus, FilterChip, MediaCard, Menu, MenuItem, Style},
};

fn greeting(hour: u8) -> &'static str {
    match hour {
        5..=11 => "Доброе утро",
        12..=16 => "Добрый день",
        17..=22 => "Добрый вечер",
        _ => "Доброй ночи",
    }
}

/// The filter chips over the feed: which sections Главная shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    /// Everything.
    #[default]
    All,
    /// What comes from the library: the board, what to continue, the style
    /// and artist mixes, the forgotten and the recently added.
    Mine,
    /// What is new: discoveries, new releases, recently added.
    New,
    /// The moods and the style mixes: music for how it should feel.
    Mood,
}

impl Filter {
    pub const ALL: [Self; 4] = [Self::All, Self::Mine, Self::New, Self::Mood];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "Всё",
            Self::Mine => "Моё",
            Self::New => "Новое",
            Self::Mood => "Под настроение",
        }
    }
}

/// The rows of Главная, in the order they are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// The bento board: the daylist, «Твой микс» and the smaller mixes.
    Board,
    Moods,
    Continue,
    StyleMixes,
    ArtistMixes,
    Discover,
    NewReleases,
    LongUnplayed,
    RecentlyAdded,
}

impl Section {
    const ORDER: [Self; 9] = [
        Self::Board,
        Self::Moods,
        Self::Continue,
        Self::StyleMixes,
        Self::ArtistMixes,
        Self::Discover,
        Self::NewReleases,
        Self::LongUnplayed,
        Self::RecentlyAdded,
    ];

    /// The section belongs to what `filter` shows.
    fn in_filter(self, filter: Filter) -> bool {
        use Section::*;
        match filter {
            Filter::All => true,
            Filter::Mine => matches!(self, Board | Continue | StyleMixes | ArtistMixes | LongUnplayed | RecentlyAdded),
            Filter::New => matches!(self, Discover | NewReleases | RecentlyAdded),
            Filter::Mood => matches!(self, Moods | StyleMixes),
        }
    }

    /// The section has something to show: its cards, or (under its own
    /// chip) a quiet line on why it is empty and what to do. «Все» shows
    /// an empty «Новое для тебя» only when there is something to do about
    /// it (the services that look for new music are off).
    fn shown(self, filter: Filter, h: &HomeView, discovery_off: bool) -> bool {
        use Section::*;
        if !self.in_filter(filter) {
            return false;
        }
        match self {
            Board => h.daylist.is_some() || h.hero.is_some() || !h.mix.is_empty() || !h.tiles.is_empty(),
            Moods => !h.moods.is_empty() || filter == Filter::Mood,
            Continue => !h.continue_listening.is_empty(),
            StyleMixes => !h.style_mixes.is_empty(),
            ArtistMixes => !h.artist_mixes.is_empty(),
            Discover => !h.discover.is_empty() || filter == Filter::New || discovery_off,
            NewReleases => !h.new_releases.is_empty(),
            LongUnplayed => !h.long_unplayed.is_empty(),
            RecentlyAdded => !h.recently_added.is_empty(),
        }
    }
}

/// The sections `filter` shows for `h`, top to bottom.
fn sections(filter: Filter, h: &HomeView, discovery_off: bool) -> Vec<Section> {
    Section::ORDER.into_iter().filter(|s| s.shown(filter, h, discovery_off)).collect()
}

/// A short plain line under a mood's name: what it sounds like.
pub fn mood_line(mood: Mood) -> &'static str {
    match mood {
        Mood::Calm => "тихое и неспешное",
        Mood::Energetic => "быстрое и громкое",
        Mood::Dark => "тёмное, глухое",
        Mood::Bright => "звонкое, мажорное",
        Mood::Focus => "ровное, без всплесков",
        Mood::Drive => "напор в одном темпе",
    }
}

/// A mood card's container and content colours (scheme roles): the quiet
/// moods secondary, the loud ones primary, «Светлое» tertiary, «Мрачное»
/// the highest surface.
fn mood_colors(mood: Mood, s: &ColorScheme) -> (egui::Color32, egui::Color32) {
    match mood {
        Mood::Calm | Mood::Focus => (s.secondary_container, s.on_secondary_container),
        Mood::Energetic | Mood::Drive => (s.primary_container, s.on_primary_container),
        Mood::Bright => (s.tertiary_container, s.on_tertiary_container),
        Mood::Dark => (s.surface_container_highest, s.on_surface),
    }
}

/// The shape a mood's cover is cut into.
fn mood_shape(mood: Mood) -> MaterialShape {
    match mood {
        Mood::Calm => MaterialShape::Puffy,
        Mood::Energetic => MaterialShape::SoftBurst,
        Mood::Dark => MaterialShape::Arch,
        Mood::Bright => MaterialShape::Sunny,
        Mood::Focus => MaterialShape::Pill,
        Mood::Drive => MaterialShape::Slanted,
    }
}

/// «Анализирую звук библиотеки — миксы станут точнее · 40%», while the
/// analysis runs.
pub fn analysis_line(a: &AnalysisProgress) -> Option<String> {
    (!a.complete()).then(|| {
        let pct = (a.fraction() * 100.0).floor() as u32;
        format!("Анализирую звук библиотеки — миксы станут точнее · {pct}%")
    })
}

/// Plays the discoveries from the `i`-th one, as a queue named after the
/// section.
fn play_discover(rows: &[StreamTrackRow], i: usize) -> Command {
    Command::PlayStreamTracks {
        tracks: rows.iter().map(|r| r.remote.clone()).collect(),
        start: i,
        context: "Новое для тебя".into(),
    }
}

/// «Не рекомендовать трек» / «… исполнителя» of a discovery.
fn dislike(t: &StreamTrackRow, artist: bool) -> Command {
    let target = if artist {
        RecTarget::StreamArtist(t.artist.clone())
    } else {
        RecTarget::StreamTrack(t.remote.clone())
    };
    Command::RecFeedback { target, dislike: true }
}

/// YouTube Music and Deezer, which find new music without any login, are
/// both switched off.
fn discovery_off(state: &ViewState) -> bool {
    let on = |svc| state.account(svc).is_some_and(|a| a.enabled);
    !state.accounts.is_empty() && !on(Service::Youtube) && !on(Service::Deezer)
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let h = &state.home;
    let mut nav = None;

    ui.add_space(8.0);
    ui.label(TypeRole::HeadlineMedium.style().rich(greeting(h.greeting_hour), s.on_surface));
    ui.add_space(8.0);

    let empty = h.mix.is_empty()
        && h.hero.is_none()
        && h.daylist.is_none()
        && h.tiles.is_empty()
        && h.recently_added.is_empty()
        && h.continue_listening.is_empty();
    if empty {
        // Nothing here yet: the search already plays anything from YouTube
        // Music, without a login; own files come in from a folder.
        if widgets::empty_state(
            ui,
            Icon::Search,
            "Начни с поиска",
            "Поиск сразу ищет и в YouTube Music — без входа. Понравилось — лайк, и трек останется у тебя.",
            Some("Найти музыку"),
        ) {
            pages.focus_search = true;
        }
        ui.vertical_centered(|ui| {
            if ui
                .add(crate::components::Button::new("Добавить свои файлы…").style(Style::Text).id_salt("home-add-files"))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Добавить папку").pick_folder()
            {
                out.push(Command::ImportFolder(dir));
            }
            if !connected(state)
                && ui
                    .add(
                        crate::components::Button::new("Подключить Spotify или YouTube Music")
                            .style(Style::Text)
                            .id_salt("home-connect"),
                    )
                    .clicked()
            {
                pages.open_setting(super::SettingId::SpotifyEnabled);
            }
        });
        return None;
    }

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for f in Filter::ALL {
            if ui.add(FilterChip::new(f.label(), pages.home_filter == f).id_salt(("home-chip", f.label()))).clicked() {
                pages.home_filter = f;
            }
        }
    });
    ui.add_space(16.0);

    let filter = pages.home_filter;
    let off = discovery_off(state);
    let shown = sections(filter, h, off);
    // The analysis line sits under the board; without one, over the feed.
    let analysis = analysis_line(&state.analysis);
    let mut analysis_said = false;
    for (n, section) in shown.iter().enumerate() {
        match section {
            Section::Board => {
                board(ui, h, out);
                if let Some(line) = &analysis {
                    ui.add_space(12.0);
                    quiet(ui, line);
                    analysis_said = true;
                }
            }
            Section::Moods => {
                // (Without moods their own line says how far it is.)
                if !analysis_said
                    && !h.moods.is_empty()
                    && let Some(line) = &analysis
                {
                    quiet(ui, line);
                    ui.add_space(16.0);
                    analysis_said = true;
                }
                widgets::section_header(ui, "Под настроение", false);
                if h.moods.is_empty() {
                    moods_empty(ui, state, pages);
                } else {
                    widgets::card_row(ui, "home-moods", |ui| {
                        for m in &h.moods {
                            mood_card(ui, m, out);
                        }
                    });
                }
            }
            Section::Continue => {
                if widgets::section_header(ui, "Продолжить", true) {
                    nav = Some(Route::Library(LibraryTab::Albums));
                }
                widgets::card_row(ui, "home-continue", |ui| {
                    for c in &h.continue_listening {
                        nav = match c {
                            ContinueCard::Release(r) => widgets::release_card(ui, r, widgets::CARD_W, "continue", out),
                            // Streamed, not in the library: its service's page.
                            ContinueCard::Stream(a) => {
                                super::streams::album_card(ui, a, widgets::CARD_W, "continue", &state.downloads.wishlist, out)
                            }
                        }
                        .or(nav.take());
                    }
                });
            }
            Section::StyleMixes => {
                widgets::section_header(ui, "Твои миксы", false);
                widgets::card_row(ui, "home-styles", |ui| {
                    for m in &h.style_mixes {
                        style_card(ui, m, out);
                    }
                });
            }
            Section::ArtistMixes => {
                widgets::section_header(ui, "Миксы исполнителей", false);
                widgets::card_row(ui, "home-artists", |ui| {
                    for a in &h.artist_mixes {
                        nav = widgets::artist_mix(ui, a, 120.0, out).or(nav.take());
                    }
                });
            }
            Section::Discover => {
                let all = (!h.discover.is_empty()).then_some(("Слушать всё", Icon::PlayArrow));
                if widgets::section_header_with(ui, "Новое для тебя", all) {
                    out.push(Command::PlayMix { id: MixId::Discover });
                }
                if h.discover.is_empty() {
                    discover_empty(ui, h, off, pages);
                } else {
                    let wish = widgets::WishIndex::of(ui.ctx(), &state.downloads.wishlist);
                    widgets::card_row(ui, "home-discover", |ui| {
                        for i in 0..h.discover.len() {
                            nav = discover_card(ui, &h.discover, i, &wish, pages, out).or(nav.take());
                        }
                    });
                }
            }
            Section::NewReleases => {
                widgets::section_header(ui, "Новые релизы", false);
                widgets::card_row(ui, "home-releases", |ui| {
                    for a in &h.new_releases {
                        nav = super::streams::album_card(ui, a, widgets::CARD_W, "new-releases", &state.downloads.wishlist, out)
                            .or(nav.take());
                    }
                });
            }
            Section::LongUnplayed => {
                widgets::section_header(ui, "Давно не слушал", false);
                widgets::card_row(ui, "home-long", |ui| {
                    for r in &h.long_unplayed {
                        nav = widgets::release_card(ui, r, widgets::CARD_W, "long", out).or(nav.take());
                    }
                });
            }
            Section::RecentlyAdded => {
                if widgets::section_header(ui, "Недавно добавлено", true) {
                    nav = Some(Route::Library(LibraryTab::Albums));
                }
                widgets::card_row(ui, "home-recent", |ui| {
                    for r in &h.recently_added {
                        nav = widgets::release_card(ui, r, widgets::CARD_W, "recent", out).or(nav.take());
                    }
                });
            }
        }
        if n + 1 < shown.len() {
            ui.add_space(widgets::SECTION_GAP);
        }
    }
    if shown.is_empty() {
        filter_empty(ui, filter, pages);
    }
    nav
}

/// A quiet line of the feed (`on_surface_variant`, body medium).
fn quiet(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface_variant));
}

/// A chip that has nothing to show: why, and back to everything.
fn filter_empty(ui: &mut Ui, filter: Filter, pages: &mut PageState) {
    let text = match filter {
        Filter::Mine => "Здесь будет твоя музыка: что слушал, миксы и недавно добавленное.",
        Filter::New => "Новое появится, когда я найду его в YouTube Music и Deezer.",
        Filter::Mood | Filter::All => "Пока пусто.",
    };
    if widgets::quiet_line(ui, text, Some(("Показать всё", None, Style::Text, "home-filter-all"))) {
        pages.home_filter = Filter::All;
    }
}

/// «Под настроение» without moods: the sound is not analysed yet.
fn moods_empty(ui: &mut Ui, state: &ViewState, pages: &mut PageState) {
    let a = &state.analysis;
    if !a.complete() {
        let pct = (a.fraction() * 100.0).floor() as u32;
        quiet(ui, &format!("Настроения появятся, когда я разберу звук библиотеки · {pct}%"));
    } else if widgets::quiet_line(
        ui,
        "Настроения подбираются по звуку, а у треков его пока нет: добавь свои файлы или включи анализ звука стримов.",
        Some(("Настроить", None, Style::Tonal, "home-moods-setup")),
    ) {
        pages.open_setting(super::SettingId::StreamAnalysis);
    }
}

/// «Новое для тебя» without discoveries: why, and what to do.
fn discover_empty(ui: &mut Ui, h: &HomeView, off: bool, pages: &mut PageState) {
    if off {
        if widgets::quiet_line(
            ui,
            "Новое ищется в YouTube Music и Deezer — они оба выключены.",
            Some(("Включить", None, Style::Tonal, "home-new-enable")),
        ) {
            pages.open_setting(super::SettingId::YouTubeEnabled);
        }
    } else if h.continue_listening.is_empty() {
        quiet(ui, "Послушай то, что нравится, — по нему я найду новое в YouTube Music и Deezer.");
    } else {
        quiet(ui, "Ищу новое по тому, что ты слушаешь, — появится здесь само.");
    }
}

/// Some service is signed in.
fn connected(state: &ViewState) -> bool {
    state.accounts.iter().any(|a| a.enabled && a.account.is_logged_in())
}

/// One tile of the board: what it plays and shows.
#[derive(Clone, Debug, PartialEq)]
struct BoardTile {
    id: MixId,
    title: String,
    subtitle: String,
    covers: Vec<String>,
    /// «Твой микс» before the first build: the tracks the home already has
    /// (played as they are instead of [`Command::PlayMix`]).
    tracks: Option<Vec<TrackId>>,
}

impl BoardTile {
    fn of(m: &MixCard) -> Self {
        Self {
            id: m.id,
            title: m.title.clone(),
            subtitle: m.subtitle.clone(),
            covers: m.covers.iter().map(|c| c.uri()).collect(),
            tracks: None,
        }
    }

    fn play(&self) -> Command {
        match &self.tracks {
            Some(tracks) => Command::PlayTracks { tracks: tracks.clone(), start: 0, context: "Твой микс".into() },
            None => Command::PlayMix { id: self.id },
        }
    }
}

/// «Твой микс»: the hero, or before the first build the home's own tracks.
fn hero_tile(h: &HomeView) -> Option<BoardTile> {
    if let Some(m) = &h.hero {
        return Some(BoardTile::of(m));
    }
    if h.mix.is_empty() {
        return None;
    }
    let mut covers: Vec<String> = Vec::new();
    for c in h.mix.iter().filter_map(|t| t.cover.as_ref()) {
        if covers.len() < 4 && !covers.contains(&c.uri()) {
            covers.push(c.uri());
        }
    }
    let n = h.mix.len() as u64;
    Some(BoardTile {
        id: MixId::Daily,
        title: "Твой микс".into(),
        subtitle: format!("{n} {} · любимое и дослушанное", crate::screens::common::plural(n, "трек", "трека", "треков")),
        covers,
        tracks: Some(h.mix.iter().map(|t| t.id).collect()),
    })
}

/// The board's tiles in their order, and whether the first is the big one:
/// the daylist, or «Твой микс» without it; then «Твой микс», then the
/// smaller mixes.
fn board_tiles(h: &HomeView) -> (bool, Vec<BoardTile>) {
    let mut tiles: Vec<BoardTile> = h.daylist.iter().map(BoardTile::of).collect();
    tiles.extend(hero_tile(h));
    let big = !tiles.is_empty();
    tiles.extend(h.tiles.iter().map(BoardTile::of));
    (big, tiles)
}

/// A board tile's container and content colours (scheme roles): the big
/// tile primary, the others spread over secondary, tertiary and the
/// highest surface so neighbours differ.
fn tile_colors(id: MixId, big: bool, s: &ColorScheme) -> (Color32, Color32) {
    if big {
        return (s.primary_container, s.on_primary_container);
    }
    match id {
        MixId::OnRepeat | MixId::DeepCuts => (s.tertiary_container, s.on_tertiary_container),
        MixId::TimeCapsule | MixId::FreshLikes => (s.surface_container_highest, s.on_surface),
        _ => (s.secondary_container, s.on_secondary_container),
    }
}

/// The shape a board tile's cover is cut into.
fn tile_shape(id: MixId) -> MaterialShape {
    match id {
        MixId::OnRepeat => MaterialShape::Cookie12Sided,
        MixId::TimeCapsule => MaterialShape::Arch,
        MixId::Rediscover => MaterialShape::Clover4Leaf,
        MixId::DeepCuts => MaterialShape::Gem,
        MixId::FreshLikes => MaterialShape::Heart,
        MixId::Daylist => MaterialShape::Sunny,
        _ => MaterialShape::Cookie9Sided,
    }
}

/// The bento board (§12.1): the big tile and the smaller ones on a grid of
/// four columns (two when narrow), entering one after another once.
fn board(ui: &mut Ui, h: &HomeView, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (big, tiles) = board_tiles(h);
    if tiles.is_empty() {
        return;
    }
    let width = ui.available_width();
    let cols = bento::columns(width);
    let cells = bento::layout(cols, big, tiles.len());
    let col_w = ((width - bento::GAP * (cols - 1) as f32) / cols as f32).floor();
    let row_h = bento::row_height(cols, col_w);
    let rows = bento::rows(&cells) as f32;
    let (board, _) = ui.allocate_exact_size(vec2(width, rows * row_h + (rows - 1.0) * bento::GAP), egui::Sense::hover());
    for (i, (t, cell)) in tiles.iter().zip(&cells).enumerate() {
        let rect = bento::cell_rect(*cell, board.min, col_w, row_h);
        let is_big = big && i == 0;
        let (fill, on) = tile_colors(t.id, is_big, &s);
        let refresh = t.tracks.is_none() && matches!(t.id, MixId::Daylist | MixId::Daily);
        let spec = bento::Tile {
            fill,
            on,
            kicker: bento::kicker(t.id.kicker(), &t.title),
            title: &t.title,
            title_two_lines: true,
            subtitle: &t.subtitle,
            covers: &t.covers,
            shape: tile_shape(t.id),
            stack: false,
            play: format!("Слушать «{}»", t.title),
        };
        bento::entering(ui, rect, i, ("home-board", i), |ui, rect| {
            let id = ui.id().with(("board-tile", format!("{:?}", t.id)));
            let (play, response, more) = if is_big {
                let o = bento::big_tile(ui, rect, &spec, id, refresh);
                (o.play, o.response, o.more)
            } else {
                let o = bento::tile(ui, rect, &spec, id);
                (o.play, o.response, None)
            };
            if play {
                out.push(t.play());
            }
            // «Обновить» picks the mix anew: from ⋮ and a right click.
            if refresh {
                let menu = |key: egui::Id| Menu::new(key).item(MenuItem::new("Обновить").icon(Icon::Refresh));
                let mut picked = menu(id.with("ctx")).context_menu(&response);
                if let Some(m) = &more {
                    picked = picked.or(menu(id.with("more-menu")).align_right(true).attach(m));
                }
                if picked == Some(0) {
                    out.push(Command::RefreshMix { id: t.id });
                }
            }
        });
    }
}

pub const MOOD_W: f32 = 200.0;
pub const MOOD_H: f32 = 160.0;

/// A mood: a tile in its scheme colour, the cover of what it plays first
/// cut into its shape, the name and a plain line on how it sounds; ▶ on
/// hover. A click plays it (an endless mix).
fn mood_card(ui: &mut Ui, m: &MixCard, out: &mut Vec<Command>) {
    let MixId::Mood(mood) = m.id else { return };
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(MOOD_W, MOOD_H), egui::Sense::hover());
    let (fill, on) = mood_colors(mood, &s);
    let covers: Vec<String> = m.covers.first().map(|c| c.uri()).into_iter().collect();
    let t = bento::Tile {
        fill,
        on,
        kicker: None,
        title: &m.title,
        title_two_lines: false,
        subtitle: mood_line(mood),
        covers: &covers,
        shape: mood_shape(mood),
        stack: false,
        play: format!("Слушать «{}»", m.title),
    };
    if bento::tile(ui, rect, &t, ui.id().with(("mood", mood))).play {
        out.push(Command::PlayMix { id: m.id });
    }
}

/// A style mix: a quiet tile with its albums overlapping in shapes and its
/// name («A, B и другие» — two lines when long). A click or ▶ plays it
/// (an endless mix).
fn style_card(ui: &mut Ui, m: &MixCard, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(MOOD_W, MOOD_H), egui::Sense::hover());
    let covers: Vec<String> = m.covers.iter().map(|c| c.uri()).collect();
    let t = bento::Tile {
        fill: s.surface_container_high,
        on: s.on_surface,
        kicker: None,
        title: &m.title,
        title_two_lines: true,
        subtitle: &m.subtitle,
        covers: &covers,
        shape: MaterialShape::Cookie9Sided,
        stack: true,
        play: format!("Слушать «{}»", m.title),
    };
    if bento::tile(ui, rect, &t, ui.id().with(("style-mix", m.title.as_str()))).play {
        out.push(Command::PlayMix { id: m.id });
    }
}

/// What a discovery's cover corner says about its download.
fn discover_status(t: &StreamTrackRow, wish: &widgets::WishIndex) -> CoverStatus {
    if t.has_local {
        return CoverStatus::Done;
    }
    match t.track.and_then(|id| wish.of_track(Some(id), None)) {
        Some(WishlistStatus::Downloading) => CoverStatus::Progress(None),
        Some(WishlistStatus::Done | WishlistStatus::Dismissed) | None => CoverStatus::None,
        Some(_) => CoverStatus::Queued,
    }
}

/// A discovery: its cover, title and artist; ▶ plays the discoveries from
/// it, ⬇ downloads it (no like), ⋮ (and a right click) has the queue, the
/// radio, the like and «Не рекомендовать».
fn discover_card(
    ui: &mut Ui,
    rows: &[StreamTrackRow],
    i: usize,
    wish: &widgets::WishIndex,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let t = &rows[i];
    let status = discover_status(t, wish);
    let can_download = status == CoverStatus::None && t.playable;
    let o = MediaCard::new(&t.title)
        .subtitle(&t.artist)
        .cover_opt(t.cover_url.clone())
        .placeholder_icon(Icon::MusicNote)
        .status(status)
        .play_button(t.playable)
        .download_button(can_download.then_some("Скачать — в «Хочу скачать»"))
        .menu_button(true)
        .width(widgets::CARD_W)
        .id_salt(("discover", &t.remote))
        .show(ui);
    if o.download_clicked {
        out.push(Command::DownloadStream { tracks: vec![t.remote.clone()] });
    }
    if (o.play_clicked || o.card_clicked) && t.playable {
        out.push(play_discover(rows, i));
    }
    let mut nav = None;
    if let Some(m) = &o.menu {
        nav = discover_pick(discover_menu(("discover-more", m.id), t).align_right(true).attach(m), t, Some(m.rect), pages, out);
    }
    discover_pick(discover_menu(("discover-menu", o.response.id), t).context_menu(&o.response), t, None, pages, out).or(nav)
}

fn discover_menu(key: impl egui::AsId, t: &StreamTrackRow) -> Menu {
    Menu::new(key)
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new(super::RADIO_BY_TRACK).icon(Icon::Radio))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .item(MenuItem::new(if t.liked { "Убрать из Любимых" } else { "В Любимые" }).icon(Icon::Favorite).selected(t.liked))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(!t.artists.linked().is_empty()))
        .divider()
        .item(MenuItem::new("Не рекомендовать трек").icon(Icon::Block))
        .item(MenuItem::new("Не рекомендовать артиста").icon(Icon::Block))
}

fn discover_pick(
    picked: Option<usize>,
    t: &StreamTrackRow,
    anchor: Option<Rect>,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    match picked? {
        0 => out.push(Command::PlayStreamNext(vec![t.remote.clone()])),
        1 => out.push(Command::EnqueueStream(vec![t.remote.clone()])),
        2 => out.push(super::streams::radio_by_stream(t)),
        3 => pages.add_stream_to_playlist(vec![t.remote.clone()], anchor),
        4 => out.push(Command::LikeStream { track: t.remote.clone(), on: !t.liked }),
        5 => return t.artist_id().cloned().map(|a| Route::Stream(StreamPage::Artist(a))),
        6 => out.push(dislike(t, false)),
        7 => out.push(dislike(t, true)),
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::{ArtistCard, ArtistId, ReleaseCard, ReleaseId, ReleaseType, RemoteId, TrackId, TrackRow};

    fn mix(id: MixId) -> MixCard {
        MixCard { id, title: "Микс".into(), subtitle: String::new(), covers: Vec::new() }
    }

    fn full_home() -> HomeView {
        let release = ReleaseCard {
            id: ReleaseId::new(),
            title: "Альбом".into(),
            artist: "Кино".into(),
            artists: Default::default(),
            year: None,
            release_type: ReleaseType::Album,
            cover: None,
            local_tracks: 1,
            total_tracks: None,
            liked: false,
        };
        HomeView {
            hero: Some(mix(MixId::Daily)),
            moods: vec![mix(MixId::Mood(Mood::Calm))],
            style_mixes: vec![mix(MixId::Style(0))],
            discover: vec![row("a")],
            new_releases: Vec::new(),
            continue_listening: vec![ContinueCard::Release(release.clone())],
            artist_mixes: vec![ArtistCard { id: ArtistId::new(), name: "Кино".into(), image: None, liked: false }],
            long_unplayed: vec![release.clone()],
            recently_added: vec![release],
            ..Default::default()
        }
    }

    fn row(id: &str) -> StreamTrackRow {
        StreamTrackRow {
            remote: RemoteId::new(Service::Youtube, id),
            title: format!("Песня {id}"),
            artist: "Аквариум".into(),
            artists: Default::default(),
            album: None,
            album_id: None,
            duration: None,
            disc_no: 1,
            track_no: 1,
            cover_url: None,
            kind: Default::default(),
            playable: true,
            track: None,
            liked: false,
            in_library: false,
            has_local: false,
            also_on: Vec::new(),
        }
    }

    #[test]
    fn chips_pick_their_sections() {
        let h = full_home();
        use Section::*;
        assert_eq!(
            sections(Filter::All, &h, false),
            vec![Board, Moods, Continue, StyleMixes, ArtistMixes, Discover, LongUnplayed, RecentlyAdded]
        );
        assert_eq!(
            sections(Filter::Mine, &h, false),
            vec![Board, Continue, StyleMixes, ArtistMixes, LongUnplayed, RecentlyAdded]
        );
        assert_eq!(sections(Filter::New, &h, false), vec![Discover, RecentlyAdded]);
        assert_eq!(sections(Filter::Mood, &h, false), vec![Moods, StyleMixes]);
    }

    #[test]
    fn empty_sections_hide_unless_their_chip_explains_them() {
        let h = HomeView { recently_added: full_home().recently_added, ..Default::default() };
        // «Всё»: only what has cards.
        assert_eq!(sections(Filter::All, &h, false), vec![Section::RecentlyAdded]);
        // Its own chip says why «Новое для тебя» and the moods are empty.
        assert_eq!(sections(Filter::New, &h, false), vec![Section::Discover, Section::RecentlyAdded]);
        assert_eq!(sections(Filter::Mood, &h, false), vec![Section::Moods]);
        // Discovery switched off: «Всё» says how to switch it on.
        assert!(sections(Filter::All, &h, true).contains(&Section::Discover));
        // «Твой микс» before the first build still plays the home's tracks.
        let track = TrackRow {
            id: TrackId::new(),
            title: "Группа крови".into(),
            artist: "Кино".into(),
            album: None,
            release_id: None,
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
        };
        let h = HomeView { mix: vec![track], ..Default::default() };
        assert_eq!(sections(Filter::All, &h, false), vec![Section::Board]);
    }

    #[test]
    fn the_board_is_the_daylist_then_the_mix_then_the_tiles() {
        let tiles: Vec<MixCard> = MixId::TILES.iter().map(|id| mix(*id)).collect();
        let ids = |h: &HomeView| {
            let (big, t) = board_tiles(h);
            (big, t.iter().map(|t| t.id).collect::<Vec<_>>())
        };
        let mut h = HomeView { daylist: Some(mix(MixId::Daylist)), tiles: tiles[..2].to_vec(), ..full_home() };
        assert_eq!(ids(&h), (true, vec![MixId::Daylist, MixId::Daily, MixId::OnRepeat, MixId::TimeCapsule]));
        // No daylist: «Твой микс» takes the big spot.
        h.daylist = None;
        assert_eq!(ids(&h), (true, vec![MixId::Daily, MixId::OnRepeat, MixId::TimeCapsule]));
        // Only tiles: rows, nothing big.
        h.hero = None;
        assert_eq!(ids(&h), (false, vec![MixId::OnRepeat, MixId::TimeCapsule]));
        // The board shows with any of them, under «Всё» and «Моё».
        assert_eq!(sections(Filter::Mine, &h, false)[0], Section::Board);
        assert!(!sections(Filter::New, &h, false).contains(&Section::Board));
        h.tiles.clear();
        assert!(!sections(Filter::All, &h, false).contains(&Section::Board));
    }

    #[test]
    fn board_tiles_each_have_their_shape_and_the_big_one_is_primary() {
        let shapes: std::collections::HashSet<_> =
            MixId::TILES.iter().chain(&[MixId::Daily, MixId::Daylist]).map(|id| tile_shape(*id)).collect();
        assert_eq!(shapes.len(), MixId::TILES.len() + 2);
        let s = ColorScheme::baseline(true);
        assert_eq!(tile_colors(MixId::Daily, true, &s).0, s.primary_container);
        // Neighbours on the full board differ: «Твой микс», then the tiles.
        let order: Vec<_> = std::iter::once(MixId::Daily).chain(MixId::TILES).map(|id| tile_colors(id, false, &s).0).collect();
        assert!(order.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn every_mood_has_its_own_line_and_shape() {
        let lines: std::collections::HashSet<_> = Mood::ALL.iter().map(|m| mood_line(*m)).collect();
        assert_eq!(lines.len(), Mood::ALL.len());
        assert!(lines.iter().all(|l| !l.is_empty() && l.chars().count() <= 22));
        let shapes: std::collections::HashSet<_> = Mood::ALL.iter().map(|m| mood_shape(*m)).collect();
        assert_eq!(shapes.len(), Mood::ALL.len());
    }

    #[test]
    fn analysis_line_shows_while_it_runs() {
        let mut a = AnalysisProgress { files_done: 40, files_total: 100, streams_done: 0, streams_total: 0 };
        assert_eq!(analysis_line(&a).as_deref(), Some("Анализирую звук библиотеки — миксы станут точнее · 40%"));
        a.files_done = 100;
        assert_eq!(analysis_line(&a), None);
        assert_eq!(analysis_line(&AnalysisProgress::default()), None);
    }

    #[test]
    fn discovery_cards_play_from_themselves_and_dislike_by_song_or_name() {
        let rows = [row("a"), row("b"), row("c")];
        match play_discover(&rows, 1) {
            Command::PlayStreamTracks { tracks, start, context } => {
                assert_eq!(tracks.len(), 3);
                assert_eq!(start, 1);
                assert_eq!(context, "Новое для тебя");
            }
            c => panic!("{c:?}"),
        }
        assert_eq!(
            dislike(&rows[0], false),
            Command::RecFeedback { target: RecTarget::StreamTrack(rows[0].remote.clone()), dislike: true }
        );
        assert_eq!(
            dislike(&rows[0], true),
            Command::RecFeedback { target: RecTarget::StreamArtist("Аквариум".into()), dislike: true }
        );
        // «Радио по треку» of a discovery: the service's track, or its
        // library track once it has one.
        assert_eq!(
            super::super::streams::radio_by_stream(&rows[0]),
            Command::StartRadio { seed: nikgob_view::RadioSeed::Stream(rows[0].remote.clone()) }
        );
        let mut known = row("d");
        let id = TrackId::new();
        known.track = Some(id);
        assert_eq!(
            super::super::streams::radio_by_stream(&known),
            Command::StartRadio { seed: nikgob_view::RadioSeed::Track(id) }
        );
    }
}
