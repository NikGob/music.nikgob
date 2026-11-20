//! Toasts of the runtime's command dispatch (playlists, likes, home cards).

use super::{Locale, locale};

/// Static strings of this domain.
pub struct DispatchToastStrings {
    pub playlist_deleted: &'static str,
    pub home_card_hidden: &'static str,
    pub home_card_restore: &'static str,
    pub liked_background: &'static str,
    pub liked_when_torrents: &'static str,
    pub liked_added: &'static str,
    pub liked_removed: &'static str,
    pub kind_track: &'static str,
    pub kind_album: &'static str,
}

pub static RU: DispatchToastStrings = DispatchToastStrings {
    playlist_deleted: "Плейлист удалён",
    home_card_hidden: "Карточка скрыта. Включить её снова можно в Настройках → Внешний вид → Главная.",
    home_card_restore: "Вернуть",
    liked_background: "В Любимых · скачаю в фоне",
    liked_when_torrents: "В Любимых · скачается, когда подключишь торренты",
    liked_added: "Добавлено в Любимые",
    liked_removed: "Убрано из Любимых",
    kind_track: "Трек",
    kind_album: "Альбом",
};

pub static EN: DispatchToastStrings = DispatchToastStrings {
    playlist_deleted: "Playlist deleted",
    home_card_hidden: "Card hidden. You can turn it back on in Settings → Appearance → Home.",
    home_card_restore: "Restore",
    liked_background: "In Liked Songs · will download in the background",
    liked_when_torrents: "In Liked Songs · will download once you connect torrents",
    liked_added: "Added to Liked Songs",
    liked_removed: "Removed from Liked Songs",
    kind_track: "Track",
    kind_album: "Album",
};

locale_table! {
    /// Returns this domain's strings for `loc`.
    dispatch_toasts_in -> DispatchToastStrings,
    /// Returns this domain's strings for the active process locale.
    dispatch_toasts,
    { Ru => RU, En => EN }
}

/// Formats the toast of «Добавить в плейлист» from the added and
/// already-present counts.
pub fn fmt_added_to_in(loc: Locale, name: &str, added: usize, already: usize) -> String {
    match (loc, added, already) {
        (Locale::Ru, _, 0) => format!("Добавлено в «{name}»"),
        (Locale::En, _, 0) => format!("Added to “{name}”"),
        (Locale::Ru, 0, _) => format!("Уже есть в «{name}»"),
        (Locale::En, 0, _) => format!("Already in “{name}”"),
        (Locale::Ru, _, 1) => format!("Добавлено {added}, 1 уже был в «{name}»"),
        (Locale::En, _, 1) => format!("Added {added}, 1 was already in “{name}”"),
        (Locale::Ru, _, _) => format!("Добавлено {added}, {already} уже были в «{name}»"),
        (Locale::En, _, _) => format!("Added {added}, {already} were already in “{name}”"),
    }
}

/// Formats the «Добавить в плейлист» toast in the active locale.
pub fn fmt_added_to(name: &str, added: usize, already: usize) -> String {
    fmt_added_to_in(locale(), name, added, already)
}

/// Formats the «playlist saved» toast.
pub fn fmt_playlist_saved_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Плейлист «{name}» сохранён"),
        Locale::En => format!("Playlist “{name}” saved"),
    }
}

/// Formats the «playlist saved» toast in the active locale.
pub fn fmt_playlist_saved(name: &str) -> String {
    fmt_playlist_saved_in(locale(), name)
}

/// Formats the «removed from playlist» toast.
pub fn fmt_removed_from_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Убрано из «{name}»"),
        Locale::En => format!("Removed from “{name}”"),
    }
}

/// Formats the «removed from playlist» toast in the active locale.
pub fn fmt_removed_from(name: &str) -> String {
    fmt_removed_from_in(locale(), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmts_match_both_locales() {
        let (ru, en) = (Locale::Ru, Locale::En);
        assert_eq!(fmt_added_to_in(ru, "Микс", 0, 0), "Добавлено в «Микс»");
        assert_eq!(fmt_added_to_in(ru, "Микс", 0, 2), "Уже есть в «Микс»");
        assert_eq!(fmt_added_to_in(ru, "Микс", 3, 0), "Добавлено в «Микс»");
        assert_eq!(fmt_added_to_in(ru, "Микс", 3, 1), "Добавлено 3, 1 уже был в «Микс»");
        assert_eq!(fmt_added_to_in(ru, "Микс", 3, 4), "Добавлено 3, 4 уже были в «Микс»");
        assert_eq!(fmt_added_to_in(en, "Mix", 0, 0), "Added to “Mix”");
        assert_eq!(fmt_added_to_in(en, "Mix", 0, 2), "Already in “Mix”");
        assert_eq!(fmt_added_to_in(en, "Mix", 3, 0), "Added to “Mix”");
        assert_eq!(fmt_added_to_in(en, "Mix", 3, 1), "Added 3, 1 was already in “Mix”");
        assert_eq!(fmt_added_to_in(en, "Mix", 3, 4), "Added 3, 4 were already in “Mix”");
        assert_eq!(fmt_playlist_saved_in(ru, "Утро"), "Плейлист «Утро» сохранён");
        assert_eq!(fmt_playlist_saved_in(en, "Morning"), "Playlist “Morning” saved");
        assert_eq!(fmt_removed_from_in(ru, "Утро"), "Убрано из «Утро»");
        assert_eq!(fmt_removed_from_in(en, "Morning"), "Removed from “Morning”");
    }
}
