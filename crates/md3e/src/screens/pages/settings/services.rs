//! «Сервисы» (docs/stage3.md §0, §3.1, §3.6): Spotify, YouTube Music,
//! SoundCloud and the Deezer catalogue — each its switch and account, in
//! «Подробно» also the fallback sign-ins, Spotify's own key and sound, the
//! import of playlists and history —, the likes sent back, playlist sync,
//! the export files and the stream cache. A service that asked to wait
//! says until when (a blocker); nothing else explains itself.

use egui::{Sense, Stroke, Ui, Widget as _, vec2};
use nikgob_core::streaming::SPOTIFY_PREMIUM_REQUIRED;
use nikgob_view::{
    Account, AccountView, Command, LoginInput, LoginPrompt, Prefs, Service, SoloistAction, SoloistPhase, SoloistStatus, ViewState,
};

use super::super::PageState;
use super::super::settings_help::{self, SettingId as Id};
use super::rows::{self, Note, Num, dimmed, group, item, row, row_note, segments, switch};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Card, Dialog, DialogAction, LoadingIndicator, Size, Style, Switch, TextField},
};

/// Where the user's own Spotify app is made and what it must allow.
const DASHBOARD: &str = "https://developer.spotify.com/dashboard";
const REDIRECT_URI: &str = "http://127.0.0.1:8989/login";
/// Where each user makes their own Spotify Soloist key (Premium only).
const SOLOIST_KEY_PAGE: &str = "https://developer.spotify.com/dashboard/soloist";

/// The step of Soloist's setup a dialog asks about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SoloistDialog {
    #[default]
    None,
    Download,
    Key,
    Pair,
}

pub(super) fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    spotify(ui, state, p, pages, out);
    youtube(ui, state, p, pages, out);
    soundcloud(ui, state, p, pages, out);
    deezer(ui, state, p);

    group(ui, "Лайки");
    switch(ui, Id::PushLikes, &mut p.push_likes);
    segments(ui, Id::AlbumLike, p);
    segments(ui, Id::SingleLike, p);

    group(ui, "Плейлисты");
    segments(ui, Id::PlaylistSync, p);

    imports(ui, p, out);

    group(ui, "Стриминг");
    stream_cache(ui, state, p, out);
}

/// «Кэш стриминга»: its limit per service and «Очистить».
fn stream_cache(ui: &mut Ui, state: &ViewState, p: &mut Prefs, out: &mut Vec<Command>) {
    let used = state.stream_cache_bytes;
    let limit = p.stream_cache_mb.min(4000) as f32;
    let clear = |ui: &mut Ui| {
        if ui.add(Button::new("Очистить").style(Style::Text).enabled(used > 0).id_salt("stream-cache-clear")).clicked() {
            out.push(Command::ClearStreamCache);
        }
    };
    if let Some(v) = rows::slider_with(ui, Id::StreamCache, limit, Num::new(0.0, 4000.0, 100.0, 0, " МБ"), 104.0, clear) {
        p.stream_cache_mb = v as u32;
    }
}

/// An account as its row's value: who is signed in, or why it cannot be
/// (not built in); nothing while signing in or signed out (the buttons
/// say it), and a failure shows under the row.
fn account_note(a: Option<&AccountView>) -> Option<Note<'_>> {
    let Some(a) = a.filter(|a| a.registered) else {
        return Some(Note::Blocker("Не подключён в этой сборке"));
    };
    match &a.account {
        Account::LoggedIn { name, .. } => Some(Note::Value(name.as_str())),
        _ => None,
    }
}

fn logged_in(a: Option<&AccountView>) -> bool {
    a.is_some_and(|a| a.account.is_logged_in())
}

/// Spotify works with Premium only (docs/stage3.md §0): connected, its
/// sound, the import and «Выйти»; not connected, the steps to connect it
/// (the user's own key, then the sign-in) — in «Просто» one «Подключить»
/// that opens them.
fn spotify(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let acc = state.account(Service::Spotify);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "Spotify");
    switch(ui, Id::SpotifyEnabled, &mut p.spotify_enabled);
    let steps_shown = rows::shown(ui.ctx(), Id::SpotifyPersonal);
    dimmed(ui, !p.spotify_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        let signed = logged_in(acc);
        row_note(ui, Id::Spotify, account_note(acc), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if busy {
                    ui.add(LoadingIndicator::new().size(32.0));
                }
                if signed {
                    if ui.add(Button::new("Выйти").icon(Icon::Logout).style(Style::Outlined).id_salt("sp-logout")).clicked() {
                        out.push(Command::StreamLogout(Service::Spotify));
                    }
                } else if !busy
                    && !steps_shown
                    && ui
                        .add(Button::new("Подключить").icon(Icon::Login).style(Style::Filled).enabled(registered).id_salt("sp-connect"))
                        .clicked()
                {
                    // The steps are a row of «Подробно»: this visit opens it.
                    pages.open_setting(Id::SpotifyPersonal);
                }
            });
        });
        if let Some(a) = acc {
            login_prompt(ui, a, out);
            login_error(ui, a);
            paused_line(ui, a);
            refused_line(ui, a, !busy, out);
        }
        if !signed {
            setup_steps(ui, p, pages, registered && !busy, out);
            return;
        }
        // Signed in means Premium: the sound is one more consent away, or on.
        pages.sp_client_id.clear();
        let audio = acc.is_some_and(|a| a.audio);
        let soloist = acc.and_then(|a| a.soloist.clone()).filter(|s| s.phase != SoloistPhase::Unsupported);
        let on_soloist = soloist.as_ref().is_some_and(SoloistStatus::ready);
        dimmed(ui, on_soloist, |ui| {
            row(ui, Id::SpotifyPlayback, |ui| {
                if audio {
                    // Granted: the switch says so (it is not taken back here).
                    let mut on = true;
                    ui.add(Switch::new(&mut on).enabled(false).id_salt("sp-playback-on"));
                } else if ui
                    .add(
                        Button::new("Включить звук")
                            .icon(Icon::VolumeUp)
                            .style(Style::Tonal)
                            .enabled(!busy && !on_soloist)
                            .id_salt("sp-playback"),
                    )
                    .clicked()
                {
                    out.push(Command::StreamLogin { service: Service::Spotify, input: LoginInput::Playback });
                }
            });
        });
        // Only while Spotify is switched on: it is never downloaded otherwise.
        if let Some(st) = &soloist {
            soloist_row(ui, st, p.spotify_enabled, pages);
            if p.spotify_enabled {
                soloist_dialogs(ui, st, pages, out);
            }
        }
        import_row(ui, state, Service::Spotify, out);
        if let Some(a) = acc {
            reconnect_line(ui, a, !busy, out);
        }
    });
}

/// «Spotify Soloist»: the one step of its setup that applies; a progress
/// indicator while it downloads or starts, and only what went wrong in
/// words.
fn soloist_row(ui: &mut Ui, st: &SoloistStatus, can: bool, pages: &mut PageState) {
    let note = match &st.phase {
        SoloistPhase::NeedsKey if st.has_key => Some(Note::Error("Ключ не подошёл")),
        SoloistPhase::Failed(why) => Some(Note::Error(why.as_str())),
        _ => None,
    };
    row_note(ui, Id::SpotifySoloist, note, |ui| {
        let button = |ui: &mut Ui, label: &str, style: Style, salt: &str| ui.add(Button::new(label).style(style).enabled(can).id_salt(salt)).clicked();
        match &st.phase {
            SoloistPhase::Downloading { done, total } => match total {
                Some(t) if *t > 0 => {
                    crate::components::wavy_progress(ui, "solo-download-progress", *done as f32 / *t as f32, 96.0);
                }
                _ => {
                    ui.add(LoadingIndicator::new().size(32.0));
                }
            },
            SoloistPhase::Starting => {
                ui.add(LoadingIndicator::new().size(32.0));
            }
            SoloistPhase::Absent | SoloistPhase::Unsupported => {
                if button(ui, "Скачать", Style::Tonal, "solo-download") {
                    pages.soloist_dialog = SoloistDialog::Download;
                }
            }
            SoloistPhase::NeedsKey => {
                if button(ui, "Ввести ключ", Style::Tonal, "solo-key") {
                    pages.soloist_dialog = SoloistDialog::Key;
                }
            }
            SoloistPhase::Pairing => {
                if button(ui, "Подключить", Style::Tonal, "solo-pair") {
                    pages.soloist_dialog = SoloistDialog::Pair;
                }
            }
            SoloistPhase::Ready => {
                if button(ui, "Выключить", Style::Outlined, "solo-off") {
                    pages.soloist_action = Some(SoloistAction::Disable);
                }
            }
            SoloistPhase::Off => {
                if button(ui, "Включить", Style::Tonal, "solo-on") {
                    pages.soloist_action = Some(SoloistAction::Enable);
                    pages.soloist_dialog = SoloistDialog::Pair;
                }
            }
            SoloistPhase::Failed(_) => {
                if button(ui, "Повторить", Style::Tonal, "solo-retry") {
                    pages.soloist_action = Some(SoloistAction::Retry);
                }
            }
        }
    });
}

/// The setup's dialogs: the download asked first (also once by itself after
/// a Premium sign-in), then the key, then choosing the device in Spotify;
/// each opens by itself when the step before is done.
fn soloist_dialogs(ui: &mut Ui, st: &SoloistStatus, pages: &mut PageState, out: &mut Vec<Command>) {
    if let Some(a) = pages.soloist_action.take() {
        out.push(Command::Soloist(a));
    }
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    // The step after: open its dialog when the one before finished.
    if pages.soloist_dialog == SoloistDialog::None && pages.soloist_flow {
        pages.soloist_dialog = match st.phase {
            SoloistPhase::NeedsKey => SoloistDialog::Key,
            SoloistPhase::Pairing => SoloistDialog::Pair,
            _ => SoloistDialog::None,
        };
    }
    if st.offer && !pages.soloist_offered && st.phase == SoloistPhase::Absent {
        pages.soloist_offered = true;
        pages.soloist_dialog = SoloistDialog::Download;
    }
    if st.ready() {
        pages.soloist_flow = false;
        if pages.soloist_dialog == SoloistDialog::Pair {
            pages.soloist_dialog = SoloistDialog::None;
        }
    }
    let mut open = true;
    match pages.soloist_dialog {
        SoloistDialog::None => {}
        SoloistDialog::Download => {
            let size = st.size.map_or_else(String::new, |b| format!(" (≈{} МБ)", (b + 524_288) / 1_048_576));
            let r = Dialog::new("solo-dl-dialog", "Звук Spotify")
                .icon(Icon::Download)
                .supporting(format!("Чтобы играл звук из Spotify, нужен Spotify Soloist — официальный плеер Spotify{size}."))
                .action("Не сейчас")
                .action("Скачать")
                .show(&ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::Soloist(SoloistAction::Download));
                pages.soloist_flow = true;
            } else if r.action.is_some() || r.dismissed || !open {
                if st.offer {
                    out.push(Command::Soloist(SoloistAction::AnswerOffer));
                }
                pages.soloist_flow = false;
            }
            if r.action.is_some() || r.dismissed || !open {
                pages.soloist_dialog = SoloistDialog::None;
            }
        }
        SoloistDialog::Key => {
            let typed = pages.soloist_key.trim().to_owned();
            let refused = st.has_key && st.phase == SoloistPhase::NeedsKey;
            let r = Dialog::new("solo-key-dialog", "Ключ Soloist")
                .max_width(480.0)
                .action("Не сейчас")
                .action_with(DialogAction::new("Готово").enabled(typed.len() >= 8))
                .show_with(&ctx, &mut open, |ui| {
                    ui.add_space(12.0);
                    numbered(ui, 1, 20.0, |ui| {
                        ui.label(TypeRole::BodyMedium.style().rich_para("Войди в аккаунт Spotify с Premium", s.on_surface));
                    });
                    numbered(ui, 2, 20.0, |ui| {
                        ui.label(TypeRole::BodyMedium.style().rich_para("Создай ключ и скопируй его", s.on_surface));
                        ui.add_space(4.0);
                        if ui
                            .add(Button::new("Открыть страницу ключа").icon(Icon::OpenInNew).style(Style::Tonal).id_salt("solo-key-page"))
                            .clicked()
                        {
                            open_url(SOLOIST_KEY_PAGE);
                        }
                    });
                    numbered(ui, 3, 20.0, |ui| {
                        ui.label(TypeRole::BodyMedium.style().rich_para("Вставь его сюда", s.on_surface));
                        ui.add_space(4.0);
                        let mut field = TextField::new(&mut pages.soloist_key, "Ключ").password(true);
                        if refused {
                            field = field.error("Ключ не подошёл");
                        }
                        field.width(ui.available_width().min(380.0)).id_salt("solo-key-field").show(ui);
                    });
                });
            if r.action == Some(1) && typed.len() >= 8 {
                out.push(Command::Soloist(SoloistAction::SetKey(typed)));
                pages.soloist_key.clear();
                pages.soloist_flow = true;
            } else if r.action.is_some() || r.dismissed || !open {
                pages.soloist_key.clear();
                pages.soloist_flow = false;
            }
            if r.action.is_some() || r.dismissed || !open {
                pages.soloist_dialog = SoloistDialog::None;
            }
        }
        SoloistDialog::Pair => {
            let name = st.device_name.clone();
            let r = Dialog::new("solo-pair-dialog", "Подключить Soloist")
                .max_width(480.0)
                .action("Закрыть")
                .show_with(&ctx, &mut open, |ui| {
                    ui.add_space(12.0);
                    numbered(ui, 1, 20.0, |ui| {
                        ui.label(TypeRole::BodyMedium.style().rich_para("Открой Spotify на телефоне или компьютере в этой же сети", s.on_surface));
                    });
                    numbered(ui, 2, 20.0, |ui| {
                        ui.label(TypeRole::BodyMedium.style().rich_para(format!("В списке устройств выбери «{name}»"), s.on_surface));
                    });
                    // It closes by itself once Spotify chose it; only a failure
                    // has something to say.
                    if let SoloistPhase::Failed(why) = &st.phase {
                        ui.label(TypeRole::BodyMedium.style().rich_para(why, s.error));
                    }
                });
            if r.action.is_some() || r.dismissed || !open {
                pages.soloist_dialog = SoloistDialog::None;
                pages.soloist_flow = false;
            }
        }
    }
}

/// Not connected: the numbered steps to it — the user's own app on Spotify
/// for Developers (Spotify lets it work only for a Premium owner), its
/// Client ID, the sign-in in the browser. What it takes is in the ⓘ.
fn setup_steps(ui: &mut Ui, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    rows::titled(ui, Id::SpotifyPersonal, |ui| {
        ui.set_max_width(ui.available_width().min(640.0));
        ui.add_space(8.0);
        spotify_steps(ui, p, pages, can, out);
        ui.add_space(8.0);
    });
}

/// The steps themselves, numbered (the first run shows them too).
pub(crate) fn spotify_steps(ui: &mut Ui, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    numbered(ui, 1, 32.0, |ui| {
        if ui.add(Button::new("Открыть Spotify for Developers").icon(Icon::OpenInNew).style(Style::Tonal).id_salt("sp-dash")).clicked() {
            open_url(DASHBOARD);
        }
    });
    numbered(ui, 2, 20.0, |ui| {
        ui.label(TypeRole::BodyMedium.style().rich_para("Создай приложение (Create app), Redirect URI:", s.on_surface));
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(TypeRole::BodyMedium.emphasized().rich(REDIRECT_URI, s.on_surface));
            copy_button(ui, "sp-redirect-copy", REDIRECT_URI);
        });
        ui.label(TypeRole::BodyMedium.style().rich_para("Отметь Web API и Web Playback SDK, сохрани.", s.on_surface));
    });
    numbered(ui, 3, 20.0, |ui| {
        ui.label(TypeRole::BodyMedium.style().rich_para("Скопируй Client ID из Settings приложения:", s.on_surface));
        ui.add_space(4.0);
        let placeholder = if p.spotify_personal_app { "Сохранён" } else { "32 символа" };
        TextField::new(&mut pages.sp_client_id, "Client ID")
            .placeholder(placeholder)
            .width(ui.available_width().min(420.0))
            .id_salt("sp-client-id")
            .show(ui);
    });
    let id = pages.sp_client_id.trim().to_owned();
    let typed = id.len() >= 16;
    numbered(ui, 4, 40.0, |ui| {
        if ui
            .add(
                Button::new("Войти через Spotify")
                    .icon(Icon::Login)
                    .style(Style::Filled)
                    .size(Size::M)
                    .enabled(can && (typed || p.spotify_personal_app))
                    .id_salt("sp-login"),
            )
            .clicked()
        {
            // The typed key, else the saved one; it is kept once the
            // sign-in works.
            let input = if typed { LoginInput::PersonalApp { client_id: id.clone() } } else { LoginInput::Browser };
            out.push(Command::StreamLogin { service: Service::Spotify, input });
        }
    });
}

/// A numbered step of text.
pub(crate) fn step(ui: &mut Ui, n: u32, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    numbered(ui, n, 20.0, |ui| {
        ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface));
    });
}

/// A numbered step: the number in a small tonal circle, centred on the
/// first line (`line` high), and the step's content beside it.
pub(crate) fn numbered(ui: &mut Ui, n: u32, line: f32, add: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(vec2(24.0, line.max(24.0)), Sense::hover());
        let c = egui::pos2(r.center().x, r.top() + line.max(24.0) / 2.0);
        ui.painter().circle_filled(c, 12.0, s.secondary_container);
        ui.painter().text(c, egui::Align2::CENTER_CENTER, n.to_string(), TypeRole::LabelLarge.font_id(), s.on_secondary_container);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            add(ui);
        });
    });
    ui.add_space(12.0);
}

/// «Spotify просит подождать до 14:05»: the service paused our calls (a
/// blocker: its search and pages are silent until then).
pub(crate) fn paused_line(ui: &mut Ui, a: &AccountView) {
    let s = Theme::get(ui.ctx()).scheme;
    if let Some(hm) = &a.paused_until_hm {
        ui.horizontal(|ui| {
            Icon::HourglassEmpty.widget(false, s.on_surface_variant, 18.0).ui(ui);
            ui.add(
                egui::Label::new(
                    TypeRole::BodyMedium.style().rich_para(format!("{} просит подождать до {hm}", a.service.display_name()), s.on_surface_variant),
                )
                .wrap(),
            );
        });
        ui.add_space(8.0);
    }
}

/// The service keeps refusing this account's sound (Spotify's audio keys):
/// until when (an error), and «Повторить» to ask again now.
fn refused_line(ui: &mut Ui, a: &AccountView, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let Some(hm) = &a.audio_refused_until_hm else { return };
    ui.horizontal(|ui| {
        Icon::Error.widget(false, s.error, 18.0).ui(ui);
        let text = format!("{} не отдаёт звук до {hm}", a.service.display_name());
        ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(text, s.error)).wrap());
        if ui
            .add(Button::new("Повторить").style(Style::Text).size(Size::S).enabled(can).id_salt(("retry-audio", a.service.as_str())))
            .clicked()
        {
            out.push(Command::RetryStreamAudio(a.service));
        }
    });
    ui.add_space(8.0);
}

/// A feed of the service's suggestions that needs a new sign-in: why (a
/// blocker), with a quiet way to do it. How the feed goes otherwise is not
/// said.
fn reconnect_line(ui: &mut Ui, a: &AccountView, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let Some(note) = &a.suggestions_note else { return };
    ui.horizontal(|ui| {
        Icon::Info.widget(false, s.on_surface_variant, 18.0).ui(ui);
        ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(note, s.on_surface_variant)).wrap());
        if ui
            .add(Button::new("Переподключить").style(Style::Text).size(Size::S).enabled(can).id_salt(("reconnect", a.service.as_str())))
            .clicked()
        {
            out.push(Command::StreamLogin { service: a.service, input: LoginInput::Browser });
        }
    });
    ui.add_space(8.0);
}

/// «Плейлисты и история» of a signed-in service: its own playlists and
/// recent plays (its likes come by themselves after the sign-in).
fn import_row(ui: &mut Ui, state: &ViewState, service: Service, out: &mut Vec<Command>) {
    let id = match service {
        Service::Spotify => Id::ImportSpotify,
        Service::Youtube | Service::Deezer => Id::ImportYouTube,
        Service::Soundcloud => Id::ImportSoundCloud,
    };
    let ready = state.prefs.service_enabled(service);
    row(ui, id, |ui| {
        if ui
            .add(Button::new("Импортировать").icon(Icon::Download).style(Style::Tonal).enabled(ready).id_salt(("import", service.as_str())))
            .clicked()
        {
            out.push(Command::ImportFromService(service));
        }
    });
}

/// The browser step of a running login: the link (the browser may not
/// have opened by itself) with «Открыть» and «Копировать», or a device code.
pub(crate) fn login_prompt(ui: &mut Ui, a: &AccountView, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let Account::LoggingIn(Some(prompt)) = &a.account else { return };
    let salt = a.service.as_str();
    let (url, code) = match prompt {
        LoginPrompt::OpenUrl(u) => (u.as_str(), None),
        LoginPrompt::DeviceCode { url, code } => (url.as_str(), Some(code.as_str())),
        LoginPrompt::Window(text) => {
            window_prompt(ui, a, text, out);
            return;
        }
    };
    Card::outlined().id_salt(("login-prompt", salt)).show(ui, |ui| {
        ui.set_width(ui.available_width().min(640.0));
        ui.label(TypeRole::TitleSmall.style().rich("Заверши вход в браузере", s.on_surface));
        ui.label(TypeRole::BodyMedium.style().rich_para(
            match code {
                Some(_) => "Открой страницу и введи код:",
                None => "Если браузер не открылся сам, открой эту ссылку:",
            },
            s.on_surface_variant,
        ));
        if let Some(c) = code {
            ui.add_space(4.0);
            ui.label(TypeRole::HeadlineSmall.emphasized().rich(c, s.primary));
        }
        ui.add_space(4.0);
        ui.add(egui::Label::new(TypeRole::BodySmall.style().rich(url, s.primary)).truncate());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new("Открыть").icon(Icon::OpenInNew).style(Style::Tonal).size(Size::S).id_salt(("login-open", salt))).clicked() {
                open_url(url);
            }
            copy_button(ui, ("login-copy", salt), code.unwrap_or(url));
            if ui.add(Button::new("Отмена").style(Style::Text).size(Size::S).id_salt(("login-cancel", salt))).clicked() {
                out.push(Command::StreamLogout(a.service));
            }
        });
    });
    ui.add_space(8.0);
}

/// The app's own sign-in window is open: what to do there, and «Отмена»
/// (closes it; the window's saved Google session stays).
fn window_prompt(ui: &mut Ui, a: &AccountView, text: &str, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let salt = a.service.as_str();
    Card::outlined().id_salt(("login-window", salt)).show(ui, |ui| {
        ui.set_width(ui.available_width().min(640.0));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            Icon::OpenInNew.widget(false, s.primary, 20.0).ui(ui);
            ui.label(TypeRole::TitleSmall.style().rich("Заверши вход в окне", s.on_surface));
        });
        ui.add_space(4.0);
        ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface_variant));
        ui.add_space(8.0);
        if ui.add(Button::new("Отмена").style(Style::Text).size(Size::S).id_salt(("login-window-cancel", salt))).clicked() {
            out.push(Command::StreamLogout(a.service));
        }
    });
    ui.add_space(8.0);
}

pub(crate) fn login_error(ui: &mut Ui, a: &AccountView) {
    let s = Theme::get(ui.ctx()).scheme;
    let text = a.error.clone().or_else(|| match &a.account {
        Account::Failed(m) => Some(m.clone()),
        _ => None,
    });
    if let Some(t) = text {
        // No Premium is not a fault to fix here: said quietly.
        let (icon, color) = if t == SPOTIFY_PREMIUM_REQUIRED { (Icon::Info, s.on_surface_variant) } else { (Icon::Error, s.error) };
        ui.horizontal(|ui| {
            icon.widget(false, color, 18.0).ui(ui);
            ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(t, color)).wrap());
        });
        ui.add_space(8.0);
    }
}

/// Deezer: a catalogue without an account; one switch (and the line of a
/// pause).
fn deezer(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    group(ui, "Deezer");
    switch(ui, Id::DeezerEnabled, &mut p.deezer_enabled);
    if p.deezer_enabled
        && let Some(a) = state.account(Service::Deezer)
    {
        paused_line(ui, a);
    }
}

fn youtube(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let acc = state.account(Service::Youtube);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "YouTube Music");
    switch(ui, Id::YouTubeEnabled, &mut p.youtube_enabled);
    let fallback = rows::shown(ui.ctx(), Id::YouTubeCookies);
    dimmed(ui, !p.youtube_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        row_note(ui, Id::YouTube, account_note(acc), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if busy {
                    ui.add(LoadingIndicator::new().size(32.0));
                }
                if logged_in(acc) {
                    if ui.add(Button::new("Выйти").icon(Icon::Logout).style(Style::Outlined).id_salt("yt-logout")).clicked() {
                        out.push(Command::StreamLogout(Service::Youtube));
                    }
                } else if !busy {
                    // The Google window first (lined up with the other
                    // controls), the cookies form as the smaller fallback
                    // («Подробно»).
                    if ui
                        .add(Button::new("Войти через Google").icon(Icon::Login).style(Style::Filled).enabled(registered).id_salt("yt-login"))
                        .clicked()
                    {
                        pages.yt_form = false;
                        out.push(Command::StreamLogin { service: Service::Youtube, input: LoginInput::Browser });
                    }
                    if fallback
                        && ui
                            .add(
                                Button::new("Вставить cookies…")
                                    .style(Style::Text)
                                    .size(Size::S)
                                    .selected(pages.yt_form)
                                    .enabled(registered)
                                    .id_salt("yt-cookies-open"),
                            )
                            .clicked()
                    {
                        pages.yt_form = !pages.yt_form;
                    }
                }
            });
        });
        if let Some(a) = acc {
            login_prompt(ui, a, out);
            login_error(ui, a);
            paused_line(ui, a);
        }
        if logged_in(acc) {
            import_row(ui, state, Service::Youtube, out);
        }
        if let Some(a) = acc {
            reconnect_line(ui, a, !busy, out);
        }
        if !logged_in(acc) && (pages.yt_form || p.youtube_cookies_set) {
            cookies_form(ui, p, pages, registered && !busy, out);
        }
    });
}

/// SoundCloud: the switch, the account (its sign-in window; the token as
/// the fallback in «Подробно»), the import.
fn soundcloud(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let acc = state.account(Service::Soundcloud);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "SoundCloud");
    switch(ui, Id::SoundCloudEnabled, &mut p.soundcloud_enabled);
    let fallback = rows::shown(ui.ctx(), Id::SoundCloudToken);
    dimmed(ui, !p.soundcloud_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        row_note(ui, Id::SoundCloud, account_note(acc), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if busy {
                    ui.add(LoadingIndicator::new().size(32.0));
                }
                if logged_in(acc) {
                    if ui.add(Button::new("Выйти").icon(Icon::Logout).style(Style::Outlined).id_salt("sc-logout")).clicked() {
                        out.push(Command::StreamLogout(Service::Soundcloud));
                    }
                } else if !busy {
                    if ui
                        .add(
                            Button::new("Войти")
                                .icon(Icon::Login)
                                .style(Style::Filled)
                                .enabled(registered && p.soundcloud_enabled)
                                .id_salt("sc-login"),
                        )
                        .clicked()
                    {
                        pages.sc_form = false;
                        out.push(Command::StreamLogin { service: Service::Soundcloud, input: LoginInput::Browser });
                    }
                    if fallback
                        && ui
                            .add(
                                Button::new("Вставить токен…")
                                    .style(Style::Text)
                                    .size(Size::S)
                                    .selected(pages.sc_form)
                                    .enabled(registered && p.soundcloud_enabled)
                                    .id_salt("sc-token-open"),
                            )
                            .clicked()
                    {
                        pages.sc_form = !pages.sc_form;
                    }
                }
            });
        });
        if let Some(a) = acc {
            login_prompt(ui, a, out);
            login_error(ui, a);
            paused_line(ui, a);
        }
        if logged_in(acc) {
            import_row(ui, state, Service::Soundcloud, out);
        }
        if let Some(a) = acc {
            reconnect_line(ui, a, !busy, out);
        }
        if !logged_in(acc) && pages.sc_form {
            let can = registered && !busy && p.soundcloud_enabled;
            item(ui, Id::SoundCloudToken, |ui| {
                token_fields(ui, pages, can, true, out);
                ui.add_space(8.0);
            });
        }
    });
}

/// SoundCloud's token login: the field (its ⓘ beside it with `help`) and
/// «Войти» — Настройки' and the first run's.
pub(crate) fn token_fields(ui: &mut Ui, pages: &mut PageState, can: bool, help: bool, out: &mut Vec<Command>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        TextField::new(&mut pages.sc_token, "oauth_token").password(true).width(ui.available_width().min(420.0)).id_salt("sc-token").show(ui);
        if help {
            settings_help::help_button(ui, Id::SoundCloudToken);
        }
    });
    ui.add_space(8.0);
    let text = pages.sc_token.trim().to_owned();
    if ui.add(Button::new("Войти").style(Style::Filled).enabled(can && !text.is_empty()).id_salt("sc-token-login")).clicked() {
        out.push(Command::StreamLogin { service: Service::Soundcloud, input: LoginInput::Cookies(text) });
        pages.sc_token.clear();
        pages.sc_form = false;
    }
}

/// The cookies login: how to get them, a paste field and a file picker.
fn cookies_form(ui: &mut Ui, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    item(ui, Id::YouTubeCookies, |ui| {
        Card::filled().id_salt("yt-cookies").show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(TypeRole::TitleSmall.style().rich("Вход по cookies", s.on_surface));
                settings_help::help_button(ui, Id::YouTubeCookies);
            });
            ui.add_space(4.0);
            cookies_fields(ui, p, pages, can, out);
        });
        ui.add_space(8.0);
    });
}

/// The cookies login's steps, paste field, file picker and «Войти» —
/// Настройки' card and the first run's.
pub(crate) fn cookies_fields(ui: &mut Ui, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    step(ui, 1, "Войди на music.youtube.com в браузере.");
    step(ui, 2, "Расширением вроде «Get cookies.txt LOCALLY» выгрузи cookies.txt для этого сайта.");
    step(ui, 3, "Выбери файл или вставь его текст (или заголовок Cookie) ниже.");
    ui.add_space(4.0);
    let frame = egui::Frame::new()
        .fill(s.surface_container_low)
        .stroke(Stroke::new(1.0, s.outline_variant))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10));
    frame.show(ui, |ui| {
        // Saved ones are not shown back: the field says they are there.
        let hint = if p.youtube_cookies_set { "Cookies сохранены" } else { "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t…" };
        ui.add(
            egui::TextEdit::multiline(&mut pages.yt_cookies)
                .id_salt("yt-cookies-text")
                .hint_text(TypeRole::BodyMedium.style().rich(hint, s.on_surface_variant))
                .font(TypeRole::BodySmall.font_id())
                .text_color(s.on_surface)
                .frame(egui::Frame::NONE)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if ui
            .add(Button::new("Выбрать cookies.txt…").icon(Icon::UploadFile).style(Style::Outlined).id_salt("yt-cookies-file"))
            .clicked()
            && let Some(f) = rfd::FileDialog::new().set_title("cookies.txt для music.youtube.com").add_filter("cookies.txt", &["txt"]).pick_file()
        {
            match std::fs::read_to_string(&f) {
                Ok(t) => pages.yt_cookies = t,
                Err(e) => log::warn!("cannot read {}: {e}", f.display()),
            }
        }
        let text = pages.yt_cookies.trim().to_owned();
        if ui.add(Button::new("Войти").style(Style::Filled).enabled(can && !text.is_empty()).id_salt("yt-cookies-login")).clicked() {
            out.push(Command::StreamLogin { service: Service::Youtube, input: LoginInput::Cookies(text) });
            pages.yt_cookies.clear();
            pages.yt_form = false;
        }
        if p.youtube_cookies_set && ui.add(Button::new("Забыть cookies").style(Style::Text).id_salt("yt-cookies-clear")).clicked() {
            out.push(Command::SetStreamSecrets { spotify_client_id: None, youtube_cookies: Some(String::new()) });
        }
    });
}

/// «Импорт истории»: the export files (years of listening; where to get
/// them is in the ⓘ) and the YouTube Music library's songs.
fn imports(ui: &mut Ui, p: &mut Prefs, out: &mut Vec<Command>) {
    group(ui, "Импорт истории");
    row(ui, Id::ImportFiles, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new("Папка…").icon(Icon::Folder).style(Style::Tonal).id_salt("import-dir")).clicked() {
                super::super::dialogs::folder(|d| d.set_title("Папка с выгрузкой Spotify или Google Takeout"), Command::ImportExportFiles, out);
            }
            if ui.add(Button::new(".zip…").icon(Icon::UploadFile).style(Style::Outlined).id_salt("import-zip")).clicked() {
                super::super::dialogs::file(
                    |d| d.set_title("Архив выгрузки Spotify или Google Takeout").add_filter("Архив", &["zip"]),
                    Command::ImportExportFiles,
                    out,
                );
            }
        });
    });
    switch(ui, Id::ImportSavedSongs, &mut p.import_saved_songs);
}

/// Opens a web page in the default browser.
pub(crate) fn open_url(url: &str) {
    // A test must never launch a program on the user's desktop.
    if cfg!(test) {
        panic!("a test tried to launch an external program");
    }
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    match std::process::Command::new(opener).arg(url).spawn() {
        // Reaped in the background, or the finished opener stays a zombie.
        #[cfg(unix)]
        Ok(mut child) => {
            let _ = std::thread::Builder::new().name("opener".into()).spawn(move || {
                let _ = child.wait();
            });
        }
        #[cfg(not(unix))]
        Ok(_) => {}
        Err(e) => log::warn!("cannot open {url}: {e}"),
    }
}

/// A small «Копировать» that reads «Скопировано» for a moment.
fn copy_button(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, text: &str) {
    let key = egui::Id::new(("copied", &salt));
    let now = ui.ctx().input(|i| i.time);
    let at: Option<f64> = ui.ctx().data(|d| d.get_temp(key));
    let fresh = at.is_some_and(|t| now - t < 1.6);
    if fresh {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    let b = Button::new(if fresh { "Скопировано" } else { "Копировать" })
        .icon(if fresh { Icon::Check } else { Icon::ContentCopy })
        .style(Style::Text)
        .size(Size::S)
        .id_salt(("copy-btn", salt));
    if ui.add(b).clicked() {
        ui.ctx().copy_text(text.to_owned());
        ui.ctx().data_mut(|d| d.insert_temp(key, now));
    }
}
