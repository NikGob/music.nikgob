//! «Таймер сна»: what the player's ⋮ offers and what a running timer
//! shows.
//!
//! A timer of minutes counts the clock (a pause does not stop it, as in
//! Apple Music and Spotify); one until a track or the album ends waits for
//! the music (it ends when the music gets there). Either fades the sound out
//! over its last [`SLEEP_FADE`] and pauses. Nothing of it is kept between
//! runs.

use std::time::{Duration, Instant};

use crate::{PlayerView, Route, heard_time};

/// A choice of «Таймер сна».
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SleepChoice {
    /// In this many minutes.
    Minutes(u16),
    /// When the playing track ends (the one playing then, after a skip).
    TrackEnd,
    /// When the album, playlist or queue playing now ends.
    ContextEnd,
    /// «Выключить таймер».
    Off,
}

/// The timers of minutes the menu offers.
pub const SLEEP_MINUTES: [u16; 5] = [15, 30, 45, 60, 90];

/// The sound fades out over this much before the timer pauses it.
pub const SLEEP_FADE: Duration = Duration::from_secs(20);

/// What the queue plays, for the name of «До конца …».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepContext {
    Album,
    Playlist,
    Queue,
}

impl SleepContext {
    /// What `player`'s queue was started from.
    pub fn of(player: &PlayerView) -> Self {
        match player.context_page {
            Some(Route::Release(_)) => Self::Album,
            Some(Route::Playlist(_)) => Self::Playlist,
            _ => Self::Queue,
        }
    }
}

/// A running timer as the player shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SleepView {
    /// What was chosen (never [`SleepChoice::Off`]).
    pub choice: SleepChoice,
    /// A timer of minutes: when it ends.
    pub deadline: Option<Instant>,
    /// A timer until a track or the queue ends: how long what plays after
    /// the current track until then lasts.
    pub after_current: Duration,
}

impl SleepView {
    /// How long until the timer pauses, as of `now` (the current track's
    /// rest as the player has it now).
    pub fn left(&self, player: &PlayerView, now: Instant) -> Duration {
        match self.deadline {
            Some(deadline) => deadline.saturating_duration_since(now),
            None => {
                let rest = player
                    .duration
                    .map(|d| heard_time(d.saturating_sub(player.position_now()), player.speed))
                    .unwrap_or_default();
                rest + self.after_current
            }
        }
    }
}

/// What the timer key (T) turns the timer to: the first timer of minutes
/// longer than what is left (15 when none runs), off after the longest.
pub fn sleep_key_step(left: Option<Duration>) -> SleepChoice {
    let Some(left) = left else { return SleepChoice::Minutes(SLEEP_MINUTES[0]) };
    // Half a minute of slack: 15 just set and pressed again is 30.
    let minutes = left.as_secs_f32() / 60.0 + 0.5;
    SLEEP_MINUTES
        .iter()
        .find(|m| f32::from(**m) > minutes)
        .map_or(SleepChoice::Off, |m| SleepChoice::Minutes(*m))
}

/// What the timer's chip says is left: whole minutes rounded up, the
/// seconds of the last minute.
pub fn sleep_left_label(left: Duration) -> String {
    let secs = left.as_secs();
    if secs < 60 {
        format!("{secs} с")
    } else {
        format!("{} мин", secs.div_ceil(60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_walks_up_the_minutes_and_then_off() {
        assert_eq!(sleep_key_step(None), SleepChoice::Minutes(15));
        assert_eq!(sleep_key_step(Some(Duration::from_secs(15 * 60))), SleepChoice::Minutes(30));
        assert_eq!(sleep_key_step(Some(Duration::from_secs(15 * 60 - 3))), SleepChoice::Minutes(30));
        assert_eq!(sleep_key_step(Some(Duration::from_secs(44 * 60))), SleepChoice::Minutes(45));
        assert_eq!(sleep_key_step(Some(Duration::from_secs(60 * 60))), SleepChoice::Minutes(90));
        assert_eq!(sleep_key_step(Some(Duration::from_secs(90 * 60))), SleepChoice::Off);
        // Until a track ends, three minutes left: 15 minutes.
        assert_eq!(sleep_key_step(Some(Duration::from_secs(180))), SleepChoice::Minutes(15));
    }

    #[test]
    fn the_chip_rounds_minutes_up() {
        assert_eq!(sleep_left_label(Duration::from_secs(45)), "45 с");
        assert_eq!(sleep_left_label(Duration::from_secs(60)), "1 мин");
        assert_eq!(sleep_left_label(Duration::from_secs(61)), "2 мин");
        assert_eq!(sleep_left_label(Duration::from_secs(29 * 60 + 59)), "30 мин");
    }

    #[test]
    fn time_left_counts_the_clock_or_the_music() {
        let now = Instant::now();
        let minutes = SleepView { choice: SleepChoice::Minutes(15), deadline: Some(now + Duration::from_secs(600)), after_current: Duration::ZERO };
        assert_eq!(minutes.left(&PlayerView::default(), now), Duration::from_secs(600));
        assert_eq!(minutes.left(&PlayerView::default(), now + Duration::from_secs(700)), Duration::ZERO);
        let player = PlayerView {
            duration: Some(Duration::from_secs(200)),
            position: Duration::from_secs(50),
            ..PlayerView::default()
        };
        let album = SleepView { choice: SleepChoice::ContextEnd, deadline: None, after_current: Duration::from_secs(300) };
        assert_eq!(album.left(&player, now), Duration::from_secs(450));
        // At twice the speed the rest of the track is heard in half.
        let fast = PlayerView { speed: 2.0, ..player };
        assert_eq!(album.left(&fast, now), Duration::from_secs(375));
    }

    #[test]
    fn the_context_is_named_by_its_page() {
        let mut p = PlayerView::default();
        assert_eq!(SleepContext::of(&p), SleepContext::Queue);
        p.context_page = Some(Route::Release(crate::ReleaseId::new()));
        assert_eq!(SleepContext::of(&p), SleepContext::Album);
    }
}
