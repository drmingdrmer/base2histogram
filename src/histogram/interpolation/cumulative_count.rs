use super::interpolator::Interpolator;
use crate::histogram::scale::LogScale;

/// An incremental cursor for computing cumulative counts at monotonically
/// increasing positions over a histogram's interpolation model.
///
/// Maintains internal state (current bucket index and accumulated total)
/// so that each `count_below` call resumes scanning from where the previous
/// call left off, rather than re-scanning from bucket 0.
///
/// Each `position` passed to `count_below` must be >= the previous one.
pub struct CumulativeCount<'a> {
    interpolator: Interpolator<'a>,

    /// Index of the bucket containing or following the last queried position.
    bucket_index: usize,

    /// Sum of whole-bucket counts from all buckets before `bucket_index`.
    accumulated: u64,
}

impl<'a> CumulativeCount<'a> {
    /// Creates a new cursor starting at position 0.
    pub fn new(log_scale: &'a LogScale, buckets: &'a [u64]) -> Self {
        Self {
            interpolator: Interpolator::new(log_scale, buckets),
            bucket_index: 0,
            accumulated: 0,
        }
    }

    /// Returns the bucket index at or after the last queried position.
    pub fn current_bucket(&self) -> usize {
        self.bucket_index
    }

    /// Returns the sum of all whole-bucket counts before `current_bucket()`.
    pub fn whole_bucket_accumulated(&self) -> u64 {
        self.accumulated
    }

    /// Returns the estimated count of samples in `[0, position)`.
    ///
    /// Each successive call must pass a `position` >= the previous one.
    /// For the terminal bucket, `position == u64::MAX` still excludes the
    /// endpoint mass at `u64::MAX`.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `position` would require scanning backward.
    pub fn count_below(&mut self, position: u64) -> f64 {
        let num_buckets = self.interpolator.num_buckets();

        while self.bucket_index < num_buckets {
            let b = self.interpolator.bucket(self.bucket_index);

            debug_assert!(
                position >= b.left(),
                "CumulativeCount::count_below requires monotonically increasing positions"
            );

            let past_bucket = if b.is_last() {
                position > b.right()
            } else {
                position >= b.right()
            };

            if past_bucket {
                self.accumulated += b.count();
                self.bucket_index += 1;
            } else {
                // Position falls within this bucket
                let partial = self.interpolator.trapezoidal_cdf(self.bucket_index, position.saturating_sub(b.left()));
                return self.accumulated as f64 + partial;
            }
        }

        self.accumulated as f64
    }

    /// Returns the interpolated value at the given rank (1-based position in
    /// sorted order).
    ///
    /// Each successive call must pass a `rank` >= the previous one.
    /// Returns `0` if `rank` exceeds the total sample count.
    pub fn value_at_rank(&mut self, rank: u64) -> u64 {
        let num_buckets = self.interpolator.num_buckets();

        while self.bucket_index < num_buckets {
            let count = self.interpolator.bucket(self.bucket_index).count();
            let next = self.accumulated + count;

            if next >= rank {
                let rank_in_bucket = rank - self.accumulated;
                return self.interpolator.rank_to_position(self.bucket_index, rank_in_bucket);
            }

            self.accumulated = next;
            self.bucket_index += 1;
        }

        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::histogram::Histogram;

    fn make_hist(records: &[(u64, u64)]) -> Histogram<()> {
        let mut hist = Histogram::<()>::new();
        for &(value, count) in records {
            hist.record_n(value, count);
        }
        hist
    }

    fn make_cursor(records: &[(u64, u64)]) -> CumulativeCount<'static> {
        let hist = Box::leak(Box::new(make_hist(records)));
        hist.cumulative_count()
    }

    fn count_below_oneshot(records: &[(u64, u64)], position: u64) -> f64 {
        let h = make_hist(records);
        h.interpolator().count_below(position)
    }

    #[test]
    fn test_cursor_equivalence_with_count_below() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        for pos in [0, 5, 8, 9, 10, 11, 12, 13, 14, 100] {
            let from_cursor = cursor.count_below(pos);
            let from_density = count_below_oneshot(r, pos);
            assert!(
                (from_cursor - from_density).abs() < 1e-10,
                "pos={pos}: cursor={from_cursor}, density={from_density}"
            );
        }
    }

    #[test]
    fn test_cursor_same_bucket_calls() {
        // Two positions within bucket 9: [10,12) count=20
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        let c1 = cursor.count_below(10);
        let expected1 = count_below_oneshot(r, 10);
        assert!((c1 - expected1).abs() < 1e-10);

        let c2 = cursor.count_below(11);
        let expected2 = count_below_oneshot(r, 11);
        assert!((c2 - expected2).abs() < 1e-10);

        assert!(c2 >= c1);
    }

    #[test]
    fn test_cursor_bucket_boundary() {
        let r = &[(8, 20), (10, 50)];
        let mut cursor = make_cursor(r);

        // Position 10 = right of bucket 8 → all of bucket 8, none of bucket 9
        let c = cursor.count_below(10);
        assert!((c - 20.0).abs() < 1e-10);

        // Position 12 = right of bucket 9 → all of both
        let c = cursor.count_below(12);
        assert!((c - 70.0).abs() < 1e-10);
    }

    #[test]
    fn test_cursor_past_all_buckets() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        let c = cursor.count_below(u64::MAX);
        assert!((c - 60.0).abs() < 1e-10);
    }

    #[test]
    fn test_cursor_empty_histogram() {
        let mut cursor = make_cursor(&[]);

        let c = cursor.count_below(100);
        assert!(c.abs() < 1e-10);
    }

    #[test]
    fn test_cursor_monotonicity() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        let mut prev = 0.0;
        for pos in [0, 5, 8, 9, 10, 11, 12, 13, 14, 100] {
            let c = cursor.count_below(pos);
            assert!(c >= prev, "pos={pos}: {c} < prev={prev}");
            prev = c;
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "monotonically increasing")]
    fn test_cursor_panics_on_backward() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        cursor.count_below(11);
        cursor.count_below(9); // should panic
    }

    #[test]
    fn test_cursor_position_zero() {
        // Position 0 = left of bucket 0 [0,1) → 0
        let mut cursor = make_cursor(&[(0, 50), (5, 30)]);

        let c = cursor.count_below(0);
        assert!(c.abs() < 1e-10);
    }

    #[test]
    fn test_cursor_same_position_twice() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        let c1 = cursor.count_below(11);
        let c2 = cursor.count_below(11);
        assert!((c1 - c2).abs() < 1e-10);
    }

    #[test]
    fn test_cursor_gap_between_buckets() {
        // Bucket 5: [5,6) c=10, Bucket 12: [16,20) c=40
        // Position 10 is in an empty bucket between them
        let mut cursor = make_cursor(&[(5, 10), (16, 40)]);

        let c = cursor.count_below(10);
        assert!((c - 10.0).abs() < 1e-10);

        // Then jump into the far bucket
        let c = cursor.count_below(18);
        assert!(c > 10.0);
        assert!(c < 50.0);
    }

    #[test]
    fn test_cursor_first_bucket() {
        // Data only in bucket 0: [0,1) and bucket 1: [1,2)
        let mut cursor = make_cursor(&[(0, 10), (1, 20)]);

        let c = cursor.count_below(0);
        assert!(c.abs() < 1e-10);

        let c = cursor.count_below(1);
        assert!((c - 10.0).abs() < 1e-10);

        let c = cursor.count_below(2);
        assert!((c - 30.0).abs() < 1e-10);
    }

    #[test]
    fn test_cursor_last_bucket() {
        let mut cursor = make_cursor(&[(0b111 << 61, 100)]);

        // Before last bucket
        let c = cursor.count_below(0b110 << 61);
        assert!(c.abs() < 1e-10);

        // Inside last bucket
        let c = cursor.count_below(u64::MAX - 1);
        assert!(c > 0.0);
        assert!(c <= 100.0);
    }

    #[test]
    fn test_cursor_u64_max_excludes_terminal_endpoint_mass() {
        let mut hist = Histogram::<()>::with_log_scale(16, 1);
        hist.record_n(u64::MAX, 100);
        let hist = Box::leak(Box::new(hist));

        let mut cursor = hist.cumulative_count();
        let c = cursor.count_below(u64::MAX);
        assert!(c > 0.0);
        assert!(c < 100.0, "count_below(u64::MAX) should exclude the endpoint");
    }

    #[test]
    fn test_cursor_u64_max_after_earlier_calls() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut cursor = make_cursor(r);

        // Advance cursor partway
        let c = cursor.count_below(11);
        assert!(c > 0.0);

        // Then jump to u64::MAX → should equal total
        let c = cursor.count_below(u64::MAX);
        assert!((c - 60.0).abs() < 1e-10);
    }

    #[test]
    fn test_cursor_single_call_past_all() {
        // One call from fresh cursor to past all data
        let r = &[(0, 5), (5, 10), (10, 20), (100, 50)];
        let h = Box::leak(Box::new(make_hist(r)));
        let total = h.total() as f64;
        let mut cursor = h.cumulative_count();

        let c = cursor.count_below(u64::MAX);
        assert!((c - total).abs() < 1e-6);
    }

    // === value_at_rank tests ===

    #[test]
    fn test_value_at_rank_empty() {
        let mut cursor = make_cursor(&[]);
        assert_eq!(cursor.value_at_rank(1), 0);
    }

    #[test]
    fn test_value_at_rank_single_value() {
        // Bucket 9: [10,12) midpoint=11, count=1
        let mut cursor = make_cursor(&[(10, 1)]);
        assert_eq!(cursor.value_at_rank(1), 11);
    }

    #[test]
    fn test_value_at_rank_boundary_and_interior() {
        // Bucket 8:[8,10) c=10, Bucket 9:[10,12) c=20, Bucket 10:[12,14) c=30
        let mut cursor = make_cursor(&[(8, 10), (10, 20), (12, 30)]);

        // First and last rank in bucket 8
        assert_eq!(cursor.value_at_rank(1), 8);
        assert_eq!(cursor.value_at_rank(10), 10);

        // First, middle, last rank in bucket 9
        assert_eq!(cursor.value_at_rank(11), 10);
        assert_eq!(cursor.value_at_rank(19), 11);
        assert_eq!(cursor.value_at_rank(30), 12);

        // First, middle, last rank in bucket 10
        assert_eq!(cursor.value_at_rank(31), 12);
        assert_eq!(cursor.value_at_rank(47), 13);
        assert_eq!(cursor.value_at_rank(60), 14);
    }

    #[test]
    fn test_value_at_rank_past_total() {
        // Bucket 8:[8,10) c=10, Bucket 9:[10,12) c=20, total=30
        let mut cursor = make_cursor(&[(8, 10), (10, 20)]);

        assert_eq!(cursor.value_at_rank(1), 8);
        assert_eq!(cursor.value_at_rank(15), 10);
        assert_eq!(cursor.value_at_rank(30), 12);
        // Past total → 0
        assert_eq!(cursor.value_at_rank(31), 0);
        assert_eq!(cursor.value_at_rank(100), 0);
    }

    #[test]
    fn test_value_at_rank_same_rank_twice() {
        let mut cursor = make_cursor(&[(8, 10), (10, 20), (12, 30)]);

        assert_eq!(cursor.value_at_rank(15), 10);
        assert_eq!(cursor.value_at_rank(15), 10);
    }

    #[test]
    fn test_value_at_rank_gap_between_buckets() {
        // Bucket 5:[5,6) c=10, Bucket 12:[16,20) c=40
        let mut cursor = make_cursor(&[(5, 10), (16, 40)]);

        // Last rank in first bucket
        assert_eq!(cursor.value_at_rank(10), 5);
        // First rank in second bucket (after empty gap)
        assert_eq!(cursor.value_at_rank(11), 16);
        // Middle and last of second bucket
        assert_eq!(cursor.value_at_rank(25), 17);
        assert_eq!(cursor.value_at_rank(50), 20);
    }
}
