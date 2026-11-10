//! The sound sheet: «Скорость» of a track and the equalizer, the two tabs
//! of one modal bottom sheet.
//!
//! It opens from the playing track's menu («Скорость…», «Эквалайзер…»),
//! from a library track's «Скорость…», from Now Playing (its sound button
//! and the speed mark by the artist), from «Настроить» of the settings and
//! from the keys S and E; it closes on Esc, on a click outside, on a drag
//! down and on its own close button.
//!
//! «Скорость» is of one track: the playing one (whichever plays while the
//! sheet is up) or the library track whose menu opened it. The slider sets
//! any speed the engine takes, the chips the ready versions; a chip is on
//! only while the track plays exactly as that version. Both are heard at
//! once. While «Скорость трека» is off in the settings, or nothing plays,
//! the sheet is the equalizer alone.

use std::time::Duration;

use egui::{Align, Align2, Context, Id, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{Command, Prefs, SpeedVariant, TrackId, ViewState, slider_to_speed, speed_text, speed_to_slider};

use super::equalizer;
use crate::{
    Icon, Theme, TypeRole,
    components::{BottomSheet, ConnectedGroup, FilterChip, IconButton, Size, Slider, Style, Switch, anim},
};

/// The sheet's own room around what it shows: the drag handle's strip above
/// and the inset below (see [`BottomSheet`]).
const SHEET_INSETS: f32 = 36.0 + 16.0;
/// Height of the row of the tabs and the close button.
const HEADER_H: f32 = 40.0;
/// Between the header and the tab under it.
const HEADER_GAP: f32 = 16.0;
/// Height of the line the speed is written on.
const VALUE_H: f32 = 48.0;
/// Height of the row of a switch.
const SWITCH_ROW_H: f32 = 40.0;
/// While the slider is held, the track takes a new speed at most this
/// often (each one restarts its sound from where it is).
const SEND_EVERY: f64 = 0.12;
/// A speed asked for shows this long at most while the player has not
/// taken it (it never took it: the sheet shows what plays).
const ASKED_FOR: f64 = 1.0;

/// A tab of the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Tab {
    Speed,
    Equalizer,
}

#[derive(Clone, Debug)]
struct Sheet {
    open: bool,
    tab: Tab,
    /// The library track «Скорость» is of, and its title; `None`: the
    /// playing track.
    track: Option<(TrackId, String)>,
    /// The speed last asked for: shown until the player has it.
    asked: Option<(TrackId, SpeedVariant, f64)>,
    /// When a speed was last sent.
    sent_at: f64,
}

fn key() -> Id {
    Id::new("md3e-sound-sheet")
}

/// Opens the sheet on `tab`. `track`: the library track «Скорость» is of
/// and its title, or `None` for the playing one.
pub(crate) fn open(ctx: &Context, tab: Tab, track: Option<(TrackId, String)>) {
    let sheet = Sheet { open: true, tab, track, asked: None, sent_at: f64::MIN };
    ctx.data_mut(|d| d.insert_temp(key(), sheet));
}

/// The key of a tab (S, E): opens the sheet on it for the playing track,
/// moves an open sheet to it, and puts away the sheet that shows it.
pub(crate) fn toggle(ctx: &Context, tab: Tab, state: &ViewState) {
    let sheet = ctx.data(|d| d.get_temp::<Sheet>(key())).filter(|s| s.open);
    let Some(mut sheet) = sheet else {
        // «Скорость» needs a track to be of, and to be on in the settings.
        if tab == Tab::Equalizer || (state.speeds.enabled && state.player.current.is_some()) {
            open(ctx, tab, None);
        }
        return;
    };
    if !has_tabs(&sheet, state) || shown_tab(&sheet, state) == tab {
        sheet.open = false;
    } else {
        sheet.tab = tab;
    }
    ctx.data_mut(|d| d.insert_temp(key(), sheet));
}

/// The tab the playing track's sound button opens: its speed where there
/// is one to set, the equalizer otherwise.
pub(crate) fn first_tab(state: &ViewState) -> Tab {
    if state.speeds.enabled && state.player.current.is_some() { Tab::Speed } else { Tab::Equalizer }
}

/// The track the sheet's «Скорость» is of.
fn target(sheet: &Sheet, state: &ViewState) -> Option<TrackId> {
    sheet.track.as_ref().map(|(id, _)| *id).or_else(|| state.player.current.as_ref().map(|t| t.id))
}

/// Both tabs are there: the speed of tracks is on and there is a track.
fn has_tabs(sheet: &Sheet, state: &ViewState) -> bool {
    state.speeds.enabled && target(sheet, state).is_some()
}

fn shown_tab(sheet: &Sheet, state: &ViewState) -> Tab {
    if has_tabs(sheet, state) { sheet.tab } else { Tab::Equalizer }
}

/// What a tab takes before it was ever drawn (from then on the sheet is as
/// tall as the tab was).
fn first_height(tab: Tab) -> f32 {
    HEADER_H
        + HEADER_GAP
        + match tab {
            Tab::Speed => VALUE_H + 44.0 + 12.0 + 32.0 + 12.0 + SWITCH_ROW_H,
            Tab::Equalizer => equalizer::EDITOR_H,
        }
}

/// The sheet, while it is open (and while it leaves).
pub(crate) fn overlay(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    let Some(mut sheet) = ctx.data(|d| d.get_temp::<Sheet>(key())) else { return };
    let theme = Theme::get(ctx);
    let track = target(&sheet, state);
    let tabs = has_tabs(&sheet, state);
    let tab = shown_tab(&sheet, state);
    // The title of a library track that is not the one playing: on Now
    // Playing the song is in front of the listener already.
    let playing = state.player.current.as_ref().map(|t| t.id);
    let title = sheet.track.as_ref().filter(|(id, _)| Some(*id) != playing).map(|(_, title)| title.clone());

    let was = (state.prefs.eq_on, state.prefs.eq_gains, state.prefs.keep_timbre);
    let mut now = was;
    let mut open = sheet.open;
    let mut close = false;
    let mut to_tab = tab;

    // As tall as the tab it shows, moving between the two on a spring.
    let height_key = key().with(("content", tab));
    let content = ctx.data(|d| d.get_temp::<f32>(height_key)).unwrap_or_else(|| first_height(tab));
    let height = anim::f32(ctx, key().with("height"), content + SHEET_INSETS, theme.motion.default_spatial);
    let res = BottomSheet::new("md3e-sound").height(height).max_width(640.0).show(ctx, &mut open, |ui| {
        close = header(ui, tabs, &mut to_tab, &mut now.0);
        ui.add_space(HEADER_GAP);
        match (tab, track) {
            (Tab::Speed, Some(track)) => speed(ui, state, track, title.as_deref(), &mut sheet, &mut now.2, out),
            _ => equalizer::editor(ui, &mut now.0, &mut now.1),
        }
        let drawn = ui.min_rect().height();
        ui.ctx().data_mut(|d| d.insert_temp(height_key, drawn));
    });
    if res.dismissed || close {
        open = false;
    }
    sheet.open = open;
    if tabs {
        sheet.tab = to_tab;
    }
    ctx.data_mut(|d| d.insert_temp(key(), sheet));
    if now != was {
        out.push(Command::SetPrefs(Box::new(Prefs {
            eq_on: now.0,
            eq_gains: now.1,
            keep_timbre: now.2,
            ..state.prefs.clone()
        })));
    }
}

/// The sheet's first row: the tabs (the equalizer alone: its name), the
/// equalizer's switch on its tab, the close button. True when closed.
fn header(ui: &mut Ui, tabs: bool, tab: &mut Tab, eq_on: &mut bool) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let mut right = ui.new_child(
        UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)).id_salt("sound-header-right"),
    );
    right.spacing_mut().item_spacing.x = 8.0;
    let close = IconButton::new(Icon::Close).size(Size::S).style(Style::Standard).tooltip("Закрыть (Esc)").id_salt("sound-close");
    let closed = right.add(close).clicked();
    if *tab == Tab::Equalizer {
        right.add(Switch::new(eq_on).label("Эквалайзер").id_salt("sound-eq-on"));
    }
    let left = Rect::from_min_max(row.min, pos2((right.min_rect().left() - 12.0).max(row.left()), row.bottom()));
    let mut left = ui.new_child(
        UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)).id_salt("sound-header-left"),
    );
    if tabs {
        let at = usize::from(*tab == Tab::Equalizer);
        if let Some(picked) = ConnectedGroup::new("sound-tabs", &["Скорость", "Эквалайзер"]).show(&mut left, at) {
            *tab = if picked == 0 { Tab::Speed } else { Tab::Equalizer };
        }
    } else {
        left.label(TypeRole::TitleLarge.style().rich("Эквалайзер", s.on_surface));
    }
    closed
}

/// «Скорость» of `track`: the speed written out, the slider under it, the
/// ready versions and «Сохранять тембр голоса».
fn speed(
    ui: &mut Ui,
    state: &ViewState,
    track: TrackId,
    title: Option<&str>,
    sheet: &mut Sheet,
    keep_timbre: &mut bool,
    out: &mut Vec<Command>,
) {
    let s = Theme::get(ui.ctx()).scheme;
    let time = ui.input(|i| i.time);
    let chosen = state.speeds.of(track);
    if sheet.asked.is_some_and(|(of, asked, at)| of != track || asked == chosen || time - at > ASKED_FOR) {
        sheet.asked = None;
    }
    let plays = sheet.asked.map_or(chosen, |(_, asked, _)| asked);

    if let Some(title) = title {
        ui.add(egui::Label::new(TypeRole::TitleMedium.style().rich(title, s.on_surface_variant)).truncate());
        ui.add_space(4.0);
    }
    // The number's line first, the number itself once the slider said
    // where it is held.
    let (value, _) = ui.allocate_exact_size(vec2(ui.available_width(), VALUE_H), Sense::hover());
    let slider = Slider::new("sound-speed", speed_to_slider(plays.percent())).show(ui);
    let held = slider.dragging.or(slider.committed).map(slider_to_speed);
    // The version under the hand. On the speed the track has already it is
    // the track's own: touching the slider does not take the echo away.
    let shown = match held {
        Some(percent) if percent != plays.percent() => SpeedVariant::at(percent),
        _ => plays,
    };
    ui.painter().text(
        value.left_center(),
        Align2::LEFT_CENTER,
        speed_text(shown.percent()),
        TypeRole::HeadlineLarge.font_id(),
        s.on_surface,
    );
    let mut ask = None;
    if shown != plays {
        if slider.committed.is_some() || time - sheet.sent_at >= SEND_EVERY {
            ask = Some(shown);
        } else {
            // Held still on a speed not sent yet: it goes with the next frame.
            ui.ctx().request_repaint_after(Duration::from_secs_f64(SEND_EVERY));
        }
    }

    ui.add_space(12.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (i, version) in SpeedVariant::ALL.into_iter().enumerate() {
            let chip = FilterChip::new(version.label(), shown == version).id_salt(("sound-speed-version", i));
            if ui.add(chip).clicked() && plays != version {
                ask = Some(version);
            }
        }
    });

    ui.add_space(12.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), SWITCH_ROW_H), Sense::hover());
    let name = "Сохранять тембр голоса";
    ui.painter().text(row.left_center(), Align2::LEFT_CENTER, name, TypeRole::BodyLarge.font_id(), s.on_surface);
    let mut end = ui.new_child(
        UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)).id_salt("sound-keep-timbre-row"),
    );
    end.add(Switch::new(keep_timbre).label(name).id_salt("sound-keep-timbre"));

    if let Some(variant) = ask {
        out.push(Command::SetTrackSpeed { track, variant });
        sheet.asked = Some((track, variant, time));
        sheet.sent_at = time;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use egui::Shape;
    use nikgob_view::TrackSpeeds;

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

    /// Draws the sheet headless from `clock` on, long enough for it to
    /// have come up; returns the texts of the last frame.
    fn draw(ctx: &Context, clock: &mut f64, state: &ViewState, out: &mut Vec<Command>) -> Vec<String> {
        let mut texts = Vec::new();
        for _ in 0..40 {
            *clock += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
                time: Some(*clock),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                overlay(ui.ctx(), state, out);
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        texts
    }

    fn with_speed() -> ViewState {
        ViewState { speeds: TrackSpeeds { enabled: true, chosen: Arc::new(HashMap::new()) }, ..ViewState::default() }
    }

    /// With the speed of tracks off the sheet is the equalizer alone: its
    /// name, a name per band, and nothing sent untouched.
    #[test]
    fn without_a_speed_to_set_the_sheet_is_the_equalizer() {
        let ctx = context();
        let state = ViewState::default();
        let mut out = Vec::new();
        open(&ctx, Tab::Speed, Some((TrackId::new(), "Песня".to_owned())));
        let texts = draw(&ctx, &mut 0.0, &state, &mut out);
        assert!(out.is_empty(), "{out:?}");
        assert!(texts.iter().any(|t| t == "Эквалайзер"), "{texts:?}");
        for name in equalizer::BAND_NAMES {
            assert!(texts.iter().any(|t| t == name), "no «{name}» in {texts:?}");
        }
        assert!(!texts.iter().any(|t| t == "Сохранять тембр голоса"), "{texts:?}");
    }

    /// «Скорость» of a library track: its title, its speed written out, the
    /// timbre's row, and nothing sent untouched.
    #[test]
    fn the_speed_of_a_track_is_written_out() {
        let ctx = context();
        let track = TrackId::new();
        let mut state = with_speed();
        let mut out = Vec::new();
        let mut clock = 0.0;
        open(&ctx, Tab::Speed, Some((track, "Песня".to_owned())));
        let texts = draw(&ctx, &mut clock, &state, &mut out);
        assert!(out.is_empty(), "{out:?}");
        for text in ["Песня", "×1.0", "Сохранять тембр голоса"] {
            assert!(texts.iter().any(|t| t == text), "no «{text}» in {texts:?}");
        }
        assert!(!texts.iter().any(|t| t == "32"), "the bands are the other tab's: {texts:?}");

        // Another version chosen for it: the sheet follows.
        state.speeds.chosen = Arc::new(HashMap::from([(track, SpeedVariant::Custom(115))]));
        let texts = draw(&ctx, &mut clock, &state, &mut out);
        assert!(out.is_empty(), "{out:?}");
        assert!(texts.iter().any(|t| t == "×1.15"), "{texts:?}");
    }

    /// S and E: open on the tab, move to the other, put the sheet away.
    #[test]
    fn a_tabs_key_opens_moves_and_closes() {
        let ctx = Context::default();
        let read = || ctx.data(|d| d.get_temp::<Sheet>(key())).map(|s| (s.open, s.tab));
        // Nothing plays: no speed to set, the equalizer opens all the same.
        let state = with_speed();
        toggle(&ctx, Tab::Speed, &state);
        assert_eq!(read(), None);
        toggle(&ctx, Tab::Equalizer, &state);
        assert_eq!(read(), Some((true, Tab::Equalizer)));
        // The equalizer alone: either key puts it away.
        toggle(&ctx, Tab::Speed, &state);
        assert_eq!(read(), Some((false, Tab::Equalizer)));

        // With a track to be of, the keys move between the tabs.
        open(&ctx, Tab::Speed, Some((TrackId::new(), String::new())));
        toggle(&ctx, Tab::Equalizer, &state);
        assert_eq!(read(), Some((true, Tab::Equalizer)));
        toggle(&ctx, Tab::Speed, &state);
        assert_eq!(read(), Some((true, Tab::Speed)));
        toggle(&ctx, Tab::Speed, &state);
        assert_eq!(read(), Some((false, Tab::Speed)));
        assert_eq!(first_tab(&state), Tab::Equalizer);
    }
}
