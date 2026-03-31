use std::sync::LazyLock;

use super::log_scale_config::LogScaleConfig;

/// Logarithmic scale with precomputed lookup tables.
///
/// Handles value-to-bucket mapping:
/// - Value → bucket index (with small-value cache)
/// - Bucket index → minimum value
///
/// Use the shared [`LOG_SCALE`] instance for WIDTH=3 (default configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogScale<const WIDTH: usize> {
    /// Minimum value represented by each bucket index.
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

    /// Returns the number of buckets.
    #[inline]
    pub fn num_buckets(&self) -> usize {
        self.bucket_min_values.len()
    }

    /// Returns the minimum value for the given bucket index.
    #[inline]
    pub fn bucket_min_value(&self, bucket: usize) -> u64 {
        self.bucket_min_values[bucket]
    }

    /// Returns the maximum value for the given bucket index.
    #[inline]
    pub fn bucket_max_value(&self, bucket: usize) -> u64 {
        if bucket + 1 < self.bucket_min_values.len() {
            self.bucket_min_values[bucket + 1] - 1
        } else {
            u64::MAX
        }
    }

    /// Interpolates within a bucket using trapezoidal density estimation.
    ///
    /// Uses neighbor bucket densities to estimate a linear density gradient
    /// across the target bucket, then solves for the position corresponding
    /// to the given rank. Falls back to uniform interpolation when neighbors
    /// provide no gradient information (both empty or at histogram edges).
    #[inline]
    pub fn interpolate(&self, bucket: usize, rank: u64, count: u64, prev_count: u64, next_count: u64) -> u64 {
        let min_val = self.bucket_min_values[bucket];
        let max_val = self.bucket_max_value(bucket);
        let range = (max_val - min_val) as f64;

        if count <= 1 || range == 0.0 {
            return min_val + (max_val - min_val) / 2;
        }

        let f = (rank - 1) as f64 / (count - 1) as f64;

        // Need both neighbors for trapezoidal interpolation
        if bucket == 0 || bucket + 1 >= self.bucket_min_values.len() {
            return min_val + (range * f) as u64;
        }

        let prev_min = self.bucket_min_values[bucket - 1];
        let prev_width = (min_val - prev_min) as f64;
        let next_min = self.bucket_min_values[bucket + 1];
        let next_max = self.bucket_max_value(bucket + 1);
        let next_width = (next_max - next_min + 1) as f64;

        let d_prev = prev_count as f64 / prev_width;
        let d_next = next_count as f64 / next_width;

        if d_prev == 0.0 && d_next == 0.0 {
            return min_val + (range * f) as u64;
        }

        // Interpolate density at target bucket edges using neighbor midpoints
        let m_prev = (prev_min as f64 + (min_val - 1) as f64) / 2.0;
        let m_next = (next_min as f64 + next_max as f64) / 2.0;
        let span = m_next - m_prev;

        let d_left = (d_prev + (d_next - d_prev) * (min_val as f64 - m_prev) / span).max(0.0);
        let d_right = (d_prev + (d_next - d_prev) * (max_val as f64 - m_prev) / span).max(0.0);

        // Trapezoidal CDF: C(t) = a*t + b*t²/2, where a = d_left, b = d_right - d_left
        // Solve C(t) / C(1) = f for t ∈ [0, 1]
        let a = d_left;
        let b = d_right - d_left;

        let t = if b.abs() < a.abs() * 1e-9 {
            f
        } else {
            let target_area = f * (a + b / 2.0);
            let discriminant = a * a + 2.0 * b * target_area;
            if discriminant < 0.0 {
                f
            } else {
                (-a + discriminant.sqrt()) / b
            }
        };

        min_val + (range * t.clamp(0.0, 1.0)) as u64
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

    /// Computes the minimum value for a bucket index.
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
        assert_eq!(LOG_SCALE.bucket_min_value(0), 0);
        assert_eq!(LOG_SCALE.bucket_min_value(1), 1);
        assert_eq!(LOG_SCALE.bucket_min_value(2), 2);
        assert_eq!(LOG_SCALE.bucket_min_value(3), 3);

        // Group 1: [4, 5, 6, 7]
        assert_eq!(LOG_SCALE.bucket_min_value(4), 4);
        assert_eq!(LOG_SCALE.bucket_min_value(5), 5);
        assert_eq!(LOG_SCALE.bucket_min_value(6), 6);
        assert_eq!(LOG_SCALE.bucket_min_value(7), 7);

        // Group 2: [8, 10, 12, 14]
        assert_eq!(LOG_SCALE.bucket_min_value(8), 8);
        assert_eq!(LOG_SCALE.bucket_min_value(9), 10);
        assert_eq!(LOG_SCALE.bucket_min_value(10), 12);
        assert_eq!(LOG_SCALE.bucket_min_value(11), 14);

        // Group 3: [16, 20, 24, 28]
        assert_eq!(LOG_SCALE.bucket_min_value(12), 16);
        assert_eq!(LOG_SCALE.bucket_min_value(13), 20);
        assert_eq!(LOG_SCALE.bucket_min_value(14), 24);
        assert_eq!(LOG_SCALE.bucket_min_value(15), 28);
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
