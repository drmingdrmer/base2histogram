use super::histogram::Histogram;

/// A named histogram series within an [`super::ascii_chart::AsciiChart`].
#[derive(Debug, Clone)]
pub struct Series<T, const WIDTH: usize> {
    /// Display name used in chart legends and percentile footer.
    pub(crate) name: String,

    /// The source histogram.
    pub(crate) histogram: Histogram<T, WIDTH>,
}

impl<T, const WIDTH: usize> Series<T, WIDTH> {
    pub fn new(name: impl ToString, histogram: Histogram<T, WIDTH>) -> Self {
        Self {
            name: name.to_string(),
            histogram,
        }
    }
}
