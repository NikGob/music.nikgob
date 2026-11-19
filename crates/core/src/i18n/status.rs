//! Import, wishlist and torrent statuses rendered from runtime state.
//!
//! The runtime keeps codes and numbers in its state and says them here, so
//! a status is translated where it is drawn, not where it was made.

use serde::{Deserialize, Serialize};

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
    /// `"FLAC"` + it: `"FLAC из lossy"`.
    pub from_lossy: &'static str,

    // Torrents.
    pub seeders: PluralForms,
    pub files_plural: PluralForms,
    /// The kilohertz unit of a master's rate: «кГц» / «kHz».
    pub khz_unit: &'static str,
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
    from_lossy: " из lossy",

    seeders: PluralForms { one: "сид.", few: "сид.", many: "сид." },
    files_plural: PluralForms { one: "файл", few: "файла", many: "файлов" },
    khz_unit: "кГц",
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
    from_lossy: " from a lossy source",

    seeders: PluralForms { one: "seed", few: "seeds", many: "seeds" },
    files_plural: PluralForms { one: "file", few: "files", many: "files" },
    khz_unit: "kHz",
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
    fmt_offer_words_in(loc, &OfferWords::of(offer))
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
/// really is (`QualityWords::Label`: «FLAC из MP3 ~128»).
pub fn fmt_upgrade_job_words_in(
    loc: Locale,
    imported: usize,
    duplicates: usize,
    left_out: usize,
    failed: usize,
    dup: Option<&QualityWords>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if imported > 0 {
        parts.push(match loc {
            Locale::Ru => format!("заменено {imported}"),
            Locale::En => format!("replaced {imported}"),
        });
    }
    if duplicates > 0 {
        parts.push(match (imported, duplicates, dup) {
            (0, 1, Some(q)) => fmt_no_better_in(loc, q),
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

/// A copy that came and was no better, said by what it really is: «скачанный
/// FLAC оказался из MP3 ~128 — оставил прежний файл».
fn fmt_no_better_in(loc: Locale, q: &QualityWords) -> String {
    use crate::quality::LabelKind;
    let plain = |loc: Locale| {
        let label = fmt_quality_words_in(loc, q);
        match loc {
            Locale::Ru => format!("скачанный {label} не лучше прежнего — оставил прежний файл"),
            Locale::En => format!("the downloaded {label} is no better — kept the previous file"),
        }
    };
    let QualityWords::Label { codec, bitrate_kbps, check } = q else {
        return plain(loc);
    };
    let name = crate::quality::codec_name(codec);
    // A lossy file's label says its bitrate: «MP3 320».
    let with_rate = match bitrate_kbps.filter(|_| !codec.is_lossless()) {
        Some(b) => format!("{name} {b}"),
        None => name.clone(),
    };
    match crate::quality::label_kind(codec, check.as_ref()) {
        LabelKind::Plain => plain(loc),
        // «FLAC из MP3 ~128», «FLAC, возможно из MP3 320», «FLAC из сжатого».
        LabelKind::Fake { kbps, sure } => match (loc, sure) {
            (Locale::Ru, true) => {
                let from = kbps.map_or_else(|| "сжатого".to_owned(), |k| format!("MP3 ~{k}"));
                format!("скачанный {name} оказался из {from} — оставил прежний файл")
            }
            (Locale::En, true) => {
                let from = kbps.map_or_else(|| "lossy".to_owned(), |k| format!("MP3 ~{k}"));
                format!("the downloaded {name} turned out to be from {from} — kept the previous file")
            }
            (Locale::Ru, false) => {
                let from = kbps.map_or_else(|| "сжатого".to_owned(), |k| format!("MP3 {k}"));
                format!("скачанный {name}, возможно, из {from} — оставил прежний файл")
            }
            (Locale::En, false) => {
                let from = kbps.map_or_else(|| "lossy".to_owned(), |k| format!("MP3 {k}"));
                format!("the downloaded {name} may be from {from} — kept the previous file")
            }
        },
        // «MP3 320 из поддельного FLAC», «MP3 320 из FLAC, возможно поддельного».
        LabelKind::FromFake { from, sure } => {
            let made = from.map(crate::quality::codec_name).unwrap_or_default();
            match (loc, sure) {
                (Locale::Ru, true) => {
                    format!("скачанный {with_rate} оказался из поддельного {made} — оставил прежний файл")
                }
                (Locale::En, true) => {
                    format!("the downloaded {with_rate} turned out to be from a fake {made} — kept the previous file")
                }
                (Locale::Ru, false) => {
                    format!("скачанный {with_rate} оказался из {made}, возможно поддельного — оставил прежний файл")
                }
                (Locale::En, false) => {
                    format!("the downloaded {with_rate} turned out to be from {made}, possibly fake — kept the previous file")
                }
            }
        }
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

/// The first letter capitalised (a line that stands alone).
pub fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// `"1.2 ГБ"`, `"312 МБ"`: a download's size, in the decimal units its row
/// says it.
pub fn fmt_bytes_in(loc: Locale, b: u64) -> String {
    let (gb, mb, kb) = match loc {
        Locale::Ru => ("ГБ", "МБ", "КБ"),
        Locale::En => ("GB", "MB", "KB"),
    };
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} {gb}", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} {mb}", b / 1e6)
    } else {
        format!("{:.0} {kb}", b / 1e3)
    }
}

/// A format in words: «MP3 128», «FLAC 24/96», «Opus 160».
pub fn format_words(f: &crate::source::Format) -> String {
    let label = f.label();
    let name = crate::quality::codec_name(&f.codec);
    match label.split_once(' ') {
        Some((_, rest)) => format!("{name} {rest}"),
        None => name,
    }
}

/// «этот трек», «этот альбом», «остальное»: what a source could not give.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    Track,
    Album,
    Rest,
}

impl What {
    /// Its words in `loc`.
    pub fn words_in(self, loc: Locale) -> &'static str {
        let s = status_in(loc);
        match self {
            Self::Track => s.what_track,
            Self::Album => s.what_album,
            Self::Rest => s.what_rest,
        }
    }
}

/// A source's name in a sentence (the indexer behind «Торренты» is the one
/// that answers or not).
pub fn source_name_in(loc: Locale, key: crate::source::SourceKey) -> &'static str {
    match key {
        crate::source::SourceKey::Torrents => "Prowlarr",
        k => k.display_name_in(loc),
    }
}

/// Where an offer comes from, for the lines: the source (a Soulseek offer's
/// provider is a user's name), a torrent as such.
fn from_words_in(loc: Locale, key: crate::source::SourceKey) -> String {
    match key {
        crate::source::SourceKey::Torrents => status_in(loc).torrent_offer.to_owned(),
        k => k.display_name_in(loc).to_owned(),
    }
}

/// `"в 21:40"` / `"через 5 мин"`, kept so the words are made in any locale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtWords {
    /// The clock time (`"21:40"`), when it is known.
    pub hm: Option<String>,
    /// How long from the moment the line was written, in minutes.
    pub mins: u64,
}

impl AtWords {
    /// Its words in `loc`.
    pub fn words_in(&self, loc: Locale) -> String {
        fmt_at_time_in(loc, self.hm.as_deref(), self.mins)
    }
}

/// Why a source turned an item away, in the words that name it and say what
/// it said.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum Refusal {
    /// «у Qobuz лимит на сегодня».
    DayLimit { source: crate::source::SourceKey },
    /// «Qobuz просит подождать до 03:00» (`until` is the clock time).
    AsksToWait {
        source: crate::source::SourceKey,
        until: Option<String>,
    },
    /// «khinsider закрыт проверкой».
    Check { source: crate::source::SourceKey },
    /// «Soulseek не пускает».
    Blocked { source: crate::source::SourceKey },
    /// «Tidal находит, но не скачивает».
    FindsOnly { source: crate::source::SourceKey },
    /// «Tidal сейчас не отдаёт этот трек».
    GivesNo {
        source: crate::source::SourceKey,
        what: What,
    },
    /// «сервер Qobuz недоступен».
    ServerDown { source: crate::source::SourceKey },
    /// «Tidal не дал скачать».
    NoDownload { source: crate::source::SourceKey },
}

impl Refusal {
    /// Its words in `loc`.
    pub fn words_in(&self, loc: Locale) -> String {
        match self {
            Self::DayLimit { source } => fmt_source_limit_today_in(loc, source_name_in(loc, *source)),
            Self::AsksToWait { source, until } => {
                fmt_source_asks_to_wait_in(loc, source_name_in(loc, *source), until.as_deref())
            }
            Self::Check { source } => fmt_source_check_in(loc, source_name_in(loc, *source)),
            Self::Blocked { source } => fmt_source_blocked_in(loc, source_name_in(loc, *source)),
            Self::FindsOnly { source } => fmt_source_finds_only_in(loc, source_name_in(loc, *source)),
            Self::GivesNo { source, what } => {
                fmt_source_gives_no_in(loc, source_name_in(loc, *source), what.words_in(loc))
            }
            Self::ServerDown { source } => fmt_source_server_down_in(loc, source_name_in(loc, *source)),
            Self::NoDownload { source } => fmt_source_no_download_in(loc, source_name_in(loc, *source)),
        }
    }
}

/// The archive's master a lossy copy was made from, in words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "m", rename_all = "snake_case")]
pub enum MasterWords {
    /// A format in words: «FLAC 24/96».
    Format(crate::source::Format),
    /// The archive's file: its codec, and the rate the check of the copy saw.
    Archive {
        codec: crate::model::Codec,
        sample_rate: Option<u32>,
        hi_res: bool,
    },
}

/// A master's words: «FLAC 24/96», «FLAC Hi-Res», «FLAC».
pub fn fmt_master_words_in(loc: Locale, m: &MasterWords) -> String {
    match m {
        MasterWords::Format(f) => format_words(f),
        MasterWords::Archive { codec, sample_rate, hi_res } => {
            let name = crate::quality::codec_name(codec);
            let unit = status_in(loc).khz_unit;
            match sample_rate {
                Some(r) if *hi_res && *r > 48_000 => {
                    let khz = f64::from(*r) / 1000.0;
                    if khz.fract() == 0.0 {
                        format!("{name} {khz:.0} {unit}")
                    } else {
                        format!("{name} {khz:.1} {unit}")
                    }
                }
                _ if *hi_res => format!("{name} Hi-Res"),
                _ => name,
            }
        }
    }
}

/// A quality label as data, so it says the same in any language.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "q", rename_all = "snake_case")]
pub enum QualityWords {
    /// A file as its check found it ([`crate::quality::label_in`]): «FLAC из
    /// MP3 ~128», «MP3 320». A row's short label keeps the check only when
    /// it is sure.
    Label {
        codec: crate::model::Codec,
        bitrate_kbps: Option<u32>,
        check: Option<crate::quality::CheckSummary>,
    },
    /// A library file as a better copy's words say it (`file_label_in`):
    /// «MP3 128», «FLAC из lossy», «FLAC 24/96 (в библиотеке Opus 158)».
    File {
        codec: crate::model::Codec,
        bitrate_kbps: Option<u32>,
        bit_depth: Option<u8>,
        sample_rate: Option<u32>,
        /// Its `quality_score`: the rank says a lossless file is a fake.
        score: i32,
        /// The archive's master it was made from, when it is a copy.
        master: Option<MasterWords>,
    },
}

/// A quality label in `loc`'s words.
pub fn fmt_quality_words_in(loc: Locale, q: &QualityWords) -> String {
    let s = status_in(loc);
    match q {
        QualityWords::Label { codec, bitrate_kbps, check } => {
            crate::quality::label_in(codec, *bitrate_kbps, check.as_ref(), loc)
        }
        QualityWords::File { codec, bitrate_kbps, bit_depth, sample_rate, score, master } => {
            let label = format_words(&crate::source::Format {
                codec: codec.clone(),
                bitrate_kbps: *bitrate_kbps,
                bit_depth: *bit_depth,
                sample_rate: *sample_rate,
            });
            let own = if !codec.is_lossless() || *score >= crate::model::score::CD_LOSSLESS {
                label
            } else if *score > crate::model::score::FAKE_LOSSLESS {
                format!("{label}{}", s.maybe_lossy)
            } else {
                format!("{label}{}", s.from_lossy)
            };
            match master {
                Some(m) if !codec.is_lossless() && *score >= crate::model::score::CD_LOSSLESS => {
                    format!("{} {}", fmt_master_words_in(loc, m), fmt_in_library_in(loc, &own))
                }
                _ => own,
            }
        }
    }
}

/// What a file really is, as «Подтверди» kept it: the data of a label, or
/// the words a version before it wrote (still read, never written).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SavedQuality {
    Words(String),
    Data(QualityWords),
}

impl SavedQuality {
    /// Its words in `loc`.
    pub fn words_in(&self, loc: Locale) -> String {
        match self {
            Self::Words(w) => w.clone(),
            Self::Data(q) => fmt_quality_words_in(loc, q),
        }
    }
}

/// An offer's line, from the parts it is made of (the whole offer is not
/// kept in a line: its handle belongs to the source).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfferWords {
    pub format: Option<crate::source::Format>,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub provider: String,
    pub source: crate::source::SourceKey,
}

impl OfferWords {
    /// The parts of `offer` its line is made of.
    pub fn of(offer: &crate::source::Offer) -> Self {
        Self {
            format: offer.format.clone(),
            size_bytes: offer.size_bytes,
            seeders: offer.seeders,
            provider: offer.provider.clone(),
            source: offer.source,
        }
    }
}

/// An offer's line: `"FLAC 24/96 · 312 МБ · 14 сид. · RuTracker"`.
pub fn fmt_offer_words_in(loc: Locale, w: &OfferWords) -> String {
    let mut parts = Vec::new();
    if let Some(f) = &w.format {
        parts.push(f.label());
    }
    if let Some(b) = w.size_bytes {
        parts.push(fmt_size_in(loc, b));
    }
    if let Some(s) = w.seeders {
        parts.push(crate::i18n::backend::fmt_seeders_in(loc, s));
    }
    parts.push(if w.provider.is_empty() { w.source.display_name_in(loc).to_owned() } else { w.provider.clone() });
    parts.join(" · ")
}

/// The mark of a line kept as data.
const LINE_MARK: &str = "@line:";

/// A service's error as a line keeps it: its kind and arguments, worded
/// where it is read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum ServiceErrorWords {
    NotConfigured,
    NotLoggedIn,
    PremiumRequired,
    /// «Сервис просит подождать до 02:07»: `until` is the clock time.
    RateLimited { until: Option<String> },
    Unavailable,
    AgeRestricted,
    RegionBlocked,
    Drm,
    Unsupported,
    Http { status: u16 },
    Network,
    Parse,
}

impl ServiceErrorWords {
    /// The error as data; `until` is the clock time a rate limit ends at
    /// (told here, as only the caller knows the time).
    pub fn of(e: &crate::streaming::StreamError, until: Option<String>) -> Self {
        match e {
            crate::streaming::StreamError::NotConfigured => Self::NotConfigured,
            crate::streaming::StreamError::NotLoggedIn => Self::NotLoggedIn,
            crate::streaming::StreamError::PremiumRequired => Self::PremiumRequired,
            crate::streaming::StreamError::RateLimited { .. } => Self::RateLimited { until },
            crate::streaming::StreamError::Unavailable => Self::Unavailable,
            crate::streaming::StreamError::AgeRestricted => Self::AgeRestricted,
            crate::streaming::StreamError::RegionBlocked => Self::RegionBlocked,
            crate::streaming::StreamError::Drm => Self::Drm,
            crate::streaming::StreamError::Unsupported => Self::Unsupported,
            crate::streaming::StreamError::Http { status } => Self::Http { status: *status },
            crate::streaming::StreamError::Network(_) => Self::Network,
            crate::streaming::StreamError::Parse(_) => Self::Parse,
        }
    }

    /// Its words in `loc`.
    pub fn words_in(&self, loc: Locale) -> String {
        use crate::i18n::{ErrorStrings, errors_in};
        let err = errors_in(loc);
        match self {
            Self::NotConfigured => err.stream_not_configured.to_owned(),
            Self::NotLoggedIn => err.stream_not_logged_in.to_owned(),
            Self::PremiumRequired => err.stream_need_premium.to_owned(),
            Self::RateLimited { until } => ErrorStrings::stream_rate_limited_in(loc, until.as_deref()),
            Self::Unavailable => err.stream_unavailable.to_owned(),
            Self::AgeRestricted => err.stream_age_restricted.to_owned(),
            Self::RegionBlocked => err.stream_region_blocked.to_owned(),
            Self::Drm => err.stream_drm.to_owned(),
            Self::Unsupported => err.stream_unsupported.to_owned(),
            Self::Http { status } => ErrorStrings::stream_http_in(loc, *status),
            Self::Network => err.stream_network.to_owned(),
            Self::Parse => err.stream_parse.to_owned(),
        }
    }
}

/// Why a cold-start import did not bring everything, as data: the words
/// are made where the history line is read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColdProblem {
    /// The words themselves: an export file's own complaint, or a row
    /// written before the kinds.
    Words(String),
    Known(ColdProblemKind),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "p", rename_all = "snake_case")]
pub enum ColdProblemKind {
    /// «лайки: {why}».
    Likes { why: ServiceErrorWords },
    /// «история: {why}».
    History { why: ServiceErrorWords },
    /// «не открылись плейлисты: 3».
    PlaylistsNotOpened { count: usize },
    /// «Не получилось прочитать: …»: what an export file's reader said.
    ExportRead { problems: Vec<String> },
    /// «Не нашёл файлов выгрузки Spotify или Google Takeout».
    ExportNotFound,
}

impl ColdProblem {
    /// Its words in `loc`.
    pub fn words_in(&self, loc: Locale) -> String {
        let s = crate::i18n::activity_in(loc);
        match self {
            Self::Words(w) => w.clone(),
            Self::Known(kind) => match kind {
                ColdProblemKind::Likes { why } => {
                    crate::i18n::ErrorStrings::cold_problem_likes_in(loc, &why.words_in(loc))
                }
                ColdProblemKind::History { why } => {
                    crate::i18n::ErrorStrings::cold_problem_history_in(loc, &why.words_in(loc))
                }
                ColdProblemKind::PlaylistsNotOpened { count } => format!("{}: {count}", s.playlists_not_opened),
                ColdProblemKind::ExportRead { problems } => {
                    format!("{}: {}", s.export_read_failed, problems.join("; "))
                }
                ColdProblemKind::ExportNotFound => s.export_not_found.to_owned(),
            },
        }
    }
}

/// Why a cold import failed: its own reasons as data, or the words of an
/// outside failure (a file, the database), and rows written before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColdFailure {
    Words(String),
    Reasons(Vec<ColdProblem>),
}

impl ColdFailure {
    /// Its words in `loc`, the reasons joined as they were said.
    pub fn words_in(&self, loc: Locale) -> String {
        match self {
            Self::Words(w) => w.clone(),
            Self::Reasons(why) => why.iter().map(|p| p.words_in(loc)).collect::<Vec<_>>().join("; "),
        }
    }
}

/// A «Хочу скачать» note or a «История» line, kept as data (a kind and its
/// arguments) and worded when it is read, in the reader's locale.
///
/// It is stored as [`LINE_MARK`] and the JSON of it; a row written before
/// the lines were kept this way holds the words themselves and is shown as
/// it was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", content = "v", rename_all = "snake_case")]
pub enum StoredLine {
    /// «Скачано 3 из 10»: this much of the album is here.
    DoneCount { have: usize, all: usize },
    /// «Скачано · FLAC из MP3 ~320»: a file, as the check found it.
    DoneQuality { quality: QualityWords },
    /// «Раздача: Album (2020) FLAC».
    Torrent { title: String },
    /// «Смотрю, есть ли трек в раздаче: Album (2020) FLAC».
    CheckingTorrent { title: String },
    /// «FLAC · 312 МБ · 14 сид. · Album (2020) FLAC».
    TorrentFound { format: Option<crate::source::Format>, size: u64, seeders: u32, title: String },
    /// An offer's line and the item's name:
    /// «FLAC 24/96 · 312 МБ · 14 сид. · RuTracker · Album».
    Offer { offer: OfferWords, title: String },
    /// «Поищу снова в 21:40».
    SearchAgain { at: AtWords },
    /// «Есть 1 из 10 · Tidal сейчас не отдаёт остальное · через 5 мин спрошу Tidal».
    Later {
        have: Option<(usize, usize)>,
        why: Vec<Refusal>,
        at: AtWords,
        asked: Vec<crate::source::SourceKey>,
    },
    /// Why a source said no: «Tidal сейчас не отдаёт этот трек».
    Refusal { why: Refusal },
    /// The sources' reasons, or «Источники не дали скачать».
    Reasons { why: Vec<Refusal> },
    /// «Поищу снова через неделю».
    Week,
    /// «дальше — Qobuz».
    NextOffer { source: crate::source::SourceKey },
    /// «есть варианты под вопросом — выбери в «Хочу скачать»».
    DoubtfulPick,
    /// «есть другие варианты — в «Подтверди»».
    DoubtfulNext,
    /// «Скачано 3 из 10 · остальных 3 у Qobuz нет».
    Partial { have: usize, all: usize, why: Box<StoredLine> },
    /// «остальных 3 у Qobuz нет».
    Absent { missing: usize, source: crate::source::SourceKey },
    /// «Было MP3 128 — теперь FLAC».
    Replaced { from: Option<QualityWords>, to: Option<QualityWords> },
    /// «Нашёл FLAC вместо MP3 320 · подтверди замену в «Подтверди»».
    FoundInstead { found: Option<crate::source::Format>, have: Option<QualityWords> },
    /// «Нашёл качество лучше для 3 треков · подтверди замену в «Подтверди»».
    BetterFound { count: usize },
    /// «Скачанное не лучше прежних файлов — оставил их».
    NothingBetter,
    /// The file held another song's audio: «В файле от Soulseek другая
    /// песня — «Song» · ищу дальше».
    WrongAudio { from: Option<String>, sounds_like: Option<String>, looks: bool },
    /// An import's title: «Импорт «a.flac»».
    ImportOf { name: String, count: usize },
    /// «Импорт для «Хочу скачать»».
    ImportForWishlist,
    /// «Сохранение с YouTube».
    SavingFromYoutube,
    /// «Качество лучше: Artist — Album».
    BetterQualityTitle { title: String },
    /// «Импорт из Spotify».
    ImportFromService { service: crate::streaming::Service },
    /// «Импорт выгрузки «x.json»».
    ImportExport { name: String },
    /// «Импорт из Inbox».
    ImportFromInbox,
    /// «Импорт»: a job that never said what it was.
    ImportTitle,
    /// «добавлено 3 · дублей 1 · ошибок 2 · отправлено на подтверждение 4».
    ImportSummary { imported: usize, duplicates: usize, left_out: usize, failed: usize, pending: usize },
    /// What an import done for a better copy came to.
    UpgradeSummary {
        imported: usize,
        duplicates: usize,
        left_out: usize,
        failed: usize,
        pending: usize,
        dup: Option<QualityWords>,
    },
    /// «отменено — добавленное убрано».
    Cancelled,
    /// «Скачано 1.2 ГБ» / «Скачано 1.2 ГБ · импорт в библиотеку».
    Downloaded { size: u64, importing: bool },
    /// «Не удалось: 2», «В файле другая песня: «Song»».
    FailedWords { failed: usize, wrong: Vec<String> },
    /// A cold import's outcome: what it found, what was new, what it left
    /// out, and what it could not do.
    ColdSummary { found: [usize; 4], new: [usize; 4], skipped: usize, problems: Vec<ColdProblem> },
    /// «Импорт не удался: …».
    ColdFailed { detail: ColdFailure },
    /// A cold import's failure as the row's error line says it: the reason
    /// alone, without «Импорт не удался».
    ColdReason { detail: ColdFailure },
    /// «Перенос архива FLAC».
    MovedMastersTitle,
    /// «перенесено 3 · проверено BLAKE3 → папка».
    MovedMasters { moved: usize, dir: String },
    /// Two of ours in one line: `a · b`.
    Joined { a: Box<StoredLine>, b: Box<StoredLine> },
    /// The same, its first letter capitalised.
    Capitalised { line: Box<StoredLine> },
}

impl StoredLine {
    /// The text kept in the database: the mark and the line's JSON.
    pub fn encode(&self) -> String {
        format!("{LINE_MARK}{}", serde_json::to_string(self).unwrap_or_default())
    }

    /// The line `s` stands for, when it is one of ours.
    pub fn decode(s: &str) -> Option<Self> {
        serde_json::from_str(s.strip_prefix(LINE_MARK)?).ok()
    }

    /// Its words in `loc`.
    pub fn words_in(&self, loc: Locale) -> String {
        use crate::i18n::{activity::fmt_import_counts_in, activity::fmt_import_skipped_in, activity_in};
        match self {
            Self::DoneCount { have, all } => fmt_done_note_in(loc, Some((*have, *all)), None),
            Self::DoneQuality { quality } => fmt_done_note_in(loc, None, Some(&fmt_quality_words_in(loc, quality))),
            Self::Torrent { title } => fmt_torrent_note_in(loc, title),
            Self::CheckingTorrent { title } => fmt_checking_torrent_in(loc, title),
            Self::TorrentFound { format, size, seeders, title } => {
                let format = format.as_ref().map_or_else(|| "?".to_owned(), |f| f.label());
                fmt_torrent_found_in(loc, &format, &fmt_bytes_in(loc, *size), *seeders, title)
            }
            Self::Offer { offer, title } => format!("{} · {title}", fmt_offer_words_in(loc, offer)),
            Self::SearchAgain { at } => capitalised(&fmt_search_again_in(loc, &at.words_in(loc))),
            Self::Later { have, why, at, asked } => {
                let why: Vec<String> = why.iter().map(|r| r.words_in(loc)).collect();
                let names: Vec<&str> = asked.iter().map(|k| source_name_in(loc, *k)).collect();
                capitalised(&fmt_later_line_in(loc, *have, &why, &at.words_in(loc), &names))
            }
            Self::Refusal { why } => why.words_in(loc),
            Self::Reasons { why } => {
                if why.is_empty() {
                    status_in(loc).sources_gave_nothing.to_owned()
                } else {
                    why.iter().map(|r| r.words_in(loc)).collect::<Vec<_>>().join(", ")
                }
            }
            Self::Week => fmt_search_again_week_in(loc),
            Self::NextOffer { source } => fmt_next_offer_in(loc, &from_words_in(loc, *source)),
            Self::DoubtfulPick => fmt_doubtful_pick_in(loc, crate::i18n::downloads_in(loc).tab_wishlist),
            Self::DoubtfulNext => status_in(loc).next_doubtful.to_owned(),
            Self::Partial { have, all, why } => fmt_downloaded_of_in(loc, *have, *all, &why.words_in(loc)),
            Self::Absent { missing, source } => fmt_others_absent_in(loc, *missing, source_name_in(loc, *source)),
            Self::Replaced { from, to } => {
                let from = from.as_ref().map(|q| fmt_quality_words_in(loc, q));
                let to = to.as_ref().map(|q| fmt_quality_words_in(loc, q));
                fmt_replaced_in(loc, from.as_deref(), to.as_deref())
            }
            Self::FoundInstead { found, have } => {
                let found = found.as_ref().map(|f| f.label());
                let have = have.as_ref().map(|q| fmt_quality_words_in(loc, q));
                fmt_found_instead_in(loc, found.as_deref(), have.as_deref(), crate::i18n::downloads_in(loc).tab_confirm)
            }
            Self::BetterFound { count } => {
                fmt_better_found_in(loc, *count as u64, crate::i18n::downloads_in(loc).tab_confirm)
            }
            Self::NothingBetter => fmt_nothing_better_in(loc),
            Self::WrongAudio { from, sounds_like, looks } => {
                fmt_wrong_audio_in(loc, from.as_deref(), sounds_like.as_deref(), *looks)
            }
            Self::ImportOf { name, count } => fmt_import_of_in(loc, name, *count),
            Self::ImportForWishlist => fmt_import_for_wishlist_in(loc),
            Self::SavingFromYoutube => fmt_saving_from_youtube_in(loc),
            Self::BetterQualityTitle { title } => fmt_better_import_title_in(loc, title),
            Self::ImportFromService { service } => format!("{} {}", activity_in(loc).import_from, service.display_name()),
            Self::ImportExport { name } => {
                format!("{} {}", activity_in(loc).import_export, crate::i18n::activity::fmt_quoted_in(loc, name))
            }
            Self::ImportFromInbox => status_in(loc).import_from_inbox.to_owned(),
            Self::ImportTitle => status_in(loc).import_title.to_owned(),
            Self::ImportSummary { imported, duplicates, left_out, failed, pending } => {
                fmt_import_summary_in(loc, *imported, *duplicates, *left_out, *failed, *pending)
            }
            Self::UpgradeSummary { imported, duplicates, left_out, failed, pending, dup } => {
                let mut s = fmt_upgrade_job_words_in(loc, *imported, *duplicates, *left_out, *failed, dup.as_ref());
                if *pending > 0 {
                    s.push_str(&format!(" · {}", fmt_parked_in(loc, *pending)));
                }
                s
            }
            Self::Cancelled => status_in(loc).cancelled_summary.to_owned(),
            Self::Downloaded { size, importing } => fmt_downloaded_size_in(loc, &fmt_bytes_in(loc, *size), *importing),
            Self::FailedWords { failed, wrong } => fmt_failed_words_in(loc, *failed, wrong).unwrap_or_default(),
            Self::ColdSummary { found, new, skipped, problems } => {
                let mut text = format!(
                    "{}: {} · {}: {}",
                    activity_in(loc).import_found,
                    fmt_import_counts_in(loc, found[0], found[1], found[2], found[3]),
                    activity_in(loc).import_new,
                    fmt_import_counts_in(loc, new[0], new[1], new[2], new[3])
                );
                text.push_str(&fmt_import_skipped_in(loc, *skipped));
                if !problems.is_empty() {
                    let why: Vec<String> = problems.iter().map(|p| p.words_in(loc)).collect();
                    text.push_str(&format!(" · {}: {}", activity_in(loc).import_not_all, why.join("; ")));
                }
                text
            }
            Self::ColdFailed { detail } => format!("{}: {}", activity_in(loc).import_failed, detail.words_in(loc)),
            Self::ColdReason { detail } => detail.words_in(loc),
            Self::MovedMastersTitle => status_in(loc).moved_masters_title.to_owned(),
            Self::MovedMasters { moved, dir } => fmt_masters_move_line_in(loc, *moved, dir),
            Self::Joined { a, b } => format!("{} · {}", a.words_in(loc), b.words_in(loc)),
            Self::Capitalised { line } => capitalised(&line.words_in(loc)),
        }
    }
}

/// A job's field (its title, summary or error) in `loc`: a line kept as
/// data (`"@line:…"`), or the text as it was written. Jobs hold names of
/// their own (a torrent's title, a file's), so a bare note code or the
/// done sentinel is a name here, not a wishlist note.
pub fn job_line_words_in(loc: Locale, s: &str) -> String {
    StoredLine::decode(s).map_or_else(|| s.to_owned(), |line| line.words_in(loc))
}

/// The words a kept line stands for in `loc`: a note code, a line kept as
/// data, or the words a version before carried (which stay as they were
/// read).
pub fn line_words_in(loc: Locale, s: &str) -> String {
    if let Some(line) = StoredLine::decode(s) {
        return line.words_in(loc);
    }
    match wish_note_of(s) {
        Some(WishNote::Done) => fmt_done_note_in(loc, None, None),
        Some(note) => fmt_wish_note_in(loc, note).to_owned(),
        None => s.to_owned(),
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
        use crate::quality::{CheckSummary, SoundOrigin, Verdict};
        let flac = |check| QualityWords::Label { codec: Codec::Flac, bitrate_kbps: None, check: Some(check) };
        let sure = CheckSummary {
            verdict: Verdict::Transcoded,
            origin: SoundOrigin::Lossy { kbps: 128 },
            made_from: None,
            cutoff_hz: Some(16_000),
        };
        let unsure = CheckSummary { origin: SoundOrigin::Lossy { kbps: 320 }, cutoff_hz: Some(20_000), ..sure.clone() };
        let lossy = CheckSummary { origin: SoundOrigin::Unknown, ..sure.clone() };
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some(&flac(sure.clone()))),
            "скачанный FLAC оказался из MP3 ~128 — оставил прежний файл"
        );
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some(&flac(unsure))),
            "скачанный FLAC, возможно, из MP3 320 — оставил прежний файл"
        );
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some(&flac(lossy))),
            "скачанный FLAC оказался из сжатого — оставил прежний файл"
        );
        let mp3 = QualityWords::Label { codec: Codec::Mp3, bitrate_kbps: Some(320), check: None };
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some(&mp3)),
            "скачанный MP3 320 не лучше прежнего — оставил прежний файл"
        );
        // A lossy copy of a fake lossless file.
        let from_fake = QualityWords::Label {
            codec: Codec::Mp3,
            bitrate_kbps: Some(320),
            check: Some(CheckSummary { made_from: Some(Codec::Flac), ..sure.clone() }),
        };
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::Ru, 0, 1, 0, 0, Some(&from_fake)),
            "скачанный MP3 320 оказался из поддельного FLAC — оставил прежний файл"
        );
        assert_eq!(fmt_upgrade_job_words_in(Locale::Ru, 2, 1, 1, 0, None), "заменено 2 · не лучше прежних 1 · не из этого альбома 1");
        assert_eq!(fmt_upgrade_job_words_in(Locale::En, 1, 0, 0, 0, None), "replaced 1");
        assert_eq!(
            fmt_upgrade_job_words_in(Locale::En, 0, 1, 0, 0, Some(&flac(sure.clone()))),
            "the downloaded FLAC turned out to be from MP3 ~128 — kept the previous file"
        );
    }

    /// A note or a history line keeps its kind and arguments, not words: it
    /// says the same in every locale, and a row written before it (words,
    /// or a bare code) still reads as it was.
    #[test]
    fn a_kept_line_is_worded_when_it_is_read() {
        let count = StoredLine::DoneCount { have: 2, all: 3 };
        assert_eq!(line_words_in(Locale::Ru, &count.encode()), "Скачано 2 из 3");
        assert_eq!(line_words_in(Locale::En, &count.encode()), "Downloaded 2 of 3");
        assert_eq!(StoredLine::decode(&count.encode()), Some(count));
        let torrent = StoredLine::Torrent { title: "Album (2020) FLAC".into() };
        assert_eq!(line_words_in(Locale::Ru, &torrent.encode()), "Раздача: Album (2020) FLAC");
        assert_eq!(line_words_in(Locale::En, &torrent.encode()), "Torrent: Album (2020) FLAC");
        let found = StoredLine::TorrentFound {
            format: Some(Format::new(Codec::Flac)),
            size: 312_000_000,
            seeders: 14,
            title: "Album".into(),
        };
        assert_eq!(line_words_in(Locale::Ru, &found.encode()), "FLAC · 312.0 МБ · 14 сид. · Album");
        assert_eq!(line_words_in(Locale::En, &found.encode()), "FLAC · 312.0 MB · 14 seeds · Album");
        // No format in the torrent's name: `?`, as the note always said.
        let unknown = StoredLine::TorrentFound { format: None, size: 820 << 20, seeders: 0, title: "Album".into() };
        assert_eq!(line_words_in(Locale::Ru, &unknown.encode()), "? · 859.8 МБ · 0 сид. · Album");
        let offer = StoredLine::Offer {
            offer: OfferWords::of(&offer()),
            title: "Album".into(),
        };
        assert_eq!(line_words_in(Locale::Ru, &offer.encode()), "FLAC · 312 МБ · 14 сид. · RuTracker · Album");
        // A wait: its clock time and its sources are data.
        let later = StoredLine::Later {
            have: Some((1, 10)),
            why: vec![Refusal::ServerDown { source: SourceKey::Qobuz }, Refusal::GivesNo { source: SourceKey::Tidal, what: What::Rest }],
            at: AtWords { hm: None, mins: 30 },
            asked: vec![SourceKey::Tidal, SourceKey::Soulseek],
        };
        assert_eq!(
            line_words_in(Locale::Ru, &later.encode()),
            "Есть 1 из 10 · сервер Qobuz недоступен, Tidal сейчас не отдаёт остальное · через 30 мин спрошу Tidal, Soulseek"
        );
        assert_eq!(
            line_words_in(Locale::En, &later.encode()),
            "1 of 10 here · the Qobuz server is down, Tidal cannot provide the rest right now · in 30 min asking Tidal, Soulseek"
        );
        let week = StoredLine::Capitalised {
            line: Box::new(StoredLine::Joined {
                a: Box::new(StoredLine::Reasons { why: vec![] }),
                b: Box::new(StoredLine::Week),
            }),
        };
        assert_eq!(line_words_in(Locale::Ru, &week.encode()), "Источники не дали скачать · поищу снова через неделю");
        // Old rows: the words themselves, and the codes before them.
        assert_eq!(line_words_in(Locale::Ru, "Скачано 3 из 10"), "Скачано 3 из 10");
        assert_eq!(line_words_in(Locale::En, "Скачано 3 из 10"), "Скачано 3 из 10");
        assert_eq!(line_words_in(Locale::Ru, "files_wait"), RU.note_files_wait);
        assert_eq!(line_words_in(Locale::En, NOTE_DONE), "Downloaded");
        assert_eq!(line_words_in(Locale::Ru, LEGACY_NOTE_FILES_WAIT), RU.note_files_wait);
    }

    /// A job's row keeps its kind and counts: the title, the summary and the
    /// error say the same in every locale.
    #[test]
    fn a_kept_job_line_is_worded_when_it_is_read() {
        let title = StoredLine::ImportForWishlist;
        assert_eq!(line_words_in(Locale::Ru, &title.encode()), "Импорт для «Хочу скачать»");
        assert_eq!(line_words_in(Locale::En, &title.encode()), "Import for Wishlist");
        let service = StoredLine::ImportFromService { service: crate::streaming::Service::Spotify };
        assert_eq!(line_words_in(Locale::Ru, &service.encode()), "Импорт из Spotify");
        assert_eq!(line_words_in(Locale::En, &service.encode()), "Import from Spotify");
        let export = StoredLine::ImportExport { name: "takeout.json".into() };
        assert_eq!(line_words_in(Locale::Ru, &export.encode()), "Импорт выгрузки «takeout.json»");
        assert_eq!(line_words_in(Locale::En, &export.encode()), "Import exported data “takeout.json”");
        let summary = StoredLine::ImportSummary { imported: 3, duplicates: 1, left_out: 0, failed: 0, pending: 2 };
        assert_eq!(line_words_in(Locale::Ru, &summary.encode()), "добавлено 3 · дублей 1 · отправлено на подтверждение 2");
        let downloaded = StoredLine::Downloaded { size: 1_200_000_000, importing: true };
        assert_eq!(line_words_in(Locale::Ru, &downloaded.encode()), "Скачано 1.2 ГБ · импорт в библиотеку");
        assert_eq!(line_words_in(Locale::En, &downloaded.encode()), "Downloaded 1.2 GB · importing into the library");
        let failed = StoredLine::FailedWords { failed: 2, wrong: vec!["Song".into()] };
        assert_eq!(line_words_in(Locale::Ru, &failed.encode()), "В файле другая песня: «Song» · не удалось ещё: 1");
        assert_eq!(line_words_in(Locale::En, &failed.encode()), "Wrong song in file: “Song” · 1 more failed");
        let cold = StoredLine::ColdSummary {
            found: [3, 2, 0, 1],
            new: [1, 0, 0, 0],
            skipped: 2,
            problems: vec![ColdProblem::Words("a playlist".into())],
        };
        assert_eq!(
            line_words_in(Locale::Ru, &cold.encode()),
            "Найдено: 3 прослушивания, 2 лайка, 1 плейлист · новых: 1 прослушивание · пропущено 2 плейлиста не с музыкой · не всё: a playlist"
        );
        // A problem keeps its kind and arguments: it says the same in both
        // locales, the wait's time being the one the job wrote.
        let problems = vec![
            ColdProblem::Known(ColdProblemKind::Likes { why: ServiceErrorWords::RateLimited { until: Some("02:07".into()) } }),
            ColdProblem::Known(ColdProblemKind::PlaylistsNotOpened { count: 3 }),
            ColdProblem::Known(ColdProblemKind::ExportNotFound),
        ];
        let failed = StoredLine::ColdReason { detail: ColdFailure::Reasons(problems) };
        assert_eq!(
            line_words_in(Locale::Ru, &failed.encode()),
            "лайки: Сервис просит подождать до 02:07; не открылись плейлисты: 3; Не нашёл файлов выгрузки Spotify или Google Takeout"
        );
        assert_eq!(
            line_words_in(Locale::En, &failed.encode()),
            "likes: Please wait until 02:07 before using the service; playlists could not be opened: 3; No Spotify or Google Takeout export files found"
        );
        // A row of the older kind (the words, not the kinds) still reads.
        let old = StoredLine::ColdSummary { found: [0; 4], new: [0; 4], skipped: 0, problems: vec![ColdProblem::Words("x".into())] };
        assert_eq!(line_words_in(Locale::Ru, &old.encode()), "Найдено: ничего · новых: ничего · не всё: x");
        // The failure line keeps «Импорт не удался» in front of the reason.
        let failed = StoredLine::ColdFailed { detail: ColdFailure::Words("disk full".into()) };
        assert_eq!(line_words_in(Locale::Ru, &failed.encode()), "Импорт не удался: disk full");
        assert_eq!(line_words_in(Locale::En, &failed.encode()), "Import failed: disk full");
    }
}
