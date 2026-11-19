//! Screens, panes and dialogs: now playing, queue pane, sweep, find song, tag editor, output, sleep timer.

use super::{Locale, PluralForms, common_in, locale};

/// Static strings: screens, panes and dialogs: now playing, queue pane, sweep, find song, tag editor, output, sleep timer.
pub struct ScreensStrings {
    // The supporting pane's tabs.
    pub pane_lyrics_tab: &'static str,
    pub pane_info_tab: &'static str,
    pub close_esc: &'static str,
    // The queue pane.
    pub remove_from_queue: &'static str,
    pub played_section: &'static str,
    pub next_section: &'static str,
    pub nothing_next: &'static str,
    pub autoplay: &'static str,
    pub autoplay_on_hover: &'static str,
    pub autoplay_off_hover: &'static str,
    pub autoplay_running: &'static str,
    pub radio_running: &'static str,
    pub configure_autoplay: &'static str,
    pub wave_hover: &'static str,
    pub picking_more: &'static str,
    // The track's info.
    pub info_title: &'static str,
    pub source_row: &'static str,
    pub quality: &'static str,
    pub file_row: &'static str,
    pub show_in_folder: &'static str,
    pub copy_path: &'static str,
    pub edit_tags: &'static str,
    pub download: &'static str,
    // «Найти песню…» / «Другая версия…».
    pub find_song_hint: &'static str,
    pub other_version_hint: &'static str,
    pub find_song_title: &'static str,
    pub other_version_title: &'static str,
    pub find_song_context: &'static str,
    pub clip_cap: &'static str,
    pub clip_word: &'static str,
    pub current_word: &'static str,
    pub where_replace: &'static str,
    pub scope_here: &'static str,
    pub scope_youtube: &'static str,
    pub hint_clip_youtube: &'static str,
    pub hint_song_youtube: &'static str,
    pub hint_clip_here: &'static str,
    pub hint_song_here: &'static str,
    pub replacing: &'static str,
    pub replace: &'static str,
    pub searching_songs: &'static str,
    pub write_query: &'static str,
    pub nothing_found: &'static str,
    pub preview_tip: &'static str,
    pub pause_tip: &'static str,
    // The tag editor.
    pub tags_title: &'static str,
    pub tag_album_artist: &'static str,
    pub tag_year: &'static str,
    pub tag_track_no: &'static str,
    pub tag_disc_no: &'static str,
    pub tag_genre: &'static str,
    pub tag_lookup: &'static str,
    pub tag_suggestions: &'static str,
    // Menus, the save-queue dialog, the output popover.
    pub menu_visualizer: &'static str,
    pub menu_trash: &'static str,
    pub menu_youtube: &'static str,
    pub play_where: &'static str,
    pub sound_window: &'static str,
    pub save_queue_title: &'static str,
    pub enter_name: &'static str,
    pub mute_tooltip: &'static str,
    pub system_device: &'static str,
    pub this_computer: &'static str,
    pub output_devices: &'static str,
    pub move_queue_here: &'static str,
    pub move_here: &'static str,
    pub guest: &'static str,
    pub network_map: &'static str,
    // The sleep timer.
    pub sleep_timer: &'static str,
    pub sleep_off: &'static str,
    pub sleep_track_end: &'static str,
    pub sleep_album_end: &'static str,
    pub sleep_playlist_end: &'static str,
    pub sleep_queue_end: &'static str,
    pub hours: PluralForms,
    pub minutes: PluralForms,
    // The sound quality section.
    pub real_band: &'static str,
    // The visualizer.
    pub viz_fullscreen: &'static str,
    pub viz_close: &'static str,
}

pub static RU: ScreensStrings = ScreensStrings {
    pane_lyrics_tab: "Текст",
    pane_info_tab: "Инфо",
    close_esc: "Закрыть (Esc)",
    remove_from_queue: "Убрать из очереди",
    played_section: "Ранее",
    next_section: "Далее",
    nothing_next: "Дальше ничего нет",
    autoplay: "Автовоспроизведение",
    autoplay_on_hover: "Когда очередь кончится, она продолжится похожим",
    autoplay_off_hover: "Очередь закончится на последнем треке",
    autoplay_running: "Автовоспроизведение: дописывается, пока играет",
    radio_running: "Дописывается сама, пока играет",
    configure_autoplay: "Настроить автовоспроизведение…",
    wave_hover: "Подстраивается под тебя, пока играет",
    picking_more: "Подбираю ещё…",
    info_title: "Источник и качество",
    source_row: "Источник",
    quality: "Качество",
    file_row: "Файл",
    show_in_folder: "Показать в папке",
    copy_path: "Скопировать путь",
    edit_tags: "Редактировать теги…",
    download: "Скачать",
    find_song_hint: "Найти песню…",
    other_version_hint: "Другая версия…",
    find_song_title: "Найти песню",
    other_version_title: "Другая версия",
    find_song_context: "Найти песню",
    clip_cap: "Клип",
    clip_word: "клип",
    current_word: "Сейчас",
    where_replace: "Где заменить",
    scope_here: "Только здесь",
    scope_youtube: "И в YouTube Music",
    hint_clip_youtube: "Клип уйдёт из лайков YouTube Music, лайк получит песня",
    hint_song_youtube: "Лайк в YouTube Music перейдёт на эту песню",
    hint_clip_here: "Лайк в YouTube Music останется на клипе",
    hint_song_here: "В YouTube Music ничего не изменится",
    replacing: "Заменяю…",
    replace: "Заменить",
    searching_songs: "Ищу песни…",
    write_query: "Напиши название и исполнителя",
    nothing_found: "Ничего не нашлось — попробуй иначе",
    preview_tip: "Послушать",
    pause_tip: "Пауза",
    tags_title: "Теги",
    tag_album_artist: "Исполнитель альбома",
    tag_year: "Год",
    tag_track_no: "Трек №",
    tag_disc_no: "Диск №",
    tag_genre: "Жанр",
    tag_lookup: "Найти в MusicBrainz",
    tag_suggestions: "Варианты из MusicBrainz",
    menu_visualizer: "Визуализатор",
    menu_trash: "Удалить из библиотеки",
    menu_youtube: "Включить с YouTube",
    play_where: "Где играть",
    sound_window: "Скорость и эквалайзер",
    save_queue_title: "Сохранить очередь",
    enter_name: "Введи название",
    mute_tooltip: "Без звука",
    system_device: "Системное по умолчанию",
    this_computer: "Этот компьютер",
    output_devices: "Устройства вывода",
    move_queue_here: "Перенести мою очередь сюда",
    move_here: "Перенести сюда",
    guest: "Гость",
    network_map: "Карта сети",
    sleep_timer: "Таймер сна",
    sleep_off: "Выключить таймер",
    sleep_track_end: "До конца трека",
    sleep_album_end: "До конца альбома",
    sleep_playlist_end: "До конца плейлиста",
    sleep_queue_end: "До конца очереди",
    hours: PluralForms { one: "час", few: "часа", many: "часов" },
    minutes: PluralForms { one: "минута", few: "минуты", many: "минут" },
    real_band: "Реальная полоса",
    viz_fullscreen: "Во весь экран (F11)",
    viz_close: "Закрыть (Esc, F12)",
};

pub static EN: ScreensStrings = ScreensStrings {
    pane_lyrics_tab: "Lyrics",
    pane_info_tab: "Info",
    close_esc: "Close (Esc)",
    remove_from_queue: "Remove from queue",
    played_section: "Earlier",
    next_section: "Up next",
    nothing_next: "Nothing up next",
    autoplay: "Autoplay",
    autoplay_on_hover: "When the queue ends, it continues with similar tracks",
    autoplay_off_hover: "The queue ends with the last track",
    autoplay_running: "Autoplay: keeps adding tracks while playing",
    radio_running: "Keeps adding tracks on its own while playing",
    configure_autoplay: "Configure autoplay…",
    wave_hover: "Adapts to you while playing",
    picking_more: "Picking more…",
    info_title: "Source and quality",
    source_row: "Source",
    quality: "Quality",
    file_row: "File",
    show_in_folder: "Show in folder",
    copy_path: "Copy path",
    edit_tags: "Edit tags…",
    download: "Download",
    find_song_hint: "Find song…",
    other_version_hint: "Another version…",
    find_song_title: "Find song",
    other_version_title: "Another version",
    find_song_context: "Find song",
    clip_cap: "Video",
    clip_word: "video",
    current_word: "Current",
    where_replace: "Where to replace",
    scope_here: "Only here",
    scope_youtube: "And in YouTube Music",
    hint_clip_youtube: "The video leaves your YouTube Music likes; the song gets the like",
    hint_song_youtube: "The YouTube Music like moves to this song",
    hint_clip_here: "The YouTube Music like stays on the video",
    hint_song_here: "Nothing changes in YouTube Music",
    replacing: "Replacing…",
    replace: "Replace",
    searching_songs: "Searching for songs…",
    write_query: "Type the song's title and artist",
    nothing_found: "Nothing found — try a different search",
    preview_tip: "Play preview",
    pause_tip: "Pause",
    tags_title: "Tags",
    tag_album_artist: "Album artist",
    tag_year: "Year",
    tag_track_no: "Track #",
    tag_disc_no: "Disc #",
    tag_genre: "Genre",
    tag_lookup: "Look up on MusicBrainz",
    tag_suggestions: "Suggestions from MusicBrainz",
    menu_visualizer: "Visualizer",
    menu_trash: "Remove from library",
    menu_youtube: "Play from YouTube",
    play_where: "Play on device",
    sound_window: "Speed and equalizer",
    save_queue_title: "Save queue",
    enter_name: "Enter a name",
    mute_tooltip: "Mute",
    system_device: "System default",
    this_computer: "This computer",
    output_devices: "Output devices",
    move_queue_here: "Move my queue here",
    move_here: "Move here",
    guest: "Guest",
    network_map: "Network map",
    sleep_timer: "Sleep timer",
    sleep_off: "Turn off timer",
    sleep_track_end: "When the track ends",
    sleep_album_end: "When the album ends",
    sleep_playlist_end: "When the playlist ends",
    sleep_queue_end: "When the queue ends",
    hours: PluralForms { one: "hour", few: "hours", many: "hours" },
    minutes: PluralForms { one: "minute", few: "minutes", many: "minutes" },
    real_band: "Actual bandwidth",
    viz_fullscreen: "Fullscreen (F11)",
    viz_close: "Close (Esc, F12)",
};

locale_table! {
    /// Returns these strings for `loc`.
    screens_in -> ScreensStrings,
    /// Returns these strings for the active process locale.
    screens,
    { Ru => RU, En => EN }
}

/// The four visualizer pictures' names for `loc`, in `VizConcept::ALL` order.
pub const fn viz_names_in(loc: Locale) -> [&'static str; 4] {
    match loc {
        Locale::Ru => ["Обложка", "Спектр", "Фигуры", "Спокойно"],
        Locale::En => ["Cover", "Spectrum", "Shapes", "Calm"],
    }
}

/// The four visualizer pictures' names in the active locale.
pub fn viz_names() -> [&'static str; 4] {
    viz_names_in(locale())
}

/// Month names in the genitive («23 октября 2026») for `loc`.
pub const fn months_gen_in(loc: Locale) -> [&'static str; 12] {
    match loc {
        Locale::Ru => [
            "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря",
        ],
        Locale::En => [
            "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
        ],
    }
}

/// Month names in the nominative («Октябрь 2026») for `loc`.
pub const fn months_nom_in(loc: Locale) -> [&'static str; 12] {
    match loc {
        Locale::Ru => [
            "Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь", "Октябрь", "Ноябрь", "Декабрь",
        ],
        Locale::En => [
            "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
        ],
    }
}

/// Month names in the genitive in the active locale.
pub fn months_gen() -> [&'static str; 12] {
    months_gen_in(locale())
}

/// Month names in the nominative in the active locale.
pub fn months_nom() -> [&'static str; 12] {
    months_nom_in(locale())
}

/// Formats a half hour as a sleep choice (`"1,5 часа"` / `"1.5 hours"`).
pub fn fmt_half_hour_in(loc: Locale, h: u64) -> String {
    match loc {
        Locale::Ru => format!("{h},5 часа"),
        Locale::En => format!("{h}.5 hours"),
    }
}

/// Formats a half hour as a sleep choice in the active locale.
pub fn fmt_half_hour(h: u64) -> String {
    fmt_half_hour_in(locale(), h)
}

/// Formats the time left on a running sleep timer (`"45 с"`, `"2 мин"` / `"45 sec"`, `"2 min"`).
pub fn fmt_sleep_left_in(loc: Locale, secs: u64) -> String {
    let (sec, min) = match loc {
        Locale::Ru => ("с", "мин"),
        Locale::En => (common_in(loc).sec_short, common_in(loc).min_short),
    };
    if secs < 60 { format!("{secs} {sec}") } else { format!("{} {min}", secs.div_ceil(60)) }
}

/// Formats the time left on a running sleep timer in the active locale.
pub fn fmt_sleep_left(secs: u64) -> String {
    fmt_sleep_left_in(locale(), secs)
}

/// Formats the sleep chip's tooltip (`"Таймер сна: 2 мин"` / `"Sleep timer: 2 min"`).
pub fn fmt_sleep_tip_in(loc: Locale, left: &str) -> String {
    format!("{}: {}", screens_in(loc).sleep_timer, left)
}

/// Formats the sleep chip's tooltip in the active locale.
pub fn fmt_sleep_tip(left: &str) -> String {
    fmt_sleep_tip_in(locale(), left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::plural_forms;

    #[test]
    fn sleep_timer_formats_both_locales_with_plurals() {
        assert_eq!(fmt_sleep_left_in(Locale::Ru, 45), "45 с");
        assert_eq!(fmt_sleep_left_in(Locale::Ru, 61), "2 мин");
        assert_eq!(fmt_sleep_left_in(Locale::En, 45), "45 sec");
        assert_eq!(fmt_sleep_left_in(Locale::En, 61), "2 min");
        assert_eq!(fmt_half_hour_in(Locale::Ru, 1), "1,5 часа");
        assert_eq!(fmt_half_hour_in(Locale::En, 1), "1.5 hours");
        assert_eq!(fmt_sleep_tip_in(Locale::Ru, "2 мин"), "Таймер сна: 2 мин");
        assert_eq!(fmt_sleep_tip_in(Locale::En, "2 min"), "Sleep timer: 2 min");
        assert_eq!(plural_forms(Locale::Ru, 1, &RU.hours), "час");
        assert_eq!(plural_forms(Locale::Ru, 2, &RU.hours), "часа");
        assert_eq!(plural_forms(Locale::Ru, 5, &RU.hours), "часов");
        assert_eq!(plural_forms(Locale::En, 1, &EN.hours), "hour");
        assert_eq!(plural_forms(Locale::En, 2, &EN.hours), "hours");
        assert_eq!(plural_forms(Locale::Ru, 15, &RU.minutes), "минут");
        assert_eq!(plural_forms(Locale::En, 1, &EN.minutes), "minute");
        assert_eq!(plural_forms(Locale::En, 15, &EN.minutes), "minutes");
    }

    #[test]
    fn months_and_viz_names_cover_both_locales() {
        assert_eq!(months_gen_in(Locale::Ru)[0], "января");
        assert_eq!(months_gen_in(Locale::Ru)[9], "октября");
        assert_eq!(months_nom_in(Locale::Ru)[11], "Декабрь");
        assert_eq!(months_gen_in(Locale::En)[0], "January");
        assert_eq!(months_gen_in(Locale::En)[9], "October");
        assert_eq!(months_nom_in(Locale::En)[11], "December");
        assert_eq!(viz_names_in(Locale::Ru), ["Обложка", "Спектр", "Фигуры", "Спокойно"]);
        assert_eq!(viz_names_in(Locale::En), ["Cover", "Spectrum", "Shapes", "Calm"]);
    }
}
