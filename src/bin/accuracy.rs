use base2histogram::Histogram;
use base2histogram::LOG_SCALE;

/// Simple xorshift64 PRNG (no external dependency needed).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        assert!(seed != 0);
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Exact percentile from sorted values, using the same rank algorithm as
/// `Histogram::percentile()`: target = ceil(n * p), clamped to >= 1.
fn exact_percentile(sorted: &[u64], p: f64) -> u64 {
    let n = sorted.len() as f64;
    let target = (n * p).ceil().max(1.0) as usize;
    sorted[target - 1]
}

const PERCENTILES: &[(f64, &str)] = &[
    (0.001, "P0.1"),
    (0.01, "P1"),
    (0.05, "P5"),
    (0.10, "P10"),
    (0.25, "P25"),
    (0.50, "P50"),
    (0.75, "P75"),
    (0.90, "P90"),
    (0.95, "P95"),
    (0.99, "P99"),
    (0.999, "P99.9"),
];

/// Evaluate percentile accuracy for one distribution.
/// Returns the max relative error (%).
fn evaluate(name: &str, values: &[u64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();

    let mut hist = Histogram::<()>::new();
    for &v in values {
        hist.record(v);
    }

    println!("\n  {name}");
    println!(
        "  {:<8} {:>14} {:>14} {:>10}",
        "Pctl", "Exact", "Estimated", "Error%"
    );
    println!("  {:-<50}", "");

    let mut max_err = 0.0f64;

    for &(p, label) in PERCENTILES {
        let exact = exact_percentile(&sorted, p);
        let estimated = hist.percentile(p);

        // Error is (exact - estimated) / exact. Positive = underestimate.
        let rel_err = if exact == 0 {
            0.0
        } else {
            (exact as f64 - estimated as f64) / exact as f64 * 100.0
        };

        max_err = max_err.max(rel_err.abs());

        println!("  {:<8} {:>14} {:>14} {:>9.3}%", label, exact, estimated, rel_err);
    }

    println!("  {:-<50}", "");
    println!("  Max relative error: {max_err:.3}%");
    max_err
}

fn main() {
    println!("=== base2histogram Percentile Accuracy Report ===\n");

    let n = 1_000_000usize;
    let mut rng = Rng::new(12345);
    let mut all_max_errors = Vec::new();

    // --- Empirical percentile accuracy ---

    println!("--- Empirical Percentile Accuracy ({n} samples each) ---");

    // 1. Uniform [0, 1_000_000]
    let values: Vec<u64> = (0..n).map(|_| rng.next_u64() % 1_000_001).collect();
    all_max_errors.push(evaluate("Uniform [0, 1_000_000]", &values));

    // 2. Uniform [0, 1000]
    let values: Vec<u64> = (0..n).map(|_| rng.next_u64() % 1_001).collect();
    all_max_errors.push(evaluate("Uniform [0, 1_000]", &values));

    // 3. Powers of 2 (exact bucket boundaries -> zero error expected)
    let values: Vec<u64> = (0..n).map(|_| 1u64 << (rng.next_u64() % 20)).collect();
    all_max_errors.push(evaluate("Powers of 2 [2^0 .. 2^19]", &values));

    // 4. Latency-like: tight cluster with occasional spikes
    let values: Vec<u64> = (0..n)
        .map(|_| {
            let base = 1000 + (rng.next_u64() % 200);
            if rng.next_u64() % 100 < 5 {
                base + rng.next_u64() % 100_000
            } else {
                base
            }
        })
        .collect();
    all_max_errors.push(evaluate("Latency-like (1000+[0,200) + 5% spikes)", &values));

    // 5. Sequential [1..N]
    let values: Vec<u64> = (1..=n as u64).collect();
    all_max_errors.push(evaluate(&format!("Sequential [1..{n}]"), &values));

    // 6. Quadratic (heavy tail): x^2 / 1M
    let values: Vec<u64> = (0..n)
        .map(|_| {
            let x = rng.next_u64() % 1_000_000;
            x.wrapping_mul(x) / 1_000_000
        })
        .collect();
    all_max_errors.push(evaluate("Quadratic (x*x/1M, x in [0,1M))", &values));

    // --- Empirical summary ---

    let overall_max = all_max_errors.iter().cloned().fold(0.0f64, f64::max);
    let overall_avg = all_max_errors.iter().sum::<f64>() / all_max_errors.len() as f64;
    println!("\n--- Empirical Summary ---");
    println!("  Overall max relative error: {overall_max:.3}%");
    println!("  Overall avg of max errors:  {overall_avg:.3}%");

    // --- Theoretical per-bucket error ---

    println!("\n--- Theoretical Per-Bucket Error (WIDTH=3, 252 buckets) ---");
    println!(
        "  {:<8} {:>14} {:>14} {:>14} {:>10}",
        "Bucket", "Min Value", "Max Value", "Width", "Max Err%"
    );
    println!("  {:-<66}", "");

    let num_buckets = LOG_SCALE.num_buckets();
    let mut worst_err = 0.0f64;

    for b in 0..num_buckets {
        let min_val = LOG_SCALE.bucket_min_value(b);
        let max_val = if b + 1 < num_buckets {
            LOG_SCALE.bucket_min_value(b + 1) - 1
        } else {
            u64::MAX
        };
        let width = max_val - min_val + 1;
        let max_err = if max_val == 0 {
            0.0
        } else {
            (max_val - min_val) as f64 / max_val as f64 * 100.0
        };
        worst_err = worst_err.max(max_err);

        // Show first 20 buckets, then every 20th, plus the last
        if b < 20 || b % 20 == 0 || b == num_buckets - 1 {
            println!(
                "  {:<8} {:>14} {:>14} {:>14} {:>9.3}%",
                b, min_val, max_val, width, max_err
            );
        }
    }
    println!("  {:-<66}", "");
    println!("  Worst-case per-value error: {worst_err:.3}%");
}
