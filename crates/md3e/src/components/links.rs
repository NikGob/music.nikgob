//! Links inside a line of text: each artist name of a credit line ("A feat.
//! B & C") is its own link, the separators and the rest of the line are
//! not. A link shows the pointing hand and an underline while hovered, in
//! the text's own colour (no accent: the line stays quiet until touched).

use std::ops::Range;
use std::sync::Arc;

use egui::{
    Color32, CursorIcon, Galley, Id, Pos2, Rect, Response, Sense, Stroke, TextFormat, Ui, pos2,
    text::{LayoutJob, TextWrapping},
    vec2,
};

/// Where the text of `links` (byte ranges of the galley's text) is drawn,
/// relative to the galley's origin: one rect per link and row, with the
/// baseline of that row. Text cut by an ellipsis is not there.
pub fn link_rects(galley: &Galley, links: &[Range<usize>]) -> Vec<(usize, Rect, f32)> {
    let text = galley.text();
    let to_char = |b: usize| text.get(..b).map_or(0, |s| s.chars().count());
    let chars: Vec<Range<usize>> = links.iter().map(|r| to_char(r.start)..to_char(r.end)).collect();
    let mut out = Vec::new();
    let mut ci = 0usize;
    let rows = galley.rows.len();
    for (ri, row) in galley.rows.iter().enumerate() {
        let mut spans: Vec<Option<(Rect, f32)>> = vec![None; links.len()];
        let n = row.glyphs.len();
        for (gi, g) in row.glyphs.iter().enumerate() {
            // The ellipsis of a cut line stands for no character.
            let ellipsis = galley.elided && ri + 1 == rows && gi + 1 == n && g.chr == '…';
            if !ellipsis {
                for (k, cr) in chars.iter().enumerate() {
                    if cr.contains(&ci) {
                        let r = Rect::from_min_max(
                            pos2(row.pos.x + g.pos.x, row.min_y()),
                            pos2(row.pos.x + g.pos.x + g.advance_width, row.max_y()),
                        );
                        let base = row.pos.y + g.pos.y;
                        spans[k] = Some(match spans[k] {
                            Some((acc, b)) => (acc.union(r), b),
                            None => (r, base),
                        });
                    }
                }
            }
            ci += 1;
        }
        if row.ends_with_newline {
            ci += 1;
        }
        for (k, s) in spans.into_iter().enumerate() {
            if let Some((r, b)) = s {
                out.push((k, r, b));
            }
        }
    }
    out
}

/// Makes `links` of a galley painted at `origin` clickable: hover shows the
/// pointing hand and underlines the link in `color`. Returns the index of
/// the link clicked this frame, and whether one is hovered.
pub fn interact_links(
    ui: &Ui,
    id: Id,
    galley: &Galley,
    origin: Pos2,
    links: &[Range<usize>],
    color: Color32,
) -> (Option<usize>, bool) {
    if links.is_empty() {
        return (None, false);
    }
    let rects = link_rects(galley, links);
    let responses: Vec<(usize, Rect, f32, Response)> = rects
        .into_iter()
        .enumerate()
        .map(|(j, (k, r, base))| {
            let r = r.translate(origin.to_vec2());
            let resp = ui.interact(r, id.with(("link", k, j)), Sense::click());
            (k, r, base + origin.y, resp)
        })
        .collect();
    let hovered: Option<usize> = responses.iter().find(|(.., resp)| resp.hovered()).map(|(k, ..)| *k);
    let mut clicked = None;
    for (k, r, base, resp) in &responses {
        if Some(*k) == hovered {
            ui.painter().line_segment(
                [pos2(r.left(), base + 2.0), pos2(r.right(), base + 2.0)],
                Stroke::new(1.0, color),
            );
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            clicked = Some(*k);
        }
    }
    (clicked, hovered.is_some())
}

/// What [`link_label`] reported.
pub struct LinkLabel {
    /// The whole label (hover only: a click outside the links goes to
    /// whatever is under it).
    pub response: Response,
    /// The link clicked this frame.
    pub clicked: Option<usize>,
}

/// A one-line label in `format` with links on parts of it, cut with an
/// ellipsis to the available width.
pub fn link_label(ui: &mut Ui, id_salt: &str, text: &str, links: &[Range<usize>], format: TextFormat) -> LinkLabel {
    link_text(ui, id_salt, text, links, format, 1)
}

/// [`link_label`] that wraps onto as many rows as it needs.
pub fn link_paragraph(ui: &mut Ui, id_salt: &str, text: &str, links: &[Range<usize>], format: TextFormat) -> LinkLabel {
    link_text(ui, id_salt, text, links, format, usize::MAX)
}

fn link_text(
    ui: &mut Ui,
    id_salt: &str,
    text: &str,
    links: &[Range<usize>],
    format: TextFormat,
    max_rows: usize,
) -> LinkLabel {
    let color = format.color;
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap = TextWrapping {
        max_width: ui.available_width().max(8.0),
        max_rows,
        break_anywhere: max_rows == 1,
        ..TextWrapping::default()
    };
    let galley: Arc<Galley> = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x, galley.size().y), Sense::hover());
    let origin = rect.min;
    let (clicked, _) = interact_links(ui, ui.id().with(("link-label", id_salt)), &galley, origin, links, color);
    ui.painter().galley(origin, galley, color);
    LinkLabel { response, clicked }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(text: &str, width: f32) -> Arc<Galley> {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut out = None;
        // Fonts are ready from the second pass.
        for _ in 0..2 {
            let mut frame = ctx.run_ui(Default::default(), |ui| {
                let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::default());
                job.wrap = TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, ..Default::default() };
                out = Some(ui.painter().layout_job(job));
            });
            frame.textures_delta.clear();
        }
        out.unwrap()
    }

    #[test]
    fn each_link_gets_its_own_rect() {
        let text = "Кино, Земфира · Live";
        let g = layout(text, 1000.0);
        let a = 0..text.find(',').unwrap();
        let b = text.find('З').unwrap()..text.find(" ·").unwrap();
        let rects = link_rects(&g, &[a, b]);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].0, 0);
        assert_eq!(rects[1].0, 1);
        assert!(rects[0].1.right() < rects[1].1.left(), "the comma is between them");
        assert!(rects[1].1.right() < g.size().x, "the album is not part of the link");
    }

    #[test]
    fn a_cut_link_ends_before_the_ellipsis() {
        let text = "Somebody With A Long Name, Another Long Name";
        let g = layout(text, 60.0);
        assert!(g.elided);
        let second = text.find("Another").unwrap()..text.len();
        assert!(link_rects(&g, &[second]).is_empty(), "the second name is cut off");
        let first = 0..4;
        assert_eq!(link_rects(&g, &[first]).len(), 1);
    }
}
