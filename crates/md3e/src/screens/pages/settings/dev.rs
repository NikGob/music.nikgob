//! «Для разработчика»: the owner's own builds, in «Подробно» only (every
//! row here is [`Level::Dev`](super::super::settings_help::Level)). Where
//! the lyrics' highlight is made and where the models run, the services'
//! keys, the taste profiles' and the similar music's switches, the
//! separate profile for videos and UI tests, and the first run shown again.

use egui::Ui;
use nikgob_core::i18n;
use nikgob_view::{Command, Prefs, ViewState};

use super::super::PageState;
use super::super::settings_help::SettingId as Id;
use super::rows::{Note, block, group, row, row_note, segments, switch};
use crate::{
    Icon,
    components::{Button, Dialog, Style, TextField},
};

pub(super) fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    group(ui, st.g_dev);
    segments(ui, Id::LyricsUi, p);
    // The GPU is DirectML's, Windows' own: elsewhere the CPU is the only
    // choice, so there is none to show. A GPU that cannot run says so.
    if cfg!(windows) {
        let fallback = (state.align.on_gpu == Some(false) && p.lyrics_gpu).then_some(Note::Error(st.gpu_unavailable));
        row_note(ui, Id::LyricsGpu, fallback, |ui| super::rows::choice(ui, Id::LyricsGpu, p));
    }
    switch(ui, Id::LyricsModelsAuto, &mut p.lyrics_models_auto);

    metadata(ui, p, pages, out);
    keys(ui, p, pages, out);

    switch(ui, Id::BackgroundCountsLess, &mut p.background_counts_less);
    switch(ui, Id::ProfilesSplit, &mut p.profiles_split);
    switch(ui, Id::GraphServices, &mut p.graph_services);
    switch(ui, Id::GraphOpenData, &mut p.graph_open_data);
    switch(ui, Id::GraphPersonal, &mut p.graph_personal);
    switch(ui, Id::LibraryAsTaste, &mut p.library_as_taste);
    switch(ui, Id::DislikeInRecs, &mut p.dislike_in_recs);

    // Never inside the profile itself.
    if !nikgob_view::dev::profile() {
        demo_profile(ui, out);
    }
    // «Показать»: the first run over the whole window, as on a new install.
    row(ui, Id::FirstRun, |ui| {
        if ui.add(Button::new(st.show).style(Style::Tonal).id_salt("set-dev-first-run")).clicked() {
            out.push(Command::SetPref(nikgob_view::PrefPatch::FirstRun(true)));
        }
    });
}

/// «Контакт MusicBrainz» and «Ключ AcoustID», saved together.
fn metadata(ui: &mut Ui, p: &Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    if !pages.metadata_synced {
        pages.metadata_contact = p.metadata_contact.clone();
        pages.metadata_synced = true;
    }
    block(ui, Id::MusicBrainz, |ui| {
        TextField::new(&mut pages.metadata_contact, Id::MusicBrainz.label())
            .placeholder(st.contact_placeholder)
            .leading_icon(Icon::Person)
            .width(480.0)
            .id_salt("set-mb-contact")
            .show(ui);
    });
    block(ui, Id::AcoustId, |ui| {
        let mut field = TextField::new(&mut pages.acoustid_key, Id::AcoustId.label()).password(true);
        if p.acoustid_configured {
            field = field.placeholder(st.saved);
        }
        field.width(480.0).id_salt("set-acoustid").show(ui);
        ui.add_space(8.0);
        let contact_changed = pages.metadata_contact.trim() != p.metadata_contact.trim();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(
                    Button::new(i18n::common().save)
                        .style(Style::Tonal)
                        .enabled(contact_changed || !pages.acoustid_key.trim().is_empty())
                        .id_salt("set-metadata-save"),
                )
                .clicked()
            {
                let key = std::mem::take(&mut pages.acoustid_key).trim().to_owned();
                out.push(Command::SetMetadata { contact: pages.metadata_contact.trim().to_owned(), acoustid_key: (!key.is_empty()).then_some(key) });
            }
            // An empty key leaves the saved one; this takes it away (the
            // runtime clears it on an empty key), the contact as saved.
            if p.acoustid_configured && ui.add(Button::new(st.remove_key).style(Style::Text).id_salt("set-acoustid-clear")).clicked() {
                pages.acoustid_key.clear();
                out.push(Command::SetMetadata { contact: p.metadata_contact.trim().to_owned(), acoustid_key: Some(String::new()) });
            }
        });
    });
}

/// «Ключ Last.fm» and «Токен ListenBrainz», each saved or taken away on
/// its own (kept in settings.toml).
fn keys(ui: &mut Ui, p: &Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let st = i18n::settings_tabs();
    block(ui, Id::LastfmKey, |ui| {
        let mut field = TextField::new(&mut pages.lastfm_key, Id::LastfmKey.label()).password(true);
        if p.lastfm_configured {
            field = field.placeholder(st.saved);
        }
        field.width(480.0).id_salt("set-lastfm").show(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new(i18n::common().save).style(Style::Tonal).enabled(!pages.lastfm_key.trim().is_empty()).id_salt("set-lastfm-save")).clicked() {
                out.push(Command::SetLastfmKey(std::mem::take(&mut pages.lastfm_key).trim().to_owned()));
            }
            if p.lastfm_configured && ui.add(Button::new(st.remove_key).style(Style::Text).id_salt("set-lastfm-clear")).clicked() {
                pages.lastfm_key.clear();
                out.push(Command::SetLastfmKey(String::new()));
            }
        });
    });
    block(ui, Id::ListenbrainzToken, |ui| {
        let mut field = TextField::new(&mut pages.listenbrainz_token, Id::ListenbrainzToken.label()).password(true);
        if p.listenbrainz_configured {
            field = field.placeholder(st.saved);
        }
        field.width(480.0).id_salt("set-listenbrainz").show(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(Button::new(i18n::common().save).style(Style::Tonal).enabled(!pages.listenbrainz_token.trim().is_empty()).id_salt("set-listenbrainz-save"))
                .clicked()
            {
                out.push(Command::SetListenbrainzToken(std::mem::take(&mut pages.listenbrainz_token).trim().to_owned()));
            }
            if p.listenbrainz_configured && ui.add(Button::new(st.remove_token).style(Style::Text).id_salt("set-listenbrainz-clear")).clicked() {
                pages.listenbrainz_token.clear();
                out.push(Command::SetListenbrainzToken(String::new()));
            }
        });
    });
}

/// «Демо-профиль»: the separate profile for videos and UI tests
/// (`--demo-profile`), started beside this player or reset after a question.
fn demo_profile(ui: &mut Ui, out: &mut Vec<Command>) {
    use nikgob_view::dev::DevProfile;
    let st = i18n::settings_tabs();
    let asking = egui::Id::new("set-dev-profile-asking");
    row(ui, Id::DemoProfile, |ui| {
        ui.horizontal(|ui| {
            if ui.add(Button::new(i18n::common().open).style(Style::Tonal).id_salt("set-dev-profile-open")).clicked() {
                out.push(Command::DevProfile(DevProfile::Open));
            }
            if ui.add(Button::new(i18n::common().reset).style(Style::Outlined).id_salt("set-dev-profile-reset")).clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(asking, true));
            }
        });
    });
    if ui.ctx().data(|d| d.get_temp::<bool>(asking)).unwrap_or(false) {
        let mut open = true;
        let r = Dialog::new("md3e-dev-profile-reset", st.reset_demo_title)
            .icon(Icon::Refresh)
            .action(i18n::common().cancel)
            .action(i18n::common().reset)
            .show(ui.ctx(), &mut open);
        if r.action == Some(1) {
            out.push(Command::DevProfile(DevProfile::Reset));
        }
        if r.action.is_some() || r.dismissed || !open {
            ui.ctx().data_mut(|d| d.remove::<bool>(asking));
        }
    }
}
