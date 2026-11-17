//! Common UI strings, units and pluralization.

use std::time::Duration;

use super::{Locale, PluralForms, locale, plural_forms};

/// Static common strings shared across screens.
pub struct CommonStrings {
    pub play: &'static str,
    pub shuffle: &'static str,
    pub cancel: &'static str,
    pub undo: &'static str,
    pub save: &'static str,
    pub close: &'static str,
    pub done: &'static str,
    pub skip: &'static str,
    pub more: &'static str,
    pub queue: &'static str,
    pub search: &'static str,
    pub kbps: &'static str,
    pub mb: &'static str,
    pub sec_short: &'static str,
    pub min_short: &'static str,
    pub hr_short: &'static str,
    pub tracks: PluralForms,
    pub artists: PluralForms,
}

pub static RU: CommonStrings = CommonStrings {
    play: "Слушать",
    shuffle: "Перемешать",
    cancel: "Отмена",
    undo: "Отменить",
    save: "Сохранить",
    close: "Закрыть",
    done: "Готово",
    skip: "Пропустить",
    more: "Ещё",
    queue: "Очередь",
    search: "Поиск",
    kbps: "кбит/с",
    mb: "МБ",
    sec_short: "сек",
    min_short: "мин",
    hr_short: "ч",
    tracks: PluralForms { one: "трек", few: "трека", many: "треков" },
    artists: PluralForms { one: "исполнитель", few: "исполнителя", many: "исполнителей" },
};

pub static EN: CommonStrings = CommonStrings {
    play: "Play",
    shuffle: "Shuffle",
    cancel: "Cancel",
    undo: "Undo",
    save: "Save",
    close: "Close",
    done: "Done",
    skip: "Skip",
    more: "More",
    queue: "Queue",
    search: "Search",
    kbps: "kbps",
    mb: "MB",
    sec_short: "sec",
    min_short: "min",
    hr_short: "hr",
    tracks: PluralForms { one: "track", few: "tracks", many: "tracks" },
    artists: PluralForms { one: "artist", few: "artists", many: "artists" },
};

locale_table! {
    /// Returns common strings for `loc`.
    common_in -> CommonStrings,
    /// Returns common strings for the active process locale.
    common,
    { Ru => RU, En => EN }
}

/// Formats `n` tracks in `loc` (e.g. `"1 трек"`, `"2 трека"`, `"5 треков"` / `"1 track"`, `"2 tracks"`).
pub fn tracks_count_in(loc: Locale, n: u64) -> String {
    let unit = plural_forms(loc, n, &common_in(loc).tracks);
    format!("{n} {unit}")
}

/// Formats `n` tracks in the active locale.
pub fn tracks_count(n: u64) -> String {
    tracks_count_in(locale(), n)
}

/// Formats a total duration as `"42 сек"` / `"47 мин"` / `"1 ч 12 мин"` (or English equivalents).
pub fn fmt_total_in(loc: Locale, d: Duration) -> String {
    let s = common_in(loc);
    let m = d.as_secs() / 60;
    if m == 0 {
        format!("{} {}", d.as_secs(), s.sec_short)
    } else if m >= 60 {
        format!("{} {} {} {}", m / 60, s.hr_short, m % 60, s.min_short)
    } else {
        format!("{m} {}", s.min_short)
    }
}

/// Formats a total duration in the active locale.
pub fn fmt_total(d: Duration) -> String {
    fmt_total_in(locale(), d)
}

/// Formats `ms` rounded to minutes (`"45 мин"`, `"2 ч"`, `"1 ч 4 мин"` / `"45 min"`, `"2 hr"`, `"1 hr 4 min"`).
pub fn hours_minutes_in(loc: Locale, ms: u64) -> String {
    let s = common_in(loc);
    let m = ((ms + 30_000) / 60_000).max(1);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} {}", s.min_short),
        (h, 0) => format!("{h} {}", s.hr_short),
        (h, m) => format!("{h} {} {m} {}", s.hr_short, s.min_short),
    }
}

/// Formats `ms` rounded to minutes in the active locale.
pub fn hours_minutes(ms: u64) -> String {
    hours_minutes_in(locale(), ms)
}
