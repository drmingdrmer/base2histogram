# base2histogram

`base2histogram` is a 2 KB histogram that tracks any `u64` distribution and
answers percentile queries (P50, P99, P99.9, …) with under 2% error for
typical latency workloads.

- **Near-zero error for API latency tracking** — log-normal distributions
  (typical for API/service latency) achieve **< 0.1% error at P50/P95/P99**
  with just 2 KB (default WIDTH=3, 252 buckets):
  ```text
  LN-API  P50  0.000%     P95  0.080%     P99  0.086%
  ```
- **`O(1)` recording**, fixed memory — no sorting, no resizing, suitable for
  hot paths.
- **Full `u64` range** — from nanoseconds to hours in a single histogram.
- **Sliding-window** aggregation via multi-slot mode.

## Usage

```rust
use base2histogram::Histogram;

let mut hist = Histogram::<()>::new();

hist.record(5);
hist.record(8);
hist.record(13);
hist.record_n(21, 3);

assert_eq!(hist.total(), 6);
assert_eq!(hist.percentile(0.50), 12);
assert_eq!(hist.percentile(0.99), 23);
```

## Sliding Window

Use `with_slots()` plus `advance()` when metrics should be aggregated over a
bounded set of recent windows:

```rust
use base2histogram::Histogram;

let mut hist = Histogram::<&'static str>::with_slots(2);

hist.record_n(10, 2);
hist.advance("warm");
hist.record_n(100, 3);

assert_eq!(hist.total(), 5);

hist.advance("steady");

// The oldest slot is evicted once the slot limit is reached.
assert_eq!(hist.total(), 3);
```

## Common Stats

```rust
use base2histogram::Histogram;

let mut hist = Histogram::<()>::new();
hist.record_n(20, 80);
hist.record_n(80, 20);

let stats = hist.percentile_stats();

assert_eq!(stats.samples, 100);
assert_eq!(stats.p50, 21);
assert_eq!(stats.p90, 87);
```

## Percentile Accuracy

`percentile()` uses **trapezoidal density interpolation**: it examines
neighboring bucket counts to estimate a density gradient across the target
bucket, then solves the inverse CDF to pinpoint the value within the bucket.
This requires no additional storage beyond the bucket array itself.

The `WIDTH` parameter controls bucket granularity: each bucket group uses
`WIDTH` bits, giving `2^(WIDTH-1)` buckets per group. Higher `WIDTH` means
finer resolution but more memory. The default is `WIDTH=3` (252 buckets).

Measured with 1,000,000 samples per distribution (`cargo run --bin accuracy`).
Error = `|exact - estimated| / exact × 100%`, shown at P50 / P95 / P99:

```text
                        W=1        W=2        W=3        W=4        W=5        W=6
  --------------------------------------------------------------------------------
  Uniform P50        0.108%     0.028%     0.012%     0.018%     0.019%     0.002%
          P95        2.317%     1.988%     1.035%     0.475%     0.005%     0.005%
          P99        4.290%     4.129%     3.706%     1.486%     0.298%     0.162%

  LN-API  P50        2.281%     0.182%     0.000%     0.000%     0.000%     0.000%
          P95       20.256%     3.963%     0.080%     0.040%     0.040%     0.000%
          P99       11.951%     3.594%     0.086%     0.000%     0.029%     0.000%

  Bimodal P50        1.381%     0.394%     0.394%     0.197%     0.197%     0.197%
          P95        3.918%     0.172%     0.012%     0.028%     0.038%     0.008%
          P99        1.521%     1.344%     0.543%     0.078%     0.016%     0.014%

  Expon   P50        1.012%     0.000%     0.145%     0.145%     0.145%     0.000%
          P95       10.989%     0.200%     0.000%     0.000%     0.033%     0.033%
          P99       18.665%     4.574%     0.824%     0.022%     0.022%     0.022%

  LN-DB   P50        2.018%     0.034%     0.000%     0.000%     0.000%     0.034%
          P95        2.027%     0.368%     0.039%     0.006%     0.019%     0.026%
          P99        3.764%     1.066%     0.187%     0.007%     0.003%     0.062%

  Sequent P50        0.095%     0.000%     0.000%     0.000%     0.000%     0.000%
          P95        2.271%     1.967%     1.011%     0.496%     0.000%     0.000%
          P99        4.272%     4.118%     3.696%     1.521%     0.305%     0.169%

  Pareto  P50       10.127%     1.899%     0.633%     0.633%     0.633%     0.000%
          P95        9.239%     0.272%     0.000%     0.136%     0.000%     0.000%
          P99        3.517%     0.879%     0.231%     0.093%     0.046%     0.046%

  --------------------------------------------------------------------------------
  Buckets                65        128        252        496        976       1920
  Mem/slot            520 B     1.0 KB     2.0 KB     3.9 KB     7.6 KB    15.0 KB
  Mem total          1.0 KB     2.0 KB     3.9 KB     7.8 KB    15.2 KB    30.0 KB
  --------------------------------------------------------------------------------
```

Distributions: Uniform [0, 1M], Log-normal API latency (σ=0.5),
Bimodal cache hit/miss (90/10), Exponential IO wait, Log-normal DB query
(σ=1.0), Sequential [1..N], Pareto heavy tail (α=1.5).

## License

Licensed under Apache-2.0. See [LICENSE](./LICENSE).
