use base2histogram::Histogram;

#[test]
fn application_can_track_a_sliding_window() {
    let mut hist = Histogram::<&'static str>::with_slots(2);

    hist.record_n(10, 2);
    assert_eq!(hist.total(), 2);
    assert_eq!(hist.active_slot_count(), 1);
    assert_eq!(hist.slot_limit(), 2);

    assert_eq!(hist.advance("warm"), 2);
    hist.record_n(100, 3);
    assert_eq!(hist.total(), 5);

    let p90 = hist.percentile(0.9);
    assert!((96..=112).contains(&p90), "p90 = {p90}");

    assert_eq!(hist.advance("steady"), 2);
    assert_eq!(hist.total(), 3);

    let p50 = hist.percentile(0.5);
    assert!((96..=112).contains(&p50), "p50 = {p50}");
}
