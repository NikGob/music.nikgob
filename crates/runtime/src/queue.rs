//! The play queue: what plays now, what comes next, shuffle and repeat.
//!
//! Pure data, no I/O. Every item has a stable entry id so the UI can remove
//! or drag a specific occurrence of a track that appears twice.

use nikgob_core::model::TrackId;
use nikgob_view::Repeat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub track: TrackId,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// Items in play order (shuffle reorders this list itself, keeping
    /// the original order to restore when shuffle goes off).
    items: Vec<Entry>,
    original: Vec<Entry>,
    /// Index of the current item.
    pos: Option<usize>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub context: Option<String>,
    next_id: u64,
}

impl Queue {
    fn entry(&mut self, track: TrackId) -> Entry {
        self.next_id += 1;
        Entry {
            id: self.next_id,
            track,
        }
    }

    /// Replaces the queue and starts at `start`.
    pub fn set(&mut self, tracks: &[TrackId], start: usize, context: Option<String>) {
        let items: Vec<Entry> = tracks.iter().map(|&t| self.entry(t)).collect();
        self.original = items.clone();
        self.items = items;
        self.context = context;
        self.pos = (!self.items.is_empty()).then(|| start.min(self.items.len() - 1));
        if self.shuffle {
            self.shuffle_rest();
        }
    }

    pub fn current(&self) -> Option<Entry> {
        self.pos.and_then(|p| self.items.get(p).copied())
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Items after the current one, in play order.
    pub fn upcoming(&self) -> &[Entry] {
        match self.pos {
            Some(p) => &self.items[(p + 1).min(self.items.len())..],
            None => &self.items,
        }
    }

    /// What plays after the current item when it ends by itself.
    pub fn peek_auto_next(&self) -> Option<Entry> {
        let p = self.pos?;
        match self.repeat {
            Repeat::One => self.items.get(p).copied(),
            Repeat::All if p + 1 >= self.items.len() => self.items.first().copied(),
            _ => self.items.get(p + 1).copied(),
        }
    }

    /// Moves on when the current item ends by itself (respects repeat one).
    pub fn advance_auto(&mut self) -> Option<Entry> {
        let next = self.peek_auto_next();
        if let Some(n) = next {
            self.pos = self.items.iter().position(|e| e.id == n.id);
        } else {
            self.pos = None;
        }
        next
    }

    /// The user pressed "next": repeat-one does not hold it back.
    pub fn skip_forward(&mut self) -> Option<Entry> {
        let p = self.pos?;
        if p + 1 < self.items.len() {
            self.pos = Some(p + 1);
        } else if self.repeat != Repeat::Off && !self.items.is_empty() {
            self.pos = Some(0);
        } else {
            return None;
        }
        self.current()
    }

    pub fn previous(&mut self) -> Option<Entry> {
        let p = self.pos?;
        if p > 0 {
            self.pos = Some(p - 1);
        } else if self.repeat == Repeat::All && !self.items.is_empty() {
            self.pos = Some(self.items.len() - 1);
        }
        self.current()
    }

    /// Jumps to an entry anywhere in the queue.
    pub fn jump(&mut self, entry: u64) -> Option<Entry> {
        self.pos = Some(self.items.iter().position(|e| e.id == entry)?);
        self.current()
    }

    pub fn enqueue(&mut self, tracks: &[TrackId]) {
        let first_new = self.items.len();
        for &t in tracks {
            let e = self.entry(t);
            self.items.push(e);
            self.original.push(e);
        }
        // Nothing is current (empty queue, or it has played out): the first
        // added track becomes current, not a track that already played.
        if self.pos.is_none() && self.items.len() > first_new {
            self.pos = Some(first_new);
        }
    }

    pub fn play_next(&mut self, tracks: &[TrackId]) {
        let at = self.pos.map_or(0, |p| p + 1);
        let entries: Vec<Entry> = tracks.iter().map(|&t| self.entry(t)).collect();
        let oat = self
            .current()
            .and_then(|c| self.original.iter().position(|e| e.id == c.id))
            .map_or(0, |p| p + 1);
        self.items.splice(at..at, entries.iter().copied());
        self.original.splice(oat..oat, entries);
        if self.pos.is_none() && !self.items.is_empty() {
            self.pos = Some(0);
        }
    }

    /// Removes an upcoming (or any non-current) entry.
    pub fn remove(&mut self, entry: u64) -> bool {
        let Some(i) = self.items.iter().position(|e| e.id == entry) else {
            return false;
        };
        if Some(i) == self.pos {
            return false;
        }
        self.items.remove(i);
        self.original.retain(|e| e.id != entry);
        if let Some(p) = self.pos
            && i < p
        {
            self.pos = Some(p - 1);
        }
        true
    }

    /// Moves an upcoming entry to index `to` within the upcoming list.
    pub fn move_upcoming(&mut self, entry: u64, to: usize) -> bool {
        let base = self.pos.map_or(0, |p| p + 1);
        let Some(i) = self.items.iter().position(|e| e.id == entry) else {
            return false;
        };
        if i < base {
            return false;
        }
        let e = self.items.remove(i);
        let to = (base + to).min(self.items.len());
        self.items.insert(to, e);
        if !self.shuffle {
            // `original` is the order shuffle-off restores: keep the edit.
            self.original.clone_from(&self.items);
        }
        true
    }

    /// Drops everything except the current item.
    pub fn clear_upcoming(&mut self) {
        match self.current() {
            Some(c) => {
                self.items = vec![c];
                self.original = vec![c];
                self.pos = Some(0);
            }
            None => {
                self.items.clear();
                self.original.clear();
            }
        }
    }

    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.shuffle_rest();
        } else {
            let cur = self.current();
            self.items = self.original.clone();
            self.pos = cur.and_then(|c| self.items.iter().position(|e| e.id == c.id));
        }
    }

    /// Shuffles the items after the current one (the current one moves to
    /// the front so what plays now does not change).
    fn shuffle_rest(&mut self) {
        let cur = self.current();
        let mut rest: Vec<Entry> = self
            .items
            .iter()
            .copied()
            .filter(|e| Some(e.id) != cur.map(|c| c.id))
            .collect();
        fastrand::shuffle(&mut rest);
        self.items = cur.into_iter().chain(rest).collect();
        self.pos = cur.map(|_| 0);
    }

    pub fn tracks(&self) -> Vec<TrackId> {
        self.items.iter().map(|e| e.track).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<TrackId> {
        (0..n).map(|_| TrackId::new()).collect()
    }

    #[test]
    fn plays_through_and_stops() {
        let t = ids(3);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[1]));
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[2]));
        assert_eq!(q.advance_auto(), None);
    }

    #[test]
    fn repeat_modes() {
        let t = ids(2);
        let mut q = Queue::default();
        q.set(&t, 1, None);
        q.repeat = Repeat::All;
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[0]));
        q.repeat = Repeat::One;
        assert_eq!(q.advance_auto().map(|e| e.track), Some(t[0]));
        // Pressing next skips even with repeat one.
        assert_eq!(q.skip_forward().map(|e| e.track), Some(t[1]));
    }

    #[test]
    fn shuffle_keeps_current_and_restores_order() {
        let t = ids(20);
        let mut q = Queue::default();
        q.set(&t, 5, None);
        q.set_shuffle(true);
        assert_eq!(q.current().map(|e| e.track), Some(t[5]));
        assert_eq!(q.upcoming().len(), 19);
        q.set_shuffle(false);
        assert_eq!(q.current().map(|e| e.track), Some(t[5]));
        assert_eq!(q.tracks(), t);
    }

    #[test]
    fn manual_reorder_survives_shuffle_toggle() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        let up: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
        assert!(q.move_upcoming(up[2], 0)); // t3 first
        q.set_shuffle(true);
        q.set_shuffle(false);
        assert_eq!(q.tracks(), vec![t[0], t[3], t[1], t[2]]);
    }

    #[test]
    fn enqueue_after_the_queue_played_out_starts_with_the_new_track() {
        let t = ids(2);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        assert!(q.advance_auto().is_some());
        assert_eq!(q.advance_auto(), None);
        let extra = ids(1);
        q.enqueue(&extra);
        assert_eq!(q.current().map(|e| e.track), Some(extra[0]));
    }

    #[test]
    fn edit_upcoming() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        let up: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
        assert!(q.remove(up[0]));
        assert_eq!(q.upcoming().len(), 2);
        assert!(q.move_upcoming(up[2], 0));
        assert_eq!(q.upcoming()[0].track, t[3]);
        let extra = ids(1);
        q.play_next(&extra);
        assert_eq!(q.upcoming()[0].track, extra[0]);
        q.clear_upcoming();
        assert!(q.upcoming().is_empty());
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
    }
}
