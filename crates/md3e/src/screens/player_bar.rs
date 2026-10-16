//! The floating player: a vibrant floating toolbar with cover, title,
//! transport, progress and quick toggles (md3e-player.md §2.3).

use std::time::Duration;

use egui::{Rect, Sense, Ui, vec2};
use nikgob_view::{Command, LikeTarget, Status, ViewState};

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
        common::source_badge(ui, cover, current.and_then(|t| t.source));
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
                common::label(&mut tui, &t.artist, TypeRole::BodySmall, on.gamma_multiply(0.8));
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
        // Play/pause: filled toggle M; playing = square, paused = round.
        if ui
            .add(
                IconButton::new(Icon::PlayArrow)
                    .selected_icon(Icon::Pause)
                    .size(Size::M)
                    .style(Style::Filled)
                    .selected(playing)
                    .colors(play_colors)
                    .filled_icon(true)
                    .tooltip("Пауза / играть (Space)")
                    .id_salt("bar-play"),
            )
            .clicked()
        {
            out.push(Command::TogglePlay);
        }
        if ui
            .add(IconButton::new(Icon::SkipNext).colors(item).tooltip("Далее (Ctrl+→)"))
            .clicked()
        {
            out.push(Command::Next);
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
            let mut btn = IconButton::new(if p.muted || p.volume == 0.0 {
                Icon::VolumeOff
            } else {
                Icon::VolumeUp
            })
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
            acts.extend([Act::Playlist, Act::Artist, Act::Album, Act::Info, Act::Sep, Act::Tags]);
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
    if p.status == Status::Playing {
        // Labels tick once a second; the wave asks for frames itself.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    result
}

fn time_label(ui: &mut Ui, d: Duration, color: egui::Color32) {
    let text = common::fmt_time(d);
    ui.add_sized(
        vec2(44.0, 20.0),
        egui::Label::new(TypeRole::LabelMedium.style().rich(text, color)).selectable(false),
    );
}
