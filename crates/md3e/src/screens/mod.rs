//! The MD3E design system's screens over the shared view model.
//!
//! [`Md3eUi::frame`] draws a whole window from a [`ViewState`] and pushes
//! [`Command`]s for the runtime. It owns only presentation state: the
//! navigation stack, which panes are open, transitions.

mod ambient;
mod chrome;
mod common;
mod pages;
mod pane;
mod player_bar;
mod tag_editor;
mod visualizer;

pub mod now_playing;

use std::collections::HashMap;

use egui::{Context, Key, Modifiers, Rect, Ui, UiBuilder, pos2, vec2};
use nikgob_motion::{Animated, Spring};
use nikgob_view::{Command, Route, ThemeMode, ViewState};

use crate::{
    ColorScheme, Contrast, Icon, SchemeVariant, SizeClass, Theme,
    components::{NavRail, RailItem, SideSheet, anim, rail},
    size_class,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaneTab {
    #[default]
    Queue,
    Lyrics,
    Info,
}

/// Presentation state of the MD3E window.
pub struct Md3eUi {
    stack: Vec<Route>,
    /// +1 forward, -1 back, 0 fade-through (top-level switch).
    nav_dir: f32,
    nav_anim: Animated<f32>,
    rail_expanded: bool,
    pane: Option<PaneTab>,
    last_pane: PaneTab,
    was_inline: bool,
    now_playing: bool,
    np_anim: Animated<f32>,
    /// Where the floating player was last frame (container-transform origin).
    bar_rect: Rect,
    /// The content panel (for snackbars and centred popups).
    content_rect: Rect,
    search: String,
    search_focus: bool,
    scheme: SchemeAnim,
    pub(crate) pages: pages::PageState,
    chrome: chrome::Chrome,
    system_reduced_motion: bool,
    fullscreen: bool,
}

impl Default for Md3eUi {
    fn default() -> Self {
        Self::new()
    }
}

impl Md3eUi {
    pub fn new() -> Self {
        Self {
            stack: vec![Route::Home],
            nav_dir: 0.0,
            nav_anim: Animated::new(0.0, crate::tokens::Motion::expressive().slow_effects),
            rail_expanded: true,
            // The pane opens on demand (queue / lyrics buttons), not always.
            pane: None,
            last_pane: PaneTab::Queue,
            was_inline: true,
            now_playing: false,
            np_anim: Animated::new(0.0, crate::tokens::Motion::expressive().slow_spatial),
            bar_rect: Rect::NOTHING,
            content_rect: Rect::NOTHING,
            search: String::new(),
            search_focus: false,
            scheme: SchemeAnim::default(),
            pages: pages::PageState::default(),
            chrome: chrome::Chrome::default(),
            system_reduced_motion: false,
            fullscreen: false,
        }
    }

    pub fn route(&self) -> &Route {
        self.stack.last().unwrap_or(&Route::Home)
    }

    /// Opens a route. Top-level routes replace the stack (fade through);
    /// detail routes push (forward).
    pub fn navigate(&mut self, ctx: &Context, route: Route, out: &mut Vec<Command>) {
        if self.route() == &route {
            return;
        }
        let top_level = matches!(
            route,
            Route::Home | Route::Library(_) | Route::Downloads(_) | Route::Settings
        );
        if top_level {
            self.stack.clear();
            self.nav_dir = 0.0;
        } else {
            self.nav_dir = 1.0;
        }
        load_for(&route, out);
        self.stack.push(route);
        self.search.clear();
        self.restart_nav(ctx);
    }

    pub fn back(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.nav_dir = -1.0;
            load_for(self.route(), out);
            self.restart_nav(ctx);
            true
        } else {
            false
        }
    }

    fn restart_nav(&mut self, ctx: &Context) {
        let t = anim::now(ctx);
        let spring = anim::effective(ctx, Theme::get(ctx).motion.default_spatial);
        self.nav_anim.snap(1.0);
        self.nav_anim.animate_to(0.0, spring, t);
    }

    pub fn set_now_playing(&mut self, ctx: &Context, open: bool) {
        self.now_playing = open;
        if !open {
            self.chrome.np_sheet = false;
        } else if !self.was_inline {
            // A modal pane sheet must not stay on top of Now Playing.
            self.pane = None;
        }
        let spring = anim::effective(ctx, Theme::get(ctx).motion.slow_spatial);
        self.np_anim
            .animate_to(if open { 1.0 } else { 0.0 }, spring, anim::now(ctx));
    }

    /// Recomputes and animates the colour scheme from the current cover.
    fn update_theme(&mut self, ctx: &Context, state: &ViewState) {
        let prefs = &state.prefs;
        let dark = match prefs.theme {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => ctx.system_theme() != Some(egui::Theme::Light),
        };
        let seed = state
            .player
            .current
            .as_ref()
            .and_then(|t| t.cover.as_ref())
            .and_then(|c| c.seed)
            .unwrap_or(prefs.fallback_seed);
        let variant = match prefs.scheme_variant {
            1 => SchemeVariant::TonalSpot,
            2 => SchemeVariant::Vibrant,
            3 => SchemeVariant::Expressive,
            4 => SchemeVariant::Fidelity,
            5 => SchemeVariant::Monochrome,
            6 => SchemeVariant::Neutral,
            _ => SchemeVariant::Content,
        };
        let contrast = match prefs.contrast {
            1 => Contrast::Medium,
            2 => Contrast::High,
            _ => Contrast::Standard,
        };
        let spring = anim::effective(ctx, Theme::get(ctx).motion.slow_effects);
        let scheme = self
            .scheme
            .frame(ctx, (seed, dark, variant, contrast), spring);
        let theme = Theme::new(scheme, dark).with_reduced_motion(prefs.reduced_motion || self.system_reduced_motion);
        let changed = Theme::get(ctx) != theme;
        if changed {
            theme.install(ctx);
        }
    }

    /// Draws one frame.
    pub fn frame(&mut self, root: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {        let ctx = &root.ctx().clone();
        let first_cmd = out.len();
        self.update_theme(ctx, state);
        self.chrome.wheel(ctx, state, out);
        self.shortcuts(ctx, state, out);
        let theme = Theme::get(ctx);
        let s = theme.scheme;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(s.surface))
            .show(root, |ui| {
                let full = ui.max_rect();
                let class = size_class(full.width());
                if class == SizeClass::Compact {
                    // Mini mode: Now Playing only.
                    now_playing::show(ui, full, state, 1.0, true, &mut self.pages, &mut self.chrome, out);
                    return;
                }
                self.layout(ui, full, class, state, out);
            });

        // Requests pages made this frame.
        if std::mem::take(&mut self.pages.go_back) {
            self.back(ctx, out);
        }
        if let Some(tab) = self.pages.open_pane.take() {
            self.pane = Some(tab);
            self.pages.np_tab = tab;
        }
        if self.now_playing && self.pages.pending_nav.is_some() {
            self.set_now_playing(ctx, false);
        }
        self.pages
            .overlays(ctx, state, self.content_rect, out);
        self.chrome.overlays(ctx, state, out);

        // Now Playing overlay, transformed out of the floating player.
        let t = anim::now(ctx);
        let k = self.np_anim.value(t);
        if !self.np_anim.is_settled(t) {
            ctx.request_repaint();
        }
        if k > 0.001 {
            let full = ctx.content_rect();
            if size_class(full.width()) != SizeClass::Compact {
                egui::Area::new(egui::Id::new("md3e-now-playing"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(full.min)
                    .show(ctx, |ui| {
                        let from = if self.bar_rect.is_positive() {
                            self.bar_rect
                        } else {
                            full
                        };
                        let rect = Rect::from_min_max(
                            from.min + (full.min - from.min) * k,
                            from.max + (full.max - from.max) * k,
                        );
                        ui.set_clip_rect(rect);
                        let closed = now_playing::show(
                            ui, rect, state, k, false, &mut self.pages, &mut self.chrome, out,
                        );
                        if closed {
                            self.set_now_playing(ctx, false);
                        }
                    });
            }
        }

        if self.chrome.visualizer && self.chrome.viz.show(ctx, state, &self.chrome.bands) {
            self.chrome.visualizer = false;
        }
        // The visualizer takes the whole screen, not just the window.
        if self.chrome.visualizer != self.fullscreen {
            self.fullscreen = self.chrome.visualizer;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }

        // Requests from menus: navigate (closing Now Playing), show a pane tab.
        if let Some(route) = self.chrome.nav.take() {
            if self.now_playing {
                self.set_now_playing(ctx, false);
            }
            self.search.clear();
            self.navigate(ctx, route, out);
        }
        if let Some(tab) = self.chrome.open_pane.take() {
            self.pane = Some(tab);
            self.pages.np_tab = tab;
        }
        // A torrent search for a better copy shows its results on the search page.
        let query = out[first_cmd..].iter().rev().find_map(|c| match c {
            Command::SearchTorrents(q) => Some(q.clone()),
            _ => None,
        });
        if let Some(q) = query {
            if self.now_playing {
                self.set_now_playing(ctx, false);
            }
            self.search = q.clone();
            out.push(Command::Search(q));
        }

        // Esc last: open dialogs, menus and sheets take it first. Order:
        // visualizer (in `shortcuts`), dialogs, sheet, Now Playing, search, back.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.now_playing {
                self.set_now_playing(ctx, false);
            } else if !self.search.is_empty() {
                self.search.clear();
            } else {
                self.back(ctx, out);
            }
        }
    }

    fn shortcuts(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        let mut previous = false;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::F11) {
                self.chrome.visualizer = !self.chrome.visualizer;
            }
            if self.chrome.visualizer && i.consume_key(Modifiers::NONE, Key::Escape) {
                self.chrome.visualizer = false;
            }
            if i.consume_key(Modifiers::COMMAND, Key::K) || i.consume_key(Modifiers::COMMAND, Key::F) {
                self.search_focus = true;
            }
            if i.consume_key(Modifiers::COMMAND, Key::Q) {
                self.pane = if self.pane == Some(PaneTab::Queue) {
                    None
                } else {
                    Some(PaneTab::Queue)
                };
            }
            if i.consume_key(Modifiers::COMMAND, Key::L)
                && let Some(t) = &state.player.current
            {
                out.push(Command::Like(nikgob_view::LikeTarget::Track(t.id), !t.liked));
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowRight) {
                out.push(Command::Next);
            }
            if i.consume_key(Modifiers::COMMAND, Key::ArrowLeft) {
                previous = true;
            }
            if typing {
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Space) {
                out.push(Command::TogglePlay);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                out.push(Command::SeekBy(5_000));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                out.push(Command::SeekBy(-5_000));
            }
            if self.now_playing {
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    out.push(Command::SetVolume((state.player.volume + 0.05).min(1.0)));
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    out.push(Command::SetVolume((state.player.volume - 0.05).max(0.0)));
                }
            }
        });
        if previous {
            chrome::press_previous(ctx, state, out);
        }
        let mouse_back = ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Extra1));
        if mouse_back && !self.now_playing && !self.chrome.visualizer {
            self.back(ctx, out);
        }
    }

    fn layout(
        &mut self,
        ui: &mut Ui,
        full: Rect,
        class: SizeClass,
        state: &ViewState,
        out: &mut Vec<Command>,
    ) {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let s = theme.scheme;

        // Rail.
        let wants_expanded = self.rail_expanded && class >= SizeClass::Large;
        let expansion = anim::fraction(
            &ctx,
            egui::Id::new("rail-expansion"),
            if wants_expanded { 1.0 } else { 0.0 },
            theme.motion.default_spatial,
        );
        let rail_w = rail::width(expansion);
        let rail_rect = Rect::from_min_size(full.min, vec2(rail_w, full.height()));

        // Supporting pane (inline on Large/XL).
        let inline_pane = class >= SizeClass::Large;
        // Dropping below Large turns the inline pane into a modal sheet: do
        // not pop it open over the content just because it was visible.
        if !inline_pane && self.was_inline {
            self.pane = None;
        }
        self.was_inline = inline_pane;
        let pane_target = if inline_pane && self.pane.is_some() {
            if class == SizeClass::ExtraLarge { 412.0 } else { 360.0 }
        } else {
            0.0
        };
        let pane_w = anim::f32(
            &ctx,
            egui::Id::new("pane-width"),
            pane_target,
            theme.motion.default_spatial,
        )
        .max(0.0);
        let gap = if pane_w > 1.0 { 24.0 } else { 0.0 };
        let content = Rect::from_min_max(
            pos2(rail_rect.right(), full.top()),
            pos2(full.right() - pane_w - gap, full.bottom()),
        );
        self.content_rect = content;

        // Rail.
        let route = self.route().clone();
        let selected = match route {
            Route::Home => Some(0),
            Route::Library(_) | Route::Release(_) | Route::Artist(_) | Route::Playlist(_) => {
                Some(1)
            }
            Route::Downloads(_) => Some(2),
            Route::Settings => None,
        };
        let pending = state.downloads.confirm.len() as u32;
        let items = [
            RailItem { icon: Icon::Home, label: "Главная", badge: None },
            RailItem { icon: Icon::LibraryMusic, label: "Библиотека", badge: None },
            RailItem { icon: Icon::Download, label: "Загрузки", badge: Some(pending) },
        ];
        let bottom = [RailItem { icon: Icon::Settings, label: "Настройки", badge: None }];
        let rail_out = ui
            .scope_builder(UiBuilder::new().max_rect(rail_rect), |ui| {
                ui.set_clip_rect(rail_rect);
                NavRail {
                    id: egui::Id::new("md3e-rail"),
                    expansion,
                    fab: Some((Icon::Add, "Добавить")),
                    items: &items,
                    bottom: &bottom,
                    selected,
                    bottom_selected: (route == Route::Settings).then_some(0),
                }
                .show(ui)
            })
            .inner;
        if let Some(i) = rail_out.clicked {
            let r = [
                Route::Home,
                Route::Library(Default::default()),
                Route::Downloads(Default::default()),
            ][i]
                .clone();
            self.navigate(&ctx, r, out);
        }
        if rail_out.bottom_clicked.is_some() {
            self.navigate(&ctx, Route::Settings, out);
        }
        if let Some(fab) = &rail_out.fab {
            self.pages.add_menu(ui, fab, out);
        }
        // Rail collapse toggle on Large+, in the rail's top space.
        if class >= SizeClass::Large {
            let r = Rect::from_min_size(rail_rect.min + vec2(28.0, 2.0), vec2(40.0, 40.0));
            let mut tui = ui.new_child(UiBuilder::new().max_rect(r).id_salt("rail-toggle"));
            let expanded = self.rail_expanded;
            if tui
                .add(
                    crate::components::IconButton::new(if expanded { Icon::ChevronLeft } else { Icon::ChevronRight })
                        .size(crate::components::Size::S)
                        .tooltip(if expanded { "Свернуть меню" } else { "Развернуть меню" }),
                )
                .clicked()
            {
                self.rail_expanded = !expanded;
            }
        }

        // Content panel: search bar on top, page under it, player floating.
        let margin = class.margin();
        let inner = content.shrink2(vec2(margin, 0.0));
        let search_rect = Rect::from_min_size(inner.min + vec2(0.0, 16.0), vec2(inner.width(), 56.0));
        let page_rect = Rect::from_min_max(pos2(inner.left(), search_rect.bottom() + 16.0), inner.max);

        pages::search_bar(ui, search_rect, &mut self.search, &mut self.search_focus, out);

        // Page with nav transition.
        let t = anim::now(&ctx);
        let k = self.nav_anim.value(t);
        if !self.nav_anim.is_settled(t) {
            ctx.request_repaint();
        }
        let offset = self.nav_dir * 48.0 * k;
        let page_ui_rect = page_rect.translate(vec2(offset, 0.0));
        let mut page_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(page_ui_rect)
                .id_salt(("page", self.stack.len())),
        );
        page_ui.set_clip_rect(page_rect);
        page_ui.multiply_opacity((1.0 - k.abs()).clamp(0.0, 1.0));
        let bottom_inset = 64.0 + 16.0 + 16.0;
        let mut nav = None;
        if self.search.trim().is_empty() {
            nav = pages::page(&mut page_ui, &route, state, bottom_inset, &mut self.pages, out);
        } else {
            nav = pages::search_results(&mut page_ui, &self.search, state, bottom_inset, &mut self.pages, out).or(nav);
        }
        if let Some(r) = nav {
            self.navigate(&ctx, r, out);
        }

        // Floating player.
        let bar_w = (content.width() - 32.0).min(960.0);
        let bar = Rect::from_center_size(
            pos2(content.center().x, content.bottom() - 16.0 - 32.0),
            vec2(bar_w, 64.0),
        );
        self.bar_rect = bar;
        if state.player.current.is_some() || !state.queue.next.is_empty() {
            let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("bar"));
            let bo = player_bar::show(
                &mut bar_ui,
                bar,
                state,
                self.pane,
                class == SizeClass::Medium,
                &mut self.chrome,
                out,
            );
            if bo.open_now_playing {
                self.set_now_playing(&ctx, true);
            }
            if let Some(tab) = bo.toggle_pane {
                self.pane = if self.pane == Some(tab) { None } else { Some(tab) };
            }
        }

        // Supporting pane.
        if let Some(mut tab) = self.pane.or(Some(self.last_pane)) {
            if inline_pane && pane_w > 1.0 {
                let rect = Rect::from_min_max(
                    pos2(full.right() - pane_w, full.top() + 16.0),
                    pos2(full.right() - 16.0 * (pane_w / pane_target.max(1.0)), full.bottom() - 16.0),
                );
                ui.painter().rect_filled(
                    rect,
                    crate::tokens::shape::EXTRA_LARGE as u8,
                    s.surface_container_low,
                );
                let mut pui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(rect.shrink(16.0))
                        .id_salt("pane"),
                );
                pui.set_clip_rect(rect);
                pane::show(&mut pui, state, &mut tab, &mut self.chrome, out);
                if self.pane.is_some() {
                    self.pane = Some(tab);
                    self.last_pane = tab;
                }
            } else if !inline_pane {
                // Medium/Expanded: a modal side sheet over the content.
                let mut open = self.pane.is_some();
                SideSheet::new("md3e-pane-sheet").show(&ctx, &mut open, |ui| {
                    pane::show(ui, state, &mut tab, &mut self.chrome, out);
                });
                if self.pane.is_some() {
                    self.pane = open.then_some(tab);
                    if open {
                        self.last_pane = tab;
                    }
                }
            }
        }
    }
}

/// The load request a route needs.
fn load_for(route: &Route, out: &mut Vec<Command>) {
    match route {
        Route::Home => out.push(Command::LoadHome),
        Route::Library(_) => out.push(Command::LoadLibrary),
        Route::Release(id) => out.push(Command::LoadRelease(*id)),
        Route::Artist(id) => out.push(Command::LoadArtist(*id)),
        Route::Playlist(id) => out.push(Command::LoadPlaylist(*id)),
        Route::Downloads(_) | Route::Settings => {}
    }
}

type SchemeKey = (u32, bool, SchemeVariant, Contrast);

/// Animates the whole scheme from one palette to the next (slow effects).
struct SchemeAnim {
    key: Option<SchemeKey>,
    from: ColorScheme,
    to: ColorScheme,
    k: Animated<f32>,
    cache: HashMap<SchemeKey, ColorScheme>,
}

impl Default for SchemeAnim {
    fn default() -> Self {
        let base = ColorScheme::baseline(true);
        Self {
            key: None,
            from: base,
            to: base,
            k: Animated::new(1.0, crate::tokens::Motion::expressive().slow_effects)
                .with_threshold(0.001),
            cache: HashMap::new(),
        }
    }
}

impl SchemeAnim {
    fn frame(&mut self, ctx: &Context, key: SchemeKey, spring: Spring) -> ColorScheme {
        let t = anim::now(ctx);
        if self.key != Some(key) {
            let current = ColorScheme::lerp(&self.from, &self.to, self.k.value(t));
            let to = *self
                .cache
                .entry(key)
                .or_insert_with(|| ColorScheme::from_seed(key.0, key.2, key.1, key.3));
            if self.key.is_none() {
                self.from = to;
                self.k.snap(1.0);
            } else {
                self.from = current;
                self.k.snap(0.0);
                self.k.animate_to(1.0, spring, t);
            }
            self.to = to;
            self.key = Some(key);
        }
        if !self.k.is_settled(t) {
            ctx.request_repaint();
        }
        ColorScheme::lerp(&self.from, &self.to, self.k.value(t).clamp(0.0, 1.0))
    }
}

impl Md3eUi {
    /// Opens the queue pane, in the shell and in Now Playing (scripted
    /// screenshots).
    pub fn open_queue(&mut self) {
        if self.now_playing {
            self.chrome.np_sheet = true;
        } else {
            self.pane = Some(PaneTab::Queue);
        }
    }

    /// Switches the library page's tab (scripted screenshots).
    pub fn set_library_tab(&mut self, tab: nikgob_view::LibraryTab) {
        self.pages.library_tab = tab;
    }

    /// Types into the search bar (used by scripted screenshots).
    pub fn set_search(&mut self, query: &str, out: &mut Vec<Command>) {
        self.search = query.to_owned();
        out.push(Command::Search(self.search.clone()));
    }
}

impl Md3eUi {
    /// Whether the ambient glow is on screen and wants audio levels.
    pub fn wants_levels(&self) -> bool {
        self.now_playing || self.np_anim.target() > 0.5 || self.chrome.visualizer
    }

    /// The latest bass energy (0..1) for the ambient glow.
    pub fn set_levels(&mut self, bass: f32) {
        self.pages.levels_bass = bass;
    }

    /// The OS asks for less animation (Windows "Показывать анимацию" off).
    pub fn set_system_reduced_motion(&mut self, on: bool) {
        self.system_reduced_motion = on;
    }

    /// The latest spectrum bands (0..1, low to high) for the visualizer.
    pub fn set_bands(&mut self, bands: &[f32]) {
        self.chrome.bands.clear();
        self.chrome.bands.extend_from_slice(bands);
    }
}
