//! The most recent pulses' figures, bounded: a long run (a gunfire burst of
//! thousands of rounds) keeps every row but only the latest figures.

use std::collections::VecDeque;

/// How many pulses' figures a run keeps.
pub const DEFAULT_CAPACITY: usize = 32;

/// Items by pulse index, the oldest dropped once `capacity` are held.
#[derive(Debug, Clone)]
pub struct PulseRing<T> {
    capacity: usize,
    items: VecDeque<(u64, T)>,
    dropped: u64,
}

impl<T> PulseRing<T> {
    pub fn new(capacity: usize) -> PulseRing<T> {
        PulseRing {
            capacity: capacity.max(1),
            items: VecDeque::new(),
            dropped: 0,
        }
    }

    /// Hold `item` for pulse `index`, dropping the oldest when full.
    pub fn push(&mut self, index: u64, item: T) {
        if self.items.len() == self.capacity {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back((index, item));
    }

    pub fn get(&self, index: u64) -> Option<&T> {
        self.items.iter().find(|(i, _)| *i == index).map(|(_, t)| t)
    }

    /// The most recent pulse's item.
    pub fn last(&self) -> Option<&T> {
        self.items.back().map(|(_, t)| t)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Items dropped to keep the bound.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<T> Default for PulseRing<T> {
    fn default() -> Self {
        PulseRing::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_latest_and_counts_what_it_dropped() {
        let mut r = PulseRing::new(3);
        for i in 0..5u64 {
            r.push(i, i * 10);
        }
        assert_eq!(r.len(), 3);
        assert_eq!(r.get(0), None);
        assert_eq!(r.get(2), Some(&20));
        assert_eq!(r.last(), Some(&40));
        assert_eq!(r.dropped(), 2);
    }
}
