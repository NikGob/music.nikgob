//! Lyrics view, editor, sources and alignment strings.


/// Static strings for lyrics display, editing and alignment.
pub struct LyricsStrings {
    pub title: &'static str,
    pub not_found: &'static str,
    pub edit: &'static str,
    pub gpu: &'static str,
}

pub static RU: LyricsStrings = LyricsStrings {
    title: "Текст песни",
    not_found: "Текст не найден",
    edit: "Изменить текст…",
    gpu: "Видеокарта",
};

pub static EN: LyricsStrings = LyricsStrings {
    title: "Lyrics",
    not_found: "Lyrics not found",
    edit: "Edit lyrics…",
    gpu: "GPU",
};

locale_table! {
    /// Returns lyrics strings for `loc`.
    lyrics_in -> LyricsStrings,
    /// Returns lyrics strings for the active process locale.
    lyrics,
    { Ru => RU, En => EN }
}
