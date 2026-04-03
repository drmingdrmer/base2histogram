# base2histogram vs H2 Histogram: Comparison

Comparison of [base2histogram](https://github.com/drmingdrmer/base2histogram) ([intro](https://blog.openacid.com/algo/histogram/), Rust) and [H2 Histogram](https://github.com/iopsystems/histogram) ([intro](https://iop.systems/blog/h2-histogram/), Rust).

Versions compared:
- base2histogram v0.1.5 ([`08eb806`](https://github.com/drmingdrmer/base2histogram/commit/08eb806), 2026-04-03)
- H2 Histogram v1.0.1-alpha.0 ([`a6958d8`](https://github.com/iopsystems/histogram/commit/a6958d8), 2026-03-20)

## H2 Histogram Algorithm

H2 Histogram uses the same base-2 log-linear bucketing as base2histogram. Each value `v` is mapped to a bucket index using the position of its most significant bit (MSB) and the next `grouping_power` bits:

```
group = msb_position - grouping_power
offset = (v >> group) & mask
index = exact_count + group * group_size + offset
```

The `grouping_power` parameter (equivalent to `WIDTH - 1` in base2histogram) controls how many sub-buckets exist per power-of-2 range. At `grouping_power = 2`, each doubling of value range is split into 4 sub-buckets, giving 252 total buckets — identical to base2histogram at WIDTH=3.

- **Recording**: `leading_zeros` + shift + offset → O(1), pure integer arithmetic.
- **Querying percentile**: forward scan through buckets accumulating counts; returns the bucket range `lower..=upper` rather than a point estimate.
- **Merging**: `checked_add` / `wrapping_add` on corresponding bucket counts.

H2 provides values 0–`2^(grouping_power+1)-1` with exact 1:1 buckets (8 exact buckets at `grouping_power=2`), then logarithmic buckets beyond that. It also offers `AtomicHistogram` for lock-free concurrent recording and `SparseHistogram` for memory-efficient storage of sparse distributions.

## Summary

These two histograms share nearly identical bucket mapping math — both use base-2 log-linear schemes with the same total bucket count at equivalent configurations. The differences lie in what each builds on top of that shared foundation.

| Strength | Winner |
|----------|--------|
| Percentile accuracy | base2histogram (trapezoidal interpolation) |
| Concurrent recording | H2 Histogram (AtomicHistogram) |
| Sparse data | H2 Histogram (SparseHistogram) |
| Sliding window | base2histogram (slot-based eviction) |
| Downsampling | H2 Histogram (reduce grouping_power) |
| Recording hot-path | Tie (both use LZCNT + shifts, no FP) |
| Memory predictability | Tie (both fixed at equivalent configs) |
| Resolution tuning | Tie (both compile/init-time configurable) |
| Merge support | H2 Histogram (checked/wrapping add/sub) |
| Precomputed lookups | base2histogram (table-driven boundaries) |

## Architecture Overview

| Aspect | H2 Histogram | base2histogram |
|--------|-------------|----------------|
| Bucket mapping | `leading_zeros` + shift + offset | `leading_zeros` + shift + offset |
| Precision parameter | `grouping_power` (0–62) | `WIDTH` (1–6+), compile-time const |
| Parameter equivalence | `grouping_power = g` | `WIDTH = g + 1` |
| Small-value exact range | 0 to `2^(g+1) - 1` | 0 to `2^(WIDTH-1) - 1` |
| Total buckets (u64 range) | `2^(g+1) + (64-g-1)·2^g` | `2^(WIDTH-1) · (66 - WIDTH)` |
| At g=2 / WIDTH=3 | 252 buckets | 252 buckets |
| Memory (g=2 / WIDTH=3) | ~2 KB | ~2 KB |
| Boundary computation | Computed on demand (shifts + adds) | Precomputed table lookup |
| Interpolation | None — returns bucket range | Trapezoidal density estimation |

### Parameter Equivalence

The two schemes produce the same bucket boundaries at the same total bucket count. The mapping is:

```
H2:             grouping_power = g,  max_value_power = n
base2histogram: WIDTH = g + 1
```

At `g=2` (H2) / `WIDTH=3` (base2histogram), both produce exactly 252 buckets covering the full u64 range. The only structural difference: H2 allocates more exact buckets for small values (8 vs 4), borrowing from the logarithmic region.

| Config | Exact buckets | Log buckets | Total |
|--------|--------------|-------------|-------|
| H2 (g=2, n=64) | 8 (values 0–7) | 244 | 252 |
| base2histogram (WIDTH=3) | 4 (values 0–3) | 248 | 252 |

## H2 Histogram Advantages

### 1. AtomicHistogram for lock-free concurrent recording

`AtomicHistogram` provides lock-free `increment()` and `add()` via `AtomicU64`, with `load()` for non-destructive snapshots and `drain()` for read-and-reset. base2histogram has no concurrent variant.

### 2. SparseHistogram for low-cardinality data

`SparseHistogram` stores only non-zero buckets using parallel index/count vectors — `80 + 12·k` bytes for `k` occupied buckets. Useful when most of the 252+ buckets are empty (e.g., narrow value distributions). Supports full merge operations and converts to/from the dense `Histogram`.

### 3. Downsampling

`downsample(target_grouping_power)` reduces precision to shrink a histogram post-hoc, halving bucket count per step. Useful for tiered storage — collect at high resolution, downsample for archival. base2histogram's WIDTH is a compile-time constant and cannot change.

### 4. Richer merge API

Four merge variants: `checked_add`, `wrapping_add`, `checked_sub`, `wrapping_sub` — each with proper overflow/underflow semantics. Works across Histogram and SparseHistogram types.

### 5. More exact buckets for small values

At equivalent total bucket count (g=2 vs WIDTH=3), H2 provides exact buckets for values 0–7 vs only 0–3 in base2histogram. This gives better small-value resolution without increasing total memory.

### 6. Optional serde + JSON Schema support

Feature-gated `serde` serialization and `schemars` JSON Schema generation — ready for wire protocols and config-driven systems.

## base2histogram Advantages

### 1. Trapezoidal interpolation for superior percentile accuracy

H2 returns the raw bucket range for a percentile query — the caller gets `RangeInclusive<u64>` and must interpret it. base2histogram models a density gradient across the target bucket using neighbor densities:

```
d(t) = d₁ + s·(t − 0.5)
```

then solves the CDF inverse to place the estimate within the bucket. This produces significantly better percentile estimates for non-uniform distributions, especially at tails (P99, P99.9) where bucket widths are large.

| Distribution | H2 (bucket midpoint) | base2histogram (trapezoidal) |
|-------------|---------------------|------------------------------|
| Uniform | Acceptable | Better |
| Log-normal (P99) | Bucket range only | Interpolated estimate |
| Bimodal (P99) | Bucket range only | Interpolated estimate |

### 2. Precomputed lookup tables

`bucket_min_values[252]` and `small_value_buckets[4096]` are computed once at static initialization. H2 recomputes `index_to_lower_bound` / `index_to_upper_bound` with shifts and adds on every call. Both are cheap integer operations, but the table lookup avoids recomputation entirely — one memory access vs several instructions.

### 3. Sliding window with slot-level eviction

Multi-slot architecture maintains per-slot bucket arrays plus a running aggregate. `advance()` evicts the oldest slot in O(bucket_count) — one subtraction per bucket. H2's `checked_sub` can subtract histograms but requires the caller to manage the window and retain full per-window histograms externally.

### 4. Compile-time WIDTH optimization

WIDTH is a const generic, so all derived constants (GROUP_SIZE, MASK, BUCKETS) are computed at compile time with zero runtime overhead. The compiler can inline and optimize the entire bucket calculation path. H2's `grouping_power` is a runtime parameter stored in `Config`, requiring runtime field access.

## Percentile Calculation

### H2: `percentiles()`

Forward scan (0 → end), sorted-percentile single-pass optimization:

```rust
let count = (percentile * total_count as f64).ceil() as u128;
// scan buckets until partial_sum >= count
// return Bucket { count, range: lower..=upper }
```

- **Returns**: The entire bucket (range + count), not a point estimate
- **Batch optimization**: Sorts requested percentiles, scans once for all of them — O(buckets + percentiles)
- **Per-bucket cost**: One integer add + one comparison
- **Boundary cost**: `index_to_lower_bound()` / `index_to_upper_bound()` computed per result (several shifts and adds)

### base2histogram: `value_at_rank()`

Forward scan (0 → 251):

```rust
cumulative += count;
if cumulative >= rank {
    return self.log_scale.interpolate(bucket_index, rank - prev, count, prev_count, next_count);
}
```

- **Returns**: A single u64 value — interpolated point estimate
- **Batch optimization**: Shares `total()` computation across percentile queries
- **Per-bucket cost**: One integer add + one comparison (identical to H2)
- **Boundary cost**: `bucket_min_values[i]` — table lookup
- **Interpolation cost**: ~10 FP ops (runs once on the target bucket)

### Comparison

| Factor | H2 Histogram | base2histogram |
|--------|-------------|----------------|
| Scan direction | Forward (0 → end) | Forward (0 → end) |
| Scan bound | O(total_buckets) — configurable | O(252) — fixed at WIDTH=3 |
| Per-bucket scan cost | One integer add | One integer add |
| Result type | Bucket range | Interpolated point value |
| Boundary lookup | Computed (shifts + adds) | Precomputed table |
| Interpolation | None | Trapezoidal (~10 FP ops, once) |
| Batch percentiles | Sorted single-pass O(B+P) | Independent queries |
| Sparse percentiles | Yes (skip zero buckets) | No |

For a single percentile query both perform an O(buckets) scan with the same per-bucket cost. H2 wins on batch queries with its sorted single-pass optimization. base2histogram wins on result quality — returning a precise point estimate instead of a bucket range.

## Feature Matrix

| Feature | H2 Histogram | base2histogram |
|---------|-------------|----------------|
| Dense histogram | Yes | Yes |
| Sparse histogram | Yes | No |
| Atomic histogram | Yes (lock-free) | No |
| Sliding window | No (manual via sub) | Yes (built-in slots) |
| Merge (add) | checked + wrapping | No |
| Merge (sub) | checked + wrapping | No |
| Downsampling | Yes | No |
| Value removal | Via checked_sub | Via slot eviction |
| Interpolation | None (returns range) | Trapezoidal density |
| Percentile rank (inverse) | No | No |
| Serde serialization | Yes (feature-gated) | No |
| JSON Schema | Yes (feature-gated) | No |
| Configurable at runtime | Yes (grouping_power) | No (compile-time WIDTH) |
| Precomputed boundaries | No | Yes |
| Small-value cache | No | Yes (4096 entries) |

## Memory Comparison

At equivalent precision levels (same total buckets):

| Precision | H2 (grouping_power) | base2histogram (WIDTH) | Buckets | Memory (u64 counters) |
|-----------|---------------------|----------------------|---------|----------------------|
| 25% | g=2 | WIDTH=3 | 252 | ~2 KB |
| 12.5% | g=3 | WIDTH=4 | 504 | ~4 KB |
| 6.25% | g=4 | WIDTH=5 | 976 | ~8 KB |
| 3.13% | g=5 | WIDTH=6 | 1,920 | ~15 KB |
| 0.781% | g=7 | WIDTH=8 | 7,424 | ~58 KB |

Both have identical memory at the same precision level. H2 additionally offers SparseHistogram (`80 + 12k` bytes for `k` non-zero buckets) for memory savings on sparse data.

## When to Choose Which

**Choose H2 Histogram when:**
- You need concurrent recording (AtomicHistogram)
- Your data is sparse and memory matters (SparseHistogram)
- You need histogram merging in distributed systems
- You want runtime-configurable precision
- You need serialization for storage or wire transfer
- Bucket-range results are sufficient (no interpolation needed)

**Choose base2histogram when:**
- You need accurate point estimates for percentiles, not just bucket ranges
- You need built-in sliding-window aggregation
- You want maximum single-thread recording throughput (precomputed tables)
- You prefer zero-allocation, compile-time-fixed memory footprint
