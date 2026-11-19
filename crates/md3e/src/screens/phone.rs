//! What the phone's shell takes from these screens, as they are: Главная
//! and the pages a phone has no layout of its own for (a service's album,
//! a world, a mix, «Новые релизы», «Карта сети»…), their dialogs and
//! snackbars, the actions of a track's menu, the scheme made from the
//! playing cover, the lyrics, the ambient light, covers and avatars, the
//! cover's outline. Nothing here changes how the desktop window looks or
//! behaves: it only opens a way in.

use egui::{Context, Rect, Ui};
use nikgob_view::{Command, MeshKind, Route, TrackId, TrackRow, ViewState, World};

use super::pages;
use crate::{Icon, Theme, components::anim};

pub use super::now_playing::{Outline, cover_shape};
pub use pages::TrackAct;

/// The next queue entries whose covers stay loaded ahead.
const NEXT_COVERS: usize = 2;

/// One item of a track's menu: what it says, its icon, what it does.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackMenuEntry {
    pub label: String,
    pub icon: Option<Icon>,
    pub act: TrackAct,
}

/// The pages' state a phone keeps between frames, and the scheme it
/// animates from one cover to the next.
pub struct PhonePages {
    pages: pages::PageState,
    scheme: super::SchemeAnim,
    reduced_motion: bool,
}

impl Default for PhonePages {
    fn default() -> Self {
        Self::new()
    }
}

impl PhonePages {
    pub fn new() -> Self {
        Self { pages: pages::PageState::default(), scheme: super::SchemeAnim::default(), reduced_motion: false }
    }

    /// The system asks for less motion (the phone's accessibility setting).
    pub fn set_system_reduced_motion(&mut self, on: bool) {
        self.reduced_motion = on;
    }

    /// The bass level of what plays (0..1), for the glow of Главная's card.
    pub fn set_levels(&mut self, bass: f32) {
        self.pages.levels_bass = bass;
    }

    /// The start of a frame: the theme from the playing cover and the
    /// settings, what the pages read from the context (the marks, the
    /// track menu's places), the covers kept loaded.
    pub fn begin(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let prefs = &state.prefs;
        let seed = state
            .player
            .current
            .as_ref()
            .and_then(|t| t.cover.as_ref())
            .and_then(|c| c.seed.or_else(|| c.web_url().and_then(|u| crate::covers::web_seed(ctx, u))))
            .unwrap_or(prefs.fallback_seed);
        ctx.data_mut(|d| d.insert_temp(super::scheme_seed_id(), seed));
        let (dark, variant, contrast) = super::theme_key(ctx, prefs);
        let spring = anim::effective(ctx, Theme::get(ctx).motion.slow_effects);
        let scheme = self.scheme.frame(ctx, (seed, dark, variant, contrast), spring);
        let theme = Theme::new(scheme, dark).with_reduced_motion(prefs.reduced_motion || self.reduced_motion);
        if Theme::get(ctx) != theme {
            theme.install(ctx);
        }
        pages::set_marks(ctx, &state.dislikes);
        pages::set_track_menu(ctx, &state.prefs.track_menu);
        pages::set_window_can_hide(ctx, false);
        // Главная's worlds are the phone's own row (the stage is too wide).
        pages::hide_world_stage(ctx);
        pages::home::mark_phone(ctx);
        // A phone has no pointer to hover with.
        crate::components::tooltip::set_enabled(ctx, false);
        let pinned_id = egui::Id::new("md3e-pinned-revision");
        if ctx.data(|d| d.get_temp::<u64>(pinned_id)) != Some(state.revision) {
            ctx.data_mut(|d| d.insert_temp(pinned_id, state.revision));
            let pinned: Vec<String> = (state.player.current.iter())
                .chain(state.queue.next.iter().take(NEXT_COVERS).map(|e| &e.track))
                .filter_map(|t| t.cover.as_ref().map(|c| c.uri()))
                .collect();
            crate::covers::pin(ctx, pinned.iter().map(String::as_str));
        }
        let files_id = egui::Id::new("md3e-cover-files");
        let seen = ctx.data(|d| d.get_temp::<u64>(files_id));
        if seen != Some(state.cover_files) {
            ctx.data_mut(|d| d.insert_temp(files_id, state.cover_files));
            if seen.is_some() {
                crate::covers::refresh(ctx);
            }
        }
        crate::covers::set_real_covers(ctx, &state.real_covers);
        let thumbnails = crate::covers::take_thumbnails(ctx);
        if !thumbnails.is_empty() {
            out.push(Command::RealCovers(thumbnails));
        }
    }

    /// Главная, laid out for the width it is given (the page scrolls in
    /// the caller's scroll area).
    pub fn home(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
        pages::set_here(ui.ctx(), pages::Here::of(&Route::Home));
        pages::home::show(ui, state, &mut self.pages, out)
    }

    /// The desktop's page for `route`, in a scroll area of its own with
    /// `bottom` of room under it.
    pub fn page(&mut self, ui: &mut Ui, route: &Route, state: &ViewState, bottom: f32, out: &mut Vec<Command>) -> Option<Route> {
        pages::page(ui, route, state, bottom, &mut self.pages, out)
    }

    /// The pages' dialogs, pickers and windows, and the snackbars at
    /// `snack_bottom` over the bottom of `area`. Returns the page a chooser
    /// picked (which of a track's artists).
    pub fn overlays(&mut self, ctx: &Context, state: &ViewState, area: Rect, snack_bottom: f32, out: &mut Vec<Command>) -> Option<Route> {
        // The name, once, over the app, while it is still wanted. Not the
        // card on Главная.
        pages::home::name_dialog(ctx, &state.prefs, &mut self.pages, out);
        self.pages.overlays(ctx, state, area, (area, snack_bottom), out);
        super::artist_links::chooser(ctx)
    }

    /// A page asked for the player (the card of Главная).
    pub fn take_open_now_playing(&mut self) -> bool {
        std::mem::take(&mut self.pages.open_now_playing)
    }

    /// A page asked to go back.
    pub fn take_go_back(&mut self) -> bool {
        std::mem::take(&mut self.pages.go_back)
    }

    /// A page or a menu asked for another page.
    pub fn take_nav(&mut self) -> Option<Route> {
        self.pages.pending_nav.take()
    }

    /// The items of `track`'s menu, as the desktop's menu has them (the
    /// settings' places, nothing that cannot work for this track), the
    /// submenus laid out flat; without what leads back to `here`.
    pub fn track_menu(&self, ctx: &Context, track: &TrackRow, here: &Route, state: &ViewState) -> Vec<TrackMenuEntry> {
        let wish = pages::widgets::WishIndex::of(ctx, &state.downloads.wishlist);
        let rows = pages::library_track_rows(track, pages::LibraryRowsOf::of(state, track, &wish));
        let rows = pages::with_speed_row(rows, &state.speeds, pages::is_playing(state, track.id));
        let also = state.also_on.get(&track.id).map(|(r, _)| *r);
        let rows = pages::Here::of(here).trim(rows, track, also, Some);
        let rows = pages::placed(rows, &state.prefs.track_menu, |a| a.menu_item(track.has_local));
        let mut out = Vec::new();
        flatten(rows, &mut out);
        out
    }

    /// Does `act` of a track's menu. Returns the page it leads to.
    pub fn apply(&mut self, ctx: &Context, act: TrackAct, track: &TrackRow, here: &Route, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
        pages::set_here(ctx, pages::Here::of(here));
        self.pages.track_apply(ctx, act, track, state, None, out)
    }

    /// The playlist picker of the pages for `tracks`, at the middle of the
    /// screen.
    pub fn add_to_playlist(&mut self, ctx: &Context, tracks: Vec<TrackId>) {
        pages::request_tracks_to_playlist(ctx, tracks, None);
    }
}

fn flatten(rows: Vec<pages::widgets::Row<TrackAct>>, out: &mut Vec<TrackMenuEntry>) {
    use pages::widgets::Row;
    for r in rows {
        match r {
            Row::Item(act, item) => out.push(TrackMenuEntry { label: item.label, icon: item.icon, act }),
            Row::Sub(_, _, inner) => flatten(inner, out),
            Row::Divider | Row::Label(_) => {}
        }
    }
}

/// The lyrics of the playing song as the side pane draws them (its sizes,
/// karaoke, the soft edges, a tap on a line seeks), filling what is left of
/// `ui`. `bass`: the level of what plays.
pub fn lyrics(ui: &mut Ui, state: &ViewState, bass: f32, out: &mut Vec<Command>) -> bool {
    super::lyrics::show(ui, state, bass, false, out)
}

/// The ambient light of Now Playing over `rect`, as the settings ask.
pub fn ambient(ui: &Ui, rect: Rect, state: &ViewState, bass: f32) {
    let theme = Theme::get(ui.ctx());
    let mode = state.prefs.ambient;
    if mode == nikgob_view::Ambient::Off {
        return;
    }
    let playing = state.player.status == nikgob_view::Status::Playing;
    let bass = if mode == nikgob_view::Ambient::Reactive && playing { bass } else { 0.0 };
    let level = anim::fraction(ui.ctx(), egui::Id::new("phone-np-glow"), bass, theme.motion.fast_effects);
    super::ambient::paint(ui.ctx(), ui.painter(), rect, level.clamp(0.0, 1.0), playing, state.prefs.ambient_strength);
}

/// A cover (or any picture by its URI) filling `rect` with corners of
/// `radius`; a music note on the container while there is none.
pub fn paint_cover(ui: &Ui, rect: Rect, uri: Option<&str>, radius: f32) {
    crate::components::paint_image(ui, rect, uri, crate::tokens::corner_radius(radius, rect), Icon::MusicNote);
}

/// A photo cut to a Material shape, or the initials on `fill`.
pub fn paint_shape_avatar(
    ui: &Ui,
    rect: Rect,
    uri: Option<&str>,
    initials: &str,
    shape: nikgob_shapes::MaterialShape,
    fill: egui::Color32,
    on: egui::Color32,
) {
    crate::components::paint_shape_avatar(ui, rect, uri, initials, shape, fill, on);
}

/// A device's avatar as «Межсеть» draws it, `size` across at `center`.
pub fn device_avatar(ui: &Ui, center: egui::Pos2, size: f32, salt: &str, kind: MeshKind, online: bool, playing: bool) {
    pages::mesh_ui::paint_avatar(ui, center, size, salt, kind, pages::mesh_ui::Look { online, playing });
}

/// The icon of a kind of device.
pub fn device_icon(kind: MeshKind) -> Icon {
    pages::mesh_ui::kind_icon(kind)
}

/// The spring the player opens and closes on (`Prefs::np_motion`).
pub fn player_spring(motion: u8) -> nikgob_motion::Spring {
    super::np_spring(motion)
}

/// What a world's main button plays («Дослушать», «Начать», «С начала»).
pub fn world_play(w: &World) -> Command {
    pages::play_world(w)
}

/// Draws a new random outline for the playing cover.
pub fn reroll_cover_shape(ctx: &Context) {
    super::now_playing::reroll_shape(ctx);
}
