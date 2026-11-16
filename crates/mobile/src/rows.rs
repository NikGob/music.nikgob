//! The track row of the phone: a flat 64 dp row, a 48 dp thumb at x 14
//! (radius 4) or, on an album, the number centred in that column, the title
//! at x 74 in TitleSmall emphasized over a 12 sp line, and ⋮ at the end. A
//! tap plays; a long press or ⋮ opens the track's menu; a swipe to the
//! right plays it next (the queue's rows swipe left to leave, and keep
//! their drag handle). The row that plays is a `secondary_container`
//! rectangle, radius 8, inset 8 dp on each side, the full row tall; its
//! text stays in the normal colours and its square cover is dimmed under
//! the live bars.

use egui::{Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{Equalizer, IconButton};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Status, TrackRow, ViewState};

use crate::{gesture, text};

pub const ROW_H: f32 = 64.0;
/// The thumb, and the column an album's number is centred in.
pub const THUMB: f32 = 48.0;
pub const THUMB_X: f32 = 14.0;
/// Title x: the thumb's right plus 12 dp, so a numbered row lines up.
pub const TITLE_X: f32 = 74.0;
pub const THUMB_R: f32 = 4.0;
/// The playing row's mark: inset on each side, the full row tall, radius 8.
pub const CURRENT_INSET: f32 = 8.0;
pub const CURRENT_R: f32 = 8.0;
const MORE: f32 = 48.0;

/// The 48 dp thumb of a row, centred on its height.
pub fn thumb_rect(row: Rect) -> Rect {
    Rect::from_min_size(pos2(row.left() + THUMB_X, row.center().y - THUMB * 0.5), vec2(THUMB, THUMB))
}

/// The playing row's rectangle: 8 dp in on each side, the full height.
pub fn current_mark(row: Rect) -> Rect {
    Rect::from_min_max(pos2(row.left() + CURRENT_INSET, row.top()), pos2(row.right() - CURRENT_INSET, row.bottom()))
}

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

/// The second line: «Артист • 3:42».
fn supporting(t: &TrackRow) -> String {
    match t.duration {
        Some(d) if !t.artist.is_empty() => format!("{} • {}", t.artist, text::duration(d)),
        Some(d) => text::duration(d),
        None => t.artist.clone(),
    }
}

/// Draws the row of `t` over `rect`.
pub fn track(ui: &mut Ui, rect: Rect, id: Id, t: &TrackRow, look: Look, state: &ViewState) -> RowOut {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let body = rect;
    let g = gesture::row(ui, body, id, look.swipe_next, look.swipe_remove);
    let mut out = RowOut { tap: g.tap, menu: g.long, play_next: g.swiped, remove: g.swiped_left, handle: None };
    let live = (g.offset != 0.0).then_some(g.offset);
    let dx = gesture::follow(&ctx, id.with("x"), live, theme.motion.fast_spatial);
    if dx.abs() > 0.5 {
        let k = (dx.abs() / gesture::SWIPE_TRIGGER).clamp(0.0, 1.0);
        let (fill, on, icon) = if dx > 0.0 {
            (s.primary_container, s.on_primary_container, Icon::QueueMusic)
        } else {
            (s.error_container, s.on_error_container, Icon::Delete)
        };
        paint::container(ui.painter(), body, Radii::all(0.0), fill.gamma_multiply(0.4 + 0.6 * k), Stroke::NONE);
        let size = 20.0 + 4.0 * k;
        let c = if dx > 0.0 { pos2(body.left() + 28.0, body.center().y) } else { pos2(body.right() - 28.0, body.center().y) };
        icon.widget(k >= 1.0, on.gamma_multiply(k), size).paint_at(ui, Rect::from_center_size(c, vec2(size, size)));
    }
    let row = body.translate(vec2(dx, 0.0));
    let alpha = if look.muted { 0.38 } else { 1.0 };
    let current = nikgob_md3e::components::anim::fraction(&ctx, id.with("current"), if look.playing { 1.0 } else { 0.0 }, theme.motion.fast_effects);
    if dx.abs() > 0.5 {
        paint::container(ui.painter(), row, Radii::all(0.0), s.surface, Stroke::NONE);
    }
    let mark = current_mark(row);
    if current > 0.01 {
        paint::container(ui.painter(), mark, Radii::all(CURRENT_R), s.secondary_container.gamma_multiply(current), Stroke::NONE);
    }
    let pressed = g.response.is_pointer_button_down_on() && live.is_none();
    let layer = nikgob_md3e::components::anim::fraction(&ctx, id.with("layer"), if pressed { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    let (layer_rect, layer_r) = if current > 0.5 { (mark, Radii::all(CURRENT_R)) } else { (row, Radii::all(0.0)) };
    paint::state_layer(ui.painter(), layer_rect, layer_r, s.on_surface, layer);
    let ink = s.on_surface;
    let quiet = s.on_surface_variant;

    let now_playing = look.playing && state.player.status == Status::Playing;
    match look.lead {
        Lead::Cover => {
            let c = thumb_rect(row);
            let uri = t.cover.as_ref().map(|c| c.uri());
            let mut cui = ui.new_child(UiBuilder::new().max_rect(c));
            cui.multiply_opacity(alpha);
            phone::paint_cover(&cui, c, uri.as_deref(), THUMB_R);
            if look.playing {
                paint::container(ui.painter(), c, Radii::all(THUMB_R), egui::Color32::from_black_alpha(110), Stroke::NONE);
                Equalizer::new(id.with("eq"), egui::Color32::WHITE).size(20.0).playing(now_playing).paint_at(ui, Rect::from_center_size(c.center(), vec2(20.0, 20.0)));
            }
        }
        Lead::Number(n) => {
            let col = Rect::from_min_size(pos2(row.left() + THUMB_X, row.top()), vec2(THUMB, row.height()));
            if look.playing {
                Equalizer::new(id.with("eq"), ink).size(20.0).playing(now_playing).paint_at(ui, Rect::from_center_size(col.center(), vec2(20.0, 20.0)));
            } else if n > 0 {
                ui.painter().text(col.center(), egui::Align2::CENTER_CENTER, n.to_string(), TypeRole::BodyMedium.font_id(), quiet.gamma_multiply(alpha));
            }
        }
    }
    let x = row.left() + TITLE_X;
    let reserved = usize::from(look.more) + usize::from(look.handle);
    let text_right = row.right() - 4.0 - reserved as f32 * MORE - 8.0;
    let w = (text_right - x).max(16.0);
    let title_style = TypeRole::TitleSmall.emphasized();
    let sub_style = TypeRole::BodySmall.style();
    let top = row.center().y - (title_style.line_height + sub_style.line_height) * 0.5;
    text::paint_line(ui, pos2(x, top), t.title.clone(), title_style, ink.gamma_multiply(alpha), w);
    let mut sx = x;
    let sy = top + title_style.line_height;
    if t.has_local {
        Icon::DownloadDone
            .widget(true, s.primary.gamma_multiply(alpha), 14.0)
            .paint_at(ui, Rect::from_min_size(pos2(sx, sy + (sub_style.line_height - 14.0) * 0.5), vec2(14.0, 14.0)));
        sx += 18.0;
    }
    text::paint_line(ui, pos2(sx, sy), supporting(t), sub_style, quiet.gamma_multiply(alpha), (text_right - sx).max(16.0));

    let mut end = row.right() - 4.0;
    if look.more {
        let r = Rect::from_min_size(pos2(end - MORE, row.center().y - MORE * 0.5), vec2(MORE, MORE));
        if crate::parts::put(ui, r, IconButton::new(Icon::MoreVert).id_salt(id.with("more"))).clicked() {
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
    paint::state_layer(ui.painter(), rect, Radii::all(0.0), s.on_surface, layer);
    let c = thumb_rect(rect);
    phone::paint_cover(ui, c, uri, THUMB_R);
    let x = rect.left() + TITLE_X;
    let w = (rect.right() - 16.0 - x).max(16.0);
    let ts = TypeRole::TitleSmall.emphasized();
    let ss = TypeRole::BodySmall.style();
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
    fn a_flat_row_lines_the_title_up_with_the_thumb() {
        assert_eq!(ROW_H, 64.0);
        assert_eq!(THUMB_X + THUMB + 12.0, TITLE_X);
        let row = Rect::from_min_size(pos2(10.0, 20.0), vec2(384.0, ROW_H));
        let thumb = thumb_rect(row);
        assert_eq!(thumb.min, pos2(24.0, row.center().y - 24.0));
        assert_eq!(thumb.size(), vec2(THUMB, THUMB));
        let mark = current_mark(row);
        assert_eq!(mark.left(), 18.0);
        assert_eq!(mark.right(), 386.0);
        assert_eq!(mark.top(), row.top());
        assert_eq!(mark.bottom(), row.bottom());
        assert_eq!(CURRENT_R, 8.0);
        assert_eq!(THUMB_R, 4.0);
    }
}
