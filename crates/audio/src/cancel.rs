//! Stream reads that give way to commands.
//!
//! The decoder thread both reads the bytes of a stream and takes the
//! commands, so a read waiting for the network (a stalled download) would
//! hold Stop, Play, Next or Seek up for as long as it waits; an opener
//! thread probing a stream nobody wants any more would wait just as long.
//!
//! Every item with bytes to open (a stream or a file) that is handed to the
//! [`Player`](crate::Player) gets a [`Slot`]. Once the item's
//! [`ByteSource`](crate::ByteSource) exists, its [`Interrupter`] goes into
//! the slot. The player's methods run on the caller's thread and go through
//! [`Slots`]: a play, a stop or the engine's end cancel every item handed
//! over before, a new next item (or none) cancels the next one that has not
//! started, a seek interrupts the playing item. A read waiting in a
//! cancelled or interrupted stream returns an error at once, so the decoder
//! comes back to its commands; it knows from the slot that the error is not
//! the stream's own failure and reports nothing. An interrupted item waits
//! again once the decoder has taken the seek ([`Slot::take_interrupt`]).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Makes the reads of a [`ByteSource`](crate::ByteSource) that wait for
/// bytes give up: called with `true`, a read waiting now returns an error
/// at once, and so does every later read that would have to wait, until
/// it is called with `false`. Reads of bytes already there go on as
/// usual. Called from any thread, also while a read waits.
pub type Interrupter = Arc<dyn Fn(bool) + Send + Sync>;

#[derive(Default)]
struct State {
    /// The source's interrupter, once the item's opener gave the source.
    handle: Option<Interrupter>,
    /// Superseded: the item will never be heard (again).
    cancelled: bool,
    /// Interrupted for a seek the decoder has not taken yet.
    interrupted: bool,
    /// The item plays: it was played, or it took over as the next item. A
    /// new next item no longer supersedes it.
    started: bool,
    /// Its opener is done: the decoder reads it (a seek may interrupt it).
    opened: bool,
}

/// What the engine knows about the reads of one item; see the module docs.
pub(crate) struct Slot {
    id: u64,
    state: Mutex<State>,
}

impl Slot {
    /// A slot for item `id`; `started` for an item that plays at once.
    pub(crate) fn new(id: u64, started: bool) -> Arc<Self> {
        Arc::new(Self {
            id,
            state: Mutex::new(State {
                started,
                ..State::default()
            }),
        })
    }

    // The interrupter is called under this lock, so a cancel, an interrupt
    // and its end reach the source in the order they happen here. It only
    // takes the source's own lock, which nothing holds while taking this.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The opened source's interrupter. False when the item was cancelled
    /// meanwhile: its open stops here.
    pub(crate) fn attach(&self, handle: Option<Interrupter>) -> bool {
        let mut st = self.lock();
        if st.cancelled {
            return false;
        }
        st.handle = handle;
        true
    }

    /// The item is open: from now on the decoder reads it.
    pub(crate) fn mark_opened(&self) {
        self.lock().opened = true;
    }

    /// The item is superseded: whatever waits in its reads gives up, now
    /// and later.
    pub(crate) fn cancel(&self) {
        let mut st = self.lock();
        if st.cancelled {
            return;
        }
        st.cancelled = true;
        if let Some(h) = &st.handle {
            h(true);
        }
    }

    /// Cancels the item unless it started; whether it is cancelled now.
    fn cancel_unless_started(&self) -> bool {
        let mut st = self.lock();
        if st.started {
            return false;
        }
        st.cancelled = true;
        if let Some(h) = &st.handle {
            h(true);
        }
        true
    }

    /// A seek of the playing item: a read waiting in it gives up until the
    /// decoder takes the seek. Nothing for an item still opening (its
    /// probe must not fail) or not playing.
    fn interrupt(&self) {
        let mut st = self.lock();
        if st.cancelled || !st.started || !st.opened {
            return;
        }
        st.interrupted = true;
        if let Some(h) = &st.handle {
            h(true);
        }
    }

    /// Ends an interruption: reads wait for their bytes again. Whether the
    /// item was interrupted (so a read that just failed was made to).
    pub(crate) fn take_interrupt(&self) -> bool {
        let mut st = self.lock();
        if st.cancelled || !st.interrupted {
            return false;
        }
        st.interrupted = false;
        if let Some(h) = &st.handle {
            h(false);
        }
        true
    }

    /// The next item takes over (or its crossfade begins). False when it
    /// was superseded meanwhile: it must not start.
    pub(crate) fn start(&self) -> bool {
        let mut st = self.lock();
        if st.cancelled {
            return false;
        }
        st.started = true;
        true
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }
}

/// Whether the item behind `slot` was cancelled (no slot: never).
pub(crate) fn cancelled(slot: Option<&Arc<Slot>>) -> bool {
    slot.is_some_and(|s| s.is_cancelled())
}

/// The slots of the items handed to the engine and still held by it (an
/// item the engine let go holds no reference to its slot any more, which
/// is how it is forgotten here). A handful at most: what plays, what is
/// next, what is still opening.
#[derive(Default)]
pub(crate) struct Slots(Mutex<Vec<Arc<Slot>>>);

impl Slots {
    fn lock(&self) -> MutexGuard<'_, Vec<Arc<Slot>>> {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        all.retain(|s| Arc::strong_count(s) > 1);
        all
    }

    /// A play, a stop, the engine's end: everything handed over before is
    /// superseded. `then` is the new item's slot.
    pub(crate) fn cancel_all(&self, then: Option<Arc<Slot>>) {
        let mut all = self.lock();
        for s in all.drain(..) {
            s.cancel();
        }
        all.extend(then);
    }

    /// A new next item (`then`), or none: the next item handed over before
    /// is superseded unless it already took over.
    pub(crate) fn cancel_next(&self, then: Option<Arc<Slot>>) {
        let mut all = self.lock();
        all.retain(|s| !s.cancel_unless_started());
        all.extend(then);
    }

    /// A seek of item `id`.
    pub(crate) fn interrupt(&self, id: u64) {
        for s in self.lock().iter().filter(|s| s.id == id) {
            s.interrupt();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::*;

    /// An interrupter that records the last call (0 none, 1 off, 2 on).
    fn probe() -> (Interrupter, Arc<AtomicU8>) {
        let last = Arc::new(AtomicU8::new(0));
        let l = Arc::clone(&last);
        (
            Arc::new(move |on| l.store(if on { 2 } else { 1 }, Ordering::SeqCst)),
            last,
        )
    }

    #[test]
    fn a_cancel_reaches_the_source_also_when_it_comes_before_the_source() {
        let early = Slot::new(1, true);
        early.cancel();
        let (h, _) = probe();
        assert!(!early.attach(Some(h)), "the open stops");

        let late = Slot::new(2, true);
        let (h, last) = probe();
        assert!(late.attach(Some(h)));
        late.cancel();
        assert_eq!(last.load(Ordering::SeqCst), 2);
        assert!(!late.start());
    }

    #[test]
    fn a_new_next_item_cancels_only_a_next_item_that_has_not_started() {
        let slots = Slots::default();
        let playing = Slot::new(1, true);
        let next = Slot::new(2, false);
        let held = [Arc::clone(&playing), Arc::clone(&next)];
        slots.cancel_all(Some(Arc::clone(&playing)));
        slots.cancel_next(Some(Arc::clone(&next)));
        slots.cancel_next(Some(Slot::new(3, false)));
        assert!(next.is_cancelled());
        assert!(!playing.is_cancelled());

        // A next item that took over is not superseded by the one after it.
        let took = Slot::new(4, false);
        slots.cancel_next(Some(Arc::clone(&took)));
        assert!(took.start());
        slots.cancel_next(None);
        assert!(!took.is_cancelled());
        slots.cancel_all(None);
        assert!(took.is_cancelled() && playing.is_cancelled());
        drop(held);
    }

    #[test]
    fn an_interrupt_lasts_until_the_decoder_takes_it_and_spares_an_opening_item() {
        let slots = Slots::default();
        let s = Slot::new(7, true);
        slots.cancel_all(Some(Arc::clone(&s)));
        let (h, last) = probe();
        assert!(s.attach(Some(h)));
        slots.interrupt(7);
        assert_eq!(last.load(Ordering::SeqCst), 0, "still opening");
        s.mark_opened();
        slots.interrupt(8);
        assert!(!s.take_interrupt());
        slots.interrupt(7);
        assert_eq!(last.load(Ordering::SeqCst), 2);
        assert!(s.take_interrupt());
        assert_eq!(last.load(Ordering::SeqCst), 1);
        assert!(!s.take_interrupt());
    }

    #[test]
    fn items_the_engine_let_go_are_forgotten() {
        let slots = Slots::default();
        slots.cancel_all(Some(Slot::new(1, true)));
        slots.cancel_next(Some(Slot::new(2, false)));
        assert!(slots.lock().is_empty());
    }
}
