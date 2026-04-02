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
