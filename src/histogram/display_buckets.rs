use std::fmt;

use crate::Histogram;

/// Displays non-empty buckets of a histogram, one per line.
pub struct DisplayBuckets<'a, T, const WIDTH: usize> {
    histogram: &'a Histogram<T, WIDTH>,
}

impl<'a, T, const WIDTH: usize> DisplayBuckets<'a, T, WIDTH> {
    pub(crate) fn new(histogram: &'a Histogram<T, WIDTH>) -> Self {
        Self { histogram }
    }
}

impl<T, const WIDTH: usize> fmt::Display for DisplayBuckets<'_, T, WIDTH> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for b in self.histogram.bucket_data() {
            if b.count() == 0 {
                continue;
            }
            if !first {
                f.write_str("\n")?;
            }
            first = false;
            write!(f, "{b}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Histogram;

    #[test]
    fn test_display_empty() {
        let hist: Histogram = Histogram::new();
        assert_eq!(hist.display_buckets().to_string(), "");
    }

    #[test]
    fn test_display_single_bucket() {
        let mut hist: Histogram = Histogram::new();
        hist.record_n(10, 5);
        assert_eq!(hist.display_buckets().to_string(), "[0xa,0xc)=5");
    }

    #[test]
    fn test_display_multiple_buckets() {
        let mut hist: Histogram = Histogram::new();
        hist.record_n(5, 10);
        hist.record_n(100, 20);
        assert_eq!(hist.display_buckets().to_string(), "[0x5,0x6)=10\n[0x60,0x70)=20");
    }

    #[test]
    fn test_display_skips_empty_buckets() {
        let mut hist: Histogram = Histogram::new();
        hist.record_n(0, 3);
        hist.record_n(1000, 7);
        assert_eq!(hist.display_buckets().to_string(), "[0x0,0x1)=3\n[0x380,0x400)=7");
    }
}
