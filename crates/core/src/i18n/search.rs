//! Search page.

use super::{Locale, PluralForms, locale};

/// Static strings: search page.
pub struct SearchStrings {
    pub chip_torrents: &'static str,
    pub no_results: &'static str,
    pub library_none_body: &'static str,
    pub no_results_body: &'static str,
    pub prowlarr_hint: &'static str,
    pub setup: &'static str,
    pub search_torrents: &'static str,
    pub none_found: &'static str,
    pub recommended: &'static str,
    pub pick_files: &'static str,
    pub pick_files_title: &'static str,
    pub torrent_word: &'static str,
    pub reading_files: &'static str,
    pub no_files: &'static str,
    pub select_none: &'static str,
    pub files_readonly_note: &'static str,
    pub best_result: &'static str,
    pub more_tracks: &'static str,
    pub turn_on: &'static str,
    pub seeds: PluralForms,
}

pub static RU: SearchStrings = SearchStrings {
    chip_torrents: "Торренты",
    no_results: "Ничего не нашлось",
    library_none_body: "В библиотеке такого нет.",
    no_results_body: "Проверь, как написано, или поищи раздачи в торрентах.",
    prowlarr_hint: "Для раздач нужен Prowlarr: адрес и ключ — в Настройках → Источники → Торренты.",
    setup: "Настроить",
    search_torrents: "Искать раздачи",
    none_found: "Раздач не нашлось.",
    recommended: "рекоменд.",
    pick_files: "Выбрать файлы…",
    pick_files_title: "Выбрать файлы",
    torrent_word: "Раздача",
    reading_files: "Читаю список файлов…",
    no_files: "В раздаче нет файлов.",
    select_none: "Ничего",
    files_readonly_note: "Выбор файлов для .torrent и magnet пока недоступен: список только для просмотра.",
    best_result: "Лучший результат",
    more_tracks: "Ещё треки",
    turn_on: "Включить",
    seeds: PluralForms { one: "сид", few: "сида", many: "сидов" },
};

pub static EN: SearchStrings = SearchStrings {
    chip_torrents: "Torrents",
    no_results: "Nothing found",
    library_none_body: "Nothing found in your library.",
    no_results_body: "Check the spelling, or look for torrents.",
    prowlarr_hint: "Torrents require Prowlarr. Add its address and key in Settings → Sources → Torrents.",
    setup: "Set up",
    search_torrents: "Search torrents",
    none_found: "No torrents found.",
    recommended: "recommended",
    pick_files: "Choose files…",
    pick_files_title: "Choose files",
    torrent_word: "Torrent",
    reading_files: "Reading the file list…",
    no_files: "No files in this torrent.",
    select_none: "None",
    files_readonly_note: "File selection is not available for .torrent files or magnet links yet. You can only view the list.",
    best_result: "Top result",
    more_tracks: "More tracks",
    turn_on: "Turn on",
    seeds: PluralForms { one: "seed", few: "seeds", many: "seeds" },
};

locale_table! {
    /// Returns these strings for `loc`.
    search_in -> SearchStrings,
    /// Returns these strings for the active process locale.
    search,
    { Ru => RU, En => EN }
}

/// Formats «Результаты для «{query}»» (`Results for “{query}”`).
pub fn fmt_results_for_in(loc: Locale, query: &str) -> String {
    match loc {
        Locale::Ru => format!("Результаты для «{query}»"),
        Locale::En => format!("Results for “{query}”"),
    }
}

/// Formats «Результаты для «{query}»» in the active locale.
pub fn fmt_results_for(query: &str) -> String {
    fmt_results_for_in(locale(), query)
}

/// Formats «{name} нет в этой сборке.» (`{name} is not in this build.`).
pub fn fmt_not_in_build_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} нет в этой сборке."),
        Locale::En => format!("{name} is not in this build."),
    }
}

/// Formats «{name} нет в этой сборке.» in the active locale.
pub fn fmt_not_in_build(name: &str) -> String {
    fmt_not_in_build_in(locale(), name)
}

/// Formats «{name} выключен.» (`{name} is off.`).
pub fn fmt_service_off_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} выключен."),
        Locale::En => format!("{name} is off."),
    }
}

/// Formats «{name} выключен.» in the active locale.
pub fn fmt_service_off(name: &str) -> String {
    fmt_service_off_in(locale(), name)
}

/// Formats the sign-in empty state's title «Войди в {name}» (`Sign in to {name}`).
pub fn fmt_sign_in_title_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Войди в {name}"),
        Locale::En => format!("Sign in to {name}"),
    }
}

/// Formats the sign-in empty state's title in the active locale.
pub fn fmt_sign_in_title(name: &str) -> String {
    fmt_sign_in_title_in(locale(), name)
}

/// Formats the sign-in empty state's body.
pub fn fmt_sign_in_body_in(loc: Locale) -> &'static str {
    match loc {
        Locale::Ru => "Тогда поиск найдёт и там: треки, альбомы, исполнителей и плейлисты.",
        Locale::En => "Search will include tracks, albums, artists and playlists from this service.",
    }
}

/// Formats the sign-in empty state's body in the active locale.
pub fn fmt_sign_in_body() -> &'static str {
    fmt_sign_in_body_in(locale())
}

/// Formats the sign-in button «Войти через {name}» (`Sign in with {name}`).
pub fn fmt_sign_in_button_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Войти через {name}"),
        Locale::En => format!("Sign in with {name}"),
    }
}

/// Formats the sign-in button in the active locale.
pub fn fmt_sign_in_button(name: &str) -> String {
    fmt_sign_in_button_in(locale(), name)
}

/// Formats «Вхожу в {name}…» (`Signing in to {name}…`).
pub fn fmt_signing_in_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Вхожу в {name}…"),
        Locale::En => format!("Signing in to {name}…"),
    }
}

/// Formats «Вхожу в {name}…» in the active locale.
pub fn fmt_signing_in(name: &str) -> String {
    fmt_signing_in_in(locale(), name)
}

/// Formats «{name} просит подождать до {hm}.» (`{name} asks to wait until {hm}.`).
pub fn fmt_paused_until_in(loc: Locale, name: &str, hm: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} просит подождать до {hm}."),
        Locale::En => format!("{name} is asking you to wait until {hm}."),
    }
}

/// Formats «{name} просит подождать до {hm}.» in the active locale.
pub fn fmt_paused_until(name: &str, hm: &str) -> String {
    fmt_paused_until_in(locale(), name, hm)
}

/// Formats «В {name} ничего не нашлось.» (`Nothing found on {name}.`).
pub fn fmt_nothing_on_service_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("В {name} ничего не нашлось."),
        Locale::En => format!("Nothing found on {name}."),
    }
}

/// Formats «В {name} ничего не нашлось.» in the active locale.
pub fn fmt_nothing_on_service(name: &str) -> String {
    fmt_nothing_on_service_in(locale(), name)
}

/// Formats «{kind} · в библиотеке» (`{kind} · in library`) of a top artist card.
pub fn fmt_artist_in_library_in(loc: Locale, kind: &str) -> String {
    match loc {
        Locale::Ru => format!("{kind} · в библиотеке"),
        Locale::En => format!("{kind} · in your library"),
    }
}

/// Formats «{kind} · в библиотеке» in the active locale.
pub fn fmt_artist_in_library(kind: &str) -> String {
    fmt_artist_in_library_in(locale(), kind)
}

/// Formats the top track card's line «Трек · {artist}» (`Track · {artist}`).
pub fn fmt_track_kind_in(loc: Locale, artist: &str) -> String {
    match loc {
        Locale::Ru => format!("Трек · {artist}"),
        Locale::En => format!("Track · {artist}"),
    }
}

/// Formats the top track card's line in the active locale.
pub fn fmt_track_kind(artist: &str) -> String {
    fmt_track_kind_in(locale(), artist)
}

/// Formats the top album card's line (`Альбом · Кино · 1988` / `Album · Kino · 1988`).
pub fn fmt_album_kind_in(loc: Locale, artist: &str, year: Option<i32>) -> String {
    match (loc, year) {
        (Locale::Ru, Some(y)) if !artist.is_empty() => format!("Альбом · {artist} · {y}"),
        (Locale::Ru, Some(y)) => format!("Альбом · {y}"),
        (Locale::Ru, None) if !artist.is_empty() => format!("Альбом · {artist}"),
        (Locale::Ru, None) => "Альбом".to_owned(),
        (Locale::En, Some(y)) if !artist.is_empty() => format!("Album · {artist} · {y}"),
        (Locale::En, Some(y)) => format!("Album · {y}"),
        (Locale::En, None) if !artist.is_empty() => format!("Album · {artist}"),
        (Locale::En, None) => "Album".to_owned(),
    }
}

/// Formats the top album card's line in the active locale.
pub fn fmt_album_kind(artist: &str, year: Option<i32>) -> String {
    fmt_album_kind_in(locale(), artist, year)
}

/// Formats «Выбрано {n} из {total} · {size}» of the torrent files dialog.
pub fn fmt_selected_files_in(loc: Locale, n: usize, total: usize, size: String) -> String {
    match loc {
        Locale::Ru => format!("Выбрано {n} из {total} · {size}"),
        Locale::En => format!("{n} of {total} selected · {size}"),
    }
}

/// Formats «Выбрано {n} из {total} · {size}» in the active locale.
pub fn fmt_selected_files(n: usize, total: usize, size: String) -> String {
    fmt_selected_files_in(locale(), n, total, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::plural_forms;

    #[test]
    fn seeds_plural_and_fmts_match_both_locales() {
        let ru = Locale::Ru;
        let en = Locale::En;
        assert_eq!(plural_forms(ru, 1, &RU.seeds), "сид");
        assert_eq!(plural_forms(ru, 4, &RU.seeds), "сида");
        assert_eq!(plural_forms(ru, 5, &RU.seeds), "сидов");
        assert_eq!(plural_forms(en, 1, &EN.seeds), "seed");
        assert_eq!(plural_forms(en, 2, &EN.seeds), "seeds");
        assert_eq!(fmt_results_for_in(ru, "кино"), "Результаты для «кино»");
        assert_eq!(fmt_results_for_in(en, "kino"), "Results for “kino”");
        assert_eq!(fmt_sign_in_title_in(ru, "Spotify"), "Войди в Spotify");
        assert_eq!(fmt_sign_in_title_in(en, "Spotify"), "Sign in to Spotify");
        assert_eq!(fmt_paused_until_in(ru, "Spotify", "12:00"), "Spotify просит подождать до 12:00.");
        assert_eq!(fmt_paused_until_in(en, "Spotify", "12:00"), "Spotify is asking you to wait until 12:00.");
        assert_eq!(fmt_nothing_on_service_in(ru, "Deezer"), "В Deezer ничего не нашлось.");
        assert_eq!(fmt_nothing_on_service_in(en, "Deezer"), "Nothing found on Deezer.");
        assert_eq!(fmt_artist_in_library_in(ru, "Исполнитель"), "Исполнитель · в библиотеке");
        assert_eq!(fmt_artist_in_library_in(en, "Artist"), "Artist · in your library");
        assert_eq!(fmt_track_kind_in(ru, "Кино"), "Трек · Кино");
        assert_eq!(fmt_track_kind_in(en, "Kino"), "Track · Kino");
        assert_eq!(fmt_album_kind_in(ru, "Кино", Some(1988)), "Альбом · Кино · 1988");
        assert_eq!(fmt_album_kind_in(ru, "", Some(1988)), "Альбом · 1988");
        assert_eq!(fmt_album_kind_in(ru, "Кино", None), "Альбом · Кино");
        assert_eq!(fmt_album_kind_in(ru, "", None), "Альбом");
        assert_eq!(fmt_album_kind_in(en, "Kino", Some(1988)), "Album · Kino · 1988");
        assert_eq!(fmt_album_kind_in(en, "", None), "Album");
        assert_eq!(RU.no_results, "Ничего не нашлось");
        assert_eq!(EN.no_results, "Nothing found");
        assert_eq!(RU.pick_files, "Выбрать файлы…");
        assert_eq!(RU.pick_files_title, "Выбрать файлы");
        assert_eq!(EN.pick_files_title, "Choose files");
    }
}
