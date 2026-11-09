//! The MD3E design system's screens over the shared view model.
//!
//! [`Md3eUi::frame`] draws a whole window from a [`ViewState`] and pushes
//! [`Command`]s for the runtime. It owns only presentation state: the
//! navigation stack, which panes are open, transitions.

mod ambient;
mod artist_links;
mod bg_shapes;
mod chrome;
mod common;
mod find_song;
mod lyrics;
mod lyrics_blur;
pub mod lyrics_ui;
mod pages;
mod pane;
mod player_bar;
mod sound_quality;
pub(crate) mod stage;
pub mod sweep;
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

/// How long the search text must rest before it is searched for.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(180);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PaneTab {
    #[default]
    Queue,
    Lyrics,
    Info,
}

impl PaneTab {
    /// The pane's parts in the order of its tabs.
    pub const ALL: [Self; 3] = [Self::Queue, Self::Lyrics, Self::Info];

    /// The part is turned on in Settings (none is by default).
    pub fn on(self, prefs: &nikgob_view::Prefs) -> bool {
        match self {
            Self::Queue => prefs.pane_queue,
            Self::Lyrics => prefs.pane_lyrics,
            Self::Info => prefs.pane_info,
        }
    }
}

/// A step of the history: the whole stack, the tabs and chips the page was
/// on, and how far it was scrolled.
#[derive(Clone, Debug)]
struct Place {
    stack: Vec<Route>,
    tabs: pages::Tabs,
    scroll: f32,
}

impl Place {
    /// The same page on the same tabs (the scroll aside).
    fn same(&self, other: &Self) -> bool {
        self.stack == other.stack && self.tabs == other.tabs
    }
}

/// Presentation state of the MD3E window.
pub struct Md3eUi {
    stack: Vec<Route>,
    /// Browser-like history, for the mouse's back and forward buttons (and
    /// Alt+← / Alt+→): a page comes back on its tab, scrolled where it was.
    past: Vec<Place>,
    future: Vec<Place>,
    /// The stack and tabs drawn last frame, and how far it was scrolled: a
    /// tab or chip switched on the same page is a step of the history too.
    seen: Option<Place>,
    /// +1 forward, -1 back, 0 fade-through (top-level switch).
    nav_dir: f32,
    /// «Загрузки» is the route, as the runtime was last told.
    downloads_shown: bool,
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
    /// The search text (and how far its results were scrolled) of pages
    /// left from the search results, by the stack they were on: going back
    /// there shows the results again, where they were.
    search_marks: Vec<(Vec<Route>, String, f32)>,
    /// The query last sent to the runtime, and when the text last changed:
    /// it is sent once typing pauses for [`SEARCH_DEBOUNCE`].
    search_sent: String,
    search_edited: Option<std::time::Instant>,
    scheme: SchemeAnim,
    pub(crate) pages: pages::PageState,
    chrome: chrome::Chrome,
    /// The settings' live stage (with a chrome of its own).
    stage: stage::StageUi,
    /// «Найти песню…» over everything.
    find_song: find_song::FindSongUi,
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
            seen: None,
            nav_dir: 0.0,
            downloads_shown: false,
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
            search_marks: Vec::new(),
            search_sent: String::new(),
            search_edited: None,
            scheme: SchemeAnim::default(),
            pages: pages::PageState::default(),
            chrome: chrome::Chrome::default(),
            stage: stage::StageUi::default(),
            find_song: find_song::FindSongUi::default(),
            system_reduced_motion: false,
            fullscreen: false,
            user_fullscreen: false,
        }
    }

    pub fn route(&self) -> &Route {
        self.stack.last().unwrap_or(&Route::Home)
    }

    /// Chooses a world on the card at the top of Главная (screenshots).
    pub fn choose_world(&self, ctx: &Context, key: &str) {
        pages::choose_world(ctx, key);
    }

    /// Opens a route. Top-level routes replace the stack (fade through);
    /// detail routes push (forward).
    pub fn navigate(&mut self, ctx: &Context, route: Route, out: &mut Vec<Command>) {
        if self.route() == &route {
            return;
        }
        self.mark_search(ctx, self.stack.clone());
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
        self.load_for(&route, out);
        // A page opened anew starts at its top (not where it was last time).
        if !top_level {
            self.pages.scroll_to(route.clone(), 0.0);
        }
        self.stack.push(route);
        self.search.clear();
        self.seen = Some(self.place());
        self.restart_nav(ctx);
    }

    /// Remembers the search on screen before going to a page from it.
    fn mark_search(&mut self, ctx: &Context, stack: Vec<Route>) {
        const KEEP: usize = 20;
        if self.search.trim().is_empty() {
            return;
        }
        self.search_marks.retain(|(s, ..)| *s != stack);
        self.search_marks.push((stack, self.search.clone(), pages::search_scroll(ctx)));
        if self.search_marks.len() > KEEP {
            self.search_marks.remove(0);
        }
    }

    /// Back on a stack that was left from the search: its results again
    /// (the services answer from their cache), scrolled where they were.
    fn restore_search(&mut self, ctx: &Context) {
        if let Some(i) = self.search_marks.iter().rposition(|(s, ..)| *s == self.stack) {
            let (_, text, scroll) = self.search_marks.remove(i);
            self.search = text;
            self.search_sent.clear();
            self.search_edited = None;
            pages::restore_search_scroll(ctx, scroll);
        } else {
            self.search.clear();
        }
    }

    /// Up one level (the page's ← and Esc). When that is where the user
    /// came from it is a step back in the history, so forward returns.
    pub fn back(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        let parent = &self.stack[..self.stack.len() - 1];
        if self.past.last().is_some_and(|p| p.stack == parent) {
            return self.history_back(ctx, out);
        }
        self.remember();
        self.stack.pop();
        self.nav_dir = -1.0;
        self.load_for(&self.route().clone(), out);
        self.restore_search(ctx);
        self.seen = Some(self.place());
        self.restart_nav(ctx);
        true
    }

    /// Where the user was before (the mouse's back button).
    pub fn history_back(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        let Some(prev) = self.past.pop() else { return false };
        self.future.push(self.place());
        self.arrive(ctx, prev, -1.0, out);
        true
    }

    /// Back again to where history-back came from.
    pub fn history_forward(&mut self, ctx: &Context, out: &mut Vec<Command>) -> bool {
        let Some(next) = self.future.pop() else { return false };
        self.past.push(self.place());
        self.arrive(ctx, next, 1.0, out);
        true
    }

    /// Where the user is now.
    fn place(&self) -> Place {
        Place { stack: self.stack.clone(), tabs: self.pages.tabs(), scroll: self.pages.scroll_now }
    }

    /// Notes the current page before going somewhere new.
    fn remember(&mut self) {
        let here = self.place();
        self.remember_place(here);
    }

    fn remember_place(&mut self, here: Place) {
        const KEEP: usize = 50;
        if !self.past.last().is_some_and(|p| p.same(&here)) {
            self.past.push(here);
            if self.past.len() > KEEP {
                self.past.remove(0);
            }
        }
        self.future.clear();
    }

    /// A tab or chip switched on the page since last frame: where it was
    /// is a step back (and the new tab starts at the top).
    fn note_tab_switch(&mut self) {
        let now = self.place();
        // (Settings turning to the tab of a row a link asked for is part
        // of that one step.)
        if let Some(seen) = self.seen.take()
            && seen.stack == now.stack
            && seen.tabs != now.tabs
            && self.pages.focus_setting.is_none()
        {
            self.remember_place(seen);
            self.pages.scroll_to(self.route().clone(), 0.0);
        }
        self.seen = Some(now);
    }

    fn arrive(&mut self, ctx: &Context, place: Place, dir: f32, out: &mut Vec<Command>) {
        self.stack = place.stack;
        if self.stack.is_empty() {
            self.stack.push(Route::Home);
        }
        self.pages.set_tabs(place.tabs);
        self.pages.scroll_to(self.route().clone(), place.scroll);
        self.nav_dir = dir;
        self.load_for(&self.route().clone(), out);
        self.restore_search(ctx);
        self.seen = Some(Place { scroll: place.scroll, ..self.place() });
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
            // A stream's cover is seeded once it is decoded for drawing.
            .and_then(|c| c.seed.or_else(|| c.web_url().and_then(|u| crate::covers::web_seed(ctx, u))))
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
        let theme_scope = crate::prof::scope("theme");
        self.update_theme(ctx, state);
        drop(theme_scope);
        pages::set_marks(ctx, &state.dislikes);
        crate::components::tooltip::set_enabled(ctx, state.prefs.tooltips);
        root.spacing_mut().scroll = crate::components::scrollbars::apply(ctx);
        // The current and the next cover stay loaded while off screen (set
        // again only when the state changed; kept in the context, so a new
        // window's covers are pinned at once).
        let pinned_id = egui::Id::new("md3e-pinned-revision");
        if ctx.data(|d| d.get_temp::<u64>(pinned_id)) != Some(state.revision) {
            ctx.data_mut(|d| d.insert_temp(pinned_id, state.revision));
            let pinned: Vec<String> = (state.player.current.iter())
                .chain(state.queue.next.first().map(|e| &e.track))
                .filter_map(|t| t.cover.as_ref().map(|c| c.uri()))
                .collect();
            crate::covers::pin(ctx, pinned.iter().map(String::as_str));
        }
        // Cover files rewritten by the app (an import, a retag) or by
        // anything else while the window was away: the covers held are
        // looked at again.
        let files_id = egui::Id::new("md3e-cover-files");
        let focused = ctx.input(|i| i.focused);
        let seen = ctx.data(|d| d.get_temp::<(u64, bool)>(files_id));
        if seen != Some((state.cover_files, focused)) {
            ctx.data_mut(|d| d.insert_temp(files_id, (state.cover_files, focused)));
            let came_back = focused && seen.is_some_and(|(_, was)| !was);
            if came_back || seen.is_some_and(|(n, _)| n != state.cover_files) {
                crate::covers::refresh(ctx);
            }
        }
        // Video thumbnails of stream tracks: the real art the runtime found
        // is drawn in their place, those drawn since the last frame are
        // looked up.
        crate::covers::set_real_covers(ctx, &state.real_covers);
        let thumbnails = crate::covers::take_thumbnails(ctx);
        if !thumbnails.is_empty() {
            out.push(Command::RealCovers(thumbnails));
        }
        // The next one is decoded and uploaded at the sizes the current one
        // is drawn at, while nothing waits for it.
        if let (Some(current), Some(next)) = (state.player.current.as_ref(), state.queue.next.first()) {
            let uri = |t: &nikgob_view::TrackRow| t.cover.as_ref().map(nikgob_view::Cover::uri);
            if let (Some(c), Some(n)) = (uri(current), uri(&next.track)) {
                crate::covers::prefetch_like(ctx, &c, &n);
            }
        }
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
        // Now Playing open all the way hides the shell under its opaque
        // surface: the pages are not laid out for nothing (with the side
        // pane's lyrics they cost more than Now Playing itself).
        let covered = self.now_playing && self.np_anim.value(anim::now(ctx)) >= 0.999;
        // Shown again only if the page lays out and asks for it.
        self.stage.visible = false;

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
                if covered {
                    self.flush_search(ctx, out);
                    return;
                }
                let _p = crate::prof::scope("layout");
                self.layout(ui, full, class, state, out);
            });
        let overlays_scope = crate::prof::scope("overlays");

        // Requests pages made this frame.
        self.note_tab_switch();
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
        // Snackbars: above the floating player; on Now Playing where it
        // leaves room (the place it chose last frame).
        let np_spot = (self.now_playing || self.chrome.np_covering)
            .then(|| ctx.data(|d| d.get_temp::<(Rect, f32)>(egui::Id::new(now_playing::SNACK_SPOT))))
            .flatten();
        let snack = np_spot.unwrap_or((overlay_area, 64.0 + 16.0 + 16.0));
        self.pages.overlays(ctx, state, overlay_area, snack, out);
        self.chrome.overlays(ctx, state, out);
        self.find_song.show(ctx, state, out);
        lyrics_ui::overlays(ctx, state, out);
        // «К исполнителю» of a track with several artists: which one.
        if let Some(route) = artist_links::chooser(ctx) {
            self.chrome.nav = Some(route);
        }

        drop(overlays_scope);
        let _np_scope = crate::prof::scope("np");
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

        if let Some(id) = self.chrome.setting.take() {
            self.pages.open_setting(id);
        }
        // «Настройки текста» and the «Текст песни» page, asked under the
        // lyrics.
        match lyrics_ui::take_request(ctx) {
            Some(lyrics_ui::Request::Settings) => self.pages.open_setting_group(lyrics_ui::SETTINGS),
            Some(lyrics_ui::Request::Page) => self.pages.pending_nav = Some(Route::Lyrics),
            None => {}
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
            self.search_sent = q.clone();
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
                out.push(Command::SetPref(nikgob_view::PrefPatch::NpSlot(self.chrome.np_slot)));
            }
        }
    }

    fn shortcuts(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        // The lyrics editor takes Space, Enter and the arrows while it is up.
        let typing = (ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input()) || lyrics_ui::takes_keys(ctx);
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
            if !now_playing_open && PaneTab::Queue.on(&state.prefs) && i.consume_key(Modifiers::COMMAND, Key::Q) {
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
            // The developer profile: the playing track from YouTube Music.
            if nikgob_view::dev::profile() && state.player.current.is_some() && i.consume_key(Modifiers::NONE, Key::Y) {
                out.push(Command::PlayFromYoutube);
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
                i.pointer.button_released(egui::PointerButton::Extra1) || i.consume_key(Modifiers::ALT, Key::ArrowLeft),
                i.pointer.button_released(egui::PointerButton::Extra2) || i.consume_key(Modifiers::ALT, Key::ArrowRight),
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

        // Supporting pane (inline on Large/XL). A part turned off in
        // Settings closes, and the pane comes back on a part that is on.
        if self.pane.is_some_and(|t| !t.on(&state.prefs)) {
            self.pane = None;
        }
        if let Some(t) = PaneTab::ALL.into_iter().find(|t| t.on(&state.prefs))
            && !self.last_pane.on(&state.prefs)
        {
            self.last_pane = t;
        }
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

        if std::mem::take(&mut self.pages.focus_search) {
            self.search_focus = true;
        }
        if pages::search_bar(ui, search_rect, &mut self.search, &mut self.search_focus, state.prefs.prowlarr_configured, out) {
            self.search_edited = Some(std::time::Instant::now());
        }
        self.flush_search(&ctx, out);

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
        let _page_scope = crate::prof::scope("layout.page");
        if self.search.trim().is_empty() {
            nav = pages::page(&mut page_ui, &route, state, bottom_inset, &mut self.pages, out);
        } else {
            nav = pages::search_results(&mut page_ui, &self.search, state, bottom_inset, &mut self.pages, out).or(nav);
        }
        // The settings' live stage, when the page asked for it: in a layer
        // just over the page's, under the pane, menus, dialogs and snackbars.
        self.stage.draw(
            &ctx,
            page_ui.layer_id(),
            page_ui.clip_rect(),
            page_ui.opacity(),
            state,
            (self.pages.levels_bass, &self.chrome.bands),
        );
        if let Some(r) = nav {
            self.navigate(&ctx, r, out);
        }

        drop(_page_scope);
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
            if bo.open_now_playing || std::mem::take(&mut self.pages.open_now_playing) {
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
            self.pages.open_setting_group(lyrics_ui::SETTINGS);
        }
    }
}

impl Md3eUi {
    /// The load request a route needs, and whether «Загрузки» is shown
    /// (when that changes).
    fn load_for(&mut self, route: &Route, out: &mut Vec<Command>) {
        let downloads = matches!(route, Route::Downloads(_));
        if downloads != self.downloads_shown {
            self.downloads_shown = downloads;
            out.push(Command::DownloadsShown(downloads));
        }
        load_for(route, out);
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
        Route::World(key) => out.push(Command::LoadWorld(key.clone())),
        // Of what is on screen already (Главная, the world's page); a world
        // left meanwhile is loaded by the page itself.
        Route::Mix { .. } | Route::NewReleases => {}
        Route::Downloads(_) => out.push(Command::LoadWishlist),
        Route::Settings | Route::Lyrics => {}
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

    /// Types into the library's filter (scripted screenshots).
    pub fn set_library_filter(&mut self, text: &str) {
        self.pages.set_library_filter(text);
    }

    /// The library as a list (true) or a grid (scripted screenshots).
    pub fn set_library_list(ctx: &egui::Context, list: bool) {
        pages::set_library_list(ctx, list);
    }

    /// Opens Settings on a tab, scrolled to the row named `row` (scripted
    /// screenshots): `sources`, `spotify`, `youtube`, `import`, `torrents`,
    /// `appearance`, anything else the controls of the track screen.
    pub fn open_settings_at(&mut self, row: &str) {
        use pages::SettingId as S;
        // As «Настройки текста…» under the lyrics opens it: the whole group.
        if row == "lyrics-group" {
            self.pages.open_setting_group(lyrics_ui::SETTINGS);
            return;
        }
        self.pages.open_setting(match row {
            "sources" | "spotify" => S::SpotifyEnabled,
            "youtube" => S::YouTubeEnabled,
            "deezer" => S::DeezerEnabled,
            "soundcloud" => S::SoundCloudEnabled,
            "personal" => S::SpotifyPersonal,
            "soloist" => S::SpotifySoloist,
            "sound" => S::Gapless,
            "likes" => S::LikeDownload,
            "import" => S::ImportFiles,
            "torrents" => S::Prowlarr,
            "download-sources" => S::DownloadSource(nikgob_view::SourceKey::Torrents),
            "soulseek" => S::Soulseek,
            "graph" => S::GraphUse,
            "no-repeat" => S::RadioNoRepeat,
            "random" => S::RadioExplore,
            "unheard" => S::RadioUnheard,
            "recs" => S::RadioNewShare,
            "autoplay" => S::Autoplay,
            "mixes" => S::MixesProfiles,
            "dislikes" => S::DislikeButton,
            "blacklist" => S::DislikedArtists,
            "stream-analysis" => S::StreamAnalysis,
            "appearance" => S::Theme,
            "pane" => S::PaneQueue,
            "lyrics" => S::LyricsModels,
            "lyrics-timing" => S::LyricsGenius,
            "lyrics-ui" => S::LyricsUi,
            "lyrics-highlight" => S::LyricsWordsAll,
            "lyrics-check" => S::LyricsCheck,
            "lyrics-lead" => S::LyricsLead,
            "lyrics-streams" => S::LyricsStreamKeep,
            _ => S::CoverClickPause,
        });
    }

    /// Opens the cookies form of «Сервисы»
    /// (scripted screenshots).
    pub fn unfold_sources(&mut self) {
        self.pages.unfold_sources();
    }

    /// Types into the search bar (used by scripted screenshots); sent like
    /// typed text, once typing pauses.
    pub fn set_search(&mut self, query: &str, _out: &mut Vec<Command>) {
        self.search = query.to_owned();
        self.search_edited = Some(std::time::Instant::now());
    }

    /// Sends the search text once it has not changed for
    /// [`SEARCH_DEBOUNCE`] (clearing it is sent at once), so a burst of
    /// keys is one search, not one per key.
    fn flush_search(&mut self, ctx: &egui::Context, out: &mut Vec<Command>) {
        if self.search == self.search_sent {
            return;
        }
        let waited = self.search_edited.map_or(SEARCH_DEBOUNCE, |t| t.elapsed());
        if self.search.trim().is_empty() || waited >= SEARCH_DEBOUNCE {
            self.search_sent = self.search.clone();
            out.push(Command::Search(self.search.clone()));
        } else {
            ctx.request_repaint_after(SEARCH_DEBOUNCE - waited);
        }
    }
}

impl Md3eUi {
    /// Whether the ambient glow is on screen and wants audio levels (Now
    /// Playing, the visualizer, the pane's lyrics, the settings' stage).
    pub fn wants_levels(&self) -> bool {
        self.now_playing
            || self.chrome.np_covering
            || self.np_anim.target() > 0.5
            || self.chrome.visualizer
            || self.pane == Some(PaneTab::Lyrics)
            || self.stage.visible
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
        let of = |r: &Route| match r {
            Route::Home | Route::World(_) | Route::Mix { .. } | Route::NewReleases => Some(0),
            Route::Library(_) | Route::Release(_) | Route::Artist(_) | Route::Playlist(_) => {
                Some(1)
            }
            Route::Downloads(_) => Some(2),
            Route::Settings | Route::Stream(_) | Route::Lyrics => None,
        };
        // A service's page belongs where it was opened from (the search
        // over Главная, an artist of the library…).
        let selected = self
            .stack
            .iter()
            .rev()
            .find(|r| !matches!(r, Route::Stream(_)))
            .filter(|_| matches!(self.route(), Route::Stream(_)))
            .map_or_else(|| of(self.route()), of);
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
                    bottom_selected: (*self.route() == Route::Settings).then_some(0),
                }
                .show(ui)
            })
            .inner;
        let picked = rail_out
            .clicked
            .or(rail_out.bottom_clicked.map(|_| 3))
            .map(|i| Self::destinations()[i].clone());
        if let Some(r) = picked.clone() {
            // The page the search was typed over: its destination leaves the
            // results for it too.
            if self.route() == &r {
                self.search.clear();
            }
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
            match command {
                Command::SetPrefs(prefs) => state.prefs = *prefs,
                Command::SetPref(patch) => patch.apply(&mut state.prefs),
                _ => {}
            }
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

    /// «Настройки текста…» lands on the group of the lyrics settings and
    /// tints all of it (not one row, not the groups around it), then the
    /// tint goes.
    #[test]
    fn lyrics_settings_link_tints_its_whole_group_then_lets_go() {
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut state = ViewState::default();
        frame(&ctx, &mut ui, &mut state, 0.0, 1280.0, None);
        ui.pages.open_setting_group(lyrics_ui::SETTINGS);
        for t in [1.0, 1.05, 1.1, 1.2, 1.4] {
            frame(&ctx, &mut ui, &mut state, t, 1280.0, None);
        }
        let tint = Theme::get(&ctx).scheme.primary;
        let tinted = |output: &egui::FullOutput| -> Vec<Rect> {
            output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(r) if r.fill.a() > 0 && r.fill.r().abs_diff((tint.r() as f32 * r.fill.a() as f32 / 255.0) as u8) <= 2
                    && r.fill.g().abs_diff((tint.g() as f32 * r.fill.a() as f32 / 255.0) as u8) <= 2
                    && r.rect.height() > 120.0 => Some(r.rect),
                _ => None,
            }).collect()
        };
        let text_at = |output: &egui::FullOutput, label: &str| output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos),
            _ => None,
        });
        let output = frame(&ctx, &mut ui, &mut state, 1.6, 1280.0, None);
        assert_eq!(ui.route(), &Route::Settings);
        let rects = tinted(&output);
        assert_eq!(rects.len(), 1, "one tinted group: {rects:?}");
        // «Где делать подсветку» is a developer's row now: the link lands
        // on the lyrics' own look.
        for label in ["Вид текста", "Анимация", "Размер текста"] {
            let at = text_at(&output, label).unwrap_or_else(|| panic!("{label} not drawn"));
            assert!(rects[0].contains(at + vec2(4.0, 4.0)), "{label} outside the tint");
            assert!((0.0..900.0).contains(&at.y), "{label} not scrolled into view");
        }
        for label in ["Поиск текста", "Звук", "Для разработчика"] {
            if let Some(at) = text_at(&output, label) {
                assert!(!rects[0].contains(at + vec2(4.0, 4.0)), "{label} inside the tint");
            }
        }
        frame(&ctx, &mut ui, &mut state, 4.0, 1280.0, None);
        let output = frame(&ctx, &mut ui, &mut state, 4.1, 1280.0, None);
        assert!(tinted(&output).is_empty());
        // A row link still glows its row only.
        ui.pages.open_setting(pages::SettingId::LyricsUi);
        assert!(!ui.pages.focus_group);
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
    fn history_brings_a_page_back_on_its_tab_and_where_it_was_scrolled() {
        use nikgob_view::LibraryTab;
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut out = Vec::new();
        ui.navigate(&ctx, Route::Library(LibraryTab::All), &mut out);
        ui.note_tab_switch();
        // «Альбомы», scrolled down, then an album from there.
        ui.pages.library_tab = LibraryTab::Albums;
        ui.note_tab_switch();
        assert_eq!(ui.pages.scroll_target(), Some(0.0), "a new tab starts at the top");
        ui.pages.scroll_now = 640.0;
        ui.note_tab_switch();
        let album = Route::Release(nikgob_view::ReleaseId::new());
        ui.navigate(&ctx, album.clone(), &mut out);
        assert_eq!(ui.pages.scroll_target(), Some(0.0), "an album opens at its top");
        ui.pages.scroll_now = 200.0;

        // Back: the albums tab, scrolled where it was.
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.route(), &Route::Library(LibraryTab::All));
        assert_eq!(ui.pages.library_tab, LibraryTab::Albums);
        assert_eq!(ui.pages.scroll_target(), Some(640.0));
        ui.note_tab_switch();
        // Back again: the tab before it; forward twice: the album as it was.
        assert!(ui.history_back(&ctx, &mut out));
        assert_eq!(ui.pages.library_tab, LibraryTab::All);
        ui.note_tab_switch();
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.pages.library_tab, LibraryTab::Albums);
        assert!(ui.history_forward(&ctx, &mut out));
        assert_eq!(ui.route(), &album);
        assert_eq!(ui.pages.scroll_target(), Some(200.0));
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
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local: true, liked: false, cover: None, clip: false,
        };
        let mut out = Vec::new();
        let (ctx, state) = (egui::Context::default(), ViewState::default());
        let info = crate::screens::pages::TrackAct::Info;
        chrome.apply_track(&ctx, info, &track, &state, Rect::NOTHING, chrome::Where::Np, &mut out);
        assert_eq!(chrome.np_slot, NpSlot::Info);
        assert_eq!(chrome.open_pane, None);
        chrome.apply_track(&ctx, info, &track, &state, Rect::NOTHING, chrome::Where::Bar, &mut out);
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

    /// The settings' stage: drawn where the page asks for it, with the
    /// placeholder playing; it sends nothing, leaves the real Now Playing's
    /// springs alone, and wants levels only while it is up.
    #[test]
    fn the_stage_is_a_picture_of_its_own() {
        use stage::{Scene, Script, StageRequest};
        let ctx = context();
        let mut ui = Md3eUi::new();
        let mut state = ViewState::default();
        state.prefs.np_volume = false;
        state.prefs.np_volume_pop = true;
        state.prefs.dbl_click_shape = 2;
        state.prefs.cover_click_pause = true;
        ui.navigate(&ctx, Route::Settings, &mut Vec::new());
        frame(&ctx, &mut ui, &mut state, 0.0, 1280.0, None);
        frame(&ctx, &mut ui, &mut state, 1.0, 1280.0, None);
        assert!(!ui.wants_levels());
        let rect = Rect::from_min_size(pos2(640.0, 160.0), vec2(560.0, 350.0));
        let run = |ui: &mut Md3eUi, state: &ViewState, t: f64, req: Option<StageRequest>| {
            if let Some(r) = req {
                stage::request(&ctx, r);
            }
            let mut commands = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
                time: Some(t),
                ..Default::default()
            }, |root| ui.frame(root, state, &mut commands));
            output.textures_delta.clear();
            (commands, output)
        };
        let shows = [
            (Scene::Track, Script::None),
            (Scene::Track, Script::DoubleTap),
            (Scene::Track, Script::PausePlay),
            (Scene::Track, Script::Volume),
            (Scene::Lyrics, Script::LyricsReturn),
            (Scene::Lyrics, Script::Interlude),
            (Scene::Home, Script::None),
            (Scene::Album, Script::None),
            (Scene::Track, Script::Open),
        ];
        let mut t = 2.0;
        let mut title_seen = false;
        for (scene, script) in shows {
            for _ in 0..12 {
                t += 0.25;
                let mut req = StageRequest::new(rect, scene, state.prefs.clone());
                req.script = script;
                req.parts = vec![stage::Part::Cover];
                req.outline = 1.0;
                let (commands, output) = run(&mut ui, &state, t, Some(req));
                assert!(ui.wants_levels(), "{scene:?}: levels while the stage is up");
                for c in &commands {
                    assert!(!matches!(c,
                        Command::TogglePlay | Command::Seek(_) | Command::SetVolume(_) | Command::ToggleMute
                        | Command::Next | Command::Previous | Command::Like(..) | Command::Dislike { .. }
                        | Command::SetPref(nikgob_view::PrefPatch::CoverShape(_))
                    ), "{scene:?} {script:?}: the stage sent {c:?}");
                }
                if scene == Scene::Track && script == Script::None {
                    title_seen |= output.shapes.iter().any(|shape| matches!(&shape.shape,
                        egui::Shape::Text(text) if text.galley.text() == "Название песни" && rect.contains(text.pos)));
                }
            }
        }
        assert!(title_seen, "the placeholder plays on the stage, inside its rect");
        // The real screen's springs were never touched; the copy has its own.
        assert!(ctx.data(|d| d.get_temp::<Animated<f32>>(egui::Id::new("np-mega"))).is_none());
        assert!(ctx.data(|d| d.get_temp::<(NpSlot, Animated<[f32; 4]>)>(egui::Id::new("np-content-fade"))).is_none());
        let copy = stage::Stage::copy(1.0, false, (0.0, Vec::new()));
        assert!(ctx.data(|d| d.get_temp::<Animated<f32>>(copy.id("np-mega"))).is_some());
        // No request: no stage, no levels.
        let (_, output) = run(&mut ui, &state, t + 1.0, None);
        assert!(!ui.wants_levels());
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "Название песни")));
    }

    /// The stage stands right of the rows in a 1280 window (the rows at
    /// most 600 wide), as a strip over them at 800, and not at all at 500.
    #[test]
    fn the_stage_stands_right_wide_on_top_narrow_and_not_in_mini_mode() {
        for width in [1280.0, 800.0, 500.0] {
            let ctx = context();
            let mut ui = Md3eUi::new();
            let mut state = sweep::survey_state();
            state.prefs.settings_deep = true;
            ui.pages.open_setting(pages::SettingId::Theme);
            for i in 0..10 {
                frame(&ctx, &mut ui, &mut state, f64::from(i) * 0.2, width, None);
            }
            let rows = pages::settings_help::drawn_rows(&ctx).1;
            assert!(!rows.is_empty(), "{width}: the settings rows");
            // The rows' own extent (the clip is the whole scroll area).
            let column = rows.iter().fold(rows[0].rect, |r, row| r.union(row.rect));
            match ui.stage.rect {
                Some(s) if width > 1000.0 => {
                    assert!(column.right() < s.left(), "{width}: the stage on the right of {column:?}: {s:?}");
                    assert!(column.width() <= 600.5, "{width}: rows {}", column.width());
                    assert!(s.width() > 300.0);
                }
                Some(s) if width > 600.0 => {
                    assert!(column.top() >= s.bottom(), "{width}: a strip over the rows {column:?}: {s:?}");
                    assert!(s.width() > s.height() * 1.5, "{width}: a strip {s:?}");
                }
                None => assert!(width < 600.0, "{width}: no stage"),
                Some(s) => panic!("{width}: a stage in mini mode {s:?}"),
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
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local: true, liked: false, cover: None, clip: false,
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
