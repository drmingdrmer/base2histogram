# base2histogram

`base2histogram` is a Rust histogram library for fast percentile estimation with
base-2 logarithmic bucketing.

It is designed for latency and metrics workloads where:

- recording should be `O(1)`,
- memory usage should stay bounded,
- percentile queries should be cheap,
- and large `u64` values should still be representable without resizing.

## Features

- Fixed-size histogram over the full `u64` range.
- Base-2 logarithmic buckets with bounded relative error.
- Cheap percentile queries such as `P50`, `P99`, and `P99.9`.
- Optional multi-slot mode for sliding-window style aggregation.
- No external runtime or allocator tricks required.

## Bucket Model

The default configuration uses 3 significant bits to define a bucket:

- values `0..=7` are represented exactly,
- values `8..=15` are grouped with step size `2`,
- values `16..=31` are grouped with step size `4`,
- larger values keep doubling the bucket width with the magnitude.

This keeps relative error bounded while covering the entire `u64` range in a
small, fixed number of buckets.

## Usage

```rust
use base2histogram::Histogram;

let mut hist = Histogram::<()>::new();

hist.record(5);
hist.record(8);
hist.record(13);
hist.record_n(21, 3);

assert_eq!(hist.total(), 6);

let p50 = hist.percentile(0.50);
let p99 = hist.percentile(0.99);

assert!(p50 <= p99);
```

`percentile()` returns an interpolated estimate within the bucket that contains
the target percentile. It uses neighboring bucket densities for trapezoidal
interpolation, achieving under 2% error for typical real-world distributions.

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
assert!((20..=21).contains(&stats.p50));
assert!((80..=87).contains(&stats.p90));
```

## Development

Common local commands:

```bash
make check
make test
make lint
make doc
```

## License

Licensed under Apache-2.0. See [LICENSE](./LICENSE).
