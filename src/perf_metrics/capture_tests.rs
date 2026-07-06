use super::*;
use crate::perf_metrics::{increment_perf_counter, reset_perf_metrics as reset_metrics};

fn setup() -> std::sync::MutexGuard<'static, ()> {
    let guard = crate::perf_metrics::TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_metrics();
    *state().lock() = CaptureState::default();
    guard
}

#[test]
fn should_capture_requires_non_blank_path() {
    let mut s = CaptureState::default();
    assert!(!should_capture(&s));
    s.file_path = Some("  ".to_string());
    assert!(!should_capture(&s));
    s.file_path = Some("/tmp/metrics.jsonl".to_string());
    assert!(should_capture(&s));
}

#[test]
fn env_map_lookup_ignores_blank_values() {
    let mut env = HashMap::new();
    env.insert(PERF_METRICS_FILE_ENV.to_string(), "   ".to_string());
    assert_eq!(perf_metrics_capture_file_from_env_map(&env), None);

    env.insert(
        PERF_METRICS_FILE_ENV.to_string(),
        "/tmp/x.jsonl".to_string(),
    );
    assert_eq!(
        perf_metrics_capture_file_from_env_map(&env),
        Some("/tmp/x.jsonl".to_string())
    );
}

#[test]
fn install_without_a_path_does_not_arm_capture() {
    let _guard = setup();
    install_perf_metrics_capture(PerfMetricsCaptureOptions::default());
    assert!(!should_capture(&state().lock()));
}

#[test]
fn checkpoint_writes_a_line_and_resets_metrics_but_not_sequence() {
    let _guard = setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("metrics.jsonl");

    install_perf_metrics_capture(PerfMetricsCaptureOptions {
        argv: vec!["boltz".to_string()],
        role: Some("gui".to_string()),
        file_path: Some(file_path.display().to_string()),
    });
    increment_perf_counter("checkpoint.hit");

    checkpoint_perf_metrics_capture();

    let contents = fs::read_to_string(&file_path).expect("capture file written");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(lines[0]).expect("valid json line");
    assert_eq!(parsed["reason"], "checkpoint");
    assert_eq!(parsed["role"], "gui");
    assert_eq!(parsed["sequence"], 0);
    assert_eq!(parsed["metrics"]["counters"]["checkpoint.hit"], 1);

    // Metrics were reset after the write: a second checkpoint with
    // nothing new recorded should not append another line.
    checkpoint_perf_metrics_capture();
    let contents_after = fs::read_to_string(&file_path).expect("capture file still there");
    assert_eq!(contents_after.lines().count(), 1);
}

#[test]
fn checkpoint_advances_sequence_across_writes() {
    let _guard = setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("metrics.jsonl");
    install_perf_metrics_capture(PerfMetricsCaptureOptions {
        file_path: Some(file_path.display().to_string()),
        ..Default::default()
    });

    increment_perf_counter("a");
    checkpoint_perf_metrics_capture();
    increment_perf_counter("b");
    checkpoint_perf_metrics_capture();

    let contents = fs::read_to_string(&file_path).expect("capture file written");
    let lines: Vec<serde_json::Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid json line"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["sequence"], 0);
    assert_eq!(lines[1]["sequence"], 1);
}

#[test]
fn flush_is_idempotent_until_checkpoint_rearms_it() {
    let _guard = setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("metrics.jsonl");
    install_perf_metrics_capture(PerfMetricsCaptureOptions {
        file_path: Some(file_path.display().to_string()),
        ..Default::default()
    });
    increment_perf_counter("shutdown.hit");

    flush_perf_metrics_capture(CaptureReason::Exit);
    flush_perf_metrics_capture(CaptureReason::Exit); // no-op: already flushed

    let contents = fs::read_to_string(&file_path).expect("capture file written");
    assert_eq!(contents.lines().count(), 1);
    let parsed: serde_json::Value = serde_json::from_str(contents.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["reason"], "exit");
}

#[test]
fn write_without_new_metrics_does_not_append() {
    let _guard = setup();
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("metrics.jsonl");
    install_perf_metrics_capture(PerfMetricsCaptureOptions {
        file_path: Some(file_path.display().to_string()),
        ..Default::default()
    });
    // install_perf_metrics_capture already reset metrics; nothing was
    // recorded since, so this checkpoint has nothing to report.
    checkpoint_perf_metrics_capture();
    assert!(!file_path.exists(), "no metrics -> no file written");
}
