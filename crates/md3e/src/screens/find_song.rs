//! «Найти песню…» / «Другая версия…»: a dialog to replace a clip (or a
//! wrong YouTube match) with a song picked by hand (docs/stage3.md §7).
//!
//! A search field prefilled with the cleaned title and artist, the songs
//! of YouTube Music and Deezer with their length next to the track's own,
//! ▶ to listen first, where to replace as a connected button group
//! («Только здесь» / «И в YouTube Music», the last choice by default) and
//! «Заменить». The runtime owns the results ([`ViewState::find_song`]).

use std::time::{Duration, Instant};

use egui::{Align, Context, Layout, Ui};
use nikgob_view::{Command, FindSongView, RemoteId, SourceKind, Status, TrackRow, ViewState};

use super::common;
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, ConnectedGroup, Dialog, IconButton, Leading, ListItem, LoadingIndicator, Size, Style,
        TextField,
    },
};

/// How long the search text rests before it is searched for.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// What the ⋮ menus call the dialog for `t`, if it applies: a clip gets
/// its song, another stream track that plays from YouTube another version.
pub fn label(t: &TrackRow) -> Option<&'static str> {
    if t.has_local {
        None
    } else if t.clip {
        Some("Найти песню…")
    } else if t.source == Some(SourceKind::Youtube) {
        Some("Другая версия…")
    } else {
        None
    }
}

/// The dialog's own state (the text being typed, the pick, the scope).
#[derive(Default)]
pub struct FindSongUi {
    /// The dialog's [`FindSongView::opened`] these fields belong to.
    opened: u64,
    text: String,
    edited: Option<Instant>,
    picked: Option<RemoteId>,
    youtube: bool,
    /// The song started with ▶.
    preview: Option<RemoteId>,
    /// The last view, drawn while the dialog closes.
    last: Option<FindSongView>,
    /// The field takes the keyboard on the next frame (a fresh dialog).
    focus: bool,
}

impl FindSongUi {
    pub fn show(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
        let open_now = state.find_song.is_some();
        if let Some(f) = &state.find_song {
            if f.opened != self.opened {
                self.opened = f.opened;
                self.text = f.query.clone();
                self.edited = None;
                self.picked = None;
                self.preview = None;
                self.youtube = state.prefs.replace_in_youtube;
                self.focus = true;
            }
            self.last = Some(f.clone());
        }
        let Some(f) = self.last.clone() else { return };
        // The scope can only be YouTube Music when that is possible.
        let youtube = self.youtube && f.youtube_off.is_none();
        // The best result is the default pick.
        if open_now && self.picked.as_ref().is_none_or(|p| !f.results.iter().any(|r| &r.remote == p)) {
            self.picked = f.results.first().map(|r| r.remote.clone());
        }
        // Typing pauses: search.
        if let Some(at) = self.edited {
            let left = DEBOUNCE.saturating_sub(at.elapsed());
            if left.is_zero() {
                self.edited = None;
                out.push(Command::FindSong { track: f.track, query: Some(self.text.trim().to_owned()) });
            } else {
                ctx.request_repaint_after(left);
            }
        }

        let s = Theme::get(ctx).scheme;
        let mut open = open_now;
        let mut replace = false;
        let mut cancel = false;
        let headline = if f.clip { "Найти песню" } else { "Другая версия" };
        let r = Dialog::new("md3e-find-song", headline).max_width(560.0).show_with(ctx, &mut open, |ui| {
            ui.add_space(4.0);
            // What is replaced, with its length to compare.
            let what = if f.clip { "Клип" } else { "Сейчас" };
            let mut line = format!("{what}: {} · {}", f.title, f.artist);
            if let Some(d) = f.duration {
                line.push_str(&format!(" · {}", common::fmt_time(d)));
            }
            ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(line, s.on_surface_variant)).truncate());
            ui.add_space(16.0);
            let field = TextField::new(&mut self.text, "Поиск")
                .leading_icon(Icon::Search)
                .request_focus(std::mem::take(&mut self.focus))
                .id_salt("find-song-query")
                .show(ui);
            if field.response.changed() {
                self.edited = Some(Instant::now());
            }
            if field.submitted {
                self.edited = None;
                out.push(Command::FindSong { track: f.track, query: Some(self.text.trim().to_owned()) });
            }
            ui.add_space(12.0);
            self.results(ui, &f, state, out);
            ui.add_space(16.0);
            ui.label(TypeRole::TitleSmall.style().rich("Где заменить", s.on_surface));
            ui.add_space(8.0);
            let labels = ["Только здесь", "И в YouTube Music"];
            let enabled = [true, f.youtube_off.is_none()];
            if let Some(i) = ConnectedGroup::new("find-song-scope", &labels)
                .size(Size::S)
                .enabled(&enabled)
                .show(ui, usize::from(youtube))
            {
                self.youtube = i == 1;
            }
            ui.add_space(6.0);
            let hint = match (&f.youtube_off, youtube) {
                (Some(why), _) => why.clone(),
                (None, true) => if f.clip {
                    "Клип уйдёт из лайков YouTube Music, лайк получит песня"
                } else {
                    "Лайк в YouTube Music перейдёт на эту песню"
                }
                .to_owned(),
                (None, false) => if f.clip { "Лайк в YouTube Music останется на клипе" } else { "В YouTube Music ничего не изменится" }.to_owned(),
            };
            ui.add(egui::Label::new(TypeRole::BodySmall.style().rich_para(hint, s.on_surface_variant)).wrap());
            ui.add_space(24.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let can = self.picked.is_some() && !f.busy && open_now;
                let label = if f.busy { "Заменяю…" } else { "Заменить" };
                if ui
                    .add(Button::new(label).style(Style::Filled).size(Size::S).enabled(can).id_salt("find-song-replace"))
                    .clicked()
                    && can
                {
                    replace = true;
                }
                if ui.add(Button::new("Отмена").style(Style::Text).size(Size::S).id_salt("find-song-cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        // Closing: the last view fades out, nothing to answer.
        if !open_now {
            return;
        }
        if replace && let Some(with) = self.picked.clone() {
            out.push(Command::ReplaceTrack { track: f.track, with, youtube });
        } else if cancel || r.dismissed {
            out.push(Command::CloseFindSong);
        }
    }

    fn results(&mut self, ui: &mut Ui, f: &FindSongView, state: &ViewState, out: &mut Vec<Command>) {
        let s = Theme::get(ui.ctx()).scheme;
        let height = (ui.ctx().content_rect().height() - 420.0).clamp(160.0, 340.0);
        let quiet = |ui: &mut Ui, text: &str, color| {
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 72.0), Layout::left_to_right(Align::Center), |ui| {
                ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(text, color)).wrap());
            });
        };
        if f.results.is_empty() {
            if f.loading {
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 72.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.add(LoadingIndicator::new().size(32.0));
                    ui.add_space(8.0);
                    ui.label(TypeRole::BodyMedium.style().rich("Ищу песни…", s.on_surface_variant));
                });
            } else if let Some(e) = &f.error {
                quiet(ui, e, s.error);
            } else if f.query.is_empty() {
                quiet(ui, "Напиши название и исполнителя", s.on_surface_variant);
            } else {
                quiet(ui, "Ничего не нашлось — попробуй иначе", s.on_surface_variant);
            }
            return;
        }
        let playing = state.player.status == Status::Playing;
        egui::ScrollArea::vertical()
            .id_salt("find-song-results")
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, t) in f.results.iter().enumerate() {
                    let mut supporting = t.artist.clone();
                    if let Some(a) = &t.album {
                        supporting = format!("{supporting} · {a}");
                    }
                    let picked = self.picked.as_ref() == Some(&t.remote);
                    let previewing = playing
                        && self.preview.as_ref() == Some(&t.remote)
                        && state.player.current.as_ref().is_some_and(|c| c.title == t.title);
                    let dur = t.duration.map(common::fmt_time).unwrap_or_default();
                    let mut play = false;
                    let o = ListItem::new(&t.title)
                        .supporting(supporting)
                        .leading(Leading::Image(t.cover_url.clone(), Icon::MusicNote))
                        .selected(picked)
                        .trailing(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let icon = if previewing { Icon::Pause } else { Icon::PlayArrow };
                                let tip = if previewing { "Пауза" } else { "Послушать" };
                                play = ui
                                    .add(IconButton::new(icon).size(Size::Xs).tooltip(tip).id_salt(("find-song-play", i)))
                                    .clicked();
                                ui.label(TypeRole::BodyMedium.style().rich(dur, s.on_surface_variant));
                            });
                        })
                        .id_salt(("find-song-row", i))
                        .show(ui);
                    if play {
                        if previewing {
                            out.push(Command::TogglePlay);
                        } else {
                            self.preview = Some(t.remote.clone());
                            out.push(Command::PlayStreamTracks {
                                tracks: vec![t.remote.clone()],
                                start: 0,
                                context: "Найти песню".into(),
                            });
                        }
                    } else if o.response.clicked() {
                        self.picked = Some(t.remote.clone());
                    }
                }
            });
    }
}
