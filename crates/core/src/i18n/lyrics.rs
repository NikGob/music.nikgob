//! Lyrics view, editor, sources and alignment strings.

use super::{Locale, common_in, locale};

/// Static strings for lyrics display, editing and alignment.
pub struct LyricsStrings {
    pub title: &'static str,
    pub not_found: &'static str,
    pub edit: &'static str,
    pub gpu: &'static str,
    // Source names (`LyricsSource::label_in` reads these).
    pub src_own: &'static str,
    pub src_file: &'static str,
    pub src_recognized: &'static str,
    // Sync kind names (`LyricsSync::label_in` reads these).
    pub sync_plain: &'static str,
    pub sync_line: &'static str,
    pub sync_word: &'static str,
    pub sync_syllable: &'static str,
    // Source names («Свой текст» etc. live on `LyricsSource::label_in`).
    pub name_file: &'static str,
    pub name_recognized: &'static str,
    pub kept: &'static str,
    pub searching: &'static str,
    pub no_text: &'static str,
    pub from_recognized: &'static str,
    pub from_file: &'static str,
    pub without_highlight: &'static str,
    pub state_loading: &'static str,
    pub state_missing: &'static str,
    pub state_mismatch: &'static str,
    // How the shown text is seen (segmented choices).
    pub show_word: &'static str,
    pub show_line: &'static str,
    pub show_plain: &'static str,
    // Making timing (menu rows, buttons, the deep variant).
    pub action_line: &'static str,
    pub action_word: &'static str,
    pub action_word_deep: &'static str,
    pub action_again: &'static str,
    pub action_again_button: &'static str,
    pub action_deep: &'static str,
    pub deep: &'static str,
    // Keeping the text.
    pub save_text: &'static str,
    pub save_song_text: &'static str,
    // The control's menu and the looks.
    pub auto: &'static str,
    pub restore_found: &'static str,
    pub search_again: &'static str,
    pub cancel_sync: &'static str,
    pub settings: &'static str,
    pub settings_menu: &'static str,
    pub craft_busy: &'static str,
    pub craft_save: &'static str,
    pub craft: &'static str,
    // Alignment stages.
    pub stage_download: &'static str,
    pub stage_decode: &'static str,
    pub stage_separate: &'static str,
    pub stage_listen: &'static str,
    pub stage_align: &'static str,
    pub stage_recognise: &'static str,
    // The editor.
    pub edit_title: &'static str,
    pub mark: &'static str,
    pub nudge_earlier: &'static str,
    pub nudge_later: &'static str,
    pub split: &'static str,
    pub merge: &'static str,
    pub add_line: &'static str,
    pub delete_line: &'static str,
}

pub static RU: LyricsStrings = LyricsStrings {
    title: "Текст песни",
    not_found: "Текст не найден",
    edit: "Изменить текст…",
    gpu: "Видеокарта",
    src_own: "Свой текст",
    src_file: "Файл",
    src_recognized: "Распознан",
    sync_plain: "без синхронизации",
    sync_line: "по строкам",
    sync_word: "по словам",
    sync_syllable: "по слогам",
    name_file: "Файл трека",
    name_recognized: "Расслышал плеер",
    kept: "Сохранённый текст",
    searching: "Ищу текст…",
    no_text: "Текста нет",
    from_recognized: "Текст расслышан плеером",
    from_file: "Текст из файла трека",
    without_highlight: "без подсветки",
    state_loading: "ищу…",
    state_missing: "нет текста",
    state_mismatch: "текст другой песни или версии",
    show_word: "По словам",
    show_line: "По строкам",
    show_plain: "Без подсветки",
    action_line: "Подсветить по строкам",
    action_word: "Подсветить по словам",
    action_word_deep: "Подсветить по словам глубоко",
    action_again: "Сделать подсветку заново",
    action_again_button: "Сделать заново",
    action_deep: "Сделать глубокую подсветку",
    deep: "Глубокая подсветка",
    save_text: "Сохранить текст",
    save_song_text: "Сохранить песню и текст",
    auto: "Автоматически",
    restore_found: "Вернуть найденный",
    search_again: "Искать текст заново",
    cancel_sync: "Отменить подсветку",
    settings: "Настройки текста",
    settings_menu: "Настройки текста…",
    craft_busy: "Подсветка делается…",
    craft_save: "Подсветка и сохранение…",
    craft: "Подсветка…",
    stage_download: "Скачиваю модели",
    stage_decode: "Читаю песню",
    stage_separate: "Отделяю голос",
    stage_listen: "Слушаю голос",
    stage_align: "Расставляю слова",
    stage_recognise: "Распознаю слова",
    edit_title: "Изменить текст",
    mark: "Отметить",
    nudge_earlier: "−0,1 с",
    nudge_later: "+0,1 с",
    split: "Разделить",
    merge: "Склеить",
    add_line: "Строка ниже",
    delete_line: "Удалить строку",
};

pub static EN: LyricsStrings = LyricsStrings {
    title: "Lyrics",
    not_found: "Lyrics not found",
    edit: "Edit lyrics…",
    gpu: "GPU",
    src_own: "Your lyrics",
    src_file: "File",
    src_recognized: "Recognized",
    sync_plain: "no sync",
    sync_line: "by line",
    sync_word: "by word",
    sync_syllable: "by syllable",
    name_file: "Track file",
    name_recognized: "Heard by the player",
    kept: "Saved lyrics",
    searching: "Searching for lyrics…",
    no_text: "No lyrics",
    from_recognized: "Lyrics recognized by the player",
    from_file: "Lyrics from the track's file",
    without_highlight: "no highlighting",
    state_loading: "searching…",
    state_missing: "no lyrics",
    state_mismatch: "lyrics of another song or version",
    show_word: "By word",
    show_line: "By line",
    show_plain: "No sync",
    action_line: "Sync by line",
    action_word: "Sync by word",
    action_word_deep: "Sync by word (deep)",
    action_again: "Re-sync lyrics",
    action_again_button: "Re-sync",
    action_deep: "Deep sync",
    deep: "Deep sync",
    save_text: "Save lyrics",
    save_song_text: "Save song and lyrics",
    auto: "Automatic",
    restore_found: "Restore found lyrics",
    search_again: "Search for lyrics again",
    cancel_sync: "Cancel sync",
    settings: "Lyrics settings",
    settings_menu: "Lyrics settings…",
    craft_busy: "Syncing…",
    craft_save: "Syncing and saving…",
    craft: "Sync…",
    stage_download: "Downloading models",
    stage_decode: "Reading the song",
    stage_separate: "Separating the vocal",
    stage_listen: "Listening to the vocal",
    stage_align: "Aligning the words",
    stage_recognise: "Recognizing the words",
    edit_title: "Edit lyrics",
    mark: "Mark",
    nudge_earlier: "−0.1 s",
    nudge_later: "+0.1 s",
    split: "Split",
    merge: "Merge",
    add_line: "Add line below",
    delete_line: "Delete line",
};

locale_table! {
    /// Returns lyrics strings for `loc`.
    lyrics_in -> LyricsStrings,
    /// Returns lyrics strings for the active process locale.
    lyrics,
    { Ru => RU, En => EN }
}

/// Formats how a text is timed (`"синхронизирован по словам"`, `"плеер
/// синхронизировал по слогам"` / `"synced by word"`, `"player-synced by
/// syllable"`); `by` is the [`crate::lyrics::LyricsSync::label_in`] of the kind.
pub fn fmt_timed_in(loc: Locale, by: &str, by_player: bool) -> String {
    match loc {
        Locale::Ru => {
            if by_player { format!("плеер синхронизировал {by}") } else { format!("синхронизирован {by}") }
        }
        Locale::En => {
            if by_player { format!("player-synced {by}") } else { format!("synced {by}") }
        }
    }
}

/// Formats how a text is timed in the active locale.
pub fn fmt_timed(by: &str, by_player: bool) -> String {
    fmt_timed_in(locale(), by, by_player)
}

/// Formats where the shown text came from (`"Текст с Genius"` / `"Lyrics from Genius"`).
pub fn fmt_from_source_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("Текст с {name}"),
        Locale::En => format!("Lyrics from {name}"),
    }
}

/// Formats where the shown text came from in the active locale.
pub fn fmt_from_source(name: &str) -> String {
    fmt_from_source_in(locale(), name)
}

/// Formats the model-download stage with what is done (`"Скачиваю модели · 296
/// из 709 МБ"` / `"Downloading models · 296 of 709 MB"`), or without it.
pub fn fmt_stage_download_in(loc: Locale, amount: Option<String>) -> String {
    let s = lyrics_in(loc).stage_download;
    amount.map_or_else(|| s.to_owned(), |a| format!("{s} · {a}"))
}

/// Formats the model-download stage in the active locale.
pub fn fmt_stage_download(amount: Option<String>) -> String {
    fmt_stage_download_in(locale(), amount)
}

/// Formats a byte amount as progress (`"296 из 709 МБ"`, `"0,4 из 1,8 ГБ"` /
/// `"296 of 709 MB"`, `"0.4 of 1.8 GB"`), both in the unit of the whole.
pub fn fmt_amount_in(loc: Locale, done: u64, total: u64) -> String {
    let c = common_in(loc);
    let of = match loc {
        Locale::Ru => "из",
        Locale::En => "of",
    };
    let mb = |b: u64| b as f64 / 1_048_576.0;
    if mb(total) >= 1000.0 {
        let gb = |b: u64| match loc {
            Locale::Ru => format!("{:.1}", mb(b) / 1024.0).replace('.', ","),
            Locale::En => format!("{:.1}", mb(b) / 1024.0),
        };
        format!("{} {of} {} {}", gb(done), gb(total), c.gb)
    } else {
        format!("{:.0} {of} {:.0} {}", mb(done), mb(total), c.mb)
    }
}

/// Formats a byte amount as progress in the active locale.
pub fn fmt_amount(done: u64, total: u64) -> String {
    fmt_amount_in(locale(), done, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_and_source_phrases_translate() {
        assert_eq!(fmt_timed_in(Locale::Ru, "по словам", false), "синхронизирован по словам");
        assert_eq!(fmt_timed_in(Locale::Ru, "по слогам", true), "плеер синхронизировал по слогам");
        assert_eq!(fmt_timed_in(Locale::En, "by word", false), "synced by word");
        assert_eq!(fmt_timed_in(Locale::En, "by syllable", true), "player-synced by syllable");
        assert_eq!(fmt_from_source_in(Locale::Ru, "Genius"), "Текст с Genius");
        assert_eq!(fmt_from_source_in(Locale::En, "Genius"), "Lyrics from Genius");
        assert_eq!(fmt_stage_download_in(Locale::Ru, None), "Скачиваю модели");
        assert_eq!(fmt_stage_download_in(Locale::En, Some("296 of 709 MB".to_owned())), "Downloading models · 296 of 709 MB");
    }

    #[test]
    fn download_amounts_stay_in_the_unit_of_the_whole() {
        assert_eq!(fmt_amount_in(Locale::Ru, 310_378_496, 743_440_384), "296 из 709 МБ");
        assert_eq!(fmt_amount_in(Locale::En, 310_378_496, 743_440_384), "296 of 709 MB");
        let gb = 2_500_000_000u64;
        assert_eq!(fmt_amount_in(Locale::Ru, 429_496_730, gb), "0,4 из 2,3 ГБ");
        assert_eq!(fmt_amount_in(Locale::En, 429_496_730, gb), "0.4 of 2.3 GB");
    }
}
