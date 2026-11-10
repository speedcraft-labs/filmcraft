//! A byte-budgeted LRU cache of decoded/rendered frames, shared by monitors, thumbnails and export.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use filmcraft_frame::VideoFrame;

struct Entry<K> {
    key: K,
    frame: Arc<VideoFrame>,
    bytes: usize,
    older: Option<usize>,
    newer: Option<usize>,
}

struct Inner<K> {
    map: HashMap<K, usize>,
    entries: Vec<Option<Entry<K>>>,
    free: Vec<usize>,
    oldest: Option<usize>,
    newest: Option<usize>,
    bytes: usize,
    hits: u64,
    misses: u64,
}

impl<K: Eq + Hash> Inner<K> {
    fn unlink(&mut self, older: Option<usize>, newer: Option<usize>) {
        if let Some(index) = older {
            if let Some(Some(entry)) = self.entries.get_mut(index) {
                entry.newer = newer;
            }
        } else {
            self.oldest = newer;
        }
        if let Some(index) = newer {
            if let Some(Some(entry)) = self.entries.get_mut(index) {
                entry.older = older;
            }
        } else {
            self.newest = older;
        }
    }

    fn append(&mut self, index: usize) {
        if let Some(newest) = self.newest {
            if let Some(Some(entry)) = self.entries.get_mut(newest) {
                entry.newer = Some(index);
            }
        } else {
            self.oldest = Some(index);
        }
        if let Some(Some(entry)) = self.entries.get_mut(index) {
            entry.older = self.newest;
            entry.newer = None;
        }
        self.newest = Some(index);
    }

    fn touch(&mut self, index: usize) {
        if self.newest == Some(index) {
            return;
        }
        let Some(Some(entry)) = self.entries.get(index) else { return };
        let (older, newer) = (entry.older, entry.newer);
        self.unlink(older, newer);
        self.append(index);
    }

    fn remove(&mut self, index: usize) -> Option<Entry<K>> {
        let entry = self.entries.get_mut(index)?.take()?;
        self.unlink(entry.older, entry.newer);
        self.map.remove(&entry.key);
        self.free.push(index);
        Some(entry)
    }

    fn pop_oldest(&mut self) -> Option<Entry<K>> {
        self.remove(self.oldest?)
    }
}

pub struct FrameCache<K: Eq + Hash + Clone = (u64, i64, u16)> {
    inner: Mutex<Inner<K>>,
    budget: usize,
}

impl<K: Eq + Hash + Clone> FrameCache<K> {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner { map: HashMap::new(), entries: Vec::new(), free: Vec::new(), oldest: None, newest: None, bytes: 0, hits: 0, misses: 0 }),
            budget: budget_bytes,
        }
    }

    pub fn get(&self, key: &K) -> Option<Arc<VideoFrame>> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(&index) = inner.map.get(key)
            && let Some(Some(entry)) = inner.entries.get(index)
        {
            let frame = entry.frame.clone();
            inner.touch(index);
            inner.hits = inner.hits.saturating_add(1);
            Some(frame)
        } else {
            inner.misses = inner.misses.saturating_add(1);
            None
        }
    }

    pub fn insert(&self, key: K, frame: Arc<VideoFrame>) {
        let bytes = frame.byte_size();
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(&index) = inner.map.get(&key)
            && let Some(old) = inner.remove(index)
        {
            inner.bytes -= old.bytes;
        }
        let entry = Entry { key: key.clone(), frame, bytes, older: None, newer: None };
        let index = if let Some(slot) = inner.free.pop().and_then(|index| inner.entries.get_mut(index).map(|slot| (index, slot))) {
            let (index, slot) = slot;
            *slot = Some(entry);
            index
        } else {
            let index = inner.entries.len();
            inner.entries.push(Some(entry));
            index
        };
        inner.map.insert(key, index);
        inner.append(index);
        let mut total = inner.bytes as u128 + bytes as u128;
        if total > self.budget as u128 {
            let target = self.budget as u128 * 9 / 10;
            while total > target {
                let Some(oldest) = inner.pop_oldest() else { break };
                total -= oldest.bytes as u128;
            }
        }
        inner.bytes = total as usize;
    }

    pub fn get_or_insert_with<E>(&self, k: K, f: impl FnOnce() -> Result<Arc<VideoFrame>, E>) -> Result<Arc<VideoFrame>, E> {
        if let Some(v) = self.get(&k) {
            return Ok(v);
        }
        let v = f()?;
        self.insert(k, v.clone());
        Ok(v)
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.map.clear();
        g.entries.clear();
        g.free.clear();
        g.oldest = None;
        g.newest = None;
        g.bytes = 0;
    }

    pub fn retain(&self, mut keep: impl FnMut(&K) -> bool) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let mut next = inner.oldest;
        for _ in 0..inner.map.len() {
            let Some(index) = next else { break };
            let Some(Some(entry)) = inner.entries.get(index) else { break };
            next = entry.newer;
            if !keep(&entry.key)
                && let Some(removed) = inner.remove(index)
            {
                inner.bytes -= removed.bytes;
            }
        }
    }

    /// (entries, bytes, hits, misses)
    pub fn stats(&self) -> (usize, usize, u64, u64) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (g.map.len(), g.bytes, g.hits, g.misses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Default)]
    struct Reference {
        entries: Vec<(u8, Arc<VideoFrame>)>,
        hits: u64,
        misses: u64,
    }

    impl Reference {
        fn bytes(&self) -> usize {
            self.entries.iter().map(|(_, frame)| frame.byte_size()).sum()
        }

        fn get(&mut self, key: u8) -> Option<Arc<VideoFrame>> {
            if let Some(index) = self.entries.iter().position(|(resident, _)| *resident == key) {
                let entry = self.entries.remove(index);
                let frame = entry.1.clone();
                self.entries.push(entry);
                self.hits += 1;
                Some(frame)
            } else {
                self.misses += 1;
                None
            }
        }

        fn insert(&mut self, key: u8, frame: Arc<VideoFrame>, budget: usize) {
            self.entries.retain(|(resident, _)| *resident != key);
            self.entries.push((key, frame));
            if self.bytes() > budget {
                let target = (budget as u128 * 9 / 10) as usize;
                while self.bytes() > target {
                    self.entries.remove(0);
                }
            }
        }

        fn pop_oldest(&mut self) -> Option<(u8, Arc<VideoFrame>)> {
            if self.entries.is_empty() { None } else { Some(self.entries.remove(0)) }
        }
    }

    fn frame(pixels: u16, value: u8) -> Arc<VideoFrame> {
        Arc::new(VideoFrame::rgba8(u32::from(pixels), 1, vec![value; usize::from(pixels) * 4]))
    }

    fn assert_same_frame(actual: Option<Arc<VideoFrame>>, expected: Option<Arc<VideoFrame>>) {
        match (actual, expected) {
            (Some(actual), Some(expected)) => assert!(Arc::ptr_eq(&actual, &expected)),
            (None, None) => {}
            _ => panic!("cache and model disagree on hit/miss"),
        }
    }

    fn assert_state(cache: &FrameCache<u8>, reference: &Reference) {
        assert_eq!(cache.stats(), (reference.entries.len(), reference.bytes(), reference.hits, reference.misses));
        assert!(cache.stats().1 <= cache.budget);
        let inner = cache.inner.lock().unwrap();
        let mut next = inner.oldest;
        let mut previous = None;
        let mut visited = std::collections::HashSet::new();
        for (key, frame) in &reference.entries {
            let index = next.expect("missing linked entry");
            assert!(visited.insert(index), "cycle in recency list");
            let entry = inner.entries[index].as_ref().unwrap();
            assert_eq!(entry.key, *key);
            assert_eq!(inner.map.get(key), Some(&index));
            assert_eq!(entry.older, previous);
            assert_eq!(entry.bytes, frame.byte_size());
            assert!(Arc::ptr_eq(&entry.frame, frame));
            previous = Some(index);
            next = entry.newer;
        }
        assert_eq!(next, None);
        assert_eq!(inner.newest, previous);
        let mut older = inner.newest;
        for (key, _) in reference.entries.iter().rev() {
            let entry = inner.entries[older.unwrap()].as_ref().unwrap();
            assert_eq!(entry.key, *key);
            older = entry.older;
        }
        assert_eq!(older, None);
        for &index in &inner.free {
            assert!(visited.insert(index), "duplicate or occupied free slot");
            assert!(inner.entries[index].is_none());
        }
        assert_eq!(visited.len(), inner.entries.len());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn matches_explicit_recency_model(
            budget in 0usize..1_025,
            operations in prop::collection::vec((0u8..12, 0u8..32, 0u16..129), 0..300),
        ) {
            let cache = FrameCache::new(budget);
            let mut reference = Reference::default();
            let mut held = Vec::new();
            for (operation, key, pixels) in operations {
                match operation {
                    0..=3 => {
                        let frame = frame(pixels, key);
                        cache.insert(key, frame.clone());
                        reference.insert(key, frame, budget);
                    }
                    4..=5 => assert_same_frame(cache.get(&key), reference.get(key)),
                    6 => {
                        let actual = cache.get(&key);
                        let expected = reference.get(key);
                        assert_same_frame(actual.clone(), expected.clone());
                        if let (Some(actual), Some(expected)) = (actual, expected) {
                            held.push((actual, expected));
                        }
                    }
                    7 => { held.pop(); }
                    8 => {
                        let mut inner = cache.inner.lock().unwrap();
                        let actual = inner.pop_oldest().map(|entry| {
                            inner.bytes -= entry.bytes;
                            (entry.key, entry.frame)
                        });
                        let expected = reference.pop_oldest();
                        prop_assert_eq!(actual.as_ref().map(|entry| entry.0), expected.as_ref().map(|entry| entry.0));
                        assert_same_frame(actual.map(|entry| entry.1), expected.map(|entry| entry.1));
                    }
                    9 => {
                        cache.retain(|resident| resident % 4 != key % 4);
                        reference.entries.retain(|(resident, _)| resident % 4 != key % 4);
                    }
                    10 => {
                        cache.clear();
                        reference.entries.clear();
                    }
                    _ => {
                        let generated = frame(pixels, key);
                        let mut called = false;
                        let actual = cache.get_or_insert_with(key, || {
                            called = true;
                            if pixels % 2 == 0 { Ok(generated.clone()) } else { Err(()) }
                        });
                        let expected = reference.get(key);
                        prop_assert_eq!(called, expected.is_none());
                        let expected = if let Some(frame) = expected {
                            Ok(frame)
                        } else if pixels % 2 == 0 {
                            reference.insert(key, generated.clone(), budget);
                            Ok(generated)
                        } else {
                            Err(())
                        };
                        prop_assert_eq!(actual.is_err(), expected.is_err());
                        assert_same_frame(actual.ok(), expected.ok());
                    }
                }
                assert_state(&cache, &reference);
                for (actual, expected) in &held {
                    prop_assert!(Arc::ptr_eq(actual, expected));
                    prop_assert_eq!(actual.byte_size(), expected.byte_size());
                }
            }
        }
    }

    #[test]
    fn drains_to_ninety_percent_only_after_exceeding_budget() {
        let cache = FrameCache::new(400);
        for key in 0..100 {
            cache.insert(key, frame(1, key));
        }
        assert_eq!(cache.stats().0, 100);
        assert!(cache.get(&0).is_some());
        cache.insert(100, frame(1, 100));
        assert_eq!(cache.stats().1, 360);
        for key in 1..=11 {
            assert!(cache.get(&key).is_none());
        }
        assert!(cache.get(&0).is_some());
        assert!(cache.get(&12).is_some());
    }

    #[test]
    fn rounds_target_down_and_evicts_newest_when_necessary() {
        let cache = FrameCache::new(17);
        cache.insert(0, frame(1, 0));
        cache.insert(1, frame(4, 1));
        assert_eq!(cache.stats().0, 0);
        cache.insert(2, frame(4, 2));
        assert_eq!(cache.stats().1, 16);
        cache.insert(3, frame(5, 3));
        assert_eq!(cache.stats().1, 0);
    }

    #[test]
    fn held_arcs_are_not_pins_and_remain_usable_after_eviction() {
        let cache = FrameCache::new(16);
        let held = frame(4, 7);
        cache.insert(0, held.clone());
        let read = cache.get(&0).unwrap();
        cache.insert(1, frame(1, 1));
        assert!(cache.get(&0).is_none());
        assert_eq!(cache.stats().1, 4);
        assert!(Arc::ptr_eq(&held, &read));
        assert_eq!(read.byte_size(), 16);
    }

    #[test]
    fn replacement_updates_cost_and_recency_without_duplicate_links() {
        let cache = FrameCache::new(12);
        for key in 0..3 {
            cache.insert(key, frame(1, key));
        }
        let replacement = frame(2, 9);
        cache.insert(0, replacement.clone());
        assert_eq!(cache.stats(), (1, 8, 0, 0));
        assert_same_frame(cache.get(&0), Some(replacement));
        cache.insert(0, frame(0, 0));
        cache.insert(3, frame(3, 3));
        assert_eq!(cache.stats().1, 12);
        assert!(cache.get(&0).is_some());
    }

    #[test]
    fn zero_budget_preserves_zero_cost_entries_until_pressure() {
        let cache = FrameCache::new(0);
        cache.insert(0, frame(0, 0));
        cache.insert(1, frame(0, 1));
        assert_eq!(cache.stats().0, 2);
        cache.insert(2, frame(1, 2));
        assert_eq!(cache.stats().0, 0);
        let generated = cache.get_or_insert_with(3, || Ok::<_, ()>(frame(1, 3))).unwrap();
        assert_eq!(generated.byte_size(), 4);
        assert!(cache.get(&3).is_none());
    }

    #[test]
    fn recency_and_slots_survive_retain_and_clear() {
        let cache = FrameCache::new(16);
        let mut reference = Reference::default();
        for key in 0..4 {
            let frame = frame(1, key);
            cache.insert(key, frame.clone());
            reference.insert(key, frame, 16);
        }
        cache.retain(|key| *key == 1 || *key == 2);
        reference.entries.retain(|(key, _)| *key == 1 || *key == 2);
        assert_state(&cache, &reference);
        for key in 4..100 {
            let frame = frame(1, key);
            cache.insert(key, frame.clone());
            reference.insert(key, frame, 16);
            assert_state(&cache, &reference);
        }
        assert!(cache.inner.lock().unwrap().entries.len() <= 5);
        cache.clear();
        reference.entries.clear();
        assert_state(&cache, &reference);
        cache.insert(0, frame(1, 0));
        assert!(cache.get(&0).is_some());
    }

    #[test]
    fn maximum_budget_does_not_overflow() {
        let cache = FrameCache::new(usize::MAX);
        cache.insert(0, frame(1, 0));
        assert_eq!(cache.stats().1, 4);
    }

    #[test]
    #[ignore = "manual release-mode benchmark: 100k inserts into a 1k-entry byte budget"]
    fn bench_frame_cache_insert() {
        let frame = Arc::new(VideoFrame::rgba8(2, 2, vec![0; 16]));
        for run in 1..=5 {
            let cache: FrameCache<u32> = FrameCache::new(1_000 * frame.byte_size());
            for key in 0..1_000 {
                cache.insert(key, frame.clone());
            }
            let start = std::time::Instant::now();
            for key in 1_000..101_000 {
                cache.insert(std::hint::black_box(key), frame.clone());
            }
            println!("100k inserts, 1k-entry budget, run {run}: {:?}", start.elapsed());
            assert!(cache.stats().1 <= 16_000);
            std::hint::black_box(cache.stats());
        }
    }

    #[test]
    fn evicts_lru() {
        let c: FrameCache<u32> = FrameCache::new(4 * 4 * 3);
        for i in 0..3 {
            c.insert(i, Arc::new(VideoFrame::rgba8(2, 2, vec![0; 16])));
        }
        assert!(c.get(&0).is_some()); // touch 0
        c.insert(3, Arc::new(VideoFrame::rgba8(2, 2, vec![0; 16])));
        assert!(c.get(&0).is_some());
        assert!(c.get(&1).is_none());
        assert!(c.stats().1 <= 48);
    }
}
