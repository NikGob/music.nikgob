//! Backend crates: core names and sources, soloist, streaming services, library, metadata, playback.

use std::fmt;

use super::{Locale, common_in};

/// Static strings of the background crates: download sources, soloist,
/// streaming services and library sync.
pub struct BackendStrings {
    /// «сид.» of a torrent offer's detail line («14 сид.» / «14 seeds»).
    pub seeders_short: &'static str,
    /// Soloist: the messages the settings show when its install or run fails.
    pub soloist_unsupported: &'static str,
    pub soloist_wont_run: &'static str,
    pub soloist_only_expired: &'static str,
    pub soloist_redirected: &'static str,
    pub soloist_build_expired: &'static str,
    pub soloist_stopped: &'static str,
    pub soloist_not_connected: &'static str,
    pub soloist_disconnected: &'static str,
    pub soloist_did_not_start: &'static str,
    /// SoundCloud: account and login errors.
    pub soundcloud_session_expired: &'static str,
    pub soundcloud_no_token: &'static str,
    pub soundcloud_login_refused: &'static str,
    /// What the player of a SoundCloud page plays as («Плеер SoundCloud»).
    pub soundcloud_player: &'static str,
    /// Spotify without Premium is not connected at all: what its account
    /// says.
    pub spotify_premium_required: &'static str,
    /// Why syncing is off on this database (see `nikgob-library`'s sync).
    pub sync_epoch_unwritten: &'static str,
}

pub static RU: BackendStrings = BackendStrings {
    seeders_short: "сид.",
    soloist_unsupported: "Spotify не выпускает Soloist для этой системы",
    soloist_wont_run: "Soloist не запускается на этом компьютере",
    soloist_only_expired: "у Spotify пока только просроченная сборка Soloist",
    soloist_redirected: "не скачалось: загрузка ушла с сервера Spotify",
    soloist_build_expired: "Сборка Soloist устарела, новой пока нет",
    soloist_stopped: "Soloist остановился",
    soloist_not_connected: "Spotify Soloist не подключён",
    soloist_disconnected: "Spotify Soloist отключился",
    soloist_did_not_start: "Spotify Soloist не начал трек",
    soundcloud_session_expired: "Вход в SoundCloud истёк — войди снова",
    soundcloud_no_token: "Нет oauth_token SoundCloud — вставь cookies soundcloud.com или токен",
    soundcloud_login_refused: "SoundCloud не принял вход — токен устарел",
    soundcloud_player: "Плеер SoundCloud",
    spotify_premium_required: crate::streaming::SPOTIFY_PREMIUM_REQUIRED,
    sync_epoch_unwritten: "Синхронизация остановлена: метка базы не записана",
};

pub static EN: BackendStrings = BackendStrings {
    seeders_short: "seeds",
    soloist_unsupported: "Spotify Soloist is not available for this system",
    soloist_wont_run: "Soloist will not run on this computer",
    soloist_only_expired: "Spotify only has an expired Soloist build available",
    soloist_redirected: "download failed: redirected away from Spotify's server",
    soloist_build_expired: "The Soloist build has expired and there is no new one yet",
    soloist_stopped: "Soloist stopped",
    soloist_not_connected: "Spotify Soloist is not connected",
    soloist_disconnected: "Spotify Soloist disconnected",
    soloist_did_not_start: "Spotify Soloist did not start the track",
    soundcloud_session_expired: "Your SoundCloud sign-in has expired — sign in again",
    soundcloud_no_token: "No SoundCloud oauth_token — paste soundcloud.com cookies or a token",
    soundcloud_login_refused: "SoundCloud sign-in failed — the token has expired",
    soundcloud_player: "SoundCloud player",
    spotify_premium_required: "Spotify Premium is required to connect; YouTube Music works without it",
    sync_epoch_unwritten: "Sync stopped: the database sync marker was not saved",
};

locale_table! {
    /// Returns these strings for `loc`.
    backend_in -> BackendStrings,
    /// Returns these strings for the active process locale.
    backend,
    { Ru => RU, En => EN }
}

/// Localized display names of the download sources (see `crate::source::SourceKey`).
pub fn source_display_name_in(loc: Locale, key: crate::source::SourceKey) -> &'static str {
    use crate::source::SourceKey as K;
    match key {
        K::Torrents => match loc {
            Locale::Ru => "Торренты",
            Locale::En => "Torrents",
        },
        K::Khinsider => "khinsider",
        K::Soulseek => "Soulseek",
        K::Qobuz => "Qobuz",
        K::Tidal => "Tidal",
        K::Squidify => "Squidify",
        K::Archive => "Internet Archive",
        K::Bandcamp => "Bandcamp",
        K::Soundcloud => "SoundCloud",
        K::Dogmazic => "Dogmazic",
    }
}

/// «312 МБ», «1.4 ГБ» / «312 MB», «1.4 GB».
pub fn human_bytes_in(loc: Locale, b: u64) -> String {
    let s = common_in(loc);
    const MB: f64 = 1024.0 * 1024.0;
    let mb = b as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} {}", mb / 1024.0, s.gb)
    } else if mb >= 10.0 {
        format!("{mb:.0} {}", s.mb)
    } else {
        format!("{mb:.1} {}", s.mb)
    }
}

/// «14 сид.» / «14 seeds»: the seeders of a torrent offer's detail line.
pub fn fmt_seeders_in(loc: Locale, seeders: u32) -> String {
    match (loc, seeders) {
        (Locale::En, 1) => "1 seed".to_owned(),
        _ => format!("{seeders} {}", backend_in(loc).seeders_short),
    }
}

/// What is wrong with a downloaded Soloist archive (an install error's
/// archive payload; see `nikgob-soloist`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoloistArchiveFault {
    /// Above the archive's size limit.
    TooLarge,
    /// An entry's path climbs out of the unpacked folder.
    PathOutside,
    /// Too big a sum, or an entry repeated.
    SizeOrRepeat,
    /// An entry shorter than its headers said.
    Truncated,
    /// More entries than the limit.
    TooManyFiles,
    /// The archive carries a symbolic link.
    Symlink,
    /// The executable never arrived.
    NoExe,
}

impl SoloistArchiveFault {
    /// The fault's own word in `loc` («слишком большой» / «too large»).
    pub const fn word_in(self, loc: Locale) -> &'static str {
        match (loc, self) {
            (Locale::Ru, Self::TooLarge) => "слишком большой",
            (Locale::Ru, Self::PathOutside) => "путь вне папки",
            (Locale::Ru, Self::SizeOrRepeat) => "размер или повтор",
            (Locale::Ru, Self::Truncated) => "обрезан",
            (Locale::Ru, Self::TooManyFiles) => "слишком много файлов",
            (Locale::Ru, Self::Symlink) => "ссылка в архиве",
            (Locale::Ru, Self::NoExe) => "нет программы",
            (Locale::En, Self::TooLarge) => "too large",
            (Locale::En, Self::PathOutside) => "path outside the folder",
            (Locale::En, Self::SizeOrRepeat) => "invalid size or duplicate entry",
            (Locale::En, Self::Truncated) => "truncated",
            (Locale::En, Self::TooManyFiles) => "too many files",
            (Locale::En, Self::Symlink) => "symbolic link in the archive",
            (Locale::En, Self::NoExe) => "missing executable",
        }
    }
}

impl fmt::Display for SoloistArchiveFault {
    /// Log-friendly English (the settings show [`Self::word_in`]).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word_in(Locale::En))
    }
}

/// What is wrong with the signature of a downloaded Soloist build (an
/// install error's signature payload; see `nikgob-soloist`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoloistSignatureFault {
    /// Windows' verification failed with this status.
    Failed(u32),
    /// Signed by another organisation.
    SignedBy(String),
    /// The signer could not be read.
    Unknown,
}

impl SoloistSignatureFault {
    /// The fault's own words in `loc`.
    pub fn word_in(&self, loc: Locale) -> String {
        match self {
            Self::Failed(status) => match loc {
                Locale::Ru => format!("подпись не прошла проверку (0x{status:08x})"),
                Locale::En => format!("the signature check failed (0x{status:08x})"),
            },
            Self::SignedBy(name) => match loc {
                Locale::Ru => format!("подписано «{name}»"),
                Locale::En => format!("signed by “{name}”"),
            },
            Self::Unknown => match loc {
                Locale::Ru => "подписавший не известен".to_owned(),
                Locale::En => "the signer is unknown".to_owned(),
            },
        }
    }
}

impl fmt::Display for SoloistSignatureFault {
    /// Log-friendly English (the settings show [`Self::word_in`]).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.word_in(Locale::En))
    }
}

/// «не скачалось: сеть» / «download failed: network»: a failed download's reason.
pub fn fmt_soloist_network_in(loc: Locale, detail: &str) -> String {
    match loc {
        Locale::Ru => format!("не скачалось: {detail}"),
        Locale::En => format!("download failed: {detail}"),
    }
}

/// «архив Soloist не тот: слишком большой» / «the Soloist archive is wrong: too large».
pub fn fmt_soloist_archive_in(loc: Locale, fault: SoloistArchiveFault) -> String {
    match loc {
        Locale::Ru => format!("архив Soloist не тот: {}", fault.word_in(Locale::Ru)),
        Locale::En => format!("invalid Soloist archive: {}", fault.word_in(Locale::En)),
    }
}

/// [`fmt_soloist_archive_in`] with the unpacking library's own words.
pub fn fmt_soloist_archive_text_in(loc: Locale, detail: &str) -> String {
    match loc {
        Locale::Ru => format!("архив Soloist не тот: {detail}"),
        Locale::En => format!("invalid Soloist archive: {detail}"),
    }
}

/// «у сборки Soloist нет подписи Spotify: …» / «the Soloist build has no Spotify signature: …».
pub fn fmt_soloist_signature_in(loc: Locale, fault: &SoloistSignatureFault) -> String {
    match loc {
        Locale::Ru => format!("у сборки Soloist нет подписи Spotify: {}", fault.word_in(Locale::Ru)),
        Locale::En => format!("the Soloist build has no Spotify signature: {}", fault.word_in(Locale::En)),
    }
}

/// «Не сохранилось: …» / «Could not save: …»: an installed download that was not kept.
pub fn fmt_soloist_save_failed_in(loc: Locale, error: &str) -> String {
    match loc {
        Locale::Ru => format!("Не сохранилось: {error}"),
        Locale::En => format!("Could not save: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_strings_translate_every_field() {
        assert_eq!(backend_in(Locale::Ru).seeders_short, "сид.");
        assert_eq!(backend_in(Locale::En).seeders_short, "seeds");
        for ru in [
            RU.soloist_unsupported,
            RU.soloist_wont_run,
            RU.soloist_only_expired,
            RU.soloist_redirected,
            RU.soloist_build_expired,
            RU.soloist_stopped,
            RU.soloist_not_connected,
            RU.soloist_disconnected,
            RU.soloist_did_not_start,
            RU.soundcloud_session_expired,
            RU.soundcloud_no_token,
            RU.soundcloud_login_refused,
            RU.soundcloud_player,
            RU.spotify_premium_required,
            RU.sync_epoch_unwritten,
        ] {
            assert!(!ru.is_empty());
        }
        for en in [
            EN.soloist_unsupported,
            EN.soloist_wont_run,
            EN.soloist_only_expired,
            EN.soloist_redirected,
            EN.soloist_build_expired,
            EN.soloist_stopped,
            EN.soloist_not_connected,
            EN.soloist_disconnected,
            EN.soloist_did_not_start,
            EN.soundcloud_session_expired,
            EN.soundcloud_no_token,
            EN.soundcloud_login_refused,
            EN.soundcloud_player,
            EN.spotify_premium_required,
            EN.sync_epoch_unwritten,
        ] {
            assert!(!en.is_empty());
        }
    }

    #[test]
    fn sources_sizes_and_seeds_format() {
        assert_eq!(
            source_display_name_in(Locale::Ru, crate::source::SourceKey::Torrents),
            "Торренты"
        );
        assert_eq!(
            source_display_name_in(Locale::En, crate::source::SourceKey::Torrents),
            "Torrents"
        );
        assert_eq!(source_display_name_in(Locale::En, crate::source::SourceKey::Archive), "Internet Archive");
        // 30 MiB and 1.4 GiB exactly as before in Russian.
        assert_eq!(human_bytes_in(Locale::Ru, 40 << 20), "40 МБ");
        assert_eq!(human_bytes_in(Locale::Ru, 7 << 20), "7.0 МБ", "dot decimal as before");
        assert_eq!(human_bytes_in(Locale::En, 40 << 20), "40 MB");
        assert_eq!(fmt_seeders_in(Locale::Ru, 14), "14 сид.");
        assert_eq!(fmt_seeders_in(Locale::Ru, 1), "1 сид.");
        assert_eq!(fmt_seeders_in(Locale::En, 14), "14 seeds");
        assert_eq!(fmt_seeders_in(Locale::En, 1), "1 seed");
    }

    #[test]
    fn soloist_faults_translate() {
        let too_large = SoloistArchiveFault::TooLarge;
        assert_eq!(
            fmt_soloist_archive_in(Locale::Ru, too_large),
            "архив Soloist не тот: слишком большой"
        );
        assert_eq!(
            fmt_soloist_archive_in(Locale::En, too_large),
            "invalid Soloist archive: too large"
        );
        assert_eq!(too_large.to_string(), "too large");
        assert_eq!(
            fmt_soloist_signature_in(Locale::Ru, &SoloistSignatureFault::Failed(0x800b_0109)),
            "у сборки Soloist нет подписи Spotify: подпись не прошла проверку (0x800b0109)"
        );
        assert_eq!(
            fmt_soloist_signature_in(Locale::En, &SoloistSignatureFault::SignedBy("Evil".into())),
            "the Soloist build has no Spotify signature: signed by “Evil”"
        );
        assert_eq!(
            fmt_soloist_network_in(Locale::Ru, "timed out"),
            "не скачалось: timed out"
        );
        assert_eq!(fmt_soloist_network_in(Locale::En, "timed out"), "download failed: timed out");
        assert_eq!(fmt_soloist_save_failed_in(Locale::Ru, "disk"), "Не сохранилось: disk");
        assert_eq!(fmt_soloist_save_failed_in(Locale::En, "disk"), "Could not save: disk");
        assert_eq!(backend_in(Locale::Ru).soloist_build_expired, "Сборка Soloist устарела, новой пока нет");
        assert_eq!(backend_in(Locale::En).soloist_stopped, "Soloist stopped");
    }
}
