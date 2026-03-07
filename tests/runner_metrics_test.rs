use prometheus::Registry;

use chimp_chaos::runner::metrics::RunnerMetrics;

#[test]
fn creates_metrics_successfully() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry);
    assert!(metrics.is_ok());
}

#[test]
fn set_active_and_inactive() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry).unwrap();

    metrics.set_active("exp-123", "CpuStress");
    let val = metrics
        .injection_active
        .with_label_values(&["exp-123", "CpuStress"])
        .get();
    assert!((val - 1.0).abs() < f64::EPSILON);

    metrics.set_inactive("exp-123", "CpuStress");
    let val = metrics
        .injection_active
        .with_label_values(&["exp-123", "CpuStress"])
        .get();
    assert!(val.abs() < f64::EPSILON);
}

#[test]
fn record_success_increments() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry).unwrap();

    metrics.record_success("PodKiller");
    metrics.record_success("PodKiller");

    let val = metrics
        .injection_total
        .with_label_values(&["PodKiller", "success"])
        .get();
    assert_eq!(val, 2);
}

#[test]
fn record_failure_increments() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry).unwrap();

    metrics.record_failure("NetworkDelay");

    let val = metrics
        .injection_total
        .with_label_values(&["NetworkDelay", "failure"])
        .get();
    assert_eq!(val, 1);
}

#[test]
fn set_targets_affected() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry).unwrap();

    metrics.set_targets(5);
    assert!((metrics.targets_affected.get() - 5.0).abs() < f64::EPSILON);
}

#[test]
fn observe_duration() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry).unwrap();

    metrics.observe_duration(42.5);
    let count = metrics.injection_duration.get_sample_count();
    assert_eq!(count, 1);
}

#[test]
fn double_register_fails() {
    let registry = Registry::new();
    let _m1 = RunnerMetrics::new(&registry).unwrap();
    let m2 = RunnerMetrics::new(&registry);
    assert!(m2.is_err());
}

#[test]
fn metrics_gathered_from_registry() {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry).unwrap();

    metrics.set_active("exp-1", "CpuStress");
    metrics.record_success("CpuStress");

    let families = registry.gather();
    let names: Vec<&str> = families.iter().map(|f| f.get_name()).collect();
    assert!(names.contains(&"chaos_injection_active"));
    assert!(names.contains(&"chaos_injection_total"));
    assert!(names.contains(&"chaos_targets_affected"));
    assert!(names.contains(&"chaos_injection_duration_seconds"));
}
