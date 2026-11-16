//! The track row of the phone (Metrolist's song item): 64 dp with a 48 dp
//! cover (56 dp with the track's number on an album), the title over the
//! artist and the length («Артист • 3:42»), a filled mark before them when
//! the song has a file here, and one ⋮ at the end. A tap plays; a long
//! press or ⋮ opens the track's menu; a swipe to the right plays it next
//! (the queue's rows swipe left to leave). On a page the rows stand as one
//! grouped list (Metrolist's): cards 3 dp apart on the surface container,
//! the group's ends round. The playing row is a pill on the primary
//! container, its cover a circle with live bars over it.

use egui::{Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{Equalizer, IconButton};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole, lerp_color};
use nikgob_view::{Status, TrackRow, ViewState};

use crate::{gesture, text};

pub const ROW_H: f32 = 64.0;
pub const NUMBERED_H: f32 = 56.0;
/// The space under each card of a grouped list.
pub const GROUP_GAP: f32 = 3.0;
const SIDE: f32 = 16.0;
const COVER: f32 = 48.0;
const NUMBER_W: f32 = 36.0;
const MORE: f32 = 48.0;
/// A group's ends and its inner corners.
const GROUP_END: f32 = 16.0;
const GROUP_INNER: f32 = 4.0;

/// What sits at the start of the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lead {
    Cover,
    Number(u32),
}

/// How a row looks and what it can do.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub lead: Lead,
    pub playing: bool,
    /// Not playable here now (no file and no source).
    pub muted: bool,
    /// A swipe to the right plays it next.
    pub swipe_next: bool,
    /// A swipe to the left takes it out (of the queue).
    pub swipe_remove: bool,
    /// The ⋮ at the end.
    pub more: bool,
    /// A drag handle at the end (the queue's order).
    pub handle: bool,
    /// One card of a grouped list: its place and the group's length.
    pub group: Option<(usize, usize)>,
}

impl Look {
    pub fn list(t: &TrackRow, state: &ViewState) -> Self {
        Self {
            lead: Lead::Cover,
            playing: playing(t, state),
            muted: t.source.is_none() && !t.has_local,
            swipe_next: true,
            swipe_remove: false,
            more: true,
            handle: false,
            group: None,
        }
    }

    pub fn numbered(t: &TrackRow, state: &ViewState) -> Self {
        Self { lead: Lead::Number(t.track_no), ..Self::list(t, state) }
    }

    /// As card `i` of a grouped list of `n`.
    pub fn grouped(self, i: usize, n: usize) -> Self {
        Self { group: Some((i, n)), ..self }
    }
}

/// The corners of card `i` of a grouped list of `n`: the group's ends
/// round, the inside small.
pub fn group_radii(i: usize, n: usize) -> Radii {
    let top = if i == 0 { GROUP_END } else { GROUP_INNER };
    let bottom = if i + 1 >= n { GROUP_END } else { GROUP_INNER };
    Radii([top, top, bottom, bottom])
}

/// `t` is the one playing.
pub fn playing(t: &TrackRow, state: &ViewState) -> bool {
    state.player.current.as_ref().is_some_and(|c| c.id == t.id)
}

#[derive(Default)]
pub struct RowOut {
    pub tap: bool,
    pub menu: bool,
    pub play_next: bool,
    pub remove: bool,
    pub handle: Option<Response>,
}

/// The second line: «Артист • 3:42».
fn supporting(t: &TrackRow) -> String {
    match t.duration {
        Some(d) if !t.artist.is_empty() => format!("{} • {}", t.artist, text::duration(d)),
        Some(d) => text::duration(d),
        None => t.artist.clone(),
    }
}

/// Draws the row of `t` over `rect` (a grouped card: `rect` holds the gap
/// under it too).
pub fn track(ui: &mut Ui, rect: Rect, id: Id, t: &TrackRow, look: Look, state: &ViewState) -> RowOut {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    // The card of a grouped list, with the page's margins; a plain row
    // fills its rect.
    let (body, inset) = match look.group {
        Some(_) => (Rect::from_min_max(pos2(rect.left() + 16.0, rect.top()), pos2(rect.right() - 16.0, rect.bottom() - GROUP_GAP)), 8.0),
        None => (rect, SIDE),
    };
    let g = gesture::row(ui, body, id, look.swipe_next, look.swipe_remove);
    let mut out = RowOut { tap: g.tap, menu: g.long, play_next: g.swiped, remove: g.swiped_left, handle: None };
    let live = (g.offset != 0.0).then_some(g.offset);
    let dx = gesture::follow(&ctx, id.with("x"), live, theme.motion.fast_spatial);
    let rest = match look.group {
        Some((i, n)) => group_radii(i, n),
        None => Radii::all(16.0),
    };
    // What the swipe does, revealed under the row.
    if dx.abs() > 0.5 {
        let k = (dx.abs() / gesture::SWIPE_TRIGGER).clamp(0.0, 1.0);
        let (fill, on, icon) = if dx > 0.0 {
            (s.primary_container, s.on_primary_container, Icon::QueueMusic)
        } else {
            (s.error_container, s.on_error_container, Icon::Delete)
        };
        paint::container(ui.painter(), body, rest, fill.gamma_multiply(0.4 + 0.6 * k), Stroke::NONE);
        let size = 20.0 + 4.0 * k;
        let c = if dx > 0.0 { pos2(body.left() + 28.0, body.center().y) } else { pos2(body.right() - 28.0, body.center().y) };
        icon.widget(k >= 1.0, on.gamma_multiply(k), size).paint_at(ui, Rect::from_center_size(c, vec2(size, size)));
    }
    let row = body.translate(vec2(dx, 0.0));
    let alpha = if look.muted { 0.38 } else { 1.0 };
    // The card, turning into a pill on the primary container while it plays.
    let current = nikgob_md3e::components::anim::fraction(&ctx, id.with("current"), if look.playing { 1.0 } else { 0.0 }, theme.motion.fast_effects);
    let pill = Radii::all(row.height() * 0.5);
    let (card, radii) = match look.group {
        Some(_) => (row, rest.lerp(pill, current)),
        None => (row.shrink2(vec2(8.0, 2.0)), Radii::all(16.0).lerp(Radii::all((row.height() - 4.0) * 0.5), current)),
    };
    let base = if look.group.is_some() {
        s.surface_container
    } else if dx.abs() > 0.5 {
        s.surface
    } else {
        egui::Color32::TRANSPARENT
    };
    let fill = lerp_color(base, s.primary_container, current);
    if look.group.is_none() && dx.abs() > 0.5 && current < 0.01 {
        paint::container(ui.painter(), row, Radii::all(0.0), s.surface, Stroke::NONE);
    } else {
        paint::container(ui.painter(), card, radii, fill, Stroke::NONE);
    }
    let pressed = g.response.is_pointer_button_down_on() && live.is_none();
    let layer = nikgob_md3e::components::anim::fraction(&ctx, id.with("layer"), if pressed { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), card, radii, s.on_surface, layer);
    let ink = lerp_color(s.on_surface, s.on_primary_container, current);
    let quiet = lerp_color(s.on_surface_variant, s.on_primary_container, current);

    let mut x = row.left() + inset;
    let now_playing = look.playing && state.player.status == Status::Playing;
    match look.lead {
        Lead::Cover => {
            let c = Rect::from_min_size(pos2(x, row.center().y - COVER * 0.5), vec2(COVER, COVER));
            let uri = t.cover.as_ref().map(|c| c.uri());
            let mut cui = ui.new_child(UiBuilder::new().max_rect(c));
            cui.multiply_opacity(alpha);
            // The playing cover is a circle.
            let r = 8.0 + (COVER * 0.5 - 8.0) * current;
            phone::paint_cover(&cui, c, uri.as_deref(), r);
            if look.playing {
                paint::container(ui.painter(), c, Radii::all(r), egui::Color32::from_black_alpha(110), Stroke::NONE);
                Equalizer::new(id.with("eq"), egui::Color32::WHITE).size(20.0).playing(now_playing).paint_at(ui, Rect::from_center_size(c.center(), vec2(20.0, 20.0)));
            }
            x = c.right() + 12.0;
        }
        Lead::Number(n) => {
            let c = Rect::from_min_size(pos2(x, row.top()), vec2(NUMBER_W, row.height()));
            if look.playing {
                Equalizer::new(id.with("eq"), s.on_primary_container).size(20.0).playing(now_playing).paint_at(ui, Rect::from_center_size(c.center(), vec2(20.0, 20.0)));
            } else if n > 0 {
                ui.painter().text(c.center(), egui::Align2::CENTER_CENTER, n.to_string(), TypeRole::BodyMedium.font_id(), s.on_surface_variant.gamma_multiply(alpha));
            }
            x = c.right() + 8.0;
        }
    }
    let mut right = row.right() - inset;
    if look.more {
        right = row.right() - 4.0 - MORE;
    }
    if look.handle {
        right -= MORE;
    }
    let w = (right - x - 8.0).max(16.0);
    let title_style = if look.playing { TypeRole::BodyLarge.emphasized() } else { TypeRole::BodyLarge.style() };
    let sub_style = TypeRole::BodySmall.style();
    let top = row.center().y - (title_style.line_height + sub_style.line_height) * 0.5;
    text::paint_line(ui, pos2(x, top), t.title.clone(), title_style, ink.gamma_multiply(alpha), w);
    let mut sx = x;
    let sy = top + title_style.line_height;
    if t.has_local {
        let mark = lerp_color(s.primary, s.on_primary_container, current);
        Icon::DownloadDone
            .widget(true, mark.gamma_multiply(alpha), 14.0)
            .paint_at(ui, Rect::from_min_size(pos2(sx, sy + (sub_style.line_height - 14.0) * 0.5), vec2(14.0, 14.0)));
        sx += 18.0;
    }
    text::paint_line(ui, pos2(sx, sy), supporting(t), sub_style, quiet.gamma_multiply(alpha), (right - sx - 8.0).max(16.0));

    let mut end = row.right() - 4.0;
    if look.more {
        let r = Rect::from_min_size(pos2(end - MORE, row.center().y - MORE * 0.5), vec2(MORE, MORE));
        if ui.put(r, IconButton::new(Icon::MoreVert).id_salt(id.with("more"))).clicked() {
            out.menu = true;
        }
        end -= MORE;
    }
    if look.handle {
        let r = Rect::from_min_size(pos2(end - MORE, row.center().y - MORE * 0.5), vec2(MORE, MORE));
        let h = ui.interact(r, id.with("handle"), Sense::drag());
        let c = if h.dragged() { s.primary } else { s.on_surface_variant };
        Icon::DragIndicator.widget(false, c, 24.0).paint_at(ui, Rect::from_center_size(r.center(), vec2(24.0, 24.0)));
        out.handle = Some(h);
    }
    if out.menu || out.play_next || out.remove {
        out.tap = false;
    }
    out
}

/// A plain row of a service's song (no menu of its own): its cover, its
/// title over `sub`. True when tapped.
pub fn simple(ui: &mut Ui, rect: Rect, id: Id, uri: Option<&str>, title: &str, sub: &str) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let g = gesture::row(ui, rect, id, false, false);
    let pressed = g.response.is_pointer_button_down_on();
    let layer = nikgob_md3e::components::anim::fraction(&ctx, id.with("layer"), if pressed { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect.shrink2(vec2(8.0, 2.0)), Radii::all(16.0), s.on_surface, layer);
    let c = Rect::from_min_size(pos2(rect.left() + SIDE, rect.center().y - COVER * 0.5), vec2(COVER, COVER));
    phone::paint_cover(ui, c, uri, 8.0);
    let x = c.right() + SIDE;
    let w = (rect.right() - SIDE - x).max(16.0);
    let ts = TypeRole::BodyLarge.style();
    let ss = TypeRole::BodyMedium.style();
    let top = rect.center().y - (ts.line_height + ss.line_height) * 0.5;
    text::paint_line(ui, pos2(x, top), title, ts, s.on_surface, w);
    text::paint_line(ui, pos2(x, top + ts.line_height), sub, ss, s.on_surface_variant, w);
    g.tap
}

/// A list of `n` rows `row_h` tall in `ui`'s width, only those in sight
/// drawn, each in a child of its own; returns the list's rect.
pub fn list(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, n: usize, row_h: f32, mut draw: impl FnMut(&mut Ui, usize, Rect)) -> Rect {
    let width = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(width, n as f32 * row_h), Sense::hover());
    if n == 0 {
        return area;
    }
    let clip = ui.clip_rect();
    let first = ((clip.top() - area.top()) / row_h).floor().max(0.0) as usize;
    let last = (((clip.bottom() - area.top()) / row_h).ceil().max(0.0) as usize + 1).min(n);
    let base = Id::new(id);
    for i in first.min(n)..last.max(first.min(n)) {
        let r = Rect::from_min_size(pos2(area.left(), area.top() + i as f32 * row_h), vec2(width, row_h));
        let mut child = ui.new_child(UiBuilder::new().max_rect(r).id_salt(base.with(i)));
        draw(&mut child, i, r);
    }
    area
}

/// The rows in sight of a list at `top` (its first) of `n` rows: the
/// window a long list loads.
pub fn in_sight(clip: Rect, top: f32, row_h: f32, n: usize) -> std::ops::Range<usize> {
    let first = ((clip.top() - top) / row_h).floor().max(0.0) as usize;
    let last = (((clip.bottom() - top) / row_h).ceil().max(0.0) as usize).min(n);
    first.min(last)..last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_in_sight() {
        let clip = Rect::from_min_max(pos2(0.0, 100.0), pos2(400.0, 500.0));
        assert_eq!(in_sight(clip, 100.0, 64.0, 1000), 0..7);
        assert_eq!(in_sight(clip, -1180.0, 64.0, 1000), 20..27);
        assert_eq!(in_sight(clip, 100.0, 64.0, 3), 0..3);
        assert_eq!(in_sight(clip, 600.0, 64.0, 3), 0..0);
    }

    #[test]
    fn a_group_is_round_at_its_ends() {
        assert_eq!(group_radii(0, 3).0, [16.0, 16.0, 4.0, 4.0]);
        assert_eq!(group_radii(1, 3).0, [4.0; 4]);
        assert_eq!(group_radii(2, 3).0, [4.0, 4.0, 16.0, 16.0]);
        assert_eq!(group_radii(0, 1).0, [16.0; 4]);
    }
}
