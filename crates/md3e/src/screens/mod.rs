//! The MD3E design system's screens over the shared view model.
//!
//! [`Md3eUi::frame`] draws a whole window from a [`ViewState`] and pushes
//! [`Command`]s for the runtime. It owns only presentation state: the
//! navigation stack, which panes are open, transitions.

mod ambient;
mod bg_shapes;
mod chrome;
mod common;
mod lyrics;
mod pages;
mod pane;
mod player_bar;
mod tag_editor;
mod visualizer;

pub mod now_playing;

use std::collections::HashMap;

use egui::{Context, Key, Modifiers, Rect, Ui, UiBuilder, pos2, vec2};
use nikgob_motion::{Animated, Spring};
use nikgob_view::{Command, NpSlot, Route, ThemeMode, ViewState};

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
    /// Browser-like history of whole stacks, for the mouse's back and
    /// forward buttons (and Alt+← / Alt+→).
    past: Vec<Vec<Route>>,
    future: Vec<Vec<Route>>,
    /// +1 forward, -1 back, 0 fade-through (top-level switch).
    nav_dir: f32,
    nav_anim: Animated<f32>,
    rail_expanded: bool,
    pane: Option<PaneTab>,
    last_pane: PaneTab,
    was_inline: bool,
    now_playing: bool,
    np_anim: Animated<f32>,
    /// Now Playing's content takes input (it has nearly arrived), not the
    /// moving container.
    np_live: bool,
    /// `Prefs::np_motion`, for the spring Now Playing opens with.
    np_motion: u8,
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
    /// Window fullscreen asked for with F11 (the visualizer forces it too).
    user_fullscreen: bool,
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
            past: Vec::new(),
            future: Vec::new(),
            nav_dir: 0.0,
            nav_anim: Animated::new(0.0, crate::tokens::Motion::expressive().slow_effects),
            rail_expanded: true,
            // The pane opens on demand (queue / lyrics buttons), not always.
            pane: None,
            last_pane: PaneTab::Queue,
            was_inline: true,
            now_playing: false,
            np_anim: Animated::new(0.0, crate::tokens::Motion::expressive().slow_spatial),
            np_live: false,
            np_motion: 1,
            bar_rect: Rect::NOTHING,
            content_rect: Rect::NOTHING,
            search: String::new(),
            search_focus: false,
            scheme: SchemeAnim::default(),
            pages: pages::PageState::default(),
            chrome: chrome::Chrome::default(),
            system_reduced_motion: false,
            fullscreen: false,
            user_fullscreen: false,
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
        self.remember();
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
        // A route that names a tab of «Загрузки» opens it (the rail's
        // plain «Загрузки» keeps the last one).
        if let Route::Downloads(tab) = route
            && tab != nikgob_view::DownloadsTab::Active
        {
            self.pages.set_downloads_tab(tab);
        }
        load_for(&route, out);
        self.stack.push(route);
        self.search.clear();
        self.restart_nav(ctx);
    }

    /// Up one level (the page's ← and Esc). When that is where the user
    /// came from it is a step back in the history, so forward returns.
    pub fn back(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        let parent = &self.stack[..self.stack.len() - 1];
        if self.past.last().is_some_and(|p| p == parent) {
            return self.history_back(ctx, out);
        }
        self.remember();
        self.stack.pop();
        self.nav_dir = -1.0;
        load_for(self.route(), out);
        self.restart_nav(ctx);
        true
    }

    /// Where the user was before (the mouse's back button).
    pub fn history_back(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        let Some(prev) = self.past.pop() else { return false };
        self.future.push(std::mem::replace(&mut self.stack, prev));
        self.arrive(ctx, -1.0, out);
        true
    }

    /// Back again to where history-back came from.
    pub fn history_forward(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        let Some(next) = self.future.pop() else { return false };
        self.past.push(std::mem::replace(&mut self.stack, next));
        self.arrive(ctx, 1.0, out);
        true
    }

    /// Notes the current page before going somewhere new.
    fn remember(&mut self) {
        const KEEP: usize = 50;
        if self.past.last() != Some(&self.stack) {
            self.past.push(self.stack.clone());
            if self.past.len() > KEEP {
                self.past.remove(0);
            }
        }
        self.future.clear();
    }

    fn arrive(&mut self, ctx: &Context, dir: f32, out: &mut Vec<Command>) {
        if self.stack.is_empty() {
            self.stack.push(Route::Home);
        }
        self.nav_dir = dir;
        load_for(self.route(), out);
        self.search.clear();
        self.restart_nav(ctx);
    }

    fn restart_nav(&mut self, ctx: &Context) {
        let t = anim::now(ctx);
        let spring = anim::effective(ctx, Theme::get(ctx).motion.default_spatial);
        self.nav_anim.snap(1.0);
        self.nav_anim.animate_to(0.0, spring, t);
    }

    pub fn set_now_playing(&mut self, ctx: &Context, open: bool) {
        self.now_playing = open;
        if open {
            self.chrome.np_loaded = false;
        }
        if open && !self.was_inline {
            // A modal pane sheet must not stay on top of Now Playing.
            self.pane = None;
        }
        let spring = anim::effective(ctx, np_spring(self.np_motion));
        self.np_anim
            .animate_to(if open { 1.0 } else { 0.0 }, spring, anim::now(ctx));
    }

    /// Recomputes and animates the colour scheme from the current cover.
    fn update_theme(&mut self, ctx: &Context, state: &ViewState) {
        let prefs = &state.prefs;
        let seed = state
            .player
            .current
            .as_ref()
            .and_then(|t| t.cover.as_ref())
            .and_then(|c| c.seed)
            .unwrap_or(prefs.fallback_seed);
        // The settings' samples paint other variants of the same seed.
        ctx.data_mut(|d| d.insert_temp(scheme_seed_id(), seed));
        let (dark, variant, contrast) = theme_key(ctx, prefs);
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
    pub fn frame(&mut self, root: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
        let ctx = &root.ctx().clone();
        let first_cmd = out.len();
        self.update_theme(ctx, state);
        root.spacing_mut().scroll = crate::components::scrollbars::apply(ctx);
        // The current and the next cover stay loaded while off screen.
        let pinned: Vec<String> = (state.player.current.iter())
            .chain(state.queue.next.first().map(|e| &e.track))
            .filter_map(|t| t.cover.as_ref().map(|c| c.uri()))
            .collect();
        crate::covers::pin(ctx, pinned.iter().map(String::as_str));
        self.np_motion = state.prefs.np_motion;
        if !self.chrome.np_loaded {
            self.chrome.np_loaded = true;
            self.chrome.np_slot = state.prefs.np_slot;
            self.chrome.np_saved = Some(state.prefs.np_slot);
        }
        self.chrome.np_covering = self.np_anim.value(anim::now(ctx)) > 0.01;
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
                    if compact_page(self.route()) {
                        self.content_rect = full;
                        // The page margins of the full layout.
                        let inner = full.shrink2(vec2(class.margin(), 0.0));
                        let mut pui = ui.new_child(UiBuilder::new().max_rect(inner).id_salt("compact-page"));
                        pui.set_clip_rect(full);
                        if pui.add(crate::components::IconButton::new(crate::Icon::ExpandMore)
                            .tooltip("Вернуться к плееру").id_salt("compact-settings-close")).clicked() {
                            self.navigate(ctx, Route::Home, out);
                        }
                        let route = self.route().clone();
                        // Another artist from this page stays here; the
                        // rest has no room in mini mode.
                        if let Some(next @ Route::Artist(_)) = pages::page(&mut pui, &route, state, 16.0, &mut self.pages, out) {
                            self.navigate(ctx, next, out);
                        }
                        return;
                    }
                    // Mini mode: Now Playing only.
                    self.chrome.np_covering = true;
                    now_playing::container(ui, full, 1.0);
                    now_playing::show(ui, full, state, true, &mut self.pages, &mut self.chrome, out);
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
        }
        // Snackbars and centred popups: over the content panel, or the whole
        // window while Now Playing (or the visualizer) covers it.
        let overlay_area = if self.now_playing || self.chrome.np_covering || self.chrome.visualizer {
            ctx.content_rect()
        } else {
            self.content_rect
        };
        self.pages.overlays(ctx, state, overlay_area, out);
        self.chrome.overlays(ctx, state, out);

        // Now Playing overlay, transformed out of the floating player: the
        // container grows from the pill to the window while the content,
        // laid out for the full window, scales up from the pill and fades in.
        let t = anim::now(ctx);
        let k = self.np_anim.value(t);
        if !self.np_anim.is_settled(t) {
            ctx.request_repaint();
        }
        if k > 0.001 {
            let full = ctx.content_rect();
            if size_class(full.width()) != SizeClass::Compact {
                let from = if self.bar_rect.is_positive() {
                    self.bar_rect
                } else {
                    full
                };
                let kc = k.clamp(0.0, 1.0);
                let rect = Rect::from_min_max(
                    from.min + (full.min - from.min) * kc,
                    from.max + (full.max - from.max) * kc,
                );
                // Never a cooldown: a click on the moving container turns
                // the spring around from wherever it is, keeping its speed.
                // Closing, the container (and the pill riding it) takes the
                // clicks and reopens; opening, it closes again until it has
                // nearly arrived (the half-faded content's own controls, the
                // cover say, must not swallow a click meant to turn it back).
                let closing = !self.now_playing;
                // Which layer takes the clicks only changes between presses,
                // or a press on one and the release on the other is lost.
                if !ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
                    self.np_live = !closing && k > 0.9;
                }
                let live = self.np_live && !closing;
                let mut toggle = false;
                let mut bar_out = Vec::new();
                // Areas fade in when they appear; this one animates itself.
                egui::Area::new(egui::Id::new("md3e-np-container"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(full.min)
                    .interactable(!live)
                    .fade_in(false)
                    .show(ctx, |ui| {
                        ui.set_clip_rect(full);
                        if !live && ui.interact(rect, ui.id().with("np-toggle"), egui::Sense::click()).clicked() {
                            toggle = true;
                        }
                        now_playing::container(ui, rect, kc);
                        // The bar rides the container's bottom edge and
                        // dissolves into it, so the pill never goes blank.
                        let fade = 1.0 - (kc / 0.3).clamp(0.0, 1.0);
                        if fade > 0.0 && self.bar_rect.is_positive() && state.player.current.is_some() {
                            let bar = Rect::from_center_size(
                                pos2(rect.center().x, rect.bottom() - from.height() * 0.5),
                                from.size(),
                            );
                            let mut bui = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("np-bar"));
                            bui.multiply_opacity(fade * fade);
                            let medium = size_class(full.width()) == SizeClass::Medium;
                            let bo = player_bar::show(&mut bui, bar, state, self.pane, medium, &mut self.chrome, &mut bar_out);
                            toggle |= bo.open_now_playing;
                        }
                    });
                // Only a closing pill is live; opening, it is just a picture.
                if closing {
                    out.append(&mut bar_out);
                }
                // Scaled around the point between the pill and the window
                // centre; the spring's overshoot gives it a little bounce.
                let pivot = from.center().lerp(full.center(), kc).to_vec2();
                let scale = 0.86 + 0.14 * k;
                let lift = (from.center().y - full.center().y) * (1.0 - kc) * 0.25;
                let transform = egui::emath::TSTransform::from_translation(pivot + vec2(0.0, lift))
                    * egui::emath::TSTransform::from_scaling(scale)
                    * egui::emath::TSTransform::from_translation(-pivot);
                let content = egui::Area::new(egui::Id::new("md3e-now-playing"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(full.min)
                    .interactable(live)
                    .fade_in(false)
                    .show(ctx, |ui| {
                        // Revealed inside the growing container only.
                        // (inset so the corners stay inside the container's
                        // rounding)
                        let inset = 0.3 * crate::components::toolbar::HEIGHT * 0.5 * (1.0 - kc);
                        ui.set_clip_rect((transform.inverse() * rect.shrink(inset)).intersect(full));
                        let a = ((k - 0.12) / 0.5).clamp(0.0, 1.0);
                        ui.multiply_opacity(a * a * (3.0 - 2.0 * a));
                        now_playing::show(ui, full, state, false, &mut self.pages, &mut self.chrome, out)
                    });
                if content.inner && live {
                    self.set_now_playing(ctx, false);
                }
                if toggle && !live {
                    self.set_now_playing(ctx, closing && state.player.current.is_some());
                }
                ctx.set_transform_layer(content.response.layer_id, transform);
            }
        }

        if let Some(route) = self.pages.pending_nav.take() {
            if self.pages.focus_setting.is_some() && route == Route::Settings {
                self.pane = None;
                // Keep the page width stable while resolving its scroll target.
                anim::snap_f32(ctx, egui::Id::new("pane-width"), 0.0);
            }
            if self.now_playing {
                self.set_now_playing(ctx, false);
            }
            self.search.clear();
            self.navigate(ctx, route, out);
        }

        if self.chrome.visualizer && self.chrome.viz.show(ctx, state, &self.chrome.bands) {
            self.chrome.visualizer = false;
        }
        // The visualizer takes the whole screen, not just the window.
        let want = self.chrome.visualizer || self.user_fullscreen;
        if want != self.fullscreen {
            self.fullscreen = want;
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
        // visualizer (in `shortcuts`), dialogs, sheet, Now Playing, track info,
        // search, back.
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            let compact = size_class(ctx.content_rect().width()) == SizeClass::Compact;
            if (self.now_playing || (compact && !compact_page(self.route()))) && self.chrome.np_slot != NpSlot::None {
                self.chrome.np_slot = NpSlot::None;
            } else if self.now_playing {
                self.set_now_playing(ctx, false);
            } else if self.pane == Some(PaneTab::Info) {
                // Track info is a look-up opened from a menu: Esc puts it away.
                self.pane = None;
            } else if !self.search.is_empty() {
                self.search.clear();
            } else if compact && compact_page(self.route()) {
                self.navigate(ctx, Route::Home, out);
            } else {
                self.back(ctx, out);
            }
        }
        if self.chrome.np_saved != Some(self.chrome.np_slot) {
            self.chrome.np_saved = Some(self.chrome.np_slot);
            // Preserve any other preference edited during this frame.
            if let Some(Command::SetPrefs(prefs)) = out[first_cmd..].iter_mut().rev().find(|c| matches!(c, Command::SetPrefs(_))) {
                prefs.np_slot = self.chrome.np_slot;
            } else {
                out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs {
                    np_slot: self.chrome.np_slot,
                    ..state.prefs.clone()
                })));
            }
        }
    }

    fn shortcuts(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        let now_playing_open = self.now_playing || (size_class(ctx.content_rect().width()) == SizeClass::Compact && !compact_page(self.route()));
        let mut previous = false;
        let mut go = None;
        ctx.input_mut(|i| {
            // Ctrl+1…4: the rail's destinations.
            for (n, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4].into_iter().enumerate() {
                if i.consume_key(Modifiers::COMMAND, key) {
                    go = Some(n);
                }
            }
            if i.consume_key(Modifiers::NONE, Key::F11) {
                self.user_fullscreen = !self.user_fullscreen;
            }
            if i.consume_key(Modifiers::NONE, Key::F12) {
                self.chrome.visualizer = !self.chrome.visualizer;
            }
            if self.chrome.visualizer && i.consume_key(Modifiers::NONE, Key::Escape) {
                self.chrome.visualizer = false;
            }
            if i.consume_key(Modifiers::COMMAND, Key::K) || i.consume_key(Modifiers::COMMAND, Key::F) {
                self.search_focus = true;
            }
            if !now_playing_open && i.consume_key(Modifiers::COMMAND, Key::Q) {
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
            if !self.chrome.visualizer
                && now_playing_open
                && i.consume_key(Modifiers::NONE, Key::L)
            {
                self.chrome.np_slot.toggle(NpSlot::Lyrics);
            }
            if !self.chrome.visualizer && now_playing_open && i.consume_key(Modifiers::NONE, Key::Q) {
                self.chrome.np_slot.toggle(NpSlot::Queue);
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
        if let Some(n) = go {
            self.chrome.nav = Some(Self::destinations()[n].clone());
        }
        // The mouse's side buttons (and Alt+arrows) walk the history like a
        // browser; back first closes Now Playing.
        let (mouse_back, mouse_forward) = ctx.input_mut(|i| {
            (
                i.pointer.button_clicked(egui::PointerButton::Extra1) || i.consume_key(Modifiers::ALT, Key::ArrowLeft),
                i.pointer.button_clicked(egui::PointerButton::Extra2) || i.consume_key(Modifiers::ALT, Key::ArrowRight),
            )
        });
        if !self.chrome.visualizer {
            if mouse_back {
                if self.now_playing {
                    self.set_now_playing(ctx, false);
                } else if !self.history_back(ctx, out) {
                    self.back(ctx, out);
                }
            }
            if mouse_forward && !self.now_playing {
                self.history_forward(ctx, out);
            }
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
        // "Always compact": icons only, whatever the window size.
        let compact_rail = state.prefs.rail_compact;
        let wants_expanded = self.rail_expanded && class >= SizeClass::Large && !compact_rail;
        let expansion = anim::fraction(
            &ctx,
            egui::Id::new("rail-expansion"),
            if wants_expanded { 1.0 } else { 0.0 },
            // Widths move the whole layout: no bounce, or everything wobbles.
            theme.motion.default_spatial.without_bounce(),
        );
        let rail_w = rail::width(expansion);
        let rail_slot = rail_w;
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
            theme.motion.default_spatial.without_bounce(),
        )
        .max(0.0);
        let gap = 24.0 * (pane_w / 360.0).min(1.0);
        let content = Rect::from_min_max(
            pos2(full.left() + rail_slot, full.top()),
            pos2(full.right() - pane_w - gap, full.bottom()),
        );
        self.content_rect = content;

        self.rail(ui, rail_rect, expansion, state, out);
        // Rail collapse toggle on Large+, in the rail's top space (gone
        // when the rail is always compact).
        if class >= SizeClass::Large && !compact_rail {
            // Centred over the FAB icon (x 48 collapsed, 40 expanded).
            let r = Rect::from_min_size(rail_rect.min + vec2(28.0 - 8.0 * expansion, 2.0), vec2(40.0, 40.0));
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
        let route = self.route().clone();

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
        // While Now Playing grows out of it, the overlay draws the bar.
        if (state.player.current.is_some() || !state.queue.next.is_empty()) && !self.chrome.np_covering {
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
        } else if self.chrome.np_covering && state.player.current.is_some() {
            // The overlay's layers miss a click now and then (an area's first
            // frame takes no input): the pill's spot still turns Now Playing
            // around instead of clicking the page under it.
            if ui.interact(bar, ui.id().with("bar-under-np"), egui::Sense::click()).clicked() {
                let open = !self.now_playing;
                self.set_now_playing(&ctx, open);
            }
        }

        // Supporting pane.
        let mut configure_lyrics = false;
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
                let po = pane::show(&mut pui, state, &mut tab, &mut self.chrome, self.pages.levels_bass, out);
                configure_lyrics = po.configure_lyrics;
                let closed = po.closed;
                if self.pane.is_some() {
                    self.pane = (!closed).then_some(tab);
                    self.last_pane = tab;
                }
            } else if !inline_pane {
                // Medium/Expanded: a modal side sheet over the content.
                let mut open = self.pane.is_some();
                let mut closed = false;
                SideSheet::new("md3e-pane-sheet").show(&ctx, &mut open, |ui| {
                    let po = pane::show(ui, state, &mut tab, &mut self.chrome, self.pages.levels_bass, out);
                    configure_lyrics = po.configure_lyrics;
                    closed = po.closed;
                });
                open &= !closed;
                if self.pane.is_some() {
                    self.pane = open.then_some(tab);
                    if open {
                        self.last_pane = tab;
                    }
                }
            }
        }
        if configure_lyrics {
            self.pages.open_setting(pages::SettingId::LyricsStyle);
        }
    }
}

/// The load request a route needs.
fn load_for(route: &Route, out: &mut Vec<Command>) {
    match route {
        Route::Home => out.push(Command::LoadHome),
        Route::Library(_) => out.push(Command::LoadLibrary),
        Route::Release(id) => out.push(Command::LoadRelease(*id)),
        // The runtime loads the discography with the artist (cached).
        Route::Artist(id) => out.push(Command::LoadArtist(*id)),
        Route::Playlist(id) => out.push(Command::LoadPlaylist(*id)),
        Route::Stream(page) => out.push(Command::LoadStream(page.clone())),
        Route::Downloads(_) => out.push(Command::LoadWishlist),
        Route::Settings => {}
    }
}

type SchemeKey = (u32, bool, SchemeVariant, Contrast);

/// Where [`Md3eUi`] keeps the seed the scheme is made from.
pub(crate) fn scheme_seed_id() -> egui::Id {
    egui::Id::new("md3e-scheme-seed")
}

/// Dark or light, the scheme variant and the contrast `prefs` ask for.
pub(crate) fn theme_key(ctx: &Context, prefs: &nikgob_view::Prefs) -> (bool, SchemeVariant, Contrast) {
    let dark = match prefs.theme {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => ctx.system_theme() != Some(egui::Theme::Light),
    };
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
    (dark, variant, contrast)
}

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
            self.chrome.np_slot = NpSlot::Queue;
        } else {
            self.pane = Some(PaneTab::Queue);
        }
    }

    /// Opens the side pane on the lyrics (scripted screenshots).
    pub fn open_lyrics(&mut self) {
        if self.now_playing {
            self.chrome.np_slot = NpSlot::Lyrics;
        } else {
            self.pane = Some(PaneTab::Lyrics);
        }
    }

    /// Switches the library page's tab (scripted screenshots).
    pub fn set_library_tab(&mut self, tab: nikgob_view::LibraryTab) {
        self.pages.library_tab = tab;
    }

    /// Opens Settings on a tab, scrolled to the row named `row` (scripted
    /// screenshots): `sources`, `spotify`, `youtube`, `import`, `torrents`,
    /// `appearance`, anything else = «Основные».
    pub fn open_settings_at(&mut self, row: &str) {
        use pages::SettingId as S;
        self.pages.open_setting(match row {
            "sources" | "spotify" => S::SpotifyEnabled,
            "youtube" => S::YouTubeEnabled,
            "personal" => S::SpotifyPersonal,
            "likes" => S::LikeDownload,
            "import" => S::ImportFiles,
            "torrents" => S::Prowlarr,
            "appearance" => S::Design,
            _ => S::CoverClickPause,
        });
    }

    /// Opens the personal-app fold and the cookies form of «Источники»
    /// (scripted screenshots).
    pub fn unfold_sources(&mut self) {
        self.pages.unfold_sources();
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
        self.now_playing || self.chrome.np_covering || self.np_anim.target() > 0.5 || self.chrome.visualizer || self.pane == Some(PaneTab::Lyrics)
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

impl Md3eUi {
    /// The rail's destinations, in order: Главная, Библиотека, Загрузки,
    /// then Настройки at the bottom (also Ctrl+1…4).
    fn destinations() -> [Route; 4] {
        [
            Route::Home,
            Route::Library(Default::default()),
            Route::Downloads(Default::default()),
            Route::Settings,
        ]
    }

    /// Draws the rail in `rail_rect` and handles its clicks. True when a
    /// destination was picked.
    fn rail(&mut self, ui: &mut Ui, rail_rect: Rect, expansion: f32, state: &ViewState, out: &mut Vec<Command>) -> bool {
        let ctx = ui.ctx().clone();
        let route = self.route().clone();
        let selected = match route {
            Route::Home => Some(0),
            Route::Library(_) | Route::Release(_) | Route::Artist(_) | Route::Playlist(_) => {
                Some(1)
            }
            Route::Downloads(_) => Some(2),
            Route::Settings | Route::Stream(_) => None,
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
        let picked = rail_out
            .clicked
            .or(rail_out.bottom_clicked.map(|_| 3))
            .map(|i| Self::destinations()[i].clone());
        if let Some(r) = picked.clone() {
            self.navigate(&ctx, r, out);
        }
        if let Some(fab) = &rail_out.fab {
            self.pages.add_menu(ui, fab, out);
        }
        picked.is_some()
    }
}

/// The spring Now Playing opens and closes on (`Prefs::np_motion`):
/// smooth, lively (a quick one with a little bounce) or fast.
pub(crate) fn np_spring(motion: u8) -> nikgob_motion::Spring {
    match motion {
        0 => nikgob_motion::Spring::new(0.85, 220.0),
        2 => nikgob_motion::Spring::new(0.9, 1000.0),
        _ => nikgob_motion::Spring::new(0.72, 520.0),
    }
}

/// Pages mini mode shows in place of Now Playing: settings, and the artist
/// opened from the player.
fn compact_page(route: &Route) -> bool {
    matches!(route, Route::Settings | Route::Artist(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        ctx
    }

    fn frame(ctx: &Context, ui: &mut Md3eUi, state: &mut ViewState, time: f64, width: f32, key: Option<Key>) -> egui::FullOutput {
        let events = key.into_iter().map(|key| egui::Event::Key {
            key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE,
        }).collect();
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 900.0))),
            time: Some(time), events, ..Default::default()
        }, |root| ui.frame(root, state, &mut commands));
        output.textures_delta.clear();
        for command in commands {
            if let Command::SetPrefs(prefs) = command { state.prefs = *prefs; }
        }
        output
    }

    #[test]
    fn lyrics_link_closes_overlays_and_scrolls_to_its_row_in_all_window_sizes() {
        for width in [380.0, 800.0, 1440.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = ViewState::default();
            ui.set_now_playing(&ctx, true);
            ui.pane = Some(PaneTab::Lyrics);
            frame(&ctx, &mut ui, &mut state, 0.0, width, None);
            ui.pages.open_setting(pages::SettingId::LyricsStyle);
            frame(&ctx, &mut ui, &mut state, 1.0, width, None);
            assert_eq!(ui.route(), &Route::Settings);
            assert!(!ui.now_playing);
            assert_eq!(ui.pane, None);
            assert!(ui.pages.focus_setting.is_some());
            frame(&ctx, &mut ui, &mut state, 2.0, width, None);
            assert!(ui.pages.focus_setting.is_none());
            assert_eq!(ui.route(), &Route::Settings);
            frame(&ctx, &mut ui, &mut state, 2.5, width, None);
            let output = frame(&ctx, &mut ui, &mut state, 2.6, width, None);
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == "Анимация" && (150.0..750.0).contains(&text.pos.y)
            )), "lyrics animation row was not scrolled into view at width {width}");
            if width == 380.0 {
                frame(&ctx, &mut ui, &mut state, 3.0, width, Some(Key::Escape));
                assert_eq!(ui.route(), &Route::Home);
            }
            // Opening from lyrics while Settings is already the current route
            // must still request another scroll.
            ui.navigate(&ctx, Route::Settings, &mut Vec::new());
            ui.pages.open_setting(pages::SettingId::LyricsStyle);
            frame(&ctx, &mut ui, &mut state, 4.0, width, None);
            frame(&ctx, &mut ui, &mut state, 5.0, width, None);
            assert_eq!(ui.route(), &Route::Settings);
            assert!(ui.pages.focus_setting.is_none());
        }
    }

    #[test]
    fn back_and_forward_walk_the_pages_like_a_browser() {
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut out = Vec::new();
        let album = Route::Release(nikgob_view::ReleaseId::new());
        ui.navigate(&ctx, Route::Library(nikgob_view::LibraryTab::All), &mut out);
        ui.navigate(&ctx, album.clone(), &mut out);
        ui.navigate(&ctx, Route::Settings, &mut out);
        // Back across a top-level switch, which used to forget the album.
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
        // The page's own ← from there is the same step back.
        assert!(ui.back(&ctx, &mut out));
        assert_eq!(ui.route(), &Route::Library(nikgob_view::LibraryTab::All));
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.route(), &Route::Settings);
        assert!(!ui.history_forward(&ctx, &mut out), "nothing ahead");
        // Going somewhere new drops what was ahead.
        ui.history_back(&ctx, &mut out);
        ui.navigate(&ctx, Route::Home, &mut out);
        assert!(!ui.history_forward(&ctx, &mut out));
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
    }

    #[test]
    fn slots_toggle_restore_and_escape_in_narrow_and_wide_windows() {
        for width in [380.0, 800.0, 1440.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = ViewState::default();
            ui.set_now_playing(&ctx, true);
            frame(&ctx, &mut ui, &mut state, 0.0, width, None);
            // Lyrics is available even when this track has no text.
            for (time, key, slot) in [
                (1.0, Key::L, NpSlot::Lyrics),
                (2.0, Key::L, NpSlot::None),
                (3.0, Key::Q, NpSlot::Queue),
                (4.0, Key::Q, NpSlot::None),
                (5.0, Key::Q, NpSlot::Queue),
                (6.0, Key::L, NpSlot::Lyrics),
            ] {
                frame(&ctx, &mut ui, &mut state, time, width, Some(key));
                assert_eq!(ui.chrome.np_slot, slot);
                assert_eq!(state.prefs.np_slot, slot);
            }
            for (i, slot) in [NpSlot::None, NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info].into_iter().enumerate() {
                state.prefs.np_slot = slot;
                state.prefs.reduced_motion = i % 2 == 0;
                ui.set_now_playing(&ctx, true);
                let t = 10.0 + i as f64 * 5.0;
                frame(&ctx, &mut ui, &mut state, t, width, None);
                assert_eq!(ui.chrome.np_slot, slot);
                frame(&ctx, &mut ui, &mut state, t + 1.0, width, None);
                frame(&ctx, &mut ui, &mut state, t + 2.0, width, Some(Key::Escape));
                assert_eq!(state.prefs.np_slot, NpSlot::None);
                assert_eq!(ui.now_playing, slot != NpSlot::None);
                if slot != NpSlot::None {
                    frame(&ctx, &mut ui, &mut state, t + 3.0, width, Some(Key::Escape));
                    assert!(!ui.now_playing);
                }
            }
        }
    }

    #[test]
    fn info_menu_selects_slot_and_bar_menu_keeps_the_shell_pane() {
        let mut chrome = chrome::Chrome::default();
        let track = nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: None, release_id: None, artist_id: None, disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local: true, liked: false, cover: None,
        };
        let mut out = Vec::new();
        chrome.apply(chrome::Act::Info, &track, Rect::NOTHING, chrome::Where::Np, &mut out);
        assert_eq!(chrome.np_slot, NpSlot::Info);
        assert_eq!(chrome.open_pane, None);
        chrome.apply(chrome::Act::Info, &track, Rect::NOTHING, chrome::Where::Bar, &mut out);
        assert_eq!(chrome.open_pane, Some(PaneTab::Info));
        assert_eq!(chrome.np_slot, NpSlot::Info);
    }

    #[test]
    fn content_wheel_scrolls_and_the_rest_changes_volume() {
        for width in [380.0, 1440.0] {
            for slot in [NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info] {
                let ctx = context();
                let mut ui = Md3eUi::new();
                let mut state = ViewState::default();
                state.prefs.np_slot = slot;
                ui.set_now_playing(&ctx, true);
                frame(&ctx, &mut ui, &mut state, 0.0, width, None);
                frame(&ctx, &mut ui, &mut state, 1.0, width, None);
                for (i, pos) in [pos2(width * 0.75, 400.0), pos2(20.0, 40.0)].into_iter().enumerate() {
                    let mut commands = Vec::new();
                    let mut output = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 900.0))),
                        time: Some(2.0 + i as f64),
                        events: vec![egui::Event::PointerMoved(pos), egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, -1.0),
                            phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE,
                        }], ..Default::default()
                    }, |root| ui.frame(root, &state, &mut commands));
                    output.textures_delta.clear();
                    assert_eq!(commands.iter().any(|c| matches!(c, Command::SetVolume(_))), i == 1);
                }
            }
        }
    }

    /// One frame with a pointer press or release at `pos`.
    fn pointer(ctx: &Context, ui: &mut Md3eUi, state: &ViewState, time: f64, pos: egui::Pos2, pressed: bool) {
        let events = vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE },
        ];
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1263.0, 900.0))),
            time: Some(time), events, ..Default::default()
        }, |root| ui.frame(root, state, &mut commands));
        output.textures_delta.clear();
    }

    /// The point a click on the moving Now Playing container lands on.
    fn container_point(ui: &Md3eUi, ctx: &Context, time: f64) -> egui::Pos2 {
        let k = ui.np_anim.value(time).clamp(0.0, 1.0);
        let full = ctx.content_rect();
        ui.bar_rect.center().lerp(full.center(), k)
    }

    #[test]
    fn now_playing_turns_around_mid_flight_without_a_cooldown() {
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut state = ViewState::default();
        state.player.current = Some(nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: None, release_id: None, artist_id: None, disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local: true, liked: false, cover: None,
        });
        for i in 0..3 {
            frame(&ctx, &mut ui, &mut state, i as f64 * 0.016, 1263.0, None);
        }
        // Closing: a click on the shrinking container reopens it at once.
        ui.set_now_playing(&ctx, true);
        frame(&ctx, &mut ui, &mut state, 2.0, 1263.0, None);
        ui.set_now_playing(&ctx, false);
        let t = 2.05;
        frame(&ctx, &mut ui, &mut state, t, 1263.0, None);
        let k = ui.np_anim.value(t);
        assert!(k > 0.05 && k < 0.95, "mid-close, k = {k}");
        let p = container_point(&ui, &ctx, t);
        pointer(&ctx, &mut ui, &state, t + 0.01, p, true);
        pointer(&ctx, &mut ui, &state, t + 0.02, p, false);
        assert!(ui.now_playing, "a click mid-close reopens");
        // Retargeted from where it was (with its speed), not restarted from 0.
        assert_eq!(ui.np_anim.target(), 1.0);
        assert!(ui.np_anim.value(t + 0.03) > 0.05);

        // Opening: a click on the growing container closes it again, even
        // when the press and the release straddle the moment the content
        // becomes clickable.
        ui.set_now_playing(&ctx, false);
        frame(&ctx, &mut ui, &mut state, 5.0, 1263.0, None);
        ui.set_now_playing(&ctx, true);
        let t = 5.02;
        frame(&ctx, &mut ui, &mut state, t, 1263.0, None);
        let p = container_point(&ui, &ctx, t);
        pointer(&ctx, &mut ui, &state, t + 0.01, p, true);
        pointer(&ctx, &mut ui, &state, t + 0.4, p, false);
        assert!(!ui.now_playing, "a click mid-open closes");
    }
}
