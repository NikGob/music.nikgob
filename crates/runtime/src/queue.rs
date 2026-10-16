//! The play queue: what plays now, what comes next, shuffle and repeat.
//!
//! Pure data, no I/O. Every item has a stable entry id so the UI can remove
//! or drag a specific occurrence of a track that appears twice.

use nikgob_core::model::TrackId;
use nikgob_view::Repeat;
use serde::{Deserialize, Serialize};

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

/// A queue as saved between runs (entry ids are not kept: they are only
/// meaningful within one run).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// Tracks in play order.
    pub items: Vec<TrackId>,
    /// The order to go back to when shuffle goes off, as indices into `items`.
    pub original: Vec<usize>,
    pub pos: Option<usize>,
    pub shuffle: bool,
    /// 0 off, 1 all, 2 one.
    pub repeat: u8,
    pub context: Option<String>,
}

impl Queue {
    pub fn snapshot(&self) -> Snapshot {
        let original = self
            .original
            .iter()
            .filter_map(|o| self.items.iter().position(|e| e.id == o.id))
            .collect();
        Snapshot {
            items: self.items.iter().map(|e| e.track).collect(),
            original,
            pos: self.pos,
            shuffle: self.shuffle,
            repeat: match self.repeat {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            },
            context: self.context.clone(),
        }
    }

    /// Replaces the queue with a saved one (fresh entry ids).
    pub fn restore(&mut self, s: &Snapshot) {
        self.items = s.items.iter().map(|&t| self.entry(t)).collect();
        let original: Vec<Entry> = s.original.iter().filter_map(|&i| self.items.get(i).copied()).collect();
        // A damaged file: fall back to the play order.
        self.original = if original.len() == self.items.len() { original } else { self.items.clone() };
        self.pos = s.pos.filter(|&p| p < self.items.len());
        self.shuffle = s.shuffle;
        self.repeat = match s.repeat {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        };
        self.context = s.context.clone();
    }

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

    /// Items before the current one, in play order: what has played (or
    /// was skipped past) and what "previous" goes back to.
    pub fn history(&self) -> &[Entry] {
        match self.pos {
            Some(p) => &self.items[..p.min(self.items.len())],
            None => &[],
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

    /// Moves `entry` right after (or before) `anchor`, anywhere in the queue:
    /// a played track can go back among the upcoming ones and the other way
    /// round. The current item itself never moves and stays current.
    pub fn move_next_to(&mut self, entry: u64, anchor: u64, after: bool) -> bool {
        let current = self.current().map(|c| c.id);
        if entry == anchor || current == Some(entry) {
            return false;
        }
        let Some(i) = self.items.iter().position(|e| e.id == entry) else {
            return false;
        };
        if !self.items.iter().any(|e| e.id == anchor) {
            return false;
        }
        let e = self.items.remove(i);
        let j = self.items.iter().position(|e| e.id == anchor).unwrap_or(self.items.len());
        self.items.insert(if after { j + 1 } else { j }, e);
        self.pos = current.and_then(|c| self.items.iter().position(|e| e.id == c));
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
        assert!(q.move_next_to(up[2], up[0], false)); // t3 first
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
    fn history_is_what_came_before_and_jumping_back_works() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        assert!(q.history().is_empty());
        q.advance_auto();
        q.skip_forward();
        let back: Vec<TrackId> = q.history().iter().map(|e| e.track).collect();
        assert_eq!(back, vec![t[0], t[1]]);
        let first = q.history()[0].id;
        assert_eq!(q.jump(first).map(|e| e.track), Some(t[0]));
        assert!(q.history().is_empty());
        assert_eq!(q.upcoming().len(), 3);
        // Played out: nothing is current, so nothing is "before" it.
        q.set(&t, 3, None);
        assert_eq!(q.history().len(), 3);
        assert_eq!(q.advance_auto(), None);
        assert!(q.history().is_empty());
    }

    #[test]
    fn played_tracks_can_be_moved_and_removed_like_upcoming_ones() {
        let t = ids(5);
        let mut q = Queue::default();
        q.set(&t, 2, None);
        let all: Vec<u64> = q.history().iter().chain(q.current().iter()).chain(q.upcoming()).map(|e| e.id).collect();
        let cur = all[2];
        // A played track back after the current one: it plays next.
        assert!(q.move_next_to(all[0], cur, true));
        assert_eq!(q.current().map(|e| e.id), Some(cur));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[1]]);
        assert_eq!(q.upcoming()[0].track, t[0]);
        // An upcoming one into the history, before the current.
        assert!(q.move_next_to(all[4], cur, false));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[1], t[4]]);
        assert_eq!(q.current().map(|e| e.track), Some(t[2]));
        // Reorder inside the history, then drop one of it.
        assert!(q.move_next_to(all[4], all[1], false));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[4], t[1]]);
        assert!(q.remove(all[1]));
        assert_eq!(q.history().iter().map(|e| e.track).collect::<Vec<_>>(), vec![t[4]]);
        assert_eq!(q.current().map(|e| e.id), Some(cur));
        // The current item never moves.
        assert!(!q.move_next_to(cur, all[4], false));
        assert!(!q.move_next_to(all[4], all[4], true));
    }

    #[test]
    fn a_saved_queue_comes_back_the_same() {
        let t = ids(5);
        let mut q = Queue::default();
        q.set(&t, 2, Some("Альбом".into()));
        q.set_shuffle(true);
        q.repeat = Repeat::All;
        let saved = q.snapshot();
        let json = serde_json::to_string(&saved).unwrap();
        let mut back = Queue::default();
        back.restore(&serde_json::from_str(&json).unwrap());
        assert_eq!(back.snapshot(), saved);
        assert_eq!(back.current().map(|e| e.track), q.current().map(|e| e.track));
        back.set_shuffle(false);
        q.set_shuffle(false);
        assert_eq!(back.tracks(), q.tracks());
    }

    #[test]
    fn edit_upcoming() {
        let t = ids(4);
        let mut q = Queue::default();
        q.set(&t, 0, None);
        let up: Vec<u64> = q.upcoming().iter().map(|e| e.id).collect();
        assert!(q.remove(up[0]));
        assert_eq!(q.upcoming().len(), 2);
        assert!(q.move_next_to(up[2], up[1], false));
        assert_eq!(q.upcoming()[0].track, t[3]);
        let extra = ids(1);
        q.play_next(&extra);
        assert_eq!(q.upcoming()[0].track, extra[0]);
        q.clear_upcoming();
        assert!(q.upcoming().is_empty());
        assert_eq!(q.current().map(|e| e.track), Some(t[0]));
    }
}
