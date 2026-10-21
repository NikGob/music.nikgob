//! Главная (md3e-player.md §3.1, docs/stage4.md §4–§5): greeting, filter
//! chips, the «Твой микс» hero card, then feed rows: moods, the albums to
//! continue, style and artist mixes, new music from the services and the
//! library's own rows.

use egui::{Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_shapes::{MaterialShape, paint as shape_paint};
use nikgob_view::{
    AnalysisProgress, Command, ContinueCard, HomeView, LibraryTab, MixCard, MixId, Mood, RecTarget, Route, Service,
    StreamPage, StreamTrackRow, ViewState, WishlistStatus,
};

use super::{PageState, widgets};
use crate::{
    ColorScheme, Icon, Theme, TypeRole,
    components::{CoverStatus, FilterChip, IconButton, MediaCard, Menu, MenuItem, Radii, Size, Style, anim, paint},
    tokens::shape,
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
    /// What comes from the library: the mix, what to continue, the style
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
    Hero,
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
        Self::Hero,
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
            Filter::Mine => matches!(self, Hero | Continue | StyleMixes | ArtistMixes | LongUnplayed | RecentlyAdded),
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
            Hero => h.hero.is_some() || !h.mix.is_empty(),
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

    let empty = h.mix.is_empty() && h.hero.is_none() && h.recently_added.is_empty() && h.continue_listening.is_empty();
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
    // The analysis line sits under the hero; without one, over the feed.
    let analysis = analysis_line(&state.analysis);
    let mut analysis_said = false;
    for (n, section) in shown.iter().enumerate() {
        match section {
            Section::Hero => {
                hero_mix(ui, h, out);
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

/// The hero card: covers cropped into Material shapes, title, the quiet
/// line, play L and ⋮ «Обновить».
fn hero_mix(ui: &mut Ui, h: &HomeView, out: &mut Vec<Command>) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    let height = 200.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let radii = Radii::all(shape::EXTRA_EXTRA_LARGE);
    paint::container(ui.painter(), rect, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(
        ui.ctx(),
        ui.id().with("hero-layer"),
        if resp.hovered() { 0.08 } else { 0.0 },
        theme.motion.fast_effects,
    );
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);

    // Entrance: covers spring up from below once.
    let enter = anim::fraction(ui.ctx(), ui.id().with("hero-enter"), 1.0, theme.motion.slow_spatial);
    let shapes = [
        MaterialShape::Cookie9Sided,
        MaterialShape::Clover4Leaf,
        MaterialShape::Sunny,
        MaterialShape::Pill,
    ];
    // The mix's own covers; before the first build, those of its tracks.
    let mut covers: Vec<String> = h.hero.as_ref().map(|c| c.covers.iter().map(|c| c.uri()).collect()).unwrap_or_default();
    if covers.is_empty() {
        for t in &h.mix {
            if let Some(c) = &t.cover
                && !covers.contains(&c.uri())
            {
                covers.push(c.uri());
            }
        }
    }
    covers.truncate(4);
    let narrow = width < 760.0;
    let art_w = if narrow { (width * 0.34).min(240.0) } else { (width * 0.42).min(360.0) };
    let size = if narrow { 80.0 } else { 112.0 };
    for (i, uri) in covers.iter().enumerate() {
        let x = rect.left() + 24.0 + i as f32 * (art_w - size - 24.0) / 3.0_f32.max(1.0);
        let y = rect.center().y - size * 0.5 + if i % 2 == 0 { -14.0 } else { 14.0 };
        let r = Rect::from_min_size(pos2(x, y + (1.0 - enter) * 40.0), vec2(size, size));
        widgets::shape_image(ui, Some(uri), "", r, shapes[i % shapes.len()]);
    }

    let text_x = rect.left() + art_w + 48.0;
    let text_rect = Rect::from_min_max(pos2(text_x, rect.top() + 32.0), pos2(rect.right() - 128.0, rect.bottom() - 16.0));
    let mut tui = ui.new_child(UiBuilder::new().max_rect(text_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    tui.label(TypeRole::LabelLarge.style().rich("ДЛЯ ТЕБЯ", s.primary));
    let title_role = if narrow { TypeRole::HeadlineMedium } else { TypeRole::DisplaySmall };
    let title = h.hero.as_ref().map_or("Твой микс", |c| c.title.as_str());
    tui.label(title_role.emphasized().rich(title, s.on_surface));
    let subtitle = match &h.hero {
        Some(c) => c.subtitle.clone(),
        None => {
            let n = h.mix.len() as u64;
            format!("{n} {} · любимое и дослушанное", crate::screens::common::plural(n, "трек", "трека", "треков"))
        }
    };
    if !subtitle.is_empty() {
        tui.label(TypeRole::BodyLarge.style().rich_para(subtitle, s.on_surface_variant));
    }

    // ⋮ in the corner: «Обновить» picks a new mix for today.
    let more_rect = Rect::from_min_size(pos2(rect.right() - 16.0 - 40.0, rect.top() + 16.0), vec2(40.0, 40.0));
    let mut mui = ui.new_child(UiBuilder::new().max_rect(more_rect).id_salt("hero-more"));
    let more = mui.add(IconButton::new(Icon::MoreVert).size(Size::S).tooltip("Ещё").id_salt("hero-more-btn"));
    if h.hero.is_some()
        && Menu::new("hero-menu")
            .align_right(true)
            .item(MenuItem::new("Обновить").icon(Icon::Refresh))
            .attach(&more)
            == Some(0)
    {
        out.push(Command::RefreshMix { id: MixId::Daily });
    }

    let play_rect = Rect::from_center_size(pos2(rect.right() - 24.0 - 48.0, rect.center().y + 12.0), vec2(96.0, 96.0));
    let mut pui = ui.new_child(UiBuilder::new().max_rect(play_rect).id_salt("hero-play"));
    let play = pui.add(
        IconButton::new(Icon::PlayArrow)
            .size(Size::L)
            .style(Style::Filled)
            .filled_icon(true)
            .tooltip("Слушать микс")
            .id_salt("hero-play-btn"),
    );
    if (play.clicked() || resp.clicked()) && !more.clicked() {
        out.push(match &h.hero {
            Some(_) => Command::PlayMix { id: MixId::Daily },
            // Before the first build: the tracks the home already has.
            None => Command::PlayTracks {
                tracks: h.mix.iter().map(|t| t.id).collect(),
                start: 0,
                context: "Твой микс".into(),
            },
        });
    }
}

pub const MOOD_W: f32 = 200.0;
pub const MOOD_H: f32 = 160.0;

/// A mood: a large filled card in its scheme colour, the cover of what it
/// plays first cut into its shape, the name and a plain line on how it
/// sounds; ▶ on hover. A click plays it (an endless mix).
fn mood_card(ui: &mut Ui, m: &MixCard, out: &mut Vec<Command>) {
    let MixId::Mood(mood) = m.id else { return };
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (rect, resp) = ui.allocate_exact_size(vec2(MOOD_W, MOOD_H), Sense::click());
    let id = ui.id().with(("mood", mood));
    let hover = resp.contains_pointer();
    let squish = anim::f32(ui.ctx(), id.with("squish"), if resp.is_pointer_button_down_on() { 0.96 } else { 1.0 }, theme.motion.fast_spatial);
    let card = Rect::from_center_size(rect.center(), rect.size() * squish);
    let (fill, on) = mood_colors(mood, &s);
    let radii = Radii::all(shape::EXTRA_LARGE);
    paint::container(ui.painter(), card, radii, fill, Stroke::NONE);

    // The cover in the mood's shape, turning a little under the pointer.
    let turn = anim::f32(ui.ctx(), id.with("turn"), if hover { 0.12 } else { 0.0 }, theme.motion.default_spatial);
    let art = Rect::from_min_size(card.min + vec2(16.0, 16.0), vec2(64.0, 64.0));
    let shape = mood_shape(mood);
    match m.covers.first() {
        Some(c) => widgets::shape_image(ui, Some(&c.uri()), "", art, shape),
        None => shape_paint::fill_shape(ui.painter(), shape.polygon().cubics(), art, turn, on.gamma_multiply(0.16)),
    }

    let text_w = card.width() - 32.0;
    let line = |text: &str, role: TypeRole| {
        let mut job = egui::text::LayoutJob::single_section(text.to_owned(), role.style().text_format(on));
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
        ui.painter().layout_job(job)
    };
    let title = line(&m.title, TypeRole::TitleLarge);
    let sub = line(mood_line(mood), TypeRole::BodyMedium);
    let sub_y = card.bottom() - 16.0 - sub.size().y;
    let title_y = sub_y - 2.0 - title.size().y;
    ui.painter().galley(pos2(card.left() + 16.0, title_y), title, on);
    ui.painter().galley(pos2(card.left() + 16.0, sub_y), sub, on);

    let layer = anim::fraction(ui.ctx(), id.with("layer"), if hover { 0.08 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), card, radii, on, layer);

    // ▶ in the corner with the pointer (fast effects).
    let k = anim::fraction(ui.ctx(), id.with("play"), f32::from(u8::from(hover)), theme.motion.fast_effects).clamp(0.0, 1.0);
    let mut play = false;
    if k > 0.01 {
        let size = Size::M.height();
        let slot = Rect::from_min_size(pos2(card.right() - 16.0 - size, card.top() + 16.0), vec2(size, size));
        let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(("mood-play-slot", mood)));
        child.set_opacity(k);
        play = child
            .add_enabled(
                k > 0.5,
                IconButton::new(Icon::PlayArrow)
                    .size(Size::M)
                    .style(Style::Filled)
                    .filled_icon(true)
                    .tooltip(format!("Слушать «{}»", m.title))
                    .id_salt(("mood-play", mood)),
            )
            .clicked();
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if play || resp.clicked() {
        out.push(Command::PlayMix { id: m.id });
    }
}

/// A style mix: a collage of its albums and its name («A, B и другие» —
/// two lines when long). A click or ▶ plays it (an endless mix).
fn style_card(ui: &mut Ui, m: &MixCard, out: &mut Vec<Command>) {
    let mut card = MediaCard::new(&m.title)
        .collage(m.covers.iter().map(|c| c.uri()).collect())
        .placeholder_icon(Icon::QueueMusic)
        .title_two_lines(true)
        .width(widgets::CARD_W)
        .id_salt(("style-mix", m.title.as_str()));
    if !m.subtitle.is_empty() {
        card = card.subtitle(&m.subtitle);
    }
    let o = card.show(ui);
    if o.play_clicked || o.card_clicked {
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
            vec![Hero, Moods, Continue, StyleMixes, ArtistMixes, Discover, LongUnplayed, RecentlyAdded]
        );
        assert_eq!(
            sections(Filter::Mine, &h, false),
            vec![Hero, Continue, StyleMixes, ArtistMixes, LongUnplayed, RecentlyAdded]
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
        // The hero before the first build still plays the home's tracks.
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
        assert_eq!(sections(Filter::All, &h, false), vec![Section::Hero]);
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
