//! Toasts of library edits (liked albums, tracks in the library, album choices).

use super::{Locale, tracks_count_in};

/// Static strings of this domain.
pub struct LibraryToastStrings {
    pub album_saved_downloading: &'static str,
    pub album_saved_waiting_torrents: &'static str,
    pub album_saved: &'static str,
    pub album_unsaved: &'static str,
    pub track_in_library: &'static str,
    pub track_out_of_library: &'static str,
    pub track_stays_in_library: &'static str,
    pub album_in_library: &'static str,
    pub album_mine_only: &'static str,
}

pub static RU: LibraryToastStrings = LibraryToastStrings {
    album_saved_downloading: "В любимых альбомах · скачаю в фоне",
    album_saved_waiting_torrents: "В любимых альбомах · скачается, когда подключишь торренты",
    album_saved: "Добавлено в любимые альбомы",
    album_unsaved: "Убрано из любимых альбомов",
    track_in_library: "Трек в библиотеке",
    track_out_of_library: "Трек убран из библиотеки",
    track_stays_in_library: "Трек остаётся в библиотеке: он в «Любимых» или в плейлисте",
    album_in_library: "Альбом в библиотеке",
    album_mine_only: "В библиотеке только твои треки альбома",
};

pub static EN: LibraryToastStrings = LibraryToastStrings {
    album_saved_downloading: "In Liked Albums · downloading in the background",
    album_saved_waiting_torrents: "In Liked Albums · will download once torrents are connected",
    album_saved: "Added to Liked Albums",
    album_unsaved: "Removed from Liked Albums",
    track_in_library: "Track added to the library",
    track_out_of_library: "Track removed from the library",
    track_stays_in_library: "Track stays in the library: it is in Liked Songs or a playlist",
    album_in_library: "Album added to the library",
    album_mine_only: "The library now has only your tracks of the album",
};

locale_table! {
    /// Returns this domain's strings for `loc`.
    library_toasts_in -> LibraryToastStrings,
    /// Returns this domain's strings for the active process locale.
    library_toasts,
    { Ru => RU, En => EN }
}

/// Formats the toast of an album liked with its `n` tracks: «Альбом в Любимых ·
/// 5 треков · скачаю в фоне» and its two other tails.
pub fn fmt_album_liked_in(loc: Locale, n: u64, wished: bool, can_download: bool) -> String {
    let what = tracks_count_in(loc, n);
    match (loc, wished, can_download) {
        (Locale::Ru, true, true) => format!("Альбом в Любимых · {what} · скачаю в фоне"),
        (Locale::Ru, true, false) => {
            format!("Альбом в Любимых · {what} · скачается, когда подключишь торренты")
        }
        (Locale::Ru, false, _) => format!("Альбом в Любимых · {what}"),
        (Locale::En, true, true) => {
            format!("Album added to Liked Songs · {what} · downloading in the background")
        }
        (Locale::En, true, false) => {
            format!("Album added to Liked Songs · {what} · will download once torrents are connected")
        }
        (Locale::En, false, _) => format!("Album added to Liked Songs · {what}"),
    }
}

/// Formats the toast of an album unliked with its `n` tracks: «Альбом убран из
/// Любимых · 5 треков».
pub fn fmt_album_unliked_in(loc: Locale, n: u64) -> String {
    let what = tracks_count_in(loc, n);
    match loc {
        Locale::Ru => format!("Альбом убран из Любимых · {what}"),
        Locale::En => format!("Album removed from Liked Songs · {what}"),
    }
}

/// Formats the toast of a song put into the user's tracks of its album
/// (`«Звезда» в твоих треках альбома`).
pub fn fmt_album_track_mine_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("«{title}» в твоих треках альбома"),
        Locale::En => format!("“{title}” added to your tracks of the album"),
    }
}

/// Formats the toast of a song taken out of the user's tracks of its album
/// (`«Звезда» убран из твоих треков альбома`).
pub fn fmt_album_track_not_mine_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("«{title}» убран из твоих треков альбома"),
        Locale::En => format!("“{title}” removed from your tracks of the album"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_and_track_toasts_translate_both_locales() {
        assert_eq!(fmt_album_liked_in(Locale::Ru, 1, true, true), "Альбом в Любимых · 1 трек · скачаю в фоне");
        assert_eq!(
            fmt_album_liked_in(Locale::Ru, 3, true, false),
            "Альбом в Любимых · 3 трека · скачается, когда подключишь торренты"
        );
        assert_eq!(fmt_album_liked_in(Locale::Ru, 5, false, true), "Альбом в Любимых · 5 треков");
        assert_eq!(fmt_album_unliked_in(Locale::Ru, 11), "Альбом убран из Любимых · 11 треков");
        assert_eq!(fmt_album_unliked_in(Locale::Ru, 22), "Альбом убран из Любимых · 22 трека");

        assert_eq!(
            fmt_album_liked_in(Locale::En, 1, true, true),
            "Album added to Liked Songs · 1 track · downloading in the background"
        );
        assert_eq!(
            fmt_album_liked_in(Locale::En, 3, true, false),
            "Album added to Liked Songs · 3 tracks · will download once torrents are connected"
        );
        assert_eq!(fmt_album_liked_in(Locale::En, 5, false, true), "Album added to Liked Songs · 5 tracks");
        assert_eq!(fmt_album_unliked_in(Locale::En, 2), "Album removed from Liked Songs · 2 tracks");

        assert_eq!(fmt_album_track_mine_in(Locale::Ru, "Звезда"), "«Звезда» в твоих треках альбома");
        assert_eq!(fmt_album_track_not_mine_in(Locale::Ru, "Звезда"), "«Звезда» убран из твоих треков альбома");
        assert_eq!(fmt_album_track_mine_in(Locale::En, "Звезда"), "“Звезда” added to your tracks of the album");
        assert_eq!(
            fmt_album_track_not_mine_in(Locale::En, "Звезда"),
            "“Звезда” removed from your tracks of the album"
        );

        assert_eq!(RU.track_in_library, "Трек в библиотеке");
        assert_eq!(EN.track_in_library, "Track added to the library");
        assert_eq!(RU.track_out_of_library, "Трек убран из библиотеки");
        assert_eq!(EN.track_out_of_library, "Track removed from the library");
        assert_eq!(RU.track_stays_in_library, "Трек остаётся в библиотеке: он в «Любимых» или в плейлисте");
        assert_eq!(EN.track_stays_in_library, "Track stays in the library: it is in Liked Songs or a playlist");
        assert_eq!(RU.album_in_library, "Альбом в библиотеке");
        assert_eq!(EN.album_in_library, "Album added to the library");
        assert_eq!(RU.album_mine_only, "В библиотеке только твои треки альбома");
        assert_eq!(EN.album_mine_only, "The library now has only your tracks of the album");
        assert_eq!(RU.album_saved_downloading, "В любимых альбомах · скачаю в фоне");
        assert_eq!(EN.album_saved_downloading, "In Liked Albums · downloading in the background");
        assert_eq!(RU.album_saved_waiting_torrents, "В любимых альбомах · скачается, когда подключишь торренты");
        assert_eq!(EN.album_saved_waiting_torrents, "In Liked Albums · will download once torrents are connected");
        assert_eq!(RU.album_saved, "Добавлено в любимые альбомы");
        assert_eq!(EN.album_saved, "Added to Liked Albums");
        assert_eq!(RU.album_unsaved, "Убрано из любимых альбомов");
        assert_eq!(EN.album_unsaved, "Removed from Liked Albums");
    }
}
