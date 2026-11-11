//! A short memory of how memory was used: the last few minutes of [`MemoryLists`] readings, for
//! the chart on the Memory page.
//!
//! The history is bounded ([`MemoryHistory::CAPACITY`] readings), so it never grows; the caller
//! decides when readings are taken and when the history is dropped. Nothing here reads the
//! system.

use std::collections::VecDeque;

use crate::ports::MemoryLists;
use crate::time::Timestamp;

/// One reading of memory by list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryPoint {
    /// When it was taken.
    pub taken_at: Timestamp,
    /// Memory by list at that time.
    pub lists: MemoryLists,
}

/// The most recent readings, oldest first.
#[derive(Debug, Clone, Default)]
pub struct MemoryHistory {
    points: VecDeque<MemoryPoint>,
}

impl MemoryHistory {
    /// How many readings are kept: five minutes at one reading a second.
    pub const CAPACITY: usize = 300;

    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a reading and drops the oldest one when the history is full. A reading that is not
    /// newer than the last one (the clock stepped back) replaces nothing and is ignored, so the
    /// history stays in time order.
    pub fn push(&mut self, point: MemoryPoint) {
        if self
            .points
            .back()
            .is_some_and(|last| point.taken_at <= last.taken_at)
        {
            return;
        }
        if self.points.len() == Self::CAPACITY {
            self.points.pop_front();
        }
        self.points.push_back(point);
    }

    /// The readings, oldest first.
    #[must_use]
    pub fn points(&self) -> Vec<MemoryPoint> {
        self.points.iter().copied().collect()
    }

    /// Forgets every reading.
    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// How many readings are kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether there are no readings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(millis: i64) -> MemoryPoint {
        MemoryPoint {
            taken_at: Timestamp::from_unix_millis(millis),
            lists: MemoryLists::from_lists(1000, 100, 10, 200),
        }
    }

    #[test]
    fn readings_are_kept_oldest_first() {
        let mut history = MemoryHistory::new();
        history.push(point(1));
        history.push(point(2));
        let taken: Vec<_> = history
            .points()
            .iter()
            .map(|p| p.taken_at.as_unix_millis())
            .collect();
        assert_eq!(taken, [1, 2]);
    }

    #[test]
    fn the_history_is_bounded_and_drops_the_oldest() {
        let mut history = MemoryHistory::new();
        let total = i64::try_from(MemoryHistory::CAPACITY).expect("fits") + 5;
        for millis in 1..=total {
            history.push(point(millis));
        }
        assert_eq!(history.len(), MemoryHistory::CAPACITY);
        assert_eq!(history.points()[0].taken_at.as_unix_millis(), 6);
    }

    #[test]
    fn a_reading_that_is_not_newer_is_ignored() {
        let mut history = MemoryHistory::new();
        history.push(point(10));
        history.push(point(10));
        history.push(point(5));
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut history = MemoryHistory::new();
        history.push(point(1));
        history.clear();
        assert!(history.is_empty());
    }
}
