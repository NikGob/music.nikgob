//! A running download as both tabs of «Загрузки» say it: «Активные» shows
//! its row, «Хочу скачать» the line of the item it is for. Both read the one
//! [`Transfer`], so the percent, the bytes and the queue place never differ.

use nikgob_core::i18n::{Locale, locale};
use nikgob_core::i18n::status::{fmt_amount_in, fmt_paused_in, fmt_queued_in, fmt_rate_in, fmt_size_in};

use crate::{Job, size_ru};

/// A size as these lines say it: the Russian words exactly as before in
/// Russian, the locale's units otherwise.
fn size_in(loc: Locale, bytes: u64) -> String {
    match loc {
        Locale::Ru => size_ru(bytes),
        Locale::En => fmt_size_in(loc, bytes),
    }
}

/// Where a running download is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferPhase {
    /// Asked for; nothing came yet (a site answering, a torrent finding
    /// peers, a save from YouTube).
    #[default]
    Starting,
    /// Waits its turn at the sharer (a Soulseek peer's queue), with its
    /// place when the sharer told it.
    Queued(Option<u32>),
    /// The sharer is ready; the connection is being made.
    Connecting,
    /// Bytes are coming.
    Going,
    /// Stopped by the user (a torrent).
    Paused,
    /// The files came; the import takes them into the library.
    Importing,
}

/// A running download: where it is and how much of it is here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transfer {
    pub phase: TransferPhase,
    /// Bytes here.
    pub done: u64,
    /// Bytes of the whole, when the source says.
    pub total: Option<u64>,
    /// Bytes per second now; 0 when standing or not known.
    pub bps: u64,
}

impl Transfer {
    /// Bytes are counted (coming or paused).
    fn counted(&self) -> bool {
        matches!(self.phase, TransferPhase::Going | TransferPhase::Paused)
    }

    /// How much is here, 0..=100, rounded down: the one number both tabs
    /// show. `None` while nothing can be told (waiting, no size known).
    pub fn percent(&self) -> Option<u8> {
        let total = self.total.filter(|t| *t > 0 && self.counted())?;
        Some((u128::from(self.done.min(total)) * 100 / u128::from(total)) as u8)
    }

    /// The bar's fill, 0..1, of the same bytes as [`Self::percent`].
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.filter(|t| *t > 0 && self.counted())?;
        Some((self.done.min(total) as f64 / total as f64) as f32)
    }

    /// «1,2 МБ/с» while bytes come.
    pub fn rate(&self) -> Option<String> {
        self.rate_in(locale())
    }

    /// [`Self::rate`] in `loc`.
    pub fn rate_in(&self, loc: Locale) -> Option<String> {
        (self.phase == TransferPhase::Going && self.bps > 0).then(|| fmt_rate_in(loc, &size_in(loc, self.bps)))
    }

    /// How long the rest takes at this speed («51 с», «3 мин»).
    pub fn eta(&self) -> Option<String> {
        self.eta_in(locale())
    }

    /// [`Self::eta`] in `loc`.
    pub fn eta_in(&self, loc: Locale) -> Option<String> {
        if self.phase != TransferPhase::Going || self.bps == 0 {
            return None;
        }
        let left = self.total?.saturating_sub(self.done);
        (left > 0).then(|| eta_in(loc, left.div_ceil(self.bps)))
    }

    /// The bytes and the percent: «12,3 МБ из 33,1 МБ · 37%».
    fn amount_in(&self, loc: Locale) -> String {
        let total = self.total.filter(|t| *t > 0).map(|t| size_in(loc, t));
        fmt_amount_in(loc, &size_in(loc, self.done), total.as_deref(), self.percent())
    }

    /// The supporting line of its row in «Активные»; `from`: who gives it
    /// (a peer, a site), when that is not its title already.
    pub fn line(&self, from: Option<&str>) -> String {
        self.line_in(locale(), from)
    }

    /// [`Self::line`] in `loc`.
    pub fn line_in(&self, loc: Locale, from: Option<&str>) -> String {
        let s = nikgob_core::i18n::status_in(loc);
        let with = |words: String| match from {
            Some(f) if !f.is_empty() => format!("{f} · {words}"),
            _ => words,
        };
        match self.phase {
            TransferPhase::Starting => with(s.downloading.to_owned()),
            TransferPhase::Connecting => with(s.connecting.to_owned()),
            TransferPhase::Queued(place) => fmt_queued_in(loc, from, place),
            TransferPhase::Going => with(self.amount_in(loc)),
            TransferPhase::Paused => with(fmt_paused_in(loc, &self.amount_in(loc))),
            TransferPhase::Importing => with(s.importing.to_owned()),
        }
    }

    /// The words of its item in «Хочу скачать»: «качается · mugilaw ·
    /// 37%», «в очереди у mugilaw · 12-й».
    pub fn status(&self, from: Option<&str>) -> String {
        self.status_in(locale(), from)
    }

    /// [`Self::status`] in `loc`.
    pub fn status_in(&self, loc: Locale, from: Option<&str>) -> String {
        let s = nikgob_core::i18n::status_in(loc);
        let from = from.filter(|f| !f.is_empty());
        match self.phase {
            TransferPhase::Queued(place) => fmt_queued_in(loc, from, place),
            TransferPhase::Importing => s.importing.to_owned(),
            TransferPhase::Paused => match self.percent() {
                Some(p) => format!("{} · {p}%", s.paused),
                None => s.paused.to_owned(),
            },
            TransferPhase::Starting | TransferPhase::Connecting | TransferPhase::Going => {
                let mut parts = vec![s.downloading.to_owned()];
                parts.extend(from.map(str::to_owned));
                parts.extend(self.percent().map(|p| format!("{p}%")));
                parts.join(" · ")
            }
        }
    }

    /// Its row in «Активные»: `via` the source when `from` is someone of
    /// it («Soulseek» and the peer). `controllable`: ✕ cancels it;
    /// `pausable`: it can be paused too.
    pub fn job(&self, id: u64, title: String, via: Option<&str>, from: Option<&str>, controllable: bool, pausable: bool) -> Job {
        self.job_in(locale(), id, title, via, from, controllable, pausable)
    }

    /// [`Self::job`] in `loc`.
    pub fn job_in(&self, loc: Locale, id: u64, title: String, via: Option<&str>, from: Option<&str>, controllable: bool, pausable: bool) -> Job {
        let waiting = matches!(self.phase, TransferPhase::Queued(_));
        let subtitle = match via.filter(|v| !v.is_empty()) {
            Some(v) => format!("{v} · {}", self.line_in(loc, from)),
            None => self.line_in(loc, from),
        };
        Job {
            id,
            title,
            subtitle,
            progress: if waiting { None } else { self.fraction() },
            rate: self.rate_in(loc),
            eta: self.eta_in(loc),
            paused: self.phase == TransferPhase::Paused,
            queued: waiting,
            controllable,
            pausable,
            error: None,
            at: 0,
        }
    }
}

/// A time left: «51 с», «3 мин», «1 ч 5 мин».
pub fn eta_in(loc: Locale, secs: u64) -> String {
    nikgob_core::i18n::status::eta_in(loc, secs)
}

/// A time left in Russian (callers that have no locale at hand).
pub fn eta_ru(secs: u64) -> String {
    eta_in(Locale::Ru, secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    fn going(done: u64, total: u64, bps: u64) -> Transfer {
        Transfer { phase: TransferPhase::Going, done, total: Some(total), bps }
    }

    /// The percent is the bytes', rounded down, and both tabs say it.
    #[test]
    fn both_tabs_say_the_same_percent() {
        let t = going(37 * MB, 100 * MB, 0);
        assert_eq!(t.percent(), Some(37));
        assert_eq!(t.line(Some("mugilaw")), "mugilaw · 37 МБ из 100 МБ · 37%");
        assert_eq!(t.status(Some("mugilaw")), "качается · mugilaw · 37%");
        // 92.6 % is 92 in both, never 93 in one of them.
        let t = going(926, 1000, 0);
        assert_eq!(t.percent(), Some(92));
        assert!(t.line(None).ends_with("· 92%"));
        assert_eq!(t.status(None), "качается · 92%");
        let job = t.job(1, "x".into(), None, None, true, false);
        assert!((job.progress.unwrap() - 0.926).abs() < 1e-6);
    }

    /// A peer's queue is said as such: no bar, no «0 из 1».
    #[test]
    fn a_queue_is_a_queue() {
        let t = Transfer { phase: TransferPhase::Queued(Some(12)), done: 0, total: Some(30 * MB), bps: 0 };
        assert_eq!(t.percent(), None);
        assert_eq!(t.line(Some("rotted")), "в очереди у rotted · 12-й");
        assert_eq!(t.status(Some("rotted")), "в очереди у rotted · 12-й");
        let job = t.job(1, "House of Memories — Panic! At The Disco".into(), Some("Soulseek"), Some("rotted"), true, false);
        assert!(job.queued && job.progress.is_none());
        assert_eq!(job.subtitle, "Soulseek · в очереди у rotted · 12-й");
        let unknown = Transfer { phase: TransferPhase::Queued(None), ..t };
        assert_eq!(unknown.status(Some("rotted")), "в очереди у rotted");
    }

    #[test]
    fn speed_and_time_left() {
        let t = going(10 * MB, 20 * MB, MB);
        assert_eq!(t.rate().as_deref(), Some("1 МБ/с"));
        assert_eq!(t.eta().as_deref(), Some("10 с"));
        let paused = Transfer { phase: TransferPhase::Paused, ..t };
        assert_eq!(paused.rate(), None);
        assert_eq!(paused.eta(), None);
        assert_eq!(paused.status(None), "на паузе · 50%");
        assert_eq!(eta_ru(3_900), "1 ч 5 мин");
        assert_eq!(eta_ru(180), "3 мин");
    }

    #[test]
    fn without_a_size_only_the_bytes() {
        let t = Transfer { phase: TransferPhase::Going, done: 5 * MB, total: None, bps: 0 };
        assert_eq!(t.percent(), None);
        assert_eq!(t.line(Some("Internet Archive")), "Internet Archive · 5 МБ");
        assert_eq!(t.status(Some("Internet Archive")), "качается · Internet Archive");
        let s = Transfer::default();
        assert_eq!(s.line(Some("Bandcamp")), "Bandcamp · качается");
        assert_eq!(s.job(1, "x".into(), None, None, true, false).progress, None);
        let c = Transfer { phase: TransferPhase::Connecting, ..s };
        assert_eq!(c.line(Some("mugilaw")), "mugilaw · соединяюсь");
    }

    /// The same numbers in the other locale: the words are the only change.
    #[test]
    fn the_lines_say_it_in_english_too() {
        let t = going(37 * MB, 100 * MB, MB);
        assert_eq!(t.line_in(Locale::En, Some("mugilaw")), "mugilaw · 37 MB of 100 MB · 37%");
        assert_eq!(t.status_in(Locale::En, Some("mugilaw")), "downloading · mugilaw · 37%");
        let queued = Transfer { phase: TransferPhase::Queued(Some(12)), done: 0, total: Some(30 * MB), bps: 0 };
        assert_eq!(queued.line_in(Locale::En, Some("rotted")), "queued at rotted · #12");
        let paused = Transfer { phase: TransferPhase::Paused, ..t };
        assert_eq!(paused.status_in(Locale::En, None), "paused · 37%");
        assert_eq!(paused.rate_in(Locale::En), None, "a paused one comes at no speed");
        assert_eq!(eta_in(Locale::En, 3_900), "1 hr 5 min");
        let job = t.job_in(Locale::En, 1, "x".into(), Some("Soulseek"), Some("rotted"), true, false);
        assert_eq!(job.subtitle, "Soulseek · rotted · 37 MB of 100 MB · 37%");
        assert_eq!(job.rate.as_deref(), Some("1.0 MB/s"));
        assert_eq!(job.eta.as_deref(), Some("1 min"));
    }
}
