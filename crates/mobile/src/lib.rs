//! The phone app's interface: one column under the thumb, the four
//! destinations in the navigation bar (Главная, Поиск, Библиотека,
//! Настройки), each with a stack of its own; our Главная as the desktop
//! draws it, laid out for the phone's width. The same code runs in the PC's
//! phone-size window (`nikgob-phone`) and, later, inside the Android shell:
//! the host hands it a [`MobileEnv`] (the system's insets, its back
//! gesture, the levels of what plays) and the runtime's state, and sends
//! the commands it returns. Nothing reacts to hover; whatever a pointer
//! could reach is reached by a tap, a long press or a swipe.

#![forbid(unsafe_code)]

mod gesture;
mod home;
mod menu;
mod nav;
mod player;
mod queue;
mod rows;
mod text;

use egui::{Pos2, Rect, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::{NavBar, NavBarItem, anim};
use nikgob_md3e::screens::phone::{self, PhonePages};
use nikgob_md3e::{Icon, Theme};
use nikgob_view::{Command, LibraryTab, Route, ViewState};

pub use nav::Tab;
use nav::{Nav, Opened};

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
    /// Библиотека's chip, as a route asked for it.
    library_tab: LibraryTab,
    player: player::Player,
    queue: queue::QueueSheet,
    menu: menu::TrackMenu,
    /// «Где играть» was asked for (the player's output).
    output_asked: bool,
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
            library_tab: LibraryTab::All,
            player: player::Player::default(),
            queue: queue::QueueSheet::default(),
            menu: menu::TrackMenu::default(),
            output_asked: false,
        }
    }

    /// The destination shown.
    pub fn tab(&self) -> Tab {
        self.nav.tab()
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
            let edge = gesture::edge_back(&ctx, content, egui::Id::new("phone-edge"), self.nav.depth() > 0 && !self.player.is_open());
            if edge.back {
                self.nav.back(now);
            }
            let reset = std::mem::take(&mut self.reset_scroll);
            let (dx, alpha) = self.transition(&ctx, now, content.width());
            let page_rect = content.translate(vec2(dx + edge.offset, 0.0));
            let frame = PageFrame { rect: page_rect, top: ins.top, reset };
            let mut pui = ui.new_child(UiBuilder::new().max_rect(page_rect).id_salt(("phone-page", self.nav.tab().index(), self.nav.depth())));
            pui.set_clip_rect(content);
            if alpha < 1.0 {
                pui.multiply_opacity(alpha);
            }
            if let Some(r) = self.page(&mut pui, frame, state, out) {
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
            self.output_asked = true;
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
            } else if self.queue.is_open() {
                self.queue.close(&ctx);
            } else if self.player.is_open() {
                self.player.close(now, state);
            } else {
                self.nav.back(now);
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
    fn page(&mut self, ui: &mut Ui, frame: PageFrame, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
        let top = self.nav.top().cloned();
        match (self.nav.tab(), top) {
            (Tab::Home, None) => home::show(ui, frame, &mut self.shell, state, out),
            (Tab::Library, None) => {
                let route = Route::Library(self.library_tab);
                self.desktop_page(ui, frame, &route, state, false, out)
            }
            (Tab::Settings, None) => self.desktop_page(ui, frame, &Route::Settings, state, false, out),
            (Tab::Search, None) => None,
            (_, Some(route)) => self.desktop_page(ui, frame, &route, state, true, out),
        }
    }

    /// A page the phone has no layout of its own for, as the desktop draws
    /// it, under a bar with ← when it is one of the stack's.
    fn desktop_page(&mut self, ui: &mut Ui, frame: PageFrame, route: &Route, state: &ViewState, back: bool, out: &mut Vec<Command>) -> Option<Route> {
        let mut top = frame.rect.top() + frame.top;
        if back {
            let bar = Rect::from_min_size(pos2(frame.rect.left() + 4.0, top + 4.0), vec2(48.0, 48.0));
            if ui.put(bar, nikgob_md3e::components::IconButton::new(Icon::ArrowBack).id_salt("phone-page-back")).clicked() {
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
            Opened::Library(chip) => self.library_tab = chip,
            Opened::Tab(_) | Opened::Pushed | Opened::Same => {}
        }
    }

    /// A tap on a destination of the bar.
    fn select(&mut self, tab: Tab, now: f64, out: &mut Vec<Command>) {
        if self.nav.select(tab, now) {
            self.reset_scroll = true;
        }
        match tab {
            Tab::Home => out.push(Command::LoadHome),
            Tab::Library => out.push(Command::LoadLibrary),
            Tab::Search | Tab::Settings => {}
        }
    }
}
