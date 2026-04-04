use std::fmt;
use std::fmt::Write;

use super::ascii_chart::AsciiChart;
use super::ascii_chart::BAR_CHARS;
use super::ascii_chart::digit_count;

/// Detailed ASCII chart display, with header and percentile summary.
///
/// Created by [`AsciiChart::detailed`]. Renders lazily via [`fmt::Display`].
pub struct DetailedDisplay<'a, T> {
    chart: &'a AsciiChart<T>,
}

impl<'a, T> DetailedDisplay<'a, T> {
    pub(crate) fn new(chart: &'a AsciiChart<T>) -> Self {
        Self { chart }
    }
}

impl<T> fmt::Display for DetailedDisplay<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let indices = self.chart.non_empty_indices();
        if indices.is_empty() {
            return Ok(());
        }

        let max_count = self.chart.max_stacked_count(&indices);
        let single = self.chart.series.len() == 1;
        let h0 = &self.chart.series[0].histogram;

        let max_left_width = indices.iter().map(|&i| digit_count(h0.bucket(i).left())).max().unwrap_or(1);
        let max_right_width = indices.iter().map(|&i| digit_count(h0.bucket(i).right())).max().unwrap_or(1);
        let range_col_width = max_left_width + max_right_width + 4;

        // Header
        if single {
            writeln!(f, "{:>width$} | count", "range", width = range_col_width)?;
        } else {
            write!(f, "{:>width$} |", "range", width = range_col_width)?;
            for (si, series) in self.chart.series.iter().enumerate() {
                write!(f, " {} {}", BAR_CHARS[si % BAR_CHARS.len()], series.name)?;
            }
            f.write_char('\n')?;
        }

        // Data rows
        for &bucket_i in &indices {
            let b = h0.bucket(bucket_i);

            write!(
                f,
                "[{:>w1$}, {:>w2$}) | ",
                b.left(),
                b.right(),
                w1 = max_left_width,
                w2 = max_right_width
            )?;

            self.chart.write_bar(f, bucket_i, max_count)?;

            if single {
                write!(f, " {}", h0.bucket(bucket_i).count())?;
            } else {
                f.write_str("  ")?;
                for (si, series) in self.chart.series.iter().enumerate() {
                    if si > 0 {
                        f.write_str(" + ")?;
                    }
                    write!(f, "{}", series.histogram.bucket(bucket_i).count())?;
                }
            }

            f.write_char('\n')?;
        }

        // Percentile footer
        if single {
            let stats = h0.percentile_stats();
            write!(
                f,
                "total: {}  P50: {}  P90: {}  P99: {}",
                stats.samples, stats.p50, stats.p90, stats.p99
            )?;
        } else {
            let name_width = self.chart.series.iter().map(|s| s.name.len()).max().unwrap_or(0);
            for (si, series) in self.chart.series.iter().enumerate() {
                if si > 0 {
                    f.write_char('\n')?;
                }
                let stats = series.histogram.percentile_stats();
                write!(
                    f,
                    "{} {:>width$}  total: {}  P50: {}  P90: {}  P99: {}",
                    BAR_CHARS[si % BAR_CHARS.len()],
                    series.name,
                    stats.samples,
                    stats.p50,
                    stats.p90,
                    stats.p99,
                    width = name_width
                )?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::histogram::Histogram;

    #[test]
    fn test_single_detailed() {
        let mut hist = Histogram::<()>::new();
        hist.record_n(5, 10);
        hist.record_n(100, 3);

        let chart = AsciiChart::new().add("test", hist).bar_width(20);
        let expect = [
            "    range | count",
            "[ 5,   6) | ████████████████████ 10",
            "[96, 112) | ██████ 3",
            "total: 13  P50: 5  P90: 106  P99: 111",
        ]
        .join("\n");
        assert_eq!(chart.detailed().to_string(), expect);
    }

    #[test]
    fn test_stacked_detailed() {
        let mut hist_a = Histogram::<()>::new();
        let mut hist_b = Histogram::<()>::new();
        hist_a.record_n(5, 10);
        hist_b.record_n(5, 5);
        hist_b.record_n(100, 3);

        let chart = AsciiChart::new().add("a", hist_a).add("b", hist_b).bar_width(20);
        let expect = [
            "    range | █ a ▒ b",
            "[ 5,   6) | █████████████▒▒▒▒▒▒▒  10 + 5",
            "[96, 112) | ▒▒▒▒  0 + 3",
            "█ a  total: 10  P50: 5  P90: 5  P99: 5",
            "▒ b  total: 8  P50: 5  P90: 111  P99: 111",
        ]
        .join("\n");
        assert_eq!(chart.detailed().to_string(), expect);
    }
}
