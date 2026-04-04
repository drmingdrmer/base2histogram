use std::fmt;
use std::fmt::Write;

use super::compact_display::CompactDisplay;
use super::detailed_display::DetailedDisplay;
use super::histogram::Histogram;
use super::series::Series;

pub(crate) const BAR_CHARS: [char; 8] = ['█', '▒', '░', '▓', '▞', '▚', '▖', '▘'];

/// ASCII histogram chart supporting single or stacked multi-series display.
///
/// Two rendering modes:
/// - **Compact** (`compact()`): minimal output for log files
/// - **Detailed** (`detailed()`): richer output with headers and percentile summary
///
/// Both return a struct implementing `Display`, rendering lazily on format.
///
/// # Examples
///
/// ```
/// use base2histogram::{Histogram, AsciiChart};
///
/// let mut hist = Histogram::<()>::new();
/// hist.record_n(10, 5);
/// hist.record_n(100, 3);
///
/// let chart = AsciiChart::new().add("latency", hist.clone());
/// println!("{}", chart.compact());
/// println!("{}", chart.detailed());
/// ```
///
/// Compact output:
///
/// ```text
///  10-11  ████████████████████████████████████████ 5
/// 96-111  ████████████████████████ 3
/// ```
///
/// Detailed output:
///
/// ```text
///     range | count
/// [10,  11] | ████████████████████████████████████████ 5
/// [96, 111] | ████████████████████████ 3
/// total: 8  P50: 10  P90: 111  P99: 111
/// ```
#[derive(Debug, Clone)]
pub struct AsciiChart<T = ()> {
    pub(crate) series: Vec<Series<T>>,
    bar_width: usize,
}

impl<T> Default for AsciiChart<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AsciiChart<T> {
    pub fn new() -> Self {
        Self {
            series: Vec::new(),
            bar_width: 40,
        }
    }

    /// Creates a chart from named histograms.
    pub fn from_series(series: impl IntoIterator<Item = (impl ToString, Histogram<T>)>) -> Self {
        Self {
            series: series.into_iter().map(|(name, hist)| Series::new(name.to_string(), hist)).collect(),
            bar_width: 40,
        }
    }

    /// Adds a histogram as a named series.
    pub fn add(mut self, name: &str, hist: Histogram<T>) -> Self {
        self.series.push(Series::new(name, hist));
        self
    }

    /// Sets the maximum bar width in characters (default: 40).
    pub fn bar_width(mut self, width: usize) -> Self {
        self.bar_width = width;
        self
    }

    /// Returns a compact display, rendered lazily via `Display`.
    pub fn compact(&self) -> CompactDisplay<'_, T> {
        CompactDisplay::new(self)
    }

    /// Returns a detailed display with header and percentile summary, rendered lazily via
    /// `Display`.
    pub fn detailed(&self) -> DetailedDisplay<'_, T> {
        DetailedDisplay::new(self)
    }

    /// Bucket indices where at least one series has count > 0.
    pub(crate) fn non_empty_indices(&self) -> Vec<usize> {
        let Some(first) = self.series.first() else {
            return Vec::new();
        };
        (0..first.histogram.num_buckets())
            .filter(|&i| self.series.iter().any(|s| s.histogram.bucket(i).count() > 0))
            .collect()
    }

    /// Max sum of counts across all series for any single bucket.
    pub(crate) fn max_stacked_count(&self, indices: &[usize]) -> u64 {
        indices
            .iter()
            .map(|&i| self.series.iter().map(|s| s.histogram.bucket(i).count()).sum::<u64>())
            .max()
            .unwrap_or(0)
    }

    /// Writes a stacked bar for the given bucket index to the formatter.
    pub(crate) fn write_bar(&self, f: &mut fmt::Formatter<'_>, bucket_idx: usize, max_count: u64) -> fmt::Result {
        if max_count == 0 {
            return Ok(());
        }

        for (si, series) in self.series.iter().enumerate() {
            let count = series.histogram.bucket(bucket_idx).count();
            let segment = if count > 0 {
                (count as f64 / max_count as f64 * self.bar_width as f64).round().max(1.0) as usize
            } else {
                0
            };

            let ch = BAR_CHARS[si % BAR_CHARS.len()];
            for _ in 0..segment {
                f.write_char(ch)?;
            }
        }

        Ok(())
    }
}

impl<T> fmt::Display for AsciiChart<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.detailed().fmt(f)
    }
}

pub(crate) fn digit_count(n: u64) -> usize {
    if n == 0 {
        return 1;
    }
    (n.ilog10() + 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_digit_count() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(1), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(99), 2);
        assert_eq!(digit_count(100), 3);
        assert_eq!(digit_count(999), 3);
        assert_eq!(digit_count(u64::MAX), 20);
    }

    #[test]
    fn test_empty_chart() {
        let chart: AsciiChart = AsciiChart::new();
        assert_eq!(chart.compact().to_string(), "");
        assert_eq!(chart.detailed().to_string(), "");
    }

    #[test]
    fn test_empty_histogram() {
        let hist = Histogram::<()>::new();
        let chart = AsciiChart::new().add("test", hist.clone());
        assert_eq!(chart.compact().to_string(), "");
        assert_eq!(chart.detailed().to_string(), "");
    }

    #[test]
    fn test_display_uses_detailed() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(5, 10);

        let chart = AsciiChart::new().add("test", hist.clone());
        assert_eq!(format!("{}", chart), chart.detailed().to_string());
    }
}
