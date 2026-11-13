//! The audio engine as the worker drives it: [`Playback`] plus what was
//! last handed to it as the next item, so the same next item is not handed
//! over again (the engine would reopen its file or stream for nothing,
//! right next to a gapless join).

use std::ops::{Deref, DerefMut};

use crate::{PlayRequest, Playback};

/// [`Playback`] that remembers the next item it holds. Every call goes to
/// the engine as it is (through `Deref`), except the ones below, which
/// keep that memory right.
pub(crate) struct Engine {
    inner: Box<dyn Playback>,
    /// The request last handed over with `set_next`, as the engine holds
    /// it now (its speed and gain follow `set_speed` and `set_gain`). None
    /// when nothing was handed or the engine let it go.
    next: Option<PlayRequest>,
}

impl Engine {
    pub(crate) fn new(inner: Box<dyn Playback>) -> Self {
        Self { inner, next: None }
    }

    /// Hands `item` over as the next item, unless the engine already holds
    /// exactly that one (the same entry from the same source at the same
    /// speed and gain). Clearing always goes through.
    pub(crate) fn set_next(&mut self, item: Option<PlayRequest>) {
        if item.is_some() && item == self.next {
            return;
        }
        self.next = item.clone();
        self.inner.set_next(item);
    }

    /// A new item plays: the engine drops what it had prepared.
    pub(crate) fn play(&mut self, item: PlayRequest) {
        self.next = None;
        self.inner.play(item);
    }

    pub(crate) fn stop(&mut self) {
        self.next = None;
        self.inner.stop();
    }

    /// Another device: a restarted engine holds nothing.
    pub(crate) fn set_device(&mut self, device: Option<String>) -> bool {
        self.next = None;
        self.inner.set_device(device)
    }

    pub(crate) fn set_speed(&mut self, id: u64, speed: nikgob_view::SpeedVariant, keep_timbre: bool) {
        if let Some(n) = self.next.as_mut().filter(|n| n.id == id) {
            n.speed = speed;
            n.keep_timbre = keep_timbre;
        }
        self.inner.set_speed(id, speed, keep_timbre);
    }

    pub(crate) fn set_gain(&mut self, id: u64, gain_db: f32) {
        if let Some(n) = self.next.as_mut().filter(|n| n.id == id) {
            n.gain_db = gain_db;
        }
        self.inner.set_gain(id, gain_db);
    }

    /// The engine reported item `id` started (the next one took over),
    /// ended or failed: it no longer holds it as the next item.
    pub(crate) fn let_go(&mut self, id: u64) {
        if self.next.as_ref().is_some_and(|n| n.id == id) {
            self.next = None;
        }
    }

    /// What the engine holds as the next item, as far as it was told.
    #[cfg(test)]
    pub(crate) fn next(&self) -> Option<&PlayRequest> {
        self.next.as_ref()
    }
}

impl Deref for Engine {
    type Target = dyn Playback;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

impl DerefMut for Engine {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.inner
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::PlaySource;

    /// Counts what reaches the engine.
    #[derive(Clone, Default)]
    struct Calls(Arc<Mutex<Vec<String>>>);

    impl Calls {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Playback for Calls {
        fn play(&mut self, item: PlayRequest) {
            self.0.lock().unwrap().push(format!("play {}", item.id));
        }
        fn set_next(&mut self, item: Option<PlayRequest>) {
            self.0.lock().unwrap().push(match item {
                Some(i) => format!("next {}", i.id),
                None => "next none".into(),
            });
        }
        fn pause(&mut self) {}
        fn resume(&mut self) {}
        fn stop(&mut self) {}
        fn seek(&mut self, _: Duration) {
            self.0.lock().unwrap().push("seek".into());
        }
        fn set_volume(&mut self, _: f32) {}
        fn position(&self) -> Duration {
            Duration::ZERO
        }
    }

    fn req(id: u64, file: &str) -> PlayRequest {
        PlayRequest {
            id,
            source: PlaySource::File(PathBuf::from(file)),
            gain_db: 0.0,
            duration: None,
            speed: nikgob_view::SpeedVariant::default(),
            keep_timbre: false,
        }
    }

    #[test]
    fn the_same_next_item_is_handed_over_once() {
        let calls = Calls::default();
        let mut e = Engine::new(Box::new(calls.clone()));
        e.set_next(Some(req(2, "b.flac")));
        e.set_next(Some(req(2, "b.flac")));
        assert_eq!(calls.take(), ["next 2"]);
        // Another entry, or the same one from another file: handed over.
        e.set_next(Some(req(3, "b.flac")));
        e.set_next(Some(req(3, "c.flac")));
        assert_eq!(calls.take(), ["next 3", "next 3"]);
        // A speed the engine was told of already: nothing again; another
        // gain read since it was handed over: told by `set_gain`, the same.
        e.set_speed(3, nikgob_view::SpeedVariant::default(), true);
        e.set_gain(3, -4.0);
        let mut same = req(3, "c.flac");
        same.keep_timbre = true;
        same.gain_db = -4.0;
        e.set_next(Some(same.clone()));
        assert_eq!(calls.take(), Vec::<String>::new());
        // Cleared, it is handed over again; clearing always reaches it.
        e.set_next(None);
        e.set_next(None);
        e.set_next(Some(same.clone()));
        assert_eq!(calls.take(), ["next none", "next none", "next 3"]);
        // Other calls go through untouched.
        e.seek(Duration::from_secs(1));
        assert_eq!(calls.take(), ["seek"]);
    }

    #[test]
    fn what_the_engine_lets_go_is_handed_over_again() {
        let calls = Calls::default();
        let mut e = Engine::new(Box::new(calls.clone()));
        let next = || Some(req(2, "b.flac"));
        // A new item plays: the engine dropped the prepared one.
        e.set_next(next());
        e.play(req(1, "a.flac"));
        e.set_next(next());
        // It took over (repeat-one hands the same entry over again), or
        // failed: again.
        e.let_go(2);
        e.set_next(next());
        // An event of another item changes nothing.
        e.let_go(7);
        e.set_next(next());
        // Stopped, or on another device.
        e.stop();
        e.set_next(next());
        e.set_device(None);
        e.set_next(next());
        assert_eq!(calls.take(), ["next 2", "play 1", "next 2", "next 2", "next 2", "next 2"]);
        assert!(e.next().is_some());
    }
}
