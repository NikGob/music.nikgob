//! Spotify sign-in: the browser page and the login failures.

use super::{Locale, locale};

/// Static strings of this domain.
pub struct SpotifyAuthStrings {
    pub signed_in: &'static str,
    pub sign_in_failed: &'static str,
    pub cancelled: &'static str,
    pub timed_out: &'static str,
    pub login_rejected: &'static str,
    pub unreachable: &'static str,
    pub browser_denied: &'static str,
    pub invalid_grant: &'static str,
    pub invalid_client: &'static str,
    pub invalid_scope: &'static str,
    pub access_denied: &'static str,
    pub retry_sign_in: &'static str,
    pub missing_refresh_token: &'static str,
    pub account_unconfirmed: &'static str,
    pub playback_rejected: &'static str,
}

pub static RU: SpotifyAuthStrings = SpotifyAuthStrings {
    signed_in: "Вход выполнен. Вкладку можно закрыть.",
    sign_in_failed: "Вход не удался",
    cancelled: "Вход отменён",
    timed_out: "Время входа истекло, попробуй снова",
    login_rejected: "Spotify отклонил вход",
    unreachable: "Spotify недоступен",
    browser_denied: "вход отклонён в браузере",
    invalid_grant: "разрешение истекло или отозвано, войди снова",
    invalid_client: "проверь Client ID приложения",
    invalid_scope: "Spotify не принял запрошенные права",
    access_denied: "доступ не разрешён",
    retry_sign_in: "войди снова и проверь настройки приложения",
    missing_refresh_token: "Spotify не выдал refresh-токен",
    account_unconfirmed: "Spotify не подтвердил аккаунт",
    playback_rejected: "Spotify не пустил сессию воспроизведения",
};

pub static EN: SpotifyAuthStrings = SpotifyAuthStrings {
    signed_in: "Signed in. You can close this tab.",
    sign_in_failed: "Sign-in failed",
    cancelled: "Sign-in cancelled",
    timed_out: "Sign-in timed out, try again",
    login_rejected: "Spotify rejected sign-in",
    unreachable: "Spotify is unavailable",
    browser_denied: "sign-in was denied in the browser",
    invalid_grant: "permission expired or was revoked, sign in again",
    invalid_client: "check your app's Client ID",
    invalid_scope: "Spotify did not accept the requested permissions",
    access_denied: "access was denied",
    retry_sign_in: "sign in again and check your app settings",
    missing_refresh_token: "Spotify did not issue a refresh token",
    account_unconfirmed: "Spotify could not verify the account",
    playback_rejected: "Spotify rejected the playback session",
};

locale_table! {
    /// Returns this domain's strings for `loc`.
    spotify_auth_in -> SpotifyAuthStrings,
    /// Returns this domain's strings for the active process locale.
    spotify_auth,
    { Ru => RU, En => EN }
}

/// Formats a busy callback port in `loc`.
pub fn fmt_port_busy_in(loc: Locale, port: u16) -> String {
    match loc {
        Locale::Ru => format!("Порт {port} занят другой программой. Закрой её и попробуй снова."),
        Locale::En => format!("Port {port} is in use by another app. Close it and try again."),
    }
}

/// Formats a busy callback port in the active locale.
pub fn fmt_port_busy(port: u16) -> String {
    fmt_port_busy_in(locale(), port)
}

/// Formats a rejected OAuth grant in `loc`.
pub fn fmt_rejected_in(loc: Locale, status: u16, detail: &str) -> String {
    format!("{} ({status}): {detail}", spotify_auth_in(loc).login_rejected)
}

/// Formats a rejected OAuth grant in the active locale.
pub fn fmt_rejected(status: u16, detail: &str) -> String {
    fmt_rejected_in(locale(), status, detail)
}

/// Formats an unreachable Spotify endpoint in `loc`.
pub fn fmt_unreachable_in(loc: Locale, detail: &str) -> String {
    format!("{}: {detail}", spotify_auth_in(loc).unreachable)
}

/// Formats an unreachable Spotify endpoint in the active locale.
pub fn fmt_unreachable(detail: &str) -> String {
    fmt_unreachable_in(locale(), detail)
}

/// Formats an account verification failure in `loc`.
pub fn fmt_account_unconfirmed_in(loc: Locale, detail: &str) -> String {
    format!("{}: {detail}", spotify_auth_in(loc).account_unconfirmed)
}

/// Formats an account verification failure in the active locale.
pub fn fmt_account_unconfirmed(detail: &str) -> String {
    fmt_account_unconfirmed_in(locale(), detail)
}

/// Formats a playback session failure in `loc`.
pub fn fmt_playback_rejected_in(loc: Locale, detail: &str) -> String {
    format!("{}: {detail}", spotify_auth_in(loc).playback_rejected)
}

/// Formats a playback session failure in the active locale.
pub fn fmt_playback_rejected(detail: &str) -> String {
    fmt_playback_rejected_in(locale(), detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_details_are_formatted_in_both_locales() {
        for (loc, port, rejected, unreachable, account, playback) in [
            (
                Locale::Ru,
                "Порт 8989 занят другой программой. Закрой её и попробуй снова.",
                "Spotify отклонил вход (400): вход отклонён в браузере",
                "Spotify недоступен: HTTP 503",
                "Spotify не подтвердил аккаунт: HTTP 401",
                "Spotify не пустил сессию воспроизведения: disconnected",
            ),
            (
                Locale::En,
                "Port 8989 is in use by another app. Close it and try again.",
                "Spotify rejected sign-in (400): sign-in was denied in the browser",
                "Spotify is unavailable: HTTP 503",
                "Spotify could not verify the account: HTTP 401",
                "Spotify rejected the playback session: disconnected",
            ),
        ] {
            assert_eq!(fmt_port_busy_in(loc, 8989), port);
            assert_eq!(fmt_rejected_in(loc, 400, spotify_auth_in(loc).browser_denied), rejected);
            assert_eq!(fmt_unreachable_in(loc, "HTTP 503"), unreachable);
            assert_eq!(fmt_account_unconfirmed_in(loc, "HTTP 401"), account);
            assert_eq!(fmt_playback_rejected_in(loc, "disconnected"), playback);
        }
    }
}
