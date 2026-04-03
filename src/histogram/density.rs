use super::histogram::Histogram;

/// A read-only computational view for density analysis over a histogram.
///
/// Provides derived calculations such as density slopes and trapezoidal
/// interpolation, without modifying the underlying histogram.
pub struct Density<'a, T = (), const WIDTH: usize = 3> {
    hist: &'a Histogram<T, WIDTH>,
}

impl<'a, T, const WIDTH: usize> Density<'a, T, WIDTH> {
    pub fn new(hist: &'a Histogram<T, WIDTH>) -> Self {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Record values and return the density slope at the given bucket.
    fn slope(records: &[(u64, u64)], bucket: usize) -> f64 {
        let mut hist = Histogram::<()>::new();
        for &(value, count) in records {
            hist.record_n(value, count);
        }
        Density::new(&hist).density_slope(bucket)
    }

    // --- Interior buckets with equal widths ---

    #[test]
    fn test_equal_width_increasing() {
        // Buckets 8,9,10: [8,10), [10,12), [12,14) — all width=2, midpoints 9,11,13
        // d0=10/2=5, d2=30/2=15 → k = (15-5)/(13-9) = 2.5
        assert!((slope(&[(8, 10), (10, 20), (12, 30)], 9) - 2.5).abs() < 1e-10);
    }

    #[test]
    fn test_equal_width_decreasing() {
        // d0=30/2=15, d2=10/2=5 → k = -2.5
        assert!((slope(&[(8, 30), (10, 20), (12, 10)], 9) - (-2.5)).abs() < 1e-10);
    }

    #[test]
    fn test_equal_counts_zero_slope() {
        // d0=20/2=10, d2=20/2=10 → k=0
        assert!(slope(&[(8, 20), (10, 20), (12, 20)], 9).abs() < 1e-10);
    }

    // --- Interior buckets with unequal widths ---

    #[test]
    fn test_unequal_widths_equal_density() {
        // Bucket 11: [14,16) w=2, Bucket 13: [20,24) w=4
        // c0=4, c2=8 → d0=2, d2=2 → k=0
        assert!(slope(&[(14, 4), (16, 1), (20, 8)], 12).abs() < 1e-10);
    }

    #[test]
    fn test_unequal_widths_nonzero_slope() {
        // Bucket 11: [14,16) w=2 mid=15, Bucket 13: [20,24) w=4 mid=22
        // c0=2, c2=8 → d0=1, d2=2 → k=(2-1)/(22-15)=1/7
        assert!((slope(&[(14, 2), (16, 1), (20, 8)], 12) - 1.0 / 7.0).abs() < 1e-10);
    }

    // --- Edge: first and last buckets ---

    #[test]
    fn test_first_bucket() {
        // Bucket 0: [0,1) w=1 mid=0, Bucket 1: [1,2) w=1 mid=1
        // Uses (self=0, right=1): k = (30-10)/(1-0) = 20
        assert!((slope(&[(0, 10), (1, 30)], 0) - 20.0).abs() < 1e-10);
    }

    #[test]
    fn test_last_bucket() {
        // Bucket 250: [0b110<<61, 0b111<<61) w=1<<61
        // Bucket 251: [0b111<<61, u64::MAX)  w=u64::MAX-(0b111<<61)
        // Uses (left=250, self=251)
        let k = slope(&[(0b110 << 61, 100), (0b111 << 61, 200)], 251);

        let w250: f64 = (1u128 << 61) as f64;
        let w251: f64 = (u64::MAX - (0b111 << 61)) as f64;
        let m250: f64 = (0b110u128 << 61) as f64 + w250 / 2.0;
        let m251: f64 = (0b111u128 << 61) as f64 + w251 / 2.0;
        let expected = (200.0 / w251 - 100.0 / w250) / (m251 - m250);
        assert!(
            (k - expected).abs() / expected.abs() < 1e-10,
            "k = {k}, expected = {expected}"
        );
    }

    // --- Near-edge interior buckets ---

    #[test]
    fn test_second_bucket() {
        // Buckets 0,1,2: [0,1), [1,2), [2,3) — all w=1, midpoints 0,1,2
        // Uses (left=0, right=2) → k=(30-10)/(2-0)=10
        assert!((slope(&[(0, 10), (1, 20), (2, 30)], 1) - 10.0).abs() < 1e-10);
    }

    #[test]
    fn test_second_to_last_bucket() {
        // Bucket 249: [0b101<<61, 0b110<<61) w=1<<61
        // Bucket 251: [0b111<<61, u64::MAX)  w=u64::MAX-(0b111<<61)
        // Interior bucket 250: uses (left=249, right=251)
        let k = slope(&[(0b101 << 61, 100), (0b111 << 61, 200)], 250);

        let w249: f64 = (1u128 << 61) as f64;
        let w251: f64 = (u64::MAX - (0b111 << 61)) as f64;
        let m249: f64 = (0b101u128 << 61) as f64 + w249 / 2.0;
        let m251: f64 = (0b111u128 << 61) as f64 + w251 / 2.0;
        let expected = (200.0 / w251 - 100.0 / w249) / (m251 - m249);
        assert!(
            (k - expected).abs() / expected.abs() < 1e-10,
            "k = {k}, expected = {expected}"
        );
    }

    // --- Zero-count cases ---

    #[test]
    fn test_zero_counts() {
        // All empty → k=0 for any bucket position
        assert!(slope(&[], 0).abs() < 1e-10);
        assert!(slope(&[], 9).abs() < 1e-10);
        assert!(slope(&[], 251).abs() < 1e-10);
    }

    #[test]
    fn test_one_empty_neighbor() {
        // Bucket 8: [8,10) c=0, Bucket 10: [12,14) c=20
        // d0=0, d2=20/2=10 → k = (10-0)/(13-9) = 2.5
        assert!((slope(&[(10, 5), (12, 20)], 9) - 2.5).abs() < 1e-10);
    }

    #[test]
    fn test_both_neighbors_empty() {
        // Only the target bucket has data, neighbors are 0 → k=0
        assert!(slope(&[(10, 50)], 9).abs() < 1e-10);
    }
}
