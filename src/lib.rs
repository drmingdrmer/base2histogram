//! A fixed-size histogram for fast percentile estimation over `u64` values.
//!
//! `base2histogram` uses base-2 logarithmic bucketing to provide:
//!
//! - `O(1)` recording,
//! - bounded memory usage,
//! - percentile queries without sorting samples,
//! - optional multi-slot aggregation for sliding windows.
//!
//! The default histogram covers the full `u64` range with a fixed number of
//! buckets. Small values are represented more precisely, while larger values
//! trade precision for compactness.
//!
//! # Examples
//!
//! Basic percentile tracking:
//!
//! ```
//! use base2histogram::Histogram;
//!
//! let mut hist = Histogram::<()>::new();
//! hist.record(5);
//! hist.record(8);
//! hist.record(13);
//! hist.record_n(21, 3);
//!
//! assert_eq!(hist.total(), 6);
//! assert_eq!(hist.percentile(0.50), 13);
//! assert_eq!(hist.percentile(0.99), 23);
//! ```
//!
//! Sliding-window style aggregation with slots:
//!
//! ```
//! use base2histogram::Histogram;
//!
//! let mut hist = Histogram::<&'static str>::with_slots(2);
//! hist.record_n(10, 2);
//! hist.advance("warm");
//! hist.record_n(100, 3);
//!
//! assert_eq!(hist.total(), 5);
//!
//! hist.advance("steady");
//! assert_eq!(hist.total(), 3);
//! ```

pub mod histogram;

pub use histogram::AsciiChart;
pub use histogram::BucketRef;
pub use histogram::CumulativeCount;
pub use histogram::DefaultLogScaleConfig;
pub use histogram::Histogram;
pub use histogram::LOG_SCALE;
pub use histogram::LogScale;
pub use histogram::LogScale3;
pub use histogram::LogScaleConfig;
pub use histogram::PercentileStats;
