use std::f64::consts::PI;

use base2histogram::Histogram;

/// Simple xorshift64 PRNG.
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

    fn next_normal(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-15);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

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

struct Distribution {
    label: &'static str,
    values: Vec<u64>,
    sorted: Vec<u64>,
}

/// Compute absolute relative error (%) for each percentile.
fn errors_for(hist: &Histogram, sorted: &[u64]) -> Vec<f64> {
    PERCENTILES
        .iter()
        .map(|&(p, _)| {
            let exact = exact_percentile(sorted, p);
            let estimated = hist.percentile(p);
            if exact == 0 {
                0.0
            } else {
                ((exact as f64 - estimated as f64) / exact as f64 * 100.0).abs()
            }
        })
        .collect()
}

fn generate_distributions(n: usize) -> Vec<Distribution> {
    let mut rng = Rng::new(12345);
    let mut dists = Vec::new();

    macro_rules! dist {
        ($label:expr, $gen:expr) => {{
            let values: Vec<u64> = (0..n).map(|_| $gen(&mut rng)).collect();
            let mut sorted = values.clone();
            sorted.sort_unstable();
            dists.push(Distribution {
                label: $label,
                values,
                sorted,
            });
        }};
    }

    dist!("Uniform", |rng: &mut Rng| { rng.next_u64() % 1_000_001 });

    dist!("LN-API", |rng: &mut Rng| {
        (7.0 + 0.5 * rng.next_normal()).exp() as u64
    });

    dist!("Bimodal", |rng: &mut Rng| {
        if rng.next_u64() % 100 < 90 {
            (500.0 + 50.0 * rng.next_normal()).max(1.0) as u64
        } else {
            (50_000.0 + 10_000.0 * rng.next_normal()).max(1000.0) as u64
        }
    });

    dist!("Expon", |rng: &mut Rng| {
        let u = rng.next_f64().max(1e-15);
        (-u.ln() * 1000.0) as u64
    });

    dist!("LN-DB", |rng: &mut Rng| {
        (8.0 + 1.0 * rng.next_normal()).exp() as u64
    });

    let values: Vec<u64> = (1..=n as u64).collect();
    let sorted = values.clone();
    dists.push(Distribution {
        label: "Sequent",
        values,
        sorted,
    });

    dist!("Pareto", |rng: &mut Rng| {
        let u = rng.next_f64().max(1e-15);
        (100.0 / u.powf(1.0 / 1.5)) as u64
    });

    dists
}

struct Column {
    label: String,
    /// `errors[dist_idx][percentile_idx]`
    errors: Vec<Vec<f64>>,
}

fn main() {
    let n = 1_000_000usize;
    println!("=== Rescale Accuracy Benchmark ===");
    println!("\nGenerating {n} samples per distribution...");

    let distributions = generate_distributions(n);

    // Column 1: width=3 (direct)
    let w3_errors: Vec<Vec<f64>> = distributions
        .iter()
        .map(|dist| {
            let mut hist = Histogram::<()>::with_log_scale(3, 1);
            for &v in &dist.values {
                hist.record(v);
            }
            errors_for(&hist, &dist.sorted)
        })
        .collect();

    // Column 2: width=3 rescaled to width=5
    let rescaled_errors: Vec<Vec<f64>> = distributions
        .iter()
        .map(|dist| {
            let mut hist = Histogram::<()>::with_log_scale(3, 1);
            for &v in &dist.values {
                hist.record(v);
            }
            let rescaled = hist.rescale(5);
            errors_for(&rescaled, &dist.sorted)
        })
        .collect();

    // Column 3: width=5 (direct)
    let w5_errors: Vec<Vec<f64>> = distributions
        .iter()
        .map(|dist| {
            let mut hist = Histogram::<()>::with_log_scale(5, 1);
            for &v in &dist.values {
                hist.record(v);
            }
            errors_for(&hist, &dist.sorted)
        })
        .collect();

    // Column 4: difference (3→5) - (W=5)
    let diff_errors: Vec<Vec<f64>> = rescaled_errors
        .iter()
        .zip(w5_errors.iter())
        .map(|(re, w5)| re.iter().zip(w5.iter()).map(|(r, w)| r - w).collect())
        .collect();

    let columns = vec![
        Column {
            label: "W=3".into(),
            errors: w3_errors,
        },
        Column {
            label: "3→5".into(),
            errors: rescaled_errors,
        },
        Column {
            label: "W=5".into(),
            errors: w5_errors,
        },
        Column {
            label: "Δ(3→5,5)".into(),
            errors: diff_errors,
        },
    ];

    // --- Summary chart ---

    let label_w = 14;
    let col_w = 10;
    let n_cols = columns.len();
    let rule_len = label_w + 2 + n_cols * (col_w + 1);

    let summary = &[(5, "P50"), (8, "P95"), (9, "P99")];

    println!("\n{}", "=".repeat(rule_len));
    println!("Relative Error: W=3 direct vs W=3→5 rescaled vs W=5 direct");
    println!("{}", "=".repeat(rule_len));
    println!();
    println!("  W=3:       Width=3 histogram (252 buckets, 2.0 KB)");
    println!("  3→5:       Width=3 rescaled to width=5 (2016 buckets, 15.8 KB)");
    println!("  W=5:       Width=5 histogram built directly (2016 buckets, 15.8 KB)");
    println!("  Δ(3→5,5):  Error difference: (3→5) - (W=5). Positive = rescale is worse.");
    println!();
    println!("  Error: |exact - estimated| / exact x 100%.");
    println!();

    // Header
    print!("  {:<label_w$}", "");
    for col in &columns {
        print!(" {:>col_w$}", col.label);
    }
    println!();
    println!("  {:-<width$}", "", width = rule_len - 2);

    // Per-distribution rows
    for (i, dist) in distributions.iter().enumerate() {
        for (si, &(pidx, plabel)) in summary.iter().enumerate() {
            let row_label = if si == 0 {
                format!("{:<8}{}", dist.label, plabel)
            } else {
                format!("{:<8}{}", "", plabel)
            };
            print!("  {:<label_w$}", row_label);
            for col in &columns {
                print!(" {:>w$.3}%", col.errors[i][pidx], w = col_w - 1);
            }
            println!();
        }
        println!();
    }
    println!("  {:-<width$}", "", width = rule_len - 2);
}
