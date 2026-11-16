//! `GET /v1/events`: Server-Sent Events. The state at once, then whenever
//! it changes (a song, a pause, the volume, the duck, a seek) and never
//! for the clock alone: a client moves the position by `at_ms` and `rate`.
//! Bursts are folded into one event, at most ten a second; `id` is the
//! state's revision; `: keepalive` every 15 s of silence.
//!
//! Each client's stream makes its next event only when the connection has
//! taken the last one, from the state as it is then: nothing piles up for
//! a slow client.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio::sync::watch;

use crate::error::ApiError;
use crate::state::PlayerState;
use crate::{Closing, Shared};

/// At most this many event streams at once.
pub const MAX_CLIENTS: usize = 16;
/// At most one event per this.
pub const MIN_GAP: Duration = Duration::from_millis(100);
/// A position this far (ms) from where the last event's clock puts it is
/// a seek.
pub const JUMP_MS: f64 = 1500.0;

/// Tells what is news to a client: anything but the clock, or a position
/// that jumped.
#[derive(Default)]
pub struct Coalescer {
    key: Option<String>,
    anchor: Option<(i64, i64, f32)>,
}

impl Coalescer {
    /// Whether `s` is to be sent; if so it becomes what the client knows.
    pub fn news(&mut self, s: &PlayerState) -> bool {
        let key = s.change_key();
        let jumped = match self.anchor {
            Some((pos, at, rate)) => {
                let expected = pos as f64 + (s.at_ms - at) as f64 * f64::from(rate);
                (s.position_ms as f64 - expected).abs() > JUMP_MS
            }
            None => true,
        };
        let news = jumped || self.key.as_deref() != Some(key.as_str());
        if news {
            self.key = Some(key);
            self.anchor = Some((s.position_ms, s.at_ms, s.rate));
        }
        news
    }
}

/// Counts a client while its stream lives.
struct Client(Arc<Shared>);

impl Drop for Client {
    fn drop(&mut self) {
        self.0.sse_clients.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Feed {
    client: Client,
    changes: watch::Receiver<u64>,
    closing: watch::Receiver<bool>,
    known: Coalescer,
    first: bool,
    sent_at: Option<tokio::time::Instant>,
}

async fn closed(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|c| *c).await;
}

/// The next event of a client: the first state at once, then the next
/// change that is news.
async fn next_event(mut f: Feed) -> Option<(Result<Event, Infallible>, Feed)> {
    loop {
        if !f.first {
            tokio::select! {
                r = f.changes.changed() => {
                    if r.is_err() {
                        return None;
                    }
                }
                () = closed(&mut f.closing) => return None,
            }
            if let Some(at) = f.sent_at {
                let due = at + MIN_GAP;
                if tokio::time::Instant::now() < due {
                    tokio::time::sleep_until(due).await;
                }
            }
            // What changed meanwhile is in the state read now.
            f.changes.borrow_and_update();
        }
        let (state, revision) = {
            let st = f.client.0.rt.state();
            (PlayerState::of(&st, nikgob_library::now_ms()), st.revision)
        };
        let news = f.known.news(&state);
        if f.first || news {
            f.first = false;
            f.sent_at = Some(tokio::time::Instant::now());
            let data = serde_json::to_string(&state).unwrap_or_default();
            let ev = Event::default().event("state").id(revision.to_string()).data(data);
            return Some((Ok(ev), f));
        }
    }
}

#[utoipa::path(
    get,
    path = "/v1/events",
    tag = "player",
    responses(
        (status = 200, description = "text/event-stream: `event: state` с тем же JSON, что у GET /v1/state"),
        (status = 429, body = crate::error::ErrorBody),
    ),
    security(("bearer" = []), ("token" = []))
)]
pub async fn events(State(sh): State<Arc<Shared>>, Extension(closing): Extension<Closing>) -> Response {
    if sh.sse_clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
        sh.sse_clients.fetch_sub(1, Ordering::AcqRel);
        return ApiError::rate_limited().into_response();
    }
    let feed = Feed {
        changes: sh.wake.subscribe(),
        client: Client(sh),
        closing: closing.0,
        known: Coalescer::default(),
        first: true,
        sent_at: None,
    };
    let stream = futures_util::stream::unfold(feed, next_event);
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keepalive")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{OutputRef, PlayStatus, RepeatMode};

    fn state(status: PlayStatus, pos: i64, at: i64, volume: f32) -> PlayerState {
        PlayerState {
            status,
            track: None,
            next: None,
            position_ms: pos,
            at_ms: at,
            server_time_ms: at,
            rate: if status == PlayStatus::Playing { 1.0 } else { 0.0 },
            volume,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            output: OutputRef { id: "local:default".into(), name: "По умолчанию".into() },
            duck: None,
        }
    }

    /// The first state is news; the clock moving on is not; a pause, the
    /// volume and a seek are.
    #[test]
    fn only_changes_are_news() {
        let mut c = Coalescer::default();
        assert!(c.news(&state(PlayStatus::Playing, 1_000, 10_000, 0.5)));
        assert!(!c.news(&state(PlayStatus::Playing, 2_000, 11_000, 0.5)), "the clock alone");
        assert!(!c.news(&state(PlayStatus::Playing, 6_100, 15_000, 0.5)), "a little drift");
        assert!(c.news(&state(PlayStatus::Playing, 6_000, 15_000, 0.6)), "the volume");
        assert!(c.news(&state(PlayStatus::Playing, 60_000, 15_100, 0.6)), "a seek");
        assert!(c.news(&state(PlayStatus::Paused, 60_100, 15_200, 0.6)), "a pause");
        assert!(!c.news(&state(PlayStatus::Paused, 60_100, 25_000, 0.6)), "paused, time passing");
        assert!(c.news(&state(PlayStatus::Paused, 0, 25_100, 0.6)), "a seek while paused");
    }
}
