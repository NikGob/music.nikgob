//! The phone app's interface: one column under the thumb, the three
//! destinations in the navigation bar (Главная, Библиотека, Настройки),
//! each with a stack of its own, and Поиск one tap away in the search bar
//! at the top of Главная and Библиотека (Metrolist's); our Главная as the
//! desktop draws it, laid out for the phone's width. The same code runs in the PC's
//! phone-size window (`nikgob-phone`) and, later, inside the Android shell:
//! the host hands it a [`MobileEnv`] (the system's insets, its back
//! gesture, the levels of what plays) and the runtime's state, and sends
//! the commands it returns. Nothing reacts to hover; whatever a pointer
//! could reach is reached by a tap, a long press or a swipe.

#![forbid(unsafe_code)]

mod detail;
mod gesture;
mod home;
mod library;
mod menu;
mod nav;
mod output;
mod parts;
mod player;
mod queue;
mod rows;
mod search;
mod settings;
mod text;
mod worlds;

use egui::{Context, Pos2, Rect, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::{NavBar, NavBarItem, anim};
use nikgob_md3e::screens::phone::{self, PhonePages};
use nikgob_md3e::{Icon, Theme};
use nikgob_view::{Command, LIKED_PLAYLIST, LibraryTab, Route, TrackRow, ViewState};

pub use nav::Tab;
use nav::{Nav, Opened};
use parts::PageOut;

/// The parts of the screen the system draws over, in dp: the status bar
/// (and a camera's cut-out) at the top, the gesture area at the bottom, and
/// the on-screen keyboard while it is up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SafeInsets {
    pub top: f32,
    pub bottom: f32,
    pub ime_bottom: f32,
}

/// What the host tells the interface each frame.
#[derive(Clone, Debug, Default)]
pub struct MobileEnv {
    pub insets: SafeInsets,
    /// The system's back: Android's back gesture; Esc or the mouse's back
    /// button on the PC.
    pub back_pressed: bool,
    /// The bass level of what plays (0..1).
    pub bass: f32,
    /// The spectrum bands of what plays (0..1 each), low to high.
    pub bands: Vec<f32>,
    /// The system asks for less motion.
    pub reduced_motion: bool,
}

/// Where a page is drawn: its rect (the whole width, from the top of the
/// screen), the status bar's height over it, and whether its scroll goes
/// back to the top (its destination tapped again).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageFrame {
    pub rect: Rect,
    pub top: f32,
    pub reset: bool,
}

/// A vertical list that a finger (or a mouse on the PC) drags, with no
/// scroll bar: a phone shows none.
pub(crate) fn scroll(id: impl std::hash::Hash + std::fmt::Debug, reset: bool) -> egui::ScrollArea {
    use egui::containers::scroll_area::{ScrollBarVisibility, ScrollSource};
    let mut area = egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .scroll_source(ScrollSource::ALL)
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden);
    if reset {
        area = area.vertical_scroll_offset(0.0);
    }
    area
}

/// The navigation bar's height above the gesture area.
pub const NAV_H: f32 = nikgob_md3e::components::nav_bar::HEIGHT;

/// The phone's interface and what it keeps between frames.
pub struct MobileUi {
    shell: PhonePages,
    nav: Nav,
    started: bool,
    /// The destination tapped again: its first page scrolls to the top.
    reset_scroll: bool,
    library: library::Library,
    search: search::Search,
    player: player::Player,
    queue: queue::QueueSheet,
    menu: menu::TrackMenu,
    output: output::OutputSheet,
    settings: settings::Settings,
    worlds: worlds::Worlds,
    /// Поиск over a destination: which one, and how deep its stack was
    /// when it opened (pages opened from it stack over it, and back comes
    /// to it again).
    searching: Option<(Tab, usize)>,
}

impl Default for MobileUi {
    fn default() -> Self {
        Self::new()
    }
}

impl MobileUi {
    pub fn new() -> Self {
        Self {
            shell: PhonePages::new(),
            nav: Nav::default(),
            started: false,
            reset_scroll: false,
            library: library::Library::default(),
            search: search::Search::default(),
            player: player::Player::default(),
            queue: queue::QueueSheet::default(),
            menu: menu::TrackMenu::default(),
            output: output::OutputSheet::default(),
            settings: settings::Settings::default(),
            worlds: worlds::Worlds::default(),
            searching: None,
        }
    }

    /// The destination shown.
    pub fn tab(&self) -> Tab {
        self.nav.tab()
    }

    /// Поиск is the page on screen.
    fn search_shown(&self) -> bool {
        self.searching.is_some_and(|(t, d)| t == self.nav.tab() && d == self.nav.depth())
    }

    /// Opens Поиск over the page on screen, with the keyboard.
    fn open_search(&mut self, now: f64) {
        if !self.search_shown() {
            self.searching = Some((self.nav.tab(), self.nav.depth()));
            self.nav.moved(1.0, now);
        }
        self.search.focus = true;
    }

    fn close_search(&mut self, now: f64) {
        if self.searching.take().is_some() {
            self.nav.moved(-1.0, now);
        }
    }

    /// One step back on the page: Поиск closes, else the stack pops.
    fn page_back(&mut self, now: f64) {
        if self.search_shown() {
            self.close_search(now);
        } else {
            self.nav.back(now);
        }
    }

    /// A step of a screenshot run (`NIKGOB_SHOT_STEPS` of nikgob-phone):
    /// the verbs that need the interface itself. Returns the frames to wait
    /// after it, or `None` for a verb it does not know.
    ///
    /// `tab:home|library|settings|search`, `search:text` (Поиск with that
    /// text), `chip:playlists|albums|artists|tracks`, `np` (`np:n` shoots n
    /// frames in), `close` (the player back to its capsule), `lyrics`,
    /// `queue`, `album[:Title]`, `artist[:Name]`, `playlist[:Name]`,
    /// `liked`, `menu` (the track menu of the first row of the page on
    /// screen, or of what plays; `menu:playlists` on its list of
    /// playlists), `output` («Где играть»), `liblist[:0]` (Библиотека's
    /// cards as a list, or back to the grid), `all_settings` (the desktop's
    /// whole settings from Настройки).
    pub fn shot_step(&mut self, ctx: &Context, verb: &str, arg: &str, state: &ViewState, out: &mut Vec<Command>) -> Option<u32> {
        let now = anim::now(ctx);
        let wait = match verb {
            "tab" => {
                self.player.close(now, state);
                match arg {
                    "search" => self.open_search(now),
                    "library" => self.select(Tab::Library, now, out),
                    "settings" => self.select(Tab::Settings, now, out),
                    _ => self.select(Tab::Home, now, out),
                }
                30
            }
            "search" => {
                self.player.close(now, state);
                self.open_search(now);
                self.search.set_query(arg, now);
                40
            }
            "chip" => {
                let tab = match arg {
                    "playlists" => LibraryTab::Playlists,
                    "albums" => LibraryTab::Albums,
                    "artists" => LibraryTab::Artists,
                    "tracks" => LibraryTab::Tracks,
                    _ => LibraryTab::All,
                };
                self.go(Route::Library(tab), now, out);
                if tab == LibraryTab::Tracks {
                    out.push(Command::LoadTracks { offset: 0, limit: 200, sort: state.library.track_sort });
                }
                30
            }
            "np" => {
                if state.player.current.is_some() {
                    self.player.open(now, state);
                }
                arg.parse().unwrap_or(60)
            }
            "close" => {
                self.player.close(now, state);
                40
            }
            "lyrics" => {
                if state.player.current.is_some() {
                    self.player.open(now, state);
                    self.player.lyrics = true;
                }
                60
            }
            "queue" => {
                if state.player.current.is_some() {
                    self.player.open(now, state);
                }
                self.queue.open(ctx);
                60
            }
            "album" => {
                let r = state.library.releases.iter().find(|r| arg.is_empty() || r.title == arg);
                if let Some(r) = r {
                    self.go(Route::Release(r.id), now, out);
                }
                30
            }
            "artist" => {
                let a = state.library.artists.iter().find(|a| a.name == arg).or(state.library.artists.first());
                if let Some(a) = a {
                    self.go(Route::Artist(a.id), now, out);
                }
                30
            }
            "playlist" => {
                let p = state.library.playlists.iter().find(|p| !p.pinned && (arg.is_empty() || p.name == arg));
                if let Some(p) = p {
                    self.go(Route::Playlist(p.id), now, out);
                }
                30
            }
            "liked" => {
                self.go(Route::Playlist(LIKED_PLAYLIST), now, out);
                30
            }
            "menu" => {
                if let Some((t, here)) = self.first_row(state) {
                    self.menu.open(t, here);
                    if arg == "playlists" {
                        self.menu.open_picker();
                    }
                }
                40
            }
            "liblist" => {
                self.library.set_list(arg != "0");
                10
            }
            "all_settings" => {
                self.player.close(now, state);
                self.select(Tab::Settings, now, out);
                self.go(Route::Settings, now, out);
                40
            }
            "output" => {
                self.output.open();
                40
            }
            _ => return None,
        };
        Some(wait)
    }

    /// The first track row of the page on screen and that page (what plays
    /// when the page shows none).
    fn first_row(&self, state: &ViewState) -> Option<(TrackRow, Route)> {
        let top = self.nav.top().cloned();
        let page = match &top {
            Some(Route::Release(_)) => state.release.as_ref().and_then(|r| r.tracks.first()),
            Some(Route::Playlist(_)) => state.playlist.as_ref().and_then(|p| p.tracks.first()),
            Some(Route::Artist(_)) => state.artist.as_ref().and_then(|a| a.top_tracks.first()),
            _ => None,
        };
        let here = top.clone().unwrap_or(Route::Home);
        page.or(state.library.tracks.first())
            .map(|t| (t.clone(), here))
            .or_else(|| state.player.current.clone().map(|t| (t, state.player.context_page.clone().unwrap_or(Route::Home))))
    }

    /// Draws one frame over the whole of `ui` (the phone's screen) and
    /// pushes what the user asked for to `out`.
    pub fn frame(&mut self, ui: &mut Ui, env: &MobileEnv, state: &ViewState, out: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let now = anim::now(&ctx);
        self.shell.set_system_reduced_motion(env.reduced_motion);
        self.shell.set_levels(env.bass);
        self.shell.begin(&ctx, state, out);
        if !std::mem::replace(&mut self.started, true) {
            out.push(Command::LoadHome);
            out.push(Command::LoadLibrary);
        }
        let theme = Theme::get(&ctx);
        let s = theme.scheme;
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, s.surface);
        let ins = env.insets;
        let keyboard = ins.ime_bottom > 0.0;
        let has_track = state.player.current.is_some();
        if self.player.animating(now) {
            ctx.request_repaint();
        }
        // The player's spring: 0 the capsule over the bar … 1 the screen.
        // The bar slides away under the growing player.
        let k = self.player.expand(now).clamp(0.0, 1.0);
        let bar_h = NAV_H + ins.bottom;
        let bar_top = full.bottom() - bar_h + bar_h * k;
        let mini_space = if has_track { player::MINI_H + player::MINI_GAP } else { 0.0 };
        let content_bottom = if keyboard { full.bottom() - ins.ime_bottom } else { full.bottom() - bar_h - mini_space };
        let content = Rect::from_min_max(full.min, pos2(full.right(), content_bottom));
        let mini = Rect::from_min_size(
            pos2(full.left() + player::MINI_MARGIN, bar_top - player::MINI_GAP - player::MINI_H),
            vec2(full.width() - 2.0 * player::MINI_MARGIN, player::MINI_H),
        );

        // Under a player open all the way nothing else is laid out.
        if k < 0.999 {
            // The back swipe from the left edge, while there is a page to
            // go back to.
            let can_back = self.nav.depth() > 0 || self.search_shown();
            let edge = gesture::edge_back(&ctx, content, egui::Id::new("phone-edge"), can_back && !self.player.is_open());
            if edge.back {
                self.page_back(now);
            }
            let reset = std::mem::take(&mut self.reset_scroll);
            let (dx, alpha) = self.transition(&ctx, now, content.width());
            let page_rect = content.translate(vec2(dx + edge.offset, 0.0));
            let frame = PageFrame { rect: page_rect, top: ins.top, reset };
            let salt = ("phone-page", self.nav.tab().index(), self.nav.depth(), self.search_shown());
            let mut pui = ui.new_child(UiBuilder::new().max_rect(page_rect).id_salt(salt));
            pui.set_clip_rect(content);
            if alpha < 1.0 {
                pui.multiply_opacity(alpha);
            }
            let po = self.page(&mut pui, frame, state, out);
            if po.back {
                self.page_back(now);
            }
            if po.search {
                self.open_search(now);
            }
            if let Some((t, here)) = po.menu {
                self.menu.open(t, here);
            }
            if let Some(r) = po.nav {
                self.go(r, now, out);
            }
            if !keyboard {
                let items: Vec<NavBarItem<'_>> =
                    Tab::ALL.iter().map(|t| NavBarItem { icon: t.icon(), label: t.label(), badge: None }).collect();
                let bar = Rect::from_min_size(pos2(full.left(), bar_top), vec2(full.width(), bar_h));
                if let Some(i) = NavBar::new("phone-nav", &items).selected(Some(self.nav.tab().index())).bottom_inset(ins.bottom).show(ui, bar) {
                    self.select(Tab::ALL[i], now, out);
                }
                if has_track {
                    self.player.mini(ui, mini, mini.top() - full.top(), state, out);
                }
            }
        }
        if self.shell.take_open_now_playing() && has_track {
            self.player.open(now, state);
        }
        let po = self.player.full(ui, full, mini, env, state, out);
        let here = state.player.context_page.clone().unwrap_or(Route::Home);
        if po.queue {
            self.queue.open(&ctx);
        }
        if po.menu
            && let Some(t) = state.player.current.clone()
        {
            self.menu.open(t, here.clone());
        }
        if po.sound
            && let Some(t) = state.player.current.clone()
        {
            self.shell.apply(&ctx, phone::TrackAct::Equalizer, &t, &here, state, out);
        }
        if po.output {
            self.output.open();
        }
        if let Some(r) = po.nav {
            self.go(r, now, out);
        }

        // The sheets, the pages' dialogs and the snackbars over everything.
        if self.queue.is_open()
            && let Some((t, here)) = self.queue.show(&ctx, state, out)
        {
            self.menu.open(t, here);
        }
        if let Some(r) = self.menu.show(&ctx, &mut self.shell, state, out) {
            self.queue.close(&ctx);
            self.player.close(now, state);
            self.go(r, now, out);
        }
        if self.output.show(&ctx, state, out) {
            self.player.close(now, state);
            self.go(Route::Devices, now, out);
        }
        let snack = full.bottom() - content_bottom + 8.0;
        if let Some(r) = self.shell.overlays(&ctx, state, full, snack, out) {
            self.player.close(now, state);
            self.go(r, now, out);
        }
        if let Some(r) = self.shell.take_nav() {
            self.player.close(now, state);
            self.go(r, now, out);
        }
        let back = self.shell.take_go_back() || env.back_pressed;
        if back {
            // The topmost thing open closes first.
            if self.menu.is_open() {
                self.menu.close();
            } else if self.output.is_open() {
                self.output.close();
            } else if self.queue.is_open() {
                self.queue.close(&ctx);
            } else if self.player.is_open() {
                self.player.close(now, state);
            } else {
                self.page_back(now);
            }
        }
    }

    /// The page on screen's offset and opacity while it comes in: from the
    /// side it was pushed from (or back from), or fading through on another
    /// destination; the default spatial spring.
    fn transition(&self, ctx: &egui::Context, now: f64, width: f32) -> (f32, f32) {
        let Some(c) = self.nav.change() else { return (0.0, 1.0) };
        let t = (now - c.at) as f32;
        if t >= 1.0 {
            return (0.0, 1.0);
        }
        ctx.request_repaint();
        let theme = Theme::get(ctx);
        let (x, _) = theme.motion.default_spatial.solve(1.0, 0.0, t);
        let (e, _) = theme.motion.default_effects.solve(1.0, 0.0, t);
        let shift = if theme.reduced_motion { 0.0 } else { c.dir * (width * 0.12).min(48.0) * x };
        (shift, (1.0 - e).clamp(0.0, 1.0))
    }

    /// The page of the destination's top.
    fn page(&mut self, ui: &mut Ui, frame: PageFrame, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
        if self.search_shown() {
            return self.search.show(ui, frame, state, out);
        }
        let top = self.nav.top().cloned();
        match (self.nav.tab(), top) {
            (Tab::Home, None) => home::show(ui, frame, &mut self.shell, &mut self.worlds, state, out),
            (Tab::Library, None) => self.library.show(ui, frame, state, out),
            (Tab::Settings, None) => self.settings.show(ui, frame, state, out),
            (_, Some(Route::Release(id))) => detail::album(ui, frame, id, state, out),
            (_, Some(Route::Playlist(id))) => detail::playlist(ui, frame, id, state, out),
            (_, Some(Route::Artist(id))) => detail::artist(ui, frame, id, state, out),
            (_, Some(route)) => PageOut::nav(self.desktop_page(ui, frame, &route, state, true, out)),
        }
    }

    /// A page the phone has no layout of its own for, as the desktop draws
    /// it, under a bar with ← when it is one of the stack's.
    fn desktop_page(&mut self, ui: &mut Ui, frame: PageFrame, route: &Route, state: &ViewState, back: bool, out: &mut Vec<Command>) -> Option<Route> {
        let mut top = frame.rect.top() + frame.top;
        if back {
            let bar = Rect::from_min_size(pos2(frame.rect.left() + 4.0, top + 4.0), vec2(48.0, 48.0));
            if crate::parts::put(ui, bar, nikgob_md3e::components::IconButton::new(Icon::ArrowBack).id_salt("phone-page-back")).clicked() {
                self.nav.back(anim::now(ui.ctx()));
                return None;
            }
            top += 56.0;
        }
        let rect = Rect::from_min_max(Pos2::new(frame.rect.left() + 16.0, top), frame.rect.max);
        let mut cui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("desktop-page", format!("{route:?}"))));
        self.shell.page(&mut cui, route, state, 16.0, out)
    }

    /// Opens `route` where it belongs and asks for what it shows.
    fn go(&mut self, route: Route, now: f64, out: &mut Vec<Command>) {
        if let Some(c) = nav::load_for(&route) {
            out.push(c);
        }
        match self.nav.open(route, now) {
            Opened::Library(chip) => {
                self.searching = None;
                self.library.set_tab(chip);
            }
            Opened::Tab(_) => self.searching = None,
            Opened::Pushed | Opened::Same => {}
        }
    }

    /// A tap on a destination of the bar.
    fn select(&mut self, tab: Tab, now: f64, out: &mut Vec<Command>) {
        // Another destination, or this one's first page: Поиск closes.
        self.searching = None;
        if self.nav.select(tab, now) {
            self.reset_scroll = true;
        }
        match tab {
            Tab::Home => out.push(Command::LoadHome),
            Tab::Library => out.push(Command::LoadLibrary),
            Tab::Settings => {}
        }
    }
}
