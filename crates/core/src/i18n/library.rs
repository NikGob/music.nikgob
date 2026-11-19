//! Library page: tabs, sorting, empty states.

use super::{Locale, PluralForms, locale, plural_forms};

/// Static strings: library page: tabs, sorting, empty states.
pub struct LibraryStrings {
    pub tab_all: &'static str,
    pub tab_playlists: &'static str,
    pub tab_albums: &'static str,
    pub tab_artists: &'static str,
    pub tab_tracks: &'static str,
    pub tab_downloaded: &'static str,
    pub tab_trash: &'static str,
    pub sort_artist: &'static str,
    pub sort_title: &'static str,
    pub sort_year: &'static str,
    pub sort_added: &'static str,
    pub sort: &'static str,
    pub view_grid: &'static str,
    pub view_list: &'static str,
    pub only_downloaded_on: &'static str,
    pub only_downloaded_off: &'static str,
    pub hint_playlists: &'static str,
    pub hint_albums: &'static str,
    pub hint_artists: &'static str,
    pub hint_tracks: &'static str,
    pub hint_default: &'static str,
    pub empty_title: &'static str,
    pub empty_body: &'static str,
    pub no_results: &'static str,
    pub search_everywhere: &'static str,
    pub search_tracks: &'static str,
    pub recently_added: &'static str,
    pub often_played: &'static str,
    pub chip_singles: &'static str,
    pub chip_liked: &'static str,
    pub source_all: &'static str,
    pub source_downloaded: &'static str,
    pub source_stream: &'static str,
    pub new_playlist: &'static str,
    pub tracks_empty: &'static str,
    pub artists_empty: &'static str,
    pub downloaded_empty: &'static str,
    pub show_more: &'static str,
    pub empty_trash: &'static str,
    pub restore_all: &'static str,
    pub trash_empty: &'static str,
    pub restore: &'static str,
    pub quality_col: &'static str,
    pub playlists: PluralForms,
    pub albums: PluralForms,
    pub days: PluralForms,
}

pub static RU: LibraryStrings = LibraryStrings {
    tab_all: "Всё",
    tab_playlists: "Плейлисты",
    tab_albums: "Альбомы",
    tab_artists: "Исполнители",
    tab_tracks: "Треки",
    tab_downloaded: "Скачано",
    tab_trash: "Корзина",
    sort_artist: "По исполнителю",
    sort_title: "По названию",
    sort_year: "По году",
    sort_added: "Недавно добавленные",
    sort: "Сортировка",
    view_grid: "Сетка",
    view_list: "Список",
    only_downloaded_on: "Только скачанное: стрим скрыт. Нажми, чтобы показать всё",
    only_downloaded_off: "Только скачанное: скрыть то, что играет из сервисов",
    hint_playlists: "Найти плейлист",
    hint_albums: "Найти альбом или исполнителя",
    hint_artists: "Найти исполнителя",
    hint_tracks: "Найти трек, альбом, исполнителя",
    hint_default: "Найти в библиотеке",
    empty_title: "Здесь пока пусто",
    empty_body: "Лайкни трек в поиске — он появится здесь. Свои файлы: папка Inbox или «Добавить».",
    no_results: "Ничего не найдено",
    search_everywhere: "Искать везде",
    search_tracks: "Искать среди треков",
    recently_added: "Недавно добавлено",
    often_played: "Часто слушаешь",
    chip_singles: "Синглы",
    chip_liked: "Любимые",
    source_all: "Все источники",
    source_downloaded: "Скачанные",
    source_stream: "Стрим",
    new_playlist: "Новый плейлист",
    tracks_empty: "Треков пока нет",
    artists_empty: "Исполнителей пока нет",
    downloaded_empty: "Пока ничего",
    show_more: "Показать ещё",
    empty_trash: "Очистить корзину",
    restore_all: "Восстановить всё",
    trash_empty: "Корзина пуста",
    restore: "Восстановить",
    quality_col: "Качество",
    playlists: PluralForms { one: "плейлист", few: "плейлиста", many: "плейлистов" },
    albums: PluralForms { one: "альбом", few: "альбома", many: "альбомов" },
    days: PluralForms { one: "день", few: "дня", many: "дней" },
};

pub static EN: LibraryStrings = LibraryStrings {
    tab_all: "All",
    tab_playlists: "Playlists",
    tab_albums: "Albums",
    tab_artists: "Artists",
    tab_tracks: "Tracks",
    tab_downloaded: "Downloaded",
    tab_trash: "Trash",
    sort_artist: "By artist",
    sort_title: "By title",
    sort_year: "By year",
    sort_added: "Recently added",
    sort: "Sort",
    view_grid: "Grid",
    view_list: "List",
    only_downloaded_on: "Downloaded only: streams hidden. Tap to show all",
    only_downloaded_off: "Downloaded only: hide what plays from services",
    hint_playlists: "Find a playlist",
    hint_albums: "Find an album or artist",
    hint_artists: "Find an artist",
    hint_tracks: "Find a track, album or artist",
    hint_default: "Search the library",
    empty_title: "Nothing here yet",
    empty_body: "Like a track in search and it will show up here. Your own files: the Inbox folder or “Add”.",
    no_results: "Nothing found",
    search_everywhere: "Search everywhere",
    search_tracks: "Search among tracks",
    recently_added: "Recently added",
    often_played: "Often played",
    chip_singles: "Singles",
    chip_liked: "Liked",
    source_all: "All sources",
    source_downloaded: "Downloaded",
    source_stream: "Streaming",
    new_playlist: "New playlist",
    tracks_empty: "No tracks yet",
    artists_empty: "No artists yet",
    downloaded_empty: "Nothing yet",
    show_more: "Show more",
    empty_trash: "Empty trash",
    restore_all: "Restore all",
    trash_empty: "Trash is empty",
    restore: "Restore",
    quality_col: "Quality",
    playlists: PluralForms { one: "playlist", few: "playlists", many: "playlists" },
    albums: PluralForms { one: "album", few: "albums", many: "albums" },
    days: PluralForms { one: "day", few: "days", many: "days" },
};

locale_table! {
    /// Returns these strings for `loc`.
    library_in -> LibraryStrings,
    /// Returns these strings for the active process locale.
    library,
    { Ru => RU, En => EN }
}

/// Formats the sort button's tooltip (`"Сортировка: по году"` / `"Sort: By year"`).
pub fn fmt_sort_tooltip_in(loc: Locale, label: &str) -> String {
    match loc {
        Locale::Ru => format!("Сортировка: {}", label.to_lowercase()),
        Locale::En => format!("Sort: {label}"),
    }
}

/// Formats the sort button's tooltip in the active locale.
pub fn fmt_sort_tooltip(label: &str) -> String {
    fmt_sort_tooltip_in(locale(), label)
}

/// Formats «Все {n}» (`All {n}`) of the overview's «Альбомы» header action.
pub fn fmt_all_count_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("Все {n}"),
        Locale::En => format!("All {n}"),
    }
}

/// Formats «Все {n}» in the active locale.
pub fn fmt_all_count(n: usize) -> String {
    fmt_all_count_in(locale(), n)
}

/// Formats «Нашлось: {count}» (`Found: {count}`).
pub fn fmt_found_in(loc: Locale, count: &str) -> String {
    match loc {
        Locale::Ru => format!("Нашлось: {count}"),
        Locale::En => format!("Found: {count}"),
    }
}

/// Formats «Нашлось: {count}» in the active locale.
pub fn fmt_found(count: &str) -> String {
    fmt_found_in(locale(), count)
}

/// Formats «{count} в корзине» (`{count} in trash`).
pub fn fmt_in_trash_in(loc: Locale, count: &str) -> String {
    match loc {
        Locale::Ru => format!("{count} в корзине"),
        Locale::En => format!("{count} in trash"),
    }
}

/// Formats «{count} в корзине» in the active locale.
pub fn fmt_in_trash(count: &str) -> String {
    fmt_in_trash_in(locale(), count)
}

/// Formats a trash row's tail (`«… · удалено 12.11 · осталось 5 дн.»` /
/// `«… · deleted 12.11 · 5 days left»`).
pub fn fmt_trash_line_in(loc: Locale, deleted: &str, days_left: u32) -> String {
    let unit = plural_forms(loc, u64::from(days_left), &library_in(loc).days);
    match loc {
        Locale::Ru => format!("удалено {deleted} · осталось {days_left} {unit}"),
        Locale::En => format!("deleted {deleted} · {days_left} {unit} left"),
    }
}

/// Formats a trash row's tail in the active locale.
pub fn fmt_trash_line(deleted: &str, days_left: u32) -> String {
    fmt_trash_line_in(locale(), deleted, days_left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_and_fmts_match_both_locales() {
        let ru = Locale::Ru;
        let en = Locale::En;
        assert_eq!(plural_forms(ru, 1, &RU.albums), "альбом");
        assert_eq!(plural_forms(ru, 3, &RU.albums), "альбома");
        assert_eq!(plural_forms(ru, 5, &RU.albums), "альбомов");
        assert_eq!(plural_forms(en, 1, &EN.albums), "album");
        assert_eq!(plural_forms(en, 2, &EN.albums), "albums");
        assert_eq!(fmt_sort_tooltip_in(ru, "По году"), "Сортировка: по году");
        assert_eq!(fmt_sort_tooltip_in(en, "By year"), "Sort: By year");
        assert_eq!(fmt_all_count_in(ru, 7), "Все 7");
        assert_eq!(fmt_all_count_in(en, 7), "All 7");
        assert_eq!(fmt_found_in(ru, "5 треков"), "Нашлось: 5 треков");
        assert_eq!(fmt_found_in(en, "5 tracks"), "Found: 5 tracks");
        assert_eq!(fmt_in_trash_in(ru, "2 трека"), "2 трека в корзине");
        assert_eq!(fmt_in_trash_in(en, "2 tracks"), "2 tracks in trash");
        assert_eq!(fmt_trash_line_in(ru, "12.11", 1), "удалено 12.11 · осталось 1 день");
        assert_eq!(fmt_trash_line_in(ru, "12.11", 5), "удалено 12.11 · осталось 5 дней");
        assert_eq!(fmt_trash_line_in(en, "12.11", 1), "deleted 12.11 · 1 day left");
        assert_eq!(fmt_trash_line_in(en, "12.11", 5), "deleted 12.11 · 5 days left");
    }
}
