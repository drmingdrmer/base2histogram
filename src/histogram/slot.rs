/// A single slot containing bucket counts and user-defined metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot<T> {
    /// Count of samples in each bucket.
    pub(crate) buckets: Vec<u64>,
    /// User-defined metadata for this slot. Only set when slot is activated via `advance()`.
    pub(crate) data: Option<T>,
}

impl<T> Slot<T> {
    /// Creates a new slot with zeroed buckets and no metadata.
    pub(crate) fn new(num_buckets: usize) -> Self {
        Self {
            buckets: vec![0; num_buckets],
            data: None,
        }
    }

    /// Adds `count` samples to the bucket at `bucket_index`.
    #[inline]
    pub(crate) fn record_n(&mut self, bucket_index: usize, count: u64) {
        self.buckets[bucket_index] += count;
    }

    /// Returns the sample count in the bucket at `index`.
    #[inline]
    pub(crate) fn count_at(&self, index: usize) -> u64 {
        self.buckets[index]
    }

    /// Returns the total sample count across all buckets.
    pub(crate) fn total(&self) -> u64 {
        self.buckets.iter().sum()
    }

    /// Subtracts another slot's bucket counts from this slot, bucket by bucket.
    pub(crate) fn subtract(&mut self, other: &Slot<T>) {
        (0..self.buckets.len()).for_each(|i| {
            self.buckets[i] -= other.buckets[i];
        });
    }

    /// Resets all bucket counts to zero and removes user data.
    pub(crate) fn clear(&mut self) {
        self.buckets.fill(0);
        self.data = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let slot: Slot<()> = Slot::new(10);
        assert_eq!(slot.buckets.len(), 10);
        assert_eq!(slot.total(), 0);
        assert_eq!(slot.data, None);
    }

    #[test]
    fn test_record_n_and_count_at() {
        let mut slot: Slot<()> = Slot::new(10);
        slot.record_n(3, 5);
        slot.record_n(3, 2);
        slot.record_n(7, 1);

        assert_eq!(slot.count_at(3), 7);
        assert_eq!(slot.count_at(7), 1);
        assert_eq!(slot.count_at(0), 0);
    }

    #[test]
    fn test_total() {
        let mut slot: Slot<()> = Slot::new(10);
        slot.record_n(0, 3);
        slot.record_n(5, 7);
        assert_eq!(slot.total(), 10);
    }

    #[test]
    fn test_subtract() {
        let mut a: Slot<()> = Slot::new(4);
        a.buckets = vec![10, 20, 30, 40];

        let b: Slot<()> = Slot {
            buckets: vec![1, 2, 3, 4],
            data: None,
        };

        a.subtract(&b);
        assert_eq!(a.buckets, vec![9, 18, 27, 36]);
    }

    #[test]
    fn test_clear() {
        let mut slot: Slot<String> = Slot::new(10);
        slot.record_n(0, 5);
        slot.record_n(5, 10);
        slot.data = Some("test".to_string());

        slot.clear();

        assert_eq!(slot.total(), 0);
        assert!(slot.buckets.iter().all(|&c| c == 0));
        assert_eq!(slot.data, None);
    }
}
