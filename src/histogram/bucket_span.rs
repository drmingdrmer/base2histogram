use super::log_scale::LogScale;

/// A reference to a bucket's geometry in a log scale.
///
/// Provides access to left, right, width, and midpoint without carrying
/// histogram data (count). Created by [`LogScale::bucket_span`].
#[derive(Debug, Clone, Copy)]
pub struct BucketSpan<'a, const WIDTH: usize> {
    log_scale: &'a LogScale<WIDTH>,
    index: usize,
}

impl<'a, const WIDTH: usize> BucketSpan<'a, WIDTH> {
    pub(crate) fn new(log_scale: &'a LogScale<WIDTH>, index: usize) -> Self {
        Self { log_scale, index }
    }

    /// Bucket index.
    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Left (inclusive) boundary.
    #[inline]
    pub fn left(&self) -> u64 {
        self.log_scale.bucket_min_values[self.index]
    }

    /// Right (exclusive) boundary.
    #[inline]
    pub fn right(&self) -> u64 {
        if self.index + 1 < self.log_scale.bucket_min_values.len() {
            self.log_scale.bucket_min_values[self.index + 1]
        } else {
            u64::MAX
        }
    }

    /// Width of the bucket: `right - left`.
    #[inline]
    pub fn width(&self) -> u64 {
        self.right() - self.left()
    }

    /// Midpoint of the bucket: `left + width / 2`.
    #[inline]
    pub fn midpoint(&self) -> u64 {
        self.left() + self.width() / 2
    }
}

#[cfg(test)]
mod tests {
    use super::super::log_scale::LOG_SCALE;

    #[test]
    fn test_bucket_span() {
        // [0,1): width=1, midpoint=0
        let b = LOG_SCALE.bucket_span(0);
        assert_eq!(
            (b.index(), b.left(), b.right(), b.width(), b.midpoint()),
            (0, 0, 1, 1, 0)
        );

        // [5,6): width=1, midpoint=5
        let b = LOG_SCALE.bucket_span(5);
        assert_eq!(
            (b.index(), b.left(), b.right(), b.width(), b.midpoint()),
            (5, 5, 6, 1, 5)
        );

        // [8,10): width=2, midpoint=9
        let b = LOG_SCALE.bucket_span(8);
        assert_eq!(
            (b.index(), b.left(), b.right(), b.width(), b.midpoint()),
            (8, 8, 10, 2, 9)
        );

        // [16,20): width=4, midpoint=18
        let b = LOG_SCALE.bucket_span(12);
        assert_eq!(
            (b.index(), b.left(), b.right(), b.width(), b.midpoint()),
            (12, 16, 20, 4, 18)
        );

        // [32,40): width=8, midpoint=36
        let b = LOG_SCALE.bucket_span(16);
        assert_eq!(
            (b.index(), b.left(), b.right(), b.width(), b.midpoint()),
            (16, 32, 40, 8, 36)
        );

        // Last bucket (251): right=u64::MAX
        let b = LOG_SCALE.bucket_span(251);
        assert_eq!(b.index(), 251);
        assert_eq!(b.left(), 0b111 << 61);
        assert_eq!(b.right(), u64::MAX);
        assert_eq!(b.width(), u64::MAX - (0b111 << 61));
        assert_eq!(b.midpoint(), (0b111 << 61) + (u64::MAX - (0b111 << 61)) / 2);
    }
}
