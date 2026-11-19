//! «Система»: who the player greets, the start, the load it puts on the
//! computer, notifications, the library's folders, and «Для разработчика».

use egui::Ui;
use nikgob_core::i18n;
use nikgob_view::{Command, Prefs, Service, ViewState};

use super::super::PageState;
use super::super::settings_help::SettingId as Id;
use super::preset;
use super::rows::{self, Note, Num, child, group, block, row_note, segments, switch};
use crate::{
    Icon,
    components::{Button, Style, Switch, TextField},
};

pub(super) fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    group(ui, st.g_you);
    user_name(ui, p, pages, out);

    group(ui, st.g_startup);
    switch(ui, Id::Autostart, &mut p.autostart);
    let autostart = p.autostart;
    child(ui, !autostart, |ui| switch(ui, Id::AutostartHidden, &mut p.autostart_hidden));

    load(ui, state, p);

    group(ui, st.g_notifications);
    segments(ui, Id::ToastTime, p);
    switch(ui, Id::DownloadToasts, &mut p.download_toasts);

    library(ui, p, out);

    super::dev::show(ui, state, p, pages, out);
}

/// «Как к тебе обращаться»: kept when the field is left or Enter pressed.
fn user_name(ui: &mut Ui, p: &Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    if !pages.name_synced {
        pages.name_draft = p.user_name.clone();
        pages.name_synced = true;
    }
    block(ui, Id::UserName, |ui| {
        let field = TextField::new(&mut pages.name_draft, Id::UserName.label())
            .placeholder(i18n::settings_tabs().name_hint)
            .leading_icon(Icon::Person)
            .width(480.0)
            .id_salt("set-user-name")
            .show(ui);
        let name = nikgob_view::user_name(&pages.name_draft);
        if (field.submitted || field.response.lost_focus()) && name != p.user_name {
            out.push(Command::SetPref(nikgob_view::PrefPatch::UserName(name.clone())));
            pages.name_draft = name;
        }
    });
}

/// «Нагрузка»: frames, the tray, downloads at once, the analysis of
/// streams, the voice models' heavy work. Background work shows how far it
/// is as a bar under its switch, without words.
fn load(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    group(ui, i18n::settings_tabs().g_load);
    // Тихо · Обычно · Быстро · Свои: while one is under the pointer the rows
    // show what it would give.
    let mut preview = preset::row(ui, nikgob_view::presets::Area::Load, p);
    let p: &mut Prefs = preview.as_mut().unwrap_or(p);
    if let Some(v) = rows::slider(ui, Id::FpsLimit, p.fps_limit as f32, Num::new(30.0, 144.0, 1.0, 0, "")) {
        p.fps_limit = v as u32;
    }
    // Where the window cannot hide (Wayland) it always closes to the tray
    // and is made again: the switch would do nothing.
    if super::super::window_can_hide(ui.ctx()) {
        switch(ui, Id::TrayKeepWindow, &mut p.tray_keep_window);
    }
    let n = p.auto_download_limit.clamp(1, 8) as f32;
    if let Some(v) = rows::slider(ui, Id::AutoDownloads, n, Num::new(1.0, 8.0, 1.0, 0, "")) {
        p.auto_download_limit = v as u32;
    }
    let a = &state.analysis;
    let youtube = state.account(Service::Youtube).is_none_or(|a| a.enabled);
    let streams = (p.stream_analysis && youtube && a.streams_total > 0 && a.streams_done < a.streams_total)
        .then(|| a.streams_done as f32 / a.streams_total as f32);
    // It takes the streams' sound from YouTube Music.
    let blocked = (p.stream_analysis && !youtube).then_some(Note::Blocker(i18n::settings_tabs().need_youtube));
    rows::switch_with_progress(ui, Id::StreamAnalysis, &mut p.stream_analysis, streams, blocked);
    switch(ui, Id::LyricsSeparate, &mut p.lyrics_separate);
    switch(ui, Id::LyricsDeep, &mut p.lyrics_deep);
    let al = &state.align;
    let words = (p.lyrics_words_all && al.total > 0 && al.done < al.total).then(|| al.done as f32 / al.total as f32);
    rows::switch_with_progress(ui, Id::LyricsWordsAll, &mut p.lyrics_words_all, words, None);
}

/// «Библиотека»: the folders (the path is the row's value) with «Открыть»,
/// the FLAC archive's move, the Trash, the export.
fn library(ui: &mut Ui, p: &mut Prefs, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    group(ui, i18n::nav().library);
    let library = p.library_dir.display().to_string();
    row_note(ui, Id::LibraryDir, Some(Note::Value(&library)), |ui| {
        if ui.add(Button::new(i18n::common().open).style(Style::Outlined).id_salt("open-lib")).clicked() {
            let _ = rows::open_folder(&p.library_dir);
        }
    });
    let masters_dir = if p.masters_dir.as_os_str().is_empty() { p.library_dir.join(".musiclib").join("masters") } else { p.masters_dir.clone() };
    let masters = masters_dir.display().to_string();
    row_note(ui, Id::MastersDir, Some(Note::Value(&masters)), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new(st.move_btn).style(Style::Tonal).id_salt("move-masters")).clicked() {
                super::super::dialogs::folder(|d| d.set_title(st.masters_dir_title), Command::MoveMastersDir, out);
            }
            if ui.add(Button::new(i18n::common().open).style(Style::Outlined).id_salt("open-masters")).clicked() {
                let _ = rows::open_folder(&masters_dir);
            }
        });
    });
    let inbox = p.inbox_dir.display().to_string();
    row_note(ui, Id::Inbox, Some(Note::Value(&inbox)), |ui| {
        if ui.add(Button::new(i18n::common().open).style(Style::Outlined).id_salt("open-inbox")).clicked() {
            let _ = rows::open_folder(&p.inbox_dir);
        }
    });
    let trash = p.library_dir.parent().map_or_else(|| p.library_dir.join("Trash"), |d| d.join("Trash"));
    row_note(ui, Id::Trash, None, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.add(Switch::new(&mut p.use_trash).id_salt(("set-switch", Id::Trash)));
            if ui.add(Button::new(st.open_folder).style(Style::Outlined).id_salt("open-trash")).clicked() {
                let _ = rows::open_folder(&trash);
            }
        });
    });
    let userdata = p.library_dir.join(".musiclib").join("userdata");
    row_note(ui, Id::ExportData, None, |ui| {
        if ui.add(Button::new(st.open_folder).style(Style::Outlined).id_salt("open-userdata")).clicked() {
            let _ = rows::open_folder(&userdata);
        }
    });
}
