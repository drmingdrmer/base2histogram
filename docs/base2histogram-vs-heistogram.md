# base2histogram vs Heistogram: Comparison

Comparison of [base2histogram](https://github.com/drmingdrmer/base2histogram) ([intro](https://blog.openacid.com/algo/histogram/), Rust) and [Heistogram](https://github.com/oldmoe/heistogram) ([intro](https://oldmoe.blog/2025/03/03/heistogram-nanosecond-quantile-queries-for-data-intensive-applications/), C).

Versions compared:
- base2histogram v0.2.2 ([`9b3d181`](https://github.com/drmingdrmer/base2histogram/commit/9b3d181), 2026-04-05)
- Heistogram ([`bef475a`](https://github.com/oldmoe/heistogram/commit/bef475a), 2025-07-27)

## Heistogram Algorithm

Heistogram maps each value `v` to a bucket index using a ~2% growth factor via:

```
index = (int)(log2(v) * 35.0)
```

The multiplier 35 means each power-of-2 range is divided into 35 sub-buckets, giving approximately `1/35 ≈ 2.86%` relative bucket width. Values 0–57 are stored in exact 1:1 buckets (58 exact entries), then logarithmic buckets begin.

- **Recording**: one `log2` (implemented via float conversion or bit tricks) + multiply + floor → O(1).
- **Querying percentile**: linear scan through buckets accumulating counts, then linear interpolation within the target bucket to produce a point estimate.
- **Merging**: 5 merge variants including merge of serialized data without full deserialization.

The bucket array grows dynamically via `realloc` as larger values arrive — unlike fixed-size histograms, memory is proportional to the observed value range. Heistogram also supports inverse queries (`prank`: "what percentile is value X?"), value removal for manual sliding-window patterns, and a compact varint serialization format that allows computing percentiles directly on serialized data.

## Summary

| Strength | Winner |
|----------|--------|
| Percentile accuracy (tail) | base2histogram (trapezoidal interpolation) |
| Memory predictability | base2histogram (fixed 252 buckets) |
| Resolution tuning | base2histogram (compile-time WIDTH) |
| Serialization / wire format | Heistogram (varint, operate-on-serialized) |
| Merge support | Heistogram (5 merge variants) |
| Value removal | Heistogram |
| Inverse queries (prank) | Heistogram |
| Embeddability | Heistogram (single .h file, C ABI) |
| Sliding window | base2histogram (slot-based eviction) |
| Small-value granularity | Heistogram (exact up to 57) |

## Architecture Overview

| Aspect | Heistogram (C) | base2histogram (Rust) |
|--------|---------------|----------------------|
| Bucket mapping | `log2(v) * 35.0` with 2% growth factor | Base-2 log with configurable WIDTH (MSB + offset bits) |
| Small values | Exact 1:1 for values 0–57 | Exact 1:1 for values 0–3 (WIDTH=3) |
| Total buckets | Dynamic (grows via realloc) | Fixed 252 (WIDTH=3), covers full u64 |
| Memory | Variable, grows with value range | Fixed ~2.1 KB per slot (counters + region sums) |
| Interpolation | Linear within bucket | Trapezoidal density estimation using neighbor buckets |

## Heistogram Advantages

### 1. Rich merge & serialization ecosystem

Heistogram provides serialize/deserialize with varint encoding, plus the ability to compute percentiles directly on serialized data (`heistogram_percentile_serialized`) and merge serialized histograms without full deserialization. This is a major win for distributed systems where histograms are stored in databases or sent over the wire — you can merge without decode-merge-encode cycles.

### 2. Value removal

`heistogram_remove()` allows decrementing counts. Useful for sliding-window patterns implemented at the value level rather than slot level.

### 3. Percentile rank and count_upto

Inverse queries: "what percentile is value X?" (`heistogram_prank`) and "how many values ≤ X?" (`heistogram_count_upto`). base2histogram has no public equivalent.

### 4. Finer granularity for small-to-medium values

Exact buckets for values 0–57 (vs. only 0–3 in base2histogram). The 2% growth factor produces narrower buckets in the low-to-mid range, giving better resolution for typical latency values (microseconds to milliseconds).

### 5. Single-header C — trivial to embed

No build system, no dependencies, works in any C/C++ project, embeddable in databases (Redis modules, SQLite extensions, etc.).

## base2histogram Advantages

### 1. Superior interpolation accuracy

Trapezoidal density estimation using neighbor bucket densities models a density gradient:

```
d(t) = d₁ + s·(t − 0.5)
```

This produces better percentile estimates when the distribution is non-uniform within a bucket (which it almost always is in practice, especially at the tails). Heistogram uses simple linear interpolation.

### 2. Fixed, bounded memory

252 buckets covering the entire u64 range, no dynamic allocation after creation. Each slot adds a small region-sum cache (~128 bytes) for fast rank lookup but the total is still fixed at ~2.1 KB per slot. Heistogram grows via `realloc` as larger values arrive — unbounded in principle. base2histogram's fixed layout is predictable and cache-friendly.

### 3. Sliding window with slot-level eviction

Multi-slot architecture with O(bucket_count) eviction of the oldest slot, maintaining running aggregates. Heistogram's `remove()` is per-value, which requires the caller to track individual values — much harder to use for time-windowed aggregation.

### 4. Runtime-configurable resolution

The `WIDTH` parameter (1..=16) trades bucket count for resolution. WIDTH=2 gives 128 buckets (less memory, more error); WIDTH=4 gives 504 buckets (more memory, less error). Shared `LogScale` instances are created once via `LazyLock`. Heistogram's 2% growth factor is hardcoded.

### 5. Precomputed lookup tables + small-value cache

`bucket_min_values` and `small_value_buckets[0..4096]` are computed once at static init. Heistogram recomputes `get_bucket_min` / `get_bucket_max` via `ceil(pow(1.02, n))` on every percentile query — floating-point math in the hot path.

---

## Percentile Calculation Efficiency

### Heistogram: `heistogram_percentile()`

Scans backwards from `capacity - 1` to 0:

```c
double target = ((100.0 - p) / 100.0) * h->total_count;
uint64_t cumsum = 0;
for (int16_t i = h->capacity - 1; i >= 0; i--) {
    if (h->buckets[i].count > 0) {
        if (cumsum + h->buckets[i].count >= target) {
            pos = ((double)(target - cumsum)) / (double)h->buckets[i].count;
            min_val = get_bucket_min(i);
            max_val = get_bucket_max(min_val);
            return max_val - pos * (max_val - min_val);
        }
        cumsum += h->buckets[i].count;
    }
}
```

Per-bucket cost in the hot path:
- `get_bucket_min(i)` calls `ceil(fast_pow_int(1.02, i + 147))` — exponentiation via squaring (log₂(n) multiplications)
- `get_bucket_max(min)` — one multiply + add

Each non-empty bucket visited during the scan triggers a `fast_pow_int` call. For typical histograms with values in the 1K–1M range, bucket IDs reach ~400+, meaning `fast_pow_int(1.02, ~500)` does ~9 multiplies per bucket boundary computation.

### base2histogram: `bucket_at_rank()` + `rank_to_position()`

Uses a two-phase tiered scan via cached region sums (groups of 16 buckets):

```rust
// Phase 1: scan ~16 region sums to find the target region
// Phase 2: scan ~16 buckets within that region
let (bucket, cumulative_before) = slot.bucket_at_rank(rank)?;
let rank_in_bucket = rank - cumulative_before;
interp.rank_to_position(bucket, rank_in_bucket)
```

Per-region/bucket cost:
- One integer addition + one comparison — no function calls, no floating point
- Bucket boundary lookup is a precomputed table index: `self.bucket_min_values[i]`

The tiered scan reduces iterations from 252 to ~32 (16 regions + 16 buckets).
The interpolation (`trapezoidal_cdf`) involves ~10 floating-point operations (2 divisions, 1 sqrt, multiplies/adds), vs Heistogram's ~3 (one multiply, one subtract, one multiply). But interpolation runs only once — on the target bucket.

### Scan Direction Asymmetry

Heistogram scans high→low; base2histogram scans low→high with a tiered
region-sum acceleration layer. The tiered scan means direction matters
less — base2histogram visits ~32 entries regardless of which percentile
is queried, while Heistogram's scan length depends on the percentile.

| Percentile | Heistogram (high→low) | base2histogram (tiered forward) |
|------------|----------------------|-------------------------------|
| P99.9 | Fast (~few buckets) | ~32 iterations |
| P99 | Fast | ~32 iterations |
| P50 | Slow (half the buckets) | ~32 iterations |
| P1 | Slow (almost all) | ~32 iterations |

For log-normal latency data, base2histogram's forward region scan finds
the target in 2-3 region checks because samples cluster in the lower
buckets. Measured at ~10–15 ns per query.

### Efficiency Summary

| Factor | Heistogram | base2histogram |
|--------|-----------|----------------|
| Scan bound | O(capacity) — unbounded | ~32 iterations (tiered regions) |
| Per-bucket scan cost | Branch + possible `pow()` | One integer add |
| Scan direction | High→low (good for P99) | Forward with region-sum skip |
| Boundary lookup | `ceil(pow(1.02, n))` each time | Precomputed table `[i]` |
| Interpolation cost | 3 FP ops (linear) | ~10 FP ops (trapezoidal + sqrt) |
| Batch queries | No optimization | `total()` is O(1) cached |
| Serialized query | Yes (zero-alloc) | No |
| Region-sum acceleration | No | Yes (16-bucket groups) |

base2histogram's tiered scan visits ~32 entries regardless of percentile,
with per-iteration cost of one integer add. Heistogram's reverse scan can
find P99 quickly but pays `pow()` per bucket and has no bound on capacity.
For batch percentile queries, base2histogram's O(1) cached total and cheap
tiered scan dominate.
