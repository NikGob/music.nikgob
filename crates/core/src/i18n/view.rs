//! Labels of view-model enums (crates/view) and the app's one-off messages.

use std::time::Duration;

use super::{Locale, PluralForms, common_in, locale, plural_forms};

/// Static strings: labels of view-model enums (crates/view) and the app's one-off messages.
pub struct ViewStrings {
    // Worlds (crates/view lib.rs).
    pub world_soundtrack: &'static str,
    pub world_artist: &'static str,
    pub world_finish: &'static str,
    pub world_begin: &'static str,
    pub world_again: &'static str,
    // The track menu (crates/view lib.rs).
    pub menu_radio: &'static str,
    pub menu_album: &'static str,
    pub menu_artist: &'static str,
    pub menu_share: &'static str,
    pub menu_info: &'static str,
    pub menu_sound: &'static str,
    pub menu_visualizer: &'static str,
    pub menu_tags: &'static str,
    pub menu_lyrics: &'static str,
    pub menu_download: &'static str,
    pub menu_torrents: &'static str,
    pub menu_pick_release: &'static str,
    pub menu_wrong_file: &'static str,
    pub menu_dislike: &'static str,
    pub menu_ban: &'static str,
    // Home sections (crates/view lib.rs).
    pub home_shortcuts: &'static str,
    pub home_daily: &'static str,
    pub home_mixes: &'static str,
    pub home_listen_again: &'static str,
    pub home_of_day: &'static str,
    pub home_from_library: &'static str,
    pub home_new: &'static str,
    pub home_releases: &'static str,
    // Cover shapes (crates/view lib.rs).
    pub shape_original: &'static str,
    pub shape_random: &'static str,
    pub shape_cookie12: &'static str,
    pub shape_cookie9: &'static str,
    pub shape_cookie7: &'static str,
    pub shape_cookie6: &'static str,
    pub shape_cookie4: &'static str,
    pub shape_sunny: &'static str,
    pub shape_very_sunny: &'static str,
    pub shape_flower: &'static str,
    pub shape_clover4: &'static str,
    pub shape_clover8: &'static str,
    pub shape_soft_burst: &'static str,
    pub shape_soft_boom: &'static str,
    pub shape_puffy: &'static str,
    pub shape_puffy_diamond: &'static str,
    pub shape_circle: &'static str,
    pub shape_pentagon: &'static str,
    pub shape_gem: &'static str,
    pub shape_heart: &'static str,
    // Speed variants and their chips (crates/view sound.rs).
    pub speed_normal: &'static str,
    pub speed_fast: &'static str,
    pub speed_nightcore: &'static str,
    pub speed_slow: &'static str,
    pub speed_slow_echo: &'static str,
    pub chip_slow: &'static str,
    pub chip_slow_echo: &'static str,
    // Equalizer preset names (crates/view sound.rs).
    pub eq_flat: &'static str,
    pub eq_bass: &'static str,
    pub eq_bass_strong: &'static str,
    pub eq_vocal: &'static str,
    pub eq_bright: &'static str,
    pub eq_night: &'static str,
    pub eq_headphones: &'static str,
    pub eq_warm: &'static str,
    // Album-have badges (crates/view streams.rs).
    pub have_yours: &'static str,
    pub have_in_library: &'static str,
    pub songs: PluralForms,
    // What a better copy is looked for (crates/view streams.rs).
    pub upgrade_mp3_320: &'static str,
    pub upgrade_lossless: &'static str,
    // Settings presets (crates/view presets.rs).
    pub preset_custom: &'static str,
    pub preset_recs_0: &'static str,
    pub preset_recs_1: &'static str,
    pub preset_recs_2: &'static str,
    pub preset_lyrics_0: &'static str,
    pub preset_lyrics_1: &'static str,
    pub preset_lyrics_2: &'static str,
    pub preset_lyrics_3: &'static str,
    pub preset_track_0: &'static str,
    pub preset_track_1: &'static str,
    pub preset_track_2: &'static str,
    pub preset_sources_0: &'static str,
    pub preset_sources_1: &'static str,
    pub preset_seeding_0: &'static str,
    pub preset_seeding_1: &'static str,
    pub preset_seeding_2: &'static str,
    pub preset_load_0: &'static str,
    pub preset_load_1: &'static str,
    pub preset_load_2: &'static str,
    // Storage line (crates/view storage.rs).
    pub unit_b: &'static str,
    pub unit_kb: &'static str,
    pub unit_tb: &'static str,
    pub music_word: &'static str,
    pub library_word: &'static str,
    pub originals_word: &'static str,
    // The wave's choices (crates/view recs.rs).
    pub wave_balance: &'static str,
    pub wave_more: &'static str,
    pub wave_only_new: &'static str,
    pub wave_familiar: &'static str,
    pub wave_any: &'static str,
    pub wave_lively: &'static str,
    pub wave_calm: &'static str,
    pub wave_dark: &'static str,
    pub wave_background: &'static str,
    // The API's presets and actors (crates/view api.rs).
    pub api_preset_control: &'static str,
    pub api_preset_full: &'static str,
    pub api_preset_read: &'static str,
    pub api_actor_owner: &'static str,
    pub api_actor_assistant: &'static str,
    pub api_actor_agent: &'static str,
    pub api_actor_guest: &'static str,
    // The sign-in window (crates/app ytlogin.rs).
    pub sign_in_youtube: &'static str,
    pub sign_in_soundcloud: &'static str,
    pub no_webview2: &'static str,
    pub no_webkitgtk: &'static str,
    pub window_no_cookies: &'static str,
    pub window_crashed: &'static str,
}

pub static RU: ViewStrings = ViewStrings {
    world_soundtrack: "Саундтрек",
    world_artist: "Исполнитель",
    world_finish: "Дослушать",
    world_begin: "Начать",
    world_again: "С начала",
    menu_radio: "Радио по треку",
    menu_album: "К альбому",
    menu_artist: "К исполнителю",
    menu_share: "Поделиться",
    menu_info: "Источник и качество",
    menu_sound: "Скорость и эквалайзер",
    menu_visualizer: "Визуализатор",
    menu_tags: "Редактировать теги…",
    menu_lyrics: "Изменить текст…",
    menu_download: "Скачать",
    menu_torrents: "Найти качество лучше",
    menu_pick_release: "Выбрать раздачу…",
    menu_wrong_file: "В файле другая песня",
    menu_dislike: "Не нравится",
    menu_ban: "Не рекомендовать исполнителя",
    home_shortcuts: "Быстрый доступ",
    home_daily: "Миксы дня",
    home_mixes: "Миксы для тебя",
    home_listen_again: "Слушать снова",
    home_of_day: "Плитки дня",
    home_from_library: "Из библиотеки",
    home_new: "Новое",
    home_releases: "Релизы",
    shape_original: "Оригинал (квадрат)",
    shape_random: "Случайная",
    shape_cookie12: "Печенье, 12 волн",
    shape_cookie9: "Печенье, 9 волн",
    shape_cookie7: "Печенье, 7 волн",
    shape_cookie6: "Печенье, 6 волн",
    shape_cookie4: "Печенье, 4 волны",
    shape_sunny: "Солнце",
    shape_very_sunny: "Яркое солнце",
    shape_flower: "Цветок",
    shape_clover4: "Клевер, 4 листа",
    shape_clover8: "Клевер, 8 листьев",
    shape_soft_burst: "Вспышка",
    shape_soft_boom: "Взрыв",
    shape_puffy: "Облако",
    shape_puffy_diamond: "Пухлый ромб",
    shape_circle: "Круг",
    shape_pentagon: "Пятиугольник",
    shape_gem: "Самоцвет",
    shape_heart: "Сердце",
    speed_normal: "Обычно",
    speed_fast: "Быстрее",
    speed_nightcore: "Nightcore",
    speed_slow: "Замедленно",
    speed_slow_echo: "Замедленно с эхом",
    chip_slow: "замедленно",
    chip_slow_echo: "замедленно · эхо",
    eq_flat: "Ровно",
    eq_bass: "Бас",
    eq_bass_strong: "Мощный бас",
    eq_vocal: "Вокал",
    eq_bright: "Яркость",
    eq_night: "Ночью",
    eq_headphones: "Наушники",
    eq_warm: "Тёплый",
    have_yours: "у тебя",
    have_in_library: "в библиотеке",
    songs: PluralForms { one: "песня", few: "песни", many: "песен" },
    upgrade_mp3_320: "MP3 320 или лучше",
    upgrade_lossless: "lossless",
    preset_custom: "Свои",
    preset_recs_0: "Только моё",
    preset_recs_1: "Сбалансировано",
    preset_recs_2: "Больше нового",
    preset_lyrics_0: "Караоке",
    preset_lyrics_1: "Спокойно",
    preset_lyrics_2: "Прыжок",
    preset_lyrics_3: "Чётко",
    preset_track_0: "Строго",
    preset_track_1: "Живо",
    preset_track_2: "Максимум",
    preset_sources_0: "Обычно",
    preset_sources_1: "Всё",
    preset_seeding_0: "Не раздавать",
    preset_seeding_1: "Обычно",
    preset_seeding_2: "Щедро",
    preset_load_0: "Тихо",
    preset_load_1: "Обычно",
    preset_load_2: "Быстро",
    unit_b: "Б",
    unit_kb: "КБ",
    unit_tb: "ТБ",
    music_word: "Музыка",
    library_word: "Библиотека",
    originals_word: "Оригиналы без потерь",
    wave_balance: "Баланс",
    wave_more: "Больше нового",
    wave_only_new: "Только новое",
    wave_familiar: "Знакомое",
    wave_any: "Любой",
    wave_lively: "Бодрое",
    wave_calm: "Спокойное",
    wave_dark: "Мрачное",
    wave_background: "Фон",
    api_preset_control: "Управление",
    api_preset_full: "Полный",
    api_preset_read: "Только чтение",
    api_actor_owner: "Я",
    api_actor_assistant: "Ассистент",
    api_actor_agent: "Агент",
    api_actor_guest: "Гость",
    sign_in_youtube: "Вход в YouTube Music",
    sign_in_soundcloud: "Вход в SoundCloud",
    no_webview2: "в системе нет WebView2",
    no_webkitgtk: "в системе нет WebKitGTK",
    window_no_cookies: "окно не отдало cookies",
    window_crashed: "окно упало",
};

pub static EN: ViewStrings = ViewStrings {
    world_soundtrack: "Soundtrack",
    world_artist: "Artist",
    world_finish: "Resume",
    world_begin: "Start",
    world_again: "Start over",
    menu_radio: "Track radio",
    menu_album: "Go to album",
    menu_artist: "Go to artist",
    menu_share: "Share",
    menu_info: "Source & quality",
    menu_sound: "Speed & equalizer",
    menu_visualizer: "Visualizer",
    menu_tags: "Edit tags…",
    menu_lyrics: "Edit lyrics…",
    menu_download: "Download",
    menu_torrents: "Find better quality",
    menu_pick_release: "Choose torrent…",
    menu_wrong_file: "Different song in file",
    menu_dislike: "Dislike",
    menu_ban: "Don't recommend artist",
    home_shortcuts: "Shortcuts",
    home_daily: "Daily mixes",
    home_mixes: "Mixes for you",
    home_listen_again: "Listen again",
    home_of_day: "Tiles of the day",
    home_from_library: "From your library",
    home_new: "New",
    home_releases: "Releases",
    shape_original: "Original (square)",
    shape_random: "Random",
    shape_cookie12: "Cookie, 12 waves",
    shape_cookie9: "Cookie, 9 waves",
    shape_cookie7: "Cookie, 7 waves",
    shape_cookie6: "Cookie, 6 waves",
    shape_cookie4: "Cookie, 4 waves",
    shape_sunny: "Sun",
    shape_very_sunny: "Bright sun",
    shape_flower: "Flower",
    shape_clover4: "Clover, 4 leaves",
    shape_clover8: "Clover, 8 leaves",
    shape_soft_burst: "Burst",
    shape_soft_boom: "Boom",
    shape_puffy: "Cloud",
    shape_puffy_diamond: "Puffy diamond",
    shape_circle: "Circle",
    shape_pentagon: "Pentagon",
    shape_gem: "Gem",
    shape_heart: "Heart",
    speed_normal: "Normal",
    speed_fast: "Faster",
    speed_nightcore: "Nightcore",
    speed_slow: "Slowed",
    speed_slow_echo: "Slowed + echo",
    chip_slow: "slowed",
    chip_slow_echo: "slowed · echo",
    eq_flat: "Flat",
    eq_bass: "Bass",
    eq_bass_strong: "Strong bass",
    eq_vocal: "Vocals",
    eq_bright: "Bright",
    eq_night: "Night",
    eq_headphones: "Headphones",
    eq_warm: "Warm",
    have_yours: "Yours",
    have_in_library: "In library",
    songs: PluralForms { one: "song", few: "songs", many: "songs" },
    upgrade_mp3_320: "MP3 320 or better",
    upgrade_lossless: "Lossless",
    preset_custom: "Custom",
    preset_recs_0: "Only mine",
    preset_recs_1: "Balanced",
    preset_recs_2: "More new",
    preset_lyrics_0: "Karaoke",
    preset_lyrics_1: "Calm",
    preset_lyrics_2: "Bounce",
    preset_lyrics_3: "Sharp",
    preset_track_0: "Strict",
    preset_track_1: "Lively",
    preset_track_2: "Maximum",
    preset_sources_0: "Usual",
    preset_sources_1: "All",
    preset_seeding_0: "Don't seed",
    preset_seeding_1: "Usual",
    preset_seeding_2: "Generous",
    preset_load_0: "Quiet",
    preset_load_1: "Usual",
    preset_load_2: "Fast",
    unit_b: "B",
    unit_kb: "KB",
    unit_tb: "TB",
    music_word: "Music",
    library_word: "Library",
    originals_word: "Lossless originals",
    wave_balance: "Balance",
    wave_more: "More new",
    wave_only_new: "Only new",
    wave_familiar: "Familiar",
    wave_any: "Any",
    wave_lively: "Lively",
    wave_calm: "Calm",
    wave_dark: "Dark",
    wave_background: "Background",
    api_preset_control: "Control",
    api_preset_full: "Full",
    api_preset_read: "Read-only",
    api_actor_owner: "Me",
    api_actor_assistant: "Assistant",
    api_actor_agent: "Agent",
    api_actor_guest: "Guest",
    sign_in_youtube: "Sign in to YouTube Music",
    sign_in_soundcloud: "Sign in to SoundCloud",
    no_webview2: "no WebView2 in the system",
    no_webkitgtk: "no WebKitGTK in the system",
    window_no_cookies: "the window gave no cookies",
    window_crashed: "the window crashed",
};

locale_table! {
    /// Returns these strings for `loc`.
    view_in -> ViewStrings,
    /// Returns these strings for the active process locale.
    view,
    { Ru => RU, En => EN }
}

/// `n` songs of an album that are the user's: «2 песни у тебя» / "2 songs of yours".
pub fn fmt_songs_yours_in(loc: Locale, n: u32) -> String {
    let unit = plural_forms(loc, u64::from(n), &view_in(loc).songs);
    match loc {
        Locale::Ru => format!("{n} {unit} у тебя"),
        Locale::En => format!("{n} {unit} of yours"),
    }
}

/// How much of an album is local: «3 из 12» / "3 of 12".
pub fn fmt_have_of_in(loc: Locale, n: u32, total: u32) -> String {
    match loc {
        Locale::Ru => format!("{n} из {total}"),
        Locale::En => format!("{n} of {total}"),
    }
}

/// How much of an album is local, as the file chip says: «3 из 12 у тебя» / "3 of 12 yours".
pub fn fmt_have_of_yours_in(loc: Locale, n: u32, total: u32) -> String {
    match loc {
        Locale::Ru => format!("{n} из {total} у тебя"),
        Locale::En => format!("{n} of {total} yours"),
    }
}

/// A size in binary units as `loc` writes it: «41,2 ГБ» / "41.2 GB" (a decimal below 100).
pub fn fmt_size_in(loc: Locale, bytes: u64) -> String {
    let (v, i) = scaled(bytes);
    let decimals = if i == 0 || v >= 100.0 { 0 } else { 1 };
    format!("{} {}", number_in(loc, v, decimals), unit_in(loc, i))
}

/// Free space, rounder: «62 ГБ» / "62 GB" (a decimal below 10).
pub fn fmt_free_size_in(loc: Locale, bytes: u64) -> String {
    let (v, i) = scaled(bytes);
    let decimals = if i == 0 || v >= 10.0 { 0 } else { 1 };
    format!("{} {}", number_in(loc, v, decimals), unit_in(loc, i))
}

/// The storage line's head: «Музыка 41,2 ГБ» / "Music 41.2 GB".
pub fn fmt_music_line_in(loc: Locale, bytes: u64) -> String {
    format!("{} {}", view_in(loc).music_word, fmt_size_in(loc, bytes))
}

/// One disk's free space in the line: «свободно 62 ГБ» / "62 GB free".
pub fn fmt_free_phrase_in(loc: Locale, bytes: u64) -> String {
    match loc {
        Locale::Ru => format!("свободно {}", fmt_free_size_in(loc, bytes)),
        Locale::En => format!("{} free", fmt_free_size_in(loc, bytes)),
    }
}

/// The first disk's free space with its name: «свободно 62 ГБ (C:)» / "62 GB free (C:)".
pub fn fmt_free_disk_in(loc: Locale, bytes: u64, name: &str) -> String {
    match loc {
        Locale::Ru => format!("свободно {} ({name})", fmt_free_size_in(loc, bytes)),
        Locale::En => format!("{} free ({name})", fmt_free_size_in(loc, bytes)),
    }
}

/// The hover's line of the library: «Библиотека: 30 ГБ» / "Library: 30 GB".
pub fn fmt_library_line_in(loc: Locale, bytes: u64) -> String {
    format!("{}: {}", view_in(loc).library_word, fmt_size_in(loc, bytes))
}

/// The hover's line of the masters: «Оригиналы без потерь: 11,2 ГБ» / "Lossless originals: 11.2 GB".
pub fn fmt_originals_line_in(loc: Locale, bytes: u64) -> String {
    format!("{}: {}", view_in(loc).originals_word, fmt_size_in(loc, bytes))
}

/// The hover's line of one disk in full: «C:: свободно 62 ГБ из 237 ГБ» / "C:: 62 GB free of 237 GB".
pub fn fmt_disk_full_in(loc: Locale, name: &str, free: u64, total: u64) -> String {
    match loc {
        Locale::Ru => format!("{name}: свободно {} из {}", fmt_size_in(loc, free), fmt_size_in(loc, total)),
        Locale::En => format!("{name}: {} free of {}", fmt_size_in(loc, free), fmt_size_in(loc, total)),
    }
}

/// What the sleep timer's chip says is left: «45 с» / «2 мин» ("45 sec" / "2 min").
pub fn fmt_sleep_left_in(loc: Locale, left: Duration) -> String {
    let c = common_in(loc);
    let secs = left.as_secs();
    if secs < 60 {
        // The chip is tight: «45 с» in Russian.
        let sec = if loc == Locale::Ru { "с" } else { c.sec_short };
        format!("{secs} {sec}")
    } else {
        format!("{} {}", secs.div_ceil(60), c.min_short)
    }
}

/// The sign-in window could not read the cookies file: «cookies не читаются: …».
pub fn fmt_cookies_unreadable_in(loc: Locale, err: &str) -> String {
    match loc {
        Locale::Ru => format!("cookies не читаются: {err}"),
        Locale::En => format!("cookies could not be read: {err}"),
    }
}

/// The sign-in window exited with a code of its own: «ошибка 7» / "error 7".
pub fn fmt_window_error_in(loc: Locale, code: i32) -> String {
    match loc {
        Locale::Ru => format!("ошибка {code}"),
        Locale::En => format!("error {code}"),
    }
}

/// `bytes` in binary units and the unit index.
fn scaled(bytes: u64) -> (f64, usize) {
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    (v, i)
}

/// The unit of index `i` in `loc`.
fn unit_in(loc: Locale, i: usize) -> &'static str {
    let v = view_in(loc);
    let c = common_in(loc);
    match i {
        0 => v.unit_b,
        1 => v.unit_kb,
        2 => c.mb,
        3 => c.gb,
        _ => v.unit_tb,
    }
}

/// With `loc`'s decimal separator, `decimals` places, no «,0» / ".0".
fn number_in(loc: Locale, v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    match s.split_once('.') {
        Some((whole, frac)) if frac.bytes().all(|b| b == b'0') => whole.to_owned(),
        Some((whole, frac)) => match loc {
            Locale::Ru => format!("{whole},{frac}"),
            Locale::En => format!("{whole}.{frac}"),
        },
        None => s.clone(),
    }
}

/// Formats `n` songs of yours in the active locale.
pub fn fmt_songs_yours(n: u32) -> String {
    fmt_songs_yours_in(locale(), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn labels_exist_in_both_locales() {
        for loc in Locale::ALL {
            let v = view_in(loc);
            assert!(!v.menu_radio.is_empty() && !v.shape_heart.is_empty() && !v.preset_custom.is_empty());
            assert!(!v.wave_balance.is_empty() && !v.api_actor_owner.is_empty() && !v.sign_in_youtube.is_empty());
        }
        assert_eq!(view_in(Locale::Ru).menu_radio, "Радио по треку");
        assert_eq!(view_in(Locale::En).menu_radio, "Track radio");
        assert_eq!(view_in(Locale::Ru).preset_custom, "Свои");
        assert_eq!(view_in(Locale::En).preset_custom, "Custom");
        assert_eq!(view_in(Locale::En).speed_slow_echo, "Slowed + echo");
    }

    #[test]
    fn songs_yours_pluralizes() {
        assert_eq!(fmt_songs_yours_in(Locale::Ru, 1), "1 песня у тебя");
        assert_eq!(fmt_songs_yours_in(Locale::Ru, 2), "2 песни у тебя");
        assert_eq!(fmt_songs_yours_in(Locale::Ru, 5), "5 песен у тебя");
        assert_eq!(fmt_songs_yours_in(Locale::Ru, 21), "21 песня у тебя");
        assert_eq!(fmt_songs_yours_in(Locale::En, 1), "1 song of yours");
        assert_eq!(fmt_songs_yours_in(Locale::En, 3), "3 songs of yours");
        assert_eq!(fmt_have_of_in(Locale::Ru, 3, 12), "3 из 12");
        assert_eq!(fmt_have_of_in(Locale::En, 3, 12), "3 of 12");
        assert_eq!(fmt_have_of_yours_in(Locale::Ru, 3, 12), "3 из 12 у тебя");
        assert_eq!(fmt_have_of_yours_in(Locale::En, 3, 12), "3 of 12 yours");
    }

    #[test]
    fn sizes_read_the_locales_way() {
        assert_eq!(fmt_size_in(Locale::Ru, 0), "0 Б");
        assert_eq!(fmt_size_in(Locale::Ru, GB * 412 / 10), "41,2 ГБ");
        assert_eq!(fmt_size_in(Locale::Ru, GB * 3), "3 ГБ");
        assert_eq!(fmt_size_in(Locale::En, GB * 412 / 10), "41.2 GB");
        assert_eq!(fmt_size_in(Locale::En, 820 << 20), "820 MB");
        assert_eq!(fmt_size_in(Locale::En, GB * 1434), "1.4 TB");
        assert_eq!(fmt_free_size_in(Locale::Ru, GB * 75 / 10), "7,5 ГБ");
        assert_eq!(fmt_free_size_in(Locale::En, GB * 75 / 10), "7.5 GB");
        assert_eq!(fmt_music_line_in(Locale::Ru, GB * 412 / 10), "Музыка 41,2 ГБ");
        assert_eq!(fmt_music_line_in(Locale::En, GB * 412 / 10), "Music 41.2 GB");
        assert_eq!(fmt_free_phrase_in(Locale::Ru, GB * 62), "свободно 62 ГБ");
        assert_eq!(fmt_free_phrase_in(Locale::En, GB * 62), "62 GB free");
        assert_eq!(fmt_free_disk_in(Locale::Ru, GB * 62, "C:"), "свободно 62 ГБ (C:)");
        assert_eq!(fmt_free_disk_in(Locale::En, GB * 62, "C:"), "62 GB free (C:)");
        assert_eq!(fmt_library_line_in(Locale::Ru, GB * 30), "Библиотека: 30 ГБ");
        assert_eq!(fmt_originals_line_in(Locale::En, GB * 112 / 10), "Lossless originals: 11.2 GB");
        assert_eq!(fmt_disk_full_in(Locale::Ru, "C:", GB * 62, GB * 237), "C:: свободно 62 ГБ из 237 ГБ");
        assert_eq!(fmt_disk_full_in(Locale::En, "C:", GB * 62, GB * 237), "C:: 62 GB free of 237 GB");
    }

    #[test]
    fn sleep_left_reads_both_locales() {
        assert_eq!(fmt_sleep_left_in(Locale::Ru, Duration::from_secs(45)), "45 с");
        assert_eq!(fmt_sleep_left_in(Locale::Ru, Duration::from_secs(61)), "2 мин");
        assert_eq!(fmt_sleep_left_in(Locale::En, Duration::from_secs(45)), "45 sec");
        assert_eq!(fmt_sleep_left_in(Locale::En, Duration::from_secs(61)), "2 min");
    }

    #[test]
    fn window_messages_read_both_locales() {
        assert_eq!(fmt_cookies_unreadable_in(Locale::Ru, "x"), "cookies не читаются: x");
        assert_eq!(fmt_cookies_unreadable_in(Locale::En, "x"), "cookies could not be read: x");
        assert_eq!(fmt_window_error_in(Locale::Ru, 7), "ошибка 7");
        assert_eq!(fmt_window_error_in(Locale::En, 7), "error 7");
        assert_eq!(view_in(Locale::Ru).window_crashed, "окно упало");
        assert_eq!(view_in(Locale::En).window_crashed, "the window crashed");
    }
}
