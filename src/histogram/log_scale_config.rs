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
    pub fn new(width: usize) -> Self {
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

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn group_size(&self) -> usize {
        self.group_size
    }

    pub fn mask(&self) -> u64 {
        self.mask
    }

    pub fn buckets(&self) -> usize {
        self.buckets
    }

    pub fn small_value_cache_size(&self) -> usize {
        self.small_value_cache_size
    }
}
