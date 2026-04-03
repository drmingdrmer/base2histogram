use std::fmt;
use std::fmt::Formatter;

use super::log_scale::LogScale;

/// A lazy reference to a single bucket in a histogram.
///
/// Holds a reference to the log scale and the bucket index.
/// Left/right boundary values are computed only when the corresponding method is called.
#[derive(Debug, Clone, Copy)]
pub struct BucketRef<'a, const WIDTH: usize> {
    log_scale: &'a LogScale<WIDTH>,
    index: usize,
    count: u64,
}

impl<const WIDTH: usize> fmt::Display for BucketRef<'_, WIDTH> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "[{},{})={}", self.left(), self.right(), self.count())
    }
}

impl<'a, const WIDTH: usize> BucketRef<'a, WIDTH> {
    pub(crate) fn new(log_scale: &'a LogScale<WIDTH>, index: usize, count: u64) -> Self {
        Self {
            log_scale,
            index,
            count,
        }
    }

    /// The left close value that maps to this bucket.
    pub fn left(&self) -> u64 {
        self.log_scale.bucket_left(self.index)
    }

    /// The right open boundary that maps to this bucket.
    pub fn right(&self) -> u64 {
        self.log_scale.bucket_right(self.index)
    }

    /// The number of samples recorded in this bucket.
    pub fn count(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use crate::histogram::Histogram;

    #[test]
    fn test_display() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(5, 10);
        hist.record_n(100, 3);

        let buckets: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();

        assert_eq!(buckets[0].to_string(), "[5,6)=10");
        assert_eq!(buckets[1].to_string(), "[96,112)=3");
    }
}
