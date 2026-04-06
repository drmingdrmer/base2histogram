use std::sync::LazyLock;

use super::bucket_span::BucketSpan;
use super::log_scale_config::LogScaleConfig;

/// Logarithmic scale with precomputed lookup tables.
///
/// Handles value-to-bucket mapping:
/// - Value → bucket index (with small-value cache)
/// - Bucket index → left boundary value
///
/// Use [`LogScale::get`] to obtain a shared instance for a given width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogScale {
    config: LogScaleConfig,
    /// Left boundary value for each bucket index.
    pub(crate) bucket_min_values: Vec<u64>,
    /// Cached bucket indices for small values (0-4095).
    small_value_buckets: Vec<u8>,
}

static LOG_SCALES: LazyLock<Vec<LogScale>> =
    LazyLock::new(|| (LogScaleConfig::MIN_WIDTH..=LogScaleConfig::MAX_WIDTH).map(LogScale::new).collect());

impl LogScale {
    pub const DEFAULT_WIDTH: usize = 3;

    /// Returns a shared `LogScale` instance for the given width (1..=16).
    pub fn get(width: usize) -> &'static LogScale {
        LogScaleConfig::validate_width(width);
        &LOG_SCALES[width - 1]
    }

    /// Creates a new `LogScale` for the given width configuration.
    ///
    /// `width` must be in `1..=16`.
    pub fn new(width: usize) -> Self {
        let mut s = Self {
            config: LogScaleConfig::new(width),
            bucket_min_values: Vec::new(),
            small_value_buckets: Vec::new(),
        };

        s.bucket_min_values = (0..s.config.buckets()).map(|i| s.compute_bucket_min_value(i)).collect();

        // Build small_value_buckets cache.
        // Stop when the bucket index no longer fits in u8 to avoid truncation.
        s.small_value_buckets = Vec::with_capacity(s.config.small_value_cache_size());
        for v in 0..s.config.small_value_cache_size() {
            let bucket = s.calculate_bucket_uncached(v as u64);
            let Ok(bucket) = u8::try_from(bucket) else {
                break;
            };
            s.small_value_buckets.push(bucket);
        }

        s
    }

    /// Returns the configuration for this log scale.
    pub fn config(&self) -> &LogScaleConfig {
        &self.config
    }

    /// Returns the number of buckets.
    #[inline]
    pub fn num_buckets(&self) -> usize {
        self.config.buckets()
    }

    /// Returns a reference to the bucket geometry at the given index.
    pub fn bucket_span(&self, index: usize) -> BucketSpan<'_> {
        BucketSpan::new(self, index)
    }

    /// Calculates bucket index for a value, using cache for small values.
    #[inline]
    pub fn calculate_bucket(&self, value: u64) -> usize {
        if value < self.small_value_buckets.len() as u64 {
            return self.small_value_buckets[value as usize] as usize;
        }
        self.calculate_bucket_uncached(value)
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
    pub fn calculate_bucket_uncached(&self, value: u64) -> usize {
        let g_size = self.config.group_size();

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
        let group_index = bits_upto_msb - self.config.width();
        let offset_in_group = ((value >> group_index) & self.config.mask()) as usize;

        g_size + group_index * g_size + offset_in_group
    }

    /// Computes the left boundary value for a bucket index.
    fn compute_bucket_min_value(&self, bucket: usize) -> u64 {
        let g_size = self.config.group_size();

        if bucket < g_size {
            return bucket as u64;
        }
        let group_index = (bucket - g_size) / g_size;
        let offset_in_group = (bucket - g_size) % g_size;
        ((offset_in_group | g_size) << group_index) as u64
    }
}

impl Default for LogScale {
    fn default() -> Self {
        Self::new(Self::DEFAULT_WIDTH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let scale = LogScale::default();
        assert_eq!(
            scale.num_buckets(),
            LogScale::get(LogScale::DEFAULT_WIDTH).num_buckets()
        );
    }

    #[test]
    #[should_panic(expected = "width must be 1..=16, got 0")]
    fn test_get_rejects_zero_width() {
        let _ = LogScale::get(0);
    }

    #[test]
    #[should_panic(expected = "width must be 1..=16, got 17")]
    fn test_new_rejects_width_above_max() {
        let _ = LogScale::new(17);
    }

    #[test]
    fn test_num_buckets() {
        assert_eq!(LogScale::get(3).num_buckets(), 252);
    }

    #[test]
    fn test_calculate_bucket_group_0() {
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(0), 0);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(1), 1);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(2), 2);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(3), 3);
    }

    #[test]
    fn test_calculate_bucket_group_1() {
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(4), 4);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(5), 5);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(6), 6);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(7), 7);
    }

    #[test]
    fn test_calculate_bucket_group_2() {
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(8), 8);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(10), 9);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(12), 10);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(14), 11);
    }

    #[test]
    fn test_calculate_bucket_group_3() {
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(16), 12);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(20), 13);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(24), 14);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(28), 15);
    }

    #[test]
    fn test_calculate_bucket_group_4() {
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(32), 16);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(40), 17);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(48), 18);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(56), 19);
    }

    #[test]
    fn test_reasonable_bucket_ranges() {
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(1024), 36);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(2048), 40);
        assert_eq!(LogScale::get(3).calculate_bucket_uncached(4096), 44);

        let million = 1_048_576;
        let million_bucket = LogScale::get(3).calculate_bucket_uncached(million);
        assert!(million_bucket < 80);

        let billion = 1_073_741_824;
        let billion_bucket = LogScale::get(3).calculate_bucket_uncached(billion);
        assert!(billion_bucket < 120);
    }

    #[test]
    fn test_bucket_min_values_lookup_table() {
        let s = LogScale::get(3);

        // Group 0: [0, 1, 2, 3]
        assert_eq!(s.bucket_span(0).left(), 0);
        assert_eq!(s.bucket_span(1).left(), 1);
        assert_eq!(s.bucket_span(2).left(), 2);
        assert_eq!(s.bucket_span(3).left(), 3);

        // Group 1: [4, 5, 6, 7]
        assert_eq!(s.bucket_span(4).left(), 4);
        assert_eq!(s.bucket_span(5).left(), 5);
        assert_eq!(s.bucket_span(6).left(), 6);
        assert_eq!(s.bucket_span(7).left(), 7);

        // Group 2: [8, 10, 12, 14]
        assert_eq!(s.bucket_span(8).left(), 8);
        assert_eq!(s.bucket_span(9).left(), 10);
        assert_eq!(s.bucket_span(10).left(), 12);
        assert_eq!(s.bucket_span(11).left(), 14);

        // Group 3: [16, 20, 24, 28]
        assert_eq!(s.bucket_span(12).left(), 16);
        assert_eq!(s.bucket_span(13).left(), 20);
        assert_eq!(s.bucket_span(14).left(), 24);
        assert_eq!(s.bucket_span(15).left(), 28);
    }

    #[test]
    fn test_bucket_width() {
        let s = LogScale::get(3);

        // Group 0: single-value buckets, width=1
        assert_eq!(s.bucket_span(0).width(), 1); // [0,1)
        assert_eq!(s.bucket_span(1).width(), 1); // [1,2)
        assert_eq!(s.bucket_span(3).width(), 1); // [3,4)

        // Group 1: still width=1
        assert_eq!(s.bucket_span(4).width(), 1); // [4,5)
        assert_eq!(s.bucket_span(7).width(), 1); // [7,8)

        // Group 2: width=2
        assert_eq!(s.bucket_span(8).width(), 2); // [8,10)
        assert_eq!(s.bucket_span(11).width(), 2); // [14,16)

        // Group 3: width=4
        assert_eq!(s.bucket_span(12).width(), 4); // [16,20)
        assert_eq!(s.bucket_span(15).width(), 4); // [28,32)

        // Group 4: width=8
        assert_eq!(s.bucket_span(16).width(), 8); // [32,40)

        // Last bucket (251): [0b111<<61, u64::MAX]
        assert_eq!(s.bucket_span(251).width(), u64::MAX - (0b111 << 61) + 1);

        // Second-to-last bucket (250): step=2^61
        assert_eq!(s.bucket_span(250).width(), 1 << 61);
    }

    #[test]
    fn test_bucket_span() {
        // Group 0: [0,1)
        let b = LogScale::get(3).bucket_span(0);
        assert_eq!(b.index(), 0);
        assert_eq!(b.left(), 0);
        assert_eq!(b.right(), 1);
        assert_eq!(b.width(), 1);
        assert_eq!(b.midpoint(), 0);

        // Group 2: [8,10)
        let b = LogScale::get(3).bucket_span(8);
        assert_eq!(b.index(), 8);
        assert_eq!(b.left(), 8);
        assert_eq!(b.right(), 10);
        assert_eq!(b.width(), 2);
        assert_eq!(b.midpoint(), 9);

        // Group 3: [16,20)
        let b = LogScale::get(3).bucket_span(12);
        assert_eq!(b.index(), 12);
        assert_eq!(b.left(), 16);
        assert_eq!(b.right(), 20);
        assert_eq!(b.width(), 4);
        assert_eq!(b.midpoint(), 18);

        // Last bucket (251)
        let b = LogScale::get(3).bucket_span(251);
        assert_eq!(b.left(), 0b111 << 61);
        assert_eq!(b.right(), u64::MAX);
        assert!(b.is_last());
        assert_eq!(b.width(), u64::MAX - (0b111 << 61) + 1);
    }

    #[test]
    fn test_cached_bucket_matches_uncached() {
        // Sample values across cache range to verify cache correctness
        let test_values: Vec<usize> =
            (0..100).chain((100..1000).step_by(10)).chain((1000..4096).step_by(100)).collect();

        for v in test_values {
            let cached = LogScale::get(3).calculate_bucket(v as u64);
            let uncached = LogScale::get(3).calculate_bucket_uncached(v as u64);
            assert_eq!(cached, uncached, "Mismatch at value {v}");
        }
    }

    #[test]
    fn test_cached_bucket_boundary() {
        // Test at cache boundary
        let last_cached = (LogScale::get(3).config().small_value_cache_size() - 1) as u64;
        let first_uncached = LogScale::get(3).config().small_value_cache_size() as u64;

        assert_eq!(
            LogScale::get(3).calculate_bucket(last_cached),
            LogScale::get(3).calculate_bucket_uncached(last_cached)
        );
        assert_eq!(
            LogScale::get(3).calculate_bucket(first_uncached),
            LogScale::get(3).calculate_bucket_uncached(first_uncached)
        );
    }

    #[test]
    fn test_cached_bucket_large_values() {
        // Values beyond cache should still work correctly
        let large_values = [4096, 10000, 100000, 1_000_000, u64::MAX];
        for &v in &large_values {
            assert_eq!(
                LogScale::get(3).calculate_bucket(v),
                LogScale::get(3).calculate_bucket_uncached(v),
                "Mismatch at value {v}"
            );
        }
    }

    #[test]
    fn test_new_reduces_small_value_cache_when_bucket_exceeds_u8() {
        let log_scale = LogScale::new(7);

        assert_eq!(log_scale.small_value_buckets.len(), 512);
        assert_eq!(
            log_scale.calculate_bucket(511),
            log_scale.calculate_bucket_uncached(511)
        );
        assert_eq!(
            log_scale.calculate_bucket(512),
            log_scale.calculate_bucket_uncached(512)
        );
    }
}
