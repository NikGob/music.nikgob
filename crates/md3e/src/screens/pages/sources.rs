//! Настройки → «Источники» (docs/stage3.md §0 "Где настройки", §3.1, §3.6):
//! Spotify and YouTube Music (switch, account, sound, personal app,
//! cookies), what a like downloads, playlist sync, cold-start imports, and
//! the torrents (Prowlarr, seeding) moved here from «Основные».

use egui::{Sense, Stroke, Ui, Widget as _, vec2};
use nikgob_view::{Account, AccountView, Command, LoginInput, LoginPrompt, Prefs, Service, ViewState};

use super::PageState;
use super::settings::{choice, dimmed, fmt_speed, group, range_slider, row, speed_slider};
use super::settings_help::{self, SettingId as Id};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Card, LoadingIndicator, Size, Style, Switch, TextField},
};

/// Where a personal Spotify app is made and what it must allow.
const DASHBOARD: &str = "https://developer.spotify.com/dashboard";
const REDIRECT_URI: &str = "http://127.0.0.1:8989/login";

pub fn show(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(12.0);
    ui.label(TypeRole::BodyMedium.style().rich_para(
        "Каждый сервис включается отдельно, и всё работает бесплатно: Spotify Free, YouTube Music без Premium и торренты.",
        s.on_surface_variant,
    ));
    spotify(ui, state, p, pages, out);
    youtube(ui, state, p, pages, out);

    group(ui, "Лайки и загрузки");
    row(ui, Id::LikeDownload, "Что качать по лайку", Some("Лайк альбома — всегда весь альбом"), |ui| {
        choice(ui, Id::LikeDownload, p);
    });
    row(ui, Id::PushLikes, "Лайки обратно в сервисы", Some("Лайк здесь ставится и в Spotify / YouTube Music"), |ui| {
        ui.add(Switch::new(&mut p.push_likes).id_salt("set-push-likes"));
    });
    let n = p.auto_download_limit.clamp(1, 8);
    row(ui, Id::AutoDownloads, "Одновременных автозагрузок", Some(&format!("{n} из «Хочу скачать» за раз")), |ui| {
        if let Some(v) = range_slider(ui, "set-auto-dl", n as f32, (1.0, 8.0), 1.0, |v| format!("{v:.0}")) {
            p.auto_download_limit = v as u32;
        }
    });

    group(ui, "Плейлисты");
    row(
        ui,
        Id::PlaylistSync,
        "Синхронизация по умолчанию",
        Some("Для новых плейлистов из сервисов; у каждого — в его меню"),
        |ui| choice(ui, Id::PlaylistSync, p),
    );

    imports(ui, state, out);
    torrents(ui, p, pages, out);
}

/// "Вошёл как …" and friends.
fn account_line(a: Option<&AccountView>) -> String {
    let Some(a) = a.filter(|a| a.registered) else {
        return "Не подключён в этой сборке".to_owned();
    };
    match &a.account {
        Account::LoggedOut => "Не выполнен вход".to_owned(),
        Account::LoggingIn(Some(LoginPrompt::Window(_))) => "Вход… заверши его в открывшемся окне".to_owned(),
        Account::LoggingIn(_) => "Вход… заверши его в браузере".to_owned(),
        Account::LoggedIn { name, premium, .. } => match (a.service, premium) {
            (Service::Spotify, Some(true)) => format!("Вошёл как {name} · Premium"),
            (Service::Spotify, Some(false)) => format!("Вошёл как {name} · без Premium"),
            _ => format!("Вошёл как {name}"),
        },
        Account::Failed(_) => "Вход не удался".to_owned(),
    }
}

fn logged_in(a: Option<&AccountView>) -> bool {
    a.is_some_and(|a| a.account.is_logged_in())
}

fn spotify(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let acc = state.account(Service::Spotify);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "Spotify");
    row(ui, Id::SpotifyEnabled, "Spotify", Some("Поиск, лайки, плейлисты; звук — с Premium"), |ui| {
        ui.add(Switch::new(&mut p.spotify_enabled).id_salt("set-sp-on"));
    });
    dimmed(ui, !p.spotify_enabled, |ui| {
        let busy = acc.is_some_and(|a| a.busy || matches!(a.account, Account::LoggingIn(_)));
        let line = account_line(acc);
        row(ui, Id::Spotify, "Аккаунт", Some(&line), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if busy {
                    ui.add(LoadingIndicator::new().size(32.0));
                }
                if logged_in(acc) {
                    if ui.add(Button::new("Выйти").icon(Icon::Logout).style(Style::Outlined).id_salt("sp-logout")).clicked() {
                        out.push(Command::StreamLogout(Service::Spotify));
                    }
                } else if !busy {
                    let failed = acc.is_some_and(|a| a.error.is_some() || matches!(a.account, Account::Failed(_)));
                    let label = if failed { "Попробовать снова" } else { "Войти через Spotify" };
                    if ui
                        .add(
                            Button::new(label)
                                .icon(Icon::Login)
                                .style(Style::Filled)
                                .enabled(registered)
                                .id_salt("sp-login"),
                        )
                        .clicked()
                    {
                        out.push(Command::StreamLogin { service: Service::Spotify, input: LoginInput::Browser });
                    }
                }
            });
        });
        if let Some(a) = acc {
            login_prompt(ui, a, out);
            login_error(ui, a);
        }
        let audio = acc.is_some_and(|a| a.audio);
        let sound_line = if audio {
            "Включён: треки Spotify играют из Spotify"
        } else {
            "Нужен Premium. Без него те же треки играют с YouTube Music"
        };
        row(ui, Id::SpotifyPlayback, "Звук из Spotify", Some(sound_line), |ui| {
            if audio {
                ui.label(TypeRole::LabelLarge.style().rich("✓ включён", s.primary));
            } else if ui
                .add(
                    Button::new("Включить звук")
                        .icon(Icon::GraphicEq)
                        .style(Style::Tonal)
                        .enabled(logged_in(acc) && !busy)
                        .id_salt("sp-playback"),
                )
                .on_disabled_hover_text("Сначала войди в Spotify")
                .clicked()
            {
                out.push(Command::StreamLogin { service: Service::Spotify, input: LoginInput::Playback });
            }
        });
        personal_app(ui, p, pages, registered, out);
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
        ui.horizontal(|ui| {
            Icon::Error.widget(false, s.error, 18.0).ui(ui);
            ui.label(TypeRole::BodyMedium.style().rich_para(t, s.error));
        });
        ui.add_space(8.0);
    }
}

/// «Личное приложение (быстрее)»: folded; a short how-to, the Client ID.
fn personal_app(ui: &mut Ui, p: &Prefs, pages: &mut PageState, registered: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    settings_help::item(ui, Id::SpotifyPersonal, "Личное приложение (быстрее)", |ui| {
        ui.add_space(4.0);
        let head = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let icon = if pages.sp_personal_open { Icon::ExpandLess } else { Icon::ExpandMore };
                icon.widget(false, s.on_surface_variant, 24.0).ui(ui);
                ui.label(TypeRole::BodyLarge.style().rich("Личное приложение (быстрее)", s.on_surface));
                settings_help::help_button(ui, Id::SpotifyPersonal);
                if p.spotify_personal_app {
                    ui.add_space(8.0);
                    ui.label(TypeRole::LabelMedium.style().rich("задано", s.primary));
                }
            })
            .response;
        let hit = ui.interact(head.rect, egui::Id::new("sp-personal-head"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if hit.clicked() {
            pages.sp_personal_open = !pages.sp_personal_open;
        }
        if !pages.sp_personal_open {
            ui.add_space(8.0);
            return;
        }
        ui.add_space(8.0);
        Card::filled().id_salt("sp-personal").show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.label(TypeRole::BodyMedium.style().rich_para(
                "Своя квота запросов Spotify: поиск и лайки быстрее. Нужно один раз завести приложение (владельцу нужен Premium):",
                s.on_surface_variant,
            ));
            ui.add_space(8.0);
            step(ui, 1, "Открой панель разработчика и создай приложение (Create app).");
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                if ui.add(Button::new("developer.spotify.com").icon(Icon::OpenInNew).style(Style::Text).size(Size::S).id_salt("sp-dash")).clicked() {
                    open_url(DASHBOARD);
                }
            });
            step(ui, 2, "В Redirect URIs вставь адрес:");
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 36.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add_space(28.0);
                ui.label(TypeRole::BodyMedium.emphasized().rich(REDIRECT_URI, s.on_surface));
                copy_button(ui, "sp-redirect-copy", REDIRECT_URI);
            });
            step(ui, 3, "Отметь Web API, сохрани и скопируй Client ID из Settings.");
            ui.add_space(8.0);
            TextField::new(&mut pages.sp_client_id, "Client ID")
                .placeholder("32 символа из Settings приложения")
                .supporting(if p.spotify_personal_app {
                    "Client ID сохранён в settings.toml. Введи новый, чтобы заменить."
                } else {
                    "Хранится в settings.toml, не в настройках интерфейса."
                })
                .leading_icon(Icon::Link)
                .width(420.0)
                .id_salt("sp-client-id")
                .show(ui);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let id = pages.sp_client_id.trim().to_owned();
                if ui
                    .add(
                        Button::new("Сохранить и войти")
                            .style(Style::Tonal)
                            .enabled(registered && id.len() >= 16)
                            .id_salt("sp-personal-save"),
                    )
                    .clicked()
                {
                    out.push(Command::SetStreamSecrets { spotify_client_id: Some(id.clone()), youtube_cookies: None });
                    out.push(Command::StreamLogin {
                        service: Service::Spotify,
                        input: LoginInput::PersonalApp { client_id: id },
                    });
                    pages.sp_client_id.clear();
                }
                if p.spotify_personal_app
                    && ui.add(Button::new("Убрать").style(Style::Text).id_salt("sp-personal-clear")).clicked()
                {
                    out.push(Command::SetStreamSecrets { spotify_client_id: Some(String::new()), youtube_cookies: None });
                }
            });
        });
        ui.add_space(8.0);
    });
}

fn step(ui: &mut Ui, n: u32, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 10.0, s.secondary_container);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            TypeRole::LabelMedium.font_id(),
            s.on_secondary_container,
        );
        ui.add_space(4.0);
        ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface));
    });
    ui.add_space(4.0);
}

fn youtube(ui: &mut Ui, state: &ViewState, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
    let acc = state.account(Service::Youtube);
    let registered = acc.is_some_and(|a| a.registered);
    group(ui, "YouTube Music");
    row(ui, Id::YouTubeEnabled, "YouTube Music", Some("Поиск и звук работают без входа"), |ui| {
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
        }
        if !logged_in(acc) && (pages.yt_form || p.youtube_cookies_set) {
            cookies_form(ui, p, pages, registered && !busy, out);
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

/// «Импорт»: the services' APIs and the export files (cold start).
fn imports(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    group(ui, "Импорт");
    for (id, service, label, hint) in [
        (Id::ImportSpotify, Service::Spotify, "Из Spotify", "Лайки, свои плейлисты и недавние прослушивания"),
        (Id::ImportYouTube, Service::Youtube, "Из YouTube Music", "То же из YouTube Music; нужен вход"),
    ] {
        let ready = state.prefs.service_enabled(service) && logged_in(state.account(service));
        row(ui, id, label, Some(hint), |ui| {
            if ui
                .add(
                    Button::new("Импортировать")
                        .icon(Icon::Download)
                        .style(Style::Tonal)
                        .enabled(ready)
                        .id_salt(("import", service.as_str())),
                )
                .on_disabled_hover_text("Сначала войди в сервис выше")
                .clicked()
            {
                out.push(Command::ImportFromService(service));
            }
        });
    }
    row(
        ui,
        Id::ImportFiles,
        "Из файлов выгрузки…",
        Some("Вся история за годы: папка или .zip"),
        |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ui.add(Button::new("Папка…").icon(Icon::Folder).style(Style::Tonal).id_salt("import-dir")).clicked()
                    && let Some(d) = rfd::FileDialog::new().set_title("Папка с выгрузкой Spotify или Google Takeout").pick_folder()
                {
                    out.push(Command::ImportExportFiles(d));
                }
                if ui.add(Button::new(".zip…").icon(Icon::UploadFile).style(Style::Outlined).id_salt("import-zip")).clicked()
                    && let Some(f) = rfd::FileDialog::new()
                        .set_title("Архив выгрузки Spotify или Google Takeout")
                        .add_filter("Архив", &["zip"])
                        .pick_file()
                {
                    out.push(Command::ImportExportFiles(f));
                }
            });
        },
    );
    let s = Theme::get(ui.ctx()).scheme;
    ui.label(TypeRole::BodySmall.style().rich_para(
        "Где взять: Spotify — Аккаунт → Конфиденциальность → «Расширенная история стриминга» (готовится до 30 дней); Google — takeout.google.com → YouTube и YouTube Music.",
        s.on_surface_variant,
    ));
}

/// «Торренты»: Prowlarr and the seeding policy (moved from «Основные»).
fn torrents(ui: &mut Ui, p: &mut Prefs, pages: &mut PageState, out: &mut Vec<Command>) {
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
        TextField::new(&mut pages.indexer_key, "API-ключ")
            .password(true)
            .supporting(if p.prowlarr_configured {
                "Ключ сохранён. Введи новый, чтобы заменить."
            } else {
                "Хранится в settings.toml, в коде его нет."
            })
            .width(480.0)
            .id_salt("set-prowlarr-key")
            .show(ui);
        ui.add_space(8.0);
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
    if let Err(e) = std::process::Command::new("explorer").arg(url).spawn() {
        log::warn!("cannot open {url}: {e}");
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
