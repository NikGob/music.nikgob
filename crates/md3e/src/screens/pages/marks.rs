//! «Не нравится и скорость» (Settings → Музыка): the
//! tracks marked «Не нравится», the artists not to recommend and the tracks
//! with a speed of their own, each taken back with one button (the
//! snackbar's «Отменить» puts it back). A part with nothing in it is not
//! drawn.

use egui::Ui;
use nikgob_view::{Command, RecTarget, Route, TrackId, ViewState};

use super::{PageState, widgets};
use crate::components::{Button, ListItem, Style};

/// The page's title (and its button's name in Settings).
pub const TITLE: &str = "Не нравится и скорость";

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    super::detail::with_bar(ui, TITLE, pages, |ui, _| {
        let d = &state.dislikes;
        if !d.tracks.is_empty() {
            widgets::section_header(ui, "Треки с «Не нравится»", false);
            for t in &d.tracks {
                if mark(ui, ("marks-track", t.id), &t.title, Some(&t.artist), "Снять") {
                    out.push(Command::Dislike { track: t.id, dislike: false, sure: false, relike: false });
                }
            }
            ui.add_space(widgets::SECTION_GAP);
        }
        if !d.artists.is_empty() {
            widgets::section_header(ui, "Не рекомендовать исполнителей", false);
            for (id, name) in &d.artists {
                if mark(ui, ("marks-artist", *id), name, None, "Снять") {
                    out.push(Command::RecFeedback { target: RecTarget::Artist(*id), dislike: false });
                }
            }
            ui.add_space(widgets::SECTION_GAP);
        }
        let speeds = speed_rows(state);
        if !speeds.is_empty() {
            widgets::section_header(ui, "Своя скорость", false);
            for (id, title, sub) in &speeds {
                if mark(ui, ("marks-speed", *id), title, Some(sub), "Сбросить") {
                    out.push(Command::ResetTrackSpeed(*id));
                }
            }
            ui.add_space(widgets::SECTION_GAP);
        }
        None
    })
}

/// The tracks with a speed of their own, by title: the id, the title, and
/// the artists with the speed («Artist · ×1,2»).
fn speed_rows(state: &ViewState) -> Vec<(TrackId, String, String)> {
    let mut rows: Vec<(TrackId, String, String)> = state
        .speeds
        .chosen
        .iter()
        .filter_map(|(id, variant)| {
            let name = state.speed_names.get(id)?;
            let speed = variant.chip()?;
            let sub = if name.artist.is_empty() { speed.into_owned() } else { format!("{} · {speed}", name.artist) };
            Some((*id, name.title.clone(), sub))
        })
        .collect();
    rows.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then(a.0.cmp(&b.0)));
    rows
}

/// One mark: its name, under it the artists (and the speed), and the button
/// that takes it back. True when that button was clicked.
fn mark(ui: &mut Ui, salt: impl egui::AsId, title: &str, sub: Option<&str>, label: &str) -> bool {
    let salt = egui::Id::new(salt);
    let mut item = ListItem::new(title).id_salt(salt);
    if let Some(sub) = sub {
        item = item.supporting(sub);
    }
    let mut clicked = false;
    item.trailing(|ui| {
        clicked = ui.add(Button::new(label).style(Style::Text).id_salt(salt.with("take-back"))).clicked();
    })
    .show(ui);
    clicked
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use nikgob_view::{DislikedTrack, SpeedVariant, TrackSpeeds};

    use super::*;

    #[test]
    fn speeds_are_listed_by_title_with_their_speed() {
        let (a, b, c) = (TrackId::new(), TrackId::new(), TrackId::new());
        let name = |id, title: &str, artist: &str| (id, DislikedTrack { id, title: title.into(), artist: artist.into() });
        let state = ViewState {
            speeds: TrackSpeeds {
                enabled: true,
                chosen: Arc::new(HashMap::from([(a, SpeedVariant::Slow), (b, SpeedVariant::Fast), (c, SpeedVariant::Fast)])),
            },
            // `c` has no name (gone from the library): not listed.
            speed_names: Arc::new(HashMap::from([name(a, "Б", "X"), name(b, "а", "")])),
            ..ViewState::default()
        };
        let rows = speed_rows(&state);
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(rows[1].2, "X · замедленно");
        assert!(!rows[0].2.is_empty() && !rows[0].2.starts_with(' '));
    }
}
