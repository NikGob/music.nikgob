//! The floating player: a vibrant floating toolbar with cover, title,
//! transport, progress and quick toggles (md3e-player.md §2.3).

use std::time::Duration;

use egui::{Rect, Sense, Ui, vec2};
use nikgob_view::{Command, LikeTarget, Repeat, Route, Status, ViewState};

use super::{
    PaneTab,
    chrome::{self, Act, Chrome, WheelZone, Where},
    common,
};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        IconButton, Size, Slider, Style, ToolbarColor, button::Colors, floating_toolbar, toolbar,
    },
};

pub struct BarOutput {
    pub open_now_playing: bool,
    pub toggle_pane: Option<PaneTab>,
}

/// Draws the player in `rect` (64 tall). `medium` = Medium size class.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    state: &ViewState,
    pane: Option<PaneTab>,
    medium: bool,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) -> BarOutput {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let p = &state.player;
    let mut result = BarOutput {
        open_now_playing: false,
        toggle_pane: None,
    };
    let item = toolbar::item_colors(ToolbarColor::Vibrant, &s);
    // Narrow pills drop the extras (they live in Now Playing) and show a
    // thin progress line instead of the slider.
    let wide = !medium && rect.width() >= 820.0;
    let compact = medium || rect.width() < 700.0;
    let (bar_bg, on) = toolbar::colors(ToolbarColor::Vibrant, &s);
    chrome.set_volume_button(None);
    if !chrome.np_covering {
        chrome.volume_zone(rect, WheelZone::Bar);
    }
    // The hero button must stand out from the pill whatever the palette:
    // primary on primary-container can be nearly the same tone.
    let play_colors = if crate::color::contrast_ratio(s.primary, bar_bg) >= 2.5 {
        Colors { container: s.primary, content: s.on_primary, border: egui::Color32::TRANSPARENT }
    } else {
        Colors { container: on, content: bar_bg, border: egui::Color32::TRANSPARENT }
    };
    let mut title_rect = Rect::NOTHING;
    let mut no_slider = false;

    let (pill, ()) = floating_toolbar(ui, rect, ToolbarColor::Vibrant, |ui| {
        // Cover + title: opens Now Playing.
        let info_w = if wide { 220.0 } else { 172.0 };
        let (info_rect, info) = ui.allocate_exact_size(vec2(info_w, 48.0), Sense::click());
        let cover = Rect::from_min_size(info_rect.min, vec2(48.0, 48.0));
        let current = p.current.as_ref();
        common::cover(ui, current.and_then(|t| t.cover.as_ref()), cover, 24.0);
        let text_rect = Rect::from_min_max(
            info_rect.min + vec2(56.0, 4.0),
            info_rect.max - vec2(0.0, 4.0),
        );
        title_rect = text_rect;
        let mut tui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(text_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        tui.spacing_mut().item_spacing.y = 0.0;
        match current {
            Some(t) => {
                common::label(&mut tui, &t.title, TypeRole::TitleSmall, on);
                // The artist opens their page; the rest of the block opens
                // Now Playing.
                let artist = tui.add(
                    egui::Label::new(TypeRole::BodySmall.style().rich(&t.artist, on.gamma_multiply(0.8)))
                        .truncate()
                        .selectable(false)
                        .sense(Sense::click()),
                );
                if let Some(id) = t.artist_id {
                    let artist = artist.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if artist.clicked() {
                        chrome.nav = Some(Route::Artist(id));
                    }
                }
            }
            None => {
                common::label(&mut tui, "Ничего не играет", TypeRole::TitleSmall, on);
            }
        }
        if info.clicked() && current.is_some() {
            result.open_now_playing = true;
        }

        ui.add_space(8.0);
        if ui
            .add(IconButton::new(Icon::SkipPrevious).colors(item).tooltip("Назад (Ctrl+←)"))
            .clicked()
        {
            chrome::press_previous(ui.ctx(), state, out);
        }
        let playing = p.status == Status::Playing;
        let buffering = p.status == Status::Buffering;
        // Play/pause: filled toggle M; playing = square, paused = round.
        // A stream that is still opening shows a loading indicator instead.
        let play = ui.add(
            IconButton::new(Icon::PlayArrow)
                .selected_icon(Icon::Pause)
                .size(Size::M)
                .style(Style::Filled)
                .selected(playing || buffering)
                .colors(play_colors)
                .filled_icon(true)
                .tooltip(if buffering { "Загружается… (Space — пауза)" } else { "Пауза / играть (Space)" })
                .id_salt("bar-play"),
        );
        if buffering {
            common::buffering_overlay(ui, play.rect, play_colors.container, play_colors.content);
        }
        if play.clicked() {
            out.push(Command::TogglePlay);
        }
        if ui
            .add(IconButton::new(Icon::SkipNext).colors(item).tooltip("Далее (Ctrl+→)"))
            .clicked()
        {
            out.push(Command::Next);
        }
        // On (the whole queue or one track) it sits on a tinted disc: the
        // pill's flat item colours alone look the same either way.
        let repeat_colors = if p.repeat == Repeat::Off {
            item
        } else {
            Colors { container: on.gamma_multiply(0.24), ..item }
        };
        if ui
            .add(
                IconButton::new(repeat_icon(p.repeat))
                    .selected(p.repeat != Repeat::Off)
                    .colors(repeat_colors)
                    .tooltip(repeat_tip(p.repeat))
                    .id_salt("bar-repeat"),
            )
            .clicked()
        {
            out.push(Command::CycleRepeat);
        }
        ui.add_space(8.0);

        // Progress: label, slider, label; a bare slider when narrower; a
        // line under the title when there is no room at all.
        let trailing_w = if wide { 5.0 * 44.0 } else if compact { 2.0 * 44.0 } else { 3.0 * 44.0 };
        let avail = ui.available_width() - trailing_w - 8.0;
        if compact && avail >= 112.0 {
            let dur = p.duration.unwrap_or_default();
            let out_s = Slider::new("bar-progress", p.fraction_now())
                .width(avail - 8.0)
                .track(8.0)
                .wavy(playing && state.prefs.progress_wave)
                .colors(on, on.gamma_multiply(0.24))
                .value_label(move |v| common::fmt_time(dur.mul_f32(v)))
                .show(ui);
            if let Some(v) = out_s.committed {
                out.push(Command::Seek(dur.mul_f32(v)));
            }
            ui.add_space(4.0);
        } else if compact {
            no_slider = true;
            ui.add_space(avail.max(0.0));
        } else if avail > 160.0 {
            let pos = p.position_now();
            let dur = p.duration.unwrap_or_default();
            time_label(ui, pos, on);
            let slider_w = avail - 2.0 * 44.0;
            let out_s = Slider::new("bar-progress", p.fraction_now())
                .width(slider_w)
                .wavy(playing && state.prefs.progress_wave)
                .colors(on, on.gamma_multiply(0.24))
                .value_label(move |v| common::fmt_time(dur.mul_f32(v)))
                .show(ui);
            if let Some(v) = out_s.committed {
                out.push(Command::Seek(dur.mul_f32(v)));
            }
            time_label(ui, dur, on);
        } else {
            ui.add_space(avail.max(0.0));
        }

        let (show_like, show_lyrics, show_volume) = (!compact, wide, wide);
        if show_like {
            let liked = current.is_some_and(|t| t.liked);
            if ui
                .add(
                    IconButton::new(Icon::Favorite)
                        .selected(liked)
                        .colors(item)
                        .tooltip("Нравится (Ctrl+L)")
                        .id_salt("bar-like"),
                )
                .clicked()
                && let Some(t) = current
            {
                out.push(Command::Like(LikeTarget::Track(t.id), !liked));
            }
        }
        if ui
            .add(
                IconButton::new(Icon::QueueMusic)
                    .selected(pane == Some(PaneTab::Queue))
                    .colors(item)
                    .tooltip("Очередь (Ctrl+Q)")
                    .id_salt("bar-queue"),
            )
            .clicked()
        {
            result.toggle_pane = Some(PaneTab::Queue);
        }
        if show_lyrics
            && ui
                .add(
                    IconButton::new(Icon::Lyrics)
                        .selected(pane == Some(PaneTab::Lyrics))
                        .colors(item)
                        .tooltip("Текст")
                        .id_salt("bar-lyrics"),
                )
                .clicked()
        {
            result.toggle_pane = Some(PaneTab::Lyrics);
        }
        if show_volume {
            let level = chrome.volume_now(ui.ctx(), state);
            let mut btn = IconButton::new(Icon::volume(level, p.muted))
                .colors(item)
                .id_salt("bar-volume");
            if !chrome.volume_open() {
                btn = btn.tooltip("Громкость: наведи или крути колесо");
            }
            let vol = ui.add(btn);
            if !chrome.np_covering {
                chrome.set_volume_button(Some(vol.rect));
            }
            if vol.clicked() {
                out.push(Command::ToggleMute);
            }
        }
        let more = ui.add(
            IconButton::new(Icon::MoreVert)
                .colors(item)
                .tooltip("Ещё")
                .id_salt("bar-more"),
        );
        if let Some(t) = current {
            // What does not fit in the pill lives in the menu.
            let mut acts = Vec::new();
            acts.extend((!show_like).then_some(Act::Like));
            acts.extend((!show_lyrics).then_some(Act::Lyrics));
            acts.extend((!show_volume).then_some(Act::Volume));
            if !acts.is_empty() {
                acts.push(Act::Sep);
            }
            acts.extend([Act::Playlist, Act::Artist, Act::Album, Act::Info, Act::Share, Act::Sep, Act::Tags, Act::Replace]);
            chrome.track_menu(&more, "bar-more", &acts, t, state, Where::Bar, out);
        }
    });
    // A click on the pill between the controls opens Now Playing too.
    if pill.clicked() && p.current.is_some() {
        result.open_now_playing = true;
    }

    // No room for a slider: a 3 dp line under the title.
    if no_slider && title_rect.is_positive() {
        let f = p.fraction_now();
        let y = title_rect.bottom();
        let (x0, x1) = (title_rect.left(), title_rect.right() - 8.0);
        let stroke = |c| egui::Stroke::new(3.0, c);
        ui.painter().line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke(on.gamma_multiply(0.24)));
        if f > 0.0 {
            ui.painter().line_segment([egui::pos2(x0, y), egui::pos2(x0 + (x1 - x0) * f, y)], stroke(on));
        }
    }
    if matches!(p.status, Status::Playing | Status::Buffering) {
        // Labels tick once a second; the wave asks for frames itself.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    result
}

/// The repeat button's glyph: a "1" on it while one track repeats.
/// What the repeat button does now and on the next press.
pub fn repeat_tip(repeat: Repeat) -> &'static str {
    match repeat {
        Repeat::Off => "Повтор выключен — нажми, чтобы повторять очередь",
        Repeat::All => "Повторяется вся очередь — нажми, чтобы повторять один трек",
        Repeat::One => "Повторяется этот трек — нажми, чтобы выключить",
    }
}

pub fn repeat_icon(repeat: Repeat) -> Icon {
    if repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat }
}

fn time_label(ui: &mut Ui, d: Duration, color: egui::Color32) {
    let text = common::fmt_time(d);
    ui.add_sized(
        vec2(44.0, 20.0),
        egui::Label::new(TypeRole::LabelMedium.style().rich(text, color)).selectable(false),
    );
}
