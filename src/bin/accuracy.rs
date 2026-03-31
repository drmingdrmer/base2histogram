use std::f64::consts::PI;

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

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal via Box-Muller transform.
    fn next_normal(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-15);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
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
        "  {:<10} {:>14} {:>14} {:>10}",
        "Percentile", "Exact", "Estimated", "Error%"
    );
    println!("  {:-<52}", "");

    let mut max_err = 0.0f64;

    for &(p, label) in PERCENTILES {
        let exact = exact_percentile(&sorted, p);
        let estimated = hist.percentile(p);

        let rel_err = if exact == 0 {
            0.0
        } else {
            (exact as f64 - estimated as f64) / exact as f64 * 100.0
        };

        max_err = max_err.max(rel_err.abs());

        println!("  {:<10} {:>14} {:>14} {:>9.3}%", label, exact, estimated, rel_err);
    }

    println!("  {:-<52}", "");
    println!("  Max relative error: {max_err:.3}%");
    max_err
}

fn main() {
    println!("=== base2histogram Percentile Accuracy Report ===\n");

    let n = 1_000_000usize;
    let mut rng = Rng::new(12345);
    let mut all_max_errors = Vec::new();

    println!("--- Empirical Percentile Accuracy ({n} samples each) ---");

    // 1. Uniform [0, 1_000_000] — baseline
    let values: Vec<u64> = (0..n).map(|_| rng.next_u64() % 1_000_001).collect();
    all_max_errors.push(evaluate("Uniform [0, 1_000_000]", &values));

    // 2. Log-normal — typical API/service latency in microseconds mu=7 (median ~1100us), sigma=0.5
    //    (moderate spread) Realistic: most requests ~1ms, tail extends to ~10ms
    let values: Vec<u64> = (0..n)
        .map(|_| {
            let z = rng.next_normal();
            (7.0 + 0.5 * z).exp() as u64
        })
        .collect();
    all_max_errors.push(evaluate("Log-normal (median~1100us, API latency)", &values));

    // 3. Bimodal — cache hit/miss pattern 90% fast path ~500us, 10% slow path ~50ms
    let values: Vec<u64> = (0..n)
        .map(|_| {
            if rng.next_u64() % 100 < 90 {
                // Fast path: normal around 500us, sigma=50
                (500.0 + 50.0 * rng.next_normal()).max(1.0) as u64
            } else {
                // Slow path: normal around 50000us, sigma=10000
                (50_000.0 + 10_000.0 * rng.next_normal()).max(1000.0) as u64
            }
        })
        .collect();
    all_max_errors.push(evaluate("Bimodal (90% ~500us fast, 10% ~50ms slow)", &values));

    // 4. Exponential — network/IO wait times Mean ~1000us (1ms), long right tail
    let values: Vec<u64> = (0..n)
        .map(|_| {
            let u = rng.next_f64().max(1e-15);
            (-u.ln() * 1000.0) as u64
        })
        .collect();
    all_max_errors.push(evaluate("Exponential (mean=1000us, IO wait)", &values));

    // 5. Log-normal with heavier tail — database query latency mu=8 (median ~3ms), sigma=1.0 (wide
    //    spread, heavy tail)
    let values: Vec<u64> = (0..n)
        .map(|_| {
            let z = rng.next_normal();
            (8.0 + 1.0 * z).exp() as u64
        })
        .collect();
    all_max_errors.push(evaluate("Log-normal (median~3ms, sigma=1.0, DB query)", &values));

    // 6. Sequential [1..N] — perfectly smooth baseline
    let values: Vec<u64> = (1..=n as u64).collect();
    all_max_errors.push(evaluate(&format!("Sequential [1..{n}]"), &values));

    // 7. Pareto (heavy tail) — request sizes, wealth distribution P(X > x) = (x_min/x)^alpha,
    //    alpha=1.5, x_min=100
    let values: Vec<u64> = (0..n)
        .map(|_| {
            let u = rng.next_f64().max(1e-15);
            (100.0 / u.powf(1.0 / 1.5)) as u64
        })
        .collect();
    all_max_errors.push(evaluate("Pareto (alpha=1.5, xmin=100, heavy tail)", &values));

    // --- Summary ---

    let overall_max = all_max_errors.iter().cloned().fold(0.0f64, f64::max);
    let overall_avg = all_max_errors.iter().sum::<f64>() / all_max_errors.len() as f64;
    println!("\n--- Summary ---");
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
        let half_width = (max_val - min_val) / 2;
        let max_err = if min_val == 0 {
            0.0
        } else {
            half_width as f64 / min_val as f64 * 100.0
        };
        worst_err = worst_err.max(max_err);

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
