//! Главная (md3e-player.md §3.1, docs/stage4.md §4–§5, §12.1): greeting,
//! filter chips, the bento board (the daylist, «Твой микс» and the smaller
//! mixes), then feed rows: moods, the albums to continue, style and artist
//! mixes, new music from the services and the library's own rows.

use egui::{Color32, Rect, Ui, pos2, vec2};
use nikgob_shapes::MaterialShape;
use nikgob_view::{
    AnalysisProgress, Command, ContinueCard, HomeView, LibraryTab, MixCard, MixId, Mood, RecTarget, Route, Service, TileKind,
    StreamPage, StreamTrackRow, TrackId, ViewState, WishlistStatus,
};

use super::{PageState, bento, widgets};
use crate::{
    ColorScheme, Icon, Theme, TypeRole,
    components::{CoverStatus, FilterChip, Menu, MenuItem, Style, TooltipExt},
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

    chips(ui, state, pages, out);
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
                board(ui, state, out);
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
                    widgets::card_row_each(ui, "home-moods", &h.moods, |ui, _, m| mood_card(ui, m, out));
                }
            }
            Section::Continue => {
                if widgets::section_header(ui, "Продолжить", true) {
                    nav = Some(Route::Library(LibraryTab::Albums));
                }
                widgets::card_row_each(ui, "home-continue", &h.continue_listening, |ui, _, c| {
                    nav = match c {
                        ContinueCard::Release(r) => widgets::release_card(ui, r, widgets::CARD_W, "continue", out),
                        // Streamed, not in the library: its service's page.
                        ContinueCard::Stream(a) => {
                            super::streams::album_card(ui, a, widgets::CARD_W, "continue", &state.downloads.wishlist, out)
                        }
                    }
                    .or(nav.take());
                });
            }
            Section::StyleMixes => {
                widgets::section_header(ui, "Твои миксы", false);
                widgets::card_row_each(ui, "home-styles", &h.style_mixes, |ui, _, m| style_card(ui, m, out));
            }
            Section::ArtistMixes => {
                widgets::section_header(ui, "Миксы исполнителей", false);
                widgets::card_row_each(ui, "home-artists", &h.artist_mixes, |ui, _, a| {
                    nav = widgets::artist_mix(ui, a, ARTIST_W, out).or(nav.take());
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
                    nav = discover_grid(ui, &h.discover, &wish, pages, out).or(nav.take());
                }
            }
            Section::NewReleases => {
                widgets::section_header(ui, "Новые релизы", false);
                widgets::card_row_each(ui, "home-releases", &h.new_releases, |ui, _, a| {
                    nav = super::streams::album_card(ui, a, widgets::CARD_W, "new-releases", &state.downloads.wishlist, out)
                        .or(nav.take());
                });
            }
            Section::LongUnplayed => {
                widgets::section_header(ui, "Давно не слушал", false);
                widgets::card_row_each(ui, "home-long", &h.long_unplayed, |ui, _, r| {
                    nav = widgets::release_card(ui, r, widgets::CARD_W, "long", out).or(nav.take());
                });
            }
            Section::RecentlyAdded => {
                if widgets::section_header(ui, "Недавно добавлено", true) {
                    nav = Some(Route::Library(LibraryTab::Albums));
                }
                widgets::card_row_each(ui, "home-recent", &h.recently_added, |ui, _, r| {
                    nav = widgets::release_card(ui, r, widgets::CARD_W, "recent", out).or(nav.take());
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

/// The side of an artist's photo in «Миксы исполнителей».
const ARTIST_W: f32 = 140.0;

/// The filter chips, and at the right end «Только скачанное»: mixes and
/// radios made only of downloaded tracks (a preference, so it holds
/// across runs).
fn chips(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for f in Filter::ALL {
            if ui.add(FilterChip::new(f.label(), pages.home_filter == f).id_salt(("home-chip", f.label()))).clicked() {
                pages.home_filter = f;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let on = state.prefs.mixes_downloaded_only;
            let chip = FilterChip::new("Только скачанное", on).icon(Icon::DownloadDone).id_salt("home-downloaded-only");
            if ui.add(chip).md3_tooltip("Миксы и радио — только из скачанных треков", None).clicked() {
                out.push(downloaded_only(state, !on));
            }
        });
    });
}

/// «Только скачанное» switched to `on`: the prefs with the flag.
fn downloaded_only(state: &ViewState, on: bool) -> Command {
    Command::SetPrefs(Box::new(nikgob_view::Prefs { mixes_downloaded_only: on, ..state.prefs.clone() }))
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

/// How a board tile looks: its art (docs/stage4.md §12.12), each kind its
/// own.
#[derive(Clone, Debug, PartialEq)]
enum Look {
    /// The first cover cut into the kind's shape.
    Cover(MaterialShape),
    /// The covers fanned like cards.
    Fan,
    /// The first cover in a heart.
    Heart,
    /// A photo (the artist's, else the first cover) filling the tile.
    Bleed(String),
    /// A big typographic mark.
    Glyph(String),
    /// An icon in a filled shape, turned by the angle.
    Icon(Icon, MaterialShape, f32),
    /// The lottery's coin.
    Coin,
}

/// «あ», «한», «Я», «A»: the script a language tile («Японское»,
/// «Корейское», «По-русски», «Зарубежное») is about.
fn script_mark(title: &str) -> &'static str {
    let t = title.to_lowercase();
    if t.contains("япон") {
        "あ"
    } else if t.contains("корей") {
        "한"
    } else if t.contains("рус") {
        "Я"
    } else {
        "A"
    }
}

/// «90-е» for «Из 90-х», «2000-е» for «Из 2000-х»: the first number of a
/// decade tile's title.
fn decade_mark(title: &str) -> Option<String> {
    let digits: String = title.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| format!("{digits}-е"))
}

/// The look of a tile of `id` titled `title`, with `covers` and the photo
/// of its artist when it is about one.
fn look(id: MixId, title: &str, covers: &[String], photo: Option<String>) -> Look {
    use TileKind::*;
    let first = covers.first().cloned();
    let shape = tile_shape(id);
    let bleed = |uri: Option<String>| uri.map_or(Look::Cover(shape), Look::Bleed);
    let MixId::Tile(kind) = id else { return Look::Cover(shape) };
    match kind {
        ArtistOfDay | Comeback => bleed(photo.or(first)),
        YearAgo | Soundtrack | Playlist => bleed(first),
        OnRepeat | DeepCuts => Look::Fan,
        FreshLikes | FirstLikes => Look::Heart,
        Long => Look::Glyph("7:00+".into()),
        Short => Look::Glyph("2:30".into()),
        Language => Look::Glyph(script_mark(title).into()),
        Decade => decade_mark(title).map_or(Look::Cover(shape), Look::Glyph),
        SingAlong => Look::Icon(Icon::Lyrics, MaterialShape::Burst, 0.0),
        // The arrow turned down: a download.
        NotDownloaded => Look::Icon(Icon::Download, MaterialShape::Arrow, std::f32::consts::PI),
        Lottery => Look::Coin,
        Season | Rediscover | Unplayed | BecauseYouLiked | Unfinished => Look::Cover(shape),
    }
}

/// One tile of the board: what it plays and shows.
#[derive(Clone, Debug, PartialEq)]
struct BoardTile {
    id: MixId,
    kicker: String,
    title: String,
    subtitle: String,
    covers: Vec<String>,
    look: Look,
    /// «Твой микс» before the first build: the tracks the home already has
    /// (played as they are instead of [`Command::PlayMix`]).
    tracks: Option<Vec<TrackId>>,
}

impl BoardTile {
    fn of(m: &MixCard, photo: &dyn Fn(&str) -> Option<String>) -> Self {
        let covers: Vec<String> = m.covers.iter().map(|c| c.uri()).collect();
        let about_artist = matches!(m.id, MixId::Tile(TileKind::ArtistOfDay | TileKind::Comeback));
        let photo = about_artist.then(|| photo(&m.title)).flatten();
        Self {
            id: m.id,
            kicker: m.kicker.clone(),
            title: m.title.clone(),
            subtitle: m.subtitle.clone(),
            look: look(m.id, &m.title, &covers, photo),
            covers,
            tracks: None,
        }
    }

    fn play(&self) -> Command {
        match &self.tracks {
            Some(tracks) => Command::PlayTracks { tracks: tracks.clone(), start: 0, context: "Твой микс".into() },
            None => Command::PlayMix { id: self.id },
        }
    }

    /// The art of the tile, borrowing its covers.
    fn art(&self) -> bento::Art<'_> {
        let first = self.covers.first().map(String::as_str);
        match &self.look {
            Look::Cover(shape) => bento::Art::Cover { uri: first, shape: *shape },
            Look::Fan => bento::Art::Fan(&self.covers),
            Look::Heart => bento::Art::Heart(first),
            Look::Bleed(uri) => bento::Art::Bleed(uri),
            Look::Glyph(mark) => bento::Art::Glyph(mark),
            Look::Icon(icon, shape, turn) => bento::Art::Icon { icon: *icon, shape: *shape, turn: *turn },
            Look::Coin => bento::Art::Coin,
        }
    }
}

/// The photo of the artist named `name`, from the artist mixes or the
/// library's artists.
fn artist_photo(state: &ViewState, name: &str) -> Option<String> {
    let name = name.trim().to_lowercase();
    state
        .home
        .artist_mixes
        .iter()
        .chain(&state.library.artists)
        .find(|a| a.image.is_some() && a.name.trim().to_lowercase() == name)
        .and_then(|a| a.image.as_ref().map(|c| c.uri()))
}

/// «Твой микс»: the hero, or before the first build the home's own tracks.
fn hero_tile(h: &HomeView, photo: &dyn Fn(&str) -> Option<String>) -> Option<BoardTile> {
    if let Some(m) = &h.hero {
        return Some(BoardTile::of(m, photo));
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
        kicker: "ТВОЙ МИКС".into(),
        title: "Твой микс".into(),
        subtitle: format!("{n} {} · любимое и дослушанное", crate::screens::common::plural(n, "трек", "трека", "треков")),
        covers,
        look: Look::Cover(tile_shape(MixId::Daily)),
        tracks: Some(h.mix.iter().map(|t| t.id).collect()),
    })
}

/// The board's tiles in their order, and whether the first is the big one:
/// the daylist, or «Твой микс» without it; then «Твой микс», then the
/// smaller mixes.
fn board_tiles(h: &HomeView, photo: &dyn Fn(&str) -> Option<String>) -> (bool, Vec<BoardTile>) {
    let mut tiles: Vec<BoardTile> = h.daylist.iter().map(|m| BoardTile::of(m, photo)).collect();
    tiles.extend(hero_tile(h, photo));
    let big = !tiles.is_empty();
    tiles.extend(h.tiles.iter().map(|m| BoardTile::of(m, photo)));
    (big, tiles)
}

/// The colour roles of the small tiles: secondary, tertiary, the highest
/// surface (the big tile is primary; a fourth, the high surface, only
/// when three cannot keep touching tiles apart).
const ROLES: usize = 3;

/// The role a small tile would like, by its group of kinds.
fn preferred_role(id: MixId) -> usize {
    use TileKind::*;
    match id {
        MixId::Tile(OnRepeat | YearAgo | Season | Rediscover | FirstLikes | FreshLikes | Unplayed) => 1,
        MixId::Tile(DeepCuts | ArtistOfDay | Comeback) => 0,
        MixId::Tile(_) => 2,
        _ => 0,
    }
}

/// Two cells share an edge.
fn touching(a: bento::Cell, b: bento::Cell) -> bool {
    let overlap = |a0: usize, a1: usize, b0: usize, b1: usize| a0 < b1 && b0 < a1;
    let side = (a.col + a.w == b.col || b.col + b.w == a.col) && overlap(a.row, a.row + a.h, b.row, b.row + b.h);
    let stacked = (a.row + a.h == b.row || b.row + b.h == a.row) && overlap(a.col, a.col + a.w, b.col, b.col + b.w);
    side || stacked
}

/// The colour roles of the board's tiles in their cells (`None` for the
/// big one): no two touching tiles alike, each as near its kind's role as
/// that allows (the first such colouring in the order of the
/// preferences); a board three roles cannot colour takes a fourth.
fn board_roles(ids: &[MixId], cells: &[bento::Cell], big: bool) -> Vec<Option<usize>> {
    fn fill(i: usize, ids: &[MixId], cells: &[bento::Cell], big: bool, roles: usize, out: &mut Vec<Option<usize>>) -> bool {
        if i == ids.len() {
            return true;
        }
        if big && i == 0 {
            out.push(None);
            return fill(1, ids, cells, big, roles, out) || {
                out.pop();
                false
            };
        }
        let first = preferred_role(ids[i]);
        for k in 0..roles {
            let r = (first + k) % roles;
            if (0..i).any(|j| out[j] == Some(r) && touching(cells[j], cells[i])) {
                continue;
            }
            out.push(Some(r));
            if fill(i + 1, ids, cells, big, roles, out) {
                return true;
            }
            out.pop();
        }
        false
    }
    let n = ids.len().min(cells.len());
    for roles in [ROLES, ROLES + 1] {
        let mut out = Vec::with_capacity(n);
        if fill(0, &ids[..n], cells, big, roles, &mut out) {
            return out;
        }
    }
    (0..n).map(|i| (!(big && i == 0)).then_some(i % ROLES)).collect()
}

/// A board tile's container and content colours (scheme roles).
fn tile_colors(role: Option<usize>, s: &ColorScheme) -> (Color32, Color32) {
    match role {
        None => (s.primary_container, s.on_primary_container),
        Some(0) => (s.secondary_container, s.on_secondary_container),
        Some(1) => (s.tertiary_container, s.on_tertiary_container),
        Some(3) => (s.surface_container_high, s.on_surface),
        Some(_) => (s.surface_container_highest, s.on_surface),
    }
}

/// The shape a board tile's cover is cut into: each kind its own.
fn tile_shape(id: MixId) -> MaterialShape {
    use TileKind::*;
    match id {
        MixId::Daylist => MaterialShape::Sunny,
        MixId::Tile(kind) => match kind {
            OnRepeat => MaterialShape::Cookie12Sided,
            YearAgo => MaterialShape::Arch,
            Season => MaterialShape::Flower,
            Rediscover => MaterialShape::Clover4Leaf,
            FirstLikes => MaterialShape::PuffyDiamond,
            FreshLikes => MaterialShape::Heart,
            Unplayed => MaterialShape::Ghostish,
            DeepCuts => MaterialShape::Gem,
            ArtistOfDay => MaterialShape::Circle,
            Comeback => MaterialShape::Boom,
            BecauseYouLiked => MaterialShape::Clover8Leaf,
            Long => MaterialShape::Pill,
            Short => MaterialShape::Bun,
            SingAlong => MaterialShape::Burst,
            Language => MaterialShape::Pentagon,
            Decade => MaterialShape::Cookie6Sided,
            Soundtrack => MaterialShape::Fan,
            Playlist => MaterialShape::Cookie4Sided,
            Unfinished => MaterialShape::SemiCircle,
            NotDownloaded => MaterialShape::Arrow,
            Lottery => MaterialShape::PixelCircle,
        },
        _ => MaterialShape::Cookie9Sided,
    }
}

/// The bento board (§12.1, §12.12): the big tile and the smaller ones on a
/// grid of four columns (two when narrow), growing in one after another
/// once.
fn board(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (big, tiles) = board_tiles(&state.home, &|name| artist_photo(state, name));
    if tiles.is_empty() {
        return;
    }
    let width = ui.available_width();
    let cols = bento::columns(width);
    let cells = bento::layout(cols, big, tiles.len());
    let col_w = ((width - bento::GAP * (cols - 1) as f32) / cols as f32).floor();
    let row_h = bento::row_height(cols, col_w);
    let rows = bento::rows(&cells) as f32;
    let roles = board_roles(&tiles.iter().map(|t| t.id).collect::<Vec<_>>(), &cells, big);
    let (board, _) = ui.allocate_exact_size(vec2(width, rows * row_h + (rows - 1.0) * bento::GAP), egui::Sense::hover());
    for (i, ((t, cell), role)) in tiles.iter().zip(&cells).zip(&roles).enumerate() {
        let rect = bento::cell_rect(*cell, board.min, col_w, row_h);
        let is_big = big && i == 0;
        let (fill, on) = tile_colors(*role, &s);
        let refresh = t.tracks.is_none() && matches!(t.id, MixId::Daylist | MixId::Daily);
        let kicker = bento::kicker(Some(t.kicker.as_str()).filter(|k| !k.is_empty()), &t.title);
        let play = if t.look == Look::Coin {
            "Подбросить монетку: случайные треки из библиотеки".to_owned()
        } else {
            format!("Слушать «{}»", t.title)
        };
        bento::entering(ui, rect, i, ("home-board", i), |ui, rect| {
            let id = ui.id().with(("board-tile", format!("{:?}", t.id)));
            let (play, response, more) = if is_big {
                let spec = bento::BigTile {
                    fill,
                    on,
                    kicker,
                    title: &t.title,
                    subtitle: &t.subtitle,
                    covers: &t.covers,
                    shape: tile_shape(t.id),
                    play,
                };
                let o = bento::big_tile(ui, rect, &spec, id, refresh);
                (o.play, o.response, o.more)
            } else {
                let spec = bento::Tile { fill, on, kicker, title: &t.title, subtitle: &t.subtitle, art: t.art(), play };
                let o = bento::tile(ui, rect, &spec, id);
                (o.play, o.response, None)
            };
            if play {
                out.push(t.play());
            }
            // «Ещё не скачано»: «Скачать всё» on a right click.
            if t.id == MixId::Tile(TileKind::NotDownloaded) {
                let menu = Menu::new(id.with("download")).item(MenuItem::new("Скачать всё").icon(Icon::Download));
                if menu.context_menu(&response) == Some(0) {
                    out.push(Command::DownloadMix { id: t.id });
                }
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
    let cover = m.covers.first().map(|c| c.uri());
    let t = bento::Tile {
        fill,
        on,
        kicker: None,
        title: &m.title,
        subtitle: mood_line(mood),
        art: bento::Art::Cover { uri: cover.as_deref(), shape: mood_shape(mood) },
        play: format!("Слушать «{}»", m.title),
    };
    if bento::tile(ui, rect, &t, ui.id().with(("mood", mood))).play {
        out.push(Command::PlayMix { id: m.id });
    }
}

/// The side of a style mix's square.
pub const MIX_W: f32 = 184.0;

/// The covers of a style mix in a rounded square: four as a 2×2 mosaic
/// (2 dp apart), fewer as the first alone, none as a tonal square with a
/// note; magnified `zoom` times inside the square.
fn mosaic(ui: &Ui, rect: Rect, covers: &[String], zoom: f32) {
    let r = crate::tokens::shape::LARGE as u8;
    if covers.len() < 4 {
        let corner = egui::CornerRadius::same(r);
        crate::components::paint_image_zoomed(ui, rect, covers.first().map(String::as_str), corner, Icon::QueueMusic, zoom);
        return;
    }
    let gap = 2.0;
    let half = (rect.size() - vec2(gap, gap)) * 0.5;
    for (i, uri) in covers.iter().take(4).enumerate() {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        let q = Rect::from_min_size(rect.min + vec2(col * (half.x + gap), row * (half.y + gap)), half);
        let corner = egui::CornerRadius {
            nw: if i == 0 { r } else { 0 },
            ne: if i == 1 { r } else { 0 },
            sw: if i == 2 { r } else { 0 },
            se: if i == 3 { r } else { 0 },
        };
        crate::components::paint_image_zoomed(ui, q, Some(uri), corner, Icon::QueueMusic, zoom);
    }
}

/// A style mix, as a music service draws its mixes: a big square of its
/// albums (a 2×2 mosaic), its name under it in two lines that are always
/// kept for it (so every card of the row lines up) and a quiet line; ▶
/// grows in at the square's corner under the pointer, the albums come a
/// little closer, a press squishes the square. A click plays it (an
/// endless mix).
fn style_card(ui: &mut Ui, m: &MixCard, out: &mut Vec<Command>) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let title_role = TypeRole::TitleMedium.style();
    let sub_role = TypeRole::BodyMedium.style();
    let (rect, resp) =
        ui.allocate_exact_size(vec2(MIX_W, MIX_W + 8.0 + 2.0 * title_role.line_height + sub_role.line_height), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let id = resp.id;
    let hover = resp.contains_pointer() || resp.has_focus();
    let art = Rect::from_min_size(rect.min, vec2(MIX_W, MIX_W));
    let squish = crate::components::anim::f32(
        ui.ctx(),
        id.with("squish"),
        if resp.is_pointer_button_down_on() { 0.96 } else { 1.0 },
        theme.motion.fast_spatial,
    );
    let zoom = 1.0
        + 0.06
            * crate::components::anim::fraction(ui.ctx(), id.with("zoom"), f32::from(u8::from(hover)), theme.motion.default_effects);
    let layer = crate::components::anim::fraction(
        ui.ctx(),
        id.with("layer"),
        if resp.is_pointer_button_down_on() {
            crate::tokens::state::PRESSED
        } else if hover {
            crate::tokens::state::HOVER
        } else {
            0.0
        },
        theme.motion.fast_effects,
    );
    let covers: Vec<String> = m.covers.iter().map(|c| c.uri()).collect();
    let radii = crate::components::Radii::all(crate::tokens::shape::LARGE);
    let play = bento::scaled(ui, art.center(), squish, |ui| {
        mosaic(ui, art, &covers, zoom);
        crate::components::paint::state_layer(ui.painter(), art, radii, s.on_surface, layer);
        let d = crate::components::Size::M.height();
        let slot = Rect::from_min_size(art.max - vec2(8.0 + d, 8.0 + d), vec2(d, d));
        let tip = format!("Слушать «{}»", m.title);
        bento::hover_play(ui, id, hover, slot, crate::components::Size::M, s.on_primary, s.primary, &tip)
    });
    // The name in two lines whatever its length; the quiet line under them.
    let name = widgets::wrapped(ui, &m.title, title_role, s.on_surface, MIX_W, 2);
    ui.painter().galley(pos2(art.left(), art.bottom() + 8.0), name, s.on_surface);
    if !m.subtitle.is_empty() {
        let line = widgets::wrapped(ui, &m.subtitle, sub_role, s.on_surface_variant, MIX_W, 1);
        let y = art.bottom() + 8.0 + 2.0 * title_role.line_height;
        ui.painter().galley(pos2(art.left(), y), line, s.on_surface_variant);
    }
    if play || resp.clicked() {
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

/// The rows of a column of «Новое для тебя».
pub const PICK_ROWS: usize = 4;
/// The width of a column of «Новое для тебя».
pub const PICK_W: f32 = 360.0;

/// «Новое для тебя» as quick picks: columns of four track rows (cover,
/// title, artist) side by side, scrolling sideways a column at a time.
fn discover_grid(
    ui: &mut Ui,
    rows: &[StreamTrackRow],
    wish: &widgets::WishIndex,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let mut nav = None;
    let row_h = crate::components::list::TWO_LINE;
    let height = PICK_ROWS as f32 * row_h;
    widgets::card_row(ui, "home-discover", |ui| {
        for (c, column) in rows.chunks(PICK_ROWS).enumerate() {
            ui.push_id(("discover-column", c), |ui| {
                ui.allocate_ui_with_layout(vec2(PICK_W, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(PICK_W);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for k in 0..column.len() {
                        nav = discover_row(ui, rows, c * PICK_ROWS + k, wish, pages, out).or(nav.take());
                    }
                });
            });
        }
    });
    nav
}

/// A discovery: its cover, title and artist. A click (or ▶ over the
/// cover) plays the discoveries from it; under the pointer ⬇ downloads it
/// (no like) and ⋮ (and a right click) has the queue, the radio, the like
/// and «Не рекомендовать». Where it is in «Хочу скачать» stays in place of
/// ⬇. The trailing buttons keep their room when hidden, so the title
/// never rewraps under the pointer.
fn discover_row(
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
    let row = Rect::from_min_size(ui.cursor().min, vec2(PICK_W, crate::components::list::TWO_LINE));
    let hovered = ui.rect_contains_pointer(row);
    let mut more: Option<egui::Response> = None;
    let mut download = false;
    let o = crate::components::ListItem::new(&t.title)
        .leading(crate::components::Leading::Image(t.cover_url.clone(), Icon::MusicNote))
        .supporting(&t.artist)
        .hover_play(t.playable)
        .enabled(true)
        .trailing(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // ⋮ stays while its menu is open (the pointer is on the menu).
                let key = egui::Id::new(("discover-more-id", i));
                let open = ui.ctx().data(|d| {
                    d.get_temp::<egui::Id>(key).and_then(|b| d.get_temp::<bool>(b.with("md3e-menu-open"))).unwrap_or(false)
                });
                let m = ui.add_visible(
                    hovered || open,
                    crate::components::IconButton::new(Icon::MoreVert)
                        .size(crate::components::Size::Xs)
                        .tooltip("Ещё")
                        .id_salt(("discover-more-btn", i)),
                );
                ui.ctx().data_mut(|d| d.insert_temp(key, m.id));
                more = Some(m);
                match status {
                    CoverStatus::None => {
                        download = ui
                            .add_visible(
                                hovered && can_download,
                                crate::components::IconButton::new(Icon::Download)
                                    .size(crate::components::Size::Xs)
                                    .tooltip("Скачать — в «Хочу скачать»")
                                    .id_salt(("discover-dl", i)),
                            )
                            .clicked();
                    }
                    CoverStatus::Done => {
                        ui.add(
                            crate::components::IconButton::new(Icon::DownloadDone)
                                .size(crate::components::Size::Xs)
                                .tooltip("Скачано — у тебя")
                                .id_salt(("discover-done", i)),
                        );
                    }
                    CoverStatus::Progress(_) | CoverStatus::Queued => {
                        let (icon, tip) = if status == CoverStatus::Queued {
                            (Icon::HourglassEmpty, "Ждёт в «Хочу скачать»")
                        } else {
                            (Icon::CloudDownload, "Качается")
                        };
                        if ui
                            .add(
                                crate::components::IconButton::new(icon)
                                    .size(crate::components::Size::Xs)
                                    .tooltip(tip)
                                    .id_salt(("discover-wish", i)),
                            )
                            .clicked()
                        {
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(widgets::OPEN_WISHLIST), true));
                        }
                    }
                }
            });
        })
        .id_salt(("discover", i, &t.remote))
        .show(ui);
    if download {
        out.push(Command::DownloadStream { tracks: vec![t.remote.clone()] });
    }
    if (o.play_clicked || o.response.clicked()) && t.playable {
        out.push(play_discover(rows, i));
    }
    let mut nav = None;
    if let Some(m) = &more {
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
        MixCard { id, title: "Микс".into(), kicker: String::new(), subtitle: String::new(), covers: Vec::new() }
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
            added_at: 0,
        };
        HomeView {
            hero: Some(mix(MixId::Daily)),
            moods: vec![mix(MixId::Mood(Mood::Calm))],
            style_mixes: vec![mix(MixId::Style(0))],
            discover: vec![row("a")],
            new_releases: Vec::new(),
            continue_listening: vec![ContinueCard::Release(release.clone())],
            artist_mixes: vec![ArtistCard { id: ArtistId::new(), name: "Кино".into(), image: None, liked: false, track_count: 0 }],
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
        let (a, b) = (MixId::Tile(TileKind::OnRepeat), MixId::Tile(TileKind::ArtistOfDay));
        let tiles = vec![mix(a), mix(b)];
        let ids = |h: &HomeView| {
            let (big, t) = board_tiles(h, &|_| None);
            (big, t.iter().map(|t| t.id).collect::<Vec<_>>())
        };
        let mut h = HomeView { daylist: Some(mix(MixId::Daylist)), tiles, ..full_home() };
        assert_eq!(ids(&h), (true, vec![MixId::Daylist, MixId::Daily, a, b]));
        // No daylist: «Твой микс» takes the big spot.
        h.daylist = None;
        assert_eq!(ids(&h), (true, vec![MixId::Daily, a, b]));
        // Only tiles: rows, nothing big.
        h.hero = None;
        assert_eq!(ids(&h), (false, vec![a, b]));
        // The board shows with any of them, under «Всё» and «Моё».
        assert_eq!(sections(Filter::Mine, &h, false)[0], Section::Board);
        assert!(!sections(Filter::New, &h, false).contains(&Section::Board));
        h.tiles.clear();
        assert!(!sections(Filter::All, &h, false).contains(&Section::Board));
    }

    #[test]
    fn board_tiles_each_have_their_shape_and_neighbours_differ() {
        let ids: Vec<MixId> = TileKind::ALL.iter().map(|k| MixId::Tile(*k)).chain([MixId::Daily, MixId::Daylist]).collect();
        let shapes: std::collections::HashSet<_> = ids.iter().map(|id| tile_shape(*id)).collect();
        assert_eq!(shapes.len(), ids.len());
        let s = ColorScheme::baseline(true);
        assert_eq!(tile_colors(None, &s).0, s.primary_container);
        // Any day's board on four columns and on two: no two tiles that
        // touch share a colour, whatever their kinds.
        for start in 0..TileKind::ALL.len() {
            for n in 1..=8 {
                let mut board = vec![MixId::Daylist, MixId::Daily];
                board.extend((0..n).map(|k| MixId::Tile(TileKind::ALL[(start + k * 2) % TileKind::ALL.len()])));
                for cols in [4, 2] {
                    let cells = bento::layout(cols, true, board.len());
                    let roles = board_roles(&board, &cells, true);
                    for i in 1..board.len() {
                        for j in 1..i {
                            if touching(cells[i], cells[j]) {
                                assert_ne!(roles[i], roles[j], "{board:?} on {cols} columns: {i} and {j}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_kind_of_tile_has_a_look_of_its_own() {
        let covers = vec!["file:///a.jpg".to_owned(), "file:///b.jpg".to_owned()];
        let look_of = |kind: TileKind, title: &str| look(MixId::Tile(kind), title, &covers, None);
        // Artists fill the tile with their photo, else the first cover.
        assert_eq!(look(MixId::Tile(TileKind::ArtistOfDay), "Eve", &covers, Some("file:///eve.jpg".into())), Look::Bleed("file:///eve.jpg".into()));
        assert_eq!(look_of(TileKind::Comeback, "Eve"), Look::Bleed(covers[0].clone()));
        assert_eq!(look_of(TileKind::OnRepeat, "На повторе"), Look::Fan);
        assert_eq!(look_of(TileKind::FreshLikes, "Свежие лайки"), Look::Heart);
        assert_eq!(look_of(TileKind::Long, "От 7 минут"), Look::Glyph("7:00+".into()));
        assert_eq!(look_of(TileKind::Language, "Японское"), Look::Glyph("あ".into()));
        assert_eq!(look_of(TileKind::Language, "По-русски"), Look::Glyph("Я".into()));
        assert_eq!(look_of(TileKind::Language, "Зарубежное"), Look::Glyph("A".into()));
        assert_eq!(look_of(TileKind::Decade, "Из 90-х"), Look::Glyph("90-е".into()));
        assert_eq!(look_of(TileKind::Decade, "Из 2000-х"), Look::Glyph("2000-е".into()));
        // The lottery shows its coin, whatever covers come with it.
        assert_eq!(look_of(TileKind::Lottery, "Лотерея"), Look::Coin);
        // Without a cover a photo tile keeps its shape.
        assert_eq!(look(MixId::Tile(TileKind::YearAgo), "Год назад", &[], None), Look::Cover(MaterialShape::Arch));
        // Not one template: every kind of look turns up among the kinds.
        let looks: Vec<Look> = TileKind::ALL.iter().map(|k| look_of(*k, "Из 90-х")).collect();
        let kinds = |f: fn(&Look) -> bool| looks.iter().filter(|l| f(l)).count();
        assert!(kinds(|l| matches!(l, Look::Cover(_))) >= 3);
        assert!(kinds(|l| matches!(l, Look::Bleed(_))) >= 3);
        assert!(kinds(|l| matches!(l, Look::Glyph(_))) >= 3);
        assert!(kinds(|l| matches!(l, Look::Fan)) >= 1 && kinds(|l| matches!(l, Look::Heart)) >= 1);
        assert!(kinds(|l| matches!(l, Look::Icon(..))) >= 2 && kinds(|l| matches!(l, Look::Coin)) == 1);
    }

    #[test]
    fn an_artist_tile_finds_the_artists_photo() {
        let mut state = ViewState::default();
        let photo = nikgob_view::Cover::web("https://example.org/eve.jpg");
        state.home.artist_mixes =
            vec![ArtistCard { id: ArtistId::new(), name: "Eve".into(), image: Some(photo.clone()), liked: false, track_count: 0 }];
        assert_eq!(artist_photo(&state, " eve "), Some(photo.uri()));
        assert_eq!(artist_photo(&state, "Kevin Penkin"), None);
        let tile = MixCard { kicker: "ДАВНО НЕ СЛЫШАЛ".into(), title: "Eve".into(), ..mix(MixId::Tile(TileKind::Comeback)) };
        let t = BoardTile::of(&tile, &|name| artist_photo(&state, name));
        assert_eq!(t.look, Look::Bleed(photo.uri()));
        // Only the artist kinds look for one.
        let tile = MixCard { title: "Eve".into(), ..mix(MixId::Tile(TileKind::OnRepeat)) };
        assert_eq!(BoardTile::of(&tile, &|name| artist_photo(&state, name)).look, Look::Fan);
    }

    #[test]
    fn the_downloaded_only_chip_flips_the_preference() {
        let mut state = ViewState::default();
        state.prefs.fps_limit = 90;
        match downloaded_only(&state, true) {
            Command::SetPrefs(p) => {
                assert!(p.mixes_downloaded_only);
                // Nothing else changes.
                assert_eq!(*p, nikgob_view::Prefs { mixes_downloaded_only: true, ..state.prefs.clone() });
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn cards_of_one_thing_twice_in_a_row_get_their_own_ids() {
        // The feed may list a release twice: each card keeps its own id
        // (egui would warn of a clash and mix their hover and menus up).
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let release = full_home().recently_added[0].clone();
        let mut ids = Vec::new();
        // Fonts are ready from the second pass.
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut out = Vec::new();
            widgets::card_row_each(ui, "row", &[release.clone(), release.clone()], |ui, _, r| {
                let id = ui.id();
                let _ = widgets::release_card(ui, r, widgets::CARD_W, "recent", &mut out);
                ids.push(id);
            });
        });
        frame.textures_delta.clear();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
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
