//! Settings tab pages (music, lyrics, look, services, downloads, network, system, devices, dev).

use super::{Locale, PluralForms, common_in, locale, plural_forms};

/// Static strings: settings tab pages (music, lyrics, look, services, downloads, network, system, devices, dev).
pub struct SettingsTabsStrings {
    // Group and sub titles.
    pub g_likes: &'static str,
    pub g_playlists: &'static str,
    pub g_streaming: &'static str,
    pub g_import_history: &'static str,
    pub g_sources: &'static str,
    pub g_on_like: &'static str,
    pub g_quality: &'static str,
    pub g_torrents: &'static str,
    pub g_seeding: &'static str,
    pub g_dev: &'static str,
    pub g_api: &'static str,
    pub g_devices: &'static str,
    pub g_you: &'static str,
    pub g_startup: &'static str,
    pub g_notifications: &'static str,
    pub g_load: &'static str,
    pub g_library: &'static str,
    pub g_shuffle: &'static str,
    pub g_sound: &'static str,
    pub g_album_songs: &'static str,
    pub g_recs: &'static str,
    pub g_dislikes: &'static str,
    pub g_theme: &'static str,
    pub g_library_album: &'static str,
    pub g_window: &'static str,
    pub g_right_pane: &'static str,
    pub g_track_screen: &'static str,
    pub g_cover: &'static str,
    pub g_ring: &'static str,
    pub g_background: &'static str,
    pub g_progress_volume: &'static str,
    pub g_controls: &'static str,
    pub g_buttons: &'static str,
    pub models: &'static str,
    pub g_lyrics_search: &'static str,
    pub g_lyrics_auto: &'static str,
    pub g_lyrics_look: &'static str,
    pub g_sung_line: &'static str,
    pub g_full_text: &'static str,
    pub g_song_pause: &'static str,
    // Buttons, labels, notes.
    pub not_in_this_build: &'static str,
    pub logout: &'static str,
    pub connect: &'static str,
    pub enable_sound: &'static str,
    pub key_rejected: &'static str,
    pub download: &'static str,
    pub enter_key: &'static str,
    pub turn_off: &'static str,
    pub turn_on: &'static str,
    pub solo_sound_title: &'static str,
    pub not_now: &'static str,
    pub solo_key_title: &'static str,
    pub solo_login_premium: &'static str,
    pub solo_create_key: &'static str,
    pub solo_open_key_page: &'static str,
    pub solo_paste_here: &'static str,
    pub key_label: &'static str,
    pub solo_pair_title: &'static str,
    pub solo_pair_step1: &'static str,
    pub open_dashboard: &'static str,
    pub create_app: &'static str,
    pub mark_apis: &'static str,
    pub copy_client_id: &'static str,
    pub saved: &'static str,
    pub chars32: &'static str,
    pub login_spotify: &'static str,
    pub reconnect: &'static str,
    pub import: &'static str,
    pub finish_in_browser: &'static str,
    pub open_page_enter_code: &'static str,
    pub browser_not_opened: &'static str,
    pub finish_in_window: &'static str,
    pub login_google: &'static str,
    pub paste_cookies: &'static str,
    pub login: &'static str,
    pub paste_token: &'static str,
    pub cookies_title: &'static str,
    pub cookies_step1: &'static str,
    pub cookies_step2: &'static str,
    pub cookies_step3: &'static str,
    pub cookies_saved: &'static str,
    pub pick_cookies_file: &'static str,
    pub cookies_file_title: &'static str,
    pub forget_cookies: &'static str,
    pub folder_btn: &'static str,
    pub import_dir_title: &'static str,
    pub import_zip_title: &'static str,
    pub archive_filter: &'static str,
    pub copied: &'static str,
    pub restore: &'static str,
    pub need_prowlarr: &'static str,
    pub need_account: &'static str,
    pub not_connected: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub util_address: &'static str,
    pub token_label: &'static str,
    pub nick: &'static str,
    pub password: &'static str,
    pub prowlarr_address: &'static str,
    pub api_key_label: &'static str,
    pub check: &'static str,
    pub configure: &'static str,
    pub system_default: &'static str,
    pub model_failed: &'static str,
    pub gpu_unavailable: &'static str,
    pub show: &'static str,
    pub contact_placeholder: &'static str,
    pub remove_key: &'static str,
    pub remove_token: &'static str,
    pub reset_demo_title: &'static str,
    pub name_hint: &'static str,
    pub need_youtube: &'static str,
    pub move_btn: &'static str,
    pub masters_dir_title: &'static str,
    pub open_folder: &'static str,
    pub this_computer: &'static str,
    pub add_device: &'static str,
    pub network_map: &'static str,
    pub no_settings_file: &'static str,
    pub mesh_off: &'static str,
    pub open_docs: &'static str,
    pub new_token: &'static str,
    pub token_agent_pc: &'static str,
    pub token_assistant: &'static str,
    pub for_agent_pc: &'static str,
    pub for_assistant: &'static str,
    pub player_no_answer: &'static str,
    pub revoke: &'static str,
    pub create: &'static str,
    pub token_name: &'static str,
    pub token_placeholder: &'static str,
    pub rights: &'static str,
    pub who_uses: &'static str,
    pub token_failed: &'static str,
    pub local_only_notice: &'static str,
    pub enable_home_network: &'static str,
    pub beta: &'static str,
    // Units, suffixes and slider zero labels.
    pub no_limit: &'static str,
    pub no_cap: &'static str,
    pub kibps: &'static str,
    pub mibps: &'static str,
    pub suffix_mb: &'static str,
    pub suffix_gb: &'static str,
    pub suffix_sec: &'static str,
    pub suffix_ms: &'static str,
    pub suffix_pct: &'static str,
    pub days: PluralForms,
    // "ago" line.
    pub just_now: &'static str,
    pub yesterday: &'static str,
    pub ago: &'static str,
}

pub static RU: SettingsTabsStrings = SettingsTabsStrings {
    g_likes: "Лайки",
    g_playlists: "Плейлисты",
    g_streaming: "Стриминг",
    g_import_history: "Импорт истории",
    g_sources: "Откуда качать",
    g_on_like: "По лайку",
    g_quality: "Качество",
    g_torrents: "Торренты",
    g_seeding: "Раздача",
    g_dev: "Для разработчика",
    g_api: "API и MCP",
    g_devices: "Устройства",
    g_you: "Ты",
    g_startup: "Запуск",
    g_notifications: "Уведомления",
    g_load: "Нагрузка",
    g_library: "Библиотека",
    g_shuffle: "Перемешивание",
    g_sound: "Звук",
    g_album_songs: "Песни из альбомов",
    g_recs: "Рекомендации",
    g_dislikes: "Не нравится",
    g_theme: "Тема",
    g_library_album: "Библиотека и альбом",
    g_window: "Окно",
    g_right_pane: "Панель справа",
    g_track_screen: "Экран трека",
    g_cover: "Обложка",
    g_ring: "Кольцо",
    g_background: "Фон",
    g_progress_volume: "Полоса и громкость",
    g_controls: "Управление",
    g_buttons: "Кнопки",
    models: "Модели",
    g_lyrics_search: "Поиск текста",
    g_lyrics_auto: "Подсветка сама",
    g_lyrics_look: "Вид текста",
    g_sung_line: "Строка, которую поют",
    g_full_text: "Весь текст",
    g_song_pause: "Пауза в песне",
    not_in_this_build: "Не подключён в этой сборке",
    logout: "Выйти",
    connect: "Подключить",
    enable_sound: "Включить звук",
    key_rejected: "Ключ не подошёл",
    download: "Скачать",
    enter_key: "Ввести ключ",
    turn_off: "Выключить",
    turn_on: "Включить",
    solo_sound_title: "Звук Spotify",
    not_now: "Не сейчас",
    solo_key_title: "Ключ Soloist",
    solo_login_premium: "Войди в аккаунт Spotify с Premium",
    solo_create_key: "Создай ключ и скопируй его",
    solo_open_key_page: "Открыть страницу ключа",
    solo_paste_here: "Вставь его сюда",
    key_label: "Ключ",
    solo_pair_title: "Подключить Soloist",
    solo_pair_step1: "Открой Spotify на телефоне или компьютере в этой же сети",
    open_dashboard: "Открыть Spotify for Developers",
    create_app: "Создай приложение (Create app), Redirect URI:",
    mark_apis: "Отметь Web API и Web Playback SDK, сохрани.",
    copy_client_id: "Скопируй Client ID из Settings приложения:",
    saved: "Сохранён",
    chars32: "32 символа",
    login_spotify: "Войти через Spotify",
    reconnect: "Переподключить",
    import: "Импортировать",
    finish_in_browser: "Заверши вход в браузере",
    open_page_enter_code: "Открой страницу и введи код:",
    browser_not_opened: "Если браузер не открылся сам, открой эту ссылку:",
    finish_in_window: "Заверши вход в окне",
    login_google: "Войти через Google",
    paste_cookies: "Вставить cookies…",
    login: "Войти",
    paste_token: "Вставить токен…",
    cookies_title: "Вход по cookies",
    cookies_step1: "Войди на music.youtube.com в браузере.",
    cookies_step2: "Расширением вроде «Get cookies.txt LOCALLY» выгрузи cookies.txt для этого сайта.",
    cookies_step3: "Выбери файл или вставь его текст (или заголовок Cookie) ниже.",
    cookies_saved: "Cookies сохранены",
    pick_cookies_file: "Выбрать cookies.txt…",
    cookies_file_title: "cookies.txt для music.youtube.com",
    forget_cookies: "Забыть cookies",
    folder_btn: "Папка…",
    import_dir_title: "Папка с выгрузкой Spotify или Google Takeout",
    import_zip_title: "Архив выгрузки Spotify или Google Takeout",
    archive_filter: "Архив",
    copied: "Скопировано",
    restore: "Вернуть",
    need_prowlarr: "Нужен Prowlarr",
    need_account: "Нужен аккаунт",
    not_connected: "Не подключился",
    up: "Выше",
    down: "Ниже",
    util_address: "Адрес утилиты",
    token_label: "Токен",
    nick: "Ник",
    password: "Пароль",
    prowlarr_address: "Адрес Prowlarr",
    api_key_label: "API-ключ",
    check: "Проверить",
    configure: "Настроить",
    system_default: "Системное по умолчанию",
    model_failed: "Не получилось загрузить модель",
    gpu_unavailable: "Видеокарта недоступна",
    show: "Показать",
    contact_placeholder: "почта или адрес сайта",
    remove_key: "Убрать ключ",
    remove_token: "Убрать токен",
    reset_demo_title: "Сбросить демо-профиль?",
    name_hint: "Имя",
    need_youtube: "Нужен YouTube Music",
    move_btn: "Перенести…",
    masters_dir_title: "Выбери диск или папку для FLAC-оригиналов",
    open_folder: "Открыть папку",
    this_computer: "Этот компьютер",
    add_device: "Добавить устройство",
    network_map: "Карта сети",
    no_settings_file: "Нет файла настроек",
    mesh_off: "Связь с устройствами выключена",
    open_docs: "Открыть /docs",
    new_token: "Новый токен",
    token_agent_pc: "Агент на этом ПК",
    token_assistant: "Голосовой ассистент",
    for_agent_pc: "Для агента на этом ПК",
    for_assistant: "Для голосового ассистента",
    player_no_answer: "Плеер не ответил",
    revoke: "Отозвать",
    create: "Создать",
    token_name: "Название",
    token_placeholder: "Ассистент на Pi",
    rights: "Права",
    who_uses: "Кто включает",
    token_failed: "Не удалось создать токен",
    local_only_notice: "Доступ открыт только на этом компьютере",
    enable_home_network: "Включить домашнюю сеть",
    beta: "Бета",
    no_limit: "без лимита",
    no_cap: "без предела",
    kibps: "КиБ/с",
    mibps: "МиБ/с",
    suffix_mb: " МБ",
    suffix_gb: " ГБ",
    suffix_sec: " с",
    suffix_ms: " мс",
    suffix_pct: " %",
    days: PluralForms { one: "дн.", few: "дн.", many: "дн." },
    just_now: "только что",
    yesterday: "вчера",
    ago: "назад",
};

pub static EN: SettingsTabsStrings = SettingsTabsStrings {
    g_likes: "Likes",
    g_playlists: "Playlists",
    g_streaming: "Streaming",
    g_import_history: "Import history",
    g_sources: "Download sources",
    g_on_like: "When liked",
    g_quality: "Quality",
    g_torrents: "Torrents",
    g_seeding: "Seeding",
    g_dev: "Developer",
    g_api: "API & MCP",
    g_devices: "Devices",
    g_you: "You",
    g_startup: "Startup",
    g_notifications: "Notifications",
    g_load: "Performance",
    g_library: "Library",
    g_shuffle: "Shuffle",
    g_sound: "Sound",
    g_album_songs: "Album songs",
    g_recs: "Recommendations",
    g_dislikes: "Dislikes",
    g_theme: "Theme",
    g_library_album: "Library & album",
    g_window: "Window",
    g_right_pane: "Right panel",
    g_track_screen: "Track screen",
    g_cover: "Cover",
    g_ring: "Ring",
    g_background: "Background",
    g_progress_volume: "Progress & volume",
    g_controls: "Controls",
    g_buttons: "Buttons",
    models: "Models",
    g_lyrics_search: "Lyrics search",
    g_lyrics_auto: "Auto highlight",
    g_lyrics_look: "Lyrics look",
    g_sung_line: "Sung line",
    g_full_text: "Full text",
    g_song_pause: "Song pause",
    not_in_this_build: "Not available in this build",
    logout: "Sign out",
    connect: "Connect",
    enable_sound: "Enable audio",
    key_rejected: "Key didn't work",
    download: "Download",
    enter_key: "Enter key",
    turn_off: "Turn off",
    turn_on: "Turn on",
    solo_sound_title: "Spotify audio",
    not_now: "Not now",
    solo_key_title: "Soloist key",
    solo_login_premium: "Sign in to a Spotify Premium account",
    solo_create_key: "Create a key and copy it",
    solo_open_key_page: "Open the key page",
    solo_paste_here: "Paste it here",
    key_label: "Key",
    solo_pair_title: "Connect Soloist",
    solo_pair_step1: "Open Spotify on a phone or computer on the same network",
    open_dashboard: "Open Spotify for Developers",
    create_app: "Create an app (Create app), Redirect URI:",
    mark_apis: "Enable Web API and Web Playback SDK, then save.",
    copy_client_id: "Copy the Client ID from the app's Settings:",
    saved: "Saved",
    chars32: "32 characters",
    login_spotify: "Sign in with Spotify",
    reconnect: "Reconnect",
    import: "Import",
    finish_in_browser: "Finish signing in in the browser",
    open_page_enter_code: "Open the page and enter the code:",
    browser_not_opened: "If the browser didn't open by itself, open this link:",
    finish_in_window: "Finish signing in in the window",
    login_google: "Sign in with Google",
    paste_cookies: "Paste cookies…",
    login: "Sign in",
    paste_token: "Paste token…",
    cookies_title: "Cookie sign-in",
    cookies_step1: "Sign in to music.youtube.com in a browser.",
    cookies_step2: "With an extension like “Get cookies.txt LOCALLY”, export cookies.txt for this site.",
    cookies_step3: "Choose a file or paste its text (or the Cookie header) below.",
    cookies_saved: "Cookies saved",
    pick_cookies_file: "Choose cookies.txt…",
    cookies_file_title: "cookies.txt for music.youtube.com",
    forget_cookies: "Forget cookies",
    folder_btn: "Folder…",
    import_dir_title: "Folder with a Spotify or Google Takeout export",
    import_zip_title: "Spotify or Google Takeout export archive",
    archive_filter: "Archive",
    copied: "Copied",
    restore: "Restore",
    need_prowlarr: "Prowlarr needed",
    need_account: "Account needed",
    not_connected: "Couldn't connect",
    up: "Up",
    down: "Down",
    util_address: "Utility address",
    token_label: "Token",
    nick: "Username",
    password: "Password",
    prowlarr_address: "Prowlarr address",
    api_key_label: "API key",
    check: "Check",
    configure: "Configure",
    system_default: "System default",
    model_failed: "Couldn't download the model",
    gpu_unavailable: "GPU unavailable",
    show: "Show",
    contact_placeholder: "email or website",
    remove_key: "Remove key",
    remove_token: "Remove token",
    reset_demo_title: "Reset the demo profile?",
    name_hint: "Name",
    need_youtube: "YouTube Music needed",
    move_btn: "Move…",
    masters_dir_title: "Choose a disk or folder for FLAC masters",
    open_folder: "Open folder",
    this_computer: "This computer",
    add_device: "Add device",
    network_map: "Network map",
    no_settings_file: "No settings file",
    mesh_off: "Device connection is off",
    open_docs: "Open /docs",
    new_token: "New token",
    token_agent_pc: "Agent on this PC",
    token_assistant: "Voice assistant",
    for_agent_pc: "For an agent on this PC",
    for_assistant: "For a voice assistant",
    player_no_answer: "The player didn't respond",
    revoke: "Revoke",
    create: "Create",
    token_name: "Name",
    token_placeholder: "Assistant on Pi",
    rights: "Permissions",
    who_uses: "Who can use",
    token_failed: "Couldn't create the token",
    local_only_notice: "Access is open only to this computer",
    enable_home_network: "Enable home network",
    beta: "Beta",
    no_limit: "no limit",
    no_cap: "no cap",
    kibps: "KiB/s",
    mibps: "MiB/s",
    suffix_mb: " MB",
    suffix_gb: " GB",
    suffix_sec: " sec",
    suffix_ms: " ms",
    suffix_pct: "%",
    days: PluralForms { one: "day", few: "days", many: "days" },
    just_now: "just now",
    yesterday: "yesterday",
    ago: "ago",
};

locale_table! {
    /// Returns these strings for `loc`.
    settings_tabs_in -> SettingsTabsStrings,
    /// Returns these strings for the active process locale.
    settings_tabs,
    { Ru => RU, En => EN }
}

/// Decimal separator of `loc` (`,` in Russian, `.` in English).
pub const fn decimal_point_in(loc: Locale) -> char {
    match loc {
        Locale::Ru => ',',
        Locale::En => '.',
    }
}

/// The snippet tabs of the API token dialog in `loc`.
pub const fn snippets_in(loc: Locale) -> [&'static str; 4] {
    match loc {
        Locale::Ru => ["Claude Code", "mcpServers JSON", "curl", "MCP по сети"],
        Locale::En => ["Claude Code", "mcpServers JSON", "curl", "MCP on the network"],
    }
}

/// «Доступ» of the API: this computer or the home network.
pub const fn api_access_in(loc: Locale) -> [&'static str; 2] {
    match loc {
        Locale::Ru => ["Этот компьютер", "Домашняя сеть"],
        Locale::En => ["This computer", "Home network"],
    }
}

/// «Оригиналы FLAC на ПК»: always, after one copy elsewhere, after two.
pub const fn keep_masters_in(loc: Locale) -> [&'static str; 3] {
    match loc {
        Locale::Ru => ["Всегда", "После 1 копии", "После 2 копий"],
        Locale::En => ["Always", "After 1 copy", "After 2 copies"],
    }
}

/// Where Soulseek runs: in the player or on a server.
pub const fn slsk_where_in(loc: Locale) -> [&'static str; 2] {
    match loc {
        Locale::Ru => ["В плеере", "На сервере"],
        Locale::En => ["In the player", "On a server"],
    }
}

/// The places of «Меню трека»: in the menu, in «Файл», hidden.
pub const fn track_menu_places_in(loc: Locale) -> [&'static str; 3] {
    match loc {
        Locale::Ru => ["В меню", "В «Файл»", "Скрыт"],
        Locale::En => ["In menu", "In “File”", "Hidden"],
    }
}

/// How long ago `then_ms` was, in `loc` («5 мин назад» / «5 min ago», «вчера» / «yesterday»).
pub fn ago_in(loc: Locale, now_ms: i64, then_ms: i64) -> String {
    let t = settings_tabs_in(loc);
    let c = common_in(loc);
    let s = (now_ms - then_ms).max(0) / 1000;
    match s {
        0..60 => t.just_now.to_owned(),
        60..3600 => format!("{} {} {}", s / 60, c.min_short, t.ago),
        3600..86_400 => format!("{} {} {}", s / 3600, c.hr_short, t.ago),
        86_400..172_800 => t.yesterday.to_owned(),
        _ => {
            let d = s / 86_400;
            format!("{} {} {}", d, plural_forms(loc, d, &t.days), t.ago)
        }
    }
}

/// How long ago `then_ms` was, in the active locale.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    ago_in(locale(), now_ms, then_ms)
}

/// Formats a speed limit in `loc` («без лимита», «500 КиБ/с», «1,5 МиБ/с» / «no limit», «500 KiB/s», «1.5 MiB/s»).
pub fn fmt_speed_in(loc: Locale, kbps: u32) -> String {
    let t = settings_tabs_in(loc);
    match kbps {
        0 => t.no_limit.to_owned(),
        k if k < 1024 => format!("{k} {}", t.kibps),
        k => format!("{:.1} {}", k as f32 / 1024.0, t.mibps).replace('.', decimal_point_in(loc)),
    }
}

/// Formats a speed limit in the active locale.
pub fn fmt_speed(kbps: u32) -> String {
    fmt_speed_in(locale(), kbps)
}

/// Formats a byte size in `loc` («317 МБ», «1,1 ГБ» / «317 MB», «1.1 GB»).
pub fn fmt_size_in(loc: Locale, bytes: u64) -> String {
    let c = common_in(loc);
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1000.0 { format!("{:.1} {}", mb / 1024.0, c.gb).replace('.', decimal_point_in(loc)) } else { format!("{mb:.0} {}", c.mb) }
}

/// Formats a byte size in the active locale.
pub fn fmt_size(bytes: u64) -> String {
    fmt_size_in(locale(), bytes)
}

/// The approximate size shown in the Soloist download dialog, in `loc` (« (≈25 МБ)» / « (≈25 MB)»).
pub fn fmt_solo_size_in(loc: Locale, bytes: u64) -> String {
    format!(" (≈{} {})", (bytes + 524_288) / 1_048_576, common_in(loc).mb)
}

/// The Soloist download dialog's text in `loc` (`size` already formatted).
pub fn fmt_solo_need_in(loc: Locale, size: &str) -> String {
    match loc {
        Locale::Ru => format!("Чтобы играл звук из Spotify, нужен Spotify Soloist — официальный плеер Spotify{size}."),
        Locale::En => format!("To play Spotify audio, you need Spotify Soloist, the official Spotify player{size}."),
    }
}

/// The pairing dialog's second step in `loc` («В списке устройств выбери «{name}»» / `In the device list, choose “{name}”`).
pub fn fmt_pick_device_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("В списке устройств выбери «{name}»"),
        Locale::En => format!("In the device list, choose “{name}”"),
    }
}

/// «{service} просит подождать до {hm}» / `"{service} asks to wait until {hm}"`.
pub fn fmt_asks_wait_until_in(loc: Locale, service: &str, hm: &str) -> String {
    match loc {
        Locale::Ru => format!("{service} просит подождать до {hm}"),
        Locale::En => format!("{service} asks to wait until {hm}"),
    }
}

/// «{service} не отдаёт звук до {hm}» / `"{service} won't play audio until {hm}"`.
pub fn fmt_no_audio_until_in(loc: Locale, service: &str, hm: &str) -> String {
    match loc {
        Locale::Ru => format!("{service} не отдаёт звук до {hm}"),
        Locale::En => format!("{service} won't play audio until {hm}"),
    }
}

/// The models row's summary in `loc` («{ready} из {total} · {size}» / `"{ready} of {total} · {size}"`).
pub fn fmt_models_summary_in(loc: Locale, ready: usize, total: usize, size: &str) -> String {
    match loc {
        Locale::Ru => format!("{ready} из {total} · {size}"),
        Locale::En => format!("{ready} of {total} · {size}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_in_both_locales() {
        let now = 10_000_000_000;
        assert_eq!(ago_in(Locale::Ru, now, now - 20_000), "только что");
        assert_eq!(ago_in(Locale::Ru, now, now - 5 * 60_000), "5 мин назад");
        assert_eq!(ago_in(Locale::Ru, now, now - 3 * 3_600_000), "3 ч назад");
        assert_eq!(ago_in(Locale::Ru, now, now - 30 * 3_600_000), "вчера");
        assert_eq!(ago_in(Locale::Ru, now, now - 86_400_000), "1 дн. назад");
        assert_eq!(ago_in(Locale::Ru, now, now - 5 * 86_400_000), "5 дн. назад");
        assert_eq!(ago_in(Locale::En, now, now - 20_000), "just now");
        assert_eq!(ago_in(Locale::En, now, now - 5 * 60_000), "5 min ago");
        assert_eq!(ago_in(Locale::En, now, now - 3 * 3_600_000), "3 hr ago");
        assert_eq!(ago_in(Locale::En, now, now - 30 * 3_600_000), "yesterday");
        assert_eq!(ago_in(Locale::En, now, now - 86_400_000), "1 day ago");
        assert_eq!(ago_in(Locale::En, now, now - 5 * 86_400_000), "5 days ago");
        assert_eq!(ago_in(Locale::Ru, now, now + 5_000), "только что", "a clock a little ahead");
    }

    #[test]
    fn speeds_and_sizes_format_in_both_locales() {
        assert_eq!(fmt_speed_in(Locale::Ru, 0), "без лимита");
        assert_eq!(fmt_speed_in(Locale::Ru, 500), "500 КиБ/с");
        assert_eq!(fmt_speed_in(Locale::Ru, 1536), "1,5 МиБ/с");
        assert_eq!(fmt_speed_in(Locale::En, 0), "no limit");
        assert_eq!(fmt_speed_in(Locale::En, 500), "500 KiB/s");
        assert_eq!(fmt_speed_in(Locale::En, 1536), "1.5 MiB/s");
        assert_eq!(fmt_size_in(Locale::Ru, 317 * 1_048_576), "317 МБ");
        assert_eq!(fmt_size_in(Locale::Ru, 1_200_000_000), "1,1 ГБ");
        assert_eq!(fmt_size_in(Locale::En, 317 * 1_048_576), "317 MB");
        assert_eq!(fmt_size_in(Locale::En, 1_200_000_000), "1.1 GB");
        assert_eq!(fmt_solo_size_in(Locale::Ru, 26_733_824), " (≈25 МБ)");
        assert_eq!(fmt_solo_size_in(Locale::En, 26_733_824), " (≈25 MB)");
    }

    #[test]
    fn valued_strings_format_in_both_locales() {
        assert_eq!(
            fmt_solo_need_in(Locale::Ru, " (≈25 МБ)"),
            "Чтобы играл звук из Spotify, нужен Spotify Soloist — официальный плеер Spotify (≈25 МБ)."
        );
        assert_eq!(
            fmt_solo_need_in(Locale::En, " (≈25 MB)"),
            "To play Spotify audio, you need Spotify Soloist, the official Spotify player (≈25 MB)."
        );
        assert_eq!(fmt_pick_device_in(Locale::Ru, "Pi"), "В списке устройств выбери «Pi»");
        assert_eq!(fmt_pick_device_in(Locale::En, "Pi"), "In the device list, choose “Pi”");
        assert_eq!(fmt_asks_wait_until_in(Locale::Ru, "Spotify", "14:05"), "Spotify просит подождать до 14:05");
        assert_eq!(fmt_asks_wait_until_in(Locale::En, "Spotify", "14:05"), "Spotify asks to wait until 14:05");
        assert_eq!(fmt_no_audio_until_in(Locale::Ru, "Spotify", "14:05"), "Spotify не отдаёт звук до 14:05");
        assert_eq!(fmt_no_audio_until_in(Locale::En, "Spotify", "14:05"), "Spotify won't play audio until 14:05");
        assert_eq!(fmt_models_summary_in(Locale::Ru, 2, 5, "300 МБ"), "2 из 5 · 300 МБ");
        assert_eq!(fmt_models_summary_in(Locale::En, 2, 5, "300 MB"), "2 of 5 · 300 MB");
    }

    #[test]
    fn label_arrays_cover_both_locales() {
        assert_eq!(snippets_in(Locale::Ru)[3], "MCP по сети");
        assert_eq!(snippets_in(Locale::En)[3], "MCP on the network");
        assert_eq!(api_access_in(Locale::Ru), ["Этот компьютер", "Домашняя сеть"]);
        assert_eq!(api_access_in(Locale::En), ["This computer", "Home network"]);
        assert_eq!(keep_masters_in(Locale::Ru), ["Всегда", "После 1 копии", "После 2 копий"]);
        assert_eq!(keep_masters_in(Locale::En), ["Always", "After 1 copy", "After 2 copies"]);
        assert_eq!(slsk_where_in(Locale::Ru), ["В плеере", "На сервере"]);
        assert_eq!(slsk_where_in(Locale::En), ["In the player", "On a server"]);
        assert_eq!(track_menu_places_in(Locale::Ru), ["В меню", "В «Файл»", "Скрыт"]);
        assert_eq!(track_menu_places_in(Locale::En), ["In menu", "In “File”", "Hidden"]);
        assert_eq!(decimal_point_in(Locale::Ru), ',');
        assert_eq!(decimal_point_in(Locale::En), '.');
    }
}
