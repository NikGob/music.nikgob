//! One-time rewrite of the status texts older versions wrote into the
//! database: `wishlist.note` and the `job_log` (a title, a subtitle, an
//! error) become the keys they are read as now (see
//! `nikgob_core::i18n::status`): a note code, the «Скачано» sentinel, or
//! `@line:` and the JSON of a [`StoredLine`].
//!
//! A text is rewritten only when every word of it is one this app wrote and
//! the key renders back to exactly that text in the locale it was written
//! in; anything else stays as it is. It runs once per library, its mark in
//! `meta` like the other one-time repairs ([`crate::repairs`]).

use nikgob_core::i18n::activity::{fmt_import_counts_in, fmt_import_skipped_in, fmt_quoted_in};
use nikgob_core::i18n::backend::fmt_seeders_in;
use nikgob_core::i18n::status::{
    AtWords, ColdFailure, ColdProblem, ColdProblemKind, MasterWords, OfferWords, QualityWords, Refusal,
    ServiceErrorWords, StoredLine, What, WishNote, capitalised, fmt_added_in, fmt_at_time_in, fmt_better_found_in,
    fmt_better_import_title_in, fmt_bytes_in, fmt_checking_torrent_in, fmt_done_note_in, fmt_doubtful_pick_in,
    fmt_downloaded_of_in, fmt_downloaded_size_in, fmt_duplicates_in, fmt_errors_in, fmt_failed_words_in,
    fmt_found_instead_in, fmt_import_for_wishlist_in, fmt_import_of_in, fmt_in_library_in, fmt_later_line_in,
    fmt_left_out_in, fmt_masters_move_line_in, fmt_next_offer_in, fmt_nothing_better_in, fmt_others_absent_in,
    fmt_parked_in, fmt_quality_words_in, fmt_replaced_in, fmt_saving_from_youtube_in, fmt_search_again_in,
    fmt_search_again_week_in, fmt_size_in, fmt_source_asks_to_wait_in, fmt_source_blocked_in, fmt_source_check_in,
    fmt_source_finds_only_in, fmt_source_gives_no_in, fmt_source_limit_today_in, fmt_source_no_download_in,
    fmt_source_server_down_in, fmt_torrent_found_in, fmt_torrent_note_in, fmt_upgrade_job_words_in, fmt_wrong_audio_in,
    format_words, line_words_in, source_name_in, status_in,
};
use nikgob_core::i18n::{ErrorStrings, Locale, activity_in, downloads_in, errors_in};
use nikgob_core::model::{Codec, score};
use nikgob_core::quality::{CheckSummary, SoundOrigin, Verdict, codec_name};
use nikgob_core::source::{Format, SourceKey};
use nikgob_core::streaming::Service;
use rusqlite::{Transaction, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::now_ms;

/// `meta` key of [`Library::translate_legacy_status_texts`].
pub(crate) const LEGACY_TEXTS_KEY: &str = "legacy_status_texts.v1";

/// Stands in for a name in a sample line (no real text has it).
const MARK: &str = "\u{1}";
/// The second such mark.
const MARK2: &str = "\u{2}";
/// The number sample lines are rendered with.
const NUM: usize = 987_654_321;
/// [`NUM`] as it is rendered.
const NUM_TEXT: &str = "987654321";

/// Which field of a job the text is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Subtitle,
    Error,
}

impl Library {
    /// Once per library (`legacy_status_texts.v1`): the texts older versions
    /// wrote into `wishlist.note` and the `job_log` become the keys they are
    /// read as now. What is not recognized stays. Returns how many rows it
    /// rewrote.
    pub fn translate_legacy_status_texts(&self) -> Result<usize> {
        Ok(self
            .repair(LEGACY_TEXTS_KEY, |tx| {
                let notes = rewrite_notes(tx)?;
                let jobs = rewrite_jobs(tx)?;
                note!("library: old status texts: {notes} wishlist notes and {jobs} job rows kept as keys");
                Ok((notes + jobs, notes + jobs))
            })?
            .unwrap_or(0))
    }
}

/// `wishlist.note` values become keys; a note is user data, so its
/// `updated_at` follows.
fn rewrite_notes(tx: &Transaction<'_>) -> Result<usize> {
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, note FROM wishlist WHERE note IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut n = 0;
    for (id, text) in rows {
        let Some(key) = note_key(&text).filter(|key| *key != text) else {
            continue;
        };
        tx.execute("UPDATE wishlist SET note = ?2, updated_at = ?3 WHERE id = ?1", params![id, key, now_ms()])?;
        note!("library: «Хочу скачать» {id}: «{text}» is kept as a key");
        n += 1;
    }
    Ok(n)
}

/// A history row's title, subtitle and error become keys in place.
fn rewrite_jobs(tx: &Transaction<'_>) -> Result<usize> {
    let rows: Vec<(i64, String, String, String, Option<String>)> = {
        let mut stmt = tx.prepare("SELECT seq, kind, title, subtitle, error FROM job_log")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut n = 0;
    for (seq, kind, title, subtitle, error) in rows {
        let new_title = job_key(&kind, Field::Title, &title).filter(|key| *key != title);
        let new_subtitle = job_key(&kind, Field::Subtitle, &subtitle).filter(|key| *key != subtitle);
        let new_error = error
            .as_ref()
            .and_then(|text| job_key(&kind, Field::Error, text))
            .filter(|key| Some(key) != error.as_ref());
        if new_title.is_none() && new_subtitle.is_none() && new_error.is_none() {
            continue;
        }
        tx.execute(
            "UPDATE job_log SET title = ?2, subtitle = ?3, error = ?4 WHERE seq = ?1",
            params![
                seq,
                new_title.as_deref().unwrap_or(title.as_str()),
                new_subtitle.as_deref().unwrap_or(subtitle.as_str()),
                new_error.as_deref().or(error.as_deref()),
            ],
        )?;
        note!("library: history {seq}: its words are kept as keys");
        n += 1;
    }
    Ok(n)
}

/// The key the note `text` stands for (a code, or a line's `@line:`), when
/// every word of it was written by this app.
fn note_key(text: &str) -> Option<String> {
    if WishNote::from_code(text).is_some() || StoredLine::decode(text).is_some() {
        return None;
    }
    [Locale::Ru, Locale::En].into_iter().find_map(|loc| note_in(text, loc))
}

fn note_in(text: &str, loc: Locale) -> Option<String> {
    let notes = [
        WishNote::FilesWait,
        WishNote::NotFound,
        WishNote::HasCopy,
        WishNote::Searching,
        WishNote::SourcesSilent,
        WishNote::FoundBetter,
        WishNote::NotSure,
        WishNote::OwnTorrent,
        WishNote::OwnFile,
        WishNote::SavingYoutube,
        WishNote::SaveFailed,
        WishNote::NothingBetter,
        WishNote::KeptFile,
        WishNote::KeptFiles,
        WishNote::FilesBack,
        WishNote::Done,
    ];
    if let Some(note) = notes.into_iter().find(|note| line_words_in(loc, note.code()) == text) {
        return Some(note.code().to_owned());
    }
    let line = note_line(text, loc)?;
    (line.words_in(loc) == text).then(|| line.encode())
}

fn note_line(text: &str, loc: Locale) -> Option<StoredLine> {
    if let Some(title) = one(text, &fmt_torrent_note_in(loc, MARK)) {
        return Some(StoredLine::Torrent { title });
    }
    if let Some(title) = one(text, &fmt_checking_torrent_in(loc, MARK)) {
        return Some(StoredLine::CheckingTorrent { title });
    }
    // A torrent's line ends with its name; an offer's, with the provider
    // and then the item's name.
    let with_provider = text.splitn(4, " · ").nth(3).is_some_and(|tail| tail.contains(" · "));
    let found = if with_provider {
        offer_in(text, loc).or_else(|| torrent_found_in(text, loc))
    } else {
        torrent_found_in(text, loc).or_else(|| offer_in(text, loc))
    };
    found
        .or_else(|| search_again_in(text, loc))
        .or_else(|| done_count_in(text, loc))
        .or_else(|| done_quality_in(text, loc))
        .or_else(|| replaced_in(text, loc))
        .or_else(|| later_in(text, loc))
}

/// The key a job's field stands for, when every word of it was written by
/// this app. A torrent's title (its name) and a wished item's title (a name
/// of its own) are never rewritten, nor is a torrent's own error.
fn job_key(kind: &str, field: Field, text: &str) -> Option<String> {
    if StoredLine::decode(text).is_some() {
        return None;
    }
    if field == Field::Title && kind != "import" {
        return None;
    }
    if field == Field::Error && kind == "torrent" {
        return None;
    }
    for loc in [Locale::Ru, Locale::En] {
        let line = match field {
            Field::Title => title_in(text, loc),
            Field::Subtitle if kind == "torrent" => downloaded_in(text, loc),
            _ => job_line_in(text, loc),
        };
        if let Some(line) = line
            && line.words_in(loc) == text
        {
            return Some(line.encode());
        }
    }
    None
}

// --------------------------------------------------------------- titles

fn title_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let s = status_in(loc);
    if text == s.moved_masters_title {
        return Some(StoredLine::MovedMastersTitle);
    }
    if text == s.import_from_inbox {
        return Some(StoredLine::ImportFromInbox);
    }
    if text == s.import_title {
        return Some(StoredLine::ImportTitle);
    }
    if text == fmt_import_for_wishlist_in(loc) {
        return Some(StoredLine::ImportForWishlist);
    }
    if text == fmt_saving_from_youtube_in(loc) {
        return Some(StoredLine::SavingFromYoutube);
    }
    if let Some(title) = one(text, &fmt_better_import_title_in(loc, MARK)) {
        return Some(StoredLine::BetterQualityTitle { title });
    }
    let a = activity_in(loc);
    for service in Service::ALL {
        if text == format!("{} {}", a.import_from, service.display_name()) {
            return Some(StoredLine::ImportFromService { service: *service });
        }
    }
    if let Some(name) = one(text, &format!("{} {}", a.import_export, fmt_quoted_in(loc, MARK))) {
        return Some(StoredLine::ImportExport { name });
    }
    // «Импорт «a.flac»», «Импорт 3 объектов».
    if let Some(name) = one(text, &fmt_import_of_in(loc, MARK, 1)) {
        return Some(StoredLine::ImportOf { name, count: 1 });
    }
    let count = number(text, &fmt_import_of_in(loc, "", NUM))?;
    Some(StoredLine::ImportOf { name: String::new(), count })
}

// ------------------------------------------------------- jobs and notes

fn job_line_in(text: &str, loc: Locale) -> Option<StoredLine> {
    import_summary_in(text, loc)
        .or_else(|| upgrade_settle_in(text, loc))
        .or_else(|| upgrade_summary_in(text, loc))
        .or_else(|| cancelled_in(text, loc))
        .or_else(|| moved_masters_in(text, loc))
        .or_else(|| downloaded_in(text, loc))
        .or_else(|| cold_failed_in(text, loc))
        .or_else(|| cold_summary_in(text, loc))
        .or_else(|| cold_reason_in(text, loc))
        .or_else(|| failed_words_in(text, loc))
        .or_else(|| later_in(text, loc))
        .or_else(|| week_in(text, loc))
        .or_else(|| refusal_next_in(text, loc))
        .or_else(|| partial_in(text, loc))
        .or_else(|| replaced_in(text, loc))
        .or_else(|| found_instead_in(text, loc))
        .or_else(|| better_found_in(text, loc))
        .or_else(|| nothing_better_in(text, loc))
        .or_else(|| wrong_audio_in(text, loc))
}

fn cancelled_in(text: &str, loc: Locale) -> Option<StoredLine> {
    (text == status_in(loc).cancelled_summary).then_some(StoredLine::Cancelled)
}

fn moved_masters_in(text: &str, loc: Locale) -> Option<StoredLine> {
    if let Some(dir) = one(text, &fmt_masters_move_line_in(loc, 0, MARK)) {
        return Some(StoredLine::MovedMasters { moved: 0, dir });
    }
    let (moved, dir) = marked_and_numbered(text, &fmt_masters_move_line_in(loc, NUM, MARK))?;
    Some(StoredLine::MovedMasters { moved: moved.parse().ok()?, dir })
}

fn downloaded_in(text: &str, loc: Locale) -> Option<StoredLine> {
    if let Some(size) = one(text, &fmt_downloaded_size_in(loc, MARK, true)) {
        return Some(StoredLine::Downloaded { size: bytes_in(&size, loc)?, importing: true });
    }
    let size = one(text, &fmt_downloaded_size_in(loc, MARK, false))?;
    Some(StoredLine::Downloaded { size: bytes_in(&size, loc)?, importing: false })
}

/// «добавлено 3 · дублей 1 · не из этого альбома 2 · ошибок 2 · отправлено
/// на подтверждение 4».
fn import_summary_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let mut parts = text.split(" · ");
    let imported = number(parts.next()?, &fmt_added_in(loc, NUM))?;
    let (mut duplicates, mut left_out, mut failed, mut pending) = (0, 0, 0, 0);
    let mut next = parts.next();
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_duplicates_in(loc, NUM))
    {
        duplicates = n;
        next = parts.next();
    }
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_left_out_in(loc, NUM))
    {
        left_out = n;
        next = parts.next();
    }
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_errors_in(loc, NUM))
    {
        failed = n;
        next = parts.next();
    }
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_parked_in(loc, NUM))
    {
        pending = n;
        next = parts.next();
    }
    if next.is_some() {
        return None;
    }
    Some(StoredLine::ImportSummary { imported, duplicates, left_out, failed, pending })
}

/// What an import done for a better copy came to («заменено 3 · не лучше
/// прежних 1 · не из этого альбома 2»).
fn upgrade_summary_in(text: &str, loc: Locale) -> Option<StoredLine> {
    if text == fmt_upgrade_job_words_in(loc, 0, 0, 0, 0, None) {
        return Some(StoredLine::UpgradeSummary {
            imported: 0,
            duplicates: 0,
            left_out: 0,
            failed: 0,
            pending: 0,
            dup: None,
        });
    }
    let mut parts = text.split(" · ");
    let (mut imported, mut duplicates, mut dup) = (0, 0, None);
    let (mut left_out, mut failed, mut pending) = (0, 0, 0);
    let mut next = parts.next();
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_upgrade_job_words_in(loc, NUM, 0, 0, 0, None))
    {
        imported = n;
        next = parts.next();
    }
    if let Some(seg) = next
        && let Some((n, q)) = dup_in(seg, imported, loc)
    {
        duplicates = n;
        dup = q;
        next = parts.next();
    }
    if let Some(seg) = next {
        if imported == 0 && duplicates == 0 && seg == fmt_upgrade_job_words_in(loc, 0, 0, 1, 0, None) {
            // The words never named the count: any of them says them alike.
            left_out = 1;
            next = parts.next();
        } else if let Some(n) = number(seg, &fmt_left_out_in(loc, NUM)) {
            left_out = n;
            next = parts.next();
        }
    }
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_errors_in(loc, NUM))
    {
        failed = n;
        next = parts.next();
    }
    if let Some(seg) = next
        && let Some(n) = number(seg, &fmt_parked_in(loc, NUM))
    {
        pending = n;
        next = parts.next();
    }
    if next.is_some() {
        return None;
    }
    Some(StoredLine::UpgradeSummary { imported, duplicates, left_out, failed, pending, dup })
}

/// The part of an upgrade's summary that says what a copy that came and was
/// no better really is.
fn dup_in(text: &str, imported: usize, loc: Locale) -> Option<(usize, Option<QualityWords>)> {
    if imported > 0 {
        // «не лучше прежних 2» names no quality.
        let n = (1..=99).find(|n| fmt_upgrade_job_words_in(loc, imported, *n, 0, 0, None) == text)?;
        return Some((n, None));
    }
    if let Some(n) = (2..=99).find(|n| fmt_upgrade_job_words_in(loc, 0, *n, 0, 0, None) == text) {
        return Some((n, None));
    }
    for q in quality_candidates(text) {
        if fmt_upgrade_job_words_in(loc, 0, 1, 0, 0, Some(&q)) == text {
            return Some((1, Some(q)));
        }
    }
    (fmt_upgrade_job_words_in(loc, 0, 1, 0, 0, None) == text).then_some((1, None))
}

/// A finished better copy's line with what came of it («Скачанный … ·
/// дальше — Qobuz»).
fn upgrade_settle_in(text: &str, loc: Locale) -> Option<StoredLine> {
    for candidate in [text.to_owned(), lower_first(text)] {
        let Some((summary, then)) = candidate.rsplit_once(" · ") else {
            continue;
        };
        let Some(summary) = upgrade_summary_in(summary, loc) else {
            continue;
        };
        let then = if then == status_in(loc).next_doubtful {
            StoredLine::DoubtfulNext
        } else {
            let Some(then) = next_in(then, loc) else {
                continue;
            };
            then
        };
        return Some(StoredLine::Capitalised {
            line: Box::new(StoredLine::Joined { a: Box::new(summary), b: Box::new(then) }),
        });
    }
    None
}

fn failed_words_in(text: &str, loc: Locale) -> Option<StoredLine> {
    if let Some(failed) = number(text, &fmt_failed_words_in(loc, NUM, &[])?) {
        return Some(StoredLine::FailedWords { failed, wrong: Vec::new() });
    }
    // «В файле другая песня: «A», «B»[ · не удалось ещё: 1]».
    let sample = fmt_failed_words_in(loc, 0, &[MARK.to_owned(), MARK2.to_owned()])?;
    let (pre, rest) = sample.split_once(MARK)?;
    let (mid, suf) = rest.split_once(MARK2)?;
    let with_count = fmt_failed_words_in(loc, 2, &[MARK.to_owned()])?;
    let (_, tail) = with_count.split_once(&fmt_quoted_in(loc, MARK))?;
    let digits_at = tail.find(|c: char| c.is_ascii_digit())?;
    let (tail_pre, _) = tail.split_at(digits_at);
    let (head, other) = match text.rsplit_once(tail_pre) {
        Some((head, digits)) => (head, digits.parse::<usize>().ok()?),
        None => (text, 0),
    };
    let inner = head.strip_prefix(pre)?.strip_suffix(suf)?;
    let wrong: Vec<String> = inner.split(mid).map(str::to_owned).collect();
    Some(StoredLine::FailedWords { failed: wrong.len() + other, wrong })
}

fn later_in(text: &str, loc: Locale) -> Option<StoredLine> {
    for candidate in [text.to_owned(), lower_first(text)] {
        if let Some(line) = later_body(&candidate, loc) {
            return Some(StoredLine::Capitalised { line: Box::new(line) });
        }
    }
    None
}

fn later_body(text: &str, loc: Locale) -> Option<StoredLine> {
    let parts: Vec<&str> = text.split(" · ").collect();
    if parts.len() < 2 {
        return None;
    }
    let (at, asked) = ask_part(parts.last()?, loc)?;
    let mut rest = &parts[..parts.len() - 1];
    let mut have = None;
    if let Some((n, all)) = rest.first().and_then(|part| have_part(part, loc)) {
        have = Some((n, all));
        rest = &rest[1..];
    }
    let why = reasons(&rest.join(", "), loc)?;
    Some(StoredLine::Later { have, why, at, asked })
}

/// «в 21:40 спрошу Qobuz» or «попробую снова через 5 мин».
fn ask_part(text: &str, loc: Locale) -> Option<(AtWords, Vec<SourceKey>)> {
    let sample = fmt_later_line_in(loc, None, &[], MARK, &[MARK2]);
    let part = sample.rsplit(" · ").next()?;
    let (pre, rest) = part.split_once(MARK)?;
    let (mid, suf) = rest.split_once(MARK2)?;
    if let Some(body) = text.strip_prefix(pre).and_then(|t| t.strip_suffix(suf))
        && let Some((at, names)) = body.split_once(mid)
    {
        let names: Vec<SourceKey> = names.split(", ").map(|name| source_key(name, loc)).collect::<Option<_>>()?;
        return Some((at_words(at, loc)?, names));
    }
    let sample = fmt_later_line_in(loc, None, &[], MARK, &[]);
    let part = sample.rsplit(" · ").next()?;
    let (pre, suf) = part.split_once(MARK)?;
    let at = text.strip_prefix(pre)?.strip_suffix(suf)?;
    Some((at_words(at, loc)?, Vec::new()))
}

/// «Есть 3 из 10».
fn have_part(text: &str, loc: Locale) -> Option<(usize, usize)> {
    let sample = fmt_later_line_in(loc, Some((NUM, NUM)), &[], MARK, &[]);
    two_numbers(text, sample.split(" · ").next()?)
}

/// A list of refusals joined by «, », or the words of sources that gave
/// nothing.
fn reasons(text: &str, loc: Locale) -> Option<Vec<Refusal>> {
    if text == status_in(loc).sources_gave_nothing {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let (refusal, tail) = refusal_at(rest, loc)?;
        out.push(refusal);
        if tail.is_empty() {
            return Some(out);
        }
        rest = tail.strip_prefix(", ")?;
    }
}

fn refusal_in(text: &str, loc: Locale) -> Option<Refusal> {
    let (refusal, tail) = refusal_at(text, loc)?;
    tail.is_empty().then_some(refusal)
}

fn refusal_at<'a>(text: &'a str, loc: Locale) -> Option<(Refusal, &'a str)> {
    for key in SourceKey::ALL {
        let name = source_name_in(loc, key);
        let fixed = [
            (fmt_source_limit_today_in(loc, name), Refusal::DayLimit { source: *key }),
            (fmt_source_asks_to_wait_in(loc, name, None), Refusal::AsksToWait { source: *key, until: None }),
            (fmt_source_check_in(loc, name), Refusal::Check { source: *key }),
            (fmt_source_blocked_in(loc, name), Refusal::Blocked { source: *key }),
            (fmt_source_finds_only_in(loc, name), Refusal::FindsOnly { source: *key }),
            (fmt_source_server_down_in(loc, name), Refusal::ServerDown { source: *key }),
            (fmt_source_no_download_in(loc, name), Refusal::NoDownload { source: *key }),
        ];
        for (words, refusal) in fixed {
            if let Some(tail) = text.strip_prefix(&words)
                && (tail.is_empty() || tail.starts_with(", "))
            {
                return Some((refusal, tail));
            }
        }
        for what in [What::Track, What::Album, What::Rest] {
            let words = fmt_source_gives_no_in(loc, name, what.words_in(loc));
            if let Some(tail) = text.strip_prefix(&words)
                && (tail.is_empty() || tail.starts_with(", "))
            {
                return Some((Refusal::GivesNo { source: *key, what }, tail));
            }
        }
        // «Qobuz просит подождать до 03:00»: the clock is the one part not
        // in the words themselves.
        let sample = fmt_source_asks_to_wait_in(loc, name, Some(MARK));
        if let Some((pre, _)) = sample.split_once(MARK)
            && let Some(rest) = text.strip_prefix(pre)
        {
            let (until, tail) = match rest.find(", ") {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, ""),
            };
            if is_hm(until) {
                return Some((Refusal::AsksToWait { source: *key, until: Some(until.to_owned()) }, tail));
            }
        }
    }
    None
}

fn week_in(text: &str, loc: Locale) -> Option<StoredLine> {
    for candidate in [text.to_owned(), lower_first(text)] {
        let Some((why, week)) = candidate.rsplit_once(" · ") else {
            continue;
        };
        if week != fmt_search_again_week_in(loc) {
            continue;
        }
        let why = reasons(why, loc)?;
        return Some(StoredLine::Capitalised {
            line: Box::new(StoredLine::Joined {
                a: Box::new(StoredLine::Reasons { why }),
                b: Box::new(StoredLine::Week),
            }),
        });
    }
    None
}

fn refusal_next_in(text: &str, loc: Locale) -> Option<StoredLine> {
    for candidate in [text.to_owned(), lower_first(text)] {
        let Some((why, then)) = candidate.rsplit_once(" · ") else {
            continue;
        };
        let (Some(why), Some(then)) = (refusal_in(why, loc), next_in(then, loc)) else {
            continue;
        };
        return Some(StoredLine::Capitalised {
            line: Box::new(StoredLine::Joined { a: Box::new(StoredLine::Refusal { why }), b: Box::new(then) }),
        });
    }
    None
}

fn next_in(text: &str, loc: Locale) -> Option<StoredLine> {
    if text == fmt_doubtful_pick_in(loc, downloads_in(loc).tab_wishlist) {
        return Some(StoredLine::DoubtfulPick);
    }
    let from = one(text, &fmt_next_offer_in(loc, MARK))?;
    let source = if from == status_in(loc).torrent_offer { SourceKey::Torrents } else { source_key(&from, loc)? };
    Some(StoredLine::NextOffer { source })
}

fn partial_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let sample = fmt_downloaded_of_in(loc, NUM, NUM, MARK);
    let (pre, rest) = sample.split_once(NUM_TEXT)?;
    let (mid, rest) = rest.split_once(NUM_TEXT)?;
    let (suf, _) = rest.split_once(MARK)?;
    let rest = text.strip_prefix(pre)?;
    let (have, rest) = rest.split_once(mid)?;
    let (all, why) = rest.split_once(suf)?;
    let why = match absent_in(why, loc) {
        Some((missing, source)) => StoredLine::Absent { missing, source },
        None => StoredLine::Refusal { why: refusal_in(why, loc)? },
    };
    Some(StoredLine::Partial { have: have.parse().ok()?, all: all.parse().ok()?, why: Box::new(why) })
}

/// «остальных 3 у Qobuz нет».
fn absent_in(text: &str, loc: Locale) -> Option<(usize, SourceKey)> {
    let sample = fmt_others_absent_in(loc, NUM, MARK);
    let (a, b) = marked_and_numbered(text, &sample)?;
    let (missing, name) = if sample.find(MARK)? < sample.find(NUM_TEXT)? { (b, a) } else { (a, b) };
    Some((missing.parse().ok()?, source_key(&name, loc)?))
}

fn search_again_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let at = one(text, &capitalised(&fmt_search_again_in(loc, MARK)))?;
    let at = at_words(&at, loc)?;
    Some(StoredLine::Capitalised { line: Box::new(StoredLine::SearchAgain { at }) })
}

/// «в 21:40» or «через 5 мин».
fn at_words(text: &str, loc: Locale) -> Option<AtWords> {
    if is_hm(text) {
        return Some(AtWords { hm: Some(text.to_owned()), mins: 0 });
    }
    let mins = number(text, &fmt_at_time_in(loc, None, NUM as u64))?;
    Some(AtWords { hm: None, mins: mins as u64 })
}

fn done_count_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let (have, all) = two_numbers(text, &fmt_done_note_in(loc, Some((NUM, NUM)), None))?;
    Some(StoredLine::DoneCount { have, all })
}

fn done_quality_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let quality = one(text, &fmt_done_note_in(loc, None, Some(MARK)))?;
    Some(StoredLine::DoneQuality { quality: quality_words(&quality, loc)? })
}

fn replaced_in(text: &str, loc: Locale) -> Option<StoredLine> {
    if text == fmt_replaced_in(loc, None, None) {
        return Some(StoredLine::Replaced { from: None, to: None });
    }
    if let Some((from, to)) = two(text, &fmt_replaced_in(loc, Some(MARK), Some(MARK2))) {
        let (from, to) = (quality_words(&from, loc)?, quality_words(&to, loc)?);
        return Some(StoredLine::Replaced { from: Some(from), to: Some(to) });
    }
    let to = one(text, &fmt_replaced_in(loc, None, Some(MARK)))?;
    Some(StoredLine::Replaced { from: None, to: Some(quality_words(&to, loc)?) })
}

fn found_instead_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let tab = downloads_in(loc).tab_confirm;
    if text == fmt_found_instead_in(loc, None, None, tab) {
        return Some(StoredLine::FoundInstead { found: None, have: None });
    }
    if let Some(found) = one(text, &fmt_found_instead_in(loc, Some(MARK), None, tab)) {
        return Some(StoredLine::FoundInstead { found: Some(format_of(&found)?), have: None });
    }
    let (found, have) = two(text, &fmt_found_instead_in(loc, Some(MARK), Some(MARK2), tab))?;
    Some(StoredLine::FoundInstead { found: Some(format_of(&found)?), have: Some(quality_words(&have, loc)?) })
}

fn better_found_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let tab = downloads_in(loc).tab_confirm;
    // The word of the count changes with it («для 1 трека», «для 3 треков»).
    for sample in [fmt_better_found_in(loc, 1, tab), fmt_better_found_in(loc, NUM as u64, tab)] {
        if let Some(count) = number(text, &sample) {
            return Some(StoredLine::BetterFound { count });
        }
    }
    None
}

fn nothing_better_in(text: &str, loc: Locale) -> Option<StoredLine> {
    (text == fmt_nothing_better_in(loc)).then_some(StoredLine::NothingBetter)
}

fn wrong_audio_in(text: &str, loc: Locale) -> Option<StoredLine> {
    for looks in [true, false] {
        if text == fmt_wrong_audio_in(loc, None, None, looks) {
            return Some(StoredLine::WrongAudio { from: None, sounds_like: None, looks });
        }
        if let Some(from) = one(text, &fmt_wrong_audio_in(loc, Some(MARK), None, looks)) {
            return Some(StoredLine::WrongAudio { from: Some(from), sounds_like: None, looks });
        }
        if let Some(song) = one(text, &fmt_wrong_audio_in(loc, None, Some(MARK), looks)) {
            return Some(StoredLine::WrongAudio { from: None, sounds_like: Some(song), looks });
        }
        if let Some((from, song)) = two(text, &fmt_wrong_audio_in(loc, Some(MARK), Some(MARK2), looks)) {
            return Some(StoredLine::WrongAudio { from: Some(from), sounds_like: Some(song), looks });
        }
    }
    None
}

// ---------------------------------------------------------- cold starts

fn cold_failed_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let detail = text.strip_prefix(&format!("{}: ", activity_in(loc).import_failed))?;
    if detail.is_empty() {
        return None;
    }
    let detail = cold_failure(detail, loc).unwrap_or_else(|| ColdFailure::Words(detail.to_owned()));
    Some(StoredLine::ColdFailed { detail })
}

fn cold_reason_in(text: &str, loc: Locale) -> Option<StoredLine> {
    Some(StoredLine::ColdReason { detail: cold_failure(text, loc)? })
}

fn cold_failure(text: &str, loc: Locale) -> Option<ColdFailure> {
    let problems: Vec<ColdProblem> = text.split("; ").map(|p| cold_problem(p, loc)).collect::<Option<_>>()?;
    (!problems.is_empty()).then_some(ColdFailure::Reasons(problems))
}

fn cold_problem(text: &str, loc: Locale) -> Option<ColdProblem> {
    let a = activity_in(loc);
    if text == a.export_not_found {
        return Some(ColdProblem::Known(ColdProblemKind::ExportNotFound));
    }
    if let Some(count) = number(text, &format!("{}: {}", a.playlists_not_opened, NUM)) {
        return Some(ColdProblem::Known(ColdProblemKind::PlaylistsNotOpened { count }));
    }
    if let Some(rest) = text.strip_prefix(&format!("{}: ", a.export_read_failed)) {
        let problems: Vec<String> = rest.split("; ").map(str::to_owned).collect();
        return Some(ColdProblem::Known(ColdProblemKind::ExportRead { problems }));
    }
    if let Some(why) = one(text, &ErrorStrings::cold_problem_likes_in(loc, MARK)) {
        let why = service_error(&why, loc)?;
        return Some(ColdProblem::Known(ColdProblemKind::Likes { why }));
    }
    if let Some(why) = one(text, &ErrorStrings::cold_problem_history_in(loc, MARK)) {
        let why = service_error(&why, loc)?;
        return Some(ColdProblem::Known(ColdProblemKind::History { why }));
    }
    None
}

/// «Найдено: 3 прослушивания · новых: 1 прослушивание · пропущено 2
/// плейлиста не с музыкой».
fn cold_summary_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let a = activity_in(loc);
    let rest = text.strip_prefix(&format!("{}: ", a.import_found))?;
    let (found, rest) = rest.split_once(&format!(" · {}: ", a.import_new))?;
    let new = counts_in(rest.split(" · ").next()?, loc)?;
    let mut skipped = 0;
    let mut problems = Vec::new();
    for part in rest.split(" · ").skip(1) {
        if let Some(rest) = part.strip_prefix(&format!("{}: ", a.import_not_all)) {
            problems = rest.split("; ").map(|p| cold_problem(p, loc)).collect::<Option<_>>()?;
        } else if let Some(n) = skipped_in(part, loc) {
            skipped = n;
        } else {
            return None;
        }
    }
    Some(StoredLine::ColdSummary { found: counts_in(found, loc)?, new, skipped, problems })
}

/// «3 прослушивания, 2 лайка».
fn counts_in(text: &str, loc: Locale) -> Option<[usize; 4]> {
    if text == activity_in(loc).import_nothing {
        return Some([0; 4]);
    }
    let mut out = [0; 4];
    for part in text.split(", ") {
        let (n, kind) = count_part(part, loc)?;
        out[kind] = n;
    }
    (fmt_import_counts_in(loc, out[0], out[1], out[2], out[3]) == text).then_some(out)
}

fn count_part(text: &str, loc: Locale) -> Option<(usize, usize)> {
    let n = leading_number(text)?;
    for kind in 0..4 {
        let mut counts = [0; 4];
        counts[kind] = n;
        if fmt_import_counts_in(loc, counts[0], counts[1], counts[2], counts[3]) == text {
            return Some((n, kind));
        }
    }
    None
}

/// « · пропущено 2 плейлиста не с музыкой» without its leading « · ».
fn skipped_in(text: &str, loc: Locale) -> Option<usize> {
    let n = numbers_in(text).into_iter().next()? as usize;
    (fmt_import_skipped_in(loc, n).strip_prefix(" · ") == Some(text)).then_some(n)
}

/// A service's error, in the words it was written with.
fn service_error(text: &str, loc: Locale) -> Option<ServiceErrorWords> {
    let err = errors_in(loc);
    let fixed = [
        (err.stream_not_configured, ServiceErrorWords::NotConfigured),
        (err.stream_not_logged_in, ServiceErrorWords::NotLoggedIn),
        (err.stream_need_premium, ServiceErrorWords::PremiumRequired),
        (err.stream_unavailable, ServiceErrorWords::Unavailable),
        (err.stream_age_restricted, ServiceErrorWords::AgeRestricted),
        (err.stream_region_blocked, ServiceErrorWords::RegionBlocked),
        (err.stream_drm, ServiceErrorWords::Drm),
        (err.stream_unsupported, ServiceErrorWords::Unsupported),
        (err.stream_network, ServiceErrorWords::Network),
        (err.stream_parse, ServiceErrorWords::Parse),
    ];
    let candidate = fixed
        .into_iter()
        .find(|(words, _)| *words == text)
        .map(|(_, error)| error)
        .or_else(|| {
            let until = one(text, &ErrorStrings::stream_rate_limited_in(loc, Some(MARK)))?;
            is_hm(&until).then_some(ServiceErrorWords::RateLimited { until: Some(until) })
        })
        .or_else(|| {
            (text == ErrorStrings::stream_rate_limited_in(loc, None))
                .then_some(ServiceErrorWords::RateLimited { until: None })
        })
        .or_else(|| {
            let sample = ErrorStrings::stream_http_in(loc, 999);
            let (pre, suf) = sample.split_once("999")?;
            let status: u16 = text.strip_prefix(pre)?.strip_suffix(suf)?.parse().ok()?;
            Some(ServiceErrorWords::Http { status })
        })?;
    (candidate.words_in(loc) == text).then_some(candidate)
}

// --------------------------------------------------------------- offers

fn torrent_found_in(text: &str, loc: Locale) -> Option<StoredLine> {
    let sample = fmt_torrent_found_in(loc, MARK, MARK2, NUM as u32, MARK);
    let sample_parts: Vec<&str> = sample.splitn(4, " · ").collect();
    let parts: Vec<&str> = text.splitn(4, " · ").collect();
    if parts.len() != 4 || sample_parts.len() != 4 {
        return None;
    }
    let format = match parts[0] {
        "?" => None,
        label => Some(format_of(label)?),
    };
    Some(StoredLine::TorrentFound {
        format,
        size: bytes_in(parts[1], loc)?,
        seeders: number(parts[2], sample_parts[2])? as u32,
        title: parts[3].to_owned(),
    })
}

/// An offer's line: the parts of an offer, then the item's name.
fn offer_in(text: &str, loc: Locale) -> Option<StoredLine> {
    for mask in (0..8u8).rev() {
        let mut parts = text.split(" · ");
        let format = if mask & 1 != 0 {
            let Some(format) = parts.next().and_then(format_of) else {
                continue;
            };
            Some(format)
        } else {
            None
        };
        let size = if mask & 2 != 0 {
            let Some(size) = parts.next().and_then(|p| size_in(p, loc)) else {
                continue;
            };
            Some(size)
        } else {
            None
        };
        let seeders = if mask & 4 != 0 {
            let Some(seeders) = parts.next().and_then(|p| seeders_in(p, loc)) else {
                continue;
            };
            Some(seeders)
        } else {
            None
        };
        let Some(provider) = parts.next() else {
            continue;
        };
        let title: Vec<&str> = parts.collect();
        if title.is_empty() {
            continue;
        }
        let offer = OfferWords {
            format,
            size_bytes: size,
            seeders,
            provider: provider.to_owned(),
            source: source_key(provider, loc).unwrap_or(SourceKey::Soulseek),
        };
        let line = StoredLine::Offer { offer, title: title.join(" · ") };
        if line.words_in(loc) == text {
            return Some(line);
        }
    }
    None
}

/// A format as its label («FLAC 24/96», «MP3 320», «?») or as a file's own
/// words say it («Opus 160»): the caller's render-back check takes the right
/// one.
fn format_of(text: &str) -> Option<Format> {
    if text == "?" {
        return Some(Format::new(Codec::Other(String::new())));
    }
    for codec in codecs_in(text) {
        let name = codec_name(&codec);
        for spelling in [name.clone(), name.to_uppercase()] {
            let rest = match text.strip_prefix(&spelling) {
                Some("") => None,
                Some(rest) => match rest.strip_prefix(' ') {
                    Some(rest) => Some(rest),
                    None => continue,
                },
                None => continue,
            };
            let mut format = Format::new(codec.clone());
            if let Some(rest) = rest {
                if let Some((bits, khz)) = rest.split_once('/') {
                    format.bit_depth = Some(bits.parse().ok()?);
                    format.sample_rate = Some((khz.parse::<f64>().ok()? * 1000.0).round() as u32);
                } else {
                    format.bitrate_kbps = Some(rest.parse().ok()?);
                }
            }
            if format.label() == text || format_words(&format) == text {
                return Some(format);
            }
        }
    }
    None
}

/// A size as [`fmt_size_in`] says it («312 МБ», «1.4 ГБ»), back to bytes.
fn size_in(text: &str, loc: Locale) -> Option<u64> {
    for (bytes, factor) in [(1_500_000_000u64, 1u64 << 30), (1_500_000, 1 << 20)] {
        let (_, unit) = fmt_size_in(loc, bytes).split_once(' ')?;
        if let Some(number) = text.strip_suffix(&format!(" {unit}")) {
            return Some((number.parse::<f64>().ok()? * factor as f64).round() as u64);
        }
    }
    None
}

/// A download's size as [`fmt_bytes_in`] says it («1.2 ГБ», «312.0 МБ»).
fn bytes_in(text: &str, loc: Locale) -> Option<u64> {
    for (bytes, factor) in [(1_500_000_000u64, 1_000_000_000u64), (1_500_000, 1_000_000), (999_000, 1_000)] {
        let (_, unit) = fmt_bytes_in(loc, bytes).split_once(' ')?;
        if let Some(number) = text.strip_suffix(&format!(" {unit}")) {
            return Some((number.parse::<f64>().ok()? * factor as f64).round() as u64);
        }
    }
    None
}

/// «14 сид.» / «14 seeds».
fn seeders_in(text: &str, loc: Locale) -> Option<u32> {
    for n in [1u32, 2, 5, 11, 21] {
        let sample = fmt_seeders_in(loc, n);
        let (pre, suf) = sample.split_once(&n.to_string())?;
        if let Some(digits) = text.strip_prefix(pre).and_then(|t| t.strip_suffix(suf))
            && let Ok(seeders) = digits.parse()
        {
            return Some(seeders);
        }
    }
    None
}

fn source_key(name: &str, loc: Locale) -> Option<SourceKey> {
    SourceKey::ALL.into_iter().find(|key| source_name_in(loc, *key) == name)
}

// ------------------------------------------------------------- quality

/// The words of a quality label or of a file's own, as the app wrote them.
fn quality_words(text: &str, loc: Locale) -> Option<QualityWords> {
    in_library(text, loc)
        .or_else(|| file_words(text, loc))
        .or_else(|| quality_candidates(text).into_iter().find(|q| fmt_quality_words_in(loc, q) == text))
}

/// «FLAC из lossy», «FLAC, возможно из lossy»: a library file as a better
/// copy's words say it.
fn file_words(text: &str, loc: Locale) -> Option<QualityWords> {
    let s = status_in(loc);
    let (label, score) = if let Some(label) = text.strip_suffix(s.from_lossy) {
        (label, score::FAKE_LOSSLESS)
    } else if let Some(label) = text.strip_suffix(s.maybe_lossy) {
        (label, score::CD_LOSSLESS - 1)
    } else {
        return None;
    };
    let format = format_of(label)?;
    let q = QualityWords::File {
        codec: format.codec,
        bitrate_kbps: format.bitrate_kbps,
        bit_depth: format.bit_depth,
        sample_rate: format.sample_rate,
        score,
        master: None,
    };
    (fmt_quality_words_in(loc, &q) == text).then_some(q)
}

/// «FLAC 24/96 (в библиотеке Opus 158)»: a lossy copy of a master the
/// archive keeps.
fn in_library(text: &str, loc: Locale) -> Option<QualityWords> {
    let sample = fmt_in_library_in(loc, MARK);
    let (open, close) = sample.split_once(MARK)?;
    let (master, own) = text.rsplit_once(open)?;
    let master = master.strip_suffix(' ')?;
    let own = own.strip_suffix(close)?;
    let format = format_of(own)?;
    let master = masters_in(master, loc)?;
    for score in [score::CD_LOSSLESS, score::HI_RES_LOSSLESS] {
        let q = QualityWords::File {
            codec: format.codec.clone(),
            bitrate_kbps: format.bitrate_kbps,
            bit_depth: format.bit_depth,
            sample_rate: format.sample_rate,
            score,
            master: Some(master.clone()),
        };
        if fmt_quality_words_in(loc, &q) == text {
            return Some(q);
        }
    }
    None
}

/// A master as its words say it: «FLAC 24/96», «FLAC Hi-Res», «FLAC».
fn masters_in(text: &str, loc: Locale) -> Option<MasterWords> {
    if let Some(format) = format_of(text) {
        return Some(MasterWords::Format(format));
    }
    let (name, rest) = text.rsplit_once(' ')?;
    let codec = Codec::parse(name);
    if rest == "Hi-Res" {
        return Some(MasterWords::Archive { codec, sample_rate: None, hi_res: true });
    }
    let khz = rest.strip_suffix(&format!(" {}", status_in(loc).khz_unit))?;
    let rate = (khz.parse::<f64>().ok()? * 1000.0).round() as u32;
    Some(MasterWords::Archive { codec, sample_rate: Some(rate), hi_res: true })
}

/// Every label and file's own words these words may hold, for the parse to
/// pick the one that says them exactly.
fn quality_candidates(text: &str) -> Vec<QualityWords> {
    let codecs = codecs_in(text);
    let numbers = numbers_in(text);
    let mut out = Vec::new();
    if let Some(format) = format_of(text) {
        out.push(QualityWords::Label { codec: format.codec.clone(), bitrate_kbps: format.bitrate_kbps, check: None });
        for score in [score::CD_LOSSLESS, score::HI_RES_LOSSLESS] {
            out.push(QualityWords::File {
                codec: format.codec.clone(),
                bitrate_kbps: format.bitrate_kbps,
                bit_depth: format.bit_depth,
                sample_rate: format.sample_rate,
                score,
                master: None,
            });
        }
    }
    for codec in codecs {
        out.push(QualityWords::Label { codec: codec.clone(), bitrate_kbps: None, check: None });
        for rate in numbers.iter().copied() {
            out.push(QualityWords::Label { codec: codec.clone(), bitrate_kbps: Some(rate), check: None });
        }
        for sure in [true, false] {
            out.push(QualityWords::Label {
                codec: codec.clone(),
                bitrate_kbps: None,
                check: Some(fake_check(None, sure)),
            });
            for rate in numbers.iter().copied() {
                out.push(QualityWords::Label {
                    codec: codec.clone(),
                    bitrate_kbps: None,
                    check: Some(fake_check(Some(rate), sure)),
                });
            }
            for from in [Codec::Flac, Codec::Alac, Codec::Wav] {
                out.push(QualityWords::Label {
                    codec: codec.clone(),
                    bitrate_kbps: None,
                    check: Some(from_fake_check(from.clone(), sure)),
                });
                for rate in numbers.iter().copied() {
                    out.push(QualityWords::Label {
                        codec: codec.clone(),
                        bitrate_kbps: Some(rate),
                        check: Some(from_fake_check(from.clone(), sure)),
                    });
                }
            }
        }
    }
    out
}

fn fake_check(kbps: Option<u32>, sure: bool) -> CheckSummary {
    CheckSummary {
        verdict: Verdict::Transcoded,
        origin: kbps.map_or(SoundOrigin::Unknown, |kbps| SoundOrigin::Lossy { kbps }),
        made_from: None,
        cutoff_hz: Some(if sure { 19_000 } else { 20_000 }),
    }
}

fn from_fake_check(from: Codec, sure: bool) -> CheckSummary {
    CheckSummary {
        verdict: Verdict::Transcoded,
        origin: SoundOrigin::Lossy { kbps: 128 },
        made_from: Some(from),
        cutoff_hz: Some(if sure { 19_000 } else { 20_000 }),
    }
}

/// The codecs a label may name: every known one and every word of `text`
/// that names one.
fn codecs_in(text: &str) -> Vec<Codec> {
    let mut out = vec![Codec::Flac, Codec::Opus, Codec::Mp3, Codec::Aac, Codec::Vorbis, Codec::Alac, Codec::Wav];
    for word in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        let codec = Codec::parse(word);
        if codec_name(&codec) == word {
            out.push(codec);
        }
    }
    out.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    out.dedup();
    out
}

/// The numbers the words hold (a rate, a bitrate).
fn numbers_in(text: &str) -> Vec<u32> {
    let mut out: Vec<u32> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|digits| !digits.is_empty())
        .filter_map(|digits| digits.parse().ok())
        .collect();
    out.truncate(8);
    out
}

// ------------------------------------------------------------- helpers

/// The text `text` holds where `sample` holds [`MARK`].
fn one(text: &str, sample: &str) -> Option<String> {
    let (pre, suf) = sample.split_once(MARK)?;
    Some(text.strip_prefix(pre)?.strip_suffix(suf)?.to_owned())
}

/// The two texts `text` holds where `sample` holds [`MARK`] and [`MARK2`].
fn two(text: &str, sample: &str) -> Option<(String, String)> {
    let (pre, rest) = sample.split_once(MARK)?;
    let (mid, suf) = rest.split_once(MARK2)?;
    let rest = text.strip_prefix(pre)?.strip_suffix(suf)?;
    let (a, b) = rest.split_once(mid)?;
    Some((a.to_owned(), b.to_owned()))
}

/// The two texts `text` holds where `sample` holds [`MARK`] and [`NUM`], in
/// the order the sample has them.
fn marked_and_numbered(text: &str, sample: &str) -> Option<(String, String)> {
    if sample.find(MARK)? < sample.find(NUM_TEXT)? {
        let (pre, rest) = sample.split_once(MARK)?;
        let (mid, suf) = rest.split_once(NUM_TEXT)?;
        let rest = text.strip_prefix(pre)?.strip_suffix(suf)?;
        let (a, b) = rest.split_once(mid)?;
        Some((a.to_owned(), b.to_owned()))
    } else {
        let (pre, rest) = sample.split_once(NUM_TEXT)?;
        let (mid, suf) = rest.split_once(MARK)?;
        let rest = text.strip_prefix(pre)?.strip_suffix(suf)?;
        let (a, b) = rest.split_once(mid)?;
        Some((a.to_owned(), b.to_owned()))
    }
}

/// The number `text` holds where `sample` holds [`NUM`].
fn number(text: &str, sample: &str) -> Option<usize> {
    let (pre, suf) = sample.split_once(NUM_TEXT)?;
    text.strip_prefix(pre)?.strip_suffix(suf)?.parse().ok()
}

/// The two numbers `text` holds where `sample` holds [`NUM`] twice.
fn two_numbers(text: &str, sample: &str) -> Option<(usize, usize)> {
    let (pre, rest) = sample.split_once(NUM_TEXT)?;
    let (mid, suf) = rest.split_once(NUM_TEXT)?;
    let rest = text.strip_prefix(pre)?.strip_suffix(suf)?;
    let (a, b) = rest.split_once(mid)?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// The number the words start with («1 240», «3»).
fn leading_number(text: &str) -> Option<usize> {
    let word = text.split(' ').next()?;
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_digit() || c == '\u{a0}' || c == ',') {
        return None;
    }
    word.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
}

/// «21:40»: a clock time as the lines tell it.
fn is_hm(text: &str) -> bool {
    let Some((hours, minutes)) = text.split_once(':') else {
        return false;
    };
    hours.len() == 2
        && minutes.len() == 2
        && hours.bytes().all(|b| b.is_ascii_digit())
        && minutes.bytes().all(|b| b.is_ascii_digit())
        && hours.parse::<u32>().is_ok_and(|h| h < 24)
        && minutes.parse::<u32>().is_ok_and(|m| m < 60)
}

/// `text` with its first letter lowercased (a line that stood alone).
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_lowercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{WishlistItem, WishlistStatus};

    use super::*;

    /// The notes the old code wrote, in Russian and in the English of this
    /// branch's test builds: each becomes a key that says it back exactly.
    #[test]
    fn old_notes_become_keys() {
        let ru = [
            "Файлы ждут подтверждения",
            "Не нашёл, где скачать — поищу снова через неделю",
            "Есть копия с YouTube — раз в неделю ищу качество лучше",
            "Ищу…",
            "Источники не ответили — попробую через час",
            "Нашёл качество лучше — подтверди замену",
            "Не уверен, что это оно — выбери",
            "Своя раздача",
            "Свой файл",
            "Сохраняю с YouTube…",
            "Не получилось сохранить с YouTube",
            "Лучше пока нет",
            "Оставил прежний файл",
            "Оставил прежние файлы",
            "Вернул прежние файлы",
            "Смотрю, есть ли трек в раздаче: Album (2020) FLAC",
            "Раздача: Album (2020) FLAC",
            "FLAC · 1.2 ГБ · 14 сид. · Album (2020) FLAC",
            "FLAC · 312 МБ · 14 сид. · RuTracker · Album (2020) FLAC",
            "Поищу снова в 21:40",
            "Поищу снова через 6 мин",
            "Скачано 3 из 10",
            "Скачано · FLAC",
            "Скачано · FLAC из MP3 ~128",
            "Было MP3 128 — теперь FLAC",
            "Теперь FLAC",
            "Заменил на качество лучше",
            "Есть 3 из 10 · Qobuz просит подождать до 03:00 · в 21:40 спрошу Qobuz",
            "Источники не дали скачать · попробую снова в 21:40",
        ];
        for text in ru {
            let key = note_key(text).unwrap_or_else(|| panic!("not recognized: «{text}»"));
            assert_ne!(key, text, "«{text}» is still its own words");
            assert_eq!(line_words_in(Locale::Ru, &key), text, "«{text}» became «{key}»");
        }
        let en = [
            "Files are waiting for confirmation",
            "No download found — searching again in a week",
            "Searching…",
            "Your torrent",
            "Torrent: Album (2020) FLAC",
            "Searching again at 21:40",
            "Downloaded 3 of 10",
            "Downloaded · FLAC from MP3 ~128",
            "Was MP3 128 — now FLAC",
            "Now FLAC",
        ];
        for text in en {
            let key = note_key(text).unwrap_or_else(|| panic!("not recognized: «{text}»"));
            assert_ne!(key, text);
            assert_eq!(line_words_in(Locale::En, &key), text, "«{text}» became «{key}»");
        }
    }

    /// The sentinel and the codes already stored need no rewrite.
    #[test]
    fn notes_that_are_keys_already_stay() {
        assert_eq!(note_key("Скачано").as_deref(), Some("Скачано"));
        assert_eq!(note_key("Downloaded").as_deref(), Some("Скачано"));
        assert_eq!(note_key("files_wait"), None);
        assert_eq!(note_key("has_copy"), None);
        assert_eq!(note_key(&StoredLine::Week.encode()), None);
        assert_eq!(note_key("Скачано куда-то"), None);
    }

    /// The job lines the old code wrote: each becomes a key that says it
    /// back exactly.
    #[test]
    fn old_job_lines_become_keys() {
        let import = [
            "добавлено 3 · дублей 1 · не из этого альбома 2 · ошибок 2 · отправлено на подтверждение 4",
            "добавлено 1",
            "отменено — добавленное убрано",
            "заменено 3 · не лучше прежних 1 · не из этого альбома 2 · ошибок 1",
            "скачанный FLAC оказался из MP3 ~128 — оставил прежний файл",
            "скачанный файл не лучше прежнего — оставил прежний",
            "скачанное не лучше прежнего (2) — оставил прежние файлы",
            "скачанное не из этого альбома — ничего не добавил",
            "ничего не пришло",
            "Скачанный FLAC оказался из MP3 ~128 — оставил прежний файл · дальше — Qobuz",
            "Скачанный файл не лучше прежнего — оставил прежний · есть другие варианты — в «Подтверди»",
            "перенесено 3 · проверено BLAKE3 → D:\\flac",
            "папка → D:\\flac",
            "Скачано 1.2 ГБ",
            "Скачано 1.2 ГБ · импорт в библиотеку",
            "Не удалось: 2",
            "В файле другая песня: «Song» · не удалось ещё: 1",
            "В файле от Soulseek другая песня — «Song» · ищу дальше",
            "В файле другая песня",
            "Нашёл FLAC вместо MP3 320 · подтверди замену в «Подтверди»",
            "Нашёл FLAC · подтверди замену в «Подтверди»",
            "Нашёл качество лучше · подтверди замену в «Подтверди»",
            "Нашёл качество лучше для 3 треков · подтверди замену в «Подтверди»",
            "Скачанное не лучше прежних файлов — оставил их",
            "Скачано 1 из 10 · Tidal сейчас не отдаёт остальное",
            "Скачано 1 из 10 · остальных 3 у Qobuz нет",
            "Tidal сейчас не отдаёт этот трек · дальше — Qobuz",
            "Tidal не пускает · есть варианты под вопросом — выбери в «Хочу скачать»",
            "Tidal не пускает · поищу снова через неделю",
            "Найдено: 3 прослушивания, 2 лайка · новых: 1 прослушивание",
            "Найдено: 3 прослушивания · новых: 1 прослушивание · пропущено 2 плейлиста не с музыкой",
            "Найдено: ничего · новых: ничего",
            "Найдено: 3 прослушивания · новых: 1 прослушивание · не всё: лайки: Сервис просит подождать",
            "Импорт не удался: лайки: Сервис просит подождать",
            "лайки: Сервис просит подождать",
            "не открылись плейлисты: 3",
            "Не нашёл файлов выгрузки Spotify или Google Takeout",
        ];
        for text in import {
            let kind = if text.starts_with("Скачано 1.2") { "torrent" } else { "import" };
            let key = job_key(kind, Field::Subtitle, text).unwrap_or_else(|| panic!("not recognized: «{text}»"));
            assert_ne!(key, text, "«{text}» is still its own words");
            let line = StoredLine::decode(&key).unwrap_or_else(|| panic!("not a line: «{key}»"));
            assert_eq!(line.words_in(Locale::Ru), text, "«{text}» became «{key}»");
        }
        let en = [
            "added 3 · duplicates 1 · errors 2",
            "replaced 2 · 1 no better than the previous files",
            "Downloaded 1.2 GB",
            "Downloaded 1.2 GB · importing into the library",
            "No source could provide a download · searching again in a week",
            "Import from Spotify",
        ];
        for text in en {
            let kind = if text.starts_with("Downloaded 1.2") {
                "torrent"
            } else if text == "Import from Spotify" {
                "import"
            } else {
                "wish:1"
            };
            let field = if text == "Import from Spotify" { Field::Title } else { Field::Subtitle };
            let key = job_key(kind, field, text).unwrap_or_else(|| panic!("not recognized: «{text}»"));
            assert_ne!(key, text);
            let line = StoredLine::decode(&key).unwrap_or_else(|| panic!("not a line: «{key}»"));
            assert_eq!(line.words_in(Locale::En), text, "«{text}» became «{key}»");
        }
    }

    /// The titles of imports: generated ones are kept as keys, a torrent's
    /// or a wished item's own name is not touched.
    #[test]
    fn old_job_titles_become_keys_and_names_stay() {
        let titles = [
            "Импорт «a.flac»",
            "Импорт 3 объектов",
            "Импорт для «Хочу скачать»",
            "Сохранение с YouTube",
            "Качество лучше: Artist — Album",
            "Импорт из Spotify",
            "Импорт выгрузки «x.json»",
            "Импорт из Inbox",
            "Импорт",
            "Перенос архива FLAC",
        ];
        for text in titles {
            let key = job_key("import", Field::Title, text).unwrap_or_else(|| panic!("not recognized: «{text}»"));
            assert_ne!(key, text);
            let line = StoredLine::decode(&key).unwrap_or_else(|| panic!("not a line: «{key}»"));
            assert_eq!(line.words_in(Locale::Ru), text, "«{text}» became «{key}»");
        }
        // A torrent's name that reads like a title, and a wished item's name.
        assert_eq!(job_key("torrent", Field::Title, "Импорт «a.flac»"), None);
        assert_eq!(job_key("wish:1", Field::Title, "Импорт"), None);
        assert_eq!(job_key("torrent", Field::Error, "Скачано 1.2 ГБ"), None);
    }

    /// Text this app never wrote stays as it is.
    #[test]
    fn unknown_words_stay() {
        assert_eq!(note_key("Скачано вчера"), None);
        assert_eq!(note_key("Лучше пока нет, спрошу позже"), None);
        assert_eq!(job_key("import", Field::Subtitle, "добавлено много"), None);
        assert_eq!(job_key("import", Field::Subtitle, "добавлено 1 · и ещё что-то"), None);
        // The old words of a copy that was no better, spelled with a comma
        // the app does not write now: not recognized, left as it was.
        assert_eq!(
            job_key("import", Field::Subtitle, "скачанный FLAC, возможно, из MP3 320 — оставил прежний файл"),
            None
        );
    }

    /// The migration rewrites the rows it knows once, and nothing the
    /// second time.
    #[test]
    fn the_migration_rewrites_old_rows_once() {
        let lib = Library::open_in_memory().unwrap();
        let mut item = WishlistItem::new(WishlistStatus::NoTorrent, 1);
        item.title = Some("Song".into());
        item.note = Some("Не нашёл, где скачать — поищу снова через неделю".into());
        lib.put_wishlist_item(&item).unwrap();
        lib.log_job("import", "Импорт «a.flac»", "добавлено 1 · ошибок 2", Some("Не удалось: 2")).unwrap();
        lib.log_job("torrent", "Some.Torrent", "Скачано 1.2 ГБ", None).unwrap();
        lib.log_job("wish:1", "Artist — Song", "Tidal не пускает · поищу снова через неделю", None).unwrap();

        assert_eq!(lib.translate_legacy_status_texts().unwrap(), 4, "the note and three rows");
        assert_eq!(lib.get_wishlist_item(item.id).unwrap().unwrap().note.as_deref(), Some("not_found"));
        let log = lib.job_log(None, 10).unwrap();
        let by_kind = |kind: &str| log.iter().find(|e| e.kind == kind).unwrap().clone();
        let import = by_kind("import");
        assert_eq!(StoredLine::decode(&import.title).unwrap().words_in(Locale::En), "Import “a.flac”");
        assert_eq!(StoredLine::decode(&import.subtitle).unwrap().words_in(Locale::En), "added 1 · errors 2");
        assert_eq!(StoredLine::decode(import.error.as_deref().unwrap()).unwrap().words_in(Locale::En), "Failed: 2");
        let torrent = by_kind("torrent");
        assert_eq!(torrent.title, "Some.Torrent", "a torrent's name stays");
        assert_eq!(StoredLine::decode(&torrent.subtitle).unwrap().words_in(Locale::En), "Downloaded 1.2 GB");
        let wish = by_kind("wish:1");
        assert_eq!(wish.title, "Artist — Song", "a wished item's name stays");
        assert_eq!(
            StoredLine::decode(&wish.subtitle).unwrap().words_in(Locale::En),
            "Tidal denied access · searching again in a week"
        );

        let before: Vec<_> = lib.job_log(None, 10).unwrap();
        assert_eq!(lib.translate_legacy_status_texts().unwrap(), 0, "once per library");
        let after: Vec<_> = lib.job_log(None, 10).unwrap();
        assert_eq!(
            before.iter().map(|e| (e.seq, e.title.clone(), e.subtitle.clone(), e.error.clone())).collect::<Vec<_>>(),
            after.iter().map(|e| (e.seq, e.title.clone(), e.subtitle.clone(), e.error.clone())).collect::<Vec<_>>(),
        );
        assert!(lib.meta_value(LEGACY_TEXTS_KEY).unwrap().is_some());
    }
}
