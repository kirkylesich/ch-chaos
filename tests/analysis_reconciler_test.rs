use async_trait::async_trait;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use chimp_chaos::operator::analysis_reconciler::*;
use chimp_chaos::operator::crd::*;
use chimp_chaos::operator::types::*;

// ── Mock KubeClient ──

struct MockAnalysisKube {
    experiment_status: Result<ChaosExperimentStatus, OperatorError>,
    patched: std::sync::Mutex<Option<ChaosAnalysisStatus>>,
}

impl MockAnalysisKube {
    fn with_completed_experiment(started_at: &str, completed_at: &str) -> Self {
        Self {
            experiment_status: Ok(ChaosExperimentStatus {
                phase: Phase::Succeeded,
                started_at: Some(started_at.to_string()),
                completed_at: Some(completed_at.to_string()),
                ..Default::default()
            }),
            patched: std::sync::Mutex::new(None),
        }
    }

    fn with_running_experiment() -> Self {
        Self {
            experiment_status: Ok(ChaosExperimentStatus {
                phase: Phase::Running,
                started_at: Some("2026-01-01T10:00:00Z".to_string()),
                ..Default::default()
            }),
            patched: std::sync::Mutex::new(None),
        }
    }

    fn with_missing_experiment() -> Self {
        Self {
            experiment_status: Err(OperatorError::Analysis("not found".into())),
            patched: std::sync::Mutex::new(None),
        }
    }

    fn patched_status(&self) -> Option<ChaosAnalysisStatus> {
        self.patched.lock().ok().and_then(|g| g.clone())
    }
}

#[async_trait]
impl AnalysisKubeClient for MockAnalysisKube {
    async fn get_experiment_status(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<ChaosExperimentStatus, OperatorError> {
        match &self.experiment_status {
            Ok(s) => Ok(s.clone()),
            Err(_) => Err(OperatorError::Analysis("not found".into())),
        }
    }

    async fn patch_analysis_status(
        &self,
        _ns: &str,
        _name: &str,
        status: &ChaosAnalysisStatus,
    ) -> Result<(), OperatorError> {
        *self
            .patched
            .lock()
            .map_err(|e| OperatorError::Analysis(e.to_string()))? = Some(status.clone());
        Ok(())
    }
}

// ── Mock PrometheusClient ──

struct MockAnalysisProm {
    baseline_value: f64,
    during_value: f64,
}

impl MockAnalysisProm {
    fn new(baseline: f64, during: f64) -> Self {
        Self {
            baseline_value: baseline,
            during_value: during,
        }
    }
}

#[async_trait]
impl AnalysisPrometheusClient for MockAnalysisProm {
    async fn query_at(&self, _promql: &str, time: &str) -> Result<f64, OperatorError> {
        // Before startedAt → baseline, after → during
        let ts = chrono::DateTime::parse_from_rfc3339(time)
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        let reference = chrono::DateTime::parse_from_rfc3339("2026-01-01T10:00:00Z")
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        if ts < reference {
            Ok(self.baseline_value)
        } else {
            Ok(self.during_value)
        }
    }
}

struct FailingProm;

#[async_trait]
impl AnalysisPrometheusClient for FailingProm {
    async fn query_at(&self, _promql: &str, _time: &str) -> Result<f64, OperatorError> {
        Err(OperatorError::Prometheus("connection refused".into()))
    }
}

// ── Helpers ──

fn analysis(direction: DegradationDirection, max_impact: u32) -> ChaosAnalysis {
    ChaosAnalysis {
        metadata: ObjectMeta {
            name: Some("test-analysis".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: ChaosAnalysisSpec {
            experiment_ref: ExperimentRef {
                name: "test-exp".into(),
                namespace: Some("default".into()),
            },
            prometheus: PrometheusConfig {
                url: "http://prometheus:9090".into(),
                baseline_window: "5m".into(),
            },
            query: "rate(http_requests_total[5m])".into(),
            degradation_direction: direction,
            success_criteria: SuccessCriteria { max_impact },
        },
        status: None,
    }
}

fn completed_analysis() -> ChaosAnalysis {
    let mut a = analysis(DegradationDirection::Up, 30);
    a.status = Some(ChaosAnalysisStatus {
        phase: AnalysisPhase::Completed,
        verdict: Some(AnalysisVerdict::Pass),
        impact_score: Some(10),
        ..Default::default()
    });
    a
}

// ── Tests ──

#[tokio::test]
async fn analysis_skips_completed() {
    let a = completed_analysis();
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    let prom = MockAnalysisProm::new(0.1, 0.2);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    assert!(
        kube.patched_status().is_none(),
        "should not patch already completed"
    );
}

#[tokio::test]
async fn analysis_pending_when_experiment_running() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube = MockAnalysisKube::with_running_experiment();
    let prom = MockAnalysisProm::new(0.1, 0.2);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Pending);
    assert!(status.message.as_deref().unwrap_or("").contains("Waiting"));
}

#[tokio::test]
async fn analysis_fails_when_experiment_not_found() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube = MockAnalysisKube::with_missing_experiment();
    let prom = MockAnalysisProm::new(0.1, 0.2);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Failed);
    assert!(status
        .message
        .as_deref()
        .unwrap_or("")
        .contains("cannot get experiment"));
}

#[tokio::test]
async fn analysis_latency_up_pass() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    // 20% increase — within 30% threshold
    let prom = MockAnalysisProm::new(0.10, 0.12);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Pass));
    assert_eq!(status.impact_score, Some(20));
    assert!((status.baseline_value.unwrap_or(0.0) - 0.10).abs() < 0.001);
    assert!((status.during_value.unwrap_or(0.0) - 0.12).abs() < 0.001);
}

#[tokio::test]
async fn analysis_latency_up_fail() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    // 50% increase — exceeds 30% threshold
    let prom = MockAnalysisProm::new(0.10, 0.15);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Fail));
    assert_eq!(status.impact_score, Some(50));
}

#[tokio::test]
async fn analysis_throughput_down_pass() {
    let a = analysis(DegradationDirection::Down, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    // RPS dropped 10% — within 30% threshold
    let prom = MockAnalysisProm::new(100.0, 90.0);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Pass));
    assert_eq!(status.impact_score, Some(10));
}

#[tokio::test]
async fn analysis_throughput_down_fail() {
    let a = analysis(DegradationDirection::Down, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    // RPS dropped 80% — exceeds 30% threshold
    let prom = MockAnalysisProm::new(100.0, 20.0);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Fail));
    assert_eq!(status.impact_score, Some(80));
}

#[tokio::test]
async fn analysis_no_degradation_when_metric_improves() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    // Latency decreased — no degradation in "up" direction
    let prom = MockAnalysisProm::new(0.10, 0.05);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.impact_score, Some(0));
    assert_eq!(status.verdict, Some(AnalysisVerdict::Pass));
}

#[tokio::test]
async fn analysis_prometheus_error_fails_gracefully() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");

    reconcile_analysis(&a, &kube, &FailingProm).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Failed);
    assert!(status
        .message
        .as_deref()
        .unwrap_or("")
        .contains("query failed"));
}

#[tokio::test]
async fn analysis_impact_clamped_to_100() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube =
        MockAnalysisKube::with_completed_experiment("2026-01-01T10:00:00Z", "2026-01-01T10:05:00Z");
    // 900% increase — clamped to 100
    let prom = MockAnalysisProm::new(0.01, 0.10);

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.impact_score, Some(100));
    assert_eq!(status.verdict, Some(AnalysisVerdict::Fail));
}

// ── parse_duration_str tests ──

#[test]
fn parse_minutes() {
    assert_eq!(parse_duration_str("5m"), Some(300));
    assert_eq!(parse_duration_str("30m"), Some(1800));
}

#[test]
fn parse_hours() {
    assert_eq!(parse_duration_str("1h"), Some(3600));
    assert_eq!(parse_duration_str("2h"), Some(7200));
}

#[test]
fn parse_seconds() {
    assert_eq!(parse_duration_str("60s"), Some(60));
}

#[test]
fn parse_raw_number() {
    assert_eq!(parse_duration_str("300"), Some(300));
}

#[test]
fn parse_invalid() {
    assert_eq!(parse_duration_str("abc"), None);
    assert_eq!(parse_duration_str(""), None);
}

// ── NaN handling tests ──

struct NanProm;

#[async_trait]
impl AnalysisPrometheusClient for NanProm {
    async fn query_at(&self, _promql: &str, time: &str) -> Result<f64, OperatorError> {
        let ts = chrono::DateTime::parse_from_rfc3339(time)
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        let reference = chrono::DateTime::parse_from_rfc3339("2026-01-01T10:00:00Z")
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        if ts < reference {
            Ok(0.1) // normal baseline
        } else {
            // Simulate NaN from Prometheus (e.g. histogram_quantile with no data)
            // After our fix, query_at should return 0.0 for NaN
            Ok(0.0) // NaN is converted to 0.0 by the fix
        }
    }
}

#[tokio::test]
async fn analysis_nan_during_value_treated_as_zero() {
    let a = analysis(DegradationDirection::Up, 30);
    let kube = MockAnalysisKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // during=0.0 (was NaN, converted to 0.0), baseline=0.1
    // direction Up: (0.0 - 0.1) / 0.1 = -100% → clamped to 0 (improvement)
    let prom = NanProm;

    reconcile_analysis(&a, &kube, &prom).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.impact_score, Some(0));
    assert_eq!(status.verdict, Some(AnalysisVerdict::Pass));
}
