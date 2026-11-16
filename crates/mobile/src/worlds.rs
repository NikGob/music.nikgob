//! The worlds at the top of Главная on the phone. The desktop's card is a
//! stage too wide for a phone, so here they are a row of cards a finger
//! pulls sideways, each settling at the left margin with the next one
//! peeking in: the cover, the name, the facts, how much of it was heard and
//! the play of its main button. A tap opens the world.

use egui::{Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_md3e::components::anim;
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Theme, TypeRole};
use nikgob_view::{Command, Route, Status, ViewState, World};

use crate::gesture::{FLING, Sprung, rubber};
use crate::{player, text};

const H: f32 = 136.0;
const GAP: f32 = 12.0;
const RADIUS: f32 = 28.0;
/// Movement under this is still a tap.
const SLOP: f32 = 8.0;
/// A pull is sideways once it went this far, and mostly so.
const SIDEWAYS: f32 = 14.0;

/// A card's width in a row `view` wide (between the margins): one card
/// fills it, several leave the next one peeking in.
pub(crate) fn card_w(view: f32, n: usize) -> f32 {
    if n <= 1 { view.max(1.0) } else { (view * 0.84).clamp(220.0, 360.0).min(view) }
}

/// How far the row can be pulled: the last card's right edge at the right
/// margin.
pub(crate) fn max_offset(view: f32, w: f32, n: usize) -> f32 {
    (n as f32 * w + n.saturating_sub(1) as f32 * GAP - view).max(0.0)
}

/// Where a pull let go at `x` moving at `v` (dp/s, toward the later cards
/// positive) settles: a card's start (a fling goes on to the next one in
/// its direction, else the nearer), never past the ends.
pub(crate) fn settle(x: f32, v: f32, step: f32, max: f32) -> f32 {
    if step <= 0.0 {
        return 0.0;
    }
    let at = x / step;
    let i = if v > FLING {
        at.floor() + 1.0
    } else if v < -FLING {
        at.ceil() - 1.0
    } else {
        at.round()
    };
    (i.max(0.0) * step).min(max)
}

#[derive(Clone, Copy, Debug)]
struct Pull {
    origin: Pos2,
    from: f32,
    /// Decided: sideways (the row moves) or not (the page scrolls).
    sideways: Option<bool>,
}

pub(crate) struct Worlds {
    x: Sprung,
    pull: Option<Pull>,
}

impl Default for Worlds {
    fn default() -> Self {
        Self { x: Sprung::at(0.0), pull: None }
    }
}

impl Worlds {
    /// Draws the row (nothing when the worlds are off or there are none);
    /// returns the world tapped.
    pub(crate) fn show(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
        let worlds = &state.home.worlds;
        if !state.prefs.home_card_shown() || worlds.is_empty() {
            return None;
        }
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::hover());
        let view = row.width() - 32.0;
        let n = worlds.len();
        let w = card_w(view, n);
        let step = w + GAP;
        let max = max_offset(view, w, n);
        let now = anim::now(&ctx);

        let (pressed, down, origin, pos, velocity) =
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.interact_pos(), i.pointer.velocity()));
        if pressed
            && let Some(o) = origin
            && ui.clip_rect().intersect(row).contains(o)
            && n > 1
        {
            self.pull = Some(Pull { origin: o, from: self.x.value(now), sideways: None });
        }
        if let Some(mut p) = self.pull {
            let d = pos.map_or(vec2(0.0, 0.0), |q| q - p.origin);
            if down {
                if p.sideways.is_none() {
                    if d.x.abs() > SIDEWAYS && d.x.abs() > d.y.abs() * 1.4 {
                        p.sideways = Some(true);
                    } else if d.length() > SLOP {
                        p.sideways = Some(false);
                    }
                }
                if p.sideways == Some(true) {
                    let raw = p.from - d.x;
                    // Past the ends it goes on with resistance.
                    let x = if raw < 0.0 {
                        -rubber(-raw, 0.0)
                    } else if raw > max {
                        max + rubber(raw - max, 0.0)
                    } else {
                        raw
                    };
                    self.x.hold(x, now);
                    ctx.request_repaint();
                }
                self.pull = Some(p);
            } else {
                if p.sideways == Some(true) {
                    let x = self.x.value(now);
                    let v = -velocity.x;
                    self.x.release(x, v, settle(x, v, step, max), theme.motion.default_spatial, now);
                }
                self.pull = None;
            }
        }
        // A row that got fewer worlds or a narrower window stays in reach.
        if self.pull.is_none() && self.x.target() > max {
            self.x.go(settle(self.x.target(), 0.0, step, max), theme.motion.default_spatial, now);
        }
        let offset = self.x.value(now);
        if !self.x.settled(now) {
            ctx.request_repaint();
        }

        let mut nav = None;
        let clip = ui.clip_rect().intersect(row.expand2(vec2(0.0, 4.0)));
        for (i, world) in worlds.iter().enumerate() {
            let card = Rect::from_min_size(pos2(row.left() + 16.0 + i as f32 * step - offset, row.top()), vec2(w, H));
            if !clip.intersects(card) {
                continue;
            }
            if card_ui(ui, card, i, world, state, out) {
                nav = Some(Route::World(world.key.clone()));
            }
        }
        // A pull that moved the row is not a tap on a card.
        if self.pull.is_some_and(|p| p.sideways == Some(true)) { None } else { nav }
    }
}

/// One world's card; tapped: true.
fn card_ui(ui: &mut Ui, card: Rect, i: usize, world: &World, state: &ViewState, out: &mut Vec<Command>) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let id = Id::new(("phone-world", i));
    let resp = ui.interact(card, id, Sense::click());
    let radii = Radii::all(RADIUS);
    paint::container(ui.painter(), card, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(&ctx, id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), card, radii, s.on_surface, layer);

    let cover = Rect::from_min_size(card.min, vec2(H, H)).shrink(8.0);
    // The album's own cover first: a service's copy needs the web.
    let uri = world.cover.as_ref().or(world.cover_hd.as_ref()).map(|c| c.uri());
    phone::paint_cover(ui, cover, uri.as_deref(), RADIUS - 8.0);

    let x = cover.right() + 16.0;
    let right = card.right() - 16.0;
    let ts = TypeRole::TitleMedium.emphasized();
    let fs = TypeRole::BodySmall.style();
    let mut y = card.top() + 16.0;
    text::paint_line(ui, pos2(x, y), world.title.clone(), ts, s.on_surface, right - x);
    y += ts.line_height;
    if !world.facts.is_empty() {
        text::paint_line(ui, pos2(x, y), world.facts.clone(), fs, s.on_surface_variant, right - x);
    }

    // How much of it was heard, and its main button.
    let play = Rect::from_min_size(pos2(card.right() - 8.0 - 48.0, card.bottom() - 8.0 - 48.0), vec2(48.0, 48.0));
    if world.total > 0 {
        let bar = Rect::from_min_max(pos2(x, play.center().y - 2.0), pos2(play.left() - 16.0, play.center().y + 2.0));
        if bar.width() > 8.0 {
            let k = (world.heard as f32 / world.total as f32).clamp(0.0, 1.0);
            paint::container(ui.painter(), bar, Radii::all(2.0), s.secondary_container, Stroke::NONE);
            if k > 0.0 {
                let done = Rect::from_min_size(bar.min, vec2((bar.width() * k).max(4.0), bar.height()));
                paint::container(ui.painter(), done, Radii::all(2.0), s.primary, Stroke::NONE);
            }
        }
    }
    let live = world.playing && matches!(state.player.status, Status::Playing | Status::Buffering);
    let buffering = world.playing && state.player.status == Status::Buffering;
    let mut pressed_play = false;
    if !world.primary.is_empty() && player::play_button(ui, play, id.with("play"), live, buffering, 14.0, 24.0) {
        pressed_play = true;
        out.push(if world.playing { Command::TogglePlay } else { phone::world_play(world) });
    }
    resp.clicked() && !pressed_play
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_card_peeks_in_and_one_fills_the_row() {
        assert_eq!(card_w(360.0, 1), 360.0);
        let w = card_w(360.0, 3);
        assert!(w < 360.0 && w >= 220.0);
        assert_eq!(max_offset(360.0, 360.0, 1), 0.0);
        assert_eq!(max_offset(360.0, 300.0, 2), 300.0 * 2.0 + GAP - 360.0);
    }

    #[test]
    fn a_pull_settles_at_a_card_and_a_fling_goes_on_to_the_next() {
        let step = 312.0;
        let max = 2.0 * step;
        assert_eq!(settle(100.0, 0.0, step, max), 0.0);
        assert_eq!(settle(200.0, 0.0, step, max), step);
        assert_eq!(settle(40.0, 900.0, step, max), step, "a fling to the left moves on");
        assert_eq!(settle(step - 40.0, -900.0, step, max), 0.0, "and back");
        assert_eq!(settle(-30.0, -900.0, step, max), 0.0, "never before the first");
        assert_eq!(settle(max + 50.0, 900.0, step, max), max, "never past the last");
    }
}
