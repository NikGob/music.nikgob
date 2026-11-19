//! Error messages and common toast notifications.

use super::Locale;

/// Static error and toast strings.
pub struct ErrorStrings {
    pub nothing_to_play: &'static str,
    pub failed_save_prefs: &'static str,
    pub failed_prefix: &'static str,
    pub spotify_need_premium: &'static str,
    pub spotify_enable_playback: &'static str,
    pub spotify_unavailable: &'static str,
    pub spotify_audio_enabled: &'static str,
    pub spotify_connected_enable_audio: &'static str,
    pub enable_audio: &'static str,
    pub find_song: &'static str,
    pub sign_in: &'static str,
    pub playback_failed: &'static str,
    pub no_file_for_track: &'static str,
    pub audio_output_failed: &'static str,
    pub not_on_youtube: &'static str,
    pub no_settings_path: &'static str,
    pub settings_unreadable: &'static str,
    pub masters_move_unavailable: &'static str,
    pub masters_inside_work_dir: &'static str,
    pub sources_file_failed: &'static str,
    pub prowlarr_invalid: &'static str,
    pub prowlarr_saved: &'static str,
    pub cover_load_failed: &'static str,
    pub saved: &'static str,
    pub key_removed: &'static str,
    pub token_removed: &'static str,
    pub album_download_failed: &'static str,
    pub album_like_failed: &'static str,
    pub soulseek_connected: &'static str,
    pub soulseek_enable_in_sources: &'static str,
    pub soulseek_daemon_unresponsive: &'static str,
    pub soulseek_not_connected: &'static str,
    pub stream_not_configured: &'static str,
    pub stream_not_logged_in: &'static str,
    pub stream_need_premium: &'static str,
    pub stream_unavailable: &'static str,
    pub stream_unsupported: &'static str,
    pub stream_age_restricted: &'static str,
    pub stream_region_blocked: &'static str,
    pub stream_drm: &'static str,
    pub stream_network: &'static str,
    pub stream_parse: &'static str,
    pub save_setting_failed: &'static str,
    pub link_playlist_sign_in: &'static str,
    pub stream_track_stale: &'static str,
    pub song_stale: &'static str,
    pub nothing_to_share: &'static str,
    pub youtube_save_tracks_only: &'static str,
    pub youtube_disabled: &'static str,
    pub not_found_on_youtube: &'static str,
    pub mix_gone_home: &'static str,
    pub mix_gone_world: &'static str,
    pub tile_gone_home: &'static str,
    pub downloaded_only_no_new: &'static str,
    pub discover_still_loading: &'static str,
    pub nothing_to_download: &'static str,
    pub recs_still_building: &'static str,
    pub blend_impossible: &'static str,
    pub import_unavailable: &'static str,
    pub deleting_unavailable: &'static str,
    pub could_not_delete: &'static str,
    pub restored: &'static str,
    pub restore_partly: &'static str,
    pub purge_partly: &'static str,
    pub import_failed_file: &'static str,
    pub import_started: &'static str,
    pub import_cancelling: &'static str,
    pub torrent: &'static str,
    pub import: &'static str,
    pub masters_folder_in_use: &'static str,
    pub torrents_unavailable: &'static str,
    pub torrent_client_failed: &'static str,
    pub import_no_pause: &'static str,
    pub search_stale: &'static str,
    pub prowlarr_no_torrent: &'static str,
    pub adding_torrent: &'static str,
    pub torrent_gone: &'static str,
    pub already_in_wishlist: &'static str,
    pub everything_downloaded: &'static str,
    pub downloads_with_source: &'static str,
    pub finding_on_youtube: &'static str,
    pub could_not_read_torrent: &'static str,
    pub not_a_magnet: &'static str,
    pub could_not_create_folder: &'static str,
    pub files_gone: &'static str,
    pub need_download_source: &'static str,
    pub search_later: &'static str,
    pub nothing_found_to_download: &'static str,
}

impl ErrorStrings {
    /// Formats `"Could not forget “{name}”: {why}"` / `"Не удалось забыть «{name}»: {why}"`.
    pub fn mesh_forget_failed_in(loc: Locale, name: Option<&str>, why: &str) -> String {
        match loc {
            Locale::Ru => format!("Не удалось забыть «{}»: {why}", name.unwrap_or("устройство")),
            Locale::En => format!("Could not forget “{}”: {why}", name.unwrap_or("device")),
        }
    }

    /// Formats `"Could not move FLAC: {detail}"` / `"Не удалось перенести FLAC: {detail}"`.
    pub fn masters_move_failed_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Не удалось перенести FLAC: {detail}"),
            Locale::En => format!("Could not move FLAC: {detail}"),
        }
    }

    /// Formats the named-masters-in-album-folders refusal detail.
    pub fn masters_named_in_albums_in(loc: Locale, dir: &str, count: usize) -> String {
        match loc {
            Locale::Ru => format!("в {dir} лежат FLAC-оригиналы в папках альбомов ({count})"),
            Locale::En => format!("{dir} has FLAC masters stored inside album folders ({count})"),
        }
    }

    /// Formats a BLAKE3 / size verification failure detail for `path`.
    pub fn hash_verification_failed_in(
        loc: Locale,
        path: &str,
        expected: &str,
        expected_size: Option<u64>,
        actual: &str,
        actual_size: Option<u64>,
    ) -> String {
        match (loc, expected_size, actual_size) {
            (Locale::Ru, Some(es), Some(asize)) => format!(
                "проверка хеша не прошла для {path}: ожидалось {expected} ({es} B), получено {actual} ({asize} B)"
            ),
            (Locale::En, Some(es), Some(asize)) => format!(
                "hash verification failed for {path}: expected {expected} ({es} B), got {actual} ({asize} B)"
            ),
            (Locale::Ru, _, _) => {
                format!("проверка хеша не прошла для {path}: ожидалось {expected}, получено {actual}")
            }
            (Locale::En, _, _) => {
                format!("hash verification failed for {path}: expected {expected}, got {actual}")
            }
        }
    }

    /// Formats a sidecar verification failure detail for `path`.
    pub fn sidecar_verification_failed_in(loc: Locale, path: &str) -> String {
        match loc {
            Locale::Ru => format!("проверка метаданных не прошла для {path}"),
            Locale::En => format!("sidecar verification failed for {path}"),
        }
    }

    /// Formats a hash check failure of a moved file for `path`.
    pub fn hash_check_failed_in(loc: Locale, path: &str) -> String {
        match loc {
            Locale::Ru => format!("проверка хеша не прошла для {path}"),
            Locale::En => format!("hash verification failed for {path}"),
        }
    }

    /// Formats a staged copy that did not match its original: `tmp` against `src`.
    pub fn hash_copy_mismatch_in(loc: Locale, tmp: &str, src: &str) -> String {
        match loc {
            Locale::Ru => format!("проверка хеша не прошла для {tmp}: копия {src} не совпала с оригиналом"),
            Locale::En => format!("hash verification failed for {tmp}: the copy {src} did not match the original"),
        }
    }

    /// Formats the API listener's «Порт {port} занят» for the settings row.
    pub fn api_port_busy_in(loc: Locale, port: u16) -> String {
        match loc {
            Locale::Ru => format!("Порт {port} занят"),
            Locale::En => format!("Port {port} is busy"),
        }
    }

    /// Formats «Порт {port} занят другой программой» (the sign-in's redirect port).
    pub fn api_port_busy_other_in(loc: Locale, port: u16) -> String {
        match loc {
            Locale::Ru => format!("Порт {port} занят другой программой"),
            Locale::En => format!("Port {port} is used by another program"),
        }
    }

    /// Formats «У компьютера нет адреса {ip}» for the settings row.
    pub fn api_no_address_in(loc: Locale, ip: &str) -> String {
        match loc {
            Locale::Ru => format!("У компьютера нет адреса {ip}"),
            Locale::En => format!("The computer has no address {ip}"),
        }
    }

    /// Formats «Нет доступа к порту {port}» for the settings row.
    pub fn api_port_denied_in(loc: Locale, port: u16) -> String {
        match loc {
            Locale::Ru => format!("Нет доступа к порту {port}"),
            Locale::En => format!("No access to port {port}"),
        }
    }

    /// Formats «Не удалось открыть порт {port}: {e}» for the settings row.
    pub fn api_port_failed_in(loc: Locale, port: u16, e: &str) -> String {
        match loc {
            Locale::Ru => format!("Не удалось открыть порт {port}: {e}"),
            Locale::En => format!("Could not open port {port}: {e}"),
        }
    }

    /// Formats `"Sync failed: {detail}"` / `"Синхронизация не удалась: {detail}"`.
    pub fn sync_failed_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Синхронизация не удалась: {detail}"),
            Locale::En => format!("Sync failed: {detail}"),
        }
    }

    /// Formats `"Could not update playlist on service: {detail}"` / `"Не получилось обновить плейлист в сервисе: {detail}"`.
    pub fn sync_update_failed_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Не получилось обновить плейлист в сервисе: {detail}"),
            Locale::En => format!("Could not update playlist on service: {detail}"),
        }
    }

    /// Formats `"Could not create playlist on {service}: {detail}"` / `"Не получилось создать плейлист в {service}: {detail}"`.
    pub fn sync_create_failed_in(loc: Locale, service_name: &str, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Не получилось создать плейлист в {service_name}: {detail}"),
            Locale::En => format!("Could not create playlist on {service_name}: {detail}"),
        }
    }

    /// Formats `"Could not save from YouTube: {detail}"` / `"Не получилось сохранить с YouTube: {detail}"`.
    pub fn youtube_save_failed_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Не получилось сохранить с YouTube: {detail}"),
            Locale::En => format!("Could not save from YouTube: {detail}"),
        }
    }

    /// Formats `"Sign in to {service} first"` / `"Сначала войди в {service}"`.
    pub fn sign_in_service_first_in(loc: Locale, service_name: &str) -> String {
        match loc {
            Locale::Ru => format!("Сначала войди в {service_name}"),
            Locale::En => format!("Sign in to {service_name} first"),
        }
    }

    /// Formats `"likes: {detail}"` / `"лайки: {detail}"`.
    pub fn cold_problem_likes_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("лайки: {detail}"),
            Locale::En => format!("likes: {detail}"),
        }
    }

    /// Formats `"history: {detail}"` / `"история: {detail}"`.
    pub fn cold_problem_history_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("история: {detail}"),
            Locale::En => format!("history: {detail}"),
        }
    }
    /// Formats `"Service asks to wait[ until {hm}]"` / `"Сервис просит подождать[ до {hm}]"`.
    pub fn stream_rate_limited_in(loc: Locale, until_hm: Option<&str>) -> String {
        match loc {
            Locale::Ru => match until_hm {
                Some(hm) => format!("Сервис просит подождать до {hm}"),
                None => "Сервис просит подождать".into(),
            },
            Locale::En => match until_hm {
                Some(hm) => format!("Service asks to wait until {hm}"),
                None => "Service asks to wait".into(),
            },
        }
    }

    /// Formats `"Service returned an error (HTTP {status})"` / `"Сервис ответил ошибкой (HTTP {status})"`.
    pub fn stream_http_in(loc: Locale, status: u16) -> String {
        match loc {
            Locale::Ru => format!("Сервис ответил ошибкой (HTTP {status})"),
            Locale::En => format!("Service returned an error (HTTP {status})"),
        }
    }

    /// Formats `"Instances file: {path}"` / `"Файл инстансов: {path}"`.
    pub fn instances_file_in(loc: Locale, path: &str) -> String {
        match loc {
            Locale::Ru => format!("Файл инстансов: {path}"),
            Locale::En => format!("Instances file: {path}"),
        }
    }

    /// Formats `"Failed: {detail}"` / `"Не получилось: {detail}"`.
    pub fn failed_with_in(loc: Locale, detail: &str) -> String {
        format!("{}: {detail}", errors_in(loc).failed_prefix)
    }

    /// Formats `"Error: {detail}"` / `"Ошибка: {detail}"`.
    pub fn error_with_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Ошибка: {detail}"),
            Locale::En => format!("Error: {detail}"),
        }
    }
    /// Formats `"{title}: {who} asks to wait[ until {hm}] — try playing this track later"`.
    pub fn resolve_rate_limited_in(loc: Locale, title: &str, who: &str, until_hm: Option<&str>) -> String {
        match loc {
            Locale::Ru => match until_hm {
                Some(hm) => format!("{title}: {who} просит подождать до {hm} — включи трек позже"),
                None => format!("{title}: {who} просит подождать — включи трек позже"),
            },
            Locale::En => match until_hm {
                Some(hm) => format!("{title}: {who} asks to wait until {hm} — try playing this track later"),
                None => format!("{title}: {who} asks to wait — try playing this track later"),
            },
        }
    }

    /// Formats `"{who} asks to wait[ until {hm}] — will resume automatically"`.
    pub fn pause_will_resume_in(loc: Locale, who: &str, until_hm: Option<&str>) -> String {
        match loc {
            Locale::Ru => match until_hm {
                Some(hm) => format!("{who} просит подождать до {hm} — продолжу сам"),
                None => format!("{who} просит подождать — продолжу сам"),
            },
            Locale::En => match until_hm {
                Some(hm) => format!("{who} asks to wait until {hm} — will resume automatically"),
                None => format!("{who} asks to wait — will resume automatically"),
            },
        }
    }

    /// Formats `"{service} connected"` / `"{service} подключён"`.
    pub fn service_connected_in(loc: Locale, service_name: &str) -> String {
        match loc {
            Locale::Ru => format!("{service_name} подключён"),
            Locale::En => format!("{service_name} connected"),
        }
    }

    /// Formats `"{service} not connected"` / `"{service} не подключён"`.
    pub fn service_not_connected_in(loc: Locale, service_name: &str) -> String {
        match loc {
            Locale::Ru => format!("{service_name} не подключён"),
            Locale::En => format!("{service_name} not connected"),
        }
    }

    /// Formats `"{service} disabled"` / `"{service} выключен"`.
    pub fn service_disabled_in(loc: Locale, service_name: &str) -> String {
        match loc {
            Locale::Ru => format!("{service_name} выключен"),
            Locale::En => format!("{service_name} disabled"),
        }
    }

    /// Formats `"{title}: not found on {service}"` / `"{title}: не нашёл эту песню на {service}"`.
    pub fn song_not_found_on_in(loc: Locale, title: &str, service_name: &str) -> String {
        match loc {
            Locale::Ru => format!("{title}: не нашёл эту песню на {service_name}"),
            Locale::En => format!("{title}: not found on {service_name}"),
        }
    }

    /// Formats `"в файле «X» другая песня — «Y»"` / `"another song is in
    /// the file “X” — “Y”"`.
    pub fn fmt_wrong_audio_in(loc: Locale, title: &str, sounds_like: Option<&str>) -> String {
        match (loc, sounds_like) {
            (Locale::Ru, Some(s)) => format!("в файле «{title}» другая песня — «{s}»"),
            (Locale::Ru, None) => format!("в файле «{title}» другая песня"),
            (Locale::En, Some(s)) => format!("another song is in the file “{title}” — “{s}”"),
            (Locale::En, None) => format!("another song is in the file “{title}”"),
        }
    }

    /// Formats `"7 tracks in the Trash (30 days)"` / `"7 треков в корзине (30 дней)"`.
    pub fn fmt_in_trash_in(loc: Locale, what: &str) -> String {
        match loc {
            Locale::Ru => format!("{what} в корзине (30 дней)"),
            Locale::En => format!("{what} in the Trash (30 days)"),
        }
    }

    /// Formats `"Deleted for good: 7"` / `"Удалено навсегда: 7"`.
    pub fn fmt_purged_in(loc: Locale, n: usize) -> String {
        match loc {
            Locale::Ru => format!("Удалено навсегда: {n}"),
            Locale::En => format!("Deleted for good: {n}"),
        }
    }

    /// Formats `"FLAC masters moved: 3 (hash checked, old ones deleted)"`.
    pub fn fmt_masters_moved_in(loc: Locale, n: usize) -> String {
        match loc {
            Locale::Ru => format!("Перенесено FLAC-оригиналов: {n} (проверено по хешу, старые удалены)"),
            Locale::En => format!("FLAC masters moved: {n} (hash checked, old ones deleted)"),
        }
    }

    /// Formats `"FLAC folder: C:\flac"` / `"Папка FLAC: C:\flac"`.
    pub fn fmt_flac_folder_in(loc: Locale, dir: &str) -> String {
        match loc {
            Locale::Ru => format!("Папка FLAC: {dir}"),
            Locale::En => format!("FLAC folder: {dir}"),
        }
    }

    /// Formats `"Could not fetch the .torrent: {detail}"`.
    pub fn fmt_could_not_fetch_torrent_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Не скачать .torrent: {detail}"),
            Locale::En => format!("Could not fetch the .torrent: {detail}"),
        }
    }

    /// Formats `"Downloading: {name}"` / `"Качается: {name}"`.
    pub fn fmt_downloading_named_in(loc: Locale, name: &str) -> String {
        match loc {
            Locale::Ru => format!("Качается: {name}"),
            Locale::En => format!("Downloading: {name}"),
        }
    }

    /// Formats `"Downloaded: {name}"` / `"Скачано: {name}"`.
    pub fn fmt_downloaded_named_in(loc: Locale, name: &str) -> String {
        match loc {
            Locale::Ru => format!("Скачано: {name}"),
            Locale::En => format!("Downloaded: {name}"),
        }
    }

    /// Formats `"Could not add the torrent: {detail}"`.
    pub fn fmt_could_not_add_torrent_in(loc: Locale, detail: &str) -> String {
        match loc {
            Locale::Ru => format!("Не получилось добавить раздачу: {detail}"),
            Locale::En => format!("Could not add the torrent: {detail}"),
        }
    }

    /// Formats `"Nothing better found: {title}"` / `"Лучше не нашёл: {title}"`.
    pub fn fmt_better_not_found_in(loc: Locale, title: &str) -> String {
        match loc {
            Locale::Ru => format!("Лучше не нашёл: {title}"),
            Locale::En => format!("Nothing better found: {title}"),
        }
    }

    /// Formats `"Looking for better quality: “{title}”"`.
    pub fn fmt_searching_better_in(loc: Locale, title: &str) -> String {
        match loc {
            Locale::Ru => format!("Ищу качество лучше: «{title}»"),
            Locale::En => format!("Looking for better quality: “{title}”"),
        }
    }

    /// Formats `"“{album}”: replaced 3 files · old ones in the Trash"`.
    pub fn fmt_album_replaced_in(loc: Locale, album: &str, n: usize, files: &str) -> String {
        match loc {
            Locale::Ru => format!("«{album}»: заменил {n} {files} · прежние в Корзине"),
            Locale::En => format!("“{album}”: replaced {n} {files} · old ones in the Trash"),
        }
    }

    /// Formats `"Previous files restored: 3"`.
    pub fn fmt_files_back_in(loc: Locale, done: usize) -> String {
        match loc {
            Locale::Ru => format!("Вернул прежние файлы: {done}"),
            Locale::En => format!("Previous files restored: {done}"),
        }
    }

    /// Formats `"Previous files restored: 3, failed: 1"`.
    pub fn fmt_files_back_partly_in(loc: Locale, done: usize, failed: usize) -> String {
        match loc {
            Locale::Ru => format!("Вернул прежние файлы: {done}, не получилось: {failed}"),
            Locale::En => format!("Previous files restored: {done}, failed: {failed}"),
        }
    }

    /// Formats `"In Wishlist: 3 tracks"` / `"В «Хочу скачать»: 3 трека"`.
    pub fn fmt_wishlist_added_in(loc: Locale, n: u64, tracks: &str) -> String {
        match loc {
            Locale::Ru => format!("В «Хочу скачать»: {n} {tracks}"),
            Locale::En => format!("In Wishlist: {n} {tracks}"),
        }
    }

    /// Formats `"In Wishlist: “Song”"` / `"В «Хочу скачать»: «Song»"`.
    pub fn fmt_wishlist_added_one_in(loc: Locale, what: &str) -> String {
        match loc {
            Locale::Ru => format!("В «Хочу скачать»: {what}"),
            Locale::En => format!("In Wishlist: {what}"),
        }
    }

    /// Formats `"Won't download “{title}”"` / `"Не буду качать «{title}»"`.
    pub fn fmt_wont_download_one_in(loc: Locale, title: &str) -> String {
        match loc {
            Locale::Ru => format!("Не буду качать «{title}»"),
            Locale::En => format!("Won't download “{title}”"),
        }
    }

    /// Formats `"Won't download: 3"` / `"Не буду качать: 3"`.
    pub fn fmt_wont_download_in(loc: Locale, n: usize) -> String {
        match loc {
            Locale::Ru => format!("Не буду качать: {n}"),
            Locale::En => format!("Won't download: {n}"),
        }
    }

    /// Formats `"Queued for download: 3"` / `"В очереди на скачивание: 3"`.
    pub fn fmt_queued_for_download_in(loc: Locale, n: usize) -> String {
        match loc {
            Locale::Ru => format!("В очереди на скачивание: {n}"),
            Locale::En => format!("Queued for download: {n}"),
        }
    }

    /// Formats `"Searching again: 3"` / `"Ищу заново: 3"`.
    pub fn fmt_searching_again_in(loc: Locale, n: usize) -> String {
        match loc {
            Locale::Ru => format!("Ищу заново: {n}"),
            Locale::En => format!("Searching again: {n}"),
        }
    }

    /// Formats `"{what} · downloads once a download source is on"`.
    pub fn fmt_downloads_with_source_in(loc: Locale, what: &str) -> String {
        match loc {
            Locale::Ru => format!("{what} · скачается, когда включишь источник скачивания"),
            Locale::En => format!("{what} · downloads once a download source is on"),
        }
    }
}

pub static RU: ErrorStrings = ErrorStrings {
    nothing_to_play: "Нечего играть",
    failed_save_prefs: "Не удалось сохранить настройки",
    failed_prefix: "Не получилось",
    spotify_need_premium: "для звука из Spotify нужен Premium",
    spotify_enable_playback: "для звука из Spotify включи воспроизведение в настройках",
    spotify_unavailable: "трек недоступен",
    spotify_audio_enabled: "Звук из Spotify включён",
    spotify_connected_enable_audio: "Spotify подключён · осталось включить звук",
    enable_audio: "Включить звук",
    find_song: "Найти песню…",
    sign_in: "Войти",
    playback_failed: "Не получилось воспроизвести трек",
    no_file_for_track: "Нет файла для этого трека",
    audio_output_failed: "Не удалось открыть устройство вывода звука",
    not_on_youtube: "Нет на YouTube Music",
    no_settings_path: "Некуда сохранить настройки",
    settings_unreadable: "settings.toml не читается — поправь файл вручную",
    masters_move_unavailable: "Перенос папки FLAC недоступен",
    masters_inside_work_dir: "Нельзя хранить FLAC-архив внутри папки музыки, Inbox или загрузок",
    sources_file_failed: "Не получилось создать файл инстансов",
    prowlarr_invalid: "Адрес или ключ Prowlarr не подходят",
    prowlarr_saved: "Prowlarr сохранён",
    cover_load_failed: "Не получилось взять картинку",
    saved: "Сохранено",
    key_removed: "Ключ убран",
    token_removed: "Токен убран",
    album_download_failed: "Не получилось скачать альбом",
    album_like_failed: "Не получилось лайкнуть альбом",
    soulseek_connected: "Soulseek подключён",
    soulseek_enable_in_sources: "Soulseek сохранён — включи его в «Источниках скачивания»",
    soulseek_daemon_unresponsive: "Soulseek сохранён, но утилита не отвечает",
    soulseek_not_connected: "Soulseek сохранён, но не подключился",
    stream_not_configured: "Сервис не настроен",
    stream_not_logged_in: "Нужно войти",
    stream_need_premium: "Нужен Premium",
    stream_unavailable: "Недоступно",
    stream_unsupported: "Сервис так не умеет",
    stream_age_restricted: "Только для 18+ — нужно войти",
    stream_region_blocked: "Недоступно в твоём регионе",
    stream_drm: "Только в защищённом виде — сыграет с YouTube Music",
    stream_network: "Нет связи — проверь интернет",
    stream_parse: "Сервис ответил непонятно",
    save_setting_failed: "Не удалось сохранить настройку",
    link_playlist_sign_in: "Войди в Spotify или YouTube Music, чтобы связать плейлист",
    stream_track_stale: "Трек устарел — найди его заново",
    song_stale: "Песня устарела — найди её заново",
    nothing_to_share: "Нечем поделиться",
    youtube_save_tracks_only: "С YouTube сохраняется только трек, не альбом",
    youtube_disabled: "YouTube Music выключен",
    not_found_on_youtube: "Не нашёл этот трек на YouTube Music",
    mix_gone_home: "Этого микса уже нет — главная обновилась",
    mix_gone_world: "Этого микса уже нет — страница мира обновилась",
    tile_gone_home: "Этой подборки уже нет — главная обновилась",
    downloaded_only_no_new: "Включено «Только скачанное» — нового сейчас нет",
    discover_still_loading: "Новое ещё ищется — загляни чуть позже",
    nothing_to_download: "Нечего скачивать",
    recs_still_building: "Рекомендации ещё собираются — попробуй чуть позже",
    blend_impossible: "Из этих профилей микса не собрать",
    import_unavailable: "Импорт недоступен",
    deleting_unavailable: "Удаление недоступно",
    could_not_delete: "Не получилось удалить",
    restored: "Восстановлено",
    restore_partly: "Восстановить удалось не всё",
    purge_partly: "Удалено, но не все файлы получилось стереть",
    import_failed_file: "Не получилось импортировать файл",
    import_started: "Импорт начался · смотри «Загрузки»",
    import_cancelling: "Отменяю импорт — добавленное уберу",
    torrent: "Торрент",
    import: "Импорт",
    masters_folder_in_use: "Эта папка FLAC уже используется",
    torrents_unavailable: "Торренты недоступны",
    torrent_client_failed: "Не запустился торрент-клиент",
    import_no_pause: "Импорт нельзя поставить на паузу — его можно только отменить",
    search_stale: "Результат поиска устарел — найди заново",
    prowlarr_no_torrent: "Prowlarr не отдал торрент",
    adding_torrent: "Добавляю раздачу…",
    torrent_gone: "торрент пропал из сессии",
    already_in_wishlist: "Уже в «Хочу скачать»",
    everything_downloaded: "Всё уже скачано",
    downloads_with_source: "скачается, когда включишь источник скачивания",
    finding_on_youtube: "Ищу трек на YouTube Music…",
    could_not_read_torrent: "Не получилось прочитать .torrent",
    not_a_magnet: "Это не magnet-ссылка",
    could_not_create_folder: "Не получилось создать папку для загрузки",
    files_gone: "Скачанные файлы пропали — поищу снова",
    need_download_source: "Нужен источник скачивания",
    search_later: "Не то — поищу позже",
    nothing_found_to_download: "Не нашёл, где скачать",
};

pub static EN: ErrorStrings = ErrorStrings {
    nothing_to_play: "Nothing to play",
    failed_save_prefs: "Could not save settings",
    failed_prefix: "Failed",
    spotify_need_premium: "Spotify Premium is required for audio",
    spotify_enable_playback: "enable Spotify playback in settings",
    spotify_unavailable: "track unavailable",
    spotify_audio_enabled: "Spotify audio enabled",
    spotify_connected_enable_audio: "Spotify connected · enable audio next",
    enable_audio: "Enable audio",
    find_song: "Find song…",
    sign_in: "Sign in",
    playback_failed: "Failed to play track",
    no_file_for_track: "No file for this track",
    audio_output_failed: "Could not open audio output device",
    not_on_youtube: "Not on YouTube Music",
    no_settings_path: "Nowhere to save settings",
    settings_unreadable: "Could not read settings.toml — fix the file manually",
    masters_move_unavailable: "Moving the FLAC archive folder is unavailable",
    masters_inside_work_dir: "Cannot store the FLAC archive inside the music, Inbox, or downloads folder",
    sources_file_failed: "Could not create instances file",
    prowlarr_invalid: "Invalid Prowlarr URL or API key",
    prowlarr_saved: "Prowlarr saved",
    cover_load_failed: "Could not load image",
    saved: "Saved",
    key_removed: "Key removed",
    token_removed: "Token removed",
    album_download_failed: "Could not download album",
    album_like_failed: "Could not like album",
    soulseek_connected: "Soulseek connected",
    soulseek_enable_in_sources: "Soulseek saved — enable it in Download Sources",
    soulseek_daemon_unresponsive: "Soulseek saved, but the daemon is not responding",
    soulseek_not_connected: "Soulseek saved, but could not connect",
    stream_not_configured: "Service is not configured",
    stream_not_logged_in: "Sign in required",
    stream_need_premium: "Premium required",
    stream_unavailable: "Unavailable",
    stream_unsupported: "Not supported by this service",
    stream_age_restricted: "Age-restricted — sign in required",
    stream_region_blocked: "Unavailable in your region",
    stream_drm: "DRM-protected — will play from YouTube Music",
    stream_network: "No connection — check your internet",
    stream_parse: "Unexpected response from service",
    save_setting_failed: "Could not save setting",
    link_playlist_sign_in: "Sign in to Spotify or YouTube Music to link playlist",
    stream_track_stale: "Track is out of date — search for it again",
    song_stale: "Song is out of date — search for it again",
    nothing_to_share: "Nothing to share",
    youtube_save_tracks_only: "Only single tracks can be saved from YouTube, not albums",
    youtube_disabled: "YouTube Music is disabled",
    not_found_on_youtube: "Track not found on YouTube Music",
    mix_gone_home: "That mix is no longer available — Home refreshed",
    mix_gone_world: "That mix is no longer available — World page refreshed",
    tile_gone_home: "That collection is no longer available — Home refreshed",
    downloaded_only_no_new: "Downloaded Only is enabled — no new releases right now",
    discover_still_loading: "Still looking for new discoveries — check back soon",
    nothing_to_download: "Nothing to download",
    recs_still_building: "Recommendations are still building — try again shortly",
    blend_impossible: "Could not build a mix from these profiles",
    import_unavailable: "Import unavailable",
    deleting_unavailable: "Deleting unavailable",
    could_not_delete: "Could not delete",
    restored: "Restored",
    restore_partly: "Not everything was restored",
    purge_partly: "Deleted, but some files could not be erased",
    import_failed_file: "Could not import the file",
    import_started: "Import started · see Downloads",
    import_cancelling: "Cancelling the import — added files go back",
    torrent: "Torrent",
    import: "Import",
    masters_folder_in_use: "This FLAC folder is already in use",
    torrents_unavailable: "Torrents are unavailable",
    torrent_client_failed: "The torrent client did not start",
    import_no_pause: "An import cannot be paused — only cancelled",
    search_stale: "The search result is out of date — search again",
    prowlarr_no_torrent: "Prowlarr did not give the torrent",
    adding_torrent: "Adding the torrent…",
    torrent_gone: "the torrent left the session",
    already_in_wishlist: "Already in Wishlist",
    everything_downloaded: "Everything is already downloaded",
    downloads_with_source: "downloads once a download source is on",
    finding_on_youtube: "Finding the track on YouTube Music…",
    could_not_read_torrent: "Could not read the .torrent",
    not_a_magnet: "That is not a magnet link",
    could_not_create_folder: "Could not create the download folder",
    files_gone: "The downloaded files are gone — searching again",
    need_download_source: "A download source is needed",
    search_later: "Not it — search later",
    nothing_found_to_download: "Couldn't find where to download it",
};

locale_table! {
    /// Returns error/toast strings for `loc`.
    errors_in -> ErrorStrings,
    /// Returns error/toast strings for the active process locale.
    errors,
    { Ru => RU, En => EN }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_hash_errors_translate_both_locales() {
        assert_eq!(ErrorStrings::api_port_busy_in(Locale::Ru, 47210), "Порт 47210 занят");
        assert_eq!(ErrorStrings::api_port_busy_in(Locale::En, 47210), "Port 47210 is busy");
        assert_eq!(ErrorStrings::api_port_busy_other_in(Locale::Ru, 8989), "Порт 8989 занят другой программой");
        assert_eq!(ErrorStrings::api_port_busy_other_in(Locale::En, 8989), "Port 8989 is used by another program");
        assert_eq!(ErrorStrings::api_no_address_in(Locale::Ru, "192.168.0.157"), "У компьютера нет адреса 192.168.0.157");
        assert_eq!(ErrorStrings::api_no_address_in(Locale::En, "192.168.0.157"), "The computer has no address 192.168.0.157");
        assert_eq!(ErrorStrings::api_port_denied_in(Locale::Ru, 80), "Нет доступа к порту 80");
        assert_eq!(ErrorStrings::api_port_denied_in(Locale::En, 80), "No access to port 80");
        assert_eq!(ErrorStrings::api_port_failed_in(Locale::Ru, 47210, "x"), "Не удалось открыть порт 47210: x");
        assert_eq!(ErrorStrings::api_port_failed_in(Locale::En, 47210, "x"), "Could not open port 47210: x");
        assert_eq!(ErrorStrings::hash_check_failed_in(Locale::Ru, "/a.flac"), "проверка хеша не прошла для /a.flac");
        assert_eq!(ErrorStrings::hash_check_failed_in(Locale::En, "/a.flac"), "hash verification failed for /a.flac");
        assert_eq!(
            ErrorStrings::hash_copy_mismatch_in(Locale::Ru, "/t.flac", "/s.flac"),
            "проверка хеша не прошла для /t.flac: копия /s.flac не совпала с оригиналом"
        );
        assert_eq!(
            ErrorStrings::hash_copy_mismatch_in(Locale::En, "/t.flac", "/s.flac"),
            "hash verification failed for /t.flac: the copy /s.flac did not match the original"
        );
    }
}
