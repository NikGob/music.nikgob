use super::*;

/// A source that finds but gives no file.
fn finds_only_in(loc: Locale) -> &'static str {
    status_in(loc).finds_only
}

/// Now, in unix seconds.
pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A wait of `left` (from `now`, unix seconds) is a Worker's day of
/// requests: it ends at midnight UTC.
fn day_limit(left: Duration, now: u64) -> bool {
    nikgob_sources::http::ends_at_utc_midnight(now, left)
}

/// «02:00» when a wait of `left` is long enough to be told with its time.
fn told_hm(left: Option<Duration>) -> Option<String> {
    left.filter(|d| *d >= WAIT_TOLD).and_then(super::super::accounts::hm_after)
}

/// A source that could not answer, in words for «Источники»; `left`: how
/// long it still sits out, when it does (its time is told).
pub(super) fn down_text_in(loc: Locale, e: &SourceError, left: Option<Duration>) -> String {
    down_text_at_in(loc, e, left, unix_now())
}

pub(super) fn down_text_at_in(loc: Locale, e: &SourceError, left: Option<Duration>, now: u64) -> String {
    use nikgob_core::i18n::status as st;
    let hm = told_hm(left);
    match (e, hm) {
        // Cloudflare's «Just a moment…»: only a browser passes it, waiting
        // does not help, and the site never asked us to wait.
        (e, _) if nikgob_sources::http::is_check(e) => status_in(loc).site_check.to_owned(),
        (SourceError::Blocked(_), Some(hm)) => st::fmt_blocked_in(loc, Some(&hm)),
        (SourceError::Blocked(_), None) => st::fmt_blocked_in(loc, None),
        (SourceError::RateLimited { .. }, Some(hm)) if left.is_some_and(|d| day_limit(d, now)) => {
            st::fmt_day_limit_in(loc, &hm)
        }
        (SourceError::RateLimited { .. }, Some(hm)) => st::fmt_asks_to_wait_in(loc, Some(&hm)),
        (SourceError::RateLimited { .. }, None) => st::fmt_asks_to_wait_in(loc, None),
        (SourceError::Http { status }, Some(hm)) => st::fmt_answers_with_in(loc, *status, Some(&hm)),
        (SourceError::Http { status }, None) => st::fmt_answers_with_in(loc, *status, None),
        // Its search works, but no file it finds can be had (Tidal while no
        // instance gives files).
        (SourceError::NothingToFetch, _) => finds_only_in(loc).to_owned(),
        _ => status_in(loc).no_answer.to_owned(),
    }
}

/// The same state of a source (what it says, not how long is left), so
/// its log line is written once, not at every search.
pub(super) fn same_state(a: &SourceError, b: &SourceError) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
        && match (a, b) {
            (SourceError::Http { status: x }, SourceError::Http { status: y }) => x == y,
            (SourceError::Blocked(x), SourceError::Blocked(y)) => x == y,
            _ => true,
        }
}

/// A source whose download failed, in words for «Источники»; `None` when
/// the failure says nothing of the source (the disk, an empty offer).
/// `aside`: how long it is still set aside for refusing, when it is.
#[cfg(test)]
pub(super) fn fetch_down_text(e: &SourceError, aside: Option<Duration>) -> Option<String> {
    fetch_down_text_in(Locale::Ru, e, aside)
}

/// [`fetch_down_text`] in `loc`.
pub(super) fn fetch_down_text_in(loc: Locale, e: &SourceError, aside: Option<Duration>) -> Option<String> {
    match e {
        SourceError::Io(_) | SourceError::NothingToFetch | SourceError::NotConfigured => None,
        SourceError::Blocked(_) | SourceError::RateLimited { .. } => Some(down_text_in(loc, e, aside)),
        _ => Some(match aside.and_then(super::super::accounts::hm_after) {
            Some(hm) => nikgob_core::i18n::status::fmt_finds_only_until_in(loc, &hm),
            None => finds_only_in(loc).to_owned(),
        }),
    }
}

/// Why `key` turned an item away, kept as data so the words are made in
/// the reader's locale: «Tidal сейчас не отдаёт этот трек», «сервер Qobuz
/// недоступен», «Qobuz просит подождать до 03:00». `what`: «этот трек»,
/// «этот альбом», «остальное».
pub(super) fn refusal_at(key: SourceKey, e: &SourceError, what: What, now: u64) -> Refusal {
    match e {
        SourceError::RateLimited { retry_after: Some(d) } if *d >= WAIT_TOLD && day_limit(*d, now) => {
            Refusal::DayLimit { source: key }
        }
        SourceError::RateLimited { retry_after: Some(d) } if *d >= WAIT_TOLD => Refusal::AsksToWait {
            source: key,
            until: super::super::accounts::hm_after(*d),
        },
        SourceError::RateLimited { .. } => Refusal::AsksToWait { source: key, until: None },
        e if nikgob_sources::http::is_check(e) => Refusal::Check { source: key },
        SourceError::Blocked(_) => Refusal::Blocked { source: key },
        SourceError::NothingToFetch => Refusal::FindsOnly { source: key },
        // A ban of the instance's account, a file it cannot give (DRM).
        SourceError::Http { status: 401 | 403 } | SourceError::Parse(_) => Refusal::GivesNo { source: key, what },
        SourceError::Http { status } if *status >= 500 => Refusal::ServerDown { source: key },
        SourceError::Unreachable(_) => Refusal::ServerDown { source: key },
        _ => Refusal::NoDownload { source: key },
    }
}

/// «в 21:40» (or «через N мин» when the clock is unknown), as data.
pub(super) fn at_words(wait: Duration) -> AtWords {
    AtWords { hm: super::super::accounts::hm_after(wait), mins: wait.as_secs().div_ceil(60) }
}

/// When an item every offer of which was turned away is searched again,
/// and the sources worth naming for then.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Retry {
    pub(super) wait: Duration,
    pub(super) ask: Vec<SourceKey>,
}

/// The `n`th retry in a row of an item: at least [`LATER_FIRST`] doubling
/// each time (up to [`LATER_LONGEST`]), and not before the first source
/// that said no can be asked again (`turned`: each one with how long until
/// then, when known; up to a day). Named for then: those of them back by
/// then, and the `others` whose last answer for the item (how long ago)
/// will be [`ASK_AGAIN_AFTER`] old.
pub(super) fn plan_retry(n: u32, turned: &[(SourceKey, Option<Duration>)], others: &[(SourceKey, Duration)]) -> Retry {
    let backoff = (LATER_FIRST * 2u32.pow(n.saturating_sub(1).min(10))).min(LATER_LONGEST);
    let soonest = turned.iter().map(|(_, back)| back.unwrap_or_default()).min().unwrap_or_default();
    let wait = backoff.max(soonest.min(ASIDE_LONGEST));
    let mut ask: Vec<SourceKey> = Vec::new();
    for (k, back) in turned {
        if back.is_none_or(|b| b <= wait) && !ask.contains(k) {
            ask.push(*k);
        }
    }
    for (k, ago) in others {
        if *ago + wait >= ASK_AGAIN_AFTER && !ask.contains(k) {
            ask.push(*k);
        }
    }
    Retry { wait, ask }
}

/// The offers saved in an item, each on its own: one this build cannot
/// read (a source it no longer has) leaves out only itself, not the list.
/// Offers saved before stage 4.5 were the indexer's candidates.
pub(super) fn offers_of_json(json: &str) -> Vec<Offer> {
    let Ok(all) = serde_json::from_str::<Vec<serde_json::Value>>(json) else { return Vec::new() };
    all.into_iter()
        .filter_map(|o| {
            serde_json::from_value::<Offer>(o.clone())
                .ok()
                .or_else(|| serde_json::from_value::<Candidate>(o).ok().map(offer_of))
        })
        .collect()
}

/// The files picked out of a torrent can serve the item. A better copy of
/// an album asked for by hand is matched to the album's tracks file by
/// file ([`album_better`]): a torrent that by its names is a disc-image rip
/// of the album ([`image_rip`], against the album's track count) is not
/// taken for it (a release of track files is looked for instead). One the
/// names cannot tell is downloaded and its sheets read when it came.
pub(crate) fn picked_fits(item: &WishlistItem, want: &Want, files: &[(usize, String, u64)], picked: &[usize]) -> bool {
    !album_better::hand_album(item)
        || !image_rip(files.iter().filter(|(i, ..)| picked.contains(i)).map(|(_, p, _)| p.as_str()), want.album_tracks)
}

pub(super) fn status_rank(s: WishlistStatus) -> u8 {
    match s {
        WishlistStatus::Confirm => 0,
        WishlistStatus::Downloading => 1,
        WishlistStatus::Searching => 2,
        WishlistStatus::New => 3,
        WishlistStatus::Waiting => 4,
        WishlistStatus::NoTorrent => 5,
        WishlistStatus::Failed => 6,
        WishlistStatus::Done => 7,
        WishlistStatus::Dismissed => 8,
    }
}
