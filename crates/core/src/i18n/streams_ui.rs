//! Streaming pages and stream rows.

use super::{Locale, PluralForms, locale, plural_forms};

/// Static strings: streaming pages and stream rows.
pub struct StreamsUiStrings {
    /// A music video of a song, as a row's extra word.
    pub video: &'static str,
    /// A row this player cannot play.
    pub unavailable: &'static str,
    /// A song already had from a service (no file of its own).
    pub yours: &'static str,
    pub ban_artist: &'static str,
    pub download_album: &'static str,
    pub album_radio: &'static str,
    pub to_playlist: &'static str,
    pub go_artist: &'static str,
    pub open_in_library: &'static str,
    pub share: &'static str,
    pub play_mix: &'static str,
    pub artist_radio: &'static str,
    pub playlist_radio: &'static str,
    pub to_library: &'static str,
    pub in_library: &'static str,
    pub releases: PluralForms,
    /// A page's overline: release type, playlist, artist.
    pub overline_album: &'static str,
    pub overline_single: &'static str,
    pub overline_compilation: &'static str,
    pub overline_playlist: &'static str,
    pub overline_artist: &'static str,
    pub failed_open: &'static str,
    /// The album page's download button states.
    pub downloaded: &'static str,
    pub downloading: &'static str,
    pub not_found: &'static str,
    pub waits_torrents: &'static str,
    pub queued: &'static str,
    pub album_to_wishlist: &'static str,
    pub open_wishlist: &'static str,
    pub followed_tip: &'static str,
    pub follow_tip: &'static str,
    pub playlist_empty: &'static str,
    pub radio: &'static str,
    pub subscribe: &'static str,
    pub subscribed: &'static str,
    pub newest_release: &'static str,
    pub artist_radio_tip: &'static str,
    pub albums: &'static str,
    pub album: &'static str,
    pub singles: &'static str,
    pub no_releases: &'static str,
    /// Worlds: the home card and a world's page.
    pub listening_now: &'static str,
    pub now_playing: &'static str,
    pub open_world: &'static str,
    pub hide_world: &'static str,
    pub hide_card: &'static str,
    pub prev_worlds: &'static str,
    pub next_worlds: &'static str,
    pub mixes: &'static str,
    pub new: &'static str,
    pub favorite: &'static str,
}

pub static RU: StreamsUiStrings = StreamsUiStrings {
    video: "клип",
    unavailable: "недоступен",
    yours: "у тебя",
    ban_artist: "Не рекомендовать исполнителя",
    download_album: "Скачать альбом",
    album_radio: "Радио по альбому",
    to_playlist: "В плейлист…",
    go_artist: "К исполнителю",
    open_in_library: "Открыть в библиотеке",
    share: "Поделиться",
    play_mix: "Слушать микс",
    artist_radio: "Радио исполнителя",
    playlist_radio: "Радио по плейлисту",
    to_library: "В библиотеку",
    in_library: "В библиотеке",
    releases: PluralForms { one: "релиз", few: "релиза", many: "релизов" },
    overline_album: "АЛЬБОМ",
    overline_single: "СИНГЛ",
    overline_compilation: "СБОРНИК",
    overline_playlist: "ПЛЕЙЛИСТ",
    overline_artist: "ИСПОЛНИТЕЛЬ",
    failed_open: "Не открылось",
    downloaded: "Скачан",
    downloading: "Качается",
    not_found: "Не нашлось",
    waits_torrents: "Ждёт торренты",
    queued: "В очереди",
    album_to_wishlist: "Весь альбом — в «Хочу скачать» (без лайка)",
    open_wishlist: "Открыть «Хочу скачать»",
    followed_tip: "Плейлист в библиотеке и следит за сервисом — открыть",
    follow_tip: "Плейлист в библиотеку: он будет следить за сервисом",
    playlist_empty: "Плейлист пуст",
    radio: "Радио",
    subscribe: "Подписаться",
    subscribed: "В подписках",
    newest_release: "Самый новый релиз",
    artist_radio_tip: "Его треки и похожее — очередь дописывается сама",
    albums: "Альбомы",
    album: "Альбом",
    singles: "Синглы и EP",
    no_releases: "Релизов нет",
    listening_now: "Слушаешь сейчас",
    now_playing: "Сейчас играет",
    open_world: "Открыть мир",
    hide_world: "Убрать из ряда",
    hide_card: "Скрыть карточку",
    prev_worlds: "Предыдущие миры",
    next_worlds: "Следующие миры",
    mixes: "Миксы",
    new: "Новое",
    favorite: "Любимое",
};

pub static EN: StreamsUiStrings = StreamsUiStrings {
    video: "video",
    unavailable: "unavailable",
    yours: "on disk",
    ban_artist: "Don't recommend artist",
    download_album: "Download album",
    album_radio: "Album radio",
    to_playlist: "Add to playlist…",
    go_artist: "Go to artist",
    open_in_library: "Open in library",
    share: "Share",
    play_mix: "Play mix",
    artist_radio: "Artist radio",
    playlist_radio: "Playlist radio",
    to_library: "Add to library",
    in_library: "In your library",
    releases: PluralForms { one: "release", few: "releases", many: "releases" },
    overline_album: "Album",
    overline_single: "Single",
    overline_compilation: "Compilation",
    overline_playlist: "Playlist",
    overline_artist: "Artist",
    failed_open: "Couldn't open",
    downloaded: "Downloaded",
    downloading: "Downloading",
    not_found: "Not found",
    waits_torrents: "Waiting for torrents",
    queued: "Queued",
    album_to_wishlist: "Add full album to Wishlist without liking",
    open_wishlist: "Open Wishlist",
    followed_tip: "In your library and synced with the service. Select to open.",
    follow_tip: "Add to your library and keep in sync with the service.",
    playlist_empty: "Playlist is empty",
    radio: "Radio",
    subscribe: "Follow",
    subscribed: "Following",
    newest_release: "Newest release",
    artist_radio_tip: "Their tracks and similar music — the queue keeps growing.",
    albums: "Albums",
    album: "Album",
    singles: "Singles and EPs",
    no_releases: "No releases",
    listening_now: "Listening now",
    now_playing: "Now playing",
    open_world: "Open world",
    hide_world: "Remove from row",
    hide_card: "Hide card",
    prev_worlds: "Previous worlds",
    next_worlds: "Next worlds",
    mixes: "Mixes",
    new: "New",
    favorite: "Favorites",
};

locale_table! {
    /// Returns these strings for `loc`.
    streams_ui_in -> StreamsUiStrings,
    /// Returns these strings for the active process locale.
    streams_ui,
    { Ru => RU, En => EN }
}

/// A song already had from a service, on album `a`: «у тебя в «Blueprint»».
pub fn fmt_yours_in(loc: Locale, a: &str) -> String {
    match loc {
        Locale::Ru => format!("у тебя в «{a}»"),
        Locale::En => format!("on disk in “{a}”"),
    }
}

/// «{n} релизов» of an artist's page line.
pub fn releases_count_in(loc: Locale, n: u64) -> String {
    format!("{n} {}", plural_forms(loc, n, &streams_ui_in(loc).releases))
}

/// The artist page line's «{n} треков у тебя».
pub fn have_tracks_in(loc: Locale, n: u64) -> String {
    match loc {
        Locale::Ru => format!("{} у тебя", super::tracks_count_in(loc, n)),
        Locale::En => format!("{} on disk", super::tracks_count_in(loc, n)),
    }
}

/// The chip of the album's library copy: «В библиотеке: Ugly Is Beautiful».
pub fn in_library_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("В библиотеке: {title}"),
        Locale::En => format!("In your library: {title}"),
    }
}

/// [`in_library_in`] with how much of it is here: «A · 13 из 14».
pub fn in_library_of_in(loc: Locale, title: &str, local: u32, total: u32) -> String {
    match loc {
        Locale::Ru => format!("В библиотеке: {title} · {local} из {total}"),
        Locale::En => format!("In your library: {title} · {local} of {total}"),
    }
}

/// «Ты слышал 36 из 134» of the worlds card.
pub fn heard_line_in(loc: Locale, heard: u32, total: u32) -> String {
    match (loc, heard) {
        (Locale::Ru, 0) => format!("Ты ещё не слышал ни одного из {total}"),
        (Locale::En, 0) => format!("Not heard yet · {total} total"),
        _ if heard >= total => match loc {
            Locale::Ru => format!("Ты слышал все {total}"),
            Locale::En => format!("All heard · {total}"),
        },
        _ => match loc {
            Locale::Ru => format!("Ты слышал {heard} из {total}"),
            Locale::En => format!("You've heard {heard} of {total}"),
        },
    }
}

/// [`heard_line_in`] in the active locale.
pub fn heard_line(heard: u32, total: u32) -> String {
    heard_line_in(locale(), heard, total)
}

/// «слышал 5 из 52» under a world's album card.
pub fn album_tail_in(loc: Locale, heard: u32, total: u32) -> String {
    match (loc, heard) {
        (Locale::Ru, 0) => "не слушал".to_owned(),
        (Locale::En, 0) => "not heard".to_owned(),
        _ if heard >= total => match loc {
            Locale::Ru => format!("слышал все {total}"),
            Locale::En => format!("all heard · {total}"),
        },
        _ => match loc {
            Locale::Ru => format!("слышал {heard} из {total}"),
            Locale::En => format!("heard {heard} of {total}"),
        },
    }
}

/// [`album_tail_in`] in the active locale.
pub fn album_tail(heard: u32, total: u32) -> String {
    album_tail_in(locale(), heard, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn how_far_the_owner_got_says_itself_in_both_languages() {
        assert_eq!(heard_line_in(Locale::Ru, 36, 134), "Ты слышал 36 из 134");
        assert_eq!(heard_line_in(Locale::Ru, 0, 160), "Ты ещё не слышал ни одного из 160");
        assert_eq!(heard_line_in(Locale::Ru, 40, 40), "Ты слышал все 40");
        assert_eq!(heard_line_in(Locale::En, 36, 134), "You've heard 36 of 134");
        assert_eq!(heard_line_in(Locale::En, 0, 160), "Not heard yet · 160 total");
        assert_eq!(heard_line_in(Locale::En, 1, 1), "All heard · 1");
        assert_eq!(album_tail_in(Locale::Ru, 5, 52), "слышал 5 из 52");
        assert_eq!(album_tail_in(Locale::Ru, 0, 30), "не слушал");
        assert_eq!(album_tail_in(Locale::Ru, 20, 20), "слышал все 20");
        assert_eq!(album_tail_in(Locale::En, 20, 20), "all heard · 20");
    }

    #[test]
    fn a_library_copy_and_an_artists_releases_say_themselves() {
        assert_eq!(fmt_yours_in(Locale::Ru, "Blueprint"), "у тебя в «Blueprint»");
        assert_eq!(fmt_yours_in(Locale::En, "Blueprint"), "on disk in “Blueprint”");
        assert_eq!(in_library_of_in(Locale::Ru, "Ugly Is Beautiful", 13, 14), "В библиотеке: Ugly Is Beautiful · 13 из 14");
        assert_eq!(in_library_in(Locale::En, "Ugly Is Beautiful"), "In your library: Ugly Is Beautiful");
        assert_eq!(releases_count_in(Locale::Ru, 5), "5 релизов");
        assert_eq!(releases_count_in(Locale::Ru, 21), "21 релиз");
        assert_eq!(releases_count_in(Locale::En, 2), "2 releases");
        assert_eq!(have_tracks_in(Locale::Ru, 34), "34 трека у тебя");
        assert_eq!(have_tracks_in(Locale::En, 34), "34 tracks on disk");
    }
}
