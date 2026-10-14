//! Bounded cache for results computed from a pair of commits.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use kata_core::CommitId;

/// Least-recently-used map from a `(from, to)` commit pair to a value
/// computed from the two commits. Commit ids name immutable content, so
/// an entry never goes stale and needs no invalidation. Once the cache
/// holds `capacity` entries, an insert evicts the least recently used
/// one.
pub struct PairCache<V> {
    capacity: usize,
    inner: Mutex<Entries<V>>,
}

struct Entries<V> {
    values: HashMap<(CommitId, CommitId), Arc<V>>,
    /// Keys from least to most recently used.
    order: VecDeque<(CommitId, CommitId)>,
}

impl<V> PairCache<V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            inner: Mutex::new(Entries {
                values: HashMap::new(),
                order: VecDeque::new(),
            }),
        }
    }

    pub fn get(&self, from: &CommitId, to: &CommitId) -> Option<Arc<V>> {
        let key = (from.clone(), to.clone());
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let value = inner.values.get(&key).cloned()?;
        inner.touch(&key);
        Some(value)
    }

    /// Stores `value` for the pair and returns it as the shared handle
    /// later `get` calls see.
    pub fn insert(&self, from: &CommitId, to: &CommitId, value: V) -> Arc<V> {
        let key = (from.clone(), to.clone());
        let value = Arc::new(value);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.values.insert(key.clone(), value.clone()).is_some() {
            inner.touch(&key);
        } else {
            inner.order.push_back(key);
        }
        while inner.order.len() > self.capacity {
            if let Some(oldest) = inner.order.pop_front() {
                inner.values.remove(&oldest);
            }
        }
        value
    }
}

impl<V> Entries<V> {
    fn touch(&mut self, key: &(CommitId, CommitId)) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let key = self.order.remove(pos).expect("position is in range");
            self.order.push_back(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> CommitId {
        CommitId::new(s.to_string())
    }

    #[test]
    fn get_returns_the_inserted_value() {
        let cache = PairCache::new(2);
        cache.insert(&id("a"), &id("b"), 1);
        assert_eq!(cache.get(&id("a"), &id("b")).as_deref(), Some(&1));
        assert_eq!(cache.get(&id("b"), &id("a")), None);
    }

    #[test]
    fn insert_evicts_the_least_recently_used_pair() {
        let cache = PairCache::new(2);
        cache.insert(&id("a"), &id("b"), 1);
        cache.insert(&id("c"), &id("d"), 2);
        // Reading (a, b) makes (c, d) the least recently used.
        cache.get(&id("a"), &id("b"));
        cache.insert(&id("e"), &id("f"), 3);
        assert_eq!(cache.get(&id("c"), &id("d")), None);
        assert_eq!(cache.get(&id("a"), &id("b")).as_deref(), Some(&1));
        assert_eq!(cache.get(&id("e"), &id("f")).as_deref(), Some(&3));
    }

    #[test]
    fn reinserting_a_pair_replaces_its_value() {
        let cache = PairCache::new(2);
        cache.insert(&id("a"), &id("b"), 1);
        cache.insert(&id("a"), &id("b"), 2);
        assert_eq!(cache.get(&id("a"), &id("b")).as_deref(), Some(&2));
    }
}
