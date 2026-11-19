//! «Музыка»: the sound, the queue, the recommendations and what is not
//! liked. No stage: a preset here shows what it changes in its rows.

use egui::Ui;
use nikgob_core::i18n;
use nikgob_view::presets::Area;
use nikgob_view::{ListKind, Prefs, RadioKind, RadioNewShare, Route, ViewState};

use super::super::settings_help::SettingId as Id;
use super::preset;
use super::rows::{self, Note, Num, child, dimmed, group, menu, row, row_note, sub, switch, switch_enabled};
use super::super::PageState;
use crate::components::{Button, Size, Style, Switch};

pub(super) fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState) {
    sound(ui, state, p);

    group(ui, i18n::common().queue);
    switch(ui, Id::Autoplay, &mut p.autoplay);
    let autoplay = p.autoplay;
    child(ui, !autoplay, |ui| switch(ui, Id::AutoplayWorld, &mut p.autoplay_continue_world));
    menu(ui, Id::SearchPlay, p);
    menu(ui, Id::RadioNoRepeat, p);
    // How a new list starts, by its kind; radio and autoplay on their own.
    sub(ui, i18n::settings_tabs().g_shuffle);
    for kind in ListKind::ALL {
        menu(ui, Id::ListShuffle(kind), p);
    }
    switch(ui, Id::ShuffleRadio, &mut p.shuffle_radio);

    albums(ui, p);
    recommendations(ui, state, p);
    dislikes(ui, p, pages);
}

/// «Звук»: the device, gaps, crossfade, loudness, the equalizer, the speed
/// of tracks.
fn sound(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    let st = i18n::settings_tabs();
    group(ui, st.g_sound);
    row(ui, Id::OutputDevice, |ui| {
        let default = st.system_default;
        let mut options = vec![(default.to_owned(), p.output_device.is_none())];
        options.extend(p.output_devices.iter().map(|d| (d.clone(), p.output_device.as_deref() == Some(d.as_str()))));
        let cur = p.output_device.as_deref().unwrap_or(default).to_owned();
        if let Some(n) = rows::dropdown(ui, "set-device", &cur, &options) {
            p.output_device = if n == 0 { None } else { p.output_devices.get(n - 1).cloned() };
        }
    });
    // A track Spotify Soloist plays is a live stream of another player:
    // nothing can overlap it.
    let soloist = state.player.soloist;
    dimmed(ui, soloist, |ui| {
        switch_enabled(ui, Id::Gapless, &mut p.gapless, !soloist);
        ui.add_enabled_ui(!soloist, |ui| {
            if let Some(v) = rows::slider(ui, Id::Crossfade, p.crossfade_s, Num::new(0.0, 12.0, 1.0, 0, st.suffix_sec)) {
                p.crossfade_s = v;
            }
        });
    });
    switch(ui, Id::Normalization, &mut p.normalization);
    row(ui, Id::Equalizer, |ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.add(Switch::new(&mut p.eq_on).id_salt(("set-switch", Id::Equalizer)));
        if ui.add(Button::new(st.configure).style(Style::Text).id_salt("set-eq-open")).clicked() {
            super::super::sound::open(ui.ctx(), super::super::sound::Tab::Equalizer, None);
        }
    });
    switch(ui, Id::TrackSpeed, &mut p.speed_enabled);
    let speed = p.speed_enabled;
    child(ui, !speed, |ui| switch(ui, Id::KeepTimbre, &mut p.keep_timbre));
}


/// «Песни из альбомов»: what an album shows and plays (added to the library: whole,
/// not added: the user's songs of it, or always one of them), and whether
/// the other versions of its songs play with it. All «Подробно»; the
/// switch of an album's «Другие версии» wins.
fn albums(ui: &mut Ui, p: &mut Prefs) {
    group(ui, i18n::settings_tabs().g_album_songs);
    menu(ui, Id::AlbumOpens, p);
    switch(ui, Id::AlbumVersions, &mut p.album_other_versions);
}

/// «Рекомендации»: how much new music, from where, how much of it at
/// random, the unheard, the listeners' graph.
fn recommendations(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    group(ui, i18n::settings_tabs().g_recs);
    // While a preset is under the pointer the rows show what it would give.
    let mut preview = preset::row(ui, Area::Recs, p);
    let p: &mut Prefs = preview.as_mut().unwrap_or(p);
    switch(ui, Id::MixesDownloadedOnly, &mut p.mixes_downloaded_only);
    let downloaded = p.mixes_downloaded_only;
    dimmed(ui, downloaded, |ui| {
        row(ui, Id::RadioNewShare, |ui| {
            ui.add_enabled_ui(!downloaded, |ui| rows::choice(ui, Id::RadioNewShare, p));
        });
        child(ui, false, |ui| {
            for kind in [RadioKind::Track, RadioKind::Artist, RadioKind::Mix, RadioKind::Autoplay] {
                let id = Id::NewShareBy(kind);
                row(ui, id, |ui| {
                    ui.add_enabled_ui(!downloaded, |ui| rows::choice_menu(ui, id, p));
                });
            }
        });
        row(ui, Id::MixesWithNew, |ui| {
            ui.add_enabled(!downloaded, crate::components::Switch::new(&mut p.mixes_with_new).id_salt(("set-switch", Id::MixesWithNew)));
        });
    });
    // Random picks come only within the share of new music: with no new
    // music in any kind of radio the row has nothing to do.
    let no_new = p.mixes_downloaded_only
        || [RadioKind::Track, RadioKind::Artist, RadioKind::Mix, RadioKind::Autoplay]
            .into_iter()
            .all(|kind| p.new_share_of(kind) == RadioNewShare::None);
    dimmed(ui, no_new, |ui| {
        row(ui, Id::RadioExplore, |ui| {
            ui.add_enabled_ui(!no_new, |ui| rows::choice_menu(ui, Id::RadioExplore, p));
        });
    });
    menu(ui, Id::RadioUnheard, p);
    switch(ui, Id::GraphUse, &mut p.graph_use);
    menu(ui, Id::MixCardsWave, p);
    sound_model(ui, state, p);
}

/// «Нейросетевой анализ звука» and where it runs: the download's (or the
/// library's) progress under the menu while it goes, an error when the
/// model could not be had.
fn sound_model(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    let m = &state.sound_model;
    let mode = p.sound_neural_or(m.ready);
    let progress = match m.downloading {
        Some(pct) => Some(f32::from(pct) / 100.0),
        None => (mode.library() && m.ready && m.total > 0 && m.done < m.total).then(|| m.done as f32 / m.total as f32),
    };
    let note = (mode.library() && m.failed).then_some(Note::Error(i18n::settings_tabs().model_failed));
    row_note(ui, Id::SoundNeural, note, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            rows::choice_menu(ui, Id::SoundNeural, p);
            if let Some(f) = progress {
                crate::components::wavy_progress(ui, ("set-progress", Id::SoundNeural), f, 96.0);
            }
        });
    });
    let off = !mode.library();
    dimmed(ui, off, |ui| {
        row(ui, Id::SoundDevice, |ui| {
            ui.add_enabled_ui(!off, |ui| rows::choice_menu(ui, Id::SoundDevice, p));
        });
    });
}

/// «Не нравится»: the button, the skips, and the page of the marks (the
/// speeds of tracks too).
fn dislikes(ui: &mut Ui, p: &mut Prefs, pages: &mut PageState) {
    group(ui, i18n::settings_tabs().g_dislikes);
    switch(ui, Id::DislikeButton, &mut p.dislike_button);
    switch(ui, Id::DislikeSkips, &mut p.dislike_skips);
    switch(ui, Id::DislikeSkipEverywhere, &mut p.dislike_skip_everywhere);
    row(ui, Id::Marks, |ui| {
        if ui.add(Button::new(i18n::common().open).style(Style::Outlined).size(Size::S).id_salt("set-marks")).clicked() {
            pages.pending_nav = Some(Route::Marks);
        }
    });
}
