//! The player: the mini player over the navigation bar and the full one
//! it grows into (Metrolist's bottom-sheet player over our Now Playing).
//! One spring opens it, 0 the capsule … 1 the screen: a tap or a drag up
//! on the capsule, a drag down anywhere on the player (or ⌄) closes it, and
//! a finger let go hands its speed to the spring. Sideways on the capsule
//! or on the cover changes the song.
//!
//! The full player, top to bottom (Metrolist's): ⌄ in a circle, where the
//! music comes from (a link to it), where it plays in a circle; the cover
//! in its shape (or, with the lyrics on, a small cover in a head over the
//! lyrics); the title and the artists with ♥ and «Поделиться» in a
//! connected pair; the wavy progress with the times and the sound's
//! quality; previous, play and next as one connected row whose pressed
//! button grows (play turns from a pill into a rounded square while it
//! plays); and along the bottom the queue, the lyrics, shuffle, repeat and
//! the sound as connected toggles, with ⋮ at the end.

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{LoadingIndicator, Slider, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole, lerp_color};
use nikgob_view::{Command, LikeTarget, Repeat, Route, Status, TrackId, ViewState};

use crate::gesture::{self, Sprung};
use crate::{MobileEnv, text};

/// The mini player (Metrolist's modern one: a pill).
pub const MINI_H: f32 = 66.0;
/// Its side margins and the space under it.
pub const MINI_MARGIN: f32 = 12.0;
pub const MINI_GAP: f32 = 6.0;
const MINI_RADIUS: f32 = MINI_H / 2.0;
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
        paint::container(ui.painter(), rect, radii, s.surface_container, Stroke::new(1.0, s.outline.gamma_multiply(0.25)));
        let content = rect.shrink2(vec2(10.0, 8.0));
        let mut inner = ui.new_child(UiBuilder::new().max_rect(content));
        inner.set_clip_rect(rect.shrink(1.0));
        let p = &state.player;
        // The cover in a circle inside the ring of the progress; it is the
        // play button too.
        let ring = Rect::from_min_size(pos2(content.left() + dx, content.center().y - 24.0), vec2(48.0, 48.0));
        if mini_cover(&mut inner, ring, t.cover.as_ref().map(|c| c.uri()).as_deref(), p.fraction_now(), p.status) {
            out.push(Command::TogglePlay);
        }
        let like = Rect::from_min_size(pos2(content.right() - 48.0, content.center().y - 24.0), vec2(48.0, 48.0));
        let tx = ring.right() + 14.0;
        let tw = (like.left() - 10.0 - tx).max(16.0);
        let title = TypeRole::TitleSmall.style();
        let sub = TypeRole::BodySmall.style();
        let ty = content.center().y - (title.line_height + sub.line_height) * 0.5;
        text::paint_line(&inner, pos2(tx, ty), t.title.clone(), title, s.on_surface, tw);
        let quiet = s.on_surface.gamma_multiply(0.7);
        let mut sx = tx;
        if let Some(target) = &state.target {
            // Playing on another device: its icon and name first.
            phone::device_icon(target.kind).widget(true, s.primary, 12.0).paint_at(&inner, Rect::from_min_size(pos2(sx, ty + title.line_height + 2.0), vec2(12.0, 12.0)));
            sx += 16.0;
            let w = text::paint_line(&inner, pos2(sx, ty + title.line_height), format!("{} · ", target.name), sub, s.primary, tw - 16.0);
            sx += w;
        }
        text::paint_line(&inner, pos2(sx, ty + title.line_height), t.artist.clone(), sub, quiet, (tx + tw - sx).max(8.0));
        // ♥ in an outlined circle.
        if outlined_icon(&mut ui, like, Id::new("phone-mini-like"), Icon::Favorite, t.liked) {
            out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
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
        paint::container(ui.painter(), container, Radii::all(radius), lerp_color(s.surface_container, s.surface, kc), Stroke::NONE);
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
        let transport = Rect::from_min_max(pos2(col.left(), bottom.top() - 24.0 - TRANSPORT_H), pos2(col.right(), bottom.top() - 24.0));
        let progress = Rect::from_min_max(pos2(col.left(), transport.top() - 8.0 - 52.0), pos2(col.right(), transport.top() - 8.0));
        let meta = Rect::from_min_max(pos2(col.left(), progress.top() - 12.0 - 56.0), pos2(col.right(), progress.top() - 12.0));
        let stage = Rect::from_min_max(pos2(col.left(), header.bottom() + 16.0), pos2(col.right(), meta.top() - 24.0));
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

        // The header: ⌄, where the music comes from, where it plays.
        let close = Rect::from_center_size(pos2(header.left() + 18.0, header.center().y), vec2(36.0, 36.0));
        if circle_button(&mut ui, close, id.with("close"), Icon::ExpandMore, false) {
            self.close(now, state);
        }
        let where_rect = Rect::from_center_size(pos2(header.right() - 18.0, header.center().y), vec2(36.0, 36.0));
        let where_icon = state.target.as_ref().map_or(Icon::SpeakerGroup, |t| phone::device_icon(t.kind));
        if circle_button(&mut ui, where_rect, id.with("where"), where_icon, state.target.is_some()) {
            o.output = true;
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

        // The title and the artists, ♥ and «Поделиться» (beside the small
        // cover in the lyrics' head).
        let meta_rect = lerp(meta, Rect::from_min_max(pos2(small.right() + 12.0, small.top()), pos2(col.right(), small.bottom())), lyr);
        let pair = Rect::from_min_size(pos2(meta_rect.right() - PAIR_W, meta_rect.center().y - 20.0), vec2(PAIR_W, 40.0));
        let title_style = if lyr > 0.5 { TypeRole::TitleMedium.emphasized() } else { TypeRole::TitleLarge.emphasized() };
        let sub_style = if lyr > 0.5 { TypeRole::BodyMedium.style() } else { TypeRole::TitleMedium.style() };
        let tw = pair.left() - meta_rect.left() - 12.0;
        let ty = meta_rect.center().y - (title_style.line_height + 4.0 + sub_style.line_height) * 0.5;
        let tg = text::line(&ui, t.title.clone(), title_style, s.on_surface, tw);
        let tr = Rect::from_min_size(pos2(meta_rect.left(), ty), tg.size());
        let album_link = ui.interact(tr, id.with("album"), Sense::click());
        ui.painter().galley(tr.min, tg, s.on_surface);
        if album_link.clicked()
            && let Some(r) = t.release_id
        {
            o.nav = Some(Route::Release(r));
            self.close(now, state);
        }
        let quiet = s.on_surface.gamma_multiply(0.7);
        let ag = text::line(&ui, t.artist.clone(), sub_style, quiet, tw);
        let ar = Rect::from_min_size(pos2(meta_rect.left(), ty + title_style.line_height + 4.0), ag.size());
        let artist_link = ui.interact(ar, id.with("artist"), Sense::click());
        ui.painter().galley(ar.min, ag, quiet);
        if artist_link.clicked()
            && let Some(a) = t.artist_id()
        {
            o.nav = Some(Route::Artist(a));
            self.close(now, state);
        }
        let pair_items = [Toggle { icon: Icon::Favorite, on: t.liked, enabled: true }, Toggle { icon: Icon::Share, on: false, enabled: true }];
        match toggles(&mut ui, pair, id.with("pair"), &pair_items) {
            Some(0) => out.push(Command::Like(LikeTarget::Track(t.id), !t.liked)),
            Some(_) => out.push(Command::Share(nikgob_view::ShareTarget::Track(t.id))),
            None => {}
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
        let times_y = progress.top() + 36.0;
        let ts = TypeRole::LabelMedium.style();
        ui.painter().text(pos2(progress.left(), times_y), egui::Align2::LEFT_TOP, text::duration(shown), ts.font_id(), s.on_surface_variant);
        if p.duration.is_some() {
            ui.painter().text(pos2(progress.right(), times_y), egui::Align2::RIGHT_TOP, text::duration(dur), ts.font_id(), s.on_surface_variant);
        }
        if let Some(q) = p.audio_quality.clone().or_else(|| t.quality.clone()) {
            let g = text::line(&ui, q, ts, s.on_surface_variant, progress.width() * 0.5);
            ui.painter().galley(pos2(progress.center().x - g.size().x * 0.5, times_y), g, s.on_surface_variant);
        }

        // Previous, play, next: one connected row.
        match transport_row(&mut ui, transport, id.with("transport"), p.status) {
            Some(Transport::Previous) => out.push(Command::Previous),
            Some(Transport::Play) => out.push(Command::TogglePlay),
            Some(Transport::Next) => out.push(Command::Next),
            None => {}
        }

        // Along the bottom: the queue, the lyrics, shuffle, repeat, the
        // sound; ⋮ at the end.
        let has_lyrics = !state.lyrics.lines.is_empty() || state.lyrics.plain.is_some() || state.lyrics.loading;
        if !has_lyrics && self.lyrics {
            self.lyrics = false;
        }
        let repeat_icon = if p.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat };
        let row_items = [
            Toggle { icon: Icon::QueueMusic, on: false, enabled: true },
            Toggle { icon: Icon::Lyrics, on: self.lyrics, enabled: has_lyrics },
            Toggle { icon: Icon::Shuffle, on: p.shuffle, enabled: true },
            Toggle { icon: repeat_icon, on: p.repeat != Repeat::Off, enabled: true },
            Toggle { icon: Icon::Tune, on: false, enabled: true },
        ];
        let row_w = row_items.len() as f32 * (TOGGLE_W + 2.0) - 2.0;
        let row = Rect::from_min_size(pos2(bottom.left(), bottom.center().y - 20.0), vec2(row_w, 40.0));
        match toggles(&mut ui, row, id.with("row"), &row_items) {
            Some(0) => o.queue = true,
            Some(1) => self.lyrics = !self.lyrics,
            Some(2) => out.push(Command::ToggleShuffle),
            Some(3) => out.push(Command::CycleRepeat),
            Some(_) => o.sound = true,
            None => {}
        }
        let more = Rect::from_center_size(pos2(bottom.right() - 20.0, bottom.center().y), vec2(40.0, 40.0));
        if circle_button(&mut ui, more, id.with("more"), Icon::MoreVert, false) {
            o.menu = true;
        }
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

/// The mini player's cover: a 38 dp circle in a 48 dp ring of the
/// progress, dimmed with ▶ while paused, the loading indicator while it
/// waits. Tapped: true (play or pause).
fn mini_cover(ui: &mut Ui, ring: Rect, uri: Option<&str>, fraction: f32, status: Status) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let id = Id::new("phone-mini-cover");
    let resp = ui.interact(ring, id, Sense::click());
    let c = ring.center();
    let stroke = 3.0;
    let r = (ring.width() - stroke) * 0.5;
    ui.painter().circle_stroke(c, r, Stroke::new(stroke, s.on_surface.gamma_multiply(0.12)));
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        ui.painter().add(egui::Shape::line(arc(c, r, f), Stroke::new(stroke, s.primary)));
    }
    let art = Rect::from_center_size(c, vec2(38.0, 38.0));
    let press = anim::fraction(ui.ctx(), id.with("press"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    let art = art.shrink(2.0 * press);
    phone::paint_cover(ui, art, uri, art.width() * 0.5);
    match status {
        Status::Buffering => {
            ui.put(Rect::from_center_size(c, vec2(24.0, 24.0)), LoadingIndicator::new().color(s.primary).size(24.0));
        }
        Status::Playing => {}
        _ => {
            ui.painter().circle_filled(c, art.width() * 0.5, egui::Color32::from_black_alpha(90));
            Icon::PlayArrow.widget(true, egui::Color32::WHITE, 20.0).paint_at(ui, Rect::from_center_size(c, vec2(20.0, 20.0)));
        }
    }
    resp.clicked()
}

/// The points of a ring's arc from the top, clockwise, `f` of the way.
fn arc(c: egui::Pos2, r: f32, f: f32) -> Vec<egui::Pos2> {
    let n = ((64.0 * f).ceil() as usize).max(2);
    (0..=n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * f * i as f32 / n as f32;
            egui::Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

/// An icon in a thin circle (Metrolist's mini player buttons); `on` fills
/// it in the primary colour. Tapped: true.
fn outlined_icon(ui: &mut Ui, rect: Rect, id: Id, icon: Icon, on: bool) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click());
    let radii = Radii::all(rect.width() * 0.5);
    paint::container(ui.painter(), rect, radii, egui::Color32::TRANSPARENT, Stroke::new(1.0, s.on_surface.gamma_multiply(0.25)));
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);
    let color = if on { s.primary } else { s.on_surface.gamma_multiply(0.7) };
    icon.widget(on, color, 20.0).paint_at(ui, Rect::from_center_size(rect.center(), vec2(20.0, 20.0)));
    resp.clicked()
}

/// The transport row's height, a toggle's width and the ♥ pair's width.
const TRANSPORT_H: f32 = 72.0;
const TOGGLE_W: f32 = 42.0;
const PAIR_W: f32 = 2.0 * 48.0 + 2.0;

/// A 36 or 40 dp filled circle with an icon (the player's ⌄, «Где играть»
/// and ⋮); `on` in the primary colour. Tapped: true.
fn circle_button(ui: &mut Ui, rect: Rect, id: Id, icon: Icon, on: bool) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let resp = ui.interact(rect.expand(4.0), id, Sense::click());
    let radii = Radii::all(rect.width() * 0.5);
    let (fill, ink) = if on { (s.primary, s.on_primary) } else { (s.surface_container_highest, s.on_surface) };
    paint::container(ui.painter(), rect, radii, fill, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, ink, layer);
    icon.widget(on, ink, 24.0).paint_at(ui, Rect::from_center_size(rect.center(), vec2(24.0, 24.0)));
    resp.clicked()
}

/// One toggle of a connected row.
#[derive(Clone, Copy)]
struct Toggle {
    icon: Icon,
    on: bool,
    enabled: bool,
}

/// The corners of member `i` of `n` in a connected row: round at the ends
/// (and all round while on), small inside.
fn connected_radii(i: usize, n: usize, on: bool, h: f32) -> Radii {
    let full = h * 0.5;
    let inner = 4.0;
    let l = if i == 0 || on { full } else { inner };
    let r = if i + 1 == n || on { full } else { inner };
    Radii::sides(l, r)
}

/// Connected toggles 40 dp tall in `rect`, from its left: tonal while off,
/// primary while on. The one tapped.
fn toggles(ui: &mut Ui, rect: Rect, id: Id, items: &[Toggle]) -> Option<usize> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let n = items.len();
    let w = (rect.width() - 2.0 * (n as f32 - 1.0)) / n as f32;
    let rest = if theme.dark { s.secondary_container.gamma_multiply(0.5) } else { s.secondary_container };
    let mut tapped = None;
    for (i, t) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (w + 2.0), rect.top()), vec2(w, rect.height()));
        let resp = ui.interact(r, id.with(i), if t.enabled { Sense::click() } else { Sense::hover() });
        let k = anim::fraction(ui.ctx(), id.with(("on", i)), if t.on { 1.0 } else { 0.0 }, theme.motion.fast_effects);
        let radii = connected_radii(i, n, false, r.height()).lerp(connected_radii(i, n, true, r.height()), k);
        let fill = lerp_color(rest, s.primary, k);
        let ink = lerp_color(s.on_secondary_container, s.on_primary, k);
        let alpha = if t.enabled { 1.0 } else { 0.38 };
        paint::container(ui.painter(), r, radii, fill.gamma_multiply(alpha), Stroke::NONE);
        let layer = anim::fraction(ui.ctx(), id.with(("l", i)), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
        paint::state_layer(ui.painter(), r, radii, ink, layer);
        t.icon.widget(t.on, ink.gamma_multiply(alpha), 24.0).paint_at(ui, Rect::from_center_size(r.center(), vec2(24.0, 24.0)));
        if t.enabled && resp.clicked() {
            tapped = Some(i);
        }
    }
    tapped
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Transport {
    Previous,
    Play,
    Next,
}

/// How wide previous, play and next are (their weights), as one of them
/// is held: the pressed one grows and the others give way (Metrolist's).
pub(crate) fn transport_weights(pressed: Option<usize>) -> [f32; 3] {
    match pressed {
        Some(0) => [0.9, 1.3, 0.5],
        Some(1) => [0.4, 2.2, 0.4],
        Some(2) => [0.5, 1.3, 0.9],
        _ => [0.6, 1.9, 0.6],
    }
}

/// Previous, play and next as one connected row `rect` tall: the sides are
/// tonal pills, play is primary and turns from a pill into a rounded
/// square while it plays; the button held grows on a spring.
fn transport_row(ui: &mut Ui, rect: Rect, id: Id, status: Status) -> Option<Transport> {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let gap = 8.0;
    let pressed: Option<usize> = ctx.data(|d| d.get_temp(id.with("pressed")));
    let target = transport_weights(pressed);
    let weights: Vec<f32> = (0..3).map(|i| anim::f32(&ctx, id.with(("w", i)), target[i], theme.motion.fast_spatial).max(0.1)).collect();
    let sum: f32 = weights.iter().sum();
    let room = rect.width() - 2.0 * gap;
    let playing = status == Status::Playing;
    let mut x = rect.left();
    let mut now_pressed = None;
    let mut tapped = None;
    for (i, w) in weights.iter().enumerate() {
        let width = room * w / sum;
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
        x += width + gap;
        let resp = ui.interact(r, id.with(("b", i)), Sense::click());
        if resp.is_pointer_button_down_on() {
            now_pressed = Some(i);
        }
        let h = rect.height();
        let (fill, ink, radius) = if i == 1 {
            let target = if playing { 20.0 } else { h * 0.5 };
            (s.primary, s.on_primary, anim::f32(&ctx, id.with("play-r"), target, theme.motion.fast_spatial).clamp(4.0, h * 0.5))
        } else {
            (s.primary_container.gamma_multiply(0.8), s.on_primary_container, h * 0.5)
        };
        let radii = Radii::all(radius);
        paint::container(ui.painter(), r, radii, fill, Stroke::NONE);
        let layer = anim::fraction(&ctx, id.with(("l", i)), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
        paint::state_layer(ui.painter(), r, radii, ink, layer);
        let size = if i == 1 { 32.0 } else { 24.0 };
        let c = Rect::from_center_size(r.center(), vec2(size, size));
        match i {
            0 => Icon::SkipPrevious.widget(true, ink, size).paint_at(ui, c),
            2 => Icon::SkipNext.widget(true, ink, size).paint_at(ui, c),
            _ if status == Status::Buffering => {
                ui.put(c, LoadingIndicator::new().color(ink).size(size));
            }
            _ => (if playing { Icon::Pause } else { Icon::PlayArrow }).widget(true, ink, size).paint_at(ui, c),
        }
        if resp.clicked() {
            tapped = Some([Transport::Previous, Transport::Play, Transport::Next][i]);
        }
    }
    ctx.data_mut(|d| match now_pressed {
        Some(p) => {
            d.insert_temp(id.with("pressed"), p);
        }
        None => {
            d.remove::<usize>(id.with("pressed"));
        }
    });
    tapped
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pressed_transport_button_grows_and_the_others_give_way() {
        let rest = transport_weights(None);
        assert!(rest[1] > rest[0] && rest[0] == rest[2]);
        for i in 0..3 {
            let w = transport_weights(Some(i));
            assert!(w[i] > rest[i], "{i} grows");
            for j in (0..3).filter(|j| *j != i) {
                assert!(w[j] < rest[j], "{j} gives way to {i}");
            }
        }
    }

    #[test]
    fn a_connected_row_is_round_at_its_ends_and_all_round_while_on() {
        assert_eq!(connected_radii(0, 3, false, 40.0).0, [20.0, 4.0, 4.0, 20.0]);
        assert_eq!(connected_radii(1, 3, true, 40.0).0, [20.0; 4]);
    }

    #[test]
    fn the_progress_arc_starts_at_the_top() {
        let a = arc(egui::Pos2::ZERO, 10.0, 0.5);
        assert!((a[0].y + 10.0).abs() < 1e-4 && a[0].x.abs() < 1e-4);
        let last = a[a.len() - 1];
        assert!((last.y - 10.0).abs() < 1e-3, "half way is the bottom");
    }
}
