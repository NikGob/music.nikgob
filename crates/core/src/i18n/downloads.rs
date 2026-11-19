//! Downloads and wishlist pages.

use super::{Locale, PluralForms, plural_forms};

/// Static strings: downloads and wishlist pages.
pub struct DownloadsStrings {
    // The page and its tabs.
    pub title: &'static str,
    pub tab_active: &'static str,
    pub tab_confirm: &'static str,
    pub tab_wishlist: &'static str,
    pub tab_seeding: &'static str,
    pub tab_history: &'static str,
    pub nothing_downloading: &'static str,
    pub history_empty: &'static str,
    pub all_confirmed: &'static str,
    pub accept_recommended: &'static str,
    pub recommended: &'static str,
    pub select_all: &'static str,
    pub cancel_job: &'static str,
    pub pause_job: &'static str,

    // «Раздача».
    pub torrents: &'static str,
    pub seeding_off: &'static str,
    pub enable: &'static str,
    pub given_by_torrents: &'static str,
    pub no_cap: &'static str,
    pub clean_now: &'static str,
    pub seed_until_ratio: &'static str,
    pub seed_for: &'static str,
    pub cap_all: &'static str,
    pub upload_speed: &'static str,
    pub upload: &'static str,
    /// The «Загрузка» tile: the speed bytes come at.
    pub incoming: &'static str,
    pub given_today: &'static str,
    pub given_total: &'static str,
    pub seeding_disk: &'static str,
    pub uploading_now: &'static str,
    pub uploaded_recently: &'static str,
    pub share_library: &'static str,
    pub uploads_at_once: &'static str,
    pub not_connected: &'static str,
    pub set_up: &'static str,
    pub other_computer: &'static str,
    pub upload_queued: &'static str,
    pub ratio: &'static str,
    pub peers: PluralForms,

    // «Хочу скачать».
    pub wishlist: &'static str,
    pub chip_all: &'static str,
    pub chip_waiting: &'static str,
    pub chip_searching: &'static str,
    pub chip_downloading: &'static str,
    pub chip_confirm: &'static str,
    pub chip_not_found: &'static str,
    pub chip_done: &'static str,
    pub chip_dismissed: &'static str,
    pub nothing_yet: &'static str,
    pub all_sources_off: &'static str,
    pub where_from: &'static str,
    pub by_album: &'static str,
    pub by_artist: &'static str,
    pub download_all: &'static str,
    pub search_again: &'static str,
    pub loose_tracks: &'static str,
    pub dismiss_group: &'static str,
    pub whole_album: &'static str,
    pub album: &'static str,
    pub not_found: &'static str,
    pub download: &'static str,
    pub check: &'static str,
    pub restore: &'static str,
    pub save_from_youtube: &'static str,
    pub own_torrent: &'static str,
    pub magnet_link: &'static str,
    pub own_file: &'static str,
    pub album_folder: &'static str,
    pub dont_download: &'static str,
    pub other_ways: &'static str,
    pub download_album: &'static str,
    pub torrent_dialog_title: &'static str,
    pub torrent_filter: &'static str,
    pub track_file_dialog_title: &'static str,
    pub music_filter: &'static str,
    pub album_folder_dialog_title: &'static str,
    pub save_from_youtube_hint: &'static str,
    pub play_context: &'static str,
}

pub static RU: DownloadsStrings = DownloadsStrings {
    title: "Загрузки",
    tab_active: "Активные",
    tab_confirm: "Подтверди",
    tab_wishlist: "Хочу скачать",
    tab_seeding: "Раздача",
    tab_history: "История",
    nothing_downloading: "Ничего не качается",
    history_empty: "История пуста",
    all_confirmed: "Всё подтверждено",
    accept_recommended: "Принять все рекомендованные",
    recommended: "РЕКОМЕНДУЕМ",
    select_all: "Выбрать все",
    cancel_job: "Отменить",
    pause_job: "Пауза",

    torrents: "Торренты",
    seeding_off: "Раздача выключена",
    enable: "включить",
    given_by_torrents: "Отдано раздачами",
    no_cap: "без лимита",
    clean_now: "Очистить сейчас",
    seed_until_ratio: "Раздавать до ratio",
    seed_for: "Срок раздачи",
    cap_all: "Лимит на все раздачи",
    upload_speed: "Скорость отдачи",
    upload: "Отдача",
    incoming: "Загрузка",
    given_today: "Отдано сегодня",
    given_total: "Отдано всего",
    seeding_disk: "Раздачи занимают на диске",
    uploading_now: "Отдаётся сейчас",
    uploaded_recently: "Отдано недавно",
    share_library: "Раздавать библиотеку",
    uploads_at_once: "Отдач одновременно",
    not_connected: "Soulseek не подключён",
    set_up: "настроить",
    other_computer: "nikgob-slsk на другом компьютере",
    upload_queued: "в очереди",
    ratio: "ratio",
    peers: PluralForms { one: "пир", few: "пира", many: "пиров" },

    wishlist: "Хочу скачать",
    chip_all: "Все",
    chip_waiting: "Ждут",
    chip_searching: "Ищу",
    chip_downloading: "Качаются",
    chip_confirm: "Проверить",
    chip_not_found: "Не нашлось",
    chip_done: "Готово",
    chip_dismissed: "Не качать",
    nothing_yet: "Пока ничего",
    all_sources_off: "Все источники выключены",
    where_from: "Откуда качать",
    by_album: "По альбомам",
    by_artist: "По исполнителям",
    download_all: "Скачать все",
    search_again: "Искать снова",
    loose_tracks: "Отдельные треки",
    dismiss_group: "Не качать ничего отсюда",
    whole_album: "весь альбом",
    album: "альбом",
    not_found: "не нашёл",
    download: "Скачать",
    check: "Проверить",
    restore: "Вернуть",
    save_from_youtube: "Сохранить с YouTube",
    own_torrent: "Свой .torrent…",
    magnet_link: "Magnet-ссылка…",
    own_file: "Свой файл…",
    album_folder: "Папка с альбомом…",
    dont_download: "Не качать",
    other_ways: "Другие способы",
    download_album: "Скачать альбом",
    torrent_dialog_title: "Свой .torrent для этого трека",
    torrent_filter: "Торрент",
    track_file_dialog_title: "Файл этого трека",
    music_filter: "Музыка",
    album_folder_dialog_title: "Папка с альбомом",
    save_from_youtube_hint: "Аудио как есть (Opus ~160 кбит/с, качество пониже); найденное потом заменит его",
    play_context: "«Хочу скачать»",
};

pub static EN: DownloadsStrings = DownloadsStrings {
    title: "Downloads",
    tab_active: "Active",
    tab_confirm: "Confirm",
    tab_wishlist: "Wishlist",
    tab_seeding: "Seeding",
    tab_history: "History",
    nothing_downloading: "Nothing is downloading",
    history_empty: "History is empty",
    all_confirmed: "Everything is confirmed",
    accept_recommended: "Accept all recommended",
    recommended: "Recommended",
    select_all: "Select all",
    cancel_job: "Cancel",
    pause_job: "Pause",

    torrents: "Torrents",
    seeding_off: "Seeding is off",
    enable: "Turn on",
    given_by_torrents: "Uploaded via torrents",
    no_cap: "no limit",
    clean_now: "Clean up now",
    seed_until_ratio: "Seed to ratio",
    seed_for: "Seeding duration",
    cap_all: "Total seeding limit",
    upload_speed: "Upload speed",
    upload: "Upload",
    incoming: "Download",
    given_today: "Uploaded today",
    given_total: "Total uploaded",
    seeding_disk: "Seeding uses",
    uploading_now: "Uploading now",
    uploaded_recently: "Uploaded recently",
    share_library: "Share the library",
    uploads_at_once: "Uploads at once",
    not_connected: "Soulseek is not connected",
    set_up: "Set up",
    other_computer: "nikgob-slsk on another computer",
    upload_queued: "queued",
    ratio: "ratio",
    peers: PluralForms { one: "peer", few: "peers", many: "peers" },

    wishlist: "Wishlist",
    chip_all: "All",
    chip_waiting: "Waiting",
    chip_searching: "Searching",
    chip_downloading: "Downloading",
    chip_confirm: "Check",
    chip_not_found: "Not found",
    chip_done: "Done",
    chip_dismissed: "Don't download",
    nothing_yet: "Nothing yet",
    all_sources_off: "All sources are off",
    where_from: "Where to download from",
    by_album: "By album",
    by_artist: "By artist",
    download_all: "Download all",
    search_again: "Search again",
    loose_tracks: "Individual tracks",
    dismiss_group: "Don't download this group",
    whole_album: "whole album",
    album: "album",
    not_found: "not found",
    download: "Download",
    check: "Check",
    restore: "Restore",
    save_from_youtube: "Save from YouTube",
    own_torrent: "Choose .torrent…",
    magnet_link: "Magnet link…",
    own_file: "Choose file…",
    album_folder: "Album folder…",
    dont_download: "Don't download",
    other_ways: "Other ways",
    download_album: "Download album",
    torrent_dialog_title: "Choose a .torrent for this track",
    torrent_filter: "Torrent",
    track_file_dialog_title: "This track's file",
    music_filter: "Music",
    album_folder_dialog_title: "Album folder",
    save_from_youtube_hint: "Original audio (Opus ~160 kbps, lower quality); replaced when a better copy is found.",
    play_context: "Wishlist",
};

locale_table! {
    /// Returns these strings for `loc`.
    downloads_in -> DownloadsStrings,
    /// Returns these strings for the active process locale.
    downloads,
    { Ru => RU, En => EN }
}

/// `"Скачать все (3)"`.
pub fn fmt_download_all_in(loc: Locale, n: usize) -> String {
    format!("{} ({n})", downloads_in(loc).download_all)
}

/// `"Искать снова (2)"`.
pub fn fmt_search_again_in(loc: Locale, n: usize) -> String {
    format!("{} ({n})", downloads_in(loc).search_again)
}

/// `"Принять все рекомендованные (4)"`.
pub fn fmt_accept_recommended_in(loc: Locale, n: usize) -> String {
    format!("{} ({n})", downloads_in(loc).accept_recommended)
}

/// `"из лимита 20 ГБ"` / `"без лимита"`.
pub fn fmt_of_cap_in(loc: Locale, gb: u32) -> String {
    if gb == 0 {
        return downloads_in(loc).no_cap.to_owned();
    }
    match loc {
        Locale::Ru => format!("из лимита {gb} ГБ"),
        Locale::En => format!("of the {gb} GB limit"),
    }
}

/// `"20 ГБ"` / `"без лимита"`.
pub fn fmt_cap_in(loc: Locale, gb: u32) -> String {
    if gb == 0 {
        return downloads_in(loc).no_cap.to_owned();
    }
    match loc {
        Locale::Ru => format!("{gb} ГБ"),
        Locale::En => format!("{gb} GB"),
    }
}

/// `"3 дн. после скачивания"`.
pub fn fmt_days_after_download_in(loc: Locale, days: u32) -> String {
    match (loc, days) {
        (Locale::Ru, _) => format!("{days} дн. после скачивания"),
        (Locale::En, 1) => "1 day after downloading".to_owned(),
        (Locale::En, _) => format!("{days} days after downloading"),
    }
}

/// `"Раздачи занимают на диске 12 ГБ"`.
pub fn fmt_seeding_disk_in(loc: Locale, size: &str) -> String {
    format!("{} {size}", downloads_in(loc).seeding_disk)
}

/// `"торренты 1 МБ/с · Soulseek 2 МБ/с"`.
pub fn fmt_split_in(loc: Locale, torrents: &str, slsk: &str) -> String {
    match loc {
        Locale::Ru => format!("торренты {torrents} · Soulseek {slsk}"),
        Locale::En => format!("torrents {torrents} · Soulseek {slsk}"),
    }
}

/// One torrent's line in «Раздача»: `"отдаёт 100 КБ/с · 2 пира · отдано
/// 512 МБ · ratio 0.50 из 2.0 · 1.0 ГБ · чистка через 3 дн."`.
pub fn fmt_seed_line_in(
    loc: Locale,
    rate: Option<&str>,
    peers: u32,
    given: &str,
    ratio: f64,
    ratio_target: Option<&str>,
    size: &str,
    expires: Option<&str>,
) -> String {
    let s = downloads_in(loc);
    let mut parts = Vec::new();
    if let Some(rate) = rate {
        parts.push(match loc {
            Locale::Ru => format!("отдаёт {rate}"),
            Locale::En => format!("uploading {rate}"),
        });
    }
    let word = plural_forms(loc, u64::from(peers), &s.peers);
    parts.push(format!("{peers} {word}"));
    let given_word = match loc {
        Locale::Ru => "отдано",
        Locale::En => "uploaded",
    };
    let target = ratio_target.map(|t| match loc {
        Locale::Ru => format!(" из {t}"),
        Locale::En => format!(" of {t}"),
    });
    parts.push(format!("{given_word} {given} · {} {ratio:.2}{}", s.ratio, target.unwrap_or_default()));
    parts.push(size.to_owned());
    if let Some(e) = expires {
        parts.push(e.to_owned());
    }
    parts.join(" · ")
}

/// What Soulseek shares, in one line: how many files in how many folders
/// (nothing while the list is still being made), and where when another
/// computer shares them.
pub fn fmt_shared_line_in(loc: Locale, files: u32, folders: u32, remote: bool) -> Option<String> {
    let s = downloads_in(loc);
    let count = (files > 0).then(|| {
        let files_word = plural_forms(loc, u64::from(files), &super::status_in(loc).files_plural);
        let folders_word = match loc {
            Locale::Ru => {
                if folders % 10 == 1 && folders % 100 != 11 {
                    "папке"
                } else {
                    "папках"
                }
            }
            Locale::En => {
                if folders == 1 {
                    "folder"
                } else {
                    "folders"
                }
            }
        };
        match loc {
            Locale::Ru => format!("{files} {files_word} в {folders} {folders_word}"),
            Locale::En => format!("{files} {files_word} in {folders} {folders_word}"),
        }
    });
    match (remote, count) {
        (true, Some(files)) => Some(format!("{} · {files}", s.other_computer)),
        (true, None) => Some(s.other_computer.to_owned()),
        (false, files) => files,
    }
}

/// How an upload is going, for [`fmt_upload_line_in`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadState {
    Queued,
    Sending,
    Done,
    Failed,
}

/// `"peer · в очереди · 30.0 МБ"`, `"peer · 3.0 МБ из 30.0 МБ · 512 КБ/с"`.
pub fn fmt_upload_line_in(loc: Locale, user: &str, state: UploadState, sent: &str, size: &str, rate: &str) -> String {
    let s = downloads_in(loc);
    let of = super::status_in(loc).of;
    match (loc, state) {
        (_, UploadState::Queued) => format!("{user} · {} · {size}", s.upload_queued),
        (_, UploadState::Sending) => format!("{user} · {sent} {of} {size} · {rate}"),
        (_, UploadState::Done) => format!("{user} · {size} · {rate}"),
        (Locale::Ru, UploadState::Failed) => format!("{user} · прервано на {sent} {of} {size}"),
        (Locale::En, UploadState::Failed) => format!("{user} · interrupted at {sent} {of} {size}"),
    }
}

/// `"без файла 2 тр."`: what an album item still misses.
pub fn fmt_without_file_in(loc: Locale, n: u32) -> String {
    match (loc, n) {
        (Locale::Ru, _) => format!("без файла {n} тр."),
        (Locale::En, 1) => "1 track without a file".to_owned(),
        (Locale::En, _) => format!("{n} tracks without a file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_units_in_both_locales() {
        assert_eq!(fmt_download_all_in(Locale::Ru, 3), "Скачать все (3)");
        assert_eq!(fmt_download_all_in(Locale::En, 3), "Download all (3)");
        assert_eq!(fmt_search_again_in(Locale::Ru, 2), "Искать снова (2)");
        assert_eq!(fmt_accept_recommended_in(Locale::En, 4), "Accept all recommended (4)");
        assert_eq!(fmt_of_cap_in(Locale::Ru, 20), "из лимита 20 ГБ");
        assert_eq!(fmt_of_cap_in(Locale::Ru, 0), "без лимита");
        assert_eq!(fmt_cap_in(Locale::En, 0), "no limit");
        assert_eq!(fmt_days_after_download_in(Locale::Ru, 3), "3 дн. после скачивания");
        assert_eq!(fmt_days_after_download_in(Locale::En, 3), "3 days after downloading");
        assert_eq!(fmt_days_after_download_in(Locale::En, 1), "1 day after downloading");
        assert_eq!(fmt_without_file_in(Locale::Ru, 2), "без файла 2 тр.");
        assert_eq!(fmt_without_file_in(Locale::En, 2), "2 tracks without a file");
        assert_eq!(fmt_without_file_in(Locale::En, 1), "1 track without a file");
        assert_eq!(fmt_seeding_disk_in(Locale::Ru, "12 ГБ"), "Раздачи занимают на диске 12 ГБ");
    }

    #[test]
    fn a_seed_row_in_both_locales() {
        let ru = fmt_seed_line_in(Locale::Ru, Some("100 КБ/с"), 2, "512 МБ", 0.5, Some("2.0"), "1.0 ГБ", Some("чистка через 3 дн."));
        assert_eq!(ru, "отдаёт 100 КБ/с · 2 пира · отдано 512 МБ · ratio 0.50 из 2.0 · 1.0 ГБ · чистка через 3 дн.");
        let ru = fmt_seed_line_in(Locale::Ru, None, 0, "512 МБ", 0.0, None, "0 Б", None);
        assert_eq!(ru, "0 пиров · отдано 512 МБ · ratio 0.00 · 0 Б");
        let en = fmt_seed_line_in(Locale::En, Some("100 KB/s"), 5, "512 MB", 1.5, Some("2.0"), "1 GB", None);
        assert_eq!(en, "uploading 100 KB/s · 5 peers · uploaded 512 MB · ratio 1.50 of 2.0 · 1 GB");
    }

    #[test]
    fn shared_files_and_uploads_in_both_locales() {
        assert_eq!(fmt_shared_line_in(Locale::Ru, 4812, 391, false).as_deref(), Some("4812 файлов в 391 папке"));
        assert_eq!(
            fmt_shared_line_in(Locale::Ru, 2, 12, true).as_deref(),
            Some("nikgob-slsk на другом компьютере · 2 файла в 12 папках")
        );
        assert_eq!(fmt_shared_line_in(Locale::Ru, 0, 3, true).as_deref(), Some("nikgob-slsk на другом компьютере"));
        assert_eq!(fmt_shared_line_in(Locale::En, 1, 1, false).as_deref(), Some("1 file in 1 folder"));
        assert_eq!(fmt_shared_line_in(Locale::En, 2, 3, false).as_deref(), Some("2 files in 3 folders"));
        assert_eq!(fmt_shared_line_in(Locale::Ru, 0, 0, false), None);
        assert_eq!(
            fmt_upload_line_in(Locale::Ru, "peer", UploadState::Queued, "", "30.0 МБ", ""),
            "peer · в очереди · 30.0 МБ"
        );
        assert_eq!(
            fmt_upload_line_in(Locale::Ru, "peer", UploadState::Sending, "3.0 МБ", "30.0 МБ", "512 КБ/с"),
            "peer · 3.0 МБ из 30.0 МБ · 512 КБ/с"
        );
        assert_eq!(
            fmt_upload_line_in(Locale::Ru, "peer", UploadState::Failed, "3.0 МБ", "30.0 МБ", ""),
            "peer · прервано на 3.0 МБ из 30.0 МБ"
        );
        assert_eq!(
            fmt_upload_line_in(Locale::En, "peer", UploadState::Done, "", "30 MB", "512 KB/s"),
            "peer · 30 MB · 512 KB/s"
        );
    }
}
