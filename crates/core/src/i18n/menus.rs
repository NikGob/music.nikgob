//! Shared track/album/artist/playlist/mix context menu items.

use super::{Locale, locale};

/// Static strings: shared track/album/artist/playlist/mix context menu items.
pub struct MenusStrings {
    pub radio_by_track: &'static str,
    pub play_next: &'static str,
    pub to_queue: &'static str,
    pub to_library: &'static str,
    pub out_of_library: &'static str,
    pub like: &'static str,
    pub unlike: &'static str,
    pub wrong_file: &'static str,
    pub to_mp3: &'static str,
    pub find_better: &'static str,
    pub pick_release: &'static str,
    pub apart: &'static str,
    pub file: &'static str,
    pub dislike: &'static str,
    pub undislike: &'static str,
    pub ban_artist: &'static str,
    pub unban_artist: &'static str,
    pub to_playlist: &'static str,
    pub to_album: &'static str,
    pub to_artist: &'static str,
    pub share: &'static str,
    pub source_quality: &'static str,
    pub download_album: &'static str,
    pub speed: &'static str,
    pub download: &'static str,
    pub edit_tags: &'static str,
    pub delete_from_library: &'static str,
    pub purge_forever: &'static str,
    pub play_whole_album: &'static str,
    pub radio_by_album: &'static str,
    pub download_whole_album: &'static str,
    pub delete_album: &'static str,
    pub delete_album_forever: &'static str,
    pub add_album: &'static str,
    pub keep_mine: &'static str,
    pub refresh_cover: &'static str,
    pub radio_by_playlist: &'static str,
    pub edit_item: &'static str,
    pub rename_item: &'static str,
    pub sync_two_way: &'static str,
    pub sync_follow: &'static str,
    pub sync_off: &'static str,
    pub link_service: &'static str,
    pub delete_playlist: &'static str,
    pub play_mix: &'static str,
    pub radio_artist: &'static str,
    pub follow: &'static str,
    pub unfollow: &'static str,
    pub in_subscriptions: &'static str,
    pub list_tracks: &'static str,
    pub save_as_playlist: &'static str,
    pub download_missing: &'static str,
    pub redraw_mix: &'static str,
    pub blend: &'static str,
    pub hide_mix: &'static str,
    pub remove_from_playlist: &'static str,
}

pub static RU: MenusStrings = MenusStrings {
    radio_by_track: "Радио по треку",
    play_next: "Сыграть следующим",
    to_queue: "В очередь",
    to_library: "В библиотеку",
    out_of_library: "Убрать из библиотеки",
    like: "Нравится",
    unlike: "Убрать из любимого",
    wrong_file: "В файле другая песня",
    to_mp3: "Превратить в MP3",
    find_better: "Найти качество лучше",
    pick_release: "Выбрать раздачу…",
    apart: "Разные песни",
    file: "Файл",
    dislike: "Не нравится",
    undislike: "Убрать «Не нравится»",
    ban_artist: "Не рекомендовать исполнителя",
    unban_artist: "Снова рекомендовать исполнителя",
    to_playlist: "В плейлист…",
    to_album: "К альбому",
    to_artist: "К исполнителю",
    share: "Поделиться",
    source_quality: "Источник и качество",
    download_album: "Скачать альбом",
    speed: "Скорость…",
    download: "Скачать",
    edit_tags: "Редактировать теги…",
    delete_from_library: "Удалить из библиотеки",
    purge_forever: "Удалить навсегда",
    play_whole_album: "Слушать весь альбом",
    radio_by_album: "Радио по альбому",
    download_whole_album: "Скачать весь альбом",
    delete_album: "Удалить альбом",
    delete_album_forever: "Удалить альбом навсегда",
    add_album: "Добавить в библиотеку",
    keep_mine: "Оставить только мои треки",
    refresh_cover: "Обновить обложку",
    radio_by_playlist: "Радио по плейлисту",
    edit_item: "Изменить…",
    rename_item: "Переименовать…",
    sync_two_way: "Изменения в обе стороны",
    sync_follow: "Только получать изменения",
    sync_off: "Выключить синхронизацию",
    link_service: "Связать с сервисом…",
    delete_playlist: "Удалить плейлист",
    play_mix: "Слушать микс",
    radio_artist: "Радио исполнителя",
    follow: "Подписаться",
    unfollow: "Отписаться",
    in_subscriptions: "В подписках",
    list_tracks: "Список треков",
    save_as_playlist: "Сохранить как плейлист",
    download_missing: "Скачать недостающие",
    redraw_mix: "Другой состав",
    blend: "Смешать с…",
    hide_mix: "Не предлагать этот микс",
    remove_from_playlist: "Убрать из плейлиста",
};

pub static EN: MenusStrings = MenusStrings {
    radio_by_track: "Track radio",
    play_next: "Play next",
    to_queue: "Add to queue",
    to_library: "Add to library",
    out_of_library: "Remove from library",
    like: "Like",
    unlike: "Unlike",
    wrong_file: "File has a different song",
    to_mp3: "Convert to MP3",
    find_better: "Find better quality",
    pick_release: "Choose torrent…",
    apart: "Different songs",
    file: "File",
    dislike: "Dislike",
    undislike: "Remove “Dislike”",
    ban_artist: "Don't recommend artist",
    unban_artist: "Recommend artist again",
    to_playlist: "Add to playlist…",
    to_album: "Go to album",
    to_artist: "Go to artist",
    share: "Share",
    source_quality: "Source and quality",
    download_album: "Download album",
    speed: "Playback speed…",
    download: "Download",
    edit_tags: "Edit tags…",
    delete_from_library: "Delete from library",
    purge_forever: "Delete forever",
    play_whole_album: "Play whole album",
    radio_by_album: "Album radio",
    download_whole_album: "Download whole album",
    delete_album: "Delete album",
    delete_album_forever: "Delete album forever",
    add_album: "Add to library",
    keep_mine: "Keep only my tracks",
    refresh_cover: "Refresh cover",
    radio_by_playlist: "Playlist radio",
    edit_item: "Edit…",
    rename_item: "Rename…",
    sync_two_way: "Changes both ways",
    sync_follow: "Receive changes only",
    sync_off: "Turn sync off",
    link_service: "Link to a service…",
    delete_playlist: "Delete playlist",
    play_mix: "Play mix",
    radio_artist: "Artist radio",
    follow: "Follow",
    unfollow: "Unfollow",
    in_subscriptions: "Following",
    list_tracks: "Track list",
    save_as_playlist: "Save as playlist",
    download_missing: "Download missing",
    redraw_mix: "Another mix",
    blend: "Blend with…",
    hide_mix: "Don't suggest this mix",
    remove_from_playlist: "Remove from playlist",
};

locale_table! {
    /// Returns these strings for `loc`.
    menus_in -> MenusStrings,
    /// Returns these strings for the active process locale.
    menus,
    { Ru => RU, En => EN }
}

/// Formats «Есть и на альбоме «{album}»» (`Also on the album “{album}”`).
pub fn fmt_also_on_album_in(loc: Locale, album: &str) -> String {
    match loc {
        Locale::Ru => format!("Есть и на альбоме «{album}»"),
        Locale::En => format!("Also on the album “{album}”"),
    }
}

/// Formats «Есть и на альбоме «{album}»» in the active locale.
pub fn fmt_also_on_album(album: &str) -> String {
    fmt_also_on_album_in(locale(), album)
}

/// Formats the «Синхронизация с {service}» group label (`Sync with {service}`).
pub fn fmt_sync_with_in(loc: Locale, service: &str) -> String {
    match loc {
        Locale::Ru => format!("Синхронизация с {service}"),
        Locale::En => format!("Sync with {service}"),
    }
}

/// Formats the «Синхронизация с {service}» group label in the active locale.
pub fn fmt_sync_with(service: &str) -> String {
    fmt_sync_with_in(locale(), service)
}

/// Formats the delete dialog's «Альбом «{title}»» (`Album “{title}”`).
pub fn fmt_album_quoted_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("Альбом «{title}»"),
        Locale::En => format!("Album “{title}”"),
    }
}

/// Formats the delete dialog's «Альбом «{title}»» in the active locale.
pub fn fmt_album_quoted(title: &str) -> String {
    fmt_album_quoted_in(locale(), title)
}

/// Formats the «Разные песни» confirmation body
/// (««{t}» и песня с альбома «{a}» больше не будут считаться одной. Отменить нельзя.»).
pub fn fmt_apart_body(title: &str, album: &str) -> String {
    match locale() {
        Locale::Ru => format!("«{title}» и песня с альбома «{album}» больше не будут считаться одной. Отменить нельзя."),
        Locale::En => format!("“{title}” and the song from the album “{album}” will no longer count as one. This cannot be undone."),
    }
}

/// Formats «{title}» / “{title}” for a delete confirmation.
pub fn fmt_quoted(title: &str) -> String {
    match locale() {
        Locale::Ru => format!("«{title}»"),
        Locale::En => format!("“{title}”"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_items_match_both_locales() {
        assert_eq!(fmt_also_on_album_in(Locale::Ru, "X"), "Есть и на альбоме «X»");
        assert_eq!(fmt_also_on_album_in(Locale::En, "X"), "Also on the album “X”");
        assert_eq!(fmt_sync_with_in(Locale::Ru, "Spotify"), "Синхронизация с Spotify");
        assert_eq!(fmt_sync_with_in(Locale::En, "Spotify"), "Sync with Spotify");
        assert_eq!(fmt_album_quoted_in(Locale::Ru, "X"), "Альбом «X»");
        assert_eq!(fmt_album_quoted_in(Locale::En, "X"), "Album “X”");
        assert_eq!(menus_in(Locale::Ru).play_next, "Сыграть следующим");
        assert_eq!(menus_in(Locale::En).play_next, "Play next");
    }
}
