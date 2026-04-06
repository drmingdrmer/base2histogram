use std::f64::consts::PI;
use std::hint::black_box;
use std::time::Duration;
use std::time::Instant;

use base2histogram::Histogram;

const N: usize = 1_000_000;
const WARMUP_ITERS: usize = 3;
const MEASURE_ITERS: usize = 10;
const PERCENTILE_ITERS: usize = 10_000;

// ---------------------------------------------------------------------------
// PRNG (same xorshift64 used by accuracy benchmarks — no external dep)
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
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

// ---------------------------------------------------------------------------
// Distributions
// ---------------------------------------------------------------------------

fn sequential(n: usize) -> Vec<u64> {
    (1..=n as u64).collect()
}

fn uniform(n: usize) -> Vec<u64> {
    let mut rng = Rng::new(42);
    (0..n).map(|_| rng.next_u64() % 1_000_000 + 1).collect()
}

fn log_normal(n: usize) -> Vec<u64> {
    let mut rng = Rng::new(42);
    (0..n).map(|_| (6.0 + 0.5 * rng.next_normal()).exp() as u64).collect()
}

// ---------------------------------------------------------------------------
// Measurement helpers
// ---------------------------------------------------------------------------

fn median_ns_per_op(timings: &[Duration], ops: usize) -> f64 {
    let mut nanos: Vec<f64> = timings.iter().map(|d| d.as_nanos() as f64 / ops as f64).collect();
    nanos.sort_by(|a, b| a.partial_cmp(b).unwrap());
    nanos[nanos.len() / 2]
}

fn measure_record_ns(values: &[u64]) -> f64 {
    let mut timings = Vec::with_capacity(WARMUP_ITERS + MEASURE_ITERS);

    for _ in 0..(WARMUP_ITERS + MEASURE_ITERS) {
        let mut hist = Histogram::<()>::new();
        let start = Instant::now();
        for &v in values {
            hist.record(v);
        }
        let elapsed = start.elapsed();
        black_box(&hist);
        timings.push(elapsed);
    }

    median_ns_per_op(&timings[WARMUP_ITERS..], values.len())
}

fn measure_percentile_ns(hist: &Histogram, quantile: f64) -> f64 {
    let mut timings = Vec::with_capacity(WARMUP_ITERS + MEASURE_ITERS);

    for _ in 0..(WARMUP_ITERS + MEASURE_ITERS) {
        let start = Instant::now();
        for _ in 0..PERCENTILE_ITERS {
            black_box(hist.percentile(quantile));
        }
        let elapsed = start.elapsed();
        timings.push(elapsed);
    }

    median_ns_per_op(&timings[WARMUP_ITERS..], PERCENTILE_ITERS)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    eprintln!("Generating {N} samples per distribution...\n");

    let seq = sequential(N);
    let uni = uniform(N);
    let lnorm = log_normal(N);

    // --- Record throughput ---
    eprintln!("Measuring record throughput...");
    let seq_ns = measure_record_ns(&seq);
    let uni_ns = measure_record_ns(&uni);
    let ln_ns = measure_record_ns(&lnorm);

    // --- Percentile latency ---
    eprintln!("Measuring percentile latency...");
    let mut hist = Histogram::<()>::new();
    for &v in &lnorm {
        hist.record(v);
    }
    let p50_ns = measure_percentile_ns(&hist, 0.50);
    let p90_ns = measure_percentile_ns(&hist, 0.90);
    let p95_ns = measure_percentile_ns(&hist, 0.95);
    let p99_ns = measure_percentile_ns(&hist, 0.99);
    let p999_ns = measure_percentile_ns(&hist, 0.999);

    // --- Print results ---
    let col = 12;

    println!("=== base2histogram Benchmark ===");
    println!();
    println!("  {N} values per distribution, median of {MEASURE_ITERS} iterations");
    println!();

    println!("Record throughput (ns/op):");
    println!("  {:-<width$}", "", width = 3 * col + 10);
    print!("  {:<10}", "");
    for label in ["Sequential", "Uniform", "LogNormal"] {
        print!(" {:>col$}", label);
    }
    println!();
    print!("  {:<10}", "record()");
    for ns in [seq_ns, uni_ns, ln_ns] {
        print!(" {:>w$.1}", ns, w = col);
    }
    println!();
    println!("  {:-<width$}", "", width = 3 * col + 10);

    println!();
    println!("Percentile latency (ns/op, after recording {N} log-normal values):");
    println!("  {:-<width$}", "", width = 5 * col + 10);
    print!("  {:<10}", "");
    for label in ["P50", "P90", "P95", "P99", "P99.9"] {
        print!(" {:>col$}", label);
    }
    println!();
    print!("  {:<10}", "query");
    for ns in [p50_ns, p90_ns, p95_ns, p99_ns, p999_ns] {
        print!(" {:>w$.1}", ns, w = col);
    }
    println!();
    println!("  {:-<width$}", "", width = 5 * col + 10);
}
