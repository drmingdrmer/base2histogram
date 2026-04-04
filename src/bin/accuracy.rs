use std::f64::consts::PI;

use base2histogram::Histogram;
use base2histogram::LogScale;

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

struct Distribution {
    name: String,
    label: &'static str,
    values: Vec<u64>,
    sorted: Vec<u64>,
}

/// Evaluate percentile accuracy for one distribution.
/// Returns the absolute relative error (%) for each percentile point.
fn evaluate(dist: &Distribution, width: usize) -> Vec<f64> {
    let mut hist = Histogram::<()>::with_log_scale(width, 1);
    for &v in &dist.values {
        hist.record(v);
    }

    println!("\n  {}", dist.name);
    println!(
        "  {:<10} {:>14} {:>14} {:>10}",
        "Percentile", "Exact", "Estimated", "Error%"
    );
    println!("  {:-<52}", "");

    let mut errors = Vec::with_capacity(PERCENTILES.len());

    for &(p, label) in PERCENTILES {
        let exact = exact_percentile(&dist.sorted, p);
        let estimated = hist.percentile(p);

        let rel_err = if exact == 0 {
            0.0
        } else {
            (exact as f64 - estimated as f64) / exact as f64 * 100.0
        };

        errors.push(rel_err.abs());

        println!("  {:<10} {:>14} {:>14} {:>9.3}%", label, exact, estimated, rel_err);
    }

    println!("  {:-<52}", "");
    errors
}

/// Evaluate without printing — for `--summary` mode.
fn evaluate_silent(dist: &Distribution, width: usize) -> Vec<f64> {
    let mut hist = Histogram::<()>::with_log_scale(width, 1);
    for &v in &dist.values {
        hist.record(v);
    }

    PERCENTILES
        .iter()
        .map(|&(p, _)| {
            let exact = exact_percentile(&dist.sorted, p);
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
        ($name:expr, $label:expr, $gen:expr) => {{
            let values: Vec<u64> = (0..n).map(|_| $gen(&mut rng)).collect();
            let mut sorted = values.clone();
            sorted.sort_unstable();
            dists.push(Distribution {
                name: $name.into(),
                label: $label,
                values,
                sorted,
            });
        }};
    }

    dist!("Uniform [0, 1_000_000]", "Uniform", |rng: &mut Rng| {
        rng.next_u64() % 1_000_001
    });

    dist!("Log-normal (median~1100us, API latency)", "LN-API", |rng: &mut Rng| {
        (7.0 + 0.5 * rng.next_normal()).exp() as u64
    });

    dist!(
        "Bimodal (90% ~500us fast, 10% ~50ms slow)",
        "Bimodal",
        |rng: &mut Rng| {
            if rng.next_u64() % 100 < 90 {
                (500.0 + 50.0 * rng.next_normal()).max(1.0) as u64
            } else {
                (50_000.0 + 10_000.0 * rng.next_normal()).max(1000.0) as u64
            }
        }
    );

    dist!("Exponential (mean=1000us, IO wait)", "Expon", |rng: &mut Rng| {
        let u = rng.next_f64().max(1e-15);
        (-u.ln() * 1000.0) as u64
    });

    dist!(
        "Log-normal (median~3ms, sigma=1.0, DB query)",
        "LN-DB",
        |rng: &mut Rng| { (8.0 + 1.0 * rng.next_normal()).exp() as u64 }
    );

    // Sequential — already sorted, no clone needed
    let values: Vec<u64> = (1..=n as u64).collect();
    let sorted = values.clone();
    dists.push(Distribution {
        name: format!("Sequential [1..{n}]"),
        label: "Sequent",
        values,
        sorted,
    });

    dist!("Pareto (alpha=1.5, xmin=100, heavy tail)", "Pareto", |rng: &mut Rng| {
        let u = rng.next_f64().max(1e-15);
        (100.0 / u.powf(1.0 / 1.5)) as u64
    });

    dists
}

/// Summary statistics for one WIDTH across all distributions.
struct WidthResult {
    width: usize,
    buckets: usize,
    memory_bytes: usize,
    /// errors\[dist_idx\]\[percentile_idx\] — absolute relative error %
    errors: Vec<Vec<f64>>,
}

fn run_width_silent(width: usize, distributions: &[Distribution], results: &mut Vec<WidthResult>) {
    let buckets = LogScale::get(width).num_buckets();
    let memory_bytes = 2 * buckets * size_of::<u64>();

    let errors: Vec<Vec<f64>> = distributions.iter().map(|dist| evaluate_silent(dist, width)).collect();

    results.push(WidthResult {
        width,
        buckets,
        memory_bytes,
        errors,
    });
}

fn run_width_verbose(width: usize, distributions: &[Distribution], results: &mut Vec<WidthResult>) {
    let buckets = LogScale::get(width).num_buckets();
    let memory_bytes = 2 * buckets * size_of::<u64>();

    println!("\n{}", "=".repeat(60));
    println!("WIDTH={}  ({} buckets, {})", width, buckets, format_bytes(memory_bytes),);
    println!("{}", "=".repeat(60));

    let errors: Vec<Vec<f64>> = distributions.iter().map(|dist| evaluate(dist, width)).collect();

    results.push(WidthResult {
        width,
        buckets,
        memory_bytes,
        errors,
    });
}

fn main() {
    let summary_only = std::env::args().any(|a| a == "--summary");

    if !summary_only {
        println!("=== base2histogram Percentile Accuracy Report ===");
    }

    let n = 1_000_000usize;
    if !summary_only {
        println!("\nGenerating {n} samples per distribution...");
    }
    let distributions = generate_distributions(n);

    let mut results: Vec<WidthResult> = Vec::new();

    for width in 1..=6 {
        if summary_only {
            run_width_silent(width, &distributions, &mut results);
        } else {
            run_width_verbose(width, &distributions, &mut results);
        }
    }

    // --- Summary chart ---

    let label_w = 14;
    let col_w = 10;
    let n_widths = results.len();
    let rule_len = label_w + 2 + n_widths * (col_w + 1);

    // Percentile indices to show: P50, P90, P95, P99
    let summary = &[(5, "P50"), (8, "P95"), (9, "P99")];

    println!("\n{}", "=".repeat(rule_len));
    println!("Summary: Relative Error at P50 / P95 / P99");
    println!("{}", "=".repeat(rule_len));
    println!();
    println!("  WIDTH:  Bucket granularity parameter. Each bucket group uses WIDTH");
    println!("          bits, giving 2^(WIDTH-1) buckets per group. Higher WIDTH");
    println!("          means finer resolution but more memory.");
    println!();
    println!("  Memory: Per-slot = buckets x 8 bytes.");
    println!("          Total for a 1-slot histogram = 2 x per-slot");
    println!("          (one for the slot, one for the aggregate).");
    println!();
    println!("  Error:  |exact - estimated| / exact x 100%.");
    println!();

    // Header
    print!("  {:<label_w$}", "");
    for r in &results {
        print!(" {:>col_w$}", format!("W={}", r.width));
    }
    println!();
    println!("  {:-<width$}", "", width = rule_len - 2);

    // Per-distribution rows: 4 sub-rows per distribution (P50, P90, P95, P99)
    for (i, dist) in distributions.iter().enumerate() {
        for (si, &(pidx, plabel)) in summary.iter().enumerate() {
            let row_label = if si == 0 {
                format!("{:<8}{}", dist.label, plabel)
            } else {
                format!("{:<8}{}", "", plabel)
            };
            print!("  {:<label_w$}", row_label);
            for r in &results {
                print!(" {:>w$.3}%", r.errors[i][pidx], w = col_w - 1);
            }
            println!();
        }
        println!();
    }
    println!("  {:-<width$}", "", width = rule_len - 2);

    // Buckets row
    print!("  {:<label_w$}", "Buckets");
    for r in &results {
        print!(" {:>col_w$}", r.buckets);
    }
    println!();

    // Memory rows
    print!("  {:<label_w$}", "Mem/slot");
    for r in &results {
        print!(" {:>col_w$}", format_bytes(r.memory_bytes / 2));
    }
    println!();
    print!("  {:<label_w$}", "Mem total");
    for r in &results {
        print!(" {:>col_w$}", format_bytes(r.memory_bytes));
    }
    println!();
    println!("  {:-<width$}", "", width = rule_len - 2);
}

fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
