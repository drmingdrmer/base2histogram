use std::collections::VecDeque;

use super::slot::Slot;

/// A container for historical slots.
///
/// Stores up to `slot_limit - 1` historical slots. The current period's
/// bucket counts and metadata are stored in `Histogram::aggregate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlotQueue<T> {
    pub(crate) slot_limit: usize,
    pub(crate) slots: VecDeque<Slot<T>>,
}

impl<T> SlotQueue<T> {
    pub(crate) fn new(slot_limit: usize) -> Self {
        Self {
            slot_limit,
            slots: VecDeque::with_capacity(slot_limit.saturating_sub(1)),
        }
    }

    pub(crate) fn pop_front(&mut self) -> Option<Slot<T>> {
        self.slots.pop_front()
    }

    pub(crate) fn push_back(&mut self, slot: Slot<T>) {
        self.slots.push_back(slot);
    }

    pub(crate) fn iter_all(&self) -> impl Iterator<Item = &Slot<T>> {
        self.slots.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_limit_is_stable_when_vecdeque_reserves_more() {
        let mut q: SlotQueue<()> = SlotQueue::new(3);

        assert_eq!(q.slot_limit, 3);

        q.slots.reserve(16);

        assert!(q.slots.capacity() > q.slot_limit);
        assert_eq!(q.slot_limit, 3);
    }
}
