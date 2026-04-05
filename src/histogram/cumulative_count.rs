use super::histogram::Histogram;
use super::interpolator::Interpolator;

/// An incremental cursor for computing cumulative counts at monotonically
/// increasing positions over a histogram's interpolation model.
///
/// Maintains internal state (current bucket index and accumulated total)
/// so that each `count_below` call resumes scanning from where the previous
/// call left off, rather than re-scanning from bucket 0.
///
/// Each `position` passed to `count_below` must be >= the previous one.
pub struct CumulativeCount<'a, T = ()> {
    interpolator: Interpolator<'a, T>,

    /// Index of the bucket containing or following the last queried position.
    bucket_index: usize,

    /// Sum of whole-bucket counts from all buckets before `bucket_index`.
    accumulated: u64,
}

impl<'a, T> CumulativeCount<'a, T> {
    pub fn new(hist: &'a Histogram<T>) -> Self {
        Self {
            interpolator: Interpolator::new(hist),
            bucket_index: 0,
            accumulated: 0,
        }
    }

    pub fn current_bucket(&self) -> usize {
        self.bucket_index
    }

    pub fn whole_bucket_accumulated(&self) -> u64 {
        self.accumulated
    }

    /// Returns the estimated count of samples in `[0, position)`.
    ///
    /// Each successive call must pass a `position` >= the previous one.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `position` would require scanning backward.
    pub fn count_below(&mut self, position: u64) -> f64 {
        let num_buckets = self.interpolator.hist.num_buckets();

        while self.bucket_index < num_buckets {
            let b = self.interpolator.hist.bucket(self.bucket_index);

            debug_assert!(
                position >= b.left(),
                "CumulativeCount::count_below requires monotonically increasing positions"
            );

            if position >= b.right() {
                self.accumulated += b.count();
                self.bucket_index += 1;
            } else {
                // Position falls within this bucket
                let partial = self.interpolator.trapezoidal_cdf(self.bucket_index, position - b.left());
                return self.accumulated as f64 + partial;
            }
        }

        self.accumulated as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hist(records: &[(u64, u64)]) -> Histogram<()> {
        let mut hist = Histogram::<()>::new();
        for &(value, count) in records {
            hist.record_n(value, count);
        }
        hist
    }

    fn make_cursor(records: &[(u64, u64)]) -> CumulativeCount<'static, ()> {
        let hist = Box::leak(Box::new(make_hist(records)));
        CumulativeCount::new(hist)
    }

    fn count_below_oneshot(records: &[(u64, u64)], position: u64) -> f64 {
        let h = make_hist(records);
        Interpolator::new(&h).count_below(position)
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
        let mut cursor = CumulativeCount::new(h);

        let c = cursor.count_below(u64::MAX);
        assert!((c - total).abs() < 1e-6);
    }
}
