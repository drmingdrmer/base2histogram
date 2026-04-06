/// Configuration for logarithmic bucket boundaries.
///
/// The `width` parameter determines bucket granularity:
/// - width=3: 4 buckets per group, 252 total buckets, ~12.5% max error
/// - width=4: 8 buckets per group, 504 total buckets, ~6.25% max error
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogScaleConfig {
    /// The width of the bit pattern used for bucketing (most significant bits).
    ///
    /// Each bucket group uses WIDTH bits: 1 MSB + (WIDTH-1) offset bits.
    width: usize,

    /// Number of buckets per group: `1 << (width - 1)`.
    ///
    /// Also serves as the MSB bit pattern for bucket groups.
    /// For width=3: `1 << 2 = 4` buckets per group.
    group_size: usize,

    /// Mask for extracting the offset within a bucket group.
    ///
    /// Extracts the (width-1) bits after the MSB: `group_size - 1 = 0b11` for width=3.
    mask: u64,

    /// The exact number of buckets needed to cover all u64 values with logarithmic precision.
    ///
    /// Calculated as: `group_size * (66 - width)`
    /// For width=3: 4 * (66 - 3) = 4 * 63 = 252
    buckets: usize,

    /// Cache size for small value bucket lookups.
    ///
    /// Values 0-4095 map to bucket indices 0-44, fitting in u8.
    small_value_cache_size: usize,
}

impl LogScaleConfig {
    pub const MIN_WIDTH: usize = 1;
    pub const MAX_WIDTH: usize = 16;

    pub(crate) fn validate_width(width: usize) {
        assert!(
            (Self::MIN_WIDTH..=Self::MAX_WIDTH).contains(&width),
            "width must be {}..={}, got {}",
            Self::MIN_WIDTH,
            Self::MAX_WIDTH,
            width
        );
    }

    /// Creates a config for the given bit-width.
    ///
    /// `width` must be in `1..=16`.
    pub fn new(width: usize) -> Self {
        Self::validate_width(width);
        let group_size = 1 << (width - 1);
        let mask = (group_size - 1) as u64;
        let buckets = group_size * (66 - width);
        Self {
            width,
            group_size,
            mask,
            buckets,
            small_value_cache_size: 4096,
        }
    }

    /// Bit-width parameter.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Number of buckets per group: `2^(width-1)`.
    pub fn group_size(&self) -> usize {
        self.group_size
    }

    /// Bitmask for extracting the offset within a bucket group.
    pub fn mask(&self) -> u64 {
        self.mask
    }

    /// Total number of buckets covering the full u64 range.
    pub fn buckets(&self) -> usize {
        self.buckets
    }

    /// Number of values eligible for the small-value lookup cache.
    pub fn small_value_cache_size(&self) -> usize {
        self.small_value_cache_size
    }
}

#[cfg(test)]
mod tests {
    use super::LogScaleConfig;

    #[test]
    fn test_new_accepts_supported_widths() {
        assert_eq!(LogScaleConfig::new(1).width(), 1);
        assert_eq!(LogScaleConfig::new(16).width(), 16);
    }

    #[test]
    #[should_panic(expected = "width must be 1..=16, got 0")]
    fn test_new_rejects_zero_width() {
        let _ = LogScaleConfig::new(0);
    }

    #[test]
    #[should_panic(expected = "width must be 1..=16, got 17")]
    fn test_new_rejects_width_above_max() {
        let _ = LogScaleConfig::new(17);
    }
}
