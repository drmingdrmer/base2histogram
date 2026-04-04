use super::histogram::Histogram;

/// A named histogram series within an [`super::ascii_chart::AsciiChart`].
#[derive(Debug, Clone)]
pub struct Series<T> {
    /// Display name used in chart legends and percentile footer.
    pub(crate) name: String,

    /// The source histogram.
    pub(crate) histogram: Histogram<T>,
}

impl<T> Series<T> {
    pub fn new(name: impl ToString, histogram: Histogram<T>) -> Self {
        Self {
            name: name.to_string(),
            histogram,
        }
    }
}
