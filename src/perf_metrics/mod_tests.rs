use super::*;

fn setup() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_perf_metrics();
    guard
}

#[test]
fn increment_counter_defaults_to_one_and_accumulates() {
    let _guard = setup();
    increment_perf_counter("foo");
    increment_perf_counter("foo");
    increment_perf_counter_by("foo", 3);
    let snapshot = get_perf_metrics_snapshot();
    assert_eq!(snapshot.counters.get("foo"), Some(&5));
}

#[test]
fn set_perf_gauge_overwrites_previous_value() {
    let _guard = setup();
    set_perf_gauge("queue.depth", 2.0);
    set_perf_gauge("queue.depth", 7.0);
    let snapshot = get_perf_metrics_snapshot();
    assert_eq!(snapshot.gauges.get("queue.depth"), Some(&7.0));
}

#[test]
fn record_perf_duration_accumulates_count_total_and_max() {
    let _guard = setup();
    record_perf_duration("op", 10.0);
    record_perf_duration("op", 25.0);
    record_perf_duration("op", 5.0);
    let snapshot = get_perf_metrics_snapshot();
    let bucket = snapshot.timings.get("op").expect("bucket recorded");
    assert_eq!(bucket.count, 3);
    assert_eq!(bucket.total_ms, 40.0);
    assert_eq!(bucket.max_ms, 25.0);
}

#[test]
fn start_perf_timer_records_a_sample_on_stop() {
    let _guard = setup();
    let timer = start_perf_timer("timed");
    std::thread::sleep(std::time::Duration::from_millis(5));
    let elapsed = timer.stop();
    assert!(elapsed >= 5.0, "elapsed {elapsed} should be at least 5ms");
    let snapshot = get_perf_metrics_snapshot();
    assert_eq!(snapshot.timings.get("timed").expect("recorded").count, 1);
}

#[test]
fn measure_perf_records_duration_around_the_future() {
    let _guard = setup();
    smol::block_on(async {
        let result = measure_perf("async_op", async {
            smol::Timer::after(std::time::Duration::from_millis(5)).await;
            42
        })
        .await;
        assert_eq!(result, 42);
    });
    let snapshot = get_perf_metrics_snapshot();
    let bucket = snapshot.timings.get("async_op").expect("recorded");
    assert_eq!(bucket.count, 1);
    assert!(bucket.total_ms >= 5.0);
}

#[test]
fn reset_perf_metrics_clears_all_maps() {
    let _guard = setup();
    increment_perf_counter("a");
    set_perf_gauge("b", 1.0);
    record_perf_duration("c", 1.0);
    reset_perf_metrics();
    let snapshot = get_perf_metrics_snapshot();
    assert!(snapshot.is_empty());
}

#[test]
fn format_perf_metric_rounds_to_three_decimals() {
    let _guard = setup();
    assert_eq!(format_perf_metric("op", 12.34567), "op=12.346ms");
}
