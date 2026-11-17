//! Settings tabs, groups, language preference and help strings.

use serde::{Deserialize, Serialize};

use super::{LangPref, Locale, locale};

/// Static strings for the settings screen and language selector.
pub struct SettingsStrings {
    pub tab_music: &'static str,
    pub tab_lyrics: &'static str,
    pub tab_look: &'static str,
    pub tab_services: &'static str,
    pub tab_downloads: &'static str,
    pub tab_net: &'static str,
    pub tab_system: &'static str,
    pub level_simple: &'static str,
    pub level_deep: &'static str,
    pub level_dev: &'static str,
    pub language_label: &'static str,
    pub language_help: &'static str,
    pub lang_auto: &'static str,
    pub lang_ru: &'static str,
    pub lang_en: &'static str,
}

pub static RU: SettingsStrings = SettingsStrings {
    tab_music: "Музыка",
    tab_lyrics: "Текст",
    tab_look: "Вид",
    tab_services: "Сервисы",
    tab_downloads: "Загрузки",
    tab_net: "Межсеть",
    tab_system: "Система",
    level_simple: "Просто",
    level_deep: "Подробно",
    level_dev: "Для разработчика",
    language_label: "Язык",
    language_help: "Язык интерфейса. «Системный» выбирает язык по настройкам операционной системы.",
    lang_auto: "Системный",
    lang_ru: "Русский",
    lang_en: "English",
};

pub static EN: SettingsStrings = SettingsStrings {
    tab_music: "Music",
    tab_lyrics: "Lyrics",
    tab_look: "Appearance",
    tab_services: "Services",
    tab_downloads: "Downloads",
    tab_net: "Network",
    tab_system: "System",
    level_simple: "Simple",
    level_deep: "Detailed",
    level_dev: "Developer",
    language_label: "Language",
    language_help: "Interface language. “System” follows your operating system language.",
    lang_auto: "System",
    lang_ru: "Русский",
    lang_en: "English",
};

locale_table! {
    /// Returns settings strings for `loc`.
    settings_in -> SettingsStrings,
    /// Returns settings strings for the active process locale.
    settings,
    { Ru => RU, En => EN }
}

/// Localized label for a [`LangPref`] option.
pub const fn lang_pref_label_in(loc: Locale, pref: LangPref) -> &'static str {
    pref.label_in(loc)
}

/// Localized label for a [`LangPref`] option in the active locale.
pub fn lang_pref_label(pref: LangPref) -> &'static str {
    lang_pref_label_in(locale(), pref)
}

/// Typed identifier for settings groups, avoiding string comparisons against localized group titles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GroupId {
    Theme,
    NowPlaying,
    Interface,
    Home,
    LyricsView,
    LyricsSearch,
    LyricsTiming,
    Sound,
    Playback,
    Library,
    Dislikes,
    Recs,
    Services,
    Downloads,
    Quality,
    NetApi,
    Mesh,
    System,
    Dev,
}
