use std::fmt;

use crate::Histogram;

/// Displays non-empty buckets of a histogram, one per line.
pub struct DisplayBuckets<'a, T> {
    histogram: &'a Histogram<T>,
}

impl<'a, T> DisplayBuckets<'a, T> {
    pub(crate) fn new(histogram: &'a Histogram<T>) -> Self {
        Self { histogram }
    }
}

impl<T> fmt::Display for DisplayBuckets<'_, T> {
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
        assert_eq!(hist.display_buckets().to_string(), "b9[10,12)=5");
    }

    #[test]
    fn test_display_multiple_buckets() {
        let mut hist: Histogram = Histogram::new();
        hist.record_n(5, 10);
        hist.record_n(100, 20);
        assert_eq!(hist.display_buckets().to_string(), "b5[5,6)=10\nb22[96,112)=20");
    }

    #[test]
    fn test_display_skips_empty_buckets() {
        let mut hist: Histogram = Histogram::new();
        hist.record_n(0, 3);
        hist.record_n(1000, 7);
        assert_eq!(hist.display_buckets().to_string(), "b0[0,1)=3\nb35[896,1024)=7");
    }
}
