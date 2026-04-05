use super::histogram::Histogram;

/// Interpolates between discrete histogram buckets to produce continuous estimates.
///
/// Uses trapezoidal density estimation with neighbor bucket slopes
/// to compute partial-bucket counts and cumulative distributions.
#[derive(Clone, Copy)]
pub struct Interpolator<'a, T = ()> {
    pub(super) hist: &'a Histogram<T>,
}

impl<'a, T> Interpolator<'a, T> {
    pub fn new(hist: &'a Histogram<T>) -> Self {
        Self { hist }
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
        let last = self.hist.num_buckets() - 1;

        let (left, right) = if bucket == 0 {
            (bucket, bucket + 1)
        } else if bucket == last {
            (bucket - 1, bucket)
        } else {
            (bucket - 1, bucket + 1)
        };

        let bl = self.hist.bucket(left);
        let br = self.hist.bucket(right);

        let dl = bl.count() as f64 / bl.width() as f64;
        let dr = br.count() as f64 / br.width() as f64;

        (dr - dl) / (br.midpoint() as f64 - bl.midpoint() as f64)
    }

    /// Computes the estimated sample count in `[bucket.left(), bucket.left() + x)`
    /// using trapezoidal density estimation.
    ///
    /// `x` is the offset from the bucket's left boundary.
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
    /// `k` is the density slope from `density_slope()`,
    /// and `d1 = count / width` is the average density.
    ///
    /// The CDF is:
    ///
    /// ```text
    ///   C(x) = a·x + k·x²/2
    /// ```
    pub fn trapezoidal_cdf(&self, bucket: usize, x: u64) -> f64 {
        let b = self.hist.bucket(bucket);
        let w = b.width() as f64;
        let x = x as f64;

        let d1 = b.count() as f64 / w;
        let mut k = self.density_slope(bucket);

        if d1 + k * w / 2.0 < 0.0 {
            // Density at right edge would be negative → adjust slope
            k = -d1 / (w / 2.0);
        }
        if d1 - k * w / 2.0 < 0.0 {
            k = d1 / (w / 2.0); // Adjust slope to maintain average density
        }

        // d(x) = a + k·x, where a = d1 - k·w/2 is density at left edge
        let a = d1 - k * w / 2.0;
        let b = d1 + k * w / 2.0; // Density at right edge, for debugging
        println!("bucket {bucket}: width={w} d1={d1}, k={k}, a={a} b={b}");
        a * x + k * x * x / 2.0
    }

    /// Returns the estimated count of samples in `[0, position)`,
    /// i.e., strictly below `position`, across all buckets.
    ///
    /// Sums the full count of every bucket entirely before `position`,
    /// then adds the partial count within the bucket containing `position`
    /// using `trapezoidal_cdf`.
    pub fn count_below(&self, position: u64) -> f64 {
        let mut total = 0.0;

        for i in 0..self.hist.num_buckets() {
            let b = self.hist.bucket(i);

            if position >= b.right() {
                // Entire bucket is before position
                total += b.count() as f64;
            } else {
                // Position falls within this bucket
                total += self.trapezoidal_cdf(i, position - b.left());
                break;
            }
        }

        total
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

    fn slope(records: &[(u64, u64)], bucket: usize) -> f64 {
        let h = make_hist(records);
        Interpolator::new(&h).density_slope(bucket)
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
        let w251: f64 = (u64::MAX - (0b111 << 61)) as f64;
        let m250: f64 = (0b110u128 << 61) as f64 + w250 / 2.0;
        let m251: f64 = (0b111u128 << 61) as f64 + w251 / 2.0;

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

    // === trapezoidal_cdf tests ===

    fn cdf(records: &[(u64, u64)], bucket: usize, x: u64) -> f64 {
        let h = make_hist(records);
        Interpolator::new(&h).trapezoidal_cdf(bucket, x)
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
        // Bucket 251: [0b111<<61, u64::MAX)
        // Uses (left=250, self=251) for slope
        let r = &[(0b110 << 61, 100), (0b111 << 61, 200)];

        let c = cdf(r, 251, 0);
        assert!(c.abs() < 1e-10);

        let w251 = u64::MAX - (0b111 << 61);
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

    // === count_below tests ===

    fn count_below(records: &[(u64, u64)], position: u64) -> f64 {
        let h = make_hist(records);
        Interpolator::new(&h).count_below(position)
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
        let c = Interpolator::new(&h).count_below(u64::MAX);
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
    fn test_count_below_gap_between_buckets() {
        // Bucket 5: [5,6) c=10, Bucket 12: [16,20) c=40
        // Position 10 is in an empty bucket between them
        let c = count_below(&[(5, 10), (16, 40)], 10);
        assert!((c - 10.0).abs() < 1e-10);
    }
}
