use super::bucket_ref::BucketRef;
use super::log_scale::LogScale;

/// Interpolates between discrete histogram buckets to produce continuous estimates.
///
/// Uses trapezoidal density estimation with neighbor bucket slopes
/// to compute partial-bucket counts and cumulative distributions.
///
/// Operates on a `&LogScale` (for bucket geometry) and a `&[u64]` slice
/// of bucket counts, independent of any particular histogram instance.
#[derive(Clone, Copy)]
pub struct Interpolator<'a> {
    log_scale: &'a LogScale,
    buckets: &'a [u64],
}

impl<'a> Interpolator<'a> {
    /// Creates an interpolator over the given bucket counts.
    pub fn new(log_scale: &'a LogScale, buckets: &'a [u64]) -> Self {
        Self { log_scale, buckets }
    }

    /// Returns the number of buckets.
    #[inline]
    pub fn num_buckets(&self) -> usize {
        self.log_scale.num_buckets()
    }

    /// Returns a reference to the bucket at the given index.
    #[inline]
    pub fn bucket(&self, index: usize) -> BucketRef<'_> {
        BucketRef::new(self.log_scale, index, self.buckets[index])
    }

    /// Computes the density slope at `bucket`.
    ///
    /// Density slope `k` measures the rate of change in sample density
    /// (count/width) between two reference points:
    ///
    /// ```text
    ///   k = (d_right - d_left) / (m_right - m_left)
    /// ```
    ///
    /// where `d = count / width` and `m = midpoint`.
    ///
    /// For interior buckets, uses the left and right neighbors.
    /// For the first bucket, uses self and the right neighbor.
    /// For the last bucket, uses the left neighbor and self.
    pub fn density_slope(&self, bucket: usize) -> f64 {
        let last = self.num_buckets() - 1;

        let (left, right) = if bucket == 0 {
            (bucket, bucket + 1)
        } else if bucket == last {
            (bucket - 1, bucket)
        } else {
            (bucket - 1, bucket + 1)
        };

        let bl = self.bucket(left);
        let br = self.bucket(right);

        let dl = bl.count() as f64 / bl.width() as f64;
        let dr = br.count() as f64 / br.width() as f64;

        (dr - dl) / (br.midpoint() as f64 - bl.midpoint() as f64)
    }

    /// Returns the density slope at `bucket`, clamped so that the linear
    /// density model `d(x) = d1 + k·(x - w/2)` stays non-negative across
    /// the entire bucket width.
    ///
    /// This is `density_slope()` with the additional guarantee that the
    /// resulting slope can be used directly for trapezoidal interpolation
    /// without producing negative density at either edge.
    pub fn clamped_density_slope(&self, bucket: usize) -> f64 {
        let b = self.bucket(bucket);
        let w = b.width() as f64;
        let d1 = b.count() as f64 / w;
        let mut k = self.density_slope(bucket);

        if d1 + k * w / 2.0 < 0.0 {
            k = -d1 / (w / 2.0);
        }
        if d1 - k * w / 2.0 < 0.0 {
            k = d1 / (w / 2.0);
        }

        k
    }

    /// Computes the estimated sample count in `[bucket.left(), bucket.left() + rel_position)`
    /// using trapezoidal density estimation.
    ///
    /// `rel_position` is the offset from the bucket's left boundary.
    /// Since the underlying density is continuous, the boundary is
    /// effectively neither open nor closed (a single point has zero measure).
    ///
    /// Models the density inside the bucket as a linear function:
    ///
    /// ```text
    ///   d(x) = a + k·x,  x ∈ [0, width]
    /// ```
    ///
    /// where `a = d1 - k·width/2` is the density at the left edge,
    /// `k` is the clamped density slope from `clamped_density_slope()`,
    /// and `d1 = count / width` is the average density.
    ///
    /// The CDF is:
    ///
    /// ```text
    ///   C(x) = a·x + k·x²/2
    /// ```
    pub fn trapezoidal_cdf(&self, bucket: usize, rel_position: u64) -> f64 {
        let b = self.bucket(bucket);
        let w = b.width() as f64;
        let x = rel_position as f64;

        let d1 = b.count() as f64 / w;
        let k = self.clamped_density_slope(bucket);

        // d(x) = a + k·x, where a = d1 - k·w/2 is density at left edge
        let a = d1 - k * w / 2.0;
        a * x + k * x * x / 2.0
    }

    /// Returns the smallest absolute position within a bucket that
    /// corresponds to the given rank, using trapezoidal density interpolation.
    ///
    /// `rank` is the 1-based rank within this bucket (1..=count).
    ///
    /// The result is clamped to `[left, right]`.
    /// Returns the bucket midpoint when `count <= 1` or `width == 1`.
    ///
    /// Solves for offset `x` from the bucket's left edge via:
    ///
    /// ```text
    ///   a = count/w − k·w/2        (density at left edge)
    ///   Solve a·x + k·x²/2 = rank:
    ///     x = (−a + √(a² + 2·k·rank)) / k
    /// ```
    ///
    /// Then maps to absolute position: `left + floor(x)`.
    pub fn rank_to_position(&self, bucket: usize, rank: u64) -> u64 {
        let b = self.bucket(bucket);
        let count = b.count();

        if count <= 1 || b.width() == 1 {
            return b.midpoint();
        }

        let rank = rank as f64;
        let w = b.width() as f64;
        let d1 = count as f64 / w;
        let k = self.clamped_density_slope(bucket);

        // Density at left edge
        let a = d1 - k * w / 2.0;

        // Solve a·x + k·x²/2 = rank for x
        let x = if (k * w).abs() < d1.abs() * 1e-9 {
            // Near-uniform density
            rank / d1
        } else {
            let disc = a * a + 2.0 * k * rank;
            if disc < 0.0 { rank / d1 } else { (-a + disc.sqrt()) / k }
        };

        b.left().saturating_add(x.clamp(0.0, w) as u64).min(b.right())
    }

    /// Returns the estimated count of samples in `[0, position)`,
    /// i.e., strictly below `position`, across all buckets.
    ///
    /// Sums the full count of every bucket entirely before `position`,
    /// then adds the partial count within the bucket containing `position`
    /// using `trapezoidal_cdf`. For the terminal bucket, `position == u64::MAX`
    /// still excludes the endpoint mass at `u64::MAX`.
    pub fn count_below(&self, position: u64) -> f64 {
        let mut total = 0.0;

        for i in 0..self.num_buckets() {
            let b = self.bucket(i);
            let past_bucket = if b.is_last() {
                position > b.right()
            } else {
                position >= b.right()
            };

            if past_bucket {
                total += b.count() as f64;
            } else {
                total += self.trapezoidal_cdf(i, position.saturating_sub(b.left()));
                break;
            }
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use crate::histogram::Histogram;

    fn make_hist(records: &[(u64, u64)]) -> Histogram<()> {
        let mut hist = Histogram::<()>::new();
        for &(value, count) in records {
            hist.record_n(value, count);
        }
        hist
    }

    fn slope(records: &[(u64, u64)], bucket: usize) -> f64 {
        let h = make_hist(records);
        h.interpolator().density_slope(bucket)
    }

    // === density_slope tests ===

    #[test]
    fn test_slope_equal_width() {
        // Buckets 8,9,10: [8,10), [10,12), [12,14) — all width=2, midpoints 9,11,13

        // increasing: d0=5, d2=15 → k=2.5
        let k = slope(&[(8, 10), (10, 20), (12, 30)], 9);
        assert!((k - 2.5).abs() < 1e-10);

        // decreasing: d0=15, d2=5 → k=-2.5
        let k = slope(&[(8, 30), (10, 20), (12, 10)], 9);
        assert!((k - (-2.5)).abs() < 1e-10);

        // equal: d0=10, d2=10 → k=0
        let k = slope(&[(8, 20), (10, 20), (12, 20)], 9);
        assert!(k.abs() < 1e-10);
    }

    #[test]
    fn test_slope_unequal_width() {
        // Bucket 11: [14,16) w=2 mid=15, Bucket 13: [20,24) w=4 mid=22

        // c0=4, c2=8 → d0=2, d2=2 → k=0
        let k = slope(&[(14, 4), (16, 1), (20, 8)], 12);
        assert!(k.abs() < 1e-10);

        // c0=2, c2=8 → d0=1, d2=2 → k=(2-1)/(22-15)=1/7
        let k = slope(&[(14, 2), (16, 1), (20, 8)], 12);
        assert!((k - 1.0 / 7.0).abs() < 1e-10);
    }

    #[test]
    fn test_slope_edge_buckets() {
        // First bucket: uses (self=0, right=1) → k=(30-10)/(1-0)=20
        let k = slope(&[(0, 10), (1, 30)], 0);
        assert!((k - 20.0).abs() < 1e-10);

        // Second bucket: interior, uses (0, 2) → k=(30-10)/(2-0)=10
        let k = slope(&[(0, 10), (1, 20), (2, 30)], 1);
        assert!((k - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_slope_last_buckets() {
        let w250: f64 = (1u128 << 61) as f64;
        let span251: f64 = (u64::MAX - (0b111 << 61)) as f64;
        let w251: f64 = (u64::MAX - (0b111 << 61) + 1) as f64;
        let m250: f64 = (0b110u128 << 61) as f64 + w250 / 2.0;
        let m251: f64 = (0b111u128 << 61) as f64 + span251 / 2.0;

        // Last bucket (251): uses (left=250, self=251)
        let k = slope(&[(0b110 << 61, 100), (0b111 << 61, 200)], 251);
        let expected = (200.0 / w251 - 100.0 / w250) / (m251 - m250);
        assert!((k - expected).abs() / expected.abs() < 1e-10);

        // Second-to-last (250): interior, uses (249, 251)
        let k = slope(&[(0b101 << 61, 100), (0b111 << 61, 200)], 250);
        let m249: f64 = (0b101u128 << 61) as f64 + (1u128 << 61) as f64 / 2.0;
        let expected = (200.0 / w251 - 100.0 / w250) / (m251 - m249);
        assert!((k - expected).abs() / expected.abs() < 1e-10);
    }

    #[test]
    fn test_slope_zero_counts() {
        let k = slope(&[], 0);
        assert!(k.abs() < 1e-10);

        let k = slope(&[], 9);
        assert!(k.abs() < 1e-10);

        let k = slope(&[], 251);
        assert!(k.abs() < 1e-10);

        // One empty neighbor: d0=0, d2=20/2=10 → k=(10-0)/(13-9)=2.5
        let k = slope(&[(10, 5), (12, 20)], 9);
        assert!((k - 2.5).abs() < 1e-10);

        // Both neighbors empty → k=0
        let k = slope(&[(10, 50)], 9);
        assert!(k.abs() < 1e-10);
    }

    // === clamped_density_slope tests ===

    fn clamped_slope(records: &[(u64, u64)], bucket: usize) -> f64 {
        let h = make_hist(records);
        h.interpolator().clamped_density_slope(bucket)
    }

    #[test]
    fn test_clamped_slope_passes_through_when_no_clamping_needed() {
        // Moderate slope: bucket 9 [10,12) w=2, d1=10
        // k=2.5, edge densities: d1-k*w/2=7.5, d1+k*w/2=12.5 — both positive
        let r = &[(8, 10), (10, 20), (12, 30)];
        assert!((clamped_slope(r, 9) - slope(r, 9)).abs() < 1e-10);
    }

    #[test]
    fn test_clamped_slope_clamps_negative_right_edge() {
        // Bucket 9: [10,12) w=2, count=2 → d1=1
        // Steep negative slope from neighbors: left has much more than right
        // Raw k would make d1 + k*w/2 < 0 (right edge negative)
        // Clamped: k = -d1 / (w/2) = -1/1 = -1
        let r = &[(8, 100), (10, 2), (12, 0)];
        let raw = slope(r, 9);
        let clamped = clamped_slope(r, 9);

        // Raw slope is very negative
        assert!(raw < -1.0);
        // Clamped to -d1/(w/2) = -1.0
        assert!((clamped - (-1.0)).abs() < 1e-10);
    }

    #[test]
    fn test_clamped_slope_clamps_negative_left_edge() {
        // Bucket 9: [10,12) w=2, count=2 → d1=1
        // Steep positive slope: right has much more than left
        // Raw k would make d1 - k*w/2 < 0 (left edge negative)
        // Clamped: k = d1 / (w/2) = 1/1 = 1
        let r = &[(8, 0), (10, 2), (12, 100)];
        let raw = slope(r, 9);
        let clamped = clamped_slope(r, 9);

        assert!(raw > 1.0);
        assert!((clamped - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_clamped_slope_zero_count() {
        // Empty bucket: d1=0, any slope is clamped to 0
        let k = clamped_slope(&[], 9);
        assert!(k.abs() < 1e-10);
    }

    #[test]
    fn test_clamped_slope_uniform_neighbors() {
        // Equal neighbors → k=0 → no clamping
        let r = &[(8, 20), (10, 20), (12, 20)];
        assert!(clamped_slope(r, 9).abs() < 1e-10);
    }

    #[test]
    fn test_clamped_slope_guarantees_nonnegative_density() {
        // For any clamped slope, both edges must be >= 0:
        //   left edge:  d1 - k*w/2 >= 0
        //   right edge: d1 + k*w/2 >= 0
        let test_cases: &[&[(u64, u64)]] = &[
            &[(8, 100), (10, 2), (12, 0)],
            &[(8, 0), (10, 2), (12, 100)],
            &[(8, 1), (10, 1000), (12, 1)],
            &[(8, 1000), (10, 1), (12, 1000)],
        ];

        for records in test_cases {
            let h = make_hist(records);
            let interp = h.interpolator();
            let b = interp.bucket(9);
            let w = b.width() as f64;
            let d1 = b.count() as f64 / w;
            let k = interp.clamped_density_slope(9);

            let left_edge = d1 - k * w / 2.0;
            let right_edge = d1 + k * w / 2.0;
            assert!(left_edge >= -1e-10, "left edge {left_edge} < 0 for {records:?}");
            assert!(right_edge >= -1e-10, "right edge {right_edge} < 0 for {records:?}");
        }
    }

    // === trapezoidal_cdf tests ===

    fn cdf(records: &[(u64, u64)], bucket: usize, x: u64) -> f64 {
        let h = make_hist(records);
        h.interpolator().trapezoidal_cdf(bucket, x)
    }

    #[test]
    fn test_cdf_at_boundaries() {
        // Bucket 9: [10,12) count=20, width=2
        let r = &[(8, 10), (10, 20), (12, 30)];

        let c_left = cdf(r, 9, 0);
        assert!(c_left.abs() < 1e-10);

        let c_right = cdf(r, 9, 2);
        assert!((c_right - 20.0).abs() < 1e-10);
    }

    #[test]
    fn test_cdf_uniform_density() {
        // Equal neighbors → uniform → cdf(x) = count * x / width
        // Bucket 9: [10,12) count=20, width=2
        let r = &[(8, 20), (10, 20), (12, 20)];

        let c_mid = cdf(r, 9, 1);
        assert!((c_mid - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_cdf_slope_direction() {
        // Bucket 9: [10,12) width=2, midpoint at x=1
        // Increasing density: less area on left half → cdf(midpoint) < count/2
        let c = cdf(&[(8, 10), (10, 20), (12, 30)], 9, 1);
        assert!(c < 10.0);
        assert!(c > 0.0);

        // Decreasing density: more area on left half → cdf(midpoint) > count/2
        let c = cdf(&[(8, 30), (10, 20), (12, 10)], 9, 1);
        assert!(c > 10.0);
        assert!(c < 20.0);
    }

    #[test]
    fn test_cdf_monotonicity() {
        // Bucket 12: [16,20) width=4
        let r = &[(14, 10), (16, 50), (20, 30)];
        let mut prev = 0.0;
        for x in 0..=4 {
            let c = cdf(r, 12, x);
            assert!(c >= prev, "cdf({x}) = {c} < prev = {prev}");
            prev = c;
        }
    }

    #[test]
    fn test_cdf_zero_count() {
        let c = cdf(&[], 9, 1);
        assert!(c.abs() < 1e-10);
    }

    #[test]
    fn test_cdf_first_bucket() {
        // Bucket 0: [0,1) width=1
        // Uses (self=0, right=1) for slope
        let r = &[(0, 10), (1, 30)];

        let c = cdf(r, 0, 0);
        assert!(c.abs() < 1e-10);

        let c = cdf(r, 0, 1);
        assert!((c - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_cdf_last_bucket() {
        // Bucket 251: [0b111<<61, u64::MAX]
        // Uses (left=250, self=251) for slope
        let r = &[(0b110 << 61, 100), (0b111 << 61, 200)];

        let c = cdf(r, 251, 0);
        assert!(c.abs() < 1e-10);

        let w251 = u64::MAX - (0b111 << 61) + 1;
        let c = cdf(r, 251, w251);
        assert!((c - 200.0).abs() < 1e-6);
    }

    #[test]
    fn test_cdf_one_neighbor_empty() {
        // Bucket 9: [10,12) width=2, count=20
        // Left neighbor (bucket 8) empty, right neighbor count=30
        // Slope from (8,10): d0=0, d2=30/2=15 → k=(15-0)/(13-9)=3.75
        let r = &[(10, 20), (12, 30)];

        let c = cdf(r, 9, 0);
        assert!(c.abs() < 1e-10);

        let c = cdf(r, 9, 2);
        assert!((c - 20.0).abs() < 1e-10);

        // With positive slope, cdf(midpoint) < count/2
        let c = cdf(r, 9, 1);
        assert!(c < 10.0);
        assert!(c > 0.0);
    }

    #[test]
    fn test_cdf_both_neighbors_empty() {
        // Only target bucket has data → k=0 → uniform
        // Bucket 9: [10,12) width=2, count=50
        let r = &[(10, 50)];

        let c = cdf(r, 9, 1);
        assert!((c - 25.0).abs() < 1e-10);
    }

    #[test]
    fn test_cdf_large_count() {
        // Bucket 9: [10,12) width=2, count=1_000_000
        let r = &[(8, 500_000), (10, 1_000_000), (12, 500_000)];

        let c = cdf(r, 9, 0);
        assert!(c.abs() < 1e-10);

        let c = cdf(r, 9, 2);
        assert!((c - 1_000_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_cdf_wider_bucket() {
        // Bucket 12: [16,20) width=4, count=40
        // Neighbors: bucket 11 [14,16) c=20, bucket 13 [20,24) c=60
        // d0=20/2=10, d2=60/4=15, midpoints 15 and 22
        // k=(15-10)/(22-15)=5/7
        let r = &[(14, 20), (16, 40), (20, 60)];

        let c = cdf(r, 12, 0);
        assert!(c.abs() < 1e-10);

        let c = cdf(r, 12, 4);
        assert!((c - 40.0).abs() < 1e-10);

        // Positive slope → cdf(midpoint) < count/2
        let c = cdf(r, 12, 2);
        assert!(c < 20.0);
        assert!(c > 0.0);
    }

    #[test]
    fn test_cdf_symmetry() {
        // Symmetric neighbors → uniform → cdf at midpoint = count/2
        // Bucket 12: [16,20) width=4, count=100
        // Neighbors: bucket 11 c=50 w=2, bucket 13 c=100 w=4
        // d0=50/2=25, d2=100/4=25 → equal density → k=0
        let r = &[(14, 50), (16, 100), (20, 100)];

        let c = cdf(r, 12, 2);
        assert!((c - 50.0).abs() < 1e-10);
    }

    // === rank_to_position tests ===

    fn pos(records: &[(u64, u64)], bucket: usize, rank: u64) -> u64 {
        let h = make_hist(records);
        h.interpolator().rank_to_position(bucket, rank)
    }

    #[test]
    fn test_rank_to_position_single_sample() {
        // count=1 → returns midpoint
        // Bucket 9: [10,12) midpoint=11
        let r = &[(10, 1)];
        assert_eq!(pos(r, 9, 1), 11);
    }

    #[test]
    fn test_rank_to_position_width_one() {
        // Bucket 5: [5,6) width=1 → returns midpoint=5
        let r = &[(5, 10)];
        assert_eq!(pos(r, 5, 5), 5);
    }

    #[test]
    fn test_rank_to_position_uniform() {
        // Equal neighbors → uniform interpolation
        // Bucket 9: [10,12) width=2, count=20
        // rank 10 / count 20 = 0.5 → t=0.5 → left + floor(2 * 0.5) = 10 + 1 = 11
        let r = &[(8, 20), (10, 20), (12, 20)];
        assert_eq!(pos(r, 9, 10), 11);

        // rank 1 / 20 = 0.05 → t=0.05 → 10 + floor(2*0.05) = 10
        assert_eq!(pos(r, 9, 1), 10);

        // rank 20 / 20 = 1.0 → x=w → left + w = right = 12
        assert_eq!(pos(r, 9, 20), 12);
    }

    #[test]
    fn test_rank_to_position_increasing_density() {
        // Increasing density: mass shifts right → position > midpoint for median rank
        // Bucket 9: [10,12) width=2, count=20
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mid_pos = pos(r, 9, 10);
        assert!(mid_pos >= 10);
        assert!(
            mid_pos > 10,
            "increasing density: pos={mid_pos} should be > midpoint 10"
        );
    }

    #[test]
    fn test_rank_to_position_decreasing_density() {
        // Decreasing density: mass shifts left → position < midpoint for median rank
        // Bucket 9: [10,12) width=2, count=20
        let r = &[(8, 30), (10, 20), (12, 10)];
        let mid_pos = pos(r, 9, 10);
        assert!(mid_pos >= 10);
        assert!(mid_pos < 11, "decreasing density: pos={mid_pos} should be < 11");
    }

    #[test]
    fn test_rank_to_position_monotonicity() {
        // Position must be non-decreasing as rank increases
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut prev = 0;
        for rank in 1..=20 {
            let p = pos(r, 9, rank);
            assert!(p >= prev, "rank {rank}: pos={p} < prev={prev}");
            prev = p;
        }
    }

    #[test]
    fn test_rank_to_position_clamped_to_bucket() {
        // Result must be in [left, right-1]
        let r = &[(8, 10), (10, 20), (12, 30)];
        let b_left = 10u64;
        let b_right = 12u64;

        for rank in 1..=20 {
            let p = pos(r, 9, rank);
            assert!(p >= b_left, "rank {rank}: pos={p} < left={b_left}");
            assert!(p <= b_right, "rank {rank}: pos={p} > right={b_right}");
        }
    }

    #[test]
    fn test_rank_to_position_wider_bucket() {
        // Bucket 12: [16,20) width=4, count=40
        let r = &[(14, 20), (16, 40), (20, 60)];

        // First rank → near left
        let p = pos(r, 12, 1);
        assert!(p >= 16);
        assert!(p <= 17);

        // Last rank → right = 20
        let p = pos(r, 12, 40);
        assert_eq!(p, 20);

        // Monotonicity
        let mut prev = 0;
        for rank in 1..=40 {
            let p = pos(r, 12, rank);
            assert!(p >= prev, "rank {rank}: pos={p} < prev={prev}");
            prev = p;
        }
    }

    // === count_below tests ===

    fn count_below(records: &[(u64, u64)], position: u64) -> f64 {
        let h = make_hist(records);
        h.interpolator().count_below(position)
    }

    #[test]
    fn test_count_below_zero() {
        // Empty histogram
        let c = count_below(&[], 100);
        assert!(c.abs() < 1e-10);

        // Position before all data
        let c = count_below(&[(10, 100)], 5);
        assert!(c.abs() < 1e-10);

        // Position 0 with data in bucket 0: x=0 → cdf=0
        let c = count_below(&[(0, 50)], 0);
        assert!(c.abs() < 1e-10);
    }

    #[test]
    fn test_count_below_past_all_data() {
        // Single bucket
        let c = count_below(&[(10, 100)], 100);
        assert!((c - 100.0).abs() < 1e-10);

        // Multiple buckets
        let c = count_below(&[(8, 10), (10, 20), (12, 30)], 14);
        assert!((c - 60.0).abs() < 1e-10);

        // Scattered data: total must match hist.total()
        let r = &[(0, 5), (5, 10), (10, 20), (100, 50)];
        let h = make_hist(r);
        let c = h.interpolator().count_below(u64::MAX);
        assert!((c - h.total() as f64).abs() < 1e-6);
    }

    #[test]
    fn test_count_below_at_bucket_boundary() {
        // Position 10 = right of bucket 8 [8,10) = left of bucket 9 [10,12)
        // Should include all of bucket 8, none of bucket 9
        let c = count_below(&[(8, 20), (10, 50)], 10);
        assert!((c - 20.0).abs() < 1e-10);

        // Position 1 = right of bucket 0 [0,1)
        let c = count_below(&[(0, 10), (1, 20)], 1);
        assert!((c - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_count_below_single_bucket_midpoint() {
        // Bucket 9: [10,12) w=2, count=100, no neighbors → uniform
        let c = count_below(&[(10, 100)], 11);
        assert!((c - 50.0).abs() < 1e-10);
    }

    #[test]
    fn test_count_below_partial_bucket() {
        // Buckets 8,9,10: [8,10) c=10, [10,12) c=20, [12,14) c=30
        // Position 11 = midpoint of bucket 9
        // Total = 10 (full bucket 8) + trapezoidal_cdf(9, 1)
        let r = &[(8, 10), (10, 20), (12, 30)];

        let c = count_below(r, 11);
        let expected = 10.0 + cdf(r, 9, 1);
        assert!((c - expected).abs() < 1e-10);
    }

    #[test]
    fn test_count_below_monotonicity() {
        let r = &[(8, 10), (10, 20), (12, 30)];
        let mut prev = 0.0;
        for pos in [0, 5, 8, 9, 10, 11, 12, 13, 14, 100] {
            let c = count_below(r, pos);
            assert!(c >= prev, "count_below({pos}) = {c} < prev = {prev}");
            prev = c;
        }
    }

    #[test]
    fn test_count_below_last_bucket() {
        let r = &[(0b111 << 61, 100)];

        // Before last bucket → 0
        let c = count_below(r, 0b110 << 61);
        assert!(c.abs() < 1e-10);

        // Inside last bucket, near the end
        let c = count_below(r, u64::MAX - 1);
        assert!(c > 0.0);
        assert!(c <= 100.0);
    }

    #[test]
    fn test_count_below_u64_max_excludes_terminal_endpoint_mass() {
        let mut h = Histogram::<()>::with_log_scale(16, 1);
        h.record_n(u64::MAX, 100);

        let c = h.interpolator().count_below(u64::MAX);
        assert!(c > 0.0);
        assert!(c < 100.0, "count_below(u64::MAX) should exclude the endpoint");
    }

    #[test]
    fn test_count_below_gap_between_buckets() {
        // Bucket 5: [5,6) c=10, Bucket 12: [16,20) c=40
        // Position 10 is in an empty bucket between them
        let c = count_below(&[(5, 10), (16, 40)], 10);
        assert!((c - 10.0).abs() < 1e-10);
    }
}
