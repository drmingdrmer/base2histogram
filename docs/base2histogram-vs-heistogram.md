# base2histogram vs Heistogram: Comparison

Comparison of [base2histogram](https://github.com/drmingdrmer/base2histogram) (Rust) and [Heistogram](https://github.com/oldmoe/heistogram) (C).

Versions compared:
- base2histogram v0.1.5 ([`08eb806`](https://github.com/drmingdrmer/base2histogram/commit/08eb806), 2026-04-03)
- Heistogram ([`bef475a`](https://github.com/oldmoe/heistogram/commit/bef475a), 2025-07-27)

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
| Memory | Variable, grows with value range | Fixed ~2KB per slot |
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

252 buckets covering the entire u64 range, no dynamic allocation after creation. Heistogram grows via `realloc` as larger values arrive — unbounded in principle. base2histogram's fixed layout is predictable and cache-friendly.

### 3. Sliding window with slot-level eviction

Multi-slot architecture with O(bucket_count) eviction of the oldest slot, maintaining running aggregates. Heistogram's `remove()` is per-value, which requires the caller to track individual values — much harder to use for time-windowed aggregation.

### 4. Compile-time configurable resolution

The `const WIDTH` parameter trades bucket count for resolution at compile time. WIDTH=2 gives 128 buckets (less memory, more error); WIDTH=4 gives 504 buckets (more memory, less error). Heistogram's 2% growth factor is hardcoded.

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

### base2histogram: `value_at_rank()`

Scans forward from bucket 0 to 251:

```rust
let mut cumulative = 0u64;
for (bucket_index, &count) in self.aggregate_buckets.iter().enumerate() {
    cumulative += count;
    if cumulative >= rank {
        return self.log_scale.interpolate(
            bucket_index, rank - prev_cumulative, count, prev_count, next_count
        );
    }
}
```

Per-bucket cost:
- One integer addition (`cumulative += count`) — no function calls, no floating point
- Bucket boundary lookup is a precomputed table index: `self.bucket_min_values[i]`

The interpolation (`trapezoidal_t`) involves ~10 floating-point operations (2 divisions, 1 sqrt, multiplies/adds), vs Heistogram's ~3 (one multiply, one subtract, one multiply). But interpolation runs only once — on the target bucket.

### Scan Direction Asymmetry

| Percentile | Heistogram (scans high→low) | base2histogram (scans low→high) |
|------------|----------------------------|--------------------------------|
| P99.9 | Fast (finds target near top) | Slow (must scan ~all 252 buckets) |
| P99 | Fast | Slow-ish |
| P50 | Slow (scans half the buckets) | Medium (scans ~half) |
| P1 | Slow (scans almost all) | Fast (finds target near bottom) |

Neither direction universally wins. For latency monitoring, high percentiles (P99, P99.9) are typically the most queried, giving Heistogram a scan-direction advantage.

However, base2histogram's scan is always bounded at 252 iterations of cheap integer ops, while Heistogram's scan over a variable-capacity array with per-bucket `pow()` calls can be significantly more expensive per iteration.

### Efficiency Summary

| Factor | Heistogram | base2histogram |
|--------|-----------|----------------|
| Scan bound | O(capacity) — unbounded | O(252) — fixed |
| Per-bucket scan cost | Branch + possible `pow()` | One integer add |
| Scan direction | High→low (good for P99) | Low→high (good for P1–P50) |
| Boundary lookup | `ceil(pow(1.02, n))` each time | Precomputed table `[i]` |
| Interpolation cost | 3 FP ops (linear) | ~10 FP ops (trapezoidal + sqrt) |
| Batch queries | No optimization | Shares `total()` computation |
| Serialized query | Yes (zero-alloc) | No |
| Prefix-sum / binary search | No | No |

For a single P99 query, Heistogram's reverse scan finds the answer in fewer iterations. But base2histogram's per-iteration cost is dramatically lower (integer add vs. exponentiation), and its fixed 252-bucket bound means worst-case is still fast. For batch percentile queries (the common case in metrics), base2histogram's cheap scan dominates despite the suboptimal direction for high percentiles.
