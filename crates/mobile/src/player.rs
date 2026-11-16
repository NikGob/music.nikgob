//! The player: the mini player over the navigation bar and the full one
//! it grows into (Metrolist's bottom-sheet player over our Now Playing).
//! One spring opens it, 0 the capsule … 1 the screen: a tap or a drag up
//! on the capsule, a drag down anywhere on the player (or ⌄) closes it, and
//! a finger let go hands its speed to the spring. Sideways on the capsule
//! or on the cover changes the song.
//!
//! The full player, top to bottom: ⌄, where the music comes from (a link
//! to it), ⋮; the cover in its shape (or, with the lyrics on, a small cover
//! in a head over the lyrics); the title and the artists with ♥; the wavy
//! progress with the times and the sound's quality; shuffle, previous, the
//! large play that turns from a circle into a square, next, repeat; and
//! along the bottom the lyrics, the queue, the sound and where it plays.

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{IconButton, LoadingIndicator, Size, Slider, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole, lerp_color};
use nikgob_view::{Command, LikeTarget, Repeat, Route, Status, TrackId, ViewState};

use crate::gesture::{self, Sprung};
use crate::{MobileEnv, text};

/// The mini player.
pub const MINI_H: f32 = 64.0;
/// Its side margins and the space under it.
pub const MINI_MARGIN: f32 = 8.0;
pub const MINI_GAP: f32 = 4.0;
const MINI_RADIUS: f32 = 20.0;
/// A sideways drag this long changes the song.
const SKIP: f32 = 64.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Up,
    Side,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    from: f32,
    axis: Option<Axis>,
    /// The drag began on the cover (a sideways one changes the song).
    on_cover: bool,
}

/// What the player asked for this frame.
#[derive(Default)]
pub struct PlayerOut {
    pub queue: bool,
    pub output: bool,
    pub menu: bool,
    pub sound: bool,
    pub nav: Option<Route>,
}

pub struct Player {
    expand: Sprung,
    drag: Option<Drag>,
    /// The lyrics in place of the large cover.
    pub lyrics: bool,
    /// The cover moving out after a swipe (-1 left, +1 right), until the
    /// next song comes in from the other side.
    swiped: f32,
    swiped_at: f64,
    track: Option<TrackId>,
}

impl Default for Player {
    fn default() -> Self {
        Self { expand: Sprung::at(0.0), drag: None, lyrics: false, swiped: 0.0, swiped_at: 0.0, track: None }
    }
}

/// The spring the player opens on.
fn spring(state: &ViewState) -> nikgob_motion::Spring {
    phone::player_spring(state.prefs.np_motion)
}

impl Player {
    pub fn expand(&self, now: f64) -> f32 {
        self.expand.value(now)
    }

    pub fn is_open(&self) -> bool {
        self.expand.target() > 0.5
    }

    /// Asks for frames while the spring moves.
    pub fn animating(&self, now: f64) -> bool {
        !self.expand.settled(now) || self.drag.is_some()
    }

    pub fn open(&mut self, now: f64, state: &ViewState) {
        self.expand.go(1.0, spring(state), now);
    }

    pub fn close(&mut self, now: f64, state: &ViewState) {
        self.expand.go(0.0, spring(state), now);
    }

    /// A finger let go at `k` moving at `v` (dp/s, toward open positive)
    /// over a way `span` long.
    fn let_go(&mut self, k: f32, v: f32, span: f32, now: f64, state: &ViewState) {
        let open = gesture::settles_open(k, v, span);
        self.expand.release(k, v / span.max(1.0), if open { 1.0 } else { 0.0 }, spring(state), now);
    }

    /// The capsule over the navigation bar, at `rect`; `span`: how far it
    /// travels to the top of the screen.
    pub fn mini(&mut self, ui: &mut Ui, rect: Rect, span: f32, state: &ViewState, out: &mut Vec<Command>) {
        let ctx = ui.ctx().clone();
        let now = anim::now(&ctx);
        let theme = Theme::get(&ctx);
        let s = theme.scheme;
        let Some(t) = state.player.current.as_ref() else { return };
        let k = self.expand.value(now).clamp(0.0, 1.0);
        let fade = 1.0 - (k / 0.25).clamp(0.0, 1.0);
        let id = Id::new("phone-mini");
        let body = ui.interact(rect, id, Sense::click_and_drag());
        // The drag: up opens, sideways changes the song.
        let mut side = None;
        if body.drag_started() {
            self.drag = Some(Drag { from: k, axis: None, on_cover: false });
        }
        if let Some(mut d) = self.drag.filter(|_| body.dragged())
            && let Some(delta) = body.total_drag_delta()
        {
            if d.axis.is_none() && delta.length() > 8.0 {
                d.axis = Some(if delta.y.abs() > delta.x.abs() { Axis::Up } else { Axis::Side });
            }
            match d.axis {
                Some(Axis::Up) => self.expand.hold((d.from - delta.y / span.max(1.0)).clamp(0.0, 1.0), now),
                Some(Axis::Side) => side = Some(delta.x),
                None => {}
            }
            self.drag = Some(d);
        }
        if body.drag_stopped()
            && let Some(d) = self.drag.take()
        {
            let vel = ctx.input(|i| i.pointer.velocity());
            match d.axis {
                Some(Axis::Up) => self.let_go(self.expand.value(now), -vel.y, span, now, state),
                Some(Axis::Side) => {
                    let dx = ctx.input(|i| i.pointer.total_drag_delta()).map_or(0.0, |d| d.x);
                    if dx < -SKIP || vel.x < -gesture::FLING {
                        out.push(Command::Next);
                    } else if dx > SKIP || vel.x > gesture::FLING {
                        out.push(Command::Previous);
                    }
                }
                None => {}
            }
        }
        if body.clicked() {
            self.open(now, state);
        }
        if fade <= 0.01 {
            return;
        }
        let dx = gesture::follow(&ctx, id.with("x"), side.map(|x| gesture::rubber(x.abs(), SKIP) * x.signum()), theme.motion.fast_spatial);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("phone-mini-ui"));
        ui.multiply_opacity(fade);
        let radii = Radii::all(MINI_RADIUS);
        paint::shadow(ui.painter(), rect, radii, nikgob_md3e::Elevation::Level2);
        paint::container(ui.painter(), rect, radii, s.surface_container_high, Stroke::NONE);
        let content = rect.shrink(8.0);
        let mut inner = ui.new_child(UiBuilder::new().max_rect(content));
        inner.set_clip_rect(content);
        let cover = Rect::from_min_size(pos2(content.left() + dx, content.top()), vec2(48.0, 48.0));
        let uri = t.cover.as_ref().map(|c| c.uri());
        phone::paint_cover(&inner, cover, uri.as_deref(), 12.0);
        let buttons_w = 44.0 * 2.0 + 4.0;
        let tx = cover.right() + 12.0;
        let tw = (content.right() - buttons_w - 8.0 - tx).max(16.0);
        let title = TypeRole::TitleSmall.style();
        let sub = TypeRole::BodySmall.style();
        let ty = content.center().y - (title.line_height + sub.line_height) * 0.5;
        text::paint_line(&inner, pos2(tx, ty), t.title.clone(), title, s.on_surface, tw);
        let mut sx = tx;
        if let Some(target) = &state.target {
            // Playing on another device: its icon and name first.
            phone::device_icon(target.kind).widget(true, s.primary, 12.0).paint_at(&inner, Rect::from_min_size(pos2(sx, ty + title.line_height + 2.0), vec2(12.0, 12.0)));
            sx += 16.0;
            let w = text::paint_line(&inner, pos2(sx, ty + title.line_height), format!("{} · ", target.name), sub, s.primary, tw - 16.0);
            sx += w;
        }
        text::paint_line(&inner, pos2(sx, ty + title.line_height), t.artist.clone(), sub, s.on_surface_variant, (tx + tw - sx).max(8.0));
        // Play and next.
        let play = Rect::from_min_size(pos2(content.right() - buttons_w, content.center().y - 22.0), vec2(44.0, 44.0));
        let p = &state.player;
        if play_button(&mut ui, play, Id::new("phone-mini-play"), p.status == Status::Playing, p.status == Status::Buffering, 14.0, 22.0) {
            out.push(Command::TogglePlay);
        }
        let next = play.translate(vec2(48.0, 0.0));
        if ui.put(next, IconButton::new(Icon::SkipNext).id_salt("phone-mini-next")).clicked() {
            out.push(Command::Next);
        }
        // The progress along the bottom edge.
        let track = Rect::from_min_max(pos2(rect.left() + 16.0, rect.bottom() - 4.0), pos2(rect.right() - 16.0, rect.bottom() - 1.5));
        paint::container(ui.painter(), track, Radii::all(2.0), s.on_surface.gamma_multiply(0.12), Stroke::NONE);
        let f = p.fraction_now();
        if f > 0.0 {
            let done = Rect::from_min_max(track.min, pos2(track.left() + track.width() * f, track.bottom()));
            paint::container(ui.painter(), done, Radii::all(2.0), s.primary, Stroke::NONE);
        }
        if p.status == Status::Playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// The full player over `screen` as far as it is open, grown from the
    /// capsule at `from`.
    pub fn full(&mut self, ui: &mut Ui, screen: Rect, from: Rect, env: &MobileEnv, state: &ViewState, out: &mut Vec<Command>) -> PlayerOut {
        let mut o = PlayerOut::default();
        let ctx = ui.ctx().clone();
        let now = anim::now(&ctx);
        let k = self.expand.value(now);
        if k <= 0.001 && !self.drag.is_some_and(|d| d.from > 0.5) {
            return o;
        }
        let theme = Theme::get(&ctx);
        let s = theme.scheme;
        let kc = k.clamp(0.0, 1.0);
        let lerp = |a: Rect, b: Rect, t: f32| Rect::from_min_max(a.min + (b.min - a.min) * t, a.max + (b.max - a.max) * t);
        let container = if from.is_positive() { lerp(from, screen, kc) } else { screen };
        let id = Id::new("phone-np");
        let mut ui = ui.new_child(UiBuilder::new().max_rect(screen).id_salt("phone-np-ui"));
        ui.set_clip_rect(container);
        let radius = MINI_RADIUS * (1.0 - kc);
        paint::container(ui.painter(), container, Radii::all(radius), lerp_color(s.surface_container_high, s.surface, kc), Stroke::NONE);
        let content_alpha = ((kc - 0.25) / 0.75).clamp(0.0, 1.0);
        if content_alpha <= 0.0 {
            return o;
        }
        let Some(t) = state.player.current.clone() else {
            self.close(now, state);
            return o;
        };
        ui.multiply_opacity(content_alpha);
        phone::ambient(&ui, screen, state, env.bass);
        // The layout, top to bottom.
        let ins = env.insets;
        let pad = 24.0;
        let col = Rect::from_min_max(pos2(screen.left() + pad, screen.top() + ins.top + 8.0), pos2(screen.right() - pad, screen.bottom() - ins.bottom - 12.0));
        let header = Rect::from_min_size(col.min, vec2(col.width(), 48.0));
        let bottom = Rect::from_min_max(pos2(col.left(), col.bottom() - 48.0), col.max);
        let transport = Rect::from_min_max(pos2(col.left(), bottom.top() - 16.0 - 72.0), pos2(col.right(), bottom.top() - 16.0));
        let progress = Rect::from_min_max(pos2(col.left(), transport.top() - 12.0 - 56.0), pos2(col.right(), transport.top() - 12.0));
        let meta = Rect::from_min_max(pos2(col.left(), progress.top() - 8.0 - 64.0), pos2(col.right(), progress.top() - 8.0));
        let stage = Rect::from_min_max(pos2(col.left(), header.bottom() + 16.0), pos2(col.right(), meta.top() - 16.0));
        let side = stage.width().min(stage.height()).max(48.0);
        let big = Rect::from_center_size(stage.center(), vec2(side, side));
        let lyr = anim::fraction(&ctx, id.with("lyrics"), if self.lyrics { 1.0 } else { 0.0 }, theme.motion.default_spatial);
        let small = Rect::from_min_size(pos2(col.left(), header.bottom() + 8.0), vec2(48.0, 48.0));

        // A drag down closes; sideways from the cover changes the song.
        let bg = ui.interact(screen, id.with("bg"), Sense::click_and_drag());
        let span = (screen.height() - from.top().min(screen.bottom())).max(screen.height() * 0.5);
        let mut side_dx = None;
        if bg.drag_started() {
            let on_cover = ctx.input(|i| i.pointer.press_origin()).is_some_and(|p| big.contains(p) && lyr < 0.5);
            self.drag = Some(Drag { from: kc, axis: None, on_cover });
        }
        if let Some(mut d) = self.drag.filter(|_| bg.dragged())
            && let Some(delta) = bg.total_drag_delta()
        {
            if d.axis.is_none() && delta.length() > 8.0 {
                d.axis = Some(if delta.y.abs() > delta.x.abs() { Axis::Up } else { Axis::Side });
            }
            match d.axis {
                Some(Axis::Up) => self.expand.hold((d.from - delta.y / span).clamp(0.0, 1.0), now),
                Some(Axis::Side) if d.on_cover => side_dx = Some(delta.x),
                _ => {}
            }
            self.drag = Some(d);
        }
        if bg.drag_stopped()
            && let Some(d) = self.drag.take()
        {
            let vel = ctx.input(|i| i.pointer.velocity());
            match d.axis {
                Some(Axis::Up) => self.let_go(self.expand.value(now), -vel.y, span, now, state),
                Some(Axis::Side) if d.on_cover => {
                    let dx = ctx.input(|i| i.pointer.total_drag_delta()).map_or(0.0, |d| d.x);
                    if dx < -SKIP * 1.25 || vel.x < -gesture::FLING {
                        out.push(Command::Next);
                        self.swiped = -1.0;
                        self.swiped_at = now;
                    } else if dx > SKIP * 1.25 || vel.x > gesture::FLING {
                        out.push(Command::Previous);
                        self.swiped = 1.0;
                        self.swiped_at = now;
                    }
                }
                _ => {}
            }
        }

        // The header.
        let close = Rect::from_min_size(header.min - vec2(12.0, 0.0), vec2(48.0, 48.0));
        if ui.put(close, IconButton::new(Icon::ExpandMore).id_salt("phone-np-close")).clicked() {
            self.close(now, state);
        }
        let more = Rect::from_min_size(pos2(header.right() - 36.0, header.top()), vec2(48.0, 48.0));
        if ui.put(more, IconButton::new(Icon::MoreVert).id_salt("phone-np-more")).clicked() {
            o.menu = true;
        }
        let p = &state.player;
        let line = p.context_line();
        let lw = header.width() - 2.0 * 48.0;
        let g = text::line(&ui, line, TypeRole::LabelLarge.style(), s.on_surface_variant, lw);
        let gr = Rect::from_center_size(header.center(), g.size());
        let link = ui.interact(gr.expand2(vec2(8.0, 12.0)), id.with("context"), Sense::click());
        ui.painter().galley(gr.min, g, s.on_surface_variant);
        if link.clicked()
            && let Some(r) = p.context_page.clone()
        {
            o.nav = Some(r);
            self.close(now, state);
        }

        // The cover: in its shape, swiped, or small over the lyrics.
        if self.track != Some(t.id) {
            if self.track.is_some() && self.swiped != 0.0 {
                anim::snap_f32(&ctx, id.with("cover-x"), -self.swiped * side * 0.6);
            }
            self.track = Some(t.id);
            self.swiped = 0.0;
        }
        // No next song came (the end of the queue): the cover comes back.
        if self.swiped != 0.0 && now - self.swiped_at > 1.2 {
            self.swiped = 0.0;
        }
        let cover_target = match side_dx {
            Some(x) => x,
            None if self.swiped != 0.0 => self.swiped * screen.width(),
            None => 0.0,
        };
        let cover_x = anim::f32(&ctx, id.with("cover-x"), cover_target, theme.motion.default_spatial);
        if side_dx.is_some() {
            anim::snap_f32(&ctx, id.with("cover-x"), cover_target);
        }
        let cover = lerp(big, small, lyr).translate(vec2(cover_x * (1.0 - lyr), 0.0));
        self.paint_cover(&ui, cover, lyr, state, env.bass);

        // The title and artists with ♥ (beside the small cover in the
        // lyrics' head).
        let meta_rect = lerp(meta, Rect::from_min_max(pos2(small.right() + 12.0, small.top()), pos2(col.right(), small.bottom())), lyr);
        let heart = Rect::from_min_size(pos2(meta_rect.right() - 44.0, meta_rect.center().y - 24.0), vec2(48.0, 48.0));
        let title_style = if lyr > 0.5 { TypeRole::TitleMedium.style() } else { TypeRole::HeadlineSmall.style() };
        let sub_style = if lyr > 0.5 { TypeRole::BodyMedium.style() } else { TypeRole::TitleMedium.style() };
        let tw = heart.left() - meta_rect.left() - 8.0;
        let ty = meta_rect.center().y - (title_style.line_height + sub_style.line_height) * 0.5;
        text::paint_line(&ui, pos2(meta_rect.left(), ty), t.title.clone(), title_style, s.on_surface, tw);
        let artist = match &t.album {
            Some(a) if lyr < 0.5 => format!("{} · {a}", t.artist),
            _ => t.artist.clone(),
        };
        let ag = text::line(&ui, artist, sub_style, s.on_surface_variant, tw);
        let ar = Rect::from_min_size(pos2(meta_rect.left(), ty + title_style.line_height), ag.size());
        let artist_link = ui.interact(ar, id.with("artist"), Sense::click());
        ui.painter().galley(ar.min, ag, s.on_surface_variant);
        if artist_link.clicked()
            && let Some(a) = t.artist_id()
        {
            o.nav = Some(Route::Artist(a));
            self.close(now, state);
        }
        if ui.put(heart, IconButton::new(Icon::Favorite).selected(t.liked).id_salt("phone-np-like")).clicked() {
            out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
        }

        // The lyrics in place of the cover.
        if lyr > 0.01 {
            let area = Rect::from_min_max(pos2(screen.left() + 8.0, small.bottom() + 12.0), pos2(screen.right() - 8.0, progress.top() - 8.0));
            let mut lui = ui.new_child(UiBuilder::new().max_rect(area).id_salt("phone-np-lyrics"));
            lui.set_clip_rect(area.intersect(container));
            lui.multiply_opacity(lyr);
            phone::lyrics(&mut lui, state, env.bass, out);
        }

        // The progress, its times and the sound.
        let playing = p.status == Status::Playing;
        let f = p.fraction_now();
        let mut sui = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(progress.min, vec2(progress.width(), 36.0))).id_salt("phone-np-seek"));
        let so = Slider::new("phone-np-seek", f).width(progress.width()).wavy(playing).show(&mut sui);
        let dur = p.duration.unwrap_or_default();
        if let Some(v) = so.committed
            && !dur.is_zero()
        {
            out.push(Command::Seek(dur.mul_f32(v.clamp(0.0, 1.0))));
        }
        let shown = so.dragging.map_or_else(|| p.position_now(), |v| dur.mul_f32(v.clamp(0.0, 1.0)));
        let times_y = progress.bottom() - 18.0;
        let ts = TypeRole::LabelMedium.style();
        ui.painter().text(pos2(progress.left(), times_y), egui::Align2::LEFT_TOP, text::duration(shown), ts.font_id(), s.on_surface_variant);
        if p.duration.is_some() {
            ui.painter().text(pos2(progress.right(), times_y), egui::Align2::RIGHT_TOP, text::duration(dur), ts.font_id(), s.on_surface_variant);
        }
        if let Some(q) = p.audio_quality.clone().or_else(|| t.quality.clone()) {
            let g = text::line(&ui, q, ts, s.on_surface_variant, progress.width() * 0.5);
            ui.painter().galley(pos2(progress.center().x - g.size().x * 0.5, times_y), g, s.on_surface_variant);
        }

        // Shuffle, previous, play, next, repeat.
        let widths = [40.0, 56.0, 72.0, 56.0, 40.0];
        let used: f32 = widths.iter().sum();
        let gap = ((transport.width() - used) / 4.0).clamp(4.0, 28.0);
        let mut x = transport.center().x - (used + gap * 4.0) * 0.5;
        let cy = transport.center().y;
        let cell = |x: f32, w: f32| Rect::from_center_size(pos2(x + w * 0.5, cy), vec2(w, w));
        let shuffle = IconButton::new(Icon::Shuffle).selected(p.shuffle).id_salt("phone-np-shuffle");
        if ui.put(cell(x, widths[0]), shuffle).clicked() {
            out.push(Command::ToggleShuffle);
        }
        x += widths[0] + gap;
        if ui.put(cell(x, widths[1]), IconButton::new(Icon::SkipPrevious).size(Size::M).id_salt("phone-np-prev")).clicked() {
            out.push(Command::Previous);
        }
        x += widths[1] + gap;
        if play_button(&mut ui, cell(x, widths[2]), id.with("play"), p.status == Status::Playing, p.status == Status::Buffering, 20.0, 36.0) {
            out.push(Command::TogglePlay);
        }
        x += widths[2] + gap;
        if ui.put(cell(x, widths[3]), IconButton::new(Icon::SkipNext).size(Size::M).id_salt("phone-np-next")).clicked() {
            out.push(Command::Next);
        }
        x += widths[3] + gap;
        let repeat_icon = if p.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat };
        if ui.put(cell(x, widths[4]), IconButton::new(repeat_icon).selected(p.repeat != Repeat::Off).id_salt("phone-np-repeat")).clicked() {
            out.push(Command::CycleRepeat);
        }

        // Along the bottom: the lyrics, the queue, the sound, the output.
        let has_lyrics = !state.lyrics.lines.is_empty() || state.lyrics.plain.is_some() || state.lyrics.loading;
        if !has_lyrics && self.lyrics {
            self.lyrics = false;
        }
        let slots = 4;
        let sw = bottom.width() / slots as f32;
        let slot = |i: usize| Rect::from_center_size(pos2(bottom.left() + sw * (i as f32 + 0.5), bottom.center().y), vec2(48.0, 48.0));
        if ui.put(slot(0), IconButton::new(Icon::Lyrics).selected(self.lyrics).enabled(has_lyrics).id_salt("phone-np-lyrics-btn")).clicked() {
            self.lyrics = !self.lyrics;
        }
        if ui.put(slot(1), IconButton::new(Icon::QueueMusic).id_salt("phone-np-queue")).clicked() {
            o.queue = true;
        }
        if ui.put(slot(2), IconButton::new(Icon::Tune).id_salt("phone-np-sound")).clicked() {
            o.sound = true;
        }
        o.output = output_slot(&mut ui, slot(3), state);
        if p.status == Status::Playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
        o
    }

    /// The playing cover in its outline; turning and swelling with the bass
    /// as the settings ask.
    fn paint_cover(&self, ui: &Ui, rect: Rect, lyr: f32, state: &ViewState, bass: f32) {
        let ctx = ui.ctx();
        let theme = Theme::get(ctx);
        let prefs = &state.prefs;
        let p = &state.player;
        let playing = p.status == Status::Playing;
        let current = p.current.as_ref();
        let uri = current.and_then(|t| t.cover.as_ref()).map(|c| c.uri());
        if lyr > 0.5 {
            phone::paint_cover(ui, rect, uri.as_deref(), 12.0);
            return;
        }
        let shape = phone::cover_shape(ctx, prefs.cover_shape, &prefs.random_shapes, current);
        let morph = anim::fraction(ctx, Id::new("phone-np-morph"), if playing || !prefs.cover_pause_morph { 1.0 } else { 0.0 }, theme.motion.default_spatial);
        let spin = if prefs.cover_spin && playing && !theme.reduced_motion {
            let id = Id::new("phone-np-spin");
            let now = anim::now(ctx);
            let (angle, last): (f32, f64) = ctx.data(|d| d.get_temp(id)).unwrap_or((0.0, now));
            let dt = (now - last).clamp(0.0, 0.1) as f32;
            let speed = std::f32::consts::TAU / prefs.cover_spin_s.max(1.0) * if prefs.cover_ccw { -1.0 } else { 1.0 };
            let angle = (angle + dt * speed).rem_euclid(std::f32::consts::TAU);
            ctx.data_mut(|d| d.insert_temp(id, (angle, now)));
            ctx.request_repaint();
            angle
        } else {
            ctx.data(|d| d.get_temp::<(f32, f64)>(Id::new("phone-np-spin"))).map_or(0.0, |(a, _)| a)
        };
        let swell = if prefs.cover_pulse && playing { 1.0 + prefs.cover_pulse_strength * 0.25 * bass } else { 1.0 };
        let shaped = Rect::from_center_size(rect.center(), rect.size() * swell);
        if shape.is_none() {
            phone::paint_cover(ui, shaped, uri.as_deref(), 28.0);
            return;
        }
        phone::Outline::new(shape, morph, shaped, spin).paint(ui, uri.as_deref());
    }
}

/// Where the music plays: the speakers, or another device's name in a pill
/// (the micro-caption of a remote).
fn output_slot(ui: &mut Ui, cell: Rect, state: &ViewState) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    match &state.target {
        None => ui.put(cell, IconButton::new(Icon::SpeakerGroup).id_salt("phone-np-output")).clicked(),
        Some(t) => {
            let style = TypeRole::LabelSmall.emphasized();
            let g = text::line(ui, t.name.clone(), style, s.on_primary_container, 96.0);
            let w = g.size().x + 16.0 + 16.0;
            let pill = Rect::from_center_size(cell.center(), vec2(w, 28.0));
            let resp = ui.interact(pill.expand2(vec2(0.0, 10.0)), Id::new("phone-np-output-pill"), Sense::click());
            paint::container(ui.painter(), pill, Radii::all(14.0), s.primary_container, Stroke::NONE);
            phone::device_icon(t.kind).widget(true, s.on_primary_container, 14.0).paint_at(ui, Rect::from_min_size(pos2(pill.left() + 8.0, pill.center().y - 7.0), vec2(14.0, 14.0)));
            ui.painter().galley(pos2(pill.left() + 26.0, pill.center().y - g.size().y * 0.5), g, s.on_primary_container);
            resp.clicked()
        }
    }
}

/// The play button that is a circle while paused and turns into a rounded
/// square while playing; a loading indicator while it waits. `square` and
/// `round`: its radii.
pub fn play_button(ui: &mut Ui, rect: Rect, id: Id, playing: bool, buffering: bool, square: f32, round: f32) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click());
    let pressed = resp.is_pointer_button_down_on();
    let target = if pressed { square * 0.7 } else if playing { square } else { round };
    let r = anim::f32(&ctx, id.with("r"), target, theme.motion.fast_spatial).max(0.0);
    let radii = Radii::all(r.min(rect.height() * 0.5));
    paint::container(ui.painter(), rect, radii, s.primary, Stroke::NONE);
    let layer = anim::fraction(&ctx, id.with("l"), if pressed { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, s.on_primary, layer);
    let icon = (rect.height() * 0.45).round();
    if buffering {
        ui.put(Rect::from_center_size(rect.center(), vec2(icon, icon)), LoadingIndicator::new().color(s.on_primary).size(icon));
    } else {
        let glyph = if playing { Icon::Pause } else { Icon::PlayArrow };
        glyph.widget(true, s.on_primary, icon).paint_at(ui, Rect::from_center_size(rect.center(), vec2(icon, icon)));
    }
    resp.clicked()
}
