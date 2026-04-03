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

The design has two key ideas:

1. **Float-like encoding** for O(1) bucket indexing with no floating-point math
2. **Trapezoidal interpolation** for accurate percentile estimation with no extra storage

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

#### Solving for the Percentile Position

Normalize the bucket to `t ∈ [0, 1]` where `t=0` is the left edge and `t=1` is the right edge.

The density across the bucket is a linear function:

```
d(t) = d1 + s · (t - 0.5)
```

where `s = k · w1` is the total density change across the bucket. This density is anchored so that the midpoint density equals the bucket's average density `d1` (the midpoint of a linear function always equals its average).

The CDF within the bucket:

```
C(t) = (d1 - s/2) · t + s · t² / 2
```

The total area `C(1) = d1`, confirming the density integrates to the bucket's average density.

To find the target position, solve `C(t) = f · d1` where `f = rank_in_bucket / count`:

```
s/2 · t² + (d1 - s/2) · t - f · d1 = 0
```

Let `a = d1 - s/2` (density at the left edge). Then:

```
t = (-a + √(a² + 2 · s · f · d1)) / s
```

The final estimate:

```
value = left + width × t
```

#### Fallback Cases

- **Edge buckets** (first or last): no neighbor on one side → use uniform interpolation (`t = f`)
- **Single-count or width-1 bucket**: return the midpoint
- **Equal neighbor counts** (`c0 == c2`): density is uniform → `t = f`
- **Near-zero slope** (`|s| < |d1| × 10⁻⁹`): effectively uniform → `t = f`
- **Negative discriminant**: fall back to `t = f`

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

Measured with 1,000,000 samples per distribution, WIDTH=3 (252 buckets, 2 KB):

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

For the latency distributions that matter most (LN-API, LN-DB), WIDTH=3 delivers sub-0.2% error on 2 KB of memory.

## Data Structures

### `Histogram<T, WIDTH>`

The main struct. `T` is optional user metadata per slot. `WIDTH` is a compile-time const generic.

```rust
pub struct Histogram<T = (), const WIDTH: usize = 3> {
    log_scale: &'static LogScale<WIDTH>,
    slots: SlotQueue<T>,
    aggregate_buckets: Vec<u64>,
}
```

- `log_scale`: shared reference to precomputed lookup tables
- `slots`: circular buffer of time-window slots
- `aggregate_buckets`: running sum of all slot buckets, maintained incrementally

### `LogScale<WIDTH>`

Precomputed lookup tables for O(1) bucket operations.

```rust
pub struct LogScale<const WIDTH: usize> {
    bucket_min_values: Vec<u64>,
    small_value_buckets: Vec<u8>,
}
```

- `bucket_min_values[i]`: left boundary of bucket `i`
- `small_value_buckets[v]`: cached bucket index for values 0–4095

A shared static instance `LOG_SCALE` is initialized once via `LazyLock`.

### `Slot<T>`

An individual time window in the sliding-window architecture.

```rust
pub struct Slot<T> {
    buckets: Vec<u64>,
    data: Option<T>,
}
```

### `BucketSpan<WIDTH>` / `BucketRef<WIDTH>`

Lazy references to bucket geometry. `BucketSpan` provides `left()`, `right()`, `width()`, `midpoint()` via the LogScale lookup table. `BucketRef` adds the `count` field.

## Sliding Window

The histogram supports multiple slots for time-windowed aggregation. Each slot has independent bucket counts. The `aggregate_buckets` array is the sum of all active slots, maintained incrementally.

When `advance()` is called:
1. If the slot queue is full, evict the oldest slot by subtracting its buckets from the aggregate — O(bucket_count)
2. Push a new empty slot

This gives efficient sliding-window metrics without requiring the caller to track individual values or full per-window histograms.

## Complexity

| Operation | Time | Space |
|-----------|------|-------|
| Record a value | O(1) | — |
| Percentile query | O(bucket_count) | — |
| Advance slot | O(bucket_count) | — |
| Memory per slot | — | `bucket_count × 8` bytes |
| Total (WIDTH=3, 1 slot) | — | ~2 KB |
