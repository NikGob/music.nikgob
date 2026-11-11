//! «Источник и качество» of a track that is not the one playing (or of the
//! playing one while the side pane's «Инфо» is off): a modal window of its
//! own, the same kind as «Скорость | Эквалайзер». It opens from the ⋮ and
//! the right click of a list's row, a queue row, a service's song with a
//! file, and closes on its ✕, a click outside and Esc.
//!
//! The runtime loads the track's info on [`Command::TrackInfo`] into
//! [`ViewState::track_info`]; the window shows it with the same layout as
//! the side pane and Now Playing's sheet ([`super::pane::info_body`]).

use std::sync::Arc;

use egui::{Align, Context, Id, Layout, Rect, Sense, UiBuilder, pos2, vec2};
use nikgob_view::{Command, TrackId, TrackInfo, TrackRow, ViewState};

use crate::{
    Icon, Theme, TypeRole,
    components::{IconButton, LoadingIndicator, ModalWindow, Size, Style, anim, modal_window, paint, paint::Radii},
};

/// The widest the window gets: room for the two columns of the info.
const MAX_W: f32 = 880.0;
/// The window's own room above and below what it shows.
const WINDOW_INSETS: f32 = 2.0 * modal_window::SHELL_PADDING;
/// Height of the row of the title and the close button.
const HEADER_H: f32 = 40.0;
/// Between the header and the card under it.
const HEADER_GAP: f32 = 12.0;
/// The card the info stands on: its corners and the room at its edge.
const CARD_RADIUS: f32 = 24.0;
const CARD_PAD: f32 = 20.0;
/// What the card takes before its content was ever measured (the loading
/// mark).
const FIRST_H: f32 = 96.0;

#[derive(Clone)]
struct Window {
    open: bool,
    track: TrackId,
    /// Its row, when it has one (a service's song has none): what the
    /// buttons under the tags offer.
    row: Option<Arc<TrackRow>>,
    /// The info last shown: the window keeps it while it leaves.
    shown: Option<Arc<TrackInfo>>,
}

fn key() -> Id {
    Id::new("md3e-track-info-window")
}

/// The window the info is drawn in.
fn window() -> Id {
    key().with("window")
}

/// Opens the window on `track` (its `row`, if it has one) and asks the
/// runtime for its info.
pub(crate) fn open(ctx: &Context, track: TrackId, row: Option<TrackRow>, out: &mut Vec<Command>) {
    // Opened anew it comes in from the start; one that is up stays up and
    // shows the other track.
    let up = ctx.data(|d| d.get_temp::<Window>(key())).is_some_and(|w| w.open);
    if !up {
        ModalWindow::rewind(ctx, window());
    }
    let w = Window { open: true, track, row: row.map(Arc::new), shown: None };
    ctx.data_mut(|d| d.insert_temp(key(), w));
    out.push(Command::TrackInfo(Some(track)));
}

/// The track the window shows, while it is open.
#[cfg(test)]
pub(crate) fn shown_track(ctx: &Context) -> Option<TrackId> {
    ctx.data(|d| d.get_temp::<Window>(key())).filter(|w| w.open).map(|w| w.track)
}

/// The window, while it is open (and while it leaves).
pub(crate) fn overlay(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    let Some(mut w) = ctx.data(|d| d.get_temp::<Window>(key())) else { return };
    let theme = Theme::get(ctx);
    // The runtime's answer, once it is of this track.
    if w.open
        && let Some(i) = state.track_info.as_ref().filter(|i| i.track == Some(w.track))
        && w.shown.as_deref() != Some(i)
    {
        w.shown = Some(Arc::new(i.clone()));
    }
    let open = w.open;
    let mut close = false;
    // As tall as what it shows (the screen keeps it smaller: then it
    // scrolls), moving to a new height on a spring.
    let height_key = key().with("content");
    let content = ctx.data(|d| d.get_temp::<f32>(height_key)).unwrap_or(FIRST_H);
    let height = anim::f32(ctx, key().with("height"), HEADER_H + HEADER_GAP + content + WINDOW_INSETS, theme.motion.default_spatial);
    let shell = ModalWindow::new(window(), vec2(MAX_W, f32::INFINITY)).height(height).shell();
    let dismissed = shell.show(ctx, open, |ui, inner| {
        close = header(ui);
        ui.add_space(HEADER_GAP);
        let card = Rect::from_min_max(ui.cursor().min, pos2(inner.right(), inner.bottom().max(ui.cursor().min.y)));
        paint::container(ui.painter(), card, Radii::all(CARD_RADIUS), theme.scheme.surface_container_highest, egui::Stroke::NONE);
        let room = card.shrink(CARD_PAD);
        let mut on_card = ui.new_child(UiBuilder::new().max_rect(room).layout(Layout::top_down(Align::Min)).id_salt("track-info-card"));
        on_card.set_clip_rect(card.intersect(ui.clip_rect()));
        let drawn = match &w.shown {
            Some(i) => {
                let i = Arc::clone(i);
                let row = w.row.clone();
                super::pane::info_scroll(&mut on_card, "track-info", Some(w.track), |ui| {
                    super::pane::info_body(ui, &i, row.as_deref(), out);
                })
            }
            None => {
                let (slot, _) = on_card.allocate_exact_size(vec2(on_card.available_width(), 56.0), Sense::hover());
                on_card.put(Rect::from_center_size(slot.center(), vec2(48.0, 48.0)), LoadingIndicator::new().size(48.0));
                slot.height()
            }
        };
        ui.ctx().data_mut(|d| d.insert_temp(height_key, drawn + 2.0 * CARD_PAD));
    });
    w.open = open && !(dismissed || close);
    if open && !w.open {
        // No longer kept up to date.
        out.push(Command::TrackInfo(None));
    }
    ctx.data_mut(|d| d.insert_temp(key(), w));
}

/// The window's first row: its title and the close button. True when
/// closed.
fn header(ui: &mut egui::Ui) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)).id_salt("track-info-header-right"));
    let close = IconButton::new(Icon::Close).size(Size::S).style(Style::Standard).tooltip("Закрыть (Esc)").id_salt("track-info-close");
    let closed = right.add(close).clicked();
    let left = Rect::from_min_max(row.min, pos2((right.min_rect().left() - 12.0).max(row.left()), row.bottom()));
    let mut left = ui.new_child(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)).id_salt("track-info-header-left"));
    // In line with the card's content under it.
    left.add_space(CARD_PAD - modal_window::SHELL_PADDING);
    left.label(TypeRole::TitleLarge.style().rich("Источник и качество", s.on_surface));
    closed
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
        Theme::default().install(&ctx);
        ctx
    }

    /// Draws the window headless for a while; returns the texts of the last
    /// frame.
    fn draw(ctx: &Context, clock: &mut f64, state: &ViewState, out: &mut Vec<Command>) -> Vec<String> {
        let mut texts = Vec::new();
        for _ in 0..40 {
            *clock += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
                time: Some(*clock),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| overlay(ui.ctx(), state, out));
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        texts
    }

    /// The window asks for the track's info, shows that track's (never the
    /// playing one's), and lets it go when closed.
    #[test]
    fn the_window_shows_the_info_of_its_own_track() {
        let ctx = context();
        let (track, playing) = (TrackId::new(), TrackId::new());
        let mut state = ViewState::default();
        state.info = TrackInfo { track: Some(playing), source: Some("Spotify".into()), ..TrackInfo::default() };
        let mut out = Vec::new();
        open(&ctx, track, None, &mut out);
        assert_eq!(out, vec![Command::TrackInfo(Some(track))]);
        assert_eq!(shown_track(&ctx), Some(track));
        out.clear();

        // Not answered yet: nothing of another track.
        let mut clock = 0.0;
        let texts = draw(&ctx, &mut clock, &state, &mut out);
        assert!(texts.iter().any(|t| t == "Источник и качество"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Spotify"), "{texts:?}");

        state.track_info = Some(TrackInfo {
            track: Some(track),
            source: Some("Локальный файл".into()),
            tags: vec![("Название".into(), "Песня".into())],
            ..TrackInfo::default()
        });
        let texts = draw(&ctx, &mut clock, &state, &mut out);
        for text in ["Локальный файл", "Песня"] {
            assert!(texts.iter().any(|t| t == text), "no «{text}» in {texts:?}");
        }
        assert!(out.is_empty(), "{out:?}");

        // Esc: closed, and the runtime may let it go.
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            time: Some(clock + 0.05),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| overlay(ui.ctx(), &state, &mut out));
        assert_eq!(out, vec![Command::TrackInfo(None)]);
        assert_eq!(shown_track(&ctx), None);
    }
}
