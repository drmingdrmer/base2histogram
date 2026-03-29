use base2histogram::Histogram;
use base2histogram::PercentileStats;

#[test]
fn application_records_latency_distribution() {
    let mut hist = Histogram::<()>::new();

    for latency_ms in [1, 2, 3, 5, 8, 13, 21, 34, 55, 89] {
        hist.record(latency_ms);
    }

    assert_eq!(hist.total(), 10);
    assert_eq!(hist.percentile(0.0), 1);

    let p50 = hist.percentile(0.5);
    assert!((8..=21).contains(&p50), "p50 = {p50}");

    let p90 = hist.percentile(0.9);
    assert!((48..=56).contains(&p90), "p90 = {p90}");
}

#[test]
fn application_reports_precomputed_percentile_stats() {
    let mut hist = Histogram::<()>::new();

    hist.record_n(5, 20);
    hist.record_n(20, 60);
    hist.record_n(80, 20);

    let stats = hist.percentile_stats();

    assert_eq!(stats, PercentileStats {
        samples: 100,
        p0_1: 5,
        p1: 5,
        p5: 5,
        p10: 5,
        p50: 20,
        p90: 80,
        p99: 80,
        p99_9: 80,
    });

    assert_eq!(
        stats.to_string(),
        "[samples: 100, P0.1: 5, P1: 5, P5: 5, P10: 5, P50: 20, P90: 80, P99: 80, P99.9: 80]"
    );
}
