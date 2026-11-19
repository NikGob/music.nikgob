//! Album, artist and playlist detail pages.

use super::{Locale, PluralForms, locale, plural_forms};

/// Static strings: album, artist and playlist detail pages.
pub struct DetailStrings {
    pub type_album: &'static str,
    pub type_single: &'static str,
    pub type_compilation: &'static str,
    pub type_release: &'static str,
    pub dl_downloading: &'static str,
    pub dl_wait_torrents: &'static str,
    pub dl_in_queue: &'static str,
    pub dl_not_found: &'static str,
    pub replace_flac: &'static str,
    pub character: &'static str,
    pub artist: &'static str,
    pub artist_mix: &'static str,
    pub radio: &'static str,
    pub radio_tip: &'static str,
    pub popular: &'static str,
    pub singles_eps: &'static str,
    pub compilations_other: &'static str,
    pub not_in_library: &'static str,
    pub playlist_kicker: &'static str,
    pub edit_dialog: &'static str,
    pub name_label: &'static str,
    pub cover_auto: &'static str,
    pub cover_auto_tip: &'static str,
    pub cover_collage: &'static str,
    pub cover_collage_tip: &'static str,
    pub own_image: &'static str,
    pub cover_dialog_title: &'static str,
    pub images_filter: &'static str,
    pub add_tracks: &'static str,
    pub add_search: &'static str,
    pub add_hint: &'static str,
    pub add_empty: &'static str,
    pub already_in: &'static str,
    pub playlist_empty: &'static str,
    pub plus_tip: &'static str,
    pub mix_kicker: &'static str,
    pub mix_empty: &'static str,
    pub mix_empty_downloaded: &'static str,
    pub mix_empty_none: &'static str,
    pub gone_title: &'static str,
    pub gone_world: &'static str,
    pub gone_world_action: &'static str,
    pub gone_hidden: &'static str,
    pub gone_home_action: &'static str,
    pub gone_updated: &'static str,
    pub releases: PluralForms,
}

pub static RU: DetailStrings = DetailStrings {
    type_album: "АЛЬБОМ",
    type_single: "СИНГЛ",
    type_compilation: "СБОРНИК",
    type_release: "РЕЛИЗ",
    dl_downloading: "Качается",
    dl_wait_torrents: "Ждёт торренты",
    dl_in_queue: "В очереди",
    dl_not_found: "Не нашлось",
    replace_flac: "Заменить на FLAC…",
    character: "ПЕРСОНАЖ",
    artist: "ИСПОЛНИТЕЛЬ",
    artist_mix: "Микс исполнителя",
    radio: "Радио",
    radio_tip: "Его треки и похожее — очередь дописывается сама",
    popular: "Популярное у тебя",
    singles_eps: "Синглы и EP",
    compilations_other: "Сборники и другое",
    not_in_library: "Нет в библиотеке",
    playlist_kicker: "ПЛЕЙЛИСТ",
    edit_dialog: "Изменить плейлист",
    name_label: "Название",
    cover_auto: "Авто",
    cover_auto_tip: "Коллаж, если в плейлисте 4+ альбома, иначе самая частая обложка",
    cover_collage: "Коллаж",
    cover_collage_tip: "Обложки первых четырёх альбомов, 2×2",
    own_image: "Своя картинка…",
    cover_dialog_title: "Обложка плейлиста",
    images_filter: "Изображения",
    add_tracks: "Добавить треки",
    add_search: "Поиск треков",
    add_hint: "Введи название трека или исполнителя.",
    add_empty: "Ничего не нашлось.",
    already_in: "Уже в плейлисте",
    playlist_empty: "Плейлист пуст",
    plus_tip: "С дополнением",
    mix_kicker: "МИКС",
    mix_empty: "В миксе нет треков",
    mix_empty_downloaded: "Включено «Только скачанное», а скачанных треков для него нет.",
    mix_empty_none: "Его песни ещё не нашлись.",
    gone_title: "Этого микса больше нет",
    gone_world: "На странице мира теперь другие миксы.",
    gone_world_action: "К миру",
    gone_hidden: "Ты попросил его не предлагать. Вернуть можно в Настройках → «Вид» → «Главная».",
    gone_home_action: "На главную",
    gone_updated: "Миксы на главной обновились.",
    releases: PluralForms { one: "релиз", few: "релиза", many: "релизов" },
};

pub static EN: DetailStrings = DetailStrings {
    type_album: "ALBUM",
    type_single: "SINGLE",
    type_compilation: "COMPILATION",
    type_release: "RELEASE",
    dl_downloading: "Downloading",
    dl_wait_torrents: "Waiting for torrents",
    dl_in_queue: "In queue",
    dl_not_found: "Not found",
    replace_flac: "Replace with FLAC…",
    character: "CHARACTER",
    artist: "ARTIST",
    artist_mix: "Artist mix",
    radio: "Radio",
    radio_tip: "Their tracks and similar — the queue fills itself",
    popular: "Popular with you",
    singles_eps: "Singles & EPs",
    compilations_other: "Compilations & other",
    not_in_library: "Not in library",
    playlist_kicker: "PLAYLIST",
    edit_dialog: "Edit playlist",
    name_label: "Name",
    cover_auto: "Auto",
    cover_auto_tip: "Collage with 4+ albums in the playlist, else the most common cover",
    cover_collage: "Collage",
    cover_collage_tip: "Covers of the first four albums, 2×2",
    own_image: "Custom image…",
    cover_dialog_title: "Playlist cover",
    images_filter: "Images",
    add_tracks: "Add tracks",
    add_search: "Search tracks",
    add_hint: "Type a track or artist name.",
    add_empty: "Nothing found.",
    already_in: "Already in playlist",
    playlist_empty: "Playlist is empty",
    plus_tip: "With new songs",
    mix_kicker: "MIX",
    mix_empty: "No tracks in this mix",
    mix_empty_downloaded: "“Downloaded only” is on, and none of its tracks are downloaded.",
    mix_empty_none: "Its songs haven't been found yet.",
    gone_title: "This mix is gone",
    gone_world: "The world's page has different mixes now.",
    gone_world_action: "To the world",
    gone_hidden: "You asked not to be offered it. Bring it back in Settings → “View” → “Home”.",
    gone_home_action: "To home",
    gone_updated: "The mixes on Home have changed.",
    releases: PluralForms { one: "release", few: "releases", many: "releases" },
};

locale_table! {
    /// Returns these strings for `loc`.
    detail_in -> DetailStrings,
    /// Returns these strings for the active process locale.
    detail,
    { Ru => RU, En => EN }
}

/// Formats «{local} скачано · {lacking} нет» (`{local} downloaded · {lacking} missing`).
pub fn fmt_downloaded_missing_in(loc: Locale, local: u32, lacking: u32) -> String {
    match loc {
        Locale::Ru => format!("{local} скачано · {lacking} нет"),
        Locale::En => format!("{local} downloaded · {lacking} missing"),
    }
}

/// Formats «{local} скачано · {lacking} нет» in the active locale.
pub fn fmt_downloaded_missing(local: u32, lacking: u32) -> String {
    fmt_downloaded_missing_in(locale(), local, lacking)
}

/// Formats «{n} {релиз} в библиотеке» (`{n} {release} in library`).
pub fn fmt_releases_in_library_in(loc: Locale, n: u64) -> String {
    let unit = plural_forms(loc, n, &detail_in(loc).releases);
    match loc {
        Locale::Ru => format!("{n} {unit} в библиотеке"),
        Locale::En => format!("{n} {unit} in library"),
    }
}

/// Formats «{n} {релиз} в библиотеке» in the active locale.
pub fn fmt_releases_in_library(n: u64) -> String {
    fmt_releases_in_library_in(locale(), n)
}

/// Formats «Мои треки · {n}» (`My tracks · {n}`).
pub fn fmt_my_tracks_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("Мои треки · {n}"),
        Locale::En => format!("My tracks · {n}"),
    }
}

/// Formats «Мои треки · {n}» in the active locale.
pub fn fmt_my_tracks(n: usize) -> String {
    fmt_my_tracks_in(locale(), n)
}

/// Formats «Весь альбом · {n}» (`Whole album · {n}`).
pub fn fmt_whole_album_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("Весь альбом · {n}"),
        Locale::En => format!("Whole album · {n}"),
    }
}

/// Formats «Весь альбом · {n}» in the active locale.
pub fn fmt_whole_album(n: usize) -> String {
    fmt_whole_album_in(locale(), n)
}

/// Formats «Не скачано · {n}» (`Not downloaded · {n}`).
pub fn fmt_not_downloaded_in(loc: Locale, n: u32) -> String {
    match loc {
        Locale::Ru => format!("Не скачано · {n}"),
        Locale::En => format!("Not downloaded · {n}"),
    }
}

/// Formats «Не скачано · {n}» in the active locale.
pub fn fmt_not_downloaded(n: u32) -> String {
    fmt_not_downloaded_in(locale(), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmts_match_both_locales() {
        let ru = Locale::Ru;
        let en = Locale::En;
        assert_eq!(fmt_downloaded_missing_in(ru, 13, 1), "13 скачано · 1 нет");
        assert_eq!(fmt_downloaded_missing_in(en, 13, 1), "13 downloaded · 1 missing");
        assert_eq!(fmt_releases_in_library_in(ru, 1), "1 релиз в библиотеке");
        assert_eq!(fmt_releases_in_library_in(ru, 3), "3 релиза в библиотеке");
        assert_eq!(fmt_releases_in_library_in(ru, 5), "5 релизов в библиотеке");
        assert_eq!(fmt_releases_in_library_in(en, 1), "1 release in library");
        assert_eq!(fmt_releases_in_library_in(en, 3), "3 releases in library");
        assert_eq!(fmt_my_tracks_in(ru, 2), "Мои треки · 2");
        assert_eq!(fmt_my_tracks_in(en, 2), "My tracks · 2");
        assert_eq!(fmt_whole_album_in(ru, 9), "Весь альбом · 9");
        assert_eq!(fmt_whole_album_in(en, 9), "Whole album · 9");
        assert_eq!(fmt_not_downloaded_in(ru, 1), "Не скачано · 1");
        assert_eq!(fmt_not_downloaded_in(en, 1), "Not downloaded · 1");
        assert_eq!(detail_in(ru).type_album, "АЛЬБОМ");
        assert_eq!(detail_in(en).type_album, "ALBUM");
    }
}
