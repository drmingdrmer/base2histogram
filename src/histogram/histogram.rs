use super::bucket_ref::BucketRef;
use super::cumulative_count::CumulativeCount;
use super::interpolator::Interpolator;
use super::log_scale::LogScale;
use super::percentile_stats::PercentileStats;
use super::slot::Slot;
use super::slot_queue::SlotQueue;
use crate::histogram::display_buckets::DisplayBuckets;

/// A histogram for tracking the distribution of u64 values using logarithmic bucketing.
///
/// This histogram provides O(1) recording and efficient percentile calculation with
/// bounded memory usage (252 buckets = ~2KB per slot), regardless of the number of samples.
///
/// # Multi-Slot Support
///
/// The histogram supports multiple slots for sliding-window metrics. Each slot
/// contains independent bucket counts and optional user-defined metadata. Use
/// [`advance()`](Histogram::advance) to rotate to a new slot, which evicts the
/// oldest data when the histogram is full.
///
/// ## Slot Storage Layout
///
/// With `slot_limit = N`, only `N - 1` slots are physically stored. The
/// current (newest) period's counts are never materialized — they are derived
/// on the fly as `aggregate - Σ stored`:
///
/// ```text
/// stored slots:  [a, b, c]        (N - 1 historical periods)
/// aggregate:     agg = a + b + c + d   (running total across all N periods)
/// implicit:      d = agg - a - b - c   (current period, not stored)
/// ```
///
/// `record()` only touches `aggregate`; individual slot writes are avoided.
///
/// On [`advance()`](Histogram::advance), the current period `d` is
/// materialized into the evicted slot's allocation and the oldest period is
/// dropped:
///
/// ```text
/// 1. agg  ← agg - a              (drop oldest)
/// 2. a    ← agg - b - c  (= d)   (reuse evicted Vec for materialized current)
/// 3. slots: [b, c, a']            (a' now holds d's counts)
/// ```
///
/// # Bucketing Strategy
///
/// Uses logarithmic bucketing where smaller values get higher precision, similar to
/// [HDRHistogram](https://github.com/HdrHistogram/HdrHistogram). The bucket boundaries
/// are determined by the binary representation of the value:
///
/// ```text
/// Group  Bucket   Value Range     Binary Pattern (3-bit window)
/// ─────  ──────   ───────────     ─────────────────────────────
///   0      0-3    [0-3]           Direct mapping (special case)
///   1      4-7    [4-7]           100, 101, 110, 111
///   2     8-11    [8-15]          1xx0, 1xx0 (step=2)
///   3    12-15    [16-31]         1xx00, 1xx00 (step=4)
///   4    16-19    [32-63]         1xx000, 1xx000 (step=8)
///   ...
/// ```
///
/// Each group covers a power-of-2 range and contains 4 buckets. The 2 bits after the
/// MSB determine which bucket within the group:
///
/// ```text
/// Example: value = 42 (binary: 101010)
///   MSB position: 5 (counting from 0)
///   Group: 5 - 2 = 3
///   Bits after MSB: 01 (from 1[01]010)
///   Bucket within group: 1
///   Final bucket index: 4 + (3 * 4) + 1 = 17
/// ```
///
/// # Bucket Resolution
///
/// - Values 0-7: exact (1:1 mapping)
/// - Values 8-15: 2 values per bucket
/// - Values 16-31: 4 values per bucket
/// - Values 2^n to 2^(n+1)-1: 2^(n-2) values per bucket
///
/// Percentile accuracy depends on the distribution and sample count.
/// For typical real-world distributions (log-normal, exponential), percentile
/// error is under 2%. See `cargo run --bin accuracy` for detailed benchmarks.
///
/// # Memory Usage
///
/// Fixed at 252 buckets * 8 bytes = 2,016 bytes per slot, covering the entire
/// u64 range [0, 2^64-1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram<T = ()> {
    /// Log scale for value-to-bucket mapping.
    log_scale: &'static LogScale,

    /// Historical slots (up to `slot_limit - 1`).
    /// The current period is implicit: its buckets are
    /// `aggregate_buckets - Σ stored slots`.
    slots: SlotQueue<T>,

    /// Aggregate bucket counts across all periods.
    aggregate_buckets: Vec<u64>,
}

impl<T> Default for Histogram<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Histogram<T> {
    /// Creates a new histogram with 1 slot and 252 buckets.
    ///
    /// Memory usage: 252 * 8 bytes = 2,016 bytes per histogram.
    pub fn new() -> Self {
        Self::with_slots(1)
    }

    /// Creates a new histogram with the specified slot limit.
    ///
    /// When `slot_limit` is 0 or 1, no individual slots are maintained —
    /// the aggregate is the sole source of truth.
    pub fn with_slots(slot_limit: usize) -> Self {
        Self::with_log_scale(LogScale::DEFAULT_WIDTH, slot_limit)
    }

    /// Creates a new histogram with the specified bucket width and slot limit.
    ///
    /// When `slot_limit` is 0 or 1, no individual slots are maintained —
    /// the aggregate is the sole source of truth.
    pub fn with_log_scale(width: usize, slot_limit: usize) -> Self {
        let log_scale = LogScale::get(width);
        let num_buckets = log_scale.num_buckets();

        Self {
            log_scale,
            slots: SlotQueue::new(slot_limit),
            aggregate_buckets: vec![0; num_buckets],
        }
    }

    /// Records a value to the current (last) slot.
    pub fn record(&mut self, value: u64) {
        self.record_n(value, 1);
    }

    /// Record a value `count` times.
    pub fn record_n(&mut self, value: u64, count: u64) {
        let bucket_index = self.log_scale.calculate_bucket(value);
        self.aggregate_buckets[bucket_index] += count;
    }

    /// Advances to a new slot, evicting the oldest if the slot limit is reached.
    ///
    /// Returns the number of active slots after advancing.
    ///
    /// Materializes the implicit current period into a stored historical slot,
    /// then starts a new empty current period with the given `data`.
    /// At capacity the evicted slot's allocation is reused; during warmup a new
    /// slot is allocated.
    ///
    /// See [Slot Storage Layout](Histogram#slot-storage-layout) for the
    /// rotation algorithm.
    pub fn advance(&mut self, data: T) -> usize {
        if self.slots.slot_limit <= 1 {
            self.aggregate_buckets.fill(0);
            return 0;
        }

        let num_buckets = self.log_scale.num_buckets();

        // Get a Vec for the materialized current period.
        // At capacity: evict oldest and reuse its allocation.
        // Warmup: allocate a new Vec.
        let mut buckets = if self.slots.slots.len() == self.slots.slot_limit - 1 {
            let evicted = self.slots.pop_front().unwrap();
            (0..num_buckets).for_each(|i| {
                self.aggregate_buckets[i] -= evicted.buckets[i];
            });
            evicted.buckets
        } else {
            vec![0; num_buckets]
        };

        // Materialize current: aggregate - Σ stored
        buckets.copy_from_slice(&self.aggregate_buckets);
        for slot in self.slots.iter_all() {
            (0..num_buckets).for_each(|i| {
                buckets[i] -= slot.buckets[i];
            });
        }

        let current_data = self.slots.current_data.take();
        self.slots.push_back(Slot {
            buckets,
            data: current_data,
        });

        self.slots.current_data = Some(data);
        self.slots.slots.len() + 1
    }

    /// Returns the number of active slots (stored historicals + implicit current).
    #[allow(dead_code)]
    #[inline]
    pub fn active_slot_count(&self) -> usize {
        if self.slots.slot_limit <= 1 {
            0
        } else {
            self.slots.slots.len() + 1
        }
    }

    /// Returns the maximum number of active slots.
    #[allow(dead_code)]
    #[inline]
    pub fn slot_limit(&self) -> usize {
        self.slots.slot_limit
    }

    /// Returns the total number of values recorded across all slots.
    pub fn total(&self) -> u64 {
        self.aggregate_buckets.iter().sum()
    }

    /// Calculates the value at the given percentile.
    ///
    /// Returns an interpolated estimate within the bucket containing the percentile.
    /// Uses neighboring bucket densities for trapezoidal interpolation, falling back
    /// to uniform interpolation at histogram edges or when neighbors are empty.
    ///
    /// Returns `0` if the histogram is empty.
    #[allow(dead_code)]
    pub fn percentile(&self, p: f64) -> u64 {
        let total = self.total();
        self.percentile_with_total(p, total)
    }

    /// Calculates the percentile given a specific total count.
    ///
    /// This is used internally when calculating multiple percentiles to avoid
    /// recalculating the total multiple times.
    fn percentile_with_total(&self, p: f64, total: u64) -> u64 {
        if total == 0 {
            return 0;
        }

        let rank = (total as f64 * p).ceil().max(1.0) as u64;
        self.value_at_rank(rank)
    }

    /// Returns the interpolated value at the given rank (1-based position in
    /// sorted order).
    fn value_at_rank(&self, rank: u64) -> u64 {
        let interp = self.interpolator();
        let mut cumulative = 0u64;

        for (i, &count) in self.aggregate_buckets.iter().enumerate() {
            cumulative += count;
            if cumulative >= rank {
                let rank_in_bucket = rank - (cumulative - count);
                return interp.rank_to_position(i, rank_in_bucket);
            }
        }

        0
    }

    /// Returns the estimated count of samples in `[0, position)`,
    /// i.e., strictly below `position`, using trapezoidal
    /// interpolation within buckets.
    pub fn count_below(&self, position: u64) -> u64 {
        self.interpolator().count_below(position) as u64
    }

    /// Returns an [`Interpolator`] over the aggregate bucket counts.
    pub fn interpolator(&self) -> Interpolator<'_> {
        Interpolator::new(self.log_scale, &self.aggregate_buckets)
    }

    /// Returns common percentile statistics: samples, P0.1, P1, P5, P10, P50, P90, P99, P99.9.
    pub fn percentile_stats(&self) -> PercentileStats {
        let samples = self.total();
        PercentileStats {
            samples,
            p0_1: self.percentile_with_total(0.001, samples),
            p1: self.percentile_with_total(0.01, samples),
            p5: self.percentile_with_total(0.05, samples),
            p10: self.percentile_with_total(0.10, samples),
            p50: self.percentile_with_total(0.50, samples),
            p90: self.percentile_with_total(0.90, samples),
            p99: self.percentile_with_total(0.99, samples),
            p99_9: self.percentile_with_total(0.999, samples),
        }
    }

    /// Returns an iterator over all bucket data.
    ///
    /// Includes all buckets (even those with count 0). Callers can filter
    /// non-empty buckets with `.filter(|b| b.count > 0)`.
    pub fn bucket_data(&self) -> impl Iterator<Item = BucketRef<'_>> + '_ {
        (0..self.num_buckets()).map(|i| self.bucket(i))
    }

    /// Returns the number of buckets.
    pub fn num_buckets(&self) -> usize {
        self.log_scale.num_buckets()
    }

    /// Returns an incremental cursor for computing cumulative counts
    /// at monotonically increasing positions.
    pub fn cumulative_count(&self) -> CumulativeCount<'_> {
        CumulativeCount::new(self.log_scale, &self.aggregate_buckets)
    }

    /// Returns a lazy reference to the bucket at the given index.
    pub fn bucket(&self, index: usize) -> BucketRef<'_> {
        BucketRef::new(self.log_scale, index, self.aggregate_buckets[index])
    }

    /// Re-bins this histogram into a different log scale, preserving all slots.
    ///
    /// For each target bucket `[left, right)`, estimates the sample count via
    /// `count_below(right) - count_below(left)` using f64 CDF values, then
    /// rounds to the nearest integer while tracking the fractional remainder
    /// to preserve the exact total.
    pub fn rescale(&self, width: usize) -> Histogram<T>
    where T: Clone {
        let dst_scale = LogScale::get(width);

        let aggregate_buckets = Self::rebin(self.log_scale, &self.aggregate_buckets, dst_scale);

        let mut slots = SlotQueue::new(self.slots.slot_limit);
        for src_slot in self.slots.iter_all() {
            slots.push_back(Slot {
                buckets: Self::rebin(self.log_scale, &src_slot.buckets, dst_scale),
                data: src_slot.data.clone(),
            });
        }
        slots.current_data = self.slots.current_data.clone();

        Histogram {
            log_scale: dst_scale,
            slots,
            aggregate_buckets,
        }
    }

    /// Re-bins bucket counts from one log scale to another.
    fn rebin(src_scale: &LogScale, src_buckets: &[u64], dst_scale: &'static LogScale) -> Vec<u64> {
        let mut dst = vec![0u64; dst_scale.num_buckets()];
        let mut cursor = CumulativeCount::new(src_scale, src_buckets);
        let mut prev_cdf = 0u64;

        for (i, count) in dst.iter_mut().enumerate() {
            let cdf_right = cursor.count_below(dst_scale.bucket_span(i).right()).round() as u64;
            *count = cdf_right - prev_cdf;
            prev_cdf = cdf_right;
        }

        dst
    }

    /// Returns a display wrapper that prints non-empty buckets, one per line.
    pub fn display_buckets(&self) -> DisplayBuckets<'_, T> {
        DisplayBuckets::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::histogram::LogScaleConfig;

    fn scale() -> &'static LogScale {
        LogScale::get(3)
    }

    #[test]
    fn test_slot_clear() {
        let mut slot: Slot<String> = Slot::new(10);
        slot.buckets[0] = 5;
        slot.buckets[5] = 10;
        slot.data = Some("test".to_string());

        slot.clear();

        assert!(slot.buckets.iter().all(|&c| c == 0));
        assert_eq!(slot.data, None);
    }

    #[test]
    fn test_histogram_default() {
        let hist: Histogram = Histogram::default();
        assert_eq!(hist.slot_limit(), 1);
        assert_eq!(hist.active_slot_count(), 0);
        assert_eq!(hist.total(), 0);
    }

    #[test]
    fn test_record_and_total() {
        let mut hist: Histogram = Histogram::new();

        hist.record(1);
        hist.record(5);
        hist.record(10);
        hist.record(100);

        assert_eq!(hist.total(), 4);
        assert_eq!(hist.aggregate_buckets[1], 1);
        assert_eq!(hist.aggregate_buckets[5], 1);
        assert_eq!(hist.aggregate_buckets[scale().calculate_bucket(10)], 1);
        assert_eq!(hist.aggregate_buckets[scale().calculate_bucket(100)], 1);
    }

    #[test]
    fn test_record_same_bucket() {
        let mut hist: Histogram = Histogram::new();

        hist.record(8);
        hist.record(8);
        hist.record(8);

        assert_eq!(hist.total(), 3);
        assert_eq!(hist.aggregate_buckets[8], 3);
    }

    #[test]
    fn test_record_n() {
        let mut hist: Histogram = Histogram::new();

        hist.record_n(10, 5);
        hist.record_n(100, 3);

        assert_eq!(hist.total(), 8);
        assert_eq!(hist.aggregate_buckets[scale().calculate_bucket(10)], 5);
        assert_eq!(hist.aggregate_buckets[scale().calculate_bucket(100)], 3);
    }

    #[test]
    fn test_record_n_equivalent_to_record() {
        let mut hist_n: Histogram = Histogram::new();
        let mut hist_single: Histogram = Histogram::new();

        hist_n.record_n(42, 4);
        for _ in 0..4 {
            hist_single.record(42);
        }

        assert_eq!(hist_n.total(), hist_single.total());
        let bucket = scale().calculate_bucket(42);
        assert_eq!(hist_n.aggregate_buckets[bucket], hist_single.aggregate_buckets[bucket]);
    }

    #[test]
    fn test_u64_max_coverage() {
        let max_bucket = scale().calculate_bucket_uncached(u64::MAX);
        assert_eq!(max_bucket, 251, "u64::MAX should map to bucket 251");
        assert_eq!(LogScaleConfig::new(3).buckets(), 252, "Should need exactly 252 buckets");

        // Verify new() creates enough buckets to record u64::MAX
        let mut hist: Histogram = Histogram::new();
        assert_eq!(hist.num_buckets(), 252);
        hist.record(u64::MAX);
        assert_eq!(hist.aggregate_buckets[251], 1);
        assert_eq!(hist.total(), 1);
    }

    #[test]
    fn test_percentile_empty() {
        let hist: Histogram = Histogram::new();
        assert_eq!(hist.percentile(0.5), 0);
        assert_eq!(hist.percentile_stats(), PercentileStats {
            samples: 0,
            p0_1: 0,
            p1: 0,
            p5: 0,
            p10: 0,
            p50: 0,
            p90: 0,
            p99: 0,
            p99_9: 0
        });
    }

    #[test]
    fn test_value_at_rank() {
        let mut hist: Histogram = Histogram::new();

        // 10 samples at value 5 (bucket [5,6)), 3 samples at value 100 (bucket [96,112))
        hist.record_n(5, 10);
        hist.record_n(100, 3);

        // Rank 0: no samples before rank 0
        assert_eq!(hist.value_at_rank(0), 0);

        // Ranks 1-10 fall in bucket [5,6): single-width bucket returns 5
        assert_eq!(hist.value_at_rank(1), 5);
        assert_eq!(hist.value_at_rank(5), 5);
        assert_eq!(hist.value_at_rank(10), 5);

        // Ranks 11-13 fall in bucket [96,112), count=3, uniform interpolation
        // (both neighbors are empty, so t = f = rank_in_bucket / count):
        //   rank 11: f=1/3, 96 + floor(16 * 1/3) = 96 + 5  = 101
        //   rank 12: f=2/3, 96 + floor(16 * 2/3) = 96 + 10 = 106
        //   rank 13: f=3/3, 96 + 16 = 112
        assert_eq!(hist.value_at_rank(11), 101);
        assert_eq!(hist.value_at_rank(12), 106);
        assert_eq!(hist.value_at_rank(13), 112);

        // Rank beyond total returns 0
        assert_eq!(hist.value_at_rank(14), 0);
    }

    #[test]
    fn test_count_below() {
        let mut hist: Histogram = Histogram::new();

        // 10 samples at value 5 (bucket [5,6)), 20 samples at value 100 (bucket [96,112))
        hist.record_n(5, 10);
        hist.record_n(100, 20);

        // Before any samples
        assert_eq!(hist.count_below(0), 0);
        assert_eq!(hist.count_below(5), 0);

        // At bucket [5,6) right boundary: all 10 counted
        assert_eq!(hist.count_below(6), 10);

        // Between the two buckets: still 10
        assert_eq!(hist.count_below(50), 10);
        assert_eq!(hist.count_below(96), 10);

        // Within bucket [96,112), width=16, count=20:
        //   count_below(100) = 10 + 20 * (100-96)/16 = 10 + 5 = 15
        //   count_below(104) = 10 + 20 * (104-96)/16 = 10 + 10 = 20
        //   count_below(108) = 10 + 20 * (108-96)/16 = 10 + 15 = 25
        assert_eq!(hist.count_below(100), 15);
        assert_eq!(hist.count_below(104), 20);
        assert_eq!(hist.count_below(108), 25);

        // At right boundary: all 30
        assert_eq!(hist.count_below(112), 30);

        // Beyond all buckets
        assert_eq!(hist.count_below(1000), 30);

        // Trapezoidal case: three adjacent buckets with unequal counts.
        // Bucket 8:[8,10) count=10, bucket 9:[10,12) count=20, bucket 10:[12,14) count=40
        // density_slope for bucket 9 uses d0=10/2=5, d2=40/2=20, m0=9, m2=13
        // k = (20-5)/(13-9) = 3.75, s = k*2 = 7.5
        // d1 = 20/2 = 10
        // CDF: C(t) = (10 - 3.75)*t + 7.5*t^2/2 = 6.25*t + 3.75*t^2
        // C(1) = 10 = d1 ✓
        // fraction = C(t)/10
        let mut hist2: Histogram = Histogram::new();
        hist2.record_n(8, 10);
        hist2.record_n(10, 20);
        hist2.record_n(12, 40);

        // t=0 → fraction=0
        assert_eq!(hist2.count_below(10), 10);
        // t=0.5 → C(0.5) = 6.25*0.5 + 3.75*0.25 = 3.125 + 0.9375 = 4.0625
        //          fraction = 4.0625/10 = 0.40625 → partial = floor(20 * 0.40625) = 8
        assert_eq!(hist2.count_below(11), 10 + 8);
        // t=1.0 → full bucket: all 20
        assert_eq!(hist2.count_below(12), 10 + 20);
    }

    #[test]
    fn test_percentile_single_value() {
        let mut hist: Histogram = Histogram::new();
        hist.record(10);

        assert_eq!(hist.percentile(0.0), 11);
        assert_eq!(hist.percentile(0.5), 11);
        assert_eq!(hist.percentile(0.99), 11);
        assert_eq!(hist.percentile(1.0), 11);
    }

    #[test]
    fn test_percentile_multiple_values() {
        let mut hist: Histogram = Histogram::new();

        // Record 100 values: 1-10 each recorded 10 times
        for value in 1..=10 {
            for _ in 0..10 {
                hist.record(value);
            }
        }

        assert_eq!(hist.total(), 100);

        // P50 should be around value 5-6 (bucket returns left boundary)
        let p50 = hist.percentile(0.5);
        assert!((4..=6).contains(&p50), "P50 = {p50}");

        // P90 should be around value 9 (bucket 8 contains [8,9])
        let p90 = hist.percentile(0.9);
        assert!((8..=10).contains(&p90), "P90 = {p90}");

        // P99 should be around value 10
        let p99 = hist.percentile(0.99);
        assert!((9..=11).contains(&p99), "P99 = {p99}");
    }

    #[test]
    fn test_percentile_stats() {
        let mut hist: Histogram = Histogram::new();

        for i in 1..=100 {
            hist.record(i);
        }

        let stats = hist.percentile_stats();

        // Due to logarithmic bucketing, values are grouped
        // P50 around 50, bucket left boundary might be 48
        assert!(stats.p50 >= 48 && stats.p50 <= 52, "P50 = {}", stats.p50);
        // P90 around 90, bucket left boundary might be 80
        assert!(stats.p90 >= 80 && stats.p90 <= 92, "P90 = {}", stats.p90);
        // P99 around 99, interpolated within bucket [96, 111]
        assert!(stats.p99 >= 96 && stats.p99 <= 112, "P99 = {}", stats.p99);
    }

    #[test]
    fn test_percentile_large_values() {
        let mut hist: Histogram = Histogram::new();

        // Record exponentially distributed values
        hist.record(1);
        hist.record(10);
        hist.record(100);
        hist.record(1000);
        hist.record(10000);

        assert_eq!(hist.total(), 5);

        // P50 (median) should be the 3rd value (100), bucket [96,111] midpoint is 103
        let p50 = hist.percentile(0.5);
        assert!((96..=104).contains(&p50), "P50 = {p50}");

        // P80 should be the 4th value (1000), but bucket returns left boundary
        let p80 = hist.percentile(0.8);
        assert!((896..=1000).contains(&p80), "P80 = {p80}");
    }

    // Multi-slot tests

    #[test]
    fn test_with_slots_creates_correct_slot_limit() {
        let hist: Histogram<u64> = Histogram::with_slots(4);
        assert_eq!(hist.slot_limit(), 4);
        assert_eq!(hist.active_slot_count(), 1);
    }

    #[test]
    fn test_advance_single_slot() {
        let mut hist: Histogram<u64> = Histogram::new();
        // With slot_limit<=1, no slots exist; advance just clears aggregate
        hist.record(42);
        assert_eq!(hist.total(), 1);
        assert_eq!(hist.advance(10), 0);
        assert_eq!(hist.active_slot_count(), 0);
        assert_eq!(hist.total(), 0);
    }

    #[test]
    fn test_advance_multi_slot_not_full() {
        let mut hist: Histogram<u64> = Histogram::with_slots(4);

        hist.record(100);
        assert_eq!(hist.active_slot_count(), 1);
        assert_eq!(hist.total(), 1);

        // Advance adds new slot (now 2 slots)
        assert_eq!(hist.advance(10), 2);
        hist.record(200);
        assert_eq!(hist.active_slot_count(), 2);
        assert_eq!(hist.total(), 2);
        assert_eq!(hist.slots.current_data, Some(10));

        // Advance adds new slot (now 3 slots)
        assert_eq!(hist.advance(20), 3);
        assert_eq!(hist.active_slot_count(), 3);

        // Advance adds new slot (now 4 slots = full)
        assert_eq!(hist.advance(30), 4);
        assert_eq!(hist.active_slot_count(), 4);
    }

    #[test]
    fn test_advance_evicts_oldest() {
        let mut hist: Histogram<u64> = Histogram::with_slots(4);

        hist.record(100); // initial slot
        hist.advance(10); // slot with data=10
        hist.record(200); // record to current
        hist.advance(20); // slot with data=20
        hist.advance(30); // slot with data=30, now at capacity

        assert_eq!(hist.active_slot_count(), 4);
        assert_eq!(hist.total(), 2); // 100 in slot 0, 200 in slot 1

        // Advance again - evicts oldest (slot with 100), adds new slot
        assert_eq!(hist.advance(40), 4);
        assert_eq!(hist.active_slot_count(), 4);
        assert_eq!(hist.total(), 1); // Only 200 remains (in what is now slot 0)

        // After eviction, slots shifted:
        // slot 0: was slot 1 (has 200, data=10)
        // slot 1: was slot 2 (data=20)
        // slot 2: was slot 3 (data=30)
        // slot 3: new slot (data=40)
        assert_eq!(hist.slots.slots.front().unwrap().data, Some(10));
        assert_eq!(hist.slots.current_data, Some(40));
    }

    #[test]
    fn test_advance_slot_limit_stays_constant() {
        let mut hist: Histogram<u64> = Histogram::with_slots(3);
        assert_eq!(hist.slot_limit(), 3);

        // Fill to capacity
        hist.advance(1);
        hist.advance(2);
        assert_eq!(hist.active_slot_count(), 3);
        assert_eq!(hist.slot_limit(), 3);

        // Advance multiple times past the slot limit; the limit must not grow.
        for i in 3..10 {
            hist.advance(i);
            assert_eq!(hist.slot_limit(), 3, "slot limit grew unexpectedly at iteration {i}");
            assert_eq!(hist.active_slot_count(), 3);
        }

        // Verify oldest slots were evicted - only last 3 data values remain
        // 2 stored historicals + 1 implicit current
        assert_eq!(hist.slots.slots.front().unwrap().data, Some(7));
        assert_eq!(hist.slots.slots.get(1).unwrap().data, Some(8));
        assert_eq!(hist.slots.current_data, Some(9));
    }

    #[test]
    fn test_advance_uses_slot_limit_instead_of_vecdeque_capacity() {
        let mut hist: Histogram<u64> = Histogram::with_slots(2);

        hist.slots.slots.reserve(16);

        assert!(hist.slots.slots.capacity() > hist.slot_limit());

        hist.advance(1);
        assert_eq!(hist.active_slot_count(), 2);

        hist.advance(2);
        assert_eq!(hist.active_slot_count(), 2);
        assert_eq!(hist.slots.slots.front().unwrap().data, Some(1));
        assert_eq!(hist.slots.current_data, Some(2));
    }

    #[test]
    fn test_slot_data_access() {
        let mut hist: Histogram<String> = Histogram::with_slots(3);

        // Initially no stored slots, current_data is None
        assert_eq!(hist.slots.current_data, None);

        // Advance adds new slot with data
        hist.advance("first".to_string());
        assert_eq!(hist.active_slot_count(), 2);
        assert_eq!(hist.slots.current_data, Some("first".to_string()));

        // Advance adds another slot with data
        hist.advance("second".to_string());
        assert_eq!(hist.active_slot_count(), 3);
        assert_eq!(hist.slots.current_data, Some("second".to_string()));
    }

    #[test]
    fn test_percentile_across_slots() {
        let mut hist: Histogram<u64> = Histogram::with_slots(4);

        // Record in initial slot
        for v in 1..=50 {
            hist.record(v);
        }

        hist.advance(1);

        // Record in new slot
        for v in 51..=100 {
            hist.record(v);
        }

        assert_eq!(hist.total(), 100);

        // P50 should be around 50
        let p50 = hist.percentile(0.5);
        assert!((48..=52).contains(&p50), "P50 = {p50}");
    }

    #[test]
    fn test_with_slots_zero_same_as_one() {
        let mut hist0: Histogram = Histogram::with_slots(0);
        let mut hist1: Histogram = Histogram::with_slots(1);

        assert_eq!(hist0.active_slot_count(), 0);
        assert_eq!(hist1.active_slot_count(), 0);

        hist0.record(100);
        hist1.record(100);
        assert_eq!(hist0.total(), hist1.total());
        assert_eq!(hist0.percentile(0.5), hist1.percentile(0.5));
    }

    #[test]
    fn test_aggregate_buckets_consistency() {
        let mut hist: Histogram<u64> = Histogram::with_slots(3);

        // Verify invariant: aggregate[i] >= Σ historical[i] for all i
        let assert_consistent = |h: &Histogram<u64>| {
            let n = h.active_slot_count();
            if n <= 1 {
                return;
            }
            for bucket_idx in 0..h.num_buckets() {
                let hist_sum: u64 = h.slots.slots.iter().map(|s| s.buckets[bucket_idx]).sum();
                assert!(
                    h.aggregate_buckets[bucket_idx] >= hist_sum,
                    "aggregate[{bucket_idx}]={} < historical_sum={hist_sum}",
                    h.aggregate_buckets[bucket_idx]
                );
            }
        };

        // Record values in first slot
        for v in [1, 10, 100, 1000] {
            hist.record(v);
        }
        assert_consistent(&hist);
        assert_eq!(hist.total(), 4);

        // Advance and record more
        hist.advance(1);
        for v in [2, 20, 200] {
            hist.record(v);
        }
        assert_consistent(&hist);
        assert_eq!(hist.total(), 7);

        // Fill to capacity
        hist.advance(2);
        hist.record(3);
        assert_eq!(hist.active_slot_count(), 3);
        assert_consistent(&hist);
        assert_eq!(hist.total(), 8);

        // Advance past capacity - evicts first slot with [1,10,100,1000]
        hist.advance(3);
        assert_eq!(hist.active_slot_count(), 3);
        assert_consistent(&hist);
        assert_eq!(hist.total(), 4);
    }

    // Edge case tests for all practical WIDTH values (1-10).
    // WIDTH range is theoretically 1..=65, but memory grows as 2^(WIDTH-1) buckets per group.
    // WIDTH=10 already uses ~224KB per slot; beyond that is impractical for testing.

    macro_rules! test_histogram_width_edge_cases {
        ($name:ident, $width:expr) => {
            #[test]
            fn $name() {
                assert_eq!(
                    LogScale::get($width).num_buckets(),
                    LogScaleConfig::new($width).buckets()
                );

                let mut hist = Histogram::<()>::with_log_scale($width, 2);

                // Edge values: 0, 1, u64::MAX
                hist.record(0);
                hist.record(1);
                hist.record(u64::MAX);
                assert_eq!(hist.total(), 3);

                // Percentile must not panic on edge values
                let p0 = hist.percentile(0.0);
                assert_eq!(p0, 0);
                let _ = hist.percentile(0.5);
                let p100 = hist.percentile(1.0);
                assert!(p100 > 0);

                // Multi-slot: advance and verify aggregate
                hist.advance(());
                hist.record(1000);
                assert_eq!(hist.total(), 4);

                // Evict oldest slot
                hist.advance(());
                assert_eq!(hist.total(), 1);
            }
        };
    }

    #[test]
    fn test_single_slot_optimization() {
        let mut hist: Histogram<u64> = Histogram::new();
        assert_eq!(hist.active_slot_count(), 0);

        hist.record(100);
        hist.record(200);
        assert_eq!(hist.total(), 2);

        // advance clears aggregate, no slots exist
        hist.advance(1);
        assert_eq!(hist.total(), 0);
        assert_eq!(hist.active_slot_count(), 0);

        // record after advance works correctly
        hist.record(50);
        assert_eq!(hist.total(), 1);
        let p50 = hist.percentile(0.5);
        assert!((48..=52).contains(&p50), "P50 = {p50}");

        // multiple advance cycles
        hist.advance(2);
        assert_eq!(hist.total(), 0);
        hist.record(1000);
        hist.record(1000);
        assert_eq!(hist.total(), 2);
    }

    #[test]
    fn test_rescale() {
        use std::collections::BTreeMap;

        let mut src = Histogram::<()>::with_log_scale(2, 1);
        src.record_n(10, 20);
        src.record_n(100, 30);
        src.record_n(1000, 50);

        let rescaled = src.rescale(5);

        println!("src:\n{}", src.display_buckets());
        println!("rescaled:\n{}", rescaled.display_buckets());

        assert_eq!(rescaled.total(), 100);

        let ranks: Vec<u64> = (1..=100).step_by(5).collect();

        let old_values: BTreeMap<u64, u64> = ranks.iter().map(|&r| (r, src.value_at_rank(r))).collect();
        let new_values: BTreeMap<u64, u64> = ranks.iter().map(|&r| (r, rescaled.value_at_rank(r))).collect();

        println!("rank → old(W=2) / new(W=5):");
        for &r in &ranks {
            println!("  {r:>3} → {:>5} / {:>5}", old_values[&r], new_values[&r]);
        }

        assert_eq!(
            old_values,
            BTreeMap::from([
                (1, 8),
                (6, 9),
                (11, 10),
                (16, 11),
                (21, 97),
                (26, 102),
                (31, 107),
                (36, 113),
                (41, 118),
                (46, 123),
                (51, 773),
                (56, 798),
                (61, 824),
                (66, 849),
                (71, 875),
                (76, 901),
                (81, 926),
                (86, 952),
                (91, 977),
                (96, 1003),
            ])
        );

        assert_eq!(
            new_values,
            BTreeMap::from([
                (1, 8),
                (6, 9),
                (11, 10),
                (16, 11),
                (21, 97),
                (26, 101),
                (31, 108),
                (36, 113),
                (41, 117),
                (46, 124),
                (51, 774),
                (56, 800),
                (61, 822),
                (66, 847),
                (71, 874),
                (76, 901),
                (81, 928),
                (86, 950),
                (91, 975),
                (96, 1001),
            ])
        );
    }

    #[test]
    fn test_rescale_roundtrip() {
        let mut src = Histogram::<()>::with_log_scale(2, 1);
        src.record_n(10, 20);
        src.record_n(100, 30);
        src.record_n(1000, 50);

        let fine = src.rescale(5);
        let back = fine.rescale(2);

        assert_eq!(back.total(), src.total());
        assert_eq!(back.display_buckets().to_string(), src.display_buckets().to_string());
    }

    #[test]
    fn test_rescale_preserves_slots() {
        let mut src = Histogram::<&str>::with_log_scale(2, 3);

        // Slot 0 (initial): record small values
        src.record_n(10, 5);
        src.record_n(50, 3);

        // Slot 1: record medium values
        src.advance("warm");
        src.record_n(100, 4);
        src.record_n(500, 6);

        // Current (implicit): record large values
        src.advance("hot");
        src.record_n(1000, 7);

        assert_eq!(src.active_slot_count(), 3);
        assert_eq!(src.slot_limit(), 3);
        assert_eq!(src.total(), 25);

        let rescaled = src.rescale(5);

        // Slot structure preserved
        assert_eq!(rescaled.slot_limit(), 3);
        assert_eq!(rescaled.active_slot_count(), 3);
        assert_eq!(rescaled.slots.slots.len(), 2); // 2 stored historicals

        // Slot metadata preserved:
        // stored[0] = initial period (data=None)
        // stored[1] = "warm" period
        // current  = "hot" period (implicit)
        assert_eq!(rescaled.slots.slots.front().unwrap().data, None);
        assert_eq!(rescaled.slots.slots.get(1).unwrap().data, Some("warm"));
        assert_eq!(rescaled.slots.current_data, Some("hot"));

        // Total preserved
        assert_eq!(rescaled.total(), 25);

        // Individual slot totals preserved
        let slot0_total: u64 = rescaled.slots.slots.front().unwrap().buckets.iter().sum();
        let slot1_total: u64 = rescaled.slots.slots.get(1).unwrap().buckets.iter().sum();
        assert_eq!(slot0_total, 8); // 5 + 3
        assert_eq!(slot1_total, 10); // 4 + 6
        // Implicit current total = aggregate - stored = 25 - 8 - 10 = 7
        let current_total = rescaled.total() - slot0_total - slot1_total;
        assert_eq!(current_total, 7);

        // Eviction still works after rescale
        let prev_total = rescaled.total();
        let mut rescaled = rescaled;
        rescaled.advance("cool");
        // Evicts slot 0 (total=8), so total drops by 8
        assert_eq!(rescaled.total(), prev_total - slot0_total);
        assert_eq!(rescaled.active_slot_count(), 3);
    }

    test_histogram_width_edge_cases!(test_width_edge_1, 1);
    test_histogram_width_edge_cases!(test_width_edge_2, 2);
    test_histogram_width_edge_cases!(test_width_edge_3, 3);
    test_histogram_width_edge_cases!(test_width_edge_4, 4);
    test_histogram_width_edge_cases!(test_width_edge_5, 5);
    test_histogram_width_edge_cases!(test_width_edge_6, 6);
    test_histogram_width_edge_cases!(test_width_edge_7, 7);
    test_histogram_width_edge_cases!(test_width_edge_8, 8);
    test_histogram_width_edge_cases!(test_width_edge_9, 9);
    test_histogram_width_edge_cases!(test_width_edge_10, 10);
}
