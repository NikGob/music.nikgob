//! Настройки → «Источники» (docs/stage3.md §0 "Где настройки", §3.1, §3.6):
//! Spotify, YouTube Music and the Deezer catalogue (switch, account, sound, import of playlists
//! and history once signed in; Spotify's steps to connect — its own key
//! and the sign-in, Premium only —; cookies), what a like
//! downloads, playlist sync, the export files, the stream cache, and the
//! torrents (Prowlarr, seeding) moved here from «Основные». A service that
//! asked to wait says until when, quietly, and so does what its own
//! suggestions give our mixes (docs/stage4.md §12.3).

use egui::{Sense, Stroke, Ui, Widget as _, emath::TSTransform, vec2};
use nikgob_core::streaming::SPOTIFY_PREMIUM_REQUIRED;
use nikgob_view::{
    self as v, Account, AccountView, Command, LoginInput, LoginPrompt, Prefs, Service, SoloistAction, SoloistPhase,
    SoloistStatus, ViewState,
};

use super::PageState;
use super::settings::{choice, dimmed, fmt_speed, group, range_slider, row, speed_slider, speed_slider_parts};
use super::settings_help::{self, SettingId as Id};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Card, Dialog, DialogAction, IconButton, LoadingIndicator, Size, Style, Switch, TextField, anim},
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

pub fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(12.0);
    ui.label(TypeRole::BodyMedium.style().rich_para(
        "Бесплатно работают YouTube Music, SoundCloud, Deezer и торренты; Spotify — с Premium.",
        s.on_surface_variant,
    ));
    spotify(ui, state, p, pages, out);
    youtube(ui, state, p, pages, out);
    soundcloud(ui, state, p, pages, out);
    deezer(ui, state, p);
    download_sources(ui, state, p);

    group(ui, "Лайки и загрузки");
    let scope_hint = if p.like_downloads {
        "Лайк альбома — всегда весь альбом"
    } else {
        "Лайк — только лайк; «Скачать» качает и так"
    };
    row(ui, Id::LikeDownload, "Что качать по лайку", Some(scope_hint), |ui| {
        choice(ui, Id::LikeDownload, p);
    });
    row(ui, Id::PushLikes, "Лайки обратно в сервисы", Some("Лайк здесь ставится и в Spotify / YouTube Music"), |ui| {
        ui.add(Switch::new(&mut p.push_likes).id_salt("set-push-likes"));
    });
    row(ui, Id::AlbumLike, "Лайк альбома", None, |ui| choice(ui, Id::AlbumLike, p));
    row(ui, Id::SingleLike, "Лайк песни из сингла", None, |ui| choice(ui, Id::SingleLike, p));
    row(ui, Id::LikedAlbumsPlace, "Любимые альбомы", None, |ui| choice(ui, Id::LikedAlbumsPlace, p));
    let n = p.auto_download_limit.clamp(1, 8);
    row(ui, Id::AutoDownloads, "Одновременных автозагрузок", Some(&format!("{n} из «Хочу скачать» за раз")), |ui| {
        if let Some(v) = range_slider(ui, "set-auto-dl", n as f32, (1.0, 8.0), 1.0, |v| format!("{v:.0}")) {
            p.auto_download_limit = v as u32;
        }
    });
    row(ui, Id::DownloadToasts, "Уведомления о загрузках", Some("Нашлось, не нашлось, скачалось"), |ui| {
        ui.add(Switch::new(&mut p.download_toasts).id_salt("set-download-toasts"));
    });

    group(ui, "Качество");
    row(ui, Id::UpgradeEvery, "Искать копии лучше", None, |ui| choice(ui, Id::UpgradeEvery, p));
    let never = p.upgrade_every == v::UpgradeEvery::Never;
    dimmed(ui, never, |ui| {
        row(ui, Id::UpgradeBar, "Хорошее качество", None, |ui| choice(ui, Id::UpgradeBar, p));
        row(ui, Id::UpgradeAuto, "Замена файла", None, |ui| choice(ui, Id::UpgradeAuto, p));
    });

    group(ui, "Плейлисты");
    row(
        ui,
        Id::PlaylistSync,
        "Синхронизация по умолчанию",
        Some("Для новых плейлистов из сервисов; у каждого — в его меню"),
        |ui| choice(ui, Id::PlaylistSync, p),
    );

    group(ui, "Анализ звука");
    stream_analysis(ui, state, p);

    imports(ui, p, out);
    group(ui, "Кэш");
    stream_cache(ui, state, p, out);
    soulseek(ui, state, pages, out);
    torrents(ui, state, p, pages, out);
}

/// «Источники скачивания» (docs/stage4.5.md): where «Хочу скачать» looks,
/// in the order it prefers them at one quality, each with its switch and
/// «Выше» / «Ниже». Lossless comes first whichever source has it; a source
/// the app does not have is left out (torrents and Soulseek stay: they say
/// what they need; a source that needs setting up is dimmed until it is).
/// A move slides the rows to their new places.
fn download_sources(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    group(ui, "Источники скачивания");
    let mut order = p.source_order();
    let shown: Vec<v::SourceKey> = order
        .iter()
        .map(|o| o.key)
        .filter(|k| {
            let needs_setup = matches!(k, v::SourceKey::Torrents | v::SourceKey::Soulseek);
            state.downloads.sources.iter().any(|v| v.key == *k && (v.available || needs_setup))
        })
        .collect();
    let mut moved: Option<(usize, usize)> = None;
    // Each row springs from the slot it had to the one it has; the step
    // between slots is last frame's.
    let step_key = egui::Id::new("src-row-step");
    let step = ui.ctx().data(|d| d.get_temp::<f32>(step_key)).unwrap_or(64.0);
    let spring = Theme::get(ui.ctx()).motion.default_spatial;
    // The row whose arrow was pressed is lifted while it moves: a dragged
    // list item's surface and shadow over the row it passes, which slides
    // under it.
    let lifted_key = egui::Id::new("src-lifted");
    let lifted = ui.ctx().data(|d| d.get_temp::<v::SourceKey>(lifted_key));
    let mut lift_next: Option<v::SourceKey> = None;
    let top = ui.cursor().top();
    for (n, key) in shown.iter().enumerate() {
        let Some(at) = order.iter().position(|o| o.key == *key) else { continue };
        let view = state.downloads.sources.iter().find(|v| v.key == *key);
        let needs_setup = match key {
            v::SourceKey::Torrents => view.is_some_and(|v| !v.available),
            v::SourceKey::Soulseek => !state.downloads.soulseek.configured(),
            _ => false,
        };
        let hint = match (key, view) {
            (v::SourceKey::Torrents, Some(v)) if !v.available => "Нужен Prowlarr — ниже, в «Торренты»".to_owned(),
            (v::SourceKey::Soulseek, _) if !state.downloads.soulseek.configured() => "Нужен аккаунт — ниже, в «Soulseek»".to_owned(),
            (v::SourceKey::Soulseek, Some(v)) if order[at].on && !v.available => "Не подключился — проверь аккаунт ниже".to_owned(),
            (_, Some(v)) if v.status.is_some() => v.status.clone().unwrap_or_default(),
            (v::SourceKey::Torrents, _) => "Через Prowlarr".to_owned(),
            (v::SourceKey::Khinsider, _) => "Саундтреки игр".to_owned(),
            (v::SourceKey::Soulseek, _) => "Отдельные треки от людей".to_owned(),
            (v::SourceKey::Qobuz | v::SourceKey::Tidal, _) => "Бесплатно, через открытые сервисы".to_owned(),
            (v::SourceKey::Squidify, _) => "Саундтреки аниме и игр".to_owned(),
            (v::SourceKey::Archive, _) => "Редкое и старое".to_owned(),
            (v::SourceKey::Bandcamp, _) => "Бесплатные релизы".to_owned(),
        };
        // A source that is not set up shows off (its saved choice stays).
        let mut on = order[at].on && !needs_setup;
        let slot = anim::f32(ui.ctx(), egui::Id::new(("src-slot", key.as_str())), n as f32, spring);
        let shift = TSTransform::from_translation(vec2(0.0, (slot - n as f32) * step));
        // Full while it travels, settling back down over its last quarter slot.
        let lift = if lifted == Some(*key) { ((slot - n as f32).abs() / 0.25).clamp(0.0, 1.0) } else { 0.0 };
        let draw = |ui: &mut Ui| dimmed(ui, needs_setup, |ui| row(ui, Id::DownloadSource(*key), key.display_name(), Some(&hint), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let up = ui.add(
                    IconButton::new(Icon::ExpandLess)
                        .size(Size::Xs)
                        .enabled(n > 0)
                        .tooltip("Выше")
                        .id_salt(("src-up", key.as_str())),
                );
                if up.clicked() && n > 0 {
                    moved = order.iter().position(|o| o.key == shown[n - 1]).map(|to| (at, to));
                    lift_next = Some(*key);
                }
                let down = ui.add(
                    IconButton::new(Icon::ExpandMore)
                        .size(Size::Xs)
                        .enabled(n + 1 < shown.len())
                        .tooltip("Ниже")
                        .id_salt(("src-down", key.as_str())),
                );
                if down.clicked() && n + 1 < shown.len() {
                    moved = order.iter().position(|o| o.key == shown[n + 1]).map(|to| (at, to));
                    lift_next = Some(*key);
                }
                ui.add_space(8.0);
                ui.add(Switch::new(&mut on).enabled(!needs_setup).id_salt(("src-on", key.as_str())));
            });
        }));
        if lift > 0.0 {
            lifted_row(ui, shift, lift, draw);
        } else {
            ui.with_visual_transform(shift, draw);
        }
        if !needs_setup && on != order[at].on {
            order[at].on = on;
            p.download_sources = order.clone();
        }
    }
    if !shown.is_empty() {
        let h = (ui.cursor().top() - top) / shown.len() as f32;
        if h > 1.0 && (h - step).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(step_key, h));
        }
    }
    if let Some(k) = lift_next {
        ui.ctx().data_mut(|d| d.insert_temp(lifted_key, k));
    }
    if let Some((a, b)) = moved {
        order.swap(a, b);
        p.download_sources = order;
    }
}

/// A row drawn as a lifted surface (`lift` 0..1): on a layer just above the
/// page's, on a dragged list item's container with its shadow, moved by
/// `shift`. The page keeps its place for it as for any row.
fn lifted_row(ui: &mut Ui, shift: TSTransform, lift: f32, draw: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    let layer = egui::LayerId::new(ui.layer_id().order, egui::Id::new("src-lifted-layer"));
    ui.ctx().set_sublayer(ui.layer_id(), layer);
    ui.scope_builder(egui::UiBuilder::new().layer_id(layer), |ui| {
        ui.with_visual_transform(shift, |ui| {
            let (shadow, surface) = (ui.painter().add(egui::Shape::Noop), ui.painter().add(egui::Shape::Noop));
            let rect = ui.scope(draw).response.rect;
            // The whole row of the column, with the room the settings'
            // highlight gives a row.
            let r = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), rect.y_range()).expand2(vec2(12.0, 7.0));
            // It reaches past the column's edges as the highlight does: only
            // the scroll area's vertical clip applies (its shadow too).
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(egui::Rect::from_x_y_ranges(r.expand(24.0).x_range(), ui.clip_rect().y_range()));
            let mut sh = crate::Elevation::Level4.shadow();
            sh.color = sh.color.gamma_multiply(lift);
            painter.set(shadow, sh.as_shape(r, crate::tokens::shape::LARGE));
            let fill = s.surface_container_high.gamma_multiply(lift);
            let layer = s.on_surface.gamma_multiply(0.16 * lift);
            painter.set(surface, egui::Shape::Vec(vec![
                egui::Shape::rect_filled(r, crate::tokens::shape::LARGE, fill),
                egui::Shape::rect_filled(r, crate::tokens::shape::LARGE, layer),
            ]));
        });
    });
}

/// «Анализ звука стримов»: the switch and how far it is.
fn stream_analysis(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    let a = &state.analysis;
    let youtube = state.account(Service::Youtube).is_none_or(|a| a.enabled);
    let hint = if !p.stream_analysis {
        "Выключен: треки без файла подбираются по исполнителю и истории".to_owned()
    } else if !youtube {
        "Ждёт YouTube Music: он выключен выше".to_owned()
    } else if a.streams_total == 0 {
        "Лайкнутые и плейлистные треки без файла — с YouTube Music".to_owned()
    } else {
        let done = super::settings::fmt_count(a.streams_done.min(a.streams_total));
        let total = super::settings::fmt_count(a.streams_total);
        let state = if a.streams_done >= a.streams_total { "готово" } else { "до 300 в день" };
        format!("{done} из {total} без файла · {state}")
    };
    row(ui, Id::StreamAnalysis, "Анализ звука стримов", Some(&hint), |ui| {
        ui.add(Switch::new(&mut p.stream_analysis).id_salt("set-stream-analysis"));
    });
}

/// «Кэш стриминга»: its limit, how much it holds, «Очистить».
fn stream_cache(ui: &mut Ui, state: &ViewState, p: &mut Prefs, out: &mut Vec<Command>) {
    let used = state.stream_cache_bytes;
    let limit = p.stream_cache_mb;
    let hint = if limit == 0 {
        "Выключен: каждый раз качается заново".to_owned()
    } else if used == 0 {
        format!("Пока пусто · до {} на сервис", super::fmt_size(u64::from(limit) * 1024 * 1024))
    } else {
        format!("Занято {} · до {} на сервис", super::fmt_size(used), super::fmt_size(u64::from(limit) * 1024 * 1024))
    };
    row(ui, Id::StreamCache, "Кэш стриминга", Some(&hint), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let fmt = |v: f32| if v < 1.0 { "выкл.".to_owned() } else { format!("{v:.0} МБ") };
            if let Some(v) = range_slider(ui, "set-stream-cache", limit.min(4000) as f32, (0.0, 4000.0), 100.0, fmt) {
                p.stream_cache_mb = v as u32;
            }
            if ui
                .add(Button::new("Очистить").style(Style::Text).enabled(used > 0).id_salt("stream-cache-clear"))
                .clicked()
            {
                out.push(Command::ClearStreamCache);
            }
        });
    });
}

/// "Вошёл как …" and friends.
fn account_line(a: Option<&AccountView>) -> String {
    let Some(a) = a.filter(|a| a.registered) else {
        return "Не подключён в этой сборке".to_owned();
    };
    match &a.account {
        // Spotify: why it is not connected is said under the row.
        Account::LoggedOut | Account::Failed(_) if a.service == Service::Spotify => "Не подключён".to_owned(),
        Account::LoggedOut => "Не выполнен вход".to_owned(),
        Account::LoggingIn(Some(LoginPrompt::Window(_))) => "Вход… заверши его в открывшемся окне".to_owned(),
        Account::LoggingIn(_) => "Вход… заверши его в браузере".to_owned(),
        Account::LoggedIn { name, premium, .. } => match (a.service, premium) {
            (Service::Spotify, Some(true)) => format!("{name} · Premium"),
            _ => name.clone(),
        },
        Account::Failed(_) => "Вход не удался".to_owned(),
    }
}

fn logged_in(a: Option<&AccountView>) -> bool {
    a.is_some_and(|a| a.account.is_logged_in())
}

/// Spotify works with Premium only (docs/stage3.md §0): connected, its
/// sound, the import and «Выйти»; not connected, the steps to connect it
/// (the user's own key, then the sign-in).
fn spotify(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let acc = state.account(Service::Spotify);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "Spotify");
    row(ui, Id::SpotifyEnabled, "Использовать Spotify", Some("С Premium: поиск, лайки, плейлисты и звук"), |ui| {
        ui.add(Switch::new(&mut p.spotify_enabled).id_salt("set-sp-on"));
    });
    dimmed(ui, !p.spotify_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        let signed = logged_in(acc);
        let line = account_line(acc);
        row(ui, Id::Spotify, "Аккаунт", Some(&line), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if busy {
                    ui.add(LoadingIndicator::new().size(32.0));
                }
                if signed
                    && ui.add(Button::new("Выйти").icon(Icon::Logout).style(Style::Outlined).id_salt("sp-logout")).clicked()
                {
                    out.push(Command::StreamLogout(Service::Spotify));
                }
            });
        });
        if let Some(a) = acc {
            login_prompt(ui, a, out);
            login_error(ui, a);
            paused_line(ui, a);
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
        let sound_line = if audio {
            "Включён: треки Spotify играют из Spotify"
        } else {
            "Ещё одно разрешение — и звук пойдёт прямо из Spotify"
        };
        dimmed(ui, on_soloist, |ui| {
            row(ui, Id::SpotifyPlayback, "Звук из Spotify", Some(sound_line), |ui| {
                if !audio
                    && ui
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
            suggestions_line(ui, a, !busy, out);
        }
    });
}

/// «Spotify Soloist»: where its setup stands and the one step that applies.
fn soloist_row(ui: &mut Ui, st: &SoloistStatus, can: bool, pages: &mut PageState) {
    let line = match &st.phase {
        SoloistPhase::Absent | SoloistPhase::Unsupported => "Не скачан".to_owned(),
        SoloistPhase::Downloading { done, total } => match total {
            Some(t) if *t > 0 => format!("Скачивается · {} %", done * 100 / t),
            _ => "Скачивается".to_owned(),
        },
        SoloistPhase::NeedsKey if st.has_key => "Ключ не подошёл".to_owned(),
        SoloistPhase::NeedsKey => "Нужен ключ".to_owned(),
        SoloistPhase::Off => "Выключен".to_owned(),
        SoloistPhase::Starting => "Запускается".to_owned(),
        SoloistPhase::Pairing => "Не подключён".to_owned(),
        SoloistPhase::Ready => "Подключён".to_owned(),
        SoloistPhase::Failed(why) => why.clone(),
    };
    row(ui, Id::SpotifySoloist, "Spotify Soloist", Some(&line), |ui| {
        let button = |ui: &mut Ui, label: &str, style: Style, salt: &str| {
            ui.add(Button::new(label).style(style).enabled(can).id_salt(salt)).clicked()
        };
        match &st.phase {
            SoloistPhase::Downloading { .. } | SoloistPhase::Starting => {
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
                        ui.label(TypeRole::BodyMedium.style().rich_para(
                            "Открой Spotify на телефоне или компьютере в этой же сети",
                            s.on_surface,
                        ));
                    });
                    numbered(ui, 2, 20.0, |ui| {
                        ui.label(TypeRole::BodyMedium.style().rich_para(
                            format!("В списке устройств выбери «{name}»"),
                            s.on_surface,
                        ));
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

/// Not connected: what it takes, and the numbered steps to it — the user's
/// own app on Spotify for Developers (Spotify lets it work only for a
/// Premium owner), its Client ID, the sign-in in the browser.
fn setup_steps(ui: &mut Ui, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    settings_help::item(ui, Id::SpotifyPersonal, "Свой ключ Spotify", |ui| {
        ui.set_max_width(ui.available_width().min(640.0));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(TypeRole::BodyLarge.style().rich("Spotify работает с Premium: нужен свой ключ (2 минуты) и вход", s.on_surface));
            settings_help::help_button(ui, Id::SpotifyPersonal);
        });
        ui.add_space(12.0);
        numbered(ui, 1, 32.0, |ui| {
            if ui
                .add(Button::new("Открыть Spotify for Developers").icon(Icon::OpenInNew).style(Style::Tonal).id_salt("sp-dash"))
                .clicked()
            {
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
            ui.label(TypeRole::BodySmall.style().rich_para("Отметь Web API и Web Playback SDK, сохрани.", s.on_surface_variant));
        });
        numbered(ui, 3, 20.0, |ui| {
            ui.label(TypeRole::BodyMedium.style().rich_para("Скопируй Client ID из Settings приложения:", s.on_surface));
            ui.add_space(4.0);
            let mut field = TextField::new(&mut pages.sp_client_id, "Client ID").placeholder("32 символа");
            if p.spotify_personal_app {
                field = field.supporting("Сохранён. Введи новый, чтобы заменить.");
            }
            field.width(ui.available_width().min(420.0)).id_salt("sp-client-id").show(ui);
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
        ui.add_space(8.0);
    });
}

/// A numbered step of text.
fn step(ui: &mut Ui, n: u32, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    numbered(ui, n, 20.0, |ui| {
        ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface));
    });
}

/// A numbered step: the number in a small tonal circle, centred on the
/// first line (`line` high), and the step's content beside it.
fn numbered(ui: &mut Ui, n: u32, line: f32, add: impl FnOnce(&mut Ui)) {
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

/// «Spotify просит подождать до 14:05»: the service paused our calls.
fn paused_line(ui: &mut Ui, a: &AccountView) {
    let s = Theme::get(ui.ctx()).scheme;
    if let Some(hm) = &a.paused_until_hm {
        ui.horizontal(|ui| {
            Icon::HourglassEmpty.widget(false, s.on_surface_variant, 18.0).ui(ui);
            ui.add(
                egui::Label::new(TypeRole::BodyMedium.style().rich_para(
                    format!("{} просит подождать до {hm} — до тех пор поиск и страницы сервиса молчат", a.service.display_name()),
                    s.on_surface_variant,
                ))
                .wrap(),
            );
        });
        ui.add_space(8.0);
    }
}

/// «Подсказки: 48 треков · обновлено в 14:10»: what the service's own
/// suggestions give our mixes, or why there are none; a feed that needs a
/// new sign-in says so with a quiet way to do it.
fn suggestions_line(ui: &mut Ui, a: &AccountView, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let Some(line) = &a.suggestions else { return };
    ui.horizontal(|ui| {
        Icon::Info.widget(false, s.on_surface_variant, 18.0).ui(ui);
        ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(line, s.on_surface_variant)).wrap());
    });
    if let Some(note) = &a.suggestions_note {
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(note, s.on_surface_variant)).wrap());
            if ui
                .add(
                    Button::new("Переподключить")
                        .style(Style::Text)
                        .size(Size::S)
                        .enabled(can)
                        .id_salt(("reconnect", a.service.as_str())),
                )
                .clicked()
            {
                out.push(Command::StreamLogin { service: a.service, input: LoginInput::Browser });
            }
        });
    }
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
    row(ui, id, "Плейлисты и история", Some("Свои плейлисты и недавние прослушивания; лайки приходят сами"), |ui| {
        if ui
            .add(
                Button::new("Импортировать")
                    .icon(Icon::Download)
                    .style(Style::Tonal)
                    .enabled(ready)
                    .id_salt(("import", service.as_str())),
            )
            .clicked()
        {
            out.push(Command::ImportFromService(service));
        }
    });
}

/// The browser step of a running login: the link (the browser may not
/// have opened by itself) with «Открыть» and «Копировать», or a device code.
fn login_prompt(ui: &mut Ui, a: &AccountView, out: &mut Vec<Command>) {
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

fn login_error(ui: &mut Ui, a: &AccountView) {
    let s = Theme::get(ui.ctx()).scheme;
    let text = a.error.clone().or_else(|| match &a.account {
        Account::Failed(m) => Some(m.clone()),
        _ => None,
    });
    if let Some(t) = text {
        // No Premium is not a fault to fix here: said quietly.
        let (icon, color) = if t == SPOTIFY_PREMIUM_REQUIRED {
            (Icon::Info, s.on_surface_variant)
        } else {
            (Icon::Error, s.error)
        };
        ui.horizontal(|ui| {
            icon.widget(false, color, 18.0).ui(ui);
            ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(t, color)).wrap());
        });
        ui.add_space(8.0);
    }
}

/// Deezer: a catalogue without an account; one switch (and the quiet
/// line of a pause).
fn deezer(ui: &mut Ui, state: &ViewState, p: &mut Prefs) {
    group(ui, "Deezer");
    row(
        ui,
        Id::DeezerEnabled,
        "Использовать Deezer",
        Some("Каталог для поиска, альбомов и дискографий — без аккаунта; звук берётся с YouTube Music"),
        |ui| {
            ui.add(Switch::new(&mut p.deezer_enabled).id_salt("set-dz-on"));
        },
    );
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
    row(ui, Id::YouTubeEnabled, "Использовать YouTube Music", Some("Поиск и звук — без входа; вход — для лайков и плейлистов"), |ui| {
        ui.add(Switch::new(&mut p.youtube_enabled).id_salt("set-yt-on"));
    });
    dimmed(ui, !p.youtube_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        let line = account_line(acc);
        row(ui, Id::YouTube, "Аккаунт", Some(&line), |ui| {
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
                    // The Google window first (lined up with the other controls),
                    // the cookies form as the smaller fallback.
                    if ui
                        .add(
                            Button::new("Войти через Google")
                                .icon(Icon::Login)
                                .style(Style::Filled)
                                .enabled(registered)
                                .id_salt("yt-login"),
                        )
                        .clicked()
                    {
                        pages.yt_form = false;
                        out.push(Command::StreamLogin { service: Service::Youtube, input: LoginInput::Browser });
                    }
                    if ui
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
            suggestions_line(ui, a, !busy, out);
        }
        if !logged_in(acc) && (pages.yt_form || p.youtube_cookies_set) {
            cookies_form(ui, p, pages, registered && !busy, out);
        }
    });
}

/// SoundCloud: the switch, the account (its sign-in window, the token as
/// the fallback), the import and its suggestions.
fn soundcloud(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let acc = state.account(Service::Soundcloud);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "SoundCloud");
    row(ui, Id::SoundCloudEnabled, "Использовать SoundCloud", None, |ui| {
        ui.add(Switch::new(&mut p.soundcloud_enabled).id_salt("set-sc-on"));
    });
    dimmed(ui, !p.soundcloud_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        let line = account_line(acc);
        row(ui, Id::SoundCloud, "Аккаунт", Some(&line), |ui| {
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
                    if ui
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
            suggestions_line(ui, a, !busy, out);
        }
        if !logged_in(acc) && pages.sc_form {
            let can = registered && !busy && p.soundcloud_enabled;
            settings_help::item(ui, Id::SoundCloudToken, "Токен", |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    TextField::new(&mut pages.sc_token, "oauth_token")
                        .password(true)
                        .width(ui.available_width().min(420.0))
                        .id_salt("sc-token")
                        .show(ui);
                    settings_help::help_button(ui, Id::SoundCloudToken);
                });
                ui.add_space(8.0);
                let text = pages.sc_token.trim().to_owned();
                if ui
                    .add(Button::new("Войти").style(Style::Filled).enabled(can && !text.is_empty()).id_salt("sc-token-login"))
                    .clicked()
                {
                    out.push(Command::StreamLogin { service: Service::Soundcloud, input: LoginInput::Cookies(text) });
                    pages.sc_token.clear();
                    pages.sc_form = false;
                }
                ui.add_space(8.0);
            });
        }
    });
}

/// The cookies login: how to get them, a paste field and a file picker.
fn cookies_form(ui: &mut Ui, p: &Prefs, pages: &mut PageState, can: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    settings_help::item(ui, Id::YouTubeCookies, "Вход по cookies", |ui| {
        Card::filled().id_salt("yt-cookies").show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(TypeRole::TitleSmall.style().rich("Вход по cookies", s.on_surface));
                settings_help::help_button(ui, Id::YouTubeCookies);
            });
            ui.add_space(4.0);
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
                ui.add(
                    egui::TextEdit::multiline(&mut pages.yt_cookies)
                        .id_salt("yt-cookies-text")
                        .hint_text(TypeRole::BodyMedium.style().rich(
                            "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t…",
                            s.on_surface_variant,
                        ))
                        .font(TypeRole::BodySmall.font_id())
                        .text_color(s.on_surface)
                        .frame(egui::Frame::NONE)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
            });
            if p.youtube_cookies_set && pages.yt_cookies.is_empty() {
                ui.label(TypeRole::BodySmall.style().rich(
                    "Cookies сохранены в settings.toml — вход повторится при запуске.",
                    s.on_surface_variant,
                ));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui
                    .add(Button::new("Выбрать cookies.txt…").icon(Icon::UploadFile).style(Style::Outlined).id_salt("yt-cookies-file"))
                    .clicked()
                    && let Some(f) = rfd::FileDialog::new()
                        .set_title("cookies.txt для music.youtube.com")
                        .add_filter("cookies.txt", &["txt"])
                        .pick_file()
                {
                    match std::fs::read_to_string(&f) {
                        Ok(t) => pages.yt_cookies = t,
                        Err(e) => log::warn!("cannot read {}: {e}", f.display()),
                    }
                }
                let text = pages.yt_cookies.trim().to_owned();
                if ui
                    .add(Button::new("Войти").style(Style::Filled).enabled(can && !text.is_empty()).id_salt("yt-cookies-login"))
                    .clicked()
                {
                    out.push(Command::StreamLogin { service: Service::Youtube, input: LoginInput::Cookies(text) });
                    pages.yt_cookies.clear();
                    pages.yt_form = false;
                }
                if p.youtube_cookies_set
                    && ui.add(Button::new("Забыть cookies").style(Style::Text).id_salt("yt-cookies-clear")).clicked()
                {
                    out.push(Command::SetStreamSecrets { spotify_client_id: None, youtube_cookies: Some(String::new()) });
                }
            });
        });
        ui.add_space(8.0);
    });
}

/// «Импорт истории»: the export files (years of listening; the services'
/// own imports sit in their blocks above).
fn imports(ui: &mut Ui, p: &mut Prefs, out: &mut Vec<Command>) {
    group(ui, "Импорт истории");
    row(
        ui,
        Id::ImportFiles,
        "Из файлов выгрузки…",
        Some("Вся история за годы: папка или .zip"),
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(Button::new("Папка…").icon(Icon::Folder).style(Style::Tonal).id_salt("import-dir")).clicked()
                {
                    super::dialogs::folder(
                        |d| d.set_title("Папка с выгрузкой Spotify или Google Takeout"),
                        Command::ImportExportFiles,
                        out,
                    );
                }
                if ui.add(Button::new(".zip…").icon(Icon::UploadFile).style(Style::Outlined).id_salt("import-zip")).clicked() {
                    super::dialogs::file(
                        |d| d.set_title("Архив выгрузки Spotify или Google Takeout").add_filter("Архив", &["zip"]),
                        Command::ImportExportFiles,
                        out,
                    );
                }
            });
        },
    );
    row(ui, Id::ImportSavedSongs, "Песни медиатеки YouTube Music", Some("Учат рекомендации, без лайков и загрузок"), |ui| {
        ui.add(Switch::new(&mut p.import_saved_songs).id_salt("set-import-saved"));
    });
    let s = Theme::get(ui.ctx()).scheme;
    ui.label(TypeRole::BodySmall.style().rich_para(
        "Где взять: Spotify — Аккаунт → Конфиденциальность → «Расширенная история стриминга» (готовится до 30 дней); Google — takeout.google.com → YouTube и YouTube Music.",
        s.on_surface_variant,
    ));
}

/// «Soulseek»: where it runs (in the player, or `nikgob-slsk` on a server),
/// the account or the service's address, and the sharing. Its switch is
/// the row in «Источники скачивания».
fn soulseek(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let saved = &state.downloads.soulseek;
    // The saved account fills the fields until something is typed (it may
    // come a moment after the page opens).
    if !pages.slsk_synced && saved.configured() {
        pages.slsk_remote = saved.remote;
        pages.slsk_user = saved.username.clone();
        pages.slsk_url = saved.remote_url.clone();
        pages.slsk_synced = true;
    }
    group(ui, "Soulseek");
    ui.add_space(4.0);
    settings_help::item(ui, Id::Soulseek, "Soulseek", |ui| {
        let (pick, rects) = super::connected(
            ui,
            egui::Id::new("set-slsk-where"),
            &["В плеере", "На сервере"],
            usize::from(pages.slsk_remote),
            &[],
        );
        // The group fills its row: the ⓘ goes right after it, not beside
        // the fields' column further down.
        if let Some(last) = rects.last() {
            settings_help::help_button_beside(ui, Id::Soulseek, *last);
        }
        if let Some(n) = pick {
            pages.slsk_remote = n == 1;
            pages.slsk_synced = true;
        }
        ui.add_space(12.0);
        let ready = if pages.slsk_remote {
            TextField::new(&mut pages.slsk_url, "Адрес утилиты")
                .placeholder("http://…:5031")
                .leading_icon(Icon::Link)
                .width(480.0)
                .id_salt("set-slsk-url")
                .show(ui);
            ui.add_space(8.0);
            let mut token = TextField::new(&mut pages.slsk_token, "Токен").password(true);
            if saved.token_saved {
                token = token.supporting("Токен сохранён. Введи новый, чтобы заменить.");
            }
            token.width(480.0).id_salt("set-slsk-token").show(ui);
            !pages.slsk_url.trim().is_empty()
        } else {
            TextField::new(&mut pages.slsk_user, "Ник")
                .leading_icon(Icon::Person)
                .width(480.0)
                .id_salt("set-slsk-user")
                .show(ui);
            ui.add_space(8.0);
            let mut pass = TextField::new(&mut pages.slsk_pass, "Пароль").password(true);
            if saved.password_saved {
                pass = pass.supporting("Пароль сохранён. Введи новый, чтобы заменить.");
            }
            pass.width(480.0).id_salt("set-slsk-pass").show(ui);
            !pages.slsk_user.trim().is_empty() && (saved.password_saved || !pages.slsk_pass.is_empty())
        };
        ui.add_space(8.0);
        if ui.add(Button::new("Сохранить").style(Style::Tonal).enabled(ready).id_salt("set-slsk-save")).clicked() {
            out.push(Command::SetSoulseek(v::SoulseekForm {
                remote: pages.slsk_remote,
                username: pages.slsk_user.trim().to_owned(),
                password: std::mem::take(&mut pages.slsk_pass),
                remote_url: pages.slsk_url.trim().to_owned(),
                remote_token: std::mem::take(&mut pages.slsk_token),
            }));
        }
    });
    // Without an account there is nothing to share: the rows wait for it.
    let account = saved.configured();
    // Off while there is no account (the saved choice stays).
    let mut share = saved.share && account;
    dimmed(ui, !account, |ui| {
        row(ui, Id::SoulseekShare, "Раздавать библиотеку", Some("Другие могут скачивать у тебя, только чтение"), |ui| {
            ui.add(Switch::new(&mut share).enabled(account).id_salt("set-slsk-share"));
        });
    });
    if account && share != saved.share {
        out.push(Command::SetSoulseekShare { share, upload_limit_kbps: saved.upload_limit_kbps });
    }
    // The limit is written when the slider is let go; the row says the
    // value under the pointer meanwhile, and the one let go until it is saved.
    let held_key = egui::Id::new("set-slsk-ul-held");
    let held = ui.ctx().data(|d| d.get_temp::<u32>(held_key)).filter(|v| *v != saved.upload_limit_kbps);
    dimmed(ui, !account || !saved.share, |ui| {
        row(ui, Id::SoulseekUpload, "Скорость отдачи", Some(&fmt_speed(held.unwrap_or(saved.upload_limit_kbps))), |ui| {
            let (dragging, released) = ui.add_enabled_ui(account, |ui| speed_slider_parts(ui, "set-slsk-ul", saved.upload_limit_kbps)).inner;
            match released.or(dragging) {
                Some(v) => ui.ctx().data_mut(|d| {
                    d.insert_temp(held_key, v);
                }),
                None if held.is_none() => ui.ctx().data_mut(|d| d.remove::<u32>(held_key)),
                None => {}
            }
            if let Some(v) = released
                && v != saved.upload_limit_kbps
            {
                out.push(Command::SetSoulseekShare { share: saved.share, upload_limit_kbps: v });
            }
        });
    });
}

/// «Торренты»: Prowlarr and the seeding policy (moved from «Основные»).
fn torrents(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    group(ui, "Торренты");
    if pages.indexer_url.is_empty() && !p.prowlarr_url.is_empty() {
        pages.indexer_url = p.prowlarr_url.clone();
    }
    ui.add_space(4.0);
    settings_help::item(ui, Id::Prowlarr, "Prowlarr", |ui| {
        TextField::new(&mut pages.indexer_url, "Адрес Prowlarr")
            .placeholder("https://…")
            .leading_icon(Icon::Link)
            .width(480.0)
            .id_salt("set-prowlarr-url")
            .show(ui);
        ui.add_space(8.0);
        let mut key = TextField::new(&mut pages.indexer_key, "API-ключ").password(true);
        if p.prowlarr_configured {
            key = key.supporting("Ключ сохранён. Введи новый, чтобы заменить.");
        }
        key.width(480.0).id_salt("set-prowlarr-key").show(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(
                    Button::new("Сохранить")
                        .style(Style::Tonal)
                        .enabled(!pages.indexer_url.trim().is_empty() && !pages.indexer_key.trim().is_empty())
                        .id_salt("set-prowlarr-save"),
                )
                .clicked()
            {
                out.push(Command::SetIndexer {
                    url: pages.indexer_url.trim().to_owned(),
                    api_key: std::mem::take(&mut pages.indexer_key).trim().to_owned(),
                });
            }
            // Whether Prowlarr answers with what is typed, or else saved (no search).
            let checking = state.indexer_check.as_ref().is_some_and(|c| c.checking);
            let typed = !pages.indexer_url.trim().is_empty() && !pages.indexer_key.trim().is_empty();
            if (p.prowlarr_configured || typed)
                && ui
                    .add(
                        Button::new("Проверить")
                            .style(Style::Text)
                            .enabled(!checking)
                            .id_salt("set-prowlarr-check"),
                    )
                    .clicked()
            {
                out.push(Command::CheckIndexer {
                    url: pages.indexer_url.trim().to_owned(),
                    api_key: pages.indexer_key.trim().to_owned(),
                });
            }
        });
        // The answer, in words; a failure in the error colour (it is one).
        if let Some(c) = &state.indexer_check {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if c.checking {
                    ui.add(LoadingIndicator::new().size(20.0));
                }
                let color = if c.ok || c.checking { s.on_surface_variant } else { s.error };
                ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich_para(&c.text, color)).wrap());
            });
        }
    });
    row(ui, Id::NoSeed, "Не раздавать", Some("Скачанное сразу перестаёт раздаваться"), |ui| {
        ui.add(Switch::new(&mut p.no_seed).id_salt("set-noseed"));
    });
    dimmed(ui, p.no_seed, |ui| {
        row(ui, Id::SeedLimit, "Лимит раздачи", Some(&format!("{} ГБ", p.seed_limit_gb)), |ui| {
            if let Some(v) = range_slider(ui, "set-seed", p.seed_limit_gb as f32, (0.0, 500.0), 5.0, |v| format!("{v:.0} ГБ")) {
                p.seed_limit_gb = v as u32;
            }
        });
        row(ui, Id::SeedDays, "Срок раздачи", Some(&format!("чистка через {} дн.", p.seed_days)), |ui| {
            if let Some(v) = range_slider(ui, "set-seed-days", p.seed_days as f32, (1.0, 90.0), 1.0, |v| format!("{v:.0} дн.")) {
                p.seed_days = v as u32;
            }
        });
        row(
            ui,
            Id::SeedRatio,
            "Ratio",
            Some(&if p.seed_ratio < 0.05 { "без лимита".to_owned() } else { format!("до {:.1}", p.seed_ratio) }),
            |ui| {
                let label = |v: f32| if v < 0.05 { "без лимита".to_owned() } else { format!("{v:.1}") };
                if let Some(v) = range_slider(ui, "set-ratio", p.seed_ratio, (0.0, 5.0), 0.5, label) {
                    p.seed_ratio = v;
                }
            },
        );
    });
    row(ui, Id::DownloadLimit, "Скорость загрузки", Some(&fmt_speed(p.download_limit_kbps)), |ui| {
        if let Some(v) = speed_slider(ui, "set-dl-limit", p.download_limit_kbps) {
            p.download_limit_kbps = v;
        }
    });
    row(ui, Id::UploadLimit, "Скорость отдачи", Some(&fmt_speed(p.upload_limit_kbps)), |ui| {
        if let Some(v) = speed_slider(ui, "set-ul-limit", p.upload_limit_kbps) {
            p.upload_limit_kbps = v;
        }
    });
}

/// Opens a web page in the default browser.
pub(super) fn open_url(url: &str) {
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
pub(super) fn copy_button(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, text: &str) {
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

#[cfg(test)]
mod tests {
    use egui::{Context, Rect, pos2, vec2};
    use nikgob_view::{Command, SoulseekView, ViewState};

    use super::super::PageState;
    use crate::{Theme, typography};

    fn context() -> Context {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        ctx
    }

    /// Every control of «Soulseek» that takes a click is clicked: without
    /// an account the sharing and its limit do nothing, with one they work.
    #[test]
    fn soulseek_sharing_waits_for_an_account() {
        for account in [false, true] {
            let ctx = context();
            let mut state = ViewState::default();
            state.downloads.soulseek = SoulseekView {
                username: if account { "me".into() } else { String::new() },
                password_saved: account,
                share: true,
                ..SoulseekView::default()
            };
            let mut pages = PageState::default();
            let mut frame = 0u32;
            let mut run = |events: Vec<egui::Event>, pages: &mut PageState| {
                let mut out = Vec::new();
                frame += 1;
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 900.0))),
                    time: Some(f64::from(frame) * 0.05),
                    events,
                    ..Default::default()
                };
                let mut done = ctx.run_ui(input, |ui| super::soulseek(ui, &state, pages, &mut out));
                done.textures_delta.clear();
                out
            };
            run(Vec::new(), &mut pages);
            let clickable: Vec<Rect> = ctx.viewport(|v| {
                v.prev_pass
                    .widgets
                    .layers()
                    .flat_map(|(_, w)| w.iter())
                    .filter(|w| w.enabled && w.sense.senses_click())
                    .map(|w| w.interact_rect)
                    .collect()
            });
            assert!(!clickable.is_empty());
            let mut sent = Vec::new();
            for r in clickable {
                pages.slsk_remote = false;
                let at = r.center();
                let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
                sent.extend(run(vec![egui::Event::PointerMoved(at), button(true)], &mut pages));
                sent.extend(run(vec![button(false)], &mut pages));
                sent.extend(run(vec![egui::Event::PointerGone], &mut pages));
            }
            let sharing = sent.iter().filter(|c| matches!(c, Command::SetSoulseekShare { .. })).count();
            if account {
                assert!(sharing >= 2, "the switch and the limit: {sent:?}");
            } else {
                assert_eq!(sharing, 0, "{sent:?}");
            }
        }
    }
}
