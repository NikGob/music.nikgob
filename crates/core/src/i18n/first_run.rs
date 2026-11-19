//! First-run wizard.

use super::Locale;

/// Static strings: first-run wizard.
pub struct FirstRunStrings {
    pub step_sources: &'static str,
    pub step_keep: &'static str,
    pub step_connect: &'static str,
    pub step_lyrics: &'static str,
    pub step_more: &'static str,
    pub next: &'static str,
    pub keep_yes: &'static str,
    pub keep_no: &'static str,
    pub source_folder: &'static str,
    pub source_local_only: &'static str,
    pub sc_step1: &'static str,
    pub sc_step2: &'static str,
    pub other_folder: &'static str,
    pub pick_folder: &'static str,
    pub music_folder_title: &'static str,
    pub history_btn: &'static str,
    pub archive_btn: &'static str,
}

pub static RU: FirstRunStrings = FirstRunStrings {
    step_sources: "Откуда брать музыку?",
    step_keep: "Хранить музыку на компьютере?",
    step_connect: "Подключи",
    step_lyrics: "Как показывать текст песен?",
    step_more: "Ещё немного",
    next: "Далее",
    keep_yes: "Да, скачивать",
    keep_no: "Нет, только слушать",
    source_folder: "Папка на диске",
    source_local_only: "Только локальное",
    sc_step1: "Войди на soundcloud.com в браузере.",
    sc_step2: "Открой инструменты разработчика → Application → Cookies и скопируй значение oauth_token.",
    other_folder: "Другая…",
    pick_folder: "Выбрать папку…",
    music_folder_title: "Папка с музыкой",
    history_btn: "Перенести историю прослушиваний",
    archive_btn: "Архив .zip…",
};

pub static EN: FirstRunStrings = FirstRunStrings {
    step_sources: "Where should music come from?",
    step_keep: "Keep music on this computer?",
    step_connect: "Connect",
    step_lyrics: "How should lyrics be shown?",
    step_more: "A few more things",
    next: "Next",
    keep_yes: "Yes, download",
    keep_no: "No, just listen",
    source_folder: "Folder on disk",
    source_local_only: "Local only",
    sc_step1: "Sign in to soundcloud.com in a browser.",
    sc_step2: "Open developer tools → Application → Cookies and copy the oauth_token value.",
    other_folder: "Choose another…",
    pick_folder: "Choose folder…",
    music_folder_title: "Music folder",
    history_btn: "Import listening history",
    archive_btn: "Archive .zip…",
};

locale_table! {
    /// Returns these strings for `loc`.
    first_run_in -> FirstRunStrings,
    /// Returns these strings for the active process locale.
    first_run,
    { Ru => RU, En => EN }
}

/// The theme choices of the last step («Светлая · Тёмная · Системная» / «Light · Dark · System»).
pub const fn themes_in(loc: Locale) -> [&'static str; 3] {
    match loc {
        Locale::Ru => ["Светлая", "Тёмная", "Системная"],
        Locale::En => ["Light", "Dark", "System"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_titles_and_choices_cover_both_locales() {
        assert_eq!(first_run_in(Locale::Ru).step_sources, "Откуда брать музыку?");
        assert_eq!(first_run_in(Locale::En).step_sources, "Where should music come from?");
        assert_eq!(first_run_in(Locale::Ru).step_connect, "Подключи");
        assert_eq!(first_run_in(Locale::En).step_connect, "Connect");
        assert_eq!(first_run_in(Locale::Ru).keep_no, "Нет, только слушать");
        assert_eq!(first_run_in(Locale::En).keep_no, "No, just listen");
        assert_eq!(first_run_in(Locale::Ru).source_local_only, "Только локальное");
        assert_eq!(first_run_in(Locale::En).source_local_only, "Local only");
        assert_eq!(themes_in(Locale::Ru), ["Светлая", "Тёмная", "Системная"]);
        assert_eq!(themes_in(Locale::En), ["Light", "Dark", "System"]);
    }
}
