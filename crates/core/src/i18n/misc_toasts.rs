//! Smaller runtime messages: track speed, the stream cache, export files.

use super::Locale;

/// Static strings of this domain.
pub struct MiscToastStrings {
    pub speed_reset: &'static str,
    pub stream_cache_cleared: &'static str,
}

pub static RU: MiscToastStrings = MiscToastStrings {
    speed_reset: "Скорость сброшена",
    stream_cache_cleared: "Кэш стриминга очищен",
};

pub static EN: MiscToastStrings = MiscToastStrings {
    speed_reset: "Speed reset",
    stream_cache_cleared: "Streaming cache cleared",
};

locale_table! {
    /// Returns this domain's strings for `loc`.
    misc_toasts_in -> MiscToastStrings,
    /// Returns this domain's strings for the active process locale.
    misc_toasts,
    { Ru => RU, En => EN }
}

/// A track back at its recorded speed: ««{title}» снова как записан».
pub fn fmt_speed_as_recorded_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("«{title}» снова как записан"),
        Locale::En => format!("“{title}” plays as recorded again"),
    }
}

/// A file that is not an export: ««{name}» — не файл выгрузки».
pub fn fmt_not_export_file_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("«{name}» — не файл выгрузки"),
        Locale::En => format!("“{name}” is not an export file"),
    }
}

/// An export file over the size limit: «{path}: слишком большой файл».
pub fn fmt_file_too_large_in(loc: Locale, path: &str) -> String {
    match loc {
        Locale::Ru => format!("{path}: слишком большой файл"),
        Locale::En => format!("{path}: file is too large"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_cache_and_export_messages_match_both_locales() {
        assert_eq!(RU.speed_reset, "Скорость сброшена");
        assert_eq!(EN.speed_reset, "Speed reset");
        assert_eq!(RU.stream_cache_cleared, "Кэш стриминга очищен");
        assert_eq!(EN.stream_cache_cleared, "Streaming cache cleared");

        let (ru, en) = (Locale::Ru, Locale::En);
        assert_eq!(fmt_speed_as_recorded_in(ru, "Кино"), "«Кино» снова как записан");
        assert_eq!(fmt_speed_as_recorded_in(en, "Кино"), "“Кино” plays as recorded again");
        assert_eq!(fmt_not_export_file_in(ru, "notes.html"), "«notes.html» — не файл выгрузки");
        assert_eq!(fmt_not_export_file_in(en, "notes.html"), "“notes.html” is not an export file");
        assert_eq!(fmt_file_too_large_in(ru, "big.json"), "big.json: слишком большой файл");
        assert_eq!(fmt_file_too_large_in(en, "big.json"), "big.json: file is too large");
    }
}
