# base2histogram Algorithm

A detailed description of the algorithm used in [base2histogram](https://github.com/drmingdrmer/base2histogram).

## Problem

Track request latency distributions in real time with:
- **O(1) recording** — no sorting, no rebalancing, nothing that stalls a hot path
- **Tiny memory** — the system may run hundreds of histograms concurrently
- **Accurate percentile queries** — P50, P95, P99

## Overview

base2histogram is a fixed-size logarithmic histogram.
It maps each `u64` value to one of 252 buckets (at the default WIDTH=3) using pure integer arithmetic,
then estimates percentiles via trapezoidal density interpolation.

The design has three key ideas:

1. **Float-like encoding** for O(1) bucket indexing with no floating-point math
2. **Trapezoidal interpolation** for accurate percentile estimation with no extra storage
3. **CDF-based re-binning** for lossless rescale between different bucket resolutions

## Part 1: Recording — Float-Like Bucket Encoding

### Why Log-Scale Buckets

Latency follows a [log-normal distribution](https://en.wikipedia.org/wiki/Log-normal_distribution)
— a peak at lower values with a [long tail](https://en.wikipedia.org/wiki/Long_tail) to the right.
On a [log scale](https://en.wikipedia.org/wiki/Logarithmic_scale), this becomes a bell curve.

Equal-width buckets waste resolution: tiny buckets where data is sparse, huge gaps where data is dense.
Log-scale buckets — each roughly twice as wide as the previous — match the shape of the data:

```
[0,1), [1,2), [2,4), [4,8), [8,16), ...
```

Mapping a value to its bucket takes a single [leading zero count](https://en.wikipedia.org/wiki/Find_first_set#CLZ) instruction. 65 buckets cover the entire u64 range. But the resolution is too coarse — the last bucket spans half of all possible values.

### The Float-Like Encoding

The solution: treat each bucket's lower bound as a tiny
[floating-point number](https://en.wikipedia.org/wiki/Floating-point_arithmetic).
The [MSB](https://en.wikipedia.org/wiki/Bit_numbering#Most_significant_bit) position gives the exponent
(which group of buckets), and the next `WIDTH-1` bits give the offset within that group.

With WIDTH=3 (default), a bucket boundary looks like this in binary:

```
00..00 1 xx 00..00
       |
       MSB
```

The leading `1` picks the group. The two bits that follow pick the bucket within the group.
It is a 3-bit float: 1 implicit leading bit + 2 fractional bits.

### Bucket Layout

With WIDTH=3, each group contains `2^(WIDTH-1) = 4` buckets.
The first two groups (values 0–7) are exact 1:1 mappings.
From group 2 onward, bucket widths double with each group:

```
WIDTH = 3:

range     bucket index        bucket size
[0, 1)     0  0b0 ..... 000    1
[1, 2)     1  0b0 ..... 001    1
[2, 3)     2  0b0 ..... 010    1
[3, 4)     3  0b0 ..... 011    1

[4, 5)     4  0b0 ..... 100    1
[5, 6)     5  0b0 ..... 101    1
[6, 7)     6  0b0 ..... 110    1
[7, 8)     7  0b0 ..... 111    1

[8, 10)    8  0b0 .... 1000    2
[10, 12)   9  0b0 .... 1010    2
[12, 14)  10  0b0 .... 1100    2
[14, 16)  11  0b0 .... 1110    2

[16, 20)  12  0b0 ... 10000    4
[20, 24)  13  0b0 ... 10100    4
[24, 28)  14  0b0 ... 11000    4
[28, 32)  15  0b0 ... 11100    4

[32, 40)  16  0b0 .. 100000    8
[40, 48)  17  0b0 .. 101000    8
[48, 56)  18  0b0 .. 110000    8
[56, 64)  19  0b0 .. 111000    8
```

### Bucket Index Calculation

For a value `v`:

1. If `v < GROUP_SIZE` (4 for WIDTH=3): `bucket_index = v` (direct mapping)
2. Otherwise, extract the top WIDTH bits:

```
bits_upto_msb = 64 - leading_zeros(v)
group_index   = bits_upto_msb - WIDTH
offset        = (v >> group_index) & MASK
bucket_index  = GROUP_SIZE + group_index * GROUP_SIZE + offset
```

Walk-through with value = 42:

```
value = 42 (binary: 0b101010)
  MSB position: 5
  group_index: 5 - 2 = 3
  2 bits after MSB: 01 (from 1[01]010)
  offset in group: 1
  Bucket index: 4 + (3 × 4) + 1 = 17
```

The entire operation is `leading_zeros` + two shifts + a mask + two adds.
No floating-point, no division, no `log()`. Recording is O(1).

### Performance Optimizations

- **Small-value cache**: a precomputed lookup table for values 0–4095, returning the bucket index in a single memory access
- **Precomputed boundaries**: `bucket_min_values[252]` stores the left boundary of every bucket, computed once at static initialization

### Tuning WIDTH

WIDTH sets how many sub-buckets each group gets (`2^(WIDTH-1)`).
Groups top out at 64 (covering the full u64 range).

| WIDTH | Buckets per group | Total buckets | Memory/slot |
|-------|-------------------|---------------|-------------|
| 1     | 1                 | 65            | 520 B       |
| 2     | 2                 | 128           | 1.0 KB      |
| 3     | 4                 | 252           | 2.0 KB      |
| 4     | 8                 | 496           | 3.9 KB      |
| 5     | 16                | 976           | 7.6 KB      |
| 6     | 32                | 1,920         | 15.0 KB     |

Total bucket count: `2^(WIDTH-1) × (66 - WIDTH)`.
Memory per slot: `total_buckets × 8` bytes (u64 counters).

## Part 2: Percentile Estimation — Trapezoidal Interpolation

### Locating the Target Bucket

For a percentile `p` (e.g., 0.99 for P99):

1. Compute total sample count
2. Compute target rank: `rank = ceil(total × p)`
3. Scan buckets from index 0, accumulating counts
4. When cumulative count ≥ rank, the current bucket contains the target

This scan is O(bucket_count) — 252 iterations at WIDTH=3.

### The Problem: Bucket Resolution

A bucket spans a range, not a point. The target value is somewhere inside the bucket, but where? Three approaches, from rough to precise:

**Midpoint**: return `(left + right) / 2`. A blind guess that ignores sample distribution within the bucket.

**Uniform interpolation**: assume samples are evenly spread, then interpolate linearly: `estimate = left + width × rank_in_bucket / count`. Better than midpoint — at least it uses where in the bucket the target rank falls.

**Trapezoidal interpolation** (our approach): model a density gradient using neighbor buckets. Two orders of magnitude more accurate, with zero additional storage.

Measured error on log-normal distribution (API latency scenario), WIDTH=3, 1,000,000 samples:

| Method | P50 | P95 | P99 |
|--------|-----|-----|-----|
| midpoint | 5.018% | 7.732% | 4.861% |
| trapezoidal | 0.000% | 0.080% | 0.086% |

### How Trapezoidal Interpolation Works

Uniform interpolation treats the density inside a bucket as flat.
In reality, it is sloped — denser on the side closer to the peak of the distribution.

If we know which way the density tilts, we can swap the rectangle for a trapezoid and land much closer to the true value.

Each bucket stores only a count. The slope information comes from the **neighbors**.

#### Setup

Consider three adjacent buckets:

```
  bucket:  [i-1]      [i]        [i+1]
  range:   [x0,x1)    [x1,x2)    [x2,x3)
  width:     w0          w1          w2
  count:     c0          c1          c2
```

Compute average density in each:

```
d0 = c0 / w0       (left neighbor density)
d1 = c1 / w1       (target bucket density)
d2 = c2 / w2       (right neighbor density)
```

Midpoints of the left and right buckets:

```
m0 = (x0 + x1) / 2
m2 = (x2 + x3) / 2
```

#### Density Slope

Assume density varies linearly from `m0` to `m2`. The slope:

```
k = (d2 - d0) / (m2 - m0)
```

For the first bucket, uses (self, right neighbor). For the last bucket, uses (left neighbor, self).

#### Density Slope Clamping

A steep slope from distant neighbors can produce negative density at a bucket edge, which is physically meaningless. The clamped slope ensures non-negative density across the entire bucket width:

```
d(x) = a + k·x,  x ∈ [0, w]
```

where `a = d1 - k·w/2` is the density at the left edge.

The constraints are:
- Left edge:  `d1 - k·w/2 ≥ 0`  →  `k ≤ 2·d1/w`
- Right edge: `d1 + k·w/2 ≥ 0`  →  `k ≥ -2·d1/w`

If the raw slope violates either bound, it is clamped to the tightest limit.

#### Solving for the Percentile Position

Let `x ∈ [0, w]` be the offset from the bucket's left edge (where `w` is the bucket width).

The density inside the bucket is a linear function:

```
d(x) = a + k·x
```

where `a = d1 - k·w/2` is the density at the left edge, and `k` is the clamped density slope. This density is anchored so that the midpoint density equals the bucket's average density `d1` (the midpoint of a linear function always equals its average).

The CDF within the bucket:

```
C(x) = a·x + k·x²/2
```

The total area `C(w) = count`, confirming the density integrates to the bucket's sample count.

To find the target position, solve `C(x) = rank` where `rank` is the 1-based rank within the bucket:

```
k/2 · x² + a · x - rank = 0
```

Solving via the quadratic formula:

```
x = (-a + √(a² + 2·k·rank)) / k
```

The final estimate:

```
value = left + floor(x),  x clamped to [0, w]
```

#### Fallback Cases

- **Single-count or width-1 bucket**: return the midpoint
- **Near-zero slope** (`|k·w| < |d1| × 10⁻⁹`): effectively uniform → `x = rank / d1`
- **Negative discriminant**: fall back to `x = rank / d1`

## Error Bound

### Theoretical Bound

For a group with bit-length `b` (values in `[2^(b-1), 2^b)`):

- Range spans `2^(b-1)` values
- Divided into `2^(WIDTH-1)` sub-buckets
- Each bucket width = `2^(b-1) / 2^(WIDTH-1)` = `2^(b - WIDTH)`

Worst-case relative error at the smallest value in a group:

```
relative_error = bucket_width / min_value
               = 2^(b - WIDTH) / 2^(b - 1)
               = 1 / 2^(WIDTH - 1)
```

The `b` cancels out — the ratio is constant across all groups. This is the fundamental property of logarithmic bucketing: as values double, bucket width doubles too, so relative error stays constant.

If the percentile returns the **midpoint** of the bucket, the max error halves to `1 / 2^WIDTH`.

| WIDTH | Max error (full bucket) | Max error (midpoint) |
|-------|------------------------|---------------------|
| 3     | 1/4 = 25%              | 1/8 = 12.5%         |
| 4     | 1/8 = 12.5%            | 1/16 = 6.25%        |
| 5     | 1/16 = 6.25%           | 1/32 = 3.125%       |
| 6     | 1/32 = 3.125%          | 1/64 = 1.5625%      |
| 8     | 1/128 = 0.781%         | 1/256 = 0.391%      |

### Practical Accuracy

Trapezoidal interpolation reduces practical error well below the theoretical bound for smooth distributions.

Measured with 1,000,000 samples per distribution, WIDTH=3 (252 buckets, ~2.1 KB per slot):

```
                   P50      P95      P99
─────────────────────────────────────────
LN-API  (σ=0.5)  0.000%   0.080%   0.086%    API / microservice latency
LN-DB   (σ=1.0)  0.034%   0.039%   0.187%    database query latency
Expon             0.000%   0.000%   0.824%    network / IO waits
Bimodal           0.394%   0.012%   0.543%    cache hit/miss (90/10)
Pareto  (α=1.5)  0.633%   0.000%   0.231%    heavy-tailed request sizes
Uniform           0.012%   1.035%   3.706%    synthetic benchmark
Sequent           0.000%   1.011%   3.696%    adversarial worst case
```

For the latency distributions that matter most (LN-API, LN-DB), WIDTH=3 delivers sub-0.2% error on ~2 KB of memory.

## Data Structures

### `Histogram<T>`

The main struct. `T` is optional user metadata per slot (defaults to `()`).

```rust
pub struct Histogram<T = ()> {
    log_scale: &'static LogScale,
    slots: SlotQueue<T>,
    aggregate: Slot<T>,
}
```

- `log_scale`: shared reference to precomputed lookup tables
- `slots`: historical time-window slots (up to `slot_limit - 1`)
- `aggregate`: running sum of all slot buckets (`.buckets`), cached total (`.total`), and current period metadata (`.data`)

### `LogScale`

Precomputed lookup tables for O(1) bucket operations.

```rust
pub struct LogScale {
    config: LogScaleConfig,
    bucket_min_values: Vec<u64>,
    small_value_buckets: Vec<u8>,
}
```

- `config`: bucket math parameters (width, group_size, mask, total bucket count)
- `bucket_min_values[i]`: left boundary of bucket `i`
- `small_value_buckets[v]`: cached bucket index for values 0–4095

Width is a runtime parameter (1..=16). Shared static instances are initialized once via `LazyLock` and accessed through `LogScale::get(width)`.

### `LogScaleConfig`

Configuration for logarithmic bucket boundaries.

```rust
pub struct LogScaleConfig {
    width: usize,
    group_size: usize,
    mask: u64,
    buckets: usize,
    small_value_cache_size: usize,
}
```

- `width`: bit-width parameter (1..=16). Each bucket group uses `width` bits: 1 MSB + (width-1) offset bits
- `group_size`: buckets per group = `2^(width-1)`
- `mask`: bitmask for offset extraction = `group_size - 1`
- `buckets`: total bucket count = `group_size × (66 - width)`
- `small_value_cache_size`: number of cached small-value lookups (4096)

### `Slot<T>`

A bucket array with cached aggregates, used both as the aggregate
accumulator and as individual time-window snapshots.

```rust
pub struct Slot<T> {
    buckets: Vec<u64>,
    region_sums: Vec<u64>,
    total: u64,
    data: Option<T>,
}
```

- `buckets`: per-bucket sample counts
- `region_sums`: cached sum for each group of 16 buckets, enabling tiered rank lookup
- `total`: cached total sample count across all buckets
- `data`: optional user metadata (e.g., timestamp, label)

Key methods: `record_n()`, `subtract()`, `clear()`, `bucket_at_rank()`.
`bucket_at_rank()` uses a two-phase scan — first over `region_sums`
(~16 entries), then within the matching region (~16 buckets) — reducing
percentile lookup from O(bucket_count) to O(bucket_count / 16).

### `Interpolator`

Stateless interpolation engine operating on bucket counts and geometry.

```rust
pub struct Interpolator<'a> {
    log_scale: &'a LogScale,
    buckets: &'a [u64],
}
```

Independent of any particular histogram instance — constructed from a `&LogScale` and a `&[u64]` bucket slice.

### `CumulativeCount`

Incremental cursor for computing cumulative counts at monotonically increasing positions.

```rust
pub struct CumulativeCount<'a> {
    interpolator: Interpolator<'a>,
    bucket_index: usize,
    accumulated: u64,
}
```

- `bucket_index`: current scan position, advances forward only
- `accumulated`: sum of whole-bucket counts before `bucket_index`

Each `count_below(position)` call resumes from where the previous call left off, giving O(1) amortized cost when queries are monotonically increasing. Used by the [rescale algorithm](#part-3-rescale--cdf-based-re-binning) to efficiently redistribute counts across bucket boundaries.

### `BucketSpan` / `BucketRef`

Lazy references to bucket geometry. `BucketSpan` provides `left()`, `right()`, `width()`, `midpoint()` via the LogScale lookup table. `BucketRef` adds the `count` field.

## Sliding Window

The histogram supports multiple slots for time-windowed aggregation.
With `slot_limit = N`, only `N - 1` slots are physically stored.
The current (newest) period is never materialized — its counts are derived on the fly as `aggregate - Σ stored`:

```
stored slots:  [a, b, c]              (N - 1 historical periods)
aggregate:     agg = a + b + c + d    (running total across all N periods)
implicit:      d = agg - a - b - c    (current period, not stored)
```

`record()` only touches the `aggregate`; individual slot writes are avoided entirely.

### Slot Rotation (`advance()`)

`advance(data)` materializes the implicit current period into a stored
historical slot, then starts a new empty current period. Returns the
evicted slot's metadata (`Option<T>`) if a slot was evicted, or `None`
during warmup:

**At capacity** (queue full):
```
1. evict oldest:  agg ← agg - a          (subtract oldest from aggregate)
2. materialize:   a   ← agg - b - c      (reuse evicted Vec for current period)
3. push:          slots: [b, c, a']       (a' now holds d's counts)
```

**During warmup** (queue not full):
```
1. allocate new Vec
2. materialize:   new ← agg - Σ stored
3. push:          slots: [...existing, new]
```

The evicted slot's `Vec<u64>` allocation is reused, avoiding repeated allocation.

This gives efficient sliding-window metrics without requiring the caller to track individual values or full per-window histograms.

## Part 3: Rescale — CDF-Based Re-Binning

Rescaling converts a histogram from one WIDTH to another, preserving all slots and the total sample count.

### Problem

Different WIDTHs produce different bucket boundaries.
A source bucket `[s_left, s_right)` may straddle multiple destination buckets.
Simply moving whole counts between buckets would lose resolution — the information about where samples fall *within* a bucket matters.

### Solution: CDF Transfer

For each destination bucket `[d_left, d_right)`, estimate how many source samples fall in that range using the trapezoidal CDF:

```
dst_count[i] = CDF(d_right) - CDF(d_left)
```

where `CDF(x)` is the estimated count of samples below `x`, computed via trapezoidal interpolation over the source buckets.

### Algorithm

```
1. Create a CumulativeCount cursor over the source buckets
2. prev_cdf ← 0
3. For each destination bucket i:
     cdf_right ← cursor.count_below(dst_bucket[i].right())
     dst_count[i] ← round(cdf_right) - prev_cdf
     prev_cdf ← round(cdf_right)
```

The `CumulativeCount` cursor maintains scan state, so each `count_below()` call resumes from the previous position — O(src_buckets) total work across all destination buckets, not O(src_buckets) per destination bucket.

The `round()` preserves integer totals: since each destination count is derived from the difference of rounded CDF values, the sum of all destination counts equals the original total.

### Rescale Scope

`rescale()` re-bins every stored component independently:
- The aggregate slot (running sum of all periods)
- Each historical slot in the queue

Each rebinned slot recomputes its `total` and `region_sums` from the new
buckets via `Slot::from_buckets()`. Slot metadata (`T`) is cloned.

## Complexity

| Operation | Time | Space |
|-----------|------|-------|
| Record a value | O(1) | — |
| Percentile query | O(bucket_count / 16) | — |
| Advance slot | O(bucket_count) | — |
| Rescale | O(src_buckets + dst_buckets) per slot | O(dst_buckets) |
| Memory per slot | — | `bucket_count × 8 + region_count × 8 + 8` bytes |
| Total (WIDTH=3, 1 slot) | — | ~2.1 KB |

Percentile queries use the tiered region scan in `bucket_at_rank()`:
~16 region sums + ~16 buckets = ~32 iterations instead of 252.
Measured at ~10–15 ns per query on 1M log-normal samples.
