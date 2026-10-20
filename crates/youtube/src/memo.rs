//! Discovery answers kept in memory for a while (an artist page's top songs,
//! similar artists and radio id; related tracks; radios), so the daily
//! "new for you" pass and an artist opened twice do not ask YouTube again.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub(crate) struct Memo<T> {
    ttl: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, (Instant, T)>>,
}

impl<T: Clone> Memo<T> {
    pub(crate) fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<T> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        match entries.get(key) {
            Some((at, value)) if at.elapsed() < self.ttl => Some(value.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    /// Keeps `value`; when full, the expired entries go, then the oldest.
    pub(crate) fn put(&self, key: &str, value: T) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.len() >= self.capacity && !entries.contains_key(key) {
            entries.retain(|_, (at, _)| at.elapsed() < self.ttl);
            if entries.len() >= self.capacity
                && let Some(oldest) = entries.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(key.to_owned(), (Instant::now(), value));
    }

    pub(crate) fn remove(&self, key: &str) {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_expire_and_the_oldest_makes_room() {
        let memo = Memo::new(Duration::from_millis(50), 2);
        memo.put("a", 1);
        memo.put("b", 2);
        memo.put("c", 3);
        assert_eq!(memo.get("a"), None, "the oldest went");
        assert_eq!(memo.get("c"), Some(3));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(memo.get("b"), None);
        memo.put("d", 4);
        memo.remove("d");
        assert_eq!(memo.get("d"), None);
    }
}
