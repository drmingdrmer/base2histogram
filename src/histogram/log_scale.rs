use std::sync::LazyLock;

use super::log_scale_config::LogScaleConfig;

/// Logarithmic scale with precomputed lookup tables.
///
/// Handles value-to-bucket mapping:
/// - Value → bucket index (with small-value cache)
/// - Bucket index → left boundary value
///
/// Use the shared [`LOG_SCALE`] instance for WIDTH=3 (default configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogScale<const WIDTH: usize> {
    /// Left boundary value for each bucket index.
    bucket_min_values: Vec<u64>,
    /// Cached bucket indices for small values (0-4095).
    small_value_buckets: Vec<u8>,
}

impl<const WIDTH: usize> LogScale<WIDTH> {
    /// Creates a new LogScale for the given WIDTH configuration.
    pub fn new() -> Self {
        // Build bucket_min_values table
        let bucket_min_values: Vec<u64> =
            (0..LogScaleConfig::<WIDTH>::BUCKETS).map(Self::compute_bucket_min_value).collect();

        // Build small_value_buckets cache.
        // Stop when the bucket index no longer fits in u8 to avoid truncation.
        let mut small_value_buckets = Vec::with_capacity(LogScaleConfig::<WIDTH>::SMALL_VALUE_CACHE_SIZE);
        for v in 0..LogScaleConfig::<WIDTH>::SMALL_VALUE_CACHE_SIZE {
            let bucket = Self::calculate_bucket_uncached(v as u64);
            let Ok(bucket) = u8::try_from(bucket) else {
                break;
            };
            small_value_buckets.push(bucket);
        }

        Self {
            bucket_min_values,
            small_value_buckets,
        }
    }

    /// Returns the number of buckets for this `WIDTH` (compile-time constant).
    #[inline]
    pub const fn total_buckets() -> usize {
        LogScaleConfig::<WIDTH>::BUCKETS
    }

    /// Returns the number of buckets.
    #[inline]
    pub const fn num_buckets(&self) -> usize {
        Self::total_buckets()
    }

    /// Returns the left boundary value for the given bucket index.
    #[inline]
    pub fn bucket_left(&self, bucket: usize) -> u64 {
        self.bucket_min_values[bucket]
    }

    /// Returns the right open boundary value for the given bucket index.
    ///
    /// For the last bucket, returns `u64::MAX` since it overflows.
    /// Thus, there is an inaccuracy
    #[inline]
    pub fn bucket_right(&self, bucket: usize) -> u64 {
        if bucket + 1 < self.bucket_min_values.len() {
            self.bucket_min_values[bucket + 1]
        } else {
            u64::MAX
        }
    }

    /// Interpolates within a bucket using trapezoidal density estimation.
    ///
    /// Uses neighbor bucket densities to estimate a linear density gradient
    /// across the target bucket, then solves for the position corresponding
    /// to the given rank. Falls back to uniform interpolation for edge buckets
    /// (first/last) or when count <= 1.
    #[inline]
    pub fn interpolate(&self, bucket: usize, rank: u64, count: u64, c0: u64, c2: u64) -> u64 {
        let left = self.bucket_min_values[bucket];
        let right = self.bucket_right(bucket);
        let range = (right - left) as f64;

        if count <= 1 || range == 1.0 {
            return left + (right - left) / 2;
        }

        let f = rank as f64 / count as f64;

        // Edge buckets have no prev/next neighbor; use uniform interpolation
        let t = if bucket < 1 || bucket + 1 >= self.bucket_min_values.len() {
            f
        } else {
            self.trapezoidal_t(bucket, f, count, c0, c2)
        };

        (left + (range * t.clamp(0.0, 1.0)) as u64).min(right - 1)
    }

    /// Computes the density slope `k` across three adjacent buckets.
    ///
    /// Given bucket counts `c0, c1, c2` and boundaries `x0..x3`, computes the
    /// rate of change in density (count/width) per unit of x between the
    /// midpoints of bucket 0 and bucket 2:
    ///
    /// ```text
    ///   k = (d2 - d0) / (m2 - m0)
    /// ```
    ///
    /// where `d = count / width` and `m = midpoint`.
    /// Computes the density slope `k` across a bucket and its two neighbors.
    ///
    /// `bucket` must have both a previous and next neighbor (i.e., not the
    /// first or last bucket).
    fn density_slope(&self, bucket: usize, count0: u64, count2: u64) -> f64 {
        let x0 = self.bucket_min_values[bucket - 1] as f64;
        let x1 = self.bucket_min_values[bucket] as f64;
        let x2 = self.bucket_min_values[bucket + 1] as f64;
        let x3 = self.bucket_right(bucket + 1) as f64;

        let width0 = x1 - x0;
        let width2 = x3 - x2;

        let density0 = count0 as f64 / width0;
        let density2 = count2 as f64 / width2;

        let m0 = (x0 + x1) / 2.0;
        let m2 = (x2 + x3) / 2.0;

        (density2 - density0) / (m2 - m0)
    }

    /// Computes the trapezoidal interpolation parameter t for a fractional rank f.
    ///
    /// Requires that bucket has both a previous and next neighbor.
    /// If not, the caller should use uniform interpolation (t = f) instead.
    ///
    /// Models density as linear across the bucket with slope k from neighbors,
    /// anchored at the bucket's known density d1 = c1/w1 at midpoint m1:
    /// ```text
    ///   bucket:  [i-1]      [i]        [i+1]
    ///   left:     x0         x1         x2
    ///   width:  |----w0----|----w1----|----w2----|
    ///              d0         d1          d2      (density)
    ///              m0         m1          m2      (midpoint)
    /// ```
    fn trapezoidal_t(&self, bucket: usize, f: f64, c1: u64, c0: u64, c2: u64) -> f64 {
        // Equal neighbor counts: density is uniform, CDF is linear
        if c0 == c2 {
            return f;
        }

        let x1 = self.bucket_min_values[bucket] as f64;
        let x2 = self.bucket_min_values[bucket + 1] as f64;

        let w1 = x2 - x1;
        let d1 = c1 as f64 / w1;

        let k = self.density_slope(bucket, c0, c2);

        // Density across bucket: d(t) = d1 + s·(t − 0.5), t ∈ [0, 1]
        //   where s = k·w1 (total density change across bucket)
        //
        // CDF: C(t) = (d1 − s/2)·t + s·t²/2
        // C(1) = d1
        //
        // Solve C(t) = f·d1:
        //   s/2·t² + (d1 − s/2)·t − f·d1 = 0
        //
        // Let a = d1 − s/2 (density at left edge):
        //   t = (−a + √(a² + 2·s·f·d1)) / s
        let s = k * w1;

        // Near-uniform density: slope across bucket is negligible
        if s.abs() < d1.abs() * 1e-9 {
            return f;
        }

        let a = d1 - s / 2.0;
        let disc = a * a + 2.0 * s * f * d1;
        if disc < 0.0 {
            return f;
        }

        (-a + disc.sqrt()) / s
    }

    /// Calculates bucket index for a value, using cache for small values.
    #[inline]
    pub fn calculate_bucket(&self, value: u64) -> usize {
        if value < self.small_value_buckets.len() as u64 {
            return self.small_value_buckets[value as usize] as usize;
        }
        Self::calculate_bucket_uncached(value)
    }

    /// Calculates the bucket index for a given value using logarithmic bucketing.
    ///
    /// Algorithm:
    /// 1. For value < GROUP_SIZE: bucket_index = value
    /// 2. For value >= GROUP_SIZE:
    ///    - Find the position of the most significant bit (MSB)
    ///    - Determine which group of GROUP_SIZE buckets
    ///    - Extract offset within that group using the bits after MSB
    ///    - Bucket index = base of this group + offset within group
    pub fn calculate_bucket_uncached(value: u64) -> usize {
        let g_size = LogScaleConfig::<WIDTH>::GROUP_SIZE;

        if value < g_size as u64 {
            return value as usize;
        }
        // 000...00 1xxxx 0000...0000
        //          ----- -----------
        //          Group group_index
        //          -----------------
        //          bits_upto_msb
        //
        // xxxx: offset_in_group
        let bits_upto_msb = (u64::BITS - value.leading_zeros()) as usize;
        let group_index = bits_upto_msb - LogScaleConfig::<WIDTH>::WIDTH;
        let offset_in_group = ((value >> group_index) & LogScaleConfig::<WIDTH>::MASK) as usize;

        g_size + group_index * g_size + offset_in_group
    }

    /// Computes the left boundary value for a bucket index.
    fn compute_bucket_min_value(bucket: usize) -> u64 {
        let g_size = LogScaleConfig::<WIDTH>::GROUP_SIZE;

        if bucket < g_size {
            return bucket as u64;
        }
        let group_index = (bucket - g_size) / g_size;
        let offset_in_group = (bucket - g_size) % g_size;
        ((offset_in_group | LogScaleConfig::<WIDTH>::GROUP_MSB_BIT) << group_index) as u64
    }
}

impl<const WIDTH: usize> Default for LogScale<WIDTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// Default log scale with WIDTH=3 (252 buckets, ~12.5% max error).
pub type LogScale3 = LogScale<3>;

/// Shared LogScale instance for WIDTH=3 (default configuration).
pub static LOG_SCALE: LazyLock<LogScale3> = LazyLock::new(LogScale3::new);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_buckets_is_const() {
        // total_buckets() is const fn without &self, usable for array sizing at compile time.
        let buckets: [u64; LogScale3::total_buckets()] = [0; LogScale3::total_buckets()];
        assert_eq!(buckets.len(), 252);
        assert_eq!(LOG_SCALE.num_buckets(), 252);
    }

    #[test]
    fn test_calculate_bucket_group_0() {
        assert_eq!(LogScale3::calculate_bucket_uncached(0), 0);
        assert_eq!(LogScale3::calculate_bucket_uncached(1), 1);
        assert_eq!(LogScale3::calculate_bucket_uncached(2), 2);
        assert_eq!(LogScale3::calculate_bucket_uncached(3), 3);
    }

    #[test]
    fn test_calculate_bucket_group_1() {
        assert_eq!(LogScale3::calculate_bucket_uncached(4), 4);
        assert_eq!(LogScale3::calculate_bucket_uncached(5), 5);
        assert_eq!(LogScale3::calculate_bucket_uncached(6), 6);
        assert_eq!(LogScale3::calculate_bucket_uncached(7), 7);
    }

    #[test]
    fn test_calculate_bucket_group_2() {
        assert_eq!(LogScale3::calculate_bucket_uncached(8), 8);
        assert_eq!(LogScale3::calculate_bucket_uncached(10), 9);
        assert_eq!(LogScale3::calculate_bucket_uncached(12), 10);
        assert_eq!(LogScale3::calculate_bucket_uncached(14), 11);
    }

    #[test]
    fn test_calculate_bucket_group_3() {
        assert_eq!(LogScale3::calculate_bucket_uncached(16), 12);
        assert_eq!(LogScale3::calculate_bucket_uncached(20), 13);
        assert_eq!(LogScale3::calculate_bucket_uncached(24), 14);
        assert_eq!(LogScale3::calculate_bucket_uncached(28), 15);
    }

    #[test]
    fn test_calculate_bucket_group_4() {
        assert_eq!(LogScale3::calculate_bucket_uncached(32), 16);
        assert_eq!(LogScale3::calculate_bucket_uncached(40), 17);
        assert_eq!(LogScale3::calculate_bucket_uncached(48), 18);
        assert_eq!(LogScale3::calculate_bucket_uncached(56), 19);
    }

    #[test]
    fn test_reasonable_bucket_ranges() {
        assert_eq!(LogScale3::calculate_bucket_uncached(1024), 36);
        assert_eq!(LogScale3::calculate_bucket_uncached(2048), 40);
        assert_eq!(LogScale3::calculate_bucket_uncached(4096), 44);

        let million = 1_048_576;
        let million_bucket = LogScale3::calculate_bucket_uncached(million);
        assert!(million_bucket < 80);

        let billion = 1_073_741_824;
        let billion_bucket = LogScale3::calculate_bucket_uncached(billion);
        assert!(billion_bucket < 120);
    }

    #[test]
    fn test_bucket_min_values_lookup_table() {
        // Group 0: [0, 1, 2, 3]
        assert_eq!(LOG_SCALE.bucket_left(0), 0);
        assert_eq!(LOG_SCALE.bucket_left(1), 1);
        assert_eq!(LOG_SCALE.bucket_left(2), 2);
        assert_eq!(LOG_SCALE.bucket_left(3), 3);

        // Group 1: [4, 5, 6, 7]
        assert_eq!(LOG_SCALE.bucket_left(4), 4);
        assert_eq!(LOG_SCALE.bucket_left(5), 5);
        assert_eq!(LOG_SCALE.bucket_left(6), 6);
        assert_eq!(LOG_SCALE.bucket_left(7), 7);

        // Group 2: [8, 10, 12, 14]
        assert_eq!(LOG_SCALE.bucket_left(8), 8);
        assert_eq!(LOG_SCALE.bucket_left(9), 10);
        assert_eq!(LOG_SCALE.bucket_left(10), 12);
        assert_eq!(LOG_SCALE.bucket_left(11), 14);

        // Group 3: [16, 20, 24, 28]
        assert_eq!(LOG_SCALE.bucket_left(12), 16);
        assert_eq!(LOG_SCALE.bucket_left(13), 20);
        assert_eq!(LOG_SCALE.bucket_left(14), 24);
        assert_eq!(LOG_SCALE.bucket_left(15), 28);
    }

    #[test]
    fn test_cached_bucket_matches_uncached() {
        // Sample values across cache range to verify cache correctness
        let test_values: Vec<usize> =
            (0..100).chain((100..1000).step_by(10)).chain((1000..4096).step_by(100)).collect();

        for v in test_values {
            let cached = LOG_SCALE.calculate_bucket(v as u64);
            let uncached = LogScale3::calculate_bucket_uncached(v as u64);
            assert_eq!(cached, uncached, "Mismatch at value {v}");
        }
    }

    #[test]
    fn test_cached_bucket_boundary() {
        // Test at cache boundary
        let last_cached = (LogScaleConfig::<3>::SMALL_VALUE_CACHE_SIZE - 1) as u64;
        let first_uncached = LogScaleConfig::<3>::SMALL_VALUE_CACHE_SIZE as u64;

        assert_eq!(
            LOG_SCALE.calculate_bucket(last_cached),
            LogScale3::calculate_bucket_uncached(last_cached)
        );
        assert_eq!(
            LOG_SCALE.calculate_bucket(first_uncached),
            LogScale3::calculate_bucket_uncached(first_uncached)
        );
    }

    #[test]
    fn test_cached_bucket_large_values() {
        // Values beyond cache should still work correctly
        let large_values = [4096, 10000, 100000, 1_000_000, u64::MAX];
        for &v in &large_values {
            assert_eq!(
                LOG_SCALE.calculate_bucket(v),
                LogScale3::calculate_bucket_uncached(v),
                "Mismatch at value {v}"
            );
        }
    }

    #[test]
    fn test_density_slope() {
        // WIDTH=3 bucket layout:
        //   bucket 8: [8,10)   width=2
        //   bucket 9: [10,12)  width=2
        //   bucket 10: [12,14) width=2
        //
        // Equal widths, so density = count / 2, midpoints = 9, 11, 13
        // c0=10, c2=30 → d0=5, d2=15 → k = (15-5)/(13-9) = 2.5
        let k = LOG_SCALE.density_slope(9, 10, 30);
        assert!((k - 2.5).abs() < 1e-10, "k = {k}");

        // Decreasing: c0=30, c2=10 → d0=15, d2=5 → k = -2.5
        let k = LOG_SCALE.density_slope(9, 30, 10);
        assert!((k - (-2.5)).abs() < 1e-10, "k = {k}");

        // Equal neighbor counts → k = 0
        let k = LOG_SCALE.density_slope(9, 20, 20);
        assert!(k.abs() < 1e-10, "k = {k}");

        // Unequal widths:
        //   bucket 11: [14,16) width=2
        //   bucket 12: [16,20) width=4
        //   bucket 13: [20,24) width=4
        //
        // c0=4, c2=8 → d0=4/2=2, d2=8/4=2 → equal density → k=0
        let k = LOG_SCALE.density_slope(12, 4, 8);
        assert!(k.abs() < 1e-10, "k = {k}");

        // c0=2, c2=8 → d0=2/2=1, d2=8/4=2
        // m0=(14+16)/2=15, m2=(20+24)/2=22 → k=(2-1)/(22-15)=1/7
        let k = LOG_SCALE.density_slope(12, 2, 8);
        assert!((k - 1.0 / 7.0).abs() < 1e-10, "k = {k}");
    }

    #[test]
    fn test_new_reduces_small_value_cache_when_bucket_exceeds_u8() {
        let log_scale = LogScale::<7>::new();

        assert_eq!(log_scale.small_value_buckets.len(), 512);
        assert_eq!(
            log_scale.calculate_bucket(511),
            LogScale::<7>::calculate_bucket_uncached(511)
        );
        assert_eq!(
            log_scale.calculate_bucket(512),
            LogScale::<7>::calculate_bucket_uncached(512)
        );
    }
}
