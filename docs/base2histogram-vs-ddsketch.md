# base2histogram vs DDSketch: Comparison

Comparison of [base2histogram](https://github.com/drmingdrmer/base2histogram) ([intro](https://blog.openacid.com/algo/histogram/), Rust) and [DDSketch](https://github.com/DataDog/sketches-go) ([paper](http://www.vldb.org/pvldb/vol12/p2195-masson.pdf), Go).

Versions compared:
- base2histogram v0.1.5 ([`08eb806`](https://github.com/drmingdrmer/base2histogram/commit/08eb806), 2026-04-03)
- DDSketch (sketches-go) [`36e98e0`](https://github.com/DataDog/sketches-go/commit/36e98e05d756ccb225b94882831c1443fb4ed535)

## DDSketch Algorithm

DDSketch maps each value `v` to a bucket index via:

```
index = floor( log(v) / log(γ) )
```

where `γ = (1+α)/(1-α)` and `α` is the desired relative accuracy (e.g., 0.01 for 1%).

This makes every bucket have the same **relative** width — bucket `i` covers `[γ^i, γ^(i+1))`. The ratio `bucket_width / bucket_min` is always `γ - 1 = 2α/(1-α)`. Returning the geometric midpoint `γ^i · (1+α)` guarantees worst-case relative error exactly `α` for any value in the bucket.

- **Recording**: compute one `log`, floor it → O(1).
- **Querying percentile**: linear scan through bins accumulating counts until the target rank is reached, then convert the bin index back to a value via `exp(index / multiplier)`.
- **Merging**: add corresponding bin counts — works because two sketches with the same `γ` share bucket boundaries.

Three regions handle the full number line: negative values (negated and stored separately), near-zero values (a dedicated counter), and positive values.

The key trade-off knob is `α`: smaller `α` → more bins → more memory, but tighter accuracy. Bin count scales as `log(max/min) / log(γ)`.

## Summary

Both histograms use logarithmic bucketing for O(1) recording and O(buckets) percentile queries. The fundamental difference: DDSketch provides a formal relative accuracy guarantee with continuous precision control, while base2histogram uses fixed-memory integer-only bucketing with trapezoidal interpolation for superior practical accuracy on smooth distributions.

| Strength | Winner |
|----------|--------|
| Formal error guarantee | DDSketch (configurable α) |
| Practical percentile accuracy | base2histogram (trapezoidal interpolation) |
| Precision configurability | DDSketch (continuous α at runtime) |
| Memory predictability | base2histogram (fixed at compile time) |
| Recording hot-path speed | base2histogram (integer-only, no FP/log) |
| Value type support | DDSketch (float64, negatives) |
| Sliding window | base2histogram (built-in slot eviction) |
| Mergeability | Tie (both add bin counts) |
| Ecosystem adoption | DDSketch (Datadog, OpenTelemetry) |

## Architecture Overview

| Aspect | DDSketch | base2histogram |
|--------|----------|----------------|
| Language | Go | Rust |
| Value type | `float64` (including negatives) | `u64` only |
| Bucket mapping | `floor(log(v) / log(γ))` | `leading_zeros` + bit shift + offset |
| Precision parameter | `α` (relative accuracy, 0 < α < 1) | `WIDTH` (1–6+), compile-time const |
| Bucket count | Dynamic: `log(max/min) / log(γ)` | Fixed: `2^(WIDTH-1) · (66 - WIDTH)` |
| Memory | Variable, depends on observed range | Fixed at compile time |
| Negative values | Yes (separate negative store) | No |
| Zero handling | Dedicated `zeroCount` with threshold | Bucket 0 maps to value 0 |
| Interpolation | None — returns bucket midpoint | Trapezoidal density estimation |

### Bucket Mapping

**DDSketch** maps value `v` to bucket index:

```
γ = (1 + α) / (1 - α)
index = floor( log(v) / log(γ) )
```

Three mapping implementations trade off CPU vs memory:
- **LogarithmicMapping**: calls `math.Log` — optimal bin count, slowest
- **CubicallyInterpolatedMapping**: IEEE 754 bit tricks + cubic polynomial — near-optimal bins, fast
- **LinearlyInterpolatedMapping**: IEEE 754 bits + linear approximation — ~1% more bins, fastest

**base2histogram** maps value `v` to bucket index using pure integer arithmetic:

```
group = bits_upto_msb - WIDTH
offset = (v >> group) & MASK
index = GROUP_SIZE + group * GROUP_SIZE + offset
```

No floating-point, no `log()` — just `leading_zeros`, shifts, and masks.

## Error Bound Analysis

### DDSketch: Formal Relative Guarantee

For any quantile query, if the true value is `x` and the returned value is `y`:

```
|x - y| / x ≤ α
```

This holds for **any** input distribution. The guarantee comes from the bucket design: each bucket `[γ^i, γ^(i+1))` has relative width `γ - 1 = 2α/(1-α)`, and the returned value `γ^i · (1 + α)` is the geometric midpoint, giving worst-case relative error exactly `α`.

### base2histogram: Bounded by WIDTH

For a group with bit-length `b` (values in `[2^(b-1), 2^b)`):

- Range spans `2^(b-1)` values
- Divided into `2^(WIDTH-1)` sub-buckets
- Each bucket width = `2^(b-1) / 2^(WIDTH-1)` = `2^(b - WIDTH)`

Worst-case relative error at the smallest value in a group:

```
relative_error = bucket_width / min_value
               = 2^(b - WIDTH) / 2^(b - 1)
               = 2^(1 - WIDTH)
               = 1 / 2^(WIDTH-1)
```

The `b` cancels — the ratio is constant across all groups. This is the fundamental property of logarithmic bucketing.

If the percentile returns the **midpoint** of the bucket, the max error halves to `1 / 2^WIDTH`.

| WIDTH | Buckets | Max error (full bucket) | Max error (midpoint) |
|-------|---------|------------------------|---------------------|
| 3 | 252 | 1/4 = 25% | 1/8 = 12.5% |
| 4 | 504 | 1/8 = 12.5% | 1/16 = 6.25% |
| 5 | 976 | 1/16 = 6.25% | 1/32 = 3.125% |
| 6 | 1,920 | 1/32 = 3.125% | 1/64 = 1.5625% |
| 8 | 7,424 | 1/128 = 0.781% | 1/256 = 0.391% |

The trapezoidal interpolation further reduces **practical** error well below these bounds for smooth distributions (e.g., <0.3% for log-normal with WIDTH=3), but provides no formal worst-case guarantee beyond the bucket bound.

### Precision Equivalence

To achieve DDSketch-equivalent 1% worst-case error:
- DDSketch: set `α = 0.01`, bucket count depends on value range
- base2histogram: need `WIDTH ≈ 8` (7,424 buckets, ~58 KB per slot)

## DDSketch Advantages

### 1. Formal relative accuracy guarantee

The `α` parameter provides a mathematically proven worst-case bound for **any** distribution. base2histogram's interpolation improves average accuracy but has no formal guarantee beyond the bucket width bound.

### 2. Continuous precision control at runtime

`α` is a continuous parameter — set `α=0.001` for 0.1% error, or `α=0.05` for less memory. Tunable at construction time. base2histogram's `WIDTH` is a compile-time const generic with power-of-2 error steps.

### 3. float64 and negative value support

Handles the full number line: negative values (separate store), near-zero values (dedicated count with configurable threshold), and positive values. base2histogram is `u64` only.

### 4. Multiple mapping strategies

Three implementations let users trade CPU cost for memory:

| Mapping | `f(value)` | Speed | Memory overhead |
|---------|-----------|-------|-----------------|
| Logarithmic | `math.Log(v)` | Slowest | Optimal |
| Cubic interpolation | IEEE 754 bits + cubic poly | Fast | Near-optimal |
| Linear interpolation | IEEE 754 bits + linear approx | Fastest | ~1% more bins |

### 5. Memory-adaptive stores

- **DenseStore**: contiguous array, best for concentrated data
- **BufferedPaginatedStore**: paged allocation, avoids huge contiguous allocations
- **CollapsingLowestDenseStore / CollapsingHighestDenseStore**: fixed bin cap — gracefully degrades accuracy when memory is bounded

### 6. Wide ecosystem adoption

Published in VLDB 2019, adopted by Datadog and OpenTelemetry. Well-studied with formal proofs. base2histogram is a newer, less widely deployed library.

## base2histogram Advantages

### 1. Trapezoidal interpolation for superior practical accuracy

DDSketch returns the bucket geometric midpoint — no interpolation. base2histogram models a density gradient across the target bucket using neighbor densities:

```
d(t) = d₁ + s·(t − 0.5)
```

then solves the CDF inverse to place the estimate within the bucket. On real-world distributions:

| Distribution | DDSketch (midpoint) | base2histogram (trapezoidal) |
|-------------|--------------------|-----------------------------|
| Log-normal (P99) | Up to α | <0.3% with WIDTH=3 |
| Uniform (P50) | Up to α | <0.02% |
| Exponential (P99) | Up to α | <0.11% |

DDSketch requires many more bins (smaller `α`) to match base2histogram's practical accuracy on smooth distributions.

### 2. Pure integer arithmetic — no floating-point on hot path

Bucket mapping uses `leading_zeros`, bit shifts, and masks. No `math.Log`, no IEEE 754 decomposition, no floating-point at all. This is faster and deterministic across platforms. DDSketch's fastest mapping (linear interpolation) still requires IEEE 754 bit extraction and floating-point multiply.

### 3. Deterministic fixed memory

Exactly `2^(WIDTH-1) · (66 - WIDTH) · 8` bytes per slot, always. No dynamic allocation, no growth with value range. DDSketch's memory depends on `log(max/min) / log(γ)` — a distribution spanning 1 to 10^9 with `α=0.01` needs ~1000+ bins.

### 4. Precomputed lookup tables

`bucket_min_values[252]` and `small_value_buckets[4096]` are computed once at static initialization. DDSketch computes `math.Exp(...)` on every `Value()` call. The small-value cache gives instant bucket lookup for values 0–4095.

### 5. Built-in sliding window with slot-level eviction

Multi-slot architecture maintains per-slot bucket arrays plus a running aggregate. `advance()` evicts the oldest slot in O(bucket_count). DDSketch provides no built-in windowing — the caller must manage per-window sketches externally.

### 6. Compile-time optimization

`WIDTH` is a const generic, so all derived constants (`GROUP_SIZE`, `MASK`, `BUCKETS`) are resolved at compile time. The compiler inlines and optimizes the entire bucket path. DDSketch's parameters are runtime values.

## Percentile Calculation

### DDSketch: `GetValueAtQuantile()`

```go
rank := quantile * (count - 1)
// Determine region: negative store, zero bucket, or positive store
// In the relevant store, linear scan through bins:
for i, b := range bins {
    n += b
    if n > rank { return Value(i + offset) }
}
```

- **Returns**: `lowerBound * (1 + α)` — the geometric midpoint of the bucket
- **No interpolation** — returned value can be anywhere within the `α` bound
- **Three-region dispatch**: negative → zero → positive

### base2histogram: `value_at_rank()`

```rust
cumulative += count;
if cumulative >= rank {
    return self.log_scale.interpolate(
        bucket_index, rank - prev, count, prev_count, next_count
    );
}
```

- **Returns**: A single `u64` — interpolated point estimate
- **Trapezoidal interpolation**: models density gradient, solves quadratic for position within bucket
- **Fallback**: returns midpoint for edge buckets or near-uniform density

### Comparison

| Factor | DDSketch | base2histogram |
|--------|----------|----------------|
| Scan direction | Forward | Forward |
| Scan bound | O(non-empty bins) | O(252) fixed at WIDTH=3 |
| Per-bucket scan cost | One float add | One integer add |
| Result type | Geometric midpoint | Interpolated point value |
| Interpolation | None | Trapezoidal (~10 FP ops, once) |
| Negative values | Three-region dispatch | N/A (u64 only) |

## Memory Comparison

DDSketch memory depends on the observed value range. base2histogram memory is fixed.

| Scenario | DDSketch (α=0.01) | base2histogram (WIDTH=3) |
|----------|-------------------|-------------------------|
| Values in [1, 1000] | ~345 bins (~2.7 KB) | 252 buckets (~2 KB) |
| Values in [1, 10^6] | ~690 bins (~5.4 KB) | 252 buckets (~2 KB) |
| Values in [1, 10^9] | ~1035 bins (~8.1 KB) | 252 buckets (~2 KB) |
| Values in [1, 10^18] | ~2070 bins (~16.2 KB) | 252 buckets (~2 KB) |

DDSketch can bound memory via collapsing stores, at the cost of accuracy for extreme quantiles.

To match DDSketch's 1% worst-case guarantee, base2histogram needs WIDTH=8 (~58 KB fixed), but its interpolation achieves <0.3% practical error on smooth distributions even at WIDTH=3 (~2 KB).

## Feature Matrix

| Feature | DDSketch | base2histogram |
|---------|----------|----------------|
| Formal error guarantee | Yes (α) | No (bucket bound only) |
| Runtime precision tuning | Yes | No (compile-time WIDTH) |
| float64 support | Yes | No (u64 only) |
| Negative values | Yes | No |
| Multiple mapping strategies | Yes (3 options) | No (integer-only) |
| Memory-adaptive stores | Yes (dense, paged, collapsing) | No (fixed dense) |
| Trapezoidal interpolation | No | Yes |
| Precomputed lookup tables | No | Yes |
| Small-value cache | No | Yes (4096 entries) |
| Sliding window | No | Yes (built-in slots) |
| Merge support | Yes | Yes |
| Exact summary stats | Yes (Kahan summation) | No |
| Serde/serialization | Yes (protobuf) | No |

## When to Choose Which

**Choose DDSketch when:**
- You need a formal worst-case relative accuracy guarantee
- You work with float64 or negative values
- You need fine-grained precision control (continuous α)
- You need interoperability with Datadog or OpenTelemetry
- Memory can vary with data range (or use collapsing stores)
- You need exact summary statistics (min, max, sum, avg)

**Choose base2histogram when:**
- You need the best practical percentile accuracy for smooth distributions
- You work with u64 values (latencies, sizes, counts)
- You need deterministic fixed memory regardless of data range
- You need maximum recording throughput (pure integer ops)
- You need built-in sliding-window aggregation
- You prefer compile-time memory guarantees with zero dynamic allocation
