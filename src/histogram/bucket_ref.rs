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
        write!(f, "[{:#x},{:#x})={}", self.left(), self.right(), self.count())
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

    /// Bucket index.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Width of the bucket: `right - left`.
    pub fn width(&self) -> u64 {
        self.right() - self.left()
    }

    /// Midpoint of the bucket: `left + width / 2`.
    pub fn midpoint(&self) -> u64 {
        self.left() + self.width() / 2
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
    fn test_index_width_midpoint() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(0, 1);
        hist.record_n(5, 1);
        hist.record_n(9, 1);
        hist.record_n(18, 1);
        hist.record_n(50, 1);

        let buckets: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();

        // [0,1): index=0, width=1, midpoint=0
        assert_eq!(
            (buckets[0].index(), buckets[0].width(), buckets[0].midpoint()),
            (0, 1, 0)
        );
        // [5,6): index=5, width=1, midpoint=5
        assert_eq!(
            (buckets[1].index(), buckets[1].width(), buckets[1].midpoint()),
            (5, 1, 5)
        );
        // [8,10): index=8, width=2, midpoint=9
        assert_eq!(
            (buckets[2].index(), buckets[2].width(), buckets[2].midpoint()),
            (8, 2, 9)
        );
        // [16,20): index=12, width=4, midpoint=18
        assert_eq!(
            (buckets[3].index(), buckets[3].width(), buckets[3].midpoint()),
            (12, 4, 18)
        );
        // [48,56): index=18, width=8, midpoint=52
        assert_eq!(
            (buckets[4].index(), buckets[4].width(), buckets[4].midpoint()),
            (18, 8, 52)
        );
    }

    #[test]
    fn test_first_bucket() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(0, 5);

        let b = hist.bucket_data().next().unwrap();
        assert_eq!(
            (b.index(), b.left(), b.right(), b.width(), b.midpoint(), b.count()),
            (0, 0, 1, 1, 0, 5)
        );
    }

    #[test]
    fn test_last_bucket() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(u64::MAX, 2);

        let b = hist.bucket_data().last().unwrap();
        assert_eq!(b.index(), 251);
        assert_eq!(b.left(), 0b111 << 61);
        assert_eq!(b.right(), u64::MAX);
        assert_eq!(b.width(), u64::MAX - (0b111 << 61));
        assert_eq!(b.midpoint(), (0b111 << 61) + (u64::MAX - (0b111 << 61)) / 2);
        assert_eq!(b.count(), 2);
    }

    #[test]
    fn test_zero_count_bucket() {
        let hist = Histogram::<()>::new();
        let b = hist.bucket_data().next().unwrap();
        assert_eq!(b.count(), 0);
        // Geometry still works on empty buckets
        assert_eq!((b.index(), b.left(), b.right(), b.width()), (0, 0, 1, 1));
    }

    #[test]
    fn test_display() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(5, 10);
        hist.record_n(100, 3);

        let buckets: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();

        assert_eq!(buckets[0].to_string(), "[0x5,0x6)=10");
        assert_eq!(buckets[1].to_string(), "[0x60,0x70)=3");
    }

    /// Values at exact/log group boundaries: 3→4, 7→8
    #[test]
    fn test_group_boundaries() {
        let mut hist = Histogram::<()>::new();
        // Last exact bucket in group 0, first in group 1, last in group 1, first log bucket
        for v in [3, 4, 7, 8] {
            hist.record(v);
        }

        let buckets: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();
        assert_eq!(buckets.len(), 4);

        // [3,4) exact, width=1
        assert_eq!((buckets[0].index(), buckets[0].left(), buckets[0].width()), (3, 3, 1));
        // [4,5) exact, width=1
        assert_eq!((buckets[1].index(), buckets[1].left(), buckets[1].width()), (4, 4, 1));
        // [7,8) exact, width=1
        assert_eq!((buckets[2].index(), buckets[2].left(), buckets[2].width()), (7, 7, 1));
        // [8,10) first log bucket, width=2
        assert_eq!((buckets[3].index(), buckets[3].left(), buckets[3].width()), (8, 8, 2));
    }

    /// Multiple values landing in the same bucket aggregate counts
    #[test]
    fn test_count_aggregation() {
        let mut hist = Histogram::<()>::new();
        // 8 and 9 both map to bucket [8,10)
        hist.record_n(8, 3);
        hist.record_n(9, 7);

        let b: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].index(), b[0].left(), b[0].right(), b[0].count()), (8, 8, 10, 10));
    }

    /// Adjacent buckets have contiguous ranges (no gaps, no overlaps)
    #[test]
    fn test_adjacent_buckets_contiguous() {
        let hist = Histogram::<()>::new();
        let all: Vec<_> = hist.bucket_data().collect();

        for pair in all.windows(2) {
            assert_eq!(
                pair[0].right(),
                pair[1].left(),
                "gap between bucket {} and {}",
                pair[0].index(),
                pair[1].index()
            );
            assert_eq!(pair[1].index(), pair[0].index() + 1);
        }
    }

    /// Second-to-last bucket geometry
    #[test]
    fn test_second_to_last_bucket() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(0b110 << 61, 1);

        let b: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].index(), 250);
        assert_eq!(b[0].left(), 0b110 << 61);
        assert_eq!(b[0].right(), 0b111 << 61);
        assert_eq!(b[0].width(), 1 << 61);
    }

    /// Power-of-two boundaries land in the first sub-bucket of each group
    #[test]
    fn test_power_of_two_boundaries() {
        let mut hist = Histogram::<()>::new();
        for &v in &[16, 32, 64, 128, 256] {
            hist.record(v);
        }

        let buckets: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();

        // Each power-of-2 is the left boundary of the first sub-bucket in its group
        for b in &buckets {
            assert_eq!(
                b.left(),
                b.left().next_power_of_two(),
                "bucket {} left={}",
                b.index(),
                b.left()
            );
        }
        assert_eq!(buckets.iter().map(|b| b.left()).collect::<Vec<_>>(), vec![
            16, 32, 64, 128, 256
        ]);
    }

    #[test]
    fn test_bucket_data() {
        let mut hist: Histogram = Histogram::new();
        hist.record(5);
        hist.record_n(10, 3);

        let non_empty: Vec<_> = hist.bucket_data().filter(|b| b.count() > 0).collect();

        assert_eq!(non_empty.len(), 2);
        assert_eq!(
            (non_empty[0].left(), non_empty[0].right(), non_empty[0].count()),
            (5, 6, 1)
        );
        assert_eq!(
            (non_empty[1].left(), non_empty[1].right(), non_empty[1].count()),
            (10, 12, 3)
        );
    }

    #[test]
    fn test_bucket_data_includes_all_buckets() {
        let hist: Histogram = Histogram::new();
        assert_eq!(hist.bucket_data().count(), 252);
        assert!(hist.bucket_data().all(|b| b.count() == 0));
    }
}
