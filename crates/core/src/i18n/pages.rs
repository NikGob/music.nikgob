//! Smaller pages and shared page widgets (cards, shelves, marks, sound, equalizer, home).

use super::{Locale, PluralForms, locale, plural_forms};

/// Static strings: smaller pages and shared page widgets (cards, shelves, marks, sound, equalizer, home).
pub struct PagesStrings {
    pub stream: &'static str,
    pub you_have: &'static str,
    pub in_queue: &'static str,
    pub no_file: &'static str,
    pub clip: &'static str,
    pub pause: &'static str,
    pub character: &'static str,
    pub and: &'static str,
    pub and_others: &'static str,
    pub and_new: &'static str,
    pub dl_missing_tip: &'static str,
    pub dl_tip: &'static str,
    pub mine_add: &'static str,
    pub mine_remove: &'static str,
    pub versions_head: &'static str,
    pub wish_wait_torrents: &'static str,
    pub wish_wait: &'static str,
    pub wish_searching: &'static str,
    pub wish_confirm: &'static str,
    pub wish_downloading: &'static str,
    pub wish_not_found: &'static str,
    pub wish_done: &'static str,
    pub sync_state_two_way: &'static str,
    pub sync_state_follow: &'static str,
    pub sync_state_off: &'static str,
    pub syncing: &'static str,
    pub sync_link_tip: &'static str,
    pub ed_title: &'static str,
    pub ed_and: &'static str,
    pub ed_merge: &'static str,
    pub ed_keep: &'static str,
    pub ed_keep_tip: &'static str,
    pub track_no_number: &'static str,
    pub marks_title: &'static str,
    pub marks_disliked: &'static str,
    pub marks_banned: &'static str,
    pub marks_speed: &'static str,
    pub take_back: &'static str,
    pub no_year: &'static str,
    pub today: &'static str,
    pub week: &'static str,
    pub month: &'static str,
    pub year: &'static str,
    pub earlier: &'static str,
    pub close_esc: &'static str,
    pub equalizer: &'static str,
    pub speed_tab: &'static str,
    pub keep_timbre: &'static str,
    pub db_unit: &'static str,
    pub hz_unit: &'static str,
    pub empty_line: &'static str,
    pub find_music: &'static str,
    pub add_folder: &'static str,
    pub forward: &'static str,
    pub back_to_player: &'static str,
    pub collapse_menu: &'static str,
    pub expand_menu: &'static str,
    pub scene_track: &'static str,
    pub scene_lyrics: &'static str,
    pub scene_album: &'static str,
    pub fab_files: &'static str,
    pub fab_folder: &'static str,
    pub fab_torrent: &'static str,
    pub fab_magnet: &'static str,
    pub fab_new_playlist: &'static str,
    pub files_dialog_title: &'static str,
    pub files_filter: &'static str,
    pub open_torrent_title: &'static str,
    pub torrent_filter: &'static str,
    pub placeholder_title: &'static str,
    pub more_options: &'static str,
    pub liked_short: &'static str,
    pub new_playlist_dots: &'static str,
    pub ask_title_new_playlist: &'static str,
    pub ask_name_label: &'static str,
    pub create_btn: &'static str,
    pub ask_title_rename: &'static str,
    pub magnet_title: &'static str,
    pub magnet_placeholder: &'static str,
    pub magnet_as_track: &'static str,
    pub magnet_must_start: &'static str,
    pub enter_name: &'static str,
    pub delete_playlist_q: &'static str,
    pub playlist_delete_kept: &'static str,
    pub playlist_delete_plain: &'static str,
    pub delete_btn: &'static str,
    pub link_service_q: &'static str,
    pub link_service_body: &'static str,
    pub link_btn: &'static str,
    pub apart_q: &'static str,
    pub apart_btn: &'static str,
    pub trash_q: &'static str,
    pub trash_note_30: &'static str,
    pub trash_note_off: &'static str,
    pub purge_q: &'static str,
    pub purge_note: &'static str,
    pub purge_forever_field: &'static str,
    pub empty_trash_q: &'static str,
    pub empty_trash_forever: &'static str,
    pub clear_trash_btn: &'static str,
    pub clean_seeding_q: &'static str,
    pub clean_seeding_body: &'static str,
}

pub static RU: PagesStrings = PagesStrings {
    stream: "стрим",
    you_have: "у тебя",
    in_queue: "в очереди",
    no_file: "нет файла",
    clip: "клип",
    pause: "Пауза",
    character: "Персонаж",
    and: " и ",
    and_others: "другие",
    and_new: "новое: ",
    dl_missing_tip: "Скачать недостающие — в «Хочу скачать»",
    dl_tip: "Скачать — в «Хочу скачать»",
    mine_add: "Добавить в мои треки",
    mine_remove: "Убрать из моих треков",
    versions_head: "Другие версии",
    wish_wait_torrents: "Ждёт торренты — «Хочу скачать»",
    wish_wait: "Ждёт в «Хочу скачать»",
    wish_searching: "Ищу, где скачать",
    wish_confirm: "Нужно выбрать, что скачать — «Хочу скачать»",
    wish_downloading: "Качается",
    wish_not_found: "Не нашёл, где скачать — «Хочу скачать»",
    wish_done: "Скачано",
    sync_state_two_way: "изменения в обе стороны",
    sync_state_follow: "только получение изменений",
    sync_state_off: "не синхронизируется",
    syncing: "синхронизируется…",
    sync_link_tip: "Связан с плейлистом сервиса; как синхронизировать — в ⋮",
    ed_title: "Похоже, это один альбом",
    ed_and: " и ",
    ed_merge: "Объединить",
    ed_keep: "Оставить раздельно",
    ed_keep_tip: "Больше не спрашивать про эти альбомы",
    track_no_number: "Трек без номера",
    marks_title: "Не нравится и скорость",
    marks_disliked: "Треки с «Не нравится»",
    marks_banned: "Не рекомендовать исполнителей",
    marks_speed: "Своя скорость",
    take_back: "Снять",
    no_year: "Без года",
    today: "Сегодня",
    week: "За неделю",
    month: "За месяц",
    year: "За год",
    earlier: "Раньше",
    close_esc: "Закрыть (Esc)",
    equalizer: "Эквалайзер",
    speed_tab: "Скорость",
    keep_timbre: "Сохранять тембр голоса",
    db_unit: "дБ",
    hz_unit: "Гц",
    empty_line: "Музыки пока нет",
    find_music: "Найти музыку",
    add_folder: "Добавить папку",
    forward: "Дальше",
    back_to_player: "Вернуться к плееру",
    collapse_menu: "Свернуть меню",
    expand_menu: "Развернуть меню",
    scene_track: "Трек",
    scene_lyrics: "Текст",
    scene_album: "Альбом",
    fab_files: "Файлы…",
    fab_folder: "Папку…",
    fab_torrent: ".torrent…",
    fab_magnet: "Magnet-ссылку…",
    fab_new_playlist: "Новый плейлист…",
    files_dialog_title: "Добавить файлы",
    files_filter: "Музыка",
    open_torrent_title: "Открыть .torrent",
    torrent_filter: "Торрент",
    placeholder_title: "Название песни",
    more_options: "Ещё варианты",
    liked_short: "Любимые",
    new_playlist_dots: "Новый плейлист…",
    ask_title_new_playlist: "Новый плейлист",
    ask_name_label: "Название",
    create_btn: "Создать",
    ask_title_rename: "Переименовать",
    magnet_title: "Magnet-ссылка",
    magnet_placeholder: "magnet:?xt=…",
    magnet_as_track: "Раздача скачается как этот трек из «Хочу скачать».",
    magnet_must_start: "Ссылка должна начинаться с magnet:",
    enter_name: "Введи название",
    delete_playlist_q: "Удалить плейлист?",
    playlist_delete_plain: "Треки останутся в библиотеке.",
    playlist_delete_kept: "Треки останутся в библиотеке, а плейлист в {service} — как был.",
    delete_btn: "Удалить",
    link_service_q: "Связать с сервисом?",
    link_service_body: "Плейлист появится в Spotify или YouTube Music, и изменения пойдут в обе стороны.",
    link_btn: "Связать",
    apart_q: "Это разные песни?",
    apart_btn: "Разные песни",
    trash_q: "Удалить из библиотеки?",
    purge_q: "Удалить навсегда?",
    trash_note_30: "{title} переедет в Корзину на 30 дней: из библиотеки пропадёт сразу, вернуть можно из Библиотеки → Корзина.",
    trash_note_off: "{title}: файлы будут стёрты с диска. Корзина выключена в настройках, отменить нельзя.",
    purge_forever_field: "Удалить навсегда",
    purge_note: "{title}: файлы будут стёрты с диска, треки уйдут из плейлистов и Любимых. Отменить нельзя.",
    empty_trash_q: "Очистить корзину?",
    clear_trash_btn: "Очистить",
    empty_trash_forever: "{count} и их файлы будут стёрты навсегда. Отменить нельзя.",
    clean_seeding_q: "Очистить раздачу?",
    clean_seeding_body: "Завершённые торренты перестанут раздаваться, их файлы будут удалены. Музыка в библиотеке останется.",
};

pub static EN: PagesStrings = PagesStrings {
    stream: "stream",
    you_have: "yours",
    in_queue: "queued",
    no_file: "no file",
    clip: "video",
    pause: "Pause",
    character: "Character",
    and: " and ",
    and_others: "others",
    and_new: "new: ",
    dl_missing_tip: "Download missing — in “Wishlist”",
    dl_tip: "Download — in “Wishlist”",
    mine_add: "Add to my tracks",
    mine_remove: "Remove from my tracks",
    versions_head: "Other versions",
    wish_wait_torrents: "Waiting for torrents — “Wishlist”",
    wish_wait: "Waiting in “Wishlist”",
    wish_searching: "Looking for a download",
    wish_confirm: "Pick what to download — “Wishlist”",
    wish_downloading: "Downloading",
    wish_not_found: "No download found — “Wishlist”",
    wish_done: "Downloaded",
    sync_state_two_way: "two-way changes",
    sync_state_follow: "receiving changes only",
    sync_state_off: "not syncing",
    syncing: "syncing…",
    sync_link_tip: "Linked to a service playlist; how to sync — in ⋮",
    ed_title: "Looks like one album",
    ed_and: " and ",
    ed_merge: "Merge",
    ed_keep: "Keep separate",
    ed_keep_tip: "Don't ask about these albums again",
    track_no_number: "Track with no number",
    marks_title: "Dislikes & speed",
    marks_disliked: "Tracks marked “Dislike”",
    marks_banned: "Artists not recommended",
    marks_speed: "Custom speed",
    take_back: "Remove",
    no_year: "No year",
    today: "Today",
    week: "This week",
    month: "This month",
    year: "This year",
    earlier: "Earlier",
    close_esc: "Close (Esc)",
    equalizer: "Equalizer",
    speed_tab: "Speed",
    keep_timbre: "Keep voice timbre",
    db_unit: "dB",
    hz_unit: "Hz",
    empty_line: "No music yet",
    find_music: "Find music",
    add_folder: "Add folder",
    forward: "Forward",
    back_to_player: "Back to player",
    collapse_menu: "Collapse menu",
    expand_menu: "Expand menu",
    scene_track: "Track",
    scene_lyrics: "Lyrics",
    scene_album: "Album",
    fab_files: "Files…",
    fab_folder: "Folder…",
    fab_torrent: ".torrent…",
    fab_magnet: "Magnet link…",
    fab_new_playlist: "New playlist…",
    files_dialog_title: "Add files",
    files_filter: "Music",
    open_torrent_title: "Open .torrent",
    torrent_filter: "Torrent",
    placeholder_title: "Song title",
    more_options: "More options",
    liked_short: "Liked Songs",
    new_playlist_dots: "New playlist…",
    ask_title_new_playlist: "New playlist",
    ask_name_label: "Name",
    create_btn: "Create",
    ask_title_rename: "Rename",
    magnet_title: "Magnet link",
    magnet_placeholder: "magnet:?xt=…",
    magnet_as_track: "The torrent downloads as this track from “Wishlist”.",
    magnet_must_start: "The link must start with magnet:",
    enter_name: "Enter a name",
    delete_playlist_q: "Delete playlist?",
    playlist_delete_plain: "The tracks will stay in the library.",
    playlist_delete_kept: "The tracks will stay in the library, and the playlist on {service} stays as it is.",
    delete_btn: "Delete",
    link_service_q: "Link to a service?",
    link_service_body: "The playlist will appear on Spotify or YouTube Music, and changes will go both ways.",
    link_btn: "Link",
    apart_q: "Are these different songs?",
    apart_btn: "Different songs",
    trash_q: "Delete from library?",
    purge_q: "Delete forever?",
    trash_note_30: "{title} moves to the Trash for 30 days: it leaves the library at once and can be restored from Library → Trash.",
    trash_note_off: "{title}: the files will be erased from disk. The Trash is off in the settings, this cannot be undone.",
    purge_forever_field: "Delete forever",
    purge_note: "{title}: the files will be erased from disk, the tracks leave playlists and Liked Songs. This cannot be undone.",
    empty_trash_q: "Empty trash?",
    clear_trash_btn: "Empty",
    empty_trash_forever: "{count} and their files will be erased forever. This cannot be undone.",
    clean_seeding_q: "Stop seeding?",
    clean_seeding_body: "Finished torrents will stop seeding and their files will be deleted. The music stays in the library."
};

locale_table! {
    /// Returns these strings for `loc`.
    pages_in -> PagesStrings,
    /// Returns these strings for the active process locale.
    pages,
    { Ru => RU, En => EN }
}

/// Fills the `{title}` placeholder of the confirm-dialog note fields.
pub fn fill(field: &str, title: &str) -> String {
    field.replace("{title}", title)
}

/// Fills `{service}` of the linked-playlist delete note.
pub fn fill_service(field: &str, service: &str) -> String {
    field.replace("{service}", service)
}

/// Fills `{count}` of the empty-trash note.
pub fn fill_count(field: &str, count: &str) -> String {
    field.replace("{count}", count)
}

/// Formats `"{n} {unit}"` (e.g. `"5 треков"` / `"5 tracks"`).
pub fn fmt_count_in(loc: Locale, n: u64, forms: &PluralForms) -> String {
    format!("{n} {}", plural_forms(loc, n, forms))
}

/// Formats `"{n} {unit}"` in the active locale.
pub fn fmt_count(n: u64, forms: &PluralForms) -> String {
    fmt_count_in(locale(), n, forms)
}

/// Formats `"{a} из {b}"` (`"{a} of {b}"`), a release's local share.
pub fn fmt_of_in(loc: Locale, a: u32, b: u32) -> String {
    match loc {
        Locale::Ru => format!("{a} из {b}"),
        Locale::En => format!("{a} of {b}"),
    }
}

/// Formats `"{a} из {b}"` in the active locale.
pub fn fmt_of(a: u32, b: u32) -> String {
    fmt_of_in(locale(), a, b)
}

/// Formats «Трек {no}» (`Track {no}`) of a place no track has.
pub fn fmt_track_no_in(loc: Locale, no: u32) -> String {
    match loc {
        Locale::Ru => format!("Трек {no}"),
        Locale::En => format!("Track {no}"),
    }
}

/// Formats «Трек {no}» in the active locale.
pub fn fmt_track_no(no: u32) -> String {
    fmt_track_no_in(locale(), no)
}

/// Formats «Диск {disc}» (`Disc {disc}`).
pub fn fmt_disc_in(loc: Locale, disc: u32) -> String {
    match loc {
        Locale::Ru => format!("Диск {disc}"),
        Locale::En => format!("Disc {disc}"),
    }
}

/// Formats «Диск {disc}» in the active locale.
pub fn fmt_disc(disc: u32) -> String {
    fmt_disc_in(locale(), disc)
}

/// Formats a decade group header (`"1990-е"` / `"1990s"`).
pub fn fmt_decade_in(loc: Locale, year: i32) -> String {
    let d = year - year.rem_euclid(10);
    match loc {
        Locale::Ru => format!("{d}-е"),
        Locale::En => format!("{d}s"),
    }
}

/// Formats a decade group header in the active locale.
pub fn fmt_decade(year: i32) -> String {
    fmt_decade_in(locale(), year)
}

/// Formats «Слушать «{title}»» (`Play “{title}”`).
pub fn fmt_play_quoted_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("Слушать «{title}»"),
        Locale::En => format!("Play “{title}”"),
    }
}

/// Formats «Слушать «{title}»» in the active locale.
pub fn fmt_play_quoted(title: &str) -> String {
    fmt_play_quoted_in(locale(), title)
}

/// Formats «Версия {v}» (`Version {v}`).
pub fn fmt_version_in(loc: Locale, v: &str) -> String {
    match loc {
        Locale::Ru => format!("Версия {v}"),
        Locale::En => format!("Version {v}"),
    }
}

/// Formats «Версия {v}» in the active locale.
pub fn fmt_version(v: &str) -> String {
    fmt_version_in(locale(), v)
}

/// Formats «синхронизирован {t}» (`synced {t}`).
pub fn fmt_synced_in(loc: Locale, t: &str) -> String {
    match loc {
        Locale::Ru => format!("синхронизирован {t}"),
        Locale::En => format!("synced {t}"),
    }
}

/// Formats «синхронизирован {t}» in the active locale.
pub fn fmt_synced(t: &str) -> String {
    fmt_synced_in(locale(), t)
}

/// Formats the edition offer's first line (`«A», «B» — издания одного альбома (X).`).
pub fn fmt_ed_what_in(loc: Locale, names: &str, artist: &str) -> String {
    match loc {
        Locale::Ru => format!("{names} — издания одного альбома ({artist})."),
        Locale::En => format!("{names} — editions of one album ({artist})."),
    }
}

/// Formats the edition offer's first line in the active locale.
pub fn fmt_ed_what(names: &str, artist: &str) -> String {
    fmt_ed_what_in(locale(), names, artist)
}

/// Formats the edition offer's identical-songs line.
pub fn fmt_ed_same_in(loc: Locale, n: u32) -> String {
    match loc {
        Locale::Ru => format!(" Одинаковых песен: {n} — останется по одной, файл получше будет играть."),
        Locale::En => {
            let noun = if n == 1 { "song" } else { "songs" };
            format!(" {n} identical {noun} — one copy stays, the better file plays.")
        }
    }
}

/// Formats the edition offer's identical-songs line in the active locale.
pub fn fmt_ed_same(n: u32) -> String {
    fmt_ed_same_in(locale(), n)
}

/// Formats the edition offer's new-songs line (` Ещё {n} добавится в «{keep}».`).
pub fn fmt_ed_new_in(loc: Locale, n: u32, keep: &str) -> String {
    match loc {
        Locale::Ru => format!(" Ещё {n} добавится в «{keep}»."),
        Locale::En => format!(" {n} more will be added to “{keep}”."),
    }
}

/// Formats the edition offer's new-songs line in the active locale.
pub fn fmt_ed_new(n: u32, keep: &str) -> String {
    fmt_ed_new_in(locale(), n, keep)
}

/// Formats the edition offer's merge tooltip (`Всё окажется в «{keep}»; …`).
pub fn fmt_ed_merge_tip_in(loc: Locale, keep: &str) -> String {
    match loc {
        Locale::Ru => format!("Всё окажется в «{keep}»; файлы на диске не трогаются"),
        Locale::En => format!("Everything lands in “{keep}”; files on disk stay untouched"),
    }
}

/// Formats the edition offer's merge tooltip in the active locale.
pub fn fmt_ed_merge_tip(keep: &str) -> String {
    fmt_ed_merge_tip_in(locale(), keep)
}

/// Human size: `"412 МБ"`, `"1.1 ГБ"` (`"412 MB"`, `"1.1 GB"`).
pub fn fmt_size_in(loc: Locale, bytes: u64) -> String {
    let units: [&str; 5] = match loc {
        Locale::Ru => ["Б", "КБ", "МБ", "ГБ", "ТБ"],
        Locale::En => ["B", "KB", "MB", "GB", "TB"],
    };
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{bytes} {}", units[0]),
        _ if v >= 100.0 => format!("{v:.0} {}", units[i]),
        _ => format!("{v:.1} {}", units[i]),
    }
}

/// Human size in the active locale.
pub fn fmt_size(bytes: u64) -> String {
    fmt_size_in(locale(), bytes)
}

/// The equalizer bands' names under the rails (`"1к"` / `"1k"`).
pub const fn band_names_in(loc: Locale) -> [&'static str; 10] {
    match loc {
        Locale::Ru => ["32", "64", "125", "250", "500", "1к", "2к", "4к", "8к", "16к"],
        Locale::En => ["32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"],
    }
}

/// The equalizer bands' names in the active locale.
pub fn band_names() -> [&'static str; 10] {
    band_names_in(locale())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::PluralForms;

    #[test]
    fn fmts_match_both_locales() {
        let ru = Locale::Ru;
        let en = Locale::En;
        let tracks = PluralForms { one: "трек", few: "трека", many: "треков" };
        let tracks_en = PluralForms { one: "track", few: "tracks", many: "tracks" };
        assert_eq!(fmt_count_in(ru, 1, &tracks), "1 трек");
        assert_eq!(fmt_count_in(ru, 5, &tracks), "5 треков");
        assert_eq!(fmt_count_in(en, 1, &tracks_en), "1 track");
        assert_eq!(fmt_count_in(en, 5, &tracks_en), "5 tracks");
        assert_eq!(fmt_of_in(ru, 3, 12), "3 из 12");
        assert_eq!(fmt_of_in(en, 3, 12), "3 of 12");
        assert_eq!(fmt_track_no_in(ru, 7), "Трек 7");
        assert_eq!(fmt_track_no_in(en, 7), "Track 7");
        assert_eq!(fmt_disc_in(ru, 2), "Диск 2");
        assert_eq!(fmt_disc_in(en, 2), "Disc 2");
        assert_eq!(fmt_decade_in(ru, 1994), "1990-е");
        assert_eq!(fmt_decade_in(en, 1994), "1990s");
        assert_eq!(fmt_play_quoted_in(ru, "X"), "Слушать «X»");
        assert_eq!(fmt_play_quoted_in(en, "X"), "Play “X”");
        assert_eq!(fmt_version_in(ru, "0.1.0"), "Версия 0.1.0");
        assert_eq!(fmt_version_in(en, "0.1.0"), "Version 0.1.0");
        assert_eq!(fmt_synced_in(ru, "12:00"), "синхронизирован 12:00");
        assert_eq!(fmt_synced_in(en, "12:00"), "synced 12:00");
        assert_eq!(fmt_ed_what_in(ru, "«A» и «B»", "X"), "«A» и «B» — издания одного альбома (X).");
        assert_eq!(fmt_ed_what_in(en, "“A” and “B”", "X"), "“A” and “B” — editions of one album (X).");
        assert_eq!(fmt_ed_same_in(ru, 1), " Одинаковых песен: 1 — останется по одной, файл получше будет играть.");
        assert_eq!(fmt_ed_same_in(en, 1), " 1 identical song — one copy stays, the better file plays.");
        assert_eq!(fmt_ed_same_in(en, 2), " 2 identical songs — one copy stays, the better file plays.");
        assert_eq!(fmt_ed_new_in(ru, 3, "A"), " Ещё 3 добавится в «A».");
        assert_eq!(fmt_ed_new_in(en, 3, "A"), " 3 more will be added to “A”.");
        assert_eq!(fmt_size_in(ru, 500), "500 Б");
        assert_eq!(fmt_size_in(ru, 412 * 1024 * 1024), "412 МБ");
        assert_eq!(fmt_size_in(en, 412 * 1024 * 1024), "412 MB");
        assert_eq!(fmt_size_in(en, 500), "500 B");
        assert_eq!(band_names_in(ru)[5], "1к");
        assert_eq!(band_names_in(en)[5], "1k");
        assert_eq!(pages_in(ru).empty_line, "Музыки пока нет");
        assert_eq!(pages_in(en).empty_line, "No music yet");
    }
}
