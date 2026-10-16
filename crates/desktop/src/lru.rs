//! A bounded cache that forgets what was used longest ago, a quarter at a
//! time, instead of everything at once: a long chat scrolled back and forth
//! keeps what's on screen parsed.

use std::collections::HashMap;
use std::hash::Hash;

pub struct Lru<K, V> {
    map: HashMap<K, (V, u64)>,
    tick: u64,
    cap: usize,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Lru {
            map: HashMap::new(),
            tick: 0,
            cap: cap.max(4),
        }
    }

    pub fn get(&mut self, k: &K) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(k).map(|(v, t)| {
            *t = tick;
            v.clone()
        })
    }

    pub fn insert(&mut self, k: K, v: V) {
        self.tick += 1;
        self.map.insert(k, (v, self.tick));
        if self.map.len() > self.cap {
            // The oldest quarter goes.
            let mut ticks: Vec<u64> = self.map.values().map(|(_, t)| *t).collect();
            ticks.sort_unstable();
            let cut = ticks[ticks.len() / 4];
            self.map.retain(|_, (_, t)| *t > cut);
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_what_was_used_lately() {
        let mut c = super::Lru::new(8);
        for i in 0..8 {
            c.insert(i, i);
        }
        // 0 is used again, so it outlives 1 and 2.
        assert_eq!(c.get(&0), Some(0));
        c.insert(8, 8);
        assert!(c.len() <= 8 && c.len() >= 6);
        assert_eq!(c.get(&0), Some(0));
        assert_eq!(c.get(&1), None);
        assert_eq!(c.get(&8), Some(8));
    }
}
