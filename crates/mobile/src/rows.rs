//! The track row of the phone (Metrolist's song item): 64 dp with a 48 dp
//! cover (56 dp with the track's number on an album), the title over the
//! artist and the length, a filled mark before them when the song has a
//! file here, and one ⋮ at the end. A tap plays; a long press or ⋮ opens
//! the track's menu; a swipe to the right plays it next (the queue's rows
//! swipe left to leave). The playing row lies on the secondary container
//! with live bars over its cover.

use egui::{Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{Equalizer, IconButton};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Status, TrackRow, ViewState};

use crate::{gesture, text};

pub const ROW_H: f32 = 64.0;
pub const NUMBERED_H: f32 = 56.0;
const SIDE: f32 = 16.0;
const COVER: f32 = 48.0;
const NUMBER_W: f32 = 28.0;
const MORE: f32 = 48.0;

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
        }
    }

    pub fn numbered(t: &TrackRow, state: &ViewState) -> Self {
        Self { lead: Lead::Number(t.track_no), ..Self::list(t, state) }
    }
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

/// The second line: «Артист · 3:42».
fn supporting(t: &TrackRow) -> String {
    match t.duration {
        Some(d) if !t.artist.is_empty() => format!("{} · {}", t.artist, text::duration(d)),
        Some(d) => text::duration(d),
        None => t.artist.clone(),
    }
}

/// Draws the row of `t` over `rect`.
pub fn track(ui: &mut Ui, rect: Rect, id: Id, t: &TrackRow, look: Look, state: &ViewState) -> RowOut {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let g = gesture::row(ui, rect, id, look.swipe_next, look.swipe_remove);
    let mut out = RowOut { tap: g.tap, menu: g.long, play_next: g.swiped, remove: g.swiped_left, handle: None };
    let live = (g.offset != 0.0).then_some(g.offset);
    let dx = gesture::follow(&ctx, id.with("x"), live, theme.motion.fast_spatial);
    // What the swipe does, revealed under the row.
    if dx.abs() > 0.5 {
        let k = (dx.abs() / gesture::SWIPE_TRIGGER).clamp(0.0, 1.0);
        let (band, fill, on, icon) = if dx > 0.0 {
            (Rect::from_min_max(rect.min, pos2(rect.left() + dx, rect.bottom())), s.primary_container, s.on_primary_container, Icon::QueueMusic)
        } else {
            (Rect::from_min_max(pos2(rect.right() + dx, rect.top()), rect.max), s.error_container, s.on_error_container, Icon::Delete)
        };
        paint::container(ui.painter(), band, Radii::all(0.0), fill.gamma_multiply(0.4 + 0.6 * k), Stroke::NONE);
        let size = 20.0 + 4.0 * k;
        let c = if dx > 0.0 { pos2(rect.left() + 28.0, rect.center().y) } else { pos2(rect.right() - 28.0, rect.center().y) };
        icon.widget(k >= 1.0, on.gamma_multiply(k), size).paint_at(ui, Rect::from_center_size(c, vec2(size, size)));
    }
    let row = rect.translate(vec2(dx, 0.0));
    let alpha = if look.muted { 0.38 } else { 1.0 };
    if look.playing {
        paint::container(ui.painter(), row.shrink2(vec2(8.0, 2.0)), Radii::all(16.0), s.secondary_container, Stroke::NONE);
    } else if dx.abs() > 0.5 {
        paint::container(ui.painter(), row, Radii::all(0.0), s.surface, Stroke::NONE);
    }
    let pressed = g.response.is_pointer_button_down_on() && live.is_none();
    let layer = nikgob_md3e::components::anim::fraction(&ctx, id.with("layer"), if pressed { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), row.shrink2(vec2(8.0, 2.0)), Radii::all(16.0), s.on_surface, layer);

    let mut x = row.left() + SIDE;
    let now_playing = look.playing && state.player.status == Status::Playing;
    match look.lead {
        Lead::Cover => {
            let c = Rect::from_min_size(pos2(x, row.center().y - COVER * 0.5), vec2(COVER, COVER));
            let uri = t.cover.as_ref().map(|c| c.uri());
            let mut cui = ui.new_child(UiBuilder::new().max_rect(c));
            cui.multiply_opacity(alpha);
            phone::paint_cover(&cui, c, uri.as_deref(), 8.0);
            if look.playing {
                paint::container(ui.painter(), c, Radii::all(8.0), egui::Color32::from_black_alpha(110), Stroke::NONE);
                Equalizer::new(id.with("eq"), egui::Color32::WHITE).size(20.0).playing(now_playing).paint_at(ui, Rect::from_center_size(c.center(), vec2(20.0, 20.0)));
            }
            x = c.right() + SIDE;
        }
        Lead::Number(n) => {
            let c = Rect::from_min_size(pos2(x, row.top()), vec2(NUMBER_W, row.height()));
            if look.playing {
                Equalizer::new(id.with("eq"), s.primary).size(20.0).playing(now_playing).paint_at(ui, Rect::from_center_size(c.center(), vec2(20.0, 20.0)));
            } else if n > 0 {
                ui.painter().text(c.center(), egui::Align2::CENTER_CENTER, n.to_string(), TypeRole::BodyLarge.font_id(), s.on_surface_variant.gamma_multiply(alpha));
            }
            x = c.right() + SIDE;
        }
    }
    let mut right = row.right() - SIDE;
    if look.more {
        right = row.right() - 4.0 - MORE;
    }
    if look.handle {
        right -= MORE;
    }
    let w = (right - x - 8.0).max(16.0);
    let title_style = TypeRole::BodyLarge.style();
    let sub_style = TypeRole::BodyMedium.style();
    let top = row.center().y - (title_style.line_height + sub_style.line_height) * 0.5;
    let title_color = if look.playing { s.on_secondary_container } else { s.on_surface };
    text::paint_line(ui, pos2(x, top), t.title.clone(), title_style, title_color.gamma_multiply(alpha), w);
    let mut sx = x;
    let sy = top + title_style.line_height;
    if t.has_local {
        Icon::DownloadDone
            .widget(true, s.primary.gamma_multiply(alpha), 14.0)
            .paint_at(ui, Rect::from_min_size(pos2(sx, sy + (sub_style.line_height - 14.0) * 0.5), vec2(14.0, 14.0)));
        sx += 18.0;
    }
    text::paint_line(ui, pos2(sx, sy), supporting(t), sub_style, s.on_surface_variant.gamma_multiply(alpha), (right - sx - 8.0).max(16.0));

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
}
