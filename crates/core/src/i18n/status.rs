//! Import, wishlist and torrent statuses rendered from runtime state.
//!
//! The runtime keeps codes and numbers in its state and says them here, so
//! a status is translated where it is drawn, not where it was made.

use super::{Locale, PluralForms, plural_forms};

/// Static strings: import, wishlist and torrent statuses rendered from runtime state.
pub struct StatusStrings {
    // A running download.
    pub downloading: &'static str,
    pub connecting: &'static str,
    pub queued: &'static str,
    pub paused: &'static str,
    pub importing: &'static str,
    /// `"12 МБ"`, `"из"`, `"33 МБ"`: the words between the two sizes.
    pub of: &'static str,
    /// `"1,2 МБ"` + it: `"1,2 МБ/с"`.
    pub per_sec: &'static str,
    pub eta_hours: &'static str,
    pub eta_minutes: &'static str,
    pub eta_seconds: &'static str,

    // An import.
    pub stage_probing: &'static str,
    pub stage_fingerprinting: &'static str,
    pub stage_identifying: &'static str,
    pub stage_quality: &'static str,
    pub stage_deciding: &'static str,
    pub stage_converting: &'static str,
    pub stage_tagging: &'static str,
    pub stage_placing: &'static str,
    pub stage_deriving: &'static str,
    pub preparing_files: &'static str,
    pub file_tags: &'static str,
    pub liked_stream_track: &'static str,
    pub unknown_artist: &'static str,
    pub import_button: &'static str,
    pub which_track: &'static str,
    pub import_title: &'static str,
    pub import_from_inbox: &'static str,
    pub cancelled_summary: &'static str,
    pub parked: &'static str,
    pub moved_masters_title: &'static str,
    pub today: &'static str,
    pub yesterday: &'static str,
    pub date_today: &'static str,
    pub date_yesterday: &'static str,
    pub months: [&'static str; 12],
    pub album_artist: &'static str,
    pub year: &'static str,
    pub genre: &'static str,
    pub loudness: &'static str,
    /// A track without a name of its own, in a line's words.
    pub track: &'static str,

    // A «Хочу скачать» item.
    pub note_files_wait: &'static str,
    pub note_not_found: &'static str,
    pub note_has_copy: &'static str,
    pub note_searching: &'static str,
    pub note_sources_silent: &'static str,
    pub note_found_better: &'static str,
    pub note_not_sure: &'static str,
    pub note_torrent: &'static str,
    pub note_own_torrent: &'static str,
    pub note_own_file: &'static str,
    pub note_saving_youtube: &'static str,
    pub note_save_failed: &'static str,
    pub note_nothing_better: &'static str,
    pub note_kept_file: &'static str,
    pub note_kept_files: &'static str,
    pub note_files_back: &'static str,
    pub card_skip: &'static str,
    pub card_accept: &'static str,
    pub card_question_track: &'static str,
    pub card_question_album: &'static str,
    /// A source that finds but gives no file.
    pub finds_only: &'static str,
    pub site_check: &'static str,
    pub no_answer: &'static str,
    pub sources_gave_nothing: &'static str,
    pub torrent_offer: &'static str,
    pub what_track: &'static str,
    pub what_album: &'static str,
    pub what_rest: &'static str,
    /// `«Подтверди»` holds other options of the item.
    pub next_doubtful: &'static str,

    // A better copy of a file.
    pub better_quality: &'static str,
    /// What a better copy is looked for: the bar's words.
    pub lossless: &'static str,
    pub mp3_320_or_better: &'static str,
    pub better_album: &'static str,
    pub better_track: &'static str,
    pub replace_album_question: &'static str,
    pub replace_track_question: &'static str,
    pub replace_tracks_question: &'static str,
    pub keep_as_is: &'static str,
    pub replace: &'static str,
    pub replaced_better: &'static str,
    /// `"FLAC"` + it: `"FLAC, возможно из lossy"`.
    pub maybe_lossy: &'static str,
    /// The separator of a quality label whose source is another format
    /// (`"FLAC, возможно из MP3 320"`).
    pub maybe_from: &'static str,
    /// `"FLAC"` + it: `"FLAC из lossy"`.
    pub from_lossy: &'static str,
    /// The separator a quality label uses for its source (`"FLAC из MP3
    /// ~128"`); the generic one the importer's labels carry.
    pub made_from: &'static str,

    // Torrents.
    pub seeders: PluralForms,
    pub files_plural: PluralForms,
}

pub static RU: StatusStrings = StatusStrings {
    downloading: "качается",
    connecting: "соединяюсь",
    queued: "в очереди",
    paused: "на паузе",
    importing: "добавляю в библиотеку",
    of: "из",
    per_sec: "/с",
    eta_hours: "ч",
    eta_minutes: "мин",
    eta_seconds: "с",

    stage_probing: "читаю файл",
    stage_fingerprinting: "снимаю отпечаток",
    stage_identifying: "узнаю трек",
    stage_quality: "проверяю качество",
    stage_deciding: "сверяю с библиотекой",
    stage_converting: "конвертирую",
    stage_tagging: "пишу теги",
    stage_placing: "кладу в библиотеку",
    stage_deriving: "беру цвета обложки",
    preparing_files: "готовлю файлы",
    file_tags: "теги файла",
    liked_stream_track: "лайкнутый стрим-трек, другая длина",
    unknown_artist: "исполнитель неизвестен",
    import_button: "Импортировать",
    which_track: "Это какой трек?",
    import_title: "Импорт",
    import_from_inbox: "Импорт из Inbox",
    cancelled_summary: "отменено — добавленное убрано",
    parked: "отправлено на подтверждение",
    moved_masters_title: "Перенос архива FLAC",
    today: "сегодня",
    yesterday: "вчера",
    date_today: "Сегодня",
    date_yesterday: "Вчера",
    months: [
        "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября",
        "декабря",
    ],
    album_artist: "Исполнитель альбома",
    year: "Год",
    genre: "Жанр",
    loudness: "Громкость",
    track: "трек",

    note_files_wait: "Файлы ждут подтверждения",
    note_not_found: "Не нашёл, где скачать — поищу снова через неделю",
    note_has_copy: "Есть копия с YouTube — раз в неделю ищу качество лучше",
    note_searching: "Ищу…",
    note_sources_silent: "Источники не ответили — попробую через час",
    note_found_better: "Нашёл качество лучше — подтверди замену",
    note_not_sure: "Не уверен, что это оно — выбери",
    note_torrent: "Раздача: {}",
    note_own_torrent: "Своя раздача",
    note_own_file: "Свой файл",
    note_saving_youtube: "Сохраняю с YouTube…",
    note_save_failed: "Не получилось сохранить с YouTube",
    note_nothing_better: "Лучше пока нет",
    note_kept_file: "Оставил прежний файл",
    note_kept_files: "Оставил прежние файлы",
    note_files_back: "Вернул прежние файлы",
    card_skip: "Не то — поищу позже",
    card_accept: "Скачать",
    card_question_track: "Что из этого — этот трек?",
    card_question_album: "Что из этого — этот альбом?",
    finds_only: "Находит, но не скачивает",
    site_check: "Сайт закрыт проверкой — не могу зайти",
    no_answer: "Не отвечает",
    sources_gave_nothing: "Источники не дали скачать",
    torrent_offer: "торрент",
    what_track: "этот трек",
    what_album: "этот альбом",
    what_rest: "остальное",
    next_doubtful: "есть другие варианты — в «Подтверди»",

    better_quality: "качество лучше",
    lossless: "lossless",
    mp3_320_or_better: "MP3 320 или лучше",
    better_album: "Качество лучше · альбом",
    better_track: "Качество лучше · трек",
    replace_album_question: "Заменить файлы альбома на это?",
    replace_track_question: "Заменить файл трека на это?",
    replace_tracks_question: "Заменить файлы этих треков?",
    keep_as_is: "Оставить как есть",
    replace: "Заменить",
    replaced_better: "Заменил на качество лучше",
    maybe_lossy: ", возможно из lossy",
    maybe_from: ", возможно из ",
    from_lossy: " из lossy",
    made_from: " из ",

    seeders: PluralForms { one: "сид.", few: "сид.", many: "сид." },
    files_plural: PluralForms { one: "файл", few: "файла", many: "файлов" },
};

pub static EN: StatusStrings = StatusStrings {
    downloading: "downloading",
    connecting: "connecting",
    queued: "queued",
    paused: "paused",
    importing: "adding to library",
    of: "of",
    per_sec: "/s",
    eta_hours: "hr",
    eta_minutes: "min",
    eta_seconds: "sec",

    stage_probing: "reading file",
    stage_fingerprinting: "fingerprinting",
    stage_identifying: "identifying track",
    stage_quality: "checking quality",
    stage_deciding: "checking against the library",
    stage_converting: "converting",
    stage_tagging: "writing tags",
    stage_placing: "adding to library",
    stage_deriving: "extracting cover colors",
    preparing_files: "preparing files",
    file_tags: "file tags",
    liked_stream_track: "liked streaming track, different duration",
    unknown_artist: "unknown artist",
    import_button: "Import",
    which_track: "Which track is this?",
    import_title: "Import",
    import_from_inbox: "Import from Inbox",
    cancelled_summary: "cancelled — added files removed",
    parked: "sent for confirmation",
    moved_masters_title: "Moving the FLAC archive",
    today: "today",
    yesterday: "yesterday",
    date_today: "Today",
    date_yesterday: "Yesterday",
    months: [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
        "December",
    ],
    album_artist: "Album artist",
    year: "Year",
    genre: "Genre",
    loudness: "Loudness",
    track: "track",

    note_files_wait: "Files are waiting for confirmation",
    note_not_found: "No download found — searching again in a week",
    note_has_copy: "YouTube copy saved — checking weekly for better quality",
    note_searching: "Searching…",
    note_sources_silent: "Sources did not respond — trying again in an hour",
    note_found_better: "Found better quality — confirm the replacement",
    note_not_sure: "Not sure this is it — pick one",
    note_torrent: "Torrent: {}",
    note_own_torrent: "Your torrent",
    note_own_file: "Your file",
    note_saving_youtube: "Saving from YouTube…",
    note_save_failed: "Could not save from YouTube",
    note_nothing_better: "Nothing better yet",
    note_kept_file: "Kept the previous file",
    note_kept_files: "Kept the previous files",
    note_files_back: "Previous files restored",
    card_skip: "Not a match — search later",
    card_accept: "Download",
    card_question_track: "Which of these is the track?",
    card_question_album: "Which of these is the album?",
    finds_only: "Finds matches but cannot download",
    site_check: "Blocked by the site's verification check",
    no_answer: "Not responding",
    sources_gave_nothing: "No source could provide a download",
    torrent_offer: "torrent",
    what_track: "this track",
    what_album: "this album",
    what_rest: "the rest",
    next_doubtful: "other options are in “Confirm”",

    better_quality: "better quality",
    lossless: "lossless",
    mp3_320_or_better: "MP3 320 or better",
    better_album: "Better quality · album",
    better_track: "Better quality · track",
    replace_album_question: "Replace the album's files with these?",
    replace_track_question: "Replace the track's file with this?",
    replace_tracks_question: "Replace these tracks' files?",
    keep_as_is: "Keep as is",
    replace: "Replace",
    replaced_better: "Replaced with better quality",
    maybe_lossy: ", possibly from a lossy source",
    maybe_from: ", possibly from ",
    from_lossy: " from a lossy source",
    made_from: " from ",

    seeders: PluralForms { one: "seed", few: "seeds", many: "seeds" },
    files_plural: PluralForms { one: "file", few: "files", many: "files" },
};

locale_table! {
    /// Returns these strings for `loc`.
    status_in -> StatusStrings,
    /// Returns these strings for the active process locale.
    status,
    { Ru => RU, En => EN }
}

/// `name` in the locale's quotes: `«Song»` / `“Song”`.
pub fn quoted_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("«{name}»"),
        Locale::En => format!("“{name}”"),
    }
}

/// `"1,2 МБ/с"`: the size in the caller's words, the unit here.
pub fn fmt_rate_in(loc: Locale, size: &str) -> String {
    format!("{size}{}", status_in(loc).per_sec)
}

/// `"12,3 МБ из 33,1 МБ · 37%"`; without a known size, the bytes alone.
pub fn fmt_amount_in(loc: Locale, done: &str, total: Option<&str>, percent: Option<u8>) -> String {
    match (total, percent) {
        (Some(total), Some(p)) => format!("{done} {} {total} · {p}%", status_in(loc).of),
        _ => done.to_owned(),
    }
}

/// `"на паузе · 12,3 МБ из 33,1 МБ · 37%"`.
pub fn fmt_paused_in(loc: Locale, amount: &str) -> String {
    format!("{} · {amount}", status_in(loc).paused)
}

/// `"в очереди у mugilaw · 12-й"` (the place only when known).
pub fn fmt_queued_in(loc: Locale, from: Option<&str>, place: Option<u32>) -> String {
    let s = status_in(loc);
    let mut out = match (loc, from.filter(|f| !f.is_empty())) {
        (_, Some(from)) => format!("{} {} {from}", s.queued, if loc == Locale::Ru { "у" } else { "at" }),
        (_, None) => s.queued.to_owned(),
    };
    if let Some(n) = place.filter(|n| *n > 0) {
        match loc {
            Locale::Ru => out.push_str(&format!(" · {n}-й")),
            Locale::En => out.push_str(&format!(" · #{n}")),
        }
    }
    out
}

/// A time left: `"51 с"`, `"3 мин"`, `"1 ч 5 мин"` / `"51 sec"`, `"3 min"`,
/// `"1 hr 5 min"`.
pub fn eta_in(loc: Locale, secs: u64) -> String {
    let s = status_in(loc);
    if secs >= 3600 {
        format!("{} {} {} {}", secs / 3600, s.eta_hours, secs % 3600 / 60, s.eta_minutes)
    } else if secs >= 60 {
        format!("{} {}", secs / 60, s.eta_minutes)
    } else {
        format!("{secs} {}", s.eta_seconds)
    }
}

/// `"Импорт «Song»"` for one path, `"Импорт 3 объектов"` for several.
pub fn fmt_import_of_in(loc: Locale, name: &str, count: usize) -> String {
    match (loc, count > 1) {
        (Locale::Ru, false) => format!("Импорт «{name}»"),
        (Locale::Ru, true) => format!("Импорт {count} объектов"),
        (Locale::En, false) => format!("Import “{name}”"),
        (Locale::En, true) => format!("Import {count} items"),
    }
}

/// An import's row title: its name, and how many of its files are in when
/// it has several (`"Album · 3 из 12"`).
pub fn fmt_import_title_in(loc: Locale, title: &str, done: usize, files: usize) -> String {
    let all = files.max(done);
    if all > 1 {
        format!("{title} · {done} {} {all}", status_in(loc).of)
    } else {
        title.to_owned()
    }
}

/// `"Файл: x.flac"`.
pub fn fmt_file_origin_in(loc: Locale, file: &str) -> String {
    match loc {
        Locale::Ru => format!("Файл: {file}"),
        Locale::En => format!("File: {file}"),
    }
}

/// `"совпадение 87%"`.
pub fn fmt_match_in(loc: Locale, pct: f32) -> String {
    match loc {
        Locale::Ru => format!("совпадение {pct:.0}%"),
        Locale::En => format!("match {pct:.0}%"),
    }
}

/// The summary of a finished import: what it added, kept out or could not
/// take, each its own (`"добавлено 3 · дублей 1 · ошибок 2"`).
pub fn fmt_import_summary_in(
    loc: Locale,
    imported: usize,
    duplicates: usize,
    left_out: usize,
    failed: usize,
    pending: usize,
) -> String {
    let mut parts = vec![fmt_added_in(loc, imported)];
    if duplicates > 0 {
        parts.push(fmt_duplicates_in(loc, duplicates));
    }
    if left_out > 0 {
        parts.push(fmt_left_out_in(loc, left_out));
    }
    if failed > 0 {
        parts.push(fmt_errors_in(loc, failed));
    }
    if pending > 0 {
        parts.push(fmt_parked_in(loc, pending));
    }
    parts.join(" · ")
}

/// `"добавлено 3"`.
pub fn fmt_added_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("добавлено {n}"),
        Locale::En => format!("added {n}"),
    }
}

/// `"дублей 1"`.
pub fn fmt_duplicates_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("дублей {n}"),
        Locale::En => format!("duplicates {n}"),
    }
}

/// `"не из этого альбома 2"`.
pub fn fmt_left_out_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("не из этого альбома {n}"),
        Locale::En => format!("not from this album {n}"),
    }
}

/// `"ошибок 2"`.
pub fn fmt_errors_in(loc: Locale, n: usize) -> String {
    match loc {
        Locale::Ru => format!("ошибок {n}"),
        Locale::En => format!("errors {n}"),
    }
}

/// `"отправлено на подтверждение 2"`.
pub fn fmt_parked_in(loc: Locale, n: usize) -> String {
    format!("{} {n}", status_in(loc).parked)
}

/// The error line of a finished import: which files held another song
/// (said by title), and how many failed otherwise.
pub fn fmt_failed_words_in(loc: Locale, failed: usize, wrong: &[String]) -> Option<String> {
    if failed == 0 && wrong.is_empty() {
        return None;
    }
    let titles = wrong.iter().map(|t| quoted_in(loc, t)).collect::<Vec<_>>().join(", ");
    let other = failed.saturating_sub(wrong.len());
    Some(match (wrong.is_empty(), other) {
        (true, n) => match loc {
            Locale::Ru => format!("Не удалось: {n}"),
            Locale::En => format!("Failed: {n}"),
        },
        (false, 0) => match loc {
            Locale::Ru => format!("В файле другая песня: {titles}"),
            Locale::En => format!("Wrong song in file: {titles}"),
        },
        (false, n) => match loc {
            Locale::Ru => format!("В файле другая песня: {titles} · не удалось ещё: {n}"),
            Locale::En => format!("Wrong song in file: {titles} · {n} more failed"),
        },
    })
}

/// Which day a date of «Скачано» is, as its label says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayLabel {
    Today,
    Yesterday,
    /// The same year as today: the day and the month.
    SameYear,
    /// Another year: the day, the month and the year.
    Other,
}

/// A day of «Скачано»: `"Сегодня"`, `"Вчера"`, `"3 мая"`, `"1 декабря
/// 2024"` / `"May 3"`, `"December 1, 2024"`.
pub fn fmt_date_label_in(loc: Locale, day: u8, month: u8, year: i16, kind: DayLabel) -> String {
    let s = status_in(loc);
    let month = s.months[usize::from(month.clamp(1, 12)) - 1];
    match (kind, loc) {
        (DayLabel::Today, _) => s.date_today.to_owned(),
        (DayLabel::Yesterday, _) => s.date_yesterday.to_owned(),
        (DayLabel::SameYear, Locale::Ru) => format!("{day} {month}"),
        (DayLabel::SameYear, Locale::En) => format!("{month} {day}"),
        (DayLabel::Other, Locale::Ru) => format!("{day} {month} {year}"),
        (DayLabel::Other, Locale::En) => format!("{month} {day}, {year}"),
    }
}

/// `"сегодня"`, `"вчера"`, `"3 дн. назад"`.
pub fn fmt_days_ago_in(loc: Locale, days: u32) -> String {
    let s = status_in(loc);
    match (days, loc) {
        (0, _) => s.today.to_owned(),
        (1, _) => s.yesterday.to_owned(),
        (n, Locale::Ru) => format!("{n} дн. назад"),
        (n, Locale::En) => format!("{n} days ago"),
    }
}

/// `"поищу снова в 21:40"`.
pub fn fmt_search_again_in(loc: Locale, at: &str) -> String {
    match loc {
        Locale::Ru => format!("поищу снова {at}"),
        Locale::En => format!("searching again {at}"),
    }
}

/// `"поищу снова через 6 дн."`.
pub fn fmt_search_again_days_in(loc: Locale, days: u64) -> String {
    match loc {
        Locale::Ru => format!("поищу снова через {days} дн."),
        Locale::En => format!("searching again in {days} d"),
    }
}

/// `"поищу снова через неделю"`: the weekly search of an item nothing was
/// found for (its next try is a week away, but the words are the ones it
/// always had).
pub fn fmt_search_again_week_in(loc: Locale) -> String {
    match loc {
        Locale::Ru => "поищу снова через неделю".to_owned(),
        Locale::En => "searching again in a week".to_owned(),
    }
}

/// `"Смотрю, есть ли трек в раздаче: Album (2020) FLAC"`.
pub fn fmt_checking_torrent_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("Смотрю, есть ли трек в раздаче: {title}"),
        Locale::En => format!("Checking whether the torrent has the track: {title}"),
    }
}

/// `"Раздача: Album (2020) FLAC"`.
pub fn fmt_torrent_note_in(loc: Locale, title: &str) -> String {
    status_in(loc).note_torrent.replace("{}", title)
}

/// `"FLAC · 312 МБ · 14 сид. · Album (2020) FLAC"`: a torrent found for an
/// item, in its row's words.
pub fn fmt_torrent_found_in(loc: Locale, format: &str, size: &str, seeders: u32, title: &str) -> String {
    let word = plural_forms(loc, u64::from(seeders), &status_in(loc).seeders);
    format!("{format} · {size} · {seeders} {word} · {title}")
}

/// A done item's note: `"Скачано"`, `"Скачано 3 из 10"`, `"Скачано · FLAC
/// из MP3 ~320"`.
pub fn fmt_done_note_in(loc: Locale, have: Option<(usize, usize)>, quality: Option<&str>) -> String {
    let done = match loc {
        Locale::Ru => "Скачано",
        Locale::En => "Downloaded",
    };
    match (have, quality) {
        (Some((have, all)), _) => match loc {
            Locale::Ru => format!("Скачано {have} из {all}"),
            Locale::En => format!("Downloaded {have} of {all}"),
        },
        (None, Some(q)) => format!("{done} · {q}"),
        (None, None) => done.to_owned(),
    }
}

/// `"Импорт для «Хочу скачать»"`.
pub fn fmt_import_for_wishlist_in(loc: Locale) -> String {
    let tab = crate::i18n::downloads_in(loc).tab_wishlist;
    match loc {
        Locale::Ru => format!("Импорт для «{tab}»"),
        Locale::En => format!("Import for {tab}"),
    }
}

/// `"Сохранение с YouTube"`.
pub fn fmt_saving_from_youtube_in(loc: Locale) -> String {
    match loc {
        Locale::Ru => "Сохранение с YouTube".to_owned(),
        Locale::En => "Saving from YouTube".to_owned(),
    }
}

/// `"Находит, но не скачивает — пропускаю до 02:07"`.
pub fn fmt_finds_only_until_in(loc: Locale, hm: &str) -> String {
    let s = status_in(loc);
    match loc {
        Locale::Ru => format!("{} — пропускаю до {hm}", s.finds_only),
        Locale::En => format!("{} — skipping until {hm}", s.finds_only),
    }
}

/// `"Не пускает — попробую в 02:07"` / `"Не пускает — попробую позже"`.
pub fn fmt_blocked_in(loc: Locale, hm: Option<&str>) -> String {
    match (loc, hm) {
        (Locale::Ru, Some(hm)) => format!("Не пускает — попробую в {hm}"),
        (Locale::Ru, None) => "Не пускает — попробую позже".to_owned(),
        (Locale::En, Some(hm)) => format!("Access denied — trying again at {hm}"),
        (Locale::En, None) => "Access denied — trying again later".to_owned(),
    }
}

/// `"Лимит на сегодня — попробую в 02:07"`.
pub fn fmt_day_limit_in(loc: Locale, hm: &str) -> String {
    match loc {
        Locale::Ru => format!("Лимит на сегодня — попробую в {hm}"),
        Locale::En => format!("Daily limit reached — trying again at {hm}"),
    }
}

/// `"Просит подождать — попробую в 02:07"` / `"… — попробую позже"`.
pub fn fmt_asks_to_wait_in(loc: Locale, hm: Option<&str>) -> String {
    match (loc, hm) {
        (Locale::Ru, Some(hm)) => format!("Просит подождать — попробую в {hm}"),
        (Locale::Ru, None) => "Просит подождать — попробую позже".to_owned(),
        (Locale::En, Some(hm)) => format!("Please wait — trying again at {hm}"),
        (Locale::En, None) => "Please wait — trying again later".to_owned(),
    }
}

/// `"Отвечает ошибкой 403 — попробую в 02:07"` / `"… 403"`.
pub fn fmt_answers_with_in(loc: Locale, status: u16, hm: Option<&str>) -> String {
    match (loc, hm) {
        (Locale::Ru, Some(hm)) => format!("Отвечает ошибкой {status} — попробую в {hm}"),
        (Locale::Ru, None) => format!("Отвечает ошибкой {status}"),
        (Locale::En, Some(hm)) => format!("Returned error {status} — trying again at {hm}"),
        (Locale::En, None) => format!("Returned error {status}"),
    }
}

/// `"у Qobuz лимит на сегодня"`.
pub fn fmt_source_limit_today_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("у {name} лимит на сегодня"),
        Locale::En => format!("{name} has reached its daily limit"),
    }
}

/// `"Qobuz просит подождать до 03:00"` / `"Qobuz просит подождать"`.
pub fn fmt_source_asks_to_wait_in(loc: Locale, name: &str, hm: Option<&str>) -> String {
    match (loc, hm) {
        (Locale::Ru, Some(hm)) => format!("{name} просит подождать до {hm}"),
        (Locale::Ru, None) => format!("{name} просит подождать"),
        (Locale::En, Some(hm)) => format!("{name} is asking you to wait until {hm}"),
        (Locale::En, None) => format!("{name} is asking you to wait"),
    }
}

/// `"khinsider закрыт проверкой"`.
pub fn fmt_source_check_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} закрыт проверкой"),
        Locale::En => format!("{name} is behind a bot check"),
    }
}

/// `"Soulseek не пускает"`.
pub fn fmt_source_blocked_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} не пускает"),
        Locale::En => format!("{name} denied access"),
    }
}

/// `"Tidal находит, но не скачивает"`.
pub fn fmt_source_finds_only_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} находит, но не скачивает"),
        Locale::En => format!("{name} finds matches but cannot download"),
    }
}

/// `"Tidal сейчас не отдаёт этот трек"`.
pub fn fmt_source_gives_no_in(loc: Locale, name: &str, what: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} сейчас не отдаёт {what}"),
        Locale::En => format!("{name} cannot provide {what} right now"),
    }
}

/// `"сервер Qobuz недоступен"`.
pub fn fmt_source_server_down_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("сервер {name} недоступен"),
        Locale::En => format!("the {name} server is down"),
    }
}

/// `"Tidal не дал скачать"`.
pub fn fmt_source_no_download_in(loc: Locale, name: &str) -> String {
    match loc {
        Locale::Ru => format!("{name} не дал скачать"),
        Locale::En => format!("{name} could not provide a download"),
    }
}

/// `"в 21:40"` / `"через 5 мин"`.
pub fn fmt_at_time_in(loc: Locale, hm: Option<&str>, mins: u64) -> String {
    match (loc, hm) {
        (Locale::Ru, Some(hm)) => format!("в {hm}"),
        (Locale::En, Some(hm)) => format!("at {hm}"),
        (Locale::Ru, None) => format!("через {mins} мин"),
        (Locale::En, None) => format!("in {mins} min"),
    }
}

/// The line of an item every offer of which was turned away: what is here
/// of an album, why each source said no, and when (and whom) it asks
/// again.
pub fn fmt_later_line_in(loc: Locale, have: Option<(usize, usize)>, why: &[String], at: &str, asked: &[&str]) -> String {
    let s = status_in(loc);
    let mut parts: Vec<String> = Vec::new();
    if let Some((have, all)) = have {
        parts.push(match loc {
            Locale::Ru => format!("Есть {have} из {all}"),
            Locale::En => format!("{have} of {all} here"),
        });
    }
    parts.push(if why.is_empty() { s.sources_gave_nothing.to_owned() } else { why.join(", ") });
    parts.push(if asked.is_empty() {
        match loc {
            Locale::Ru => format!("попробую снова {at}"),
            Locale::En => format!("trying again {at}"),
        }
    } else {
        match loc {
            Locale::Ru => format!("{at} спрошу {}", asked.join(", ")),
            Locale::En => format!("{at} asking {}", asked.join(", ")),
        }
    });
    parts.join(" · ")
}

/// `"остальных 3 у Qobuz нет"`.
pub fn fmt_others_absent_in(loc: Locale, missing: usize, name: &str) -> String {
    match loc {
        Locale::Ru => format!("остальных {missing} у {name} нет"),
        Locale::En => format!("{name} has none of the other {missing}"),
    }
}

/// `"Скачано 3 из 10 · …"`.
pub fn fmt_downloaded_of_in(loc: Locale, have: usize, all: usize, why: &str) -> String {
    match loc {
        Locale::Ru => format!("Скачано {have} из {all} · {why}"),
        Locale::En => format!("{have} of {all} downloaded · {why}"),
    }
}

/// The line of an item a file of which held another song.
pub fn fmt_wrong_audio_in(loc: Locale, from: Option<&str>, sounds_like: Option<&str>, looks: bool) -> String {
    let song = sounds_like.map(|s| format!(" — {}", quoted_in(loc, s))).unwrap_or_default();
    let tail = match (looks, loc) {
        (true, Locale::Ru) => " · ищу дальше",
        (true, Locale::En) => " · still searching",
        (false, _) => "",
    };
    match (loc, from) {
        (Locale::Ru, Some(f)) => format!("В файле от {f} другая песня{song}{tail}"),
        (Locale::Ru, None) => format!("В файле другая песня{song}{tail}"),
        (Locale::En, Some(f)) => format!("Wrong song in the file from {f}{song}{tail}"),
        (Locale::En, None) => format!("Wrong song in file{song}{tail}"),
    }
}

/// `"Нашёл качество лучше для 3 треков · подтверди замену в «Подтверди»"`.
pub fn fmt_better_found_in(loc: Locale, n: u64, confirm_tab: &str) -> String {
    // The words the line always used: «для 1 трека», «для 3 треков».
    let what = match loc {
        Locale::Ru => super::plural(loc, n, "трека", "треков", "треков"),
        Locale::En => super::plural(loc, n, "track", "tracks", "tracks"),
    };
    match loc {
        Locale::Ru => format!("Нашёл качество лучше для {n} {what} · подтверди замену в «{confirm_tab}»"),
        Locale::En => format!("Found better quality for {n} {what} · confirm the replacement in {confirm_tab}"),
    }
}

/// `"Нашёл FLAC вместо MP3 320 · подтверди замену в «Подтверди»"`.
pub fn fmt_found_instead_in(loc: Locale, found: Option<&str>, have: Option<&str>, confirm_tab: &str) -> String {
    let what = match (loc, found, have) {
        (Locale::Ru, Some(f), Some(h)) => format!("Нашёл {f} вместо {h}"),
        (Locale::En, Some(f), Some(h)) => format!("Found {f} instead of {h}"),
        (Locale::Ru, Some(f), None) => format!("Нашёл {f}"),
        (Locale::En, Some(f), None) => format!("Found {f}"),
        (_, None, _) => match loc {
            Locale::Ru => "Нашёл качество лучше".to_owned(),
            Locale::En => "Found better quality".to_owned(),
        },
    };
    match loc {
        Locale::Ru => format!("{what} · подтверди замену в «{confirm_tab}»"),
        Locale::En => format!("{what} · confirm the replacement in {confirm_tab}"),
    }
}

/// `"FLAC вместо MP3 128 · 312 МБ · 14 сид. · RuTracker"`: an offer in an
/// item's words, saying what it improves.
pub fn fmt_offer_improving_in(loc: Locale, offer: &crate::source::Offer, have: Option<&str>) -> String {
    let detail = fmt_offer_in(loc, offer);
    match (&offer.format, have) {
        (Some(f), Some(have)) => {
            let label = f.label();
            match detail.strip_prefix(&label) {
                Some(rest) => match loc {
                    Locale::Ru => format!("{label} вместо {have}{rest}"),
                    Locale::En => format!("{label} instead of {have}{rest}"),
                },
                None => detail,
            }
        }
        _ => detail,
    }
}

/// `"FLAC 24/96 · 312 МБ · 14 сид. · RuTracker"`.
pub fn fmt_offer_in(loc: Locale, offer: &crate::source::Offer) -> String {
    let mut parts = Vec::new();
    if let Some(f) = &offer.format {
        parts.push(f.label());
    }
    if let Some(b) = offer.size_bytes {
        parts.push(fmt_size_in(loc, b));
    }
    if let Some(s) = offer.seeders {
        let word = plural_forms(loc, u64::from(s), &status_in(loc).seeders);
        parts.push(format!("{s} {word}"));
    }
    let provider = if offer.provider.is_empty() {
        offer.source.display_name_in(loc).to_owned()
    } else {
        offer.provider.clone()
    };
    parts.push(provider);
    parts.join(" · ")
}

/// `"312 МБ"`, `"1.4 ГБ"` (the units of the locale).
pub fn fmt_size_in(loc: Locale, b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = b as f64 / MB;
    let common = crate::i18n::common_in(loc);
    if mb >= 1024.0 {
        format!("{:.1} {}", mb / 1024.0, common.gb)
    } else if mb >= 10.0 {
        format!("{mb:.0} {}", common.mb)
    } else {
        format!("{mb:.1} {}", common.mb)
    }
}

/// The line of a finished masters move: `"перенесено 3 · проверено BLAKE3 →
/// D:\flac"`, or `"папка → D:\flac"` when no audio moved.
pub fn fmt_masters_move_line_in(loc: Locale, moved: usize, dir: &str) -> String {
    match (loc, moved) {
        (Locale::Ru, 0) => format!("папка → {dir}"),
        (Locale::En, 0) => format!("folder → {dir}"),
        (Locale::Ru, n) => format!("перенесено {n} · проверено BLAKE3 → {dir}"),
        (Locale::En, n) => format!("moved {n} · BLAKE3 checked → {dir}"),
    }
}

/// A finished torrent's history line: `"Скачано 1.2 ГБ"`, or `"Скачано 1.2
/// ГБ · импорт в библиотеку"` while the import takes its files.
pub fn fmt_downloaded_size_in(loc: Locale, size: &str, importing: bool) -> String {
    match (loc, importing) {
        (Locale::Ru, false) => format!("Скачано {size}"),
        (Locale::En, false) => format!("Downloaded {size}"),
        (Locale::Ru, true) => format!("Скачано {size} · импорт в библиотеку"),
        (Locale::En, true) => format!("Downloaded {size} · importing into the library"),
    }
}

/// A torrent's cleanup time: `"чистка через 3 дн."`.
pub fn fmt_seeding_cleanup_in(loc: Locale, days: u64) -> String {
    match loc {
        Locale::Ru => format!("чистка через {days} дн."),
        Locale::En => format!("cleanup in {days} d"),
    }
}

/// `"Available: later — Qobuz"`: what the next offer of the item is.
pub fn fmt_next_offer_in(loc: Locale, from: &str) -> String {
    match loc {
        Locale::Ru => format!("дальше — {from}"),
        Locale::En => format!("next — {from}"),
    }
}

/// `"MP3 128 у 6 из 10"`: an album some of whose files are below the bar.
pub fn fmt_how_many_of_in(loc: Locale, label: &str, n: usize, all: usize) -> String {
    match loc {
        Locale::Ru => format!("{label} у {n} из {all}"),
        Locale::En => format!("{label} in {n} of {all}"),
    }
}

/// `"FLAC 24/96 (в библиотеке Opus 158)"`: what a lossy copy is made from.
pub fn fmt_in_library_in(loc: Locale, own: &str) -> String {
    match loc {
        Locale::Ru => format!("(в библиотеке {own})"),
        Locale::En => format!("(library has {own})"),
    }
}

/// What a better copy did to a file: `"Было MP3 128 — теперь FLAC"`.
pub fn fmt_replaced_in(loc: Locale, from: Option<&str>, to: Option<&str>) -> String {
    match (loc, from, to) {
        (Locale::Ru, Some(f), Some(t)) if f != t => format!("Было {f} — теперь {t}"),
        (Locale::En, Some(f), Some(t)) if f != t => format!("Was {f} — now {t}"),
        (Locale::Ru, _, Some(t)) => format!("Теперь {t}"),
        (Locale::En, _, Some(t)) => format!("Now {t}"),
        (_, _, None) => status_in(loc).replaced_better.to_owned(),
    }
}

/// What an item's better-copy search says of itself now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeState {
    /// Searching for one.
    Searching,
    /// One waits in «Подтверди».
    FoundBetter,
    /// None this time.
    NothingBetter,
}

/// An item's row words for a better copy: `"MP3 128 · ищу lossless"`,
/// `"MP3 128 · нашёл лучше — подтверди замену"`, `"MP3 128 · лучше пока
/// нет, поищу снова через 7 дн."`.
pub fn fmt_upgrade_state_in(loc: Locale, have: Option<&str>, state: UpgradeState, wanted: &str, next: Option<&str>) -> String {
    let have = have.map(|h| format!("{h} · ")).unwrap_or_default();
    match (loc, state) {
        (Locale::Ru, UpgradeState::Searching) => format!("{have}ищу {wanted}"),
        (Locale::En, UpgradeState::Searching) => format!("{have}looking for {wanted}"),
        (Locale::Ru, UpgradeState::FoundBetter) => format!("{have}нашёл лучше — подтверди замену"),
        (Locale::En, UpgradeState::FoundBetter) => format!("{have}found better — confirm the replacement"),
        (Locale::Ru, UpgradeState::NothingBetter) => match next {
            Some(next) => format!("{have}лучше пока нет, {next}"),
            None => format!("{have}лучше пока нет"),
        },
        (Locale::En, UpgradeState::NothingBetter) => match next {
            Some(next) => format!("{have}nothing better yet, {next}"),
            None => format!("{have}nothing better yet"),
        },
    }
}

/// `"Скачанное не лучше прежних файлов — оставил их"`: a better copy of an
/// album came and none of it is better than its files.
pub fn fmt_nothing_better_in(loc: Locale) -> String {
    match loc {
        Locale::Ru => "Скачанное не лучше прежних файлов — оставил их".to_owned(),
        Locale::En => "Downloaded files are no better — kept the previous files".to_owned(),
    }
}

/// `"FLAC вместо MP3 128"` / `"FLAC instead of MP3 128"`.
pub fn fmt_instead_of_in(loc: Locale, to: &str, have: &str) -> String {
    match loc {
        Locale::Ru => format!("{to} вместо {have}"),
        Locale::En => format!("{to} instead of {have}"),
    }
}

/// The row title of an import done for a better copy: `"Качество лучше:
/// Artist — Album"`.
pub fn fmt_better_import_title_in(loc: Locale, title: &str) -> String {
    match loc {
        Locale::Ru => format!("Качество лучше: {title}"),
        Locale::En => format!("Better quality: {title}"),
    }
}

/// `"есть варианты под вопросом — выбери в «Хочу скачать»"`.
pub fn fmt_doubtful_pick_in(loc: Locale, tab: &str) -> String {
    match loc {
        Locale::Ru => format!("есть варианты под вопросом — выбери в «{tab}»"),
        Locale::En => format!("some matches need review — choose one in {tab}"),
    }
}

/// What an import done for a better copy came to, plainly, in place of
/// `"добавлено 0 · дублей 1"`. `dup` is what a copy that was not better
/// really is (`"FLAC из MP3 ~128"`, in the locale of the rules).
pub fn fmt_upgrade_job_words_in(
    loc: Locale,
    imported: usize,
    duplicates: usize,
    left_out: usize,
    failed: usize,
    dup: Option<&str>,
) -> String {
    let s = status_in(loc);
    let mut parts: Vec<String> = Vec::new();
    if imported > 0 {
        parts.push(match loc {
            Locale::Ru => format!("заменено {imported}"),
            Locale::En => format!("replaced {imported}"),
        });
    }
    if duplicates > 0 {
        parts.push(match (imported, duplicates, dup) {
            // «FLAC, возможно из MP3 320»: a cut a real master may have too.
            (0, 1, Some(q)) if q.contains(s.maybe_from) => {
                let (codec, from) = q.split_once(s.maybe_from).unwrap_or((q, ""));
                match loc {
                    Locale::Ru => format!("скачанный {codec}, возможно, из {from} — оставил прежний файл"),
                    Locale::En => format!("the downloaded {codec} may be from {from} — kept the previous file"),
                }
            }
            // A quality label of the importer's own («FLAC из MP3 ~128»).
            (0, 1, Some(q)) if q.contains(s.made_from) => {
                let (codec, from) = q.split_once(s.made_from).unwrap_or((q, ""));
                match loc {
                    Locale::Ru => format!("скачанный {codec} оказался из {from} — оставил прежний файл"),
                    Locale::En => format!("the downloaded {codec} turned out to be from {from} — kept the previous file"),
                }
            }
            (0, 1, Some(q)) => match loc {
                Locale::Ru => format!("скачанный {q} не лучше прежнего — оставил прежний файл"),
                Locale::En => format!("the downloaded {q} is no better — kept the previous file"),
            },
            (0, 1, None) => match loc {
                Locale::Ru => "скачанный файл не лучше прежнего — оставил прежний".to_owned(),
                Locale::En => "the downloaded file is no better — kept the previous one".to_owned(),
            },
            (0, n, _) => match loc {
                Locale::Ru => format!("скачанное не лучше прежнего ({n}) — оставил прежние файлы"),
                Locale::En => format!("downloads are no better ({n}) — kept the previous files"),
            },
            (_, n, _) => match loc {
                Locale::Ru => format!("не лучше прежних {n}"),
                Locale::En => format!("{n} no better than the previous files"),
            },
        });
    }
    if left_out > 0 {
        parts.push(if imported == 0 && duplicates == 0 {
            match loc {
                Locale::Ru => "скачанное не из этого альбома — ничего не добавил".to_owned(),
                Locale::En => "downloads are not from this album — nothing added".to_owned(),
            }
        } else {
            fmt_left_out_in(loc, left_out)
        });
    }
    if failed > 0 {
        parts.push(fmt_errors_in(loc, failed));
    }
    if parts.is_empty() {
        match loc {
            Locale::Ru => "ничего не пришло".to_owned(),
            Locale::En => "nothing downloaded".to_owned(),
        }
    } else {
        parts.join(" · ")
    }
}

/// A note a «Хочу скачать» item carries in its state, kept as a code so no
/// logic has to read the words the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WishNote {
    /// Downloaded files wait in «Подтверди».
    FilesWait,
    /// Nothing was found; searched again in a week.
    NotFound,
    /// Only a copy saved from YouTube is here.
    HasCopy,
    /// Searching now.
    Searching,
    /// Better offers wait in «Подтверди».
    FoundBetter,
    /// Doubtful offers wait in «Подтверди».
    NotSure,
    /// The user's own torrent is downloading.
    OwnTorrent,
    /// The user's own files are importing.
    OwnFile,
    /// A save from YouTube runs.
    SavingYoutube,
    /// A save from YouTube failed.
    SaveFailed,
    /// No source answered the search: looked at again in an hour.
    SourcesSilent,
    /// A better copy was searched for and none was found.
    NothingBetter,
    /// The user kept the file a better copy would have replaced.
    KeptFile,
    /// The user kept the files a better copy would have replaced.
    KeptFiles,
    /// Files the user took back from a replaced album.
    FilesBack,
    /// The item is done.
    Done,
}

impl WishNote {
    /// The code kept in `wishlist.note`: stable, never shown.
    pub const fn code(self) -> &'static str {
        match self {
            Self::FilesWait => "files_wait",
            Self::NotFound => "not_found",
            Self::HasCopy => "has_copy",
            Self::Searching => "searching",
            Self::FoundBetter => "found_better",
            Self::NotSure => "not_sure",
            Self::OwnTorrent => "own_torrent",
            Self::OwnFile => "own_file",
            Self::SavingYoutube => "saving_youtube",
            Self::SaveFailed => "save_failed",
            Self::SourcesSilent => "sources_silent",
            Self::NothingBetter => "nothing_better",
            Self::KeptFile => "kept_file",
            Self::KeptFiles => "kept_files",
            Self::FilesBack => "files_back",
            // Written by earlier versions and by the library's own SQL:
            // kept as it was, so old rows keep their meaning.
            Self::Done => NOTE_DONE,
        }
    }

    /// The note `code` stands for, when it is one of ours.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "files_wait" => Some(Self::FilesWait),
            "not_found" => Some(Self::NotFound),
            "has_copy" => Some(Self::HasCopy),
            "searching" => Some(Self::Searching),
            "found_better" => Some(Self::FoundBetter),
            "not_sure" => Some(Self::NotSure),
            "own_torrent" => Some(Self::OwnTorrent),
            "own_file" => Some(Self::OwnFile),
            "saving_youtube" => Some(Self::SavingYoutube),
            "save_failed" => Some(Self::SaveFailed),
            "sources_silent" => Some(Self::SourcesSilent),
            "nothing_better" => Some(Self::NothingBetter),
            "kept_file" => Some(Self::KeptFile),
            "kept_files" => Some(Self::KeptFiles),
            "files_back" => Some(Self::FilesBack),
            _ => None,
        }
    }
}

/// The note of a done item, as the library's own SQL writes it.
pub const NOTE_DONE: &str = "Скачано";
/// The note of an item whose files wait in «Подтверди», as versions before
/// the codes wrote it in words (still read, never written).
pub const LEGACY_NOTE_FILES_WAIT: &str = "Файлы ждут подтверждения";

/// The state `note` was written for; the two wordings older versions wrote
/// are still read as what they meant.
pub fn wish_note_of(note: &str) -> Option<WishNote> {
    WishNote::from_code(note).or(match note {
        LEGACY_NOTE_FILES_WAIT => Some(WishNote::FilesWait),
        NOTE_DONE => Some(WishNote::Done),
        _ => None,
    })
}

/// A note's words for `loc`; an empty string for [`WishNote::Done`] (its
/// row says «Скачано» itself).
pub fn fmt_wish_note_in(loc: Locale, note: WishNote) -> &'static str {
    let s = status_in(loc);
    match note {
        WishNote::FilesWait => s.note_files_wait,
        WishNote::NotFound => s.note_not_found,
        WishNote::HasCopy => s.note_has_copy,
        WishNote::Searching => s.note_searching,
        WishNote::FoundBetter => s.note_found_better,
        WishNote::NotSure => s.note_not_sure,
        WishNote::OwnTorrent => s.note_own_torrent,
        WishNote::OwnFile => s.note_own_file,
        WishNote::SavingYoutube => s.note_saving_youtube,
        WishNote::SaveFailed => s.note_save_failed,
        WishNote::SourcesSilent => s.note_sources_silent,
        WishNote::NothingBetter => s.note_nothing_better,
        WishNote::KeptFile => s.note_kept_file,
        WishNote::KeptFiles => s.note_kept_files,
        WishNote::FilesBack => s.note_files_back,
        WishNote::Done => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Codec;
    use crate::source::{Format, Offer, OfferScope, SourceKey};

    #[test]
    fn transfer_words_in_both_locales() {
        assert_eq!(eta_in(Locale::Ru, 3_900), "1 ч 5 мин");
        assert_eq!(eta_in(Locale::En, 3_900), "1 hr 5 min");
        assert_eq!(eta_in(Locale::Ru, 180), "3 мин");
        assert_eq!(eta_in(Locale::En, 180), "3 min");
        assert_eq!(eta_in(Locale::Ru, 51), "51 с");
        assert_eq!(eta_in(Locale::En, 51), "51 sec");
        assert_eq!(fmt_queued_in(Locale::Ru, Some("rotted"), Some(12)), "в очереди у rotted · 12-й");
        assert_eq!(fmt_queued_in(Locale::En, Some("rotted"), Some(12)), "queued at rotted · #12");
        assert_eq!(fmt_queued_in(Locale::Ru, None, None), "в очереди");
        assert_eq!(fmt_amount_in(Locale::Ru, "12 МБ", Some("33 МБ"), Some(37)), "12 МБ из 33 МБ · 37%");
        assert_eq!(fmt_amount_in(Locale::En, "12 MB", Some("33 MB"), Some(37)), "12 MB of 33 MB · 37%");
        assert_eq!(fmt_amount_in(Locale::Ru, "12 МБ", None, None), "12 МБ");
        assert_eq!(fmt_paused_in(Locale::Ru, "50%"), "на паузе · 50%");
        assert_eq!(fmt_rate_in(Locale::Ru, "1 МБ"), "1 МБ/с");
        assert_eq!(fmt_rate_in(Locale::En, "1 MB"), "1 MB/s");
    }

    #[test]
    fn import_words_in_both_locales() {
        assert_eq!(fmt_import_title_in(Locale::Ru, "Album", 3, 12), "Album · 3 из 12");
        assert_eq!(fmt_import_title_in(Locale::En, "Album", 3, 12), "Album · 3 of 12");
        assert_eq!(fmt_import_title_in(Locale::Ru, "a.flac", 1, 1), "a.flac");
        assert_eq!(fmt_import_of_in(Locale::Ru, "a.flac", 1), "Импорт «a.flac»");
        assert_eq!(fmt_import_of_in(Locale::En, "a.flac", 1), "Import “a.flac”");
        assert_eq!(fmt_import_of_in(Locale::Ru, "", 3), "Импорт 3 объектов");
        assert_eq!(fmt_import_of_in(Locale::En, "", 3), "Import 3 items");
        assert_eq!(fmt_file_origin_in(Locale::Ru, "a.flac"), "Файл: a.flac");
        assert_eq!(fmt_match_in(Locale::En, 87.4), "match 87%");
        assert_eq!(
            fmt_import_summary_in(Locale::Ru, 3, 1, 2, 0, 4),
            "добавлено 3 · дублей 1 · не из этого альбома 2 · отправлено на подтверждение 4"
        );
        assert_eq!(fmt_import_summary_in(Locale::Ru, 1, 0, 0, 0, 0), "добавлено 1");
        assert_eq!(fmt_import_summary_in(Locale::En, 1, 0, 0, 0, 0), "added 1");
        assert_eq!(fmt_failed_words_in(Locale::Ru, 2, &[]).as_deref(), Some("Не удалось: 2"));
        assert_eq!(
            fmt_failed_words_in(Locale::Ru, 2, &["Song".to_owned()]).as_deref(),
            Some("В файле другая песня: «Song» · не удалось ещё: 1")
        );
        assert_eq!(fmt_failed_words_in(Locale::En, 0, &[]), None);
    }

    #[test]
    fn dates_and_days_in_both_locales() {
        assert_eq!(fmt_date_label_in(Locale::Ru, 3, 5, 2026, DayLabel::Today), "Сегодня");
        assert_eq!(fmt_date_label_in(Locale::En, 3, 5, 2026, DayLabel::Yesterday), "Yesterday");
        assert_eq!(fmt_date_label_in(Locale::Ru, 3, 5, 2026, DayLabel::SameYear), "3 мая");
        assert_eq!(fmt_date_label_in(Locale::En, 3, 5, 2026, DayLabel::SameYear), "May 3");
        assert_eq!(fmt_date_label_in(Locale::Ru, 1, 12, 2024, DayLabel::Other), "1 декабря 2024");
        assert_eq!(fmt_date_label_in(Locale::En, 1, 12, 2024, DayLabel::Other), "December 1, 2024");
        assert_eq!(fmt_days_ago_in(Locale::Ru, 0), "сегодня");
        assert_eq!(fmt_days_ago_in(Locale::Ru, 1), "вчера");
        assert_eq!(fmt_days_ago_in(Locale::Ru, 3), "3 дн. назад");
        assert_eq!(fmt_days_ago_in(Locale::En, 3), "3 days ago");
    }

    fn offer() -> Offer {
        Offer {
            source: SourceKey::Torrents,
            provider: "RuTracker".into(),
            id: "1".into(),
            title: "Album".into(),
            artist: None,
            track_title: None,
            album: None,
            duration_ms: None,
            isrc: None,
            scope: OfferScope::Album,
            format: Some(Format::new(Codec::Flac)),
            size_bytes: Some(312 << 20),
            seeders: Some(14),
            info_url: None,
            handle: String::new(),
        }
    }

    #[test]
    fn an_offer_in_words() {
        assert_eq!(fmt_offer_in(Locale::Ru, &offer()), "FLAC · 312 МБ · 14 сид. · RuTracker");
        assert_eq!(fmt_offer_in(Locale::En, &offer()), "FLAC · 312 MB · 14 seeds · RuTracker");
        assert_eq!(
            fmt_offer_improving_in(Locale::Ru, &offer(), Some("MP3 128")),
            "FLAC вместо MP3 128 · 312 МБ · 14 сид. · RuTracker"
        );
        assert_eq!(fmt_size_in(Locale::Ru, 820 << 20), "820 МБ");
        assert_eq!(fmt_size_in(Locale::En, 820 << 20), "820 MB");
    }

    #[test]
    fn notes_are_codes_with_legacy_words() {
        assert_eq!(WishNote::from_code("files_wait"), Some(WishNote::FilesWait));
        assert_eq!(WishNote::from_code(NOTE_DONE), None);
        assert_eq!(wish_note_of("files_wait"), Some(WishNote::FilesWait));
        assert_eq!(wish_note_of(LEGACY_NOTE_FILES_WAIT), Some(WishNote::FilesWait));
        assert_eq!(wish_note_of(NOTE_DONE), Some(WishNote::Done));
        assert_eq!(wish_note_of("MP3 128 · ищу lossless"), None);
        assert_eq!(fmt_wish_note_in(Locale::Ru, WishNote::HasCopy), RU.note_has_copy);
        assert_eq!(fmt_wish_note_in(Locale::En, WishNote::HasCopy), EN.note_has_copy);
        assert_eq!(fmt_wish_note_in(Locale::Ru, WishNote::Done), "");
    }

    #[test]
    fn a_source_refusal_in_words() {
        assert_eq!(fmt_source_gives_no_in(Locale::Ru, "Tidal", RU.what_track), "Tidal сейчас не отдаёт этот трек");
        assert_eq!(fmt_source_server_down_in(Locale::En, "Qobuz"), "the Qobuz server is down");
        assert_eq!(fmt_source_limit_today_in(Locale::Ru, "Qobuz"), "у Qobuz лимит на сегодня");
        assert_eq!(fmt_blocked_in(Locale::Ru, None), "Не пускает — попробую позже");
        assert!(fmt_blocked_in(Locale::Ru, Some("02:00")).starts_with("Не пускает — попробую в "));
        assert!(fmt_day_limit_in(Locale::En, "02:00").contains("02:00"));
        assert_eq!(fmt_asks_to_wait_in(Locale::Ru, None), "Просит подождать — попробую позже");
        assert_eq!(fmt_answers_with_in(Locale::Ru, 400, None), "Отвечает ошибкой 400");
        assert_eq!(fmt_at_time_in(Locale::Ru, Some("21:40"), 0), "в 21:40");
        assert_eq!(fmt_at_time_in(Locale::Ru, None, 5), "через 5 мин");
        assert_eq!(fmt_others_absent_in(Locale::Ru, 3, "Qobuz"), "остальных 3 у Qobuz нет");
        let line =
            fmt_later_line_in(Locale::Ru, Some((1, 10)), &["сервер Qobuz недоступен".to_owned()], "через 5 мин", &["Tidal"]);
        assert_eq!(line, "Есть 1 из 10 · сервер Qobuz недоступен · через 5 мин спрошу Tidal");
        assert_eq!(
            fmt_later_line_in(Locale::Ru, None, &[], "в 02:00", &[]),
            "Источники не дали скачать · попробую снова в 02:00"
        );
        assert_eq!(fmt_finds_only_until_in(Locale::Ru, "02:07"), "Находит, но не скачивает — пропускаю до 02:07");
    }

    #[test]
    fn better_copy_words() {
        assert_eq!(
            fmt_better_found_in(Locale::Ru, 3, "Подтверди"),
            "Нашёл качество лучше для 3 треков · подтверди замену в «Подтверди»"
        );
        assert_eq!(
            fmt_better_found_in(Locale::En, 1, "Confirm"),
            "Found better quality for 1 track · confirm the replacement in Confirm"
        );
        assert_eq!(
            fmt_found_instead_in(Locale::Ru, Some("FLAC"), Some("MP3 128"), "Подтверди"),
            "Нашёл FLAC вместо MP3 128 · подтверди замену в «Подтверди»"
        );
        assert_eq!(
            fmt_found_instead_in(Locale::Ru, None, Some("MP3 128"), "Подтверди"),
            "Нашёл качество лучше · подтверди замену в «Подтверди»"
        );
        assert_eq!(
            fmt_found_instead_in(Locale::En, None, None, "Confirm"),
            "Found better quality · confirm the replacement in Confirm"
        );
        assert_eq!(
            fmt_wrong_audio_in(Locale::Ru, Some("Soulseek"), Some("Song"), true),
            "В файле от Soulseek другая песня — «Song» · ищу дальше"
        );
        assert_eq!(fmt_wrong_audio_in(Locale::En, None, None, false), "Wrong song in file");
        assert_eq!(fmt_downloaded_of_in(Locale::Ru, 3, 10, "why"), "Скачано 3 из 10 · why");
        assert_eq!(fmt_search_again_week_in(Locale::Ru), "поищу снова через неделю");
        assert_eq!(fmt_search_again_week_in(Locale::En), "searching again in a week");
        assert_eq!(fmt_torrent_note_in(Locale::Ru, "Album"), "Раздача: Album");
        assert_eq!(fmt_torrent_note_in(Locale::En, "Album"), "Torrent: Album");
        assert_eq!(fmt_done_note_in(Locale::Ru, Some((3, 10)), None), "Скачано 3 из 10");
        assert_eq!(fmt_done_note_in(Locale::En, Some((3, 10)), None), "Downloaded 3 of 10");
    }

    /// What a copy that was no better really is, in the words the line used
    /// before: a plain label, «из MP3 ~128», «возможно из MP3 320».
    #[test]
    fn a_copy_no_better_says_what_it_is() {
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some("FLAC из MP3 ~128")),
            "скачанный FLAC оказался из MP3 ~128 — оставил прежний файл"
        );
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some("FLAC, возможно из MP3 320")),
            "скачанный FLAC, возможно, из MP3 320 — оставил прежний файл"
        );
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some("FLAC из lossy")),
            "скачанный FLAC оказался из lossy — оставил прежний файл"
        );
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some("MP3 320")),
            "скачанный MP3 320 не лучше прежнего — оставил прежний файл"
        );
        assert_eq!(fmt_upgrade_job_words_in(Locale::Ru, 2, 1, 1, 0, None), "заменено 2 · не лучше прежних 1 · не из этого альбома 1");
        assert_eq!(fmt_upgrade_job_words_in(Locale::En, 1, 0, 0, 0, None), "replaced 1");
    }
}
