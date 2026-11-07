//! A small in-memory cache with a time to live, so opening the same album,
//! artist or playlist again does not spend quota.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub struct TtlCache<T> {
    ttl: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, (Instant, T)>>,
}

impl<T: Clone> TtlCache<T> {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            ttl,
            capacity,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<T> {
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

    pub fn put(&self, key: &str, value: T) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.len() >= self.capacity && !entries.contains_key(key) {
            entries.retain(|_, (at, _)| at.elapsed() < self.ttl);
            if entries.len() >= self.capacity
                && let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(key.to_string(), (Instant::now(), value));
    }

    pub fn remove(&self, key: &str) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_expire_and_the_oldest_makes_room() {
        let cache = TtlCache::new(Duration::from_millis(50), 2);
        cache.put("a", 1);
        cache.put("b", 2);
        cache.put("c", 3);
        assert_eq!(cache.get("a"), None, "the oldest went");
        assert_eq!(cache.get("c"), Some(3));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(cache.get("b"), None);
        cache.put("d", 4);
        cache.remove("d");
        assert_eq!(cache.get("d"), None);
    }
}
