/// Number of buckets per region for the tiered cumulative scan.
const REGION_SIZE: usize = 16;

/// A single slot containing bucket counts and user-defined metadata.
///
/// Maintains a tiered cache (`region_sums`) for fast rank lookups:
/// instead of scanning all buckets, `bucket_at_rank()` first scans
/// ~16 region sums, then ~16 buckets within the matching region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot<T> {
    /// Count of samples in each bucket.
    pub(crate) buckets: Vec<u64>,
    /// Cached sum for each group of `REGION_SIZE` buckets.
    region_sums: Vec<u64>,
    /// Cached total sample count across all buckets.
    pub(crate) total: u64,
    /// User-defined metadata for this slot. Only set when slot is activated via `advance()`.
    pub(crate) data: Option<T>,
}

impl<T> Slot<T> {
    /// Creates a new slot with zeroed buckets and no metadata.
    pub(crate) fn new(num_buckets: usize) -> Self {
        let num_regions = num_buckets.div_ceil(REGION_SIZE);
        Self {
            buckets: vec![0; num_buckets],
            region_sums: vec![0; num_regions],
            total: 0,
            data: None,
        }
    }

    /// Creates a slot from pre-filled buckets, computing the total and
    /// region sums from their values.
    pub(crate) fn from_buckets(buckets: Vec<u64>, data: Option<T>) -> Self {
        let total = buckets.iter().sum();
        let region_sums = buckets.chunks(REGION_SIZE).map(|chunk| chunk.iter().sum()).collect();
        Self {
            buckets,
            region_sums,
            total,
            data,
        }
    }

    /// Adds `count` samples to the bucket at `bucket_index`.
    #[inline]
    pub(crate) fn record_n(&mut self, bucket_index: usize, count: u64) {
        self.buckets[bucket_index] += count;
        self.region_sums[bucket_index / REGION_SIZE] += count;
        self.total += count;
    }

    /// Returns the sample count in the bucket at `index`.
    #[inline]
    pub(crate) fn count_at(&self, index: usize) -> u64 {
        self.buckets[index]
    }

    /// Returns the total sample count across all buckets.
    #[inline]
    pub(crate) fn total(&self) -> u64 {
        self.total
    }

    /// Subtracts another slot's bucket counts, region sums, and total
    /// from this slot.
    pub(crate) fn subtract(&mut self, other: &Slot<T>) {
        (0..self.buckets.len()).for_each(|i| {
            self.buckets[i] -= other.buckets[i];
        });
        (0..self.region_sums.len()).for_each(|r| {
            self.region_sums[r] -= other.region_sums[r];
        });
        self.total -= other.total;
    }

    /// Finds the bucket containing the given rank (1-based).
    ///
    /// Uses a two-phase scan: first over region sums to find the region,
    /// then over individual buckets within that region.
    ///
    /// Returns `(bucket_index, cumulative_before)` for the first bucket
    /// whose cumulative count reaches `rank`, where `cumulative_before`
    /// is the total count of all buckets before this one.
    ///
    /// Returns `None` if `rank` exceeds the total sample count.
    #[inline]
    pub(crate) fn bucket_at_rank(&self, rank: u64) -> Option<(usize, u64)> {
        // Phase 1: scan region sums to find the target region
        let mut cumulative = 0u64;
        let mut region_start = None;

        for (r, &sum) in self.region_sums.iter().enumerate() {
            let next = cumulative + sum;
            if next >= rank {
                region_start = Some(r * REGION_SIZE);
                break;
            }
            cumulative = next;
        }

        let region_start = region_start?;

        // Phase 2: scan buckets within the region
        let region_end = (region_start + REGION_SIZE).min(self.buckets.len());
        for i in region_start..region_end {
            cumulative += self.buckets[i];
            if cumulative >= rank {
                let cumulative_before = cumulative - self.buckets[i];
                return Some((i, cumulative_before));
            }
        }

        None
    }

    /// Resets all bucket counts to zero and removes user data.
    pub(crate) fn clear(&mut self) {
        self.buckets.fill(0);
        self.region_sums.fill(0);
        self.total = 0;
        self.data = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_buckets() {
        let slot: Slot<&str> = Slot::from_buckets(vec![1, 2, 3, 4], Some("tag"));
        assert_eq!(slot.buckets, vec![1, 2, 3, 4]);
        assert_eq!(slot.total(), 10);
        assert_eq!(slot.data, Some("tag"));

        let empty: Slot<()> = Slot::from_buckets(vec![0, 0, 0], None);
        assert_eq!(empty.total(), 0);
    }

    #[test]
    fn test_from_buckets_region_sums() {
        // 20 buckets → 2 regions of 16
        let mut buckets = vec![0u64; 20];
        buckets[0] = 5;
        buckets[15] = 3; // last of region 0
        buckets[16] = 7; // first of region 1

        let slot: Slot<()> = Slot::from_buckets(buckets, None);
        assert_eq!(slot.region_sums[0], 8); // 5 + 3
        assert_eq!(slot.region_sums[1], 7);
        assert_eq!(slot.total(), 15);
    }

    #[test]
    fn test_new() {
        let slot: Slot<()> = Slot::new(10);
        assert_eq!(slot.buckets.len(), 10);
        assert_eq!(slot.total(), 0);
        assert_eq!(slot.data, None);
        assert_eq!(slot.region_sums.len(), 1); // ceil(10/16) = 1
    }

    #[test]
    fn test_new_region_count() {
        let slot: Slot<()> = Slot::new(252);
        assert_eq!(slot.region_sums.len(), 16); // ceil(252/16) = 16
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
        assert_eq!(slot.total(), 8);
        assert_eq!(slot.region_sums[0], 8);
    }

    #[test]
    fn test_record_n_across_regions() {
        let mut slot: Slot<()> = Slot::new(32);
        slot.record_n(0, 10); // region 0
        slot.record_n(15, 5); // region 0
        slot.record_n(16, 3); // region 1

        assert_eq!(slot.region_sums[0], 15);
        assert_eq!(slot.region_sums[1], 3);
        assert_eq!(slot.total(), 18);
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
        a.record_n(0, 10);
        a.record_n(1, 20);
        a.record_n(2, 30);
        a.record_n(3, 40);
        assert_eq!(a.total(), 100);

        let mut b: Slot<()> = Slot::new(4);
        b.record_n(0, 1);
        b.record_n(1, 2);
        b.record_n(2, 3);
        b.record_n(3, 4);

        a.subtract(&b);
        assert_eq!(a.buckets, vec![9, 18, 27, 36]);
        assert_eq!(a.total(), 90);
        assert_eq!(a.region_sums[0], 90);
    }

    #[test]
    fn test_bucket_at_rank() {
        // buckets: [3, 0, 5, 2]  total=10
        let mut slot: Slot<()> = Slot::new(4);
        slot.record_n(0, 3);
        slot.record_n(2, 5);
        slot.record_n(3, 2);

        // Returns (bucket_index, cumulative_before_bucket)

        // Ranks 1-3 fall in bucket 0 (count=3), cumulative_before=0
        assert_eq!(slot.bucket_at_rank(1), Some((0, 0)));
        assert_eq!(slot.bucket_at_rank(3), Some((0, 0)));

        // Bucket 1 is empty (count=0), skipped.
        // Ranks 4-8 fall in bucket 2 (count=5), cumulative_before=3
        assert_eq!(slot.bucket_at_rank(4), Some((2, 3)));
        assert_eq!(slot.bucket_at_rank(8), Some((2, 3)));

        // Ranks 9-10 fall in bucket 3 (count=2), cumulative_before=8
        assert_eq!(slot.bucket_at_rank(9), Some((3, 8)));
        assert_eq!(slot.bucket_at_rank(10), Some((3, 8)));

        // Rank beyond total
        assert_eq!(slot.bucket_at_rank(11), None);

        // Rank 0 — cumulative_before=0
        assert_eq!(slot.bucket_at_rank(0), Some((0, 0)));

        // Empty slot
        let empty: Slot<()> = Slot::new(4);
        assert_eq!(empty.bucket_at_rank(1), None);
    }

    #[test]
    fn test_bucket_at_rank_across_regions() {
        // 32 buckets, 2 regions of 16
        let mut slot: Slot<()> = Slot::new(32);
        slot.record_n(0, 10); // region 0
        slot.record_n(15, 5); // region 0
        slot.record_n(16, 3); // region 1
        slot.record_n(31, 7); // region 1

        // Ranks 1-10 in bucket 0 (region 0)
        assert_eq!(slot.bucket_at_rank(1), Some((0, 0)));
        assert_eq!(slot.bucket_at_rank(10), Some((0, 0)));

        // Ranks 11-15 in bucket 15 (still region 0)
        assert_eq!(slot.bucket_at_rank(11), Some((15, 10)));
        assert_eq!(slot.bucket_at_rank(15), Some((15, 10)));

        // Ranks 16-18 in bucket 16 (region 1)
        assert_eq!(slot.bucket_at_rank(16), Some((16, 15)));
        assert_eq!(slot.bucket_at_rank(18), Some((16, 15)));

        // Ranks 19-25 in bucket 31 (region 1)
        assert_eq!(slot.bucket_at_rank(19), Some((31, 18)));
        assert_eq!(slot.bucket_at_rank(25), Some((31, 18)));
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
        assert!(slot.region_sums.iter().all(|&c| c == 0));
        assert_eq!(slot.data, None);
    }
}
