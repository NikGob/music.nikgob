//! «Тексты»: how the lyrics look, where texts come from, what lights up by
//! itself, and the models that listen to the voice. The stage beside the
//! rows shows the lyrics.

use egui::Ui;
use nikgob_view::{Command, LYRICS_BY_LETTERS, Prefs, ViewState};

use super::super::settings_help::SettingId as Id;
use super::preset;
use super::rows::{self, Num, child, dimmed, group, menu, segments, switch};
use super::{out_push, size_label, take_pushed};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, IconButton, Style},
};

pub(super) fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, out: &mut Vec<Command>) {
    look(ui, p);

    group(ui, "Поиск текста");
    switch(ui, Id::LyricsGenius, &mut p.lyrics_genius);
    switch(ui, Id::SaveLyrics, &mut p.save_found_lyrics);
    switch(ui, Id::LyricsCheck, &mut p.lyrics_check);

    group(ui, "Подсветка сама");
    switch(ui, Id::LyricsWordsPlaying, &mut p.lyrics_words_playing);
    switch(ui, Id::LyricsLinesAuto, &mut p.lyrics_lines_auto);
    switch(ui, Id::LyricsStreams, &mut p.lyrics_streams);
    let streams = p.lyrics_streams;
    child(ui, !streams, |ui| segments(ui, Id::LyricsStreamKeep, p));
    switch(ui, Id::LyricsRecognize, &mut p.lyrics_recognize);

    group(ui, "Модели");
    models(ui, state, out);
}

/// «Вид текста»: the animation and how it moves, the size and lead, the
/// lines around, the edges, the pause.
fn look(ui: &mut Ui, p: &mut Prefs) {
    group(ui, "Вид текста");
    // The tiles of `Area::Lyrics` come with its presets (TODO 36); until
    // then the area has none and the row draws nothing.
    let mut preview = preset::row(ui, nikgob_view::presets::Area::Lyrics, p);
    let p: &mut Prefs = preview.as_mut().unwrap_or(p);
    segments(ui, Id::LyricsStyle, p);
    segments(ui, Id::LyricsUnit, p);
    // Focus by letters brightens the whole line: nothing to smooth, nothing
    // lights up by itself.
    let whole_line = p.lyrics_style == 1 && p.lyrics_unit == LYRICS_BY_LETTERS;
    dimmed(ui, whole_line, |ui| {
        if let Some(v) = rows::slider(ui, Id::LyricsSmooth, p.lyrics_smooth, Num::pct(0.0, 1.0, 0.05)) {
            p.lyrics_smooth = v;
        }
        if let Some(v) = rows::slider(ui, Id::LightSpeed, p.lyrics_light_speed, Num::pct(0.0, 1.0, 0.05)) {
            p.lyrics_light_speed = v;
        }
    });
    // The jump is the wave's.
    dimmed(ui, p.lyrics_style != 2, |ui| {
        if let Some(v) = rows::slider(ui, Id::WaveJump, p.lyrics_wave_jump, Num::new(0.0, 2.0, 0.1, 1, "")) {
            p.lyrics_wave_jump = v;
        }
        if let Some(v) = rows::slider(ui, Id::JumpSpeed, p.lyrics_jump_speed, Num::pct(0.5, 2.0, 0.05)) {
            p.lyrics_jump_speed = v;
        }
    });
    if let Some(v) = rows::slider(ui, Id::LyricsScale, p.lyrics_scale, Num::pct(0.8, 1.4, 0.05)) {
        p.lyrics_scale = v;
    }
    if let Some(v) = rows::slider(ui, Id::LyricsLead, p.lyrics_lead, Num::new(-0.5, 0.5, 0.05, 2, " с").signed()) {
        p.lyrics_lead = v;
    }
    if let Some(v) = rows::slider(ui, Id::LyricsDim, p.lyrics_dim, Num::pct(0.0, 1.0, 0.05)) {
        p.lyrics_dim = v;
    }
    switch(ui, Id::LyricsCascade, &mut p.lyrics_cascade);
    switch(ui, Id::LyricsGlow, &mut p.lyrics_glow);
    switch(ui, Id::LyricsSoftEdges, &mut p.lyrics_soft_edges);
    let soft = p.lyrics_soft_edges;
    child(ui, !soft, |ui| {
        if let Some(v) = rows::slider(ui, Id::LyricsSoftDepth, p.lyrics_soft_depth, Num::pct(0.08, 0.4, 0.01)) {
            p.lyrics_soft_depth = v;
        }
        if let Some(v) = rows::slider(ui, Id::LyricsSoftStrength, p.lyrics_soft_strength, Num::pct(0.0, 1.5, 0.05)) {
            p.lyrics_soft_strength = v;
        }
        if let Some(v) = rows::slider(ui, Id::LyricsSoftFade, p.lyrics_soft_fade, Num::pct(0.0, 1.0, 0.05)) {
            p.lyrics_soft_fade = v;
        }
    });
    menu(ui, Id::LyricsGap, p);
    // The waves are the wavy rings'.
    let wavy = matches!(p.lyrics_gap_style, 1 | 3);
    child(ui, false, |ui| {
        dimmed(ui, !wavy, |ui| {
            if let Some(v) = rows::slider(ui, Id::LyricsGapWaves, p.lyrics_gap_waves, Num::new(3.0, 16.0, 1.0, 0, "")) {
                p.lyrics_gap_waves = v;
            }
            if let Some(v) = rows::slider(ui, Id::LyricsGapHeight, p.lyrics_gap_height, Num::pct(0.5, 2.0, 0.05)) {
                p.lyrics_gap_height = v;
            }
        });
        if let Some(v) = rows::slider(ui, Id::LyricsGapSize, p.lyrics_gap_size, Num::pct(0.8, 1.5, 0.05)) {
            p.lyrics_gap_size = v;
        }
    });
    switch(ui, Id::LyricsCountdown, &mut p.lyrics_countdown);
    // Every look and unit falls back but Focus by letters, which is the
    // calm Focus already.
    dimmed(ui, whole_line, |ui| menu(ui, Id::WaveFallback, p));
    if let Some(v) = rows::slider(ui, Id::LyricsReturn, p.lyrics_return_s, Num::new(1.0, 15.0, 1.0, 0, " с")) {
        p.lyrics_return_s = v;
    }
}

/// «Модели»: each with its size and «Скачать» / delete (or how far its
/// download is); a failed download says why, the one text here.
fn models(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let a = &state.align;
    let s = Theme::get(ui.ctx()).scheme;
    rows::titled(ui, Id::LyricsModels, |ui| {
        if let Some(e) = &a.error {
            ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
        }
        ui.add_space(4.0);
        crate::components::segmented_list(ui, a.models.len(), |i| {
            let m = &a.models[i];
            let key = m.key.clone();
            let state = m.state;
            let item = crate::components::ListItem::new(m.label.clone()).supporting(size_label(m.size)).id_salt(("align-model", i));
            item.trailing(move |ui: &mut Ui| match state {
                nikgob_view::ModelState::Ready => {
                    if ui.add(IconButton::new(Icon::Delete).tooltip("Удалить").id_salt(("align-model-del", key.clone()))).clicked() {
                        out_push(ui, Command::AlignModel { key: key.clone(), download: false });
                    }
                }
                // How far it is: the bar, no words.
                nikgob_view::ModelState::Downloading(f) => {
                    crate::components::wavy_progress(ui, ("align-model-dl", key.clone()), f, 96.0);
                }
                nikgob_view::ModelState::Missing => {
                    if ui
                        .add(
                            Button::new("Скачать")
                                .style(Style::Tonal)
                                .size(crate::components::Size::Xs)
                                .id_salt(("align-model-get", key.clone())),
                        )
                        .clicked()
                    {
                        out_push(ui, Command::AlignModel { key: key.clone(), download: true });
                    }
                }
            })
        });
        for c in take_pushed(ui) {
            out.push(c);
        }
    });
}
