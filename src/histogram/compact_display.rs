use std::fmt;
use std::fmt::Write;

use super::ascii_chart::AsciiChart;
use super::ascii_chart::digit_count;

/// Compact ASCII chart display, suitable for log files.
///
/// Created by [`AsciiChart::compact`]. Renders lazily via [`fmt::Display`].
pub struct CompactDisplay<'a, T, const WIDTH: usize> {
    chart: &'a AsciiChart<T, WIDTH>,
}

impl<'a, T, const WIDTH: usize> CompactDisplay<'a, T, WIDTH> {
    pub(crate) fn new(chart: &'a AsciiChart<T, WIDTH>) -> Self {
        Self { chart }
    }
}

impl<T, const WIDTH: usize> fmt::Display for CompactDisplay<'_, T, WIDTH> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let indices = self.chart.non_empty_indices();
        if indices.is_empty() {
            return Ok(());
        }

        let max_count = self.chart.max_stacked_count(&indices);
        let single = self.chart.series.len() == 1;
        let h0 = &self.chart.series[0].histogram;

        let range_width = indices
            .iter()
            .map(|&i| {
                let b = h0.bucket(i);
                compact_range_width(b.left(), b.right())
            })
            .max()
            .unwrap_or(0);

        for (idx, &bucket_i) in indices.iter().enumerate() {
            if idx > 0 {
                f.write_char('\n')?;
            }

            let b = h0.bucket(bucket_i);
            let this_width = compact_range_width(b.left(), b.right());
            for _ in this_width..range_width {
                f.write_char(' ')?;
            }
            write!(f, "[{},{})  ", b.left(), b.right())?;

            self.chart.write_bar(f, bucket_i, max_count)?;

            if single {
                write!(f, " {}", h0.bucket(bucket_i).count())?;
            } else {
                f.write_str("  ")?;
                for (si, series) in self.chart.series.iter().enumerate() {
                    if si > 0 {
                        f.write_char(' ')?;
                    }
                    write!(f, "{}:{}", series.name, series.histogram.bucket(bucket_i).count())?;
                }
            }
        }

        Ok(())
    }
}

/// Width of `"[left,right)"` without allocating.
fn compact_range_width(left: u64, right: u64) -> usize {
    1 + digit_count(left) + 1 + digit_count(right) + 1 // "[left,right)"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::histogram::Histogram;

    #[test]
    fn test_single_compact() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(5, 10);
        hist.record_n(100, 3);

        let chart = AsciiChart::new().add("test", hist.clone()).bar_width(20);
        let expect = ["   [5,6)  ████████████████████ 10", "[96,112)  ██████ 3"].join("\n");
        assert_eq!(chart.compact().to_string(), expect);
    }

    #[test]
    fn test_stacked_compact() {
        let mut hist_a = Histogram::<()>::new();
        let mut hist_b = Histogram::<()>::new();
        hist_a.record_n(5, 10);
        hist_b.record_n(5, 5);

        let chart = AsciiChart::new().add("a", hist_a).add("b", hist_b).bar_width(20);
        assert_eq!(chart.compact().to_string(), "[5,6)  █████████████▒▒▒▒▒▒▒  a:10 b:5");
    }

    #[test]
    fn test_bar_width_setting() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(5, 10);

        let chart_narrow = AsciiChart::new().add("test", hist.clone()).bar_width(10);
        assert_eq!(chart_narrow.compact().to_string(), "[5,6)  ██████████ 10");

        let chart_wide = AsciiChart::new().add("test", hist.clone()).bar_width(50);
        assert_eq!(
            chart_wide.compact().to_string(),
            "[5,6)  ██████████████████████████████████████████████████ 10"
        );
    }
}
