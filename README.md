# base2histogram

`base2histogram` is a 2 KB histogram that tracks any `u64` distribution and
answers percentile queries (P50, P99, P99.9, …) with under 2% error for
typical latency workloads.

- **< 2% percentile error with just 2 KB (252 buckets)** for latency-like
  distributions — 10× more accurate than returning raw bucket boundaries
  at the same memory cost.
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
This requires no additional storage — the same 252 × 8 = 2,016 bytes as any
histogram with the same bucket structure.

Measured with 1,000,000 samples per distribution (`cargo run --bin accuracy`):

| Distribution                  | Max Error | Typical Use Case       |
|-------------------------------|-----------|------------------------|
| Log-normal (σ=0.5)           |    0.29%  | API latency            |
| Log-normal (σ=1.0)           |    0.76%  | Database queries       |
| Exponential                   |    1.96%  | Network/IO wait        |
| Pareto (α=1.5)               |    4.00%  | Request sizes          |
| Bimodal (90/10 split)         |    5.98%  | Cache hit/miss         |
| Uniform                       |    4.73%  | Synthetic baseline     |

## License

Licensed under Apache-2.0. See [LICENSE](./LICENSE).
