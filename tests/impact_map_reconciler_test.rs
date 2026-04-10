use std::collections::HashMap;

use async_trait::async_trait;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use chimp_chaos::operator::crd::*;
use chimp_chaos::operator::impact_map_reconciler::*;
use chimp_chaos::operator::types::*;

// ── Mock KubeClient ──

struct MockImpactMapKube {
    experiment_status: Result<ChaosExperimentStatus, OperatorError>,
    patched: std::sync::Mutex<Option<ChaosImpactMapStatus>>,
}

impl MockImpactMapKube {
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

    fn patched_status(&self) -> Option<ChaosImpactMapStatus> {
        self.patched.lock().ok().and_then(|g| g.clone())
    }
}

#[async_trait]
impl ImpactMapKubeClient for MockImpactMapKube {
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

    async fn get_experiment_spec(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<ChaosExperimentSpec, OperatorError> {
        Err(OperatorError::Analysis("not implemented in mock".into()))
    }

    async fn patch_impact_map_status(
        &self,
        _ns: &str,
        _name: &str,
        status: &ChaosImpactMapStatus,
    ) -> Result<(), OperatorError> {
        *self
            .patched
            .lock()
            .map_err(|e| OperatorError::Analysis(e.to_string()))? = Some(status.clone());
        Ok(())
    }
}

// ── Mock PrometheusClient ──

struct MockImpactMapProm {
    latency_baseline: HashMap<ServiceKey, f64>,
    latency_during: HashMap<ServiceKey, f64>,
    error_baseline: HashMap<ServiceKey, f64>,
    error_during: HashMap<ServiceKey, f64>,
    throughput_baseline: HashMap<ServiceKey, f64>,
    throughput_during: HashMap<ServiceKey, f64>,
}

impl MockImpactMapProm {
    fn new() -> Self {
        Self {
            latency_baseline: HashMap::new(),
            latency_during: HashMap::new(),
            error_baseline: HashMap::new(),
            error_during: HashMap::new(),
            throughput_baseline: HashMap::new(),
            throughput_during: HashMap::new(),
        }
    }

    fn with_service(
        mut self,
        workload: &str,
        namespace: &str,
        lat_baseline: f64,
        lat_during: f64,
        err_baseline: f64,
        err_during: f64,
        thr_baseline: f64,
        thr_during: f64,
    ) -> Self {
        let key = ServiceKey {
            workload: workload.to_string(),
            namespace: namespace.to_string(),
        };
        self.latency_baseline.insert(key.clone(), lat_baseline);
        self.latency_during.insert(key.clone(), lat_during);
        self.error_baseline.insert(key.clone(), err_baseline);
        self.error_during.insert(key.clone(), err_during);
        self.throughput_baseline.insert(key.clone(), thr_baseline);
        self.throughput_during.insert(key, thr_during);
        self
    }
}

#[async_trait]
impl ImpactMapPrometheusClient for MockImpactMapProm {
    async fn query_vector_at(
        &self,
        promql: &str,
        time: &str,
    ) -> Result<HashMap<ServiceKey, f64>, OperatorError> {
        let ts = chrono::DateTime::parse_from_rfc3339(time)
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        let reference = chrono::DateTime::parse_from_rfc3339("2026-01-01T10:00:00Z")
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        let is_baseline = ts <= reference;

        if promql.contains("duration_milliseconds") {
            Ok(if is_baseline {
                self.latency_baseline.clone()
            } else {
                self.latency_during.clone()
            })
        } else if promql.contains("response_code") {
            Ok(if is_baseline {
                self.error_baseline.clone()
            } else {
                self.error_during.clone()
            })
        } else {
            Ok(if is_baseline {
                self.throughput_baseline.clone()
            } else {
                self.throughput_during.clone()
            })
        }
    }
}

struct FailingImpactMapProm;

#[async_trait]
impl ImpactMapPrometheusClient for FailingImpactMapProm {
    async fn query_vector_at(
        &self,
        _promql: &str,
        _time: &str,
    ) -> Result<HashMap<ServiceKey, f64>, OperatorError> {
        Err(OperatorError::Prometheus("connection refused".into()))
    }
}

// ── Helpers ──

fn impact_map(min_impact: u32) -> ChaosImpactMap {
    ChaosImpactMap {
        metadata: ObjectMeta {
            name: Some("test-impact-map".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: ChaosImpactMapSpec {
            experiment_ref: ExperimentRef {
                name: "test-exp".into(),
                namespace: Some("default".into()),
            },
            prometheus: PrometheusConfig {
                url: "http://prometheus:9090".into(),
                baseline_window: "5m".into(),
            },
            scope: None,
            min_impact,
        },
        status: None,
    }
}

fn impact_map_with_scope(namespaces: Vec<String>) -> ChaosImpactMap {
    let mut im = impact_map(5);
    im.spec.scope = Some(ImpactMapScope { namespaces });
    im
}

fn completed_impact_map() -> ChaosImpactMap {
    let mut im = impact_map(5);
    im.status = Some(ChaosImpactMapStatus {
        phase: AnalysisPhase::Completed,
        summary: Some(ImpactMapSummary {
            total_scanned: 1,
            total_affected: 0,
            message: None,
        }),
        ..Default::default()
    });
    im
}

// ── Tests ──

#[tokio::test]
async fn impact_map_skips_completed() {
    let im = completed_impact_map();
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    let prom = MockImpactMapProm::new();

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    assert!(
        kube.patched_status().is_none(),
        "should not patch already completed"
    );
}

#[tokio::test]
async fn impact_map_pending_when_experiment_running() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_running_experiment();
    let prom = MockImpactMapProm::new();

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Pending);
    assert!(status.message.as_deref().unwrap_or("").contains("Waiting"));
}

#[tokio::test]
async fn impact_map_fails_when_experiment_not_found() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_missing_experiment();
    let prom = MockImpactMapProm::new();

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Failed);
    assert!(status
        .message
        .as_deref()
        .unwrap_or("")
        .contains("cannot get experiment"));
}

#[tokio::test]
async fn impact_map_happy_path_multiple_services_sorted() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    let prom = MockImpactMapProm::new()
        // cartservice: latency +100% (impact 100), error +0%, throughput -5%
        .with_service(
            "cartservice",
            "production",
            0.05,
            0.10,
            0.001,
            0.001,
            100.0,
            95.0,
        )
        // frontend: latency +25% (impact 25), error +0%, throughput -3%
        .with_service(
            "frontend",
            "production",
            0.20,
            0.25,
            0.002,
            0.002,
            300.0,
            291.0,
        )
        // redis: no change (impact 0)
        .with_service("redis", "production", 0.01, 0.01, 0.0, 0.0, 500.0, 500.0);

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);

    let summary = status.summary.as_ref().expect("should have summary");
    assert_eq!(summary.total_scanned, 3);
    assert_eq!(summary.total_affected, 2); // redis filtered out (impact 0 < 5)

    assert_eq!(status.affected_services.len(), 2);
    // Sorted by impact_score descending (average of 3 metrics)
    assert_eq!(status.affected_services[0].workload, "cartservice");
    assert_eq!(status.affected_services[0].impact_score, 35); // (100+0+5)/3
    assert_eq!(status.affected_services[1].workload, "frontend");
    assert_eq!(status.affected_services[1].impact_score, 9); // (25+0+3)/3

    // Verify metric details for cartservice
    let cart = &status.affected_services[0];
    assert_eq!(cart.metrics.latency_p99.impact_score, 100);
    assert!((cart.metrics.latency_p99.baseline - 0.05).abs() < 0.001);
    assert!((cart.metrics.latency_p99.during - 0.10).abs() < 0.001);
    assert_eq!(cart.metrics.throughput.impact_score, 5);
}

#[tokio::test]
async fn impact_map_filtering_by_min_impact() {
    let im = impact_map(10); // threshold=10
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    let prom = MockImpactMapProm::new()
        // lat=100%, err=0, thr=50% → avg=(100+0+50)/3=50 — above threshold
        .with_service(
            "cartservice",
            "production",
            0.10,
            0.20,
            0.0,
            0.0,
            100.0,
            50.0,
        )
        // lat=10%, err=0, thr=0% → avg=(10+0+0)/3=3 — below threshold
        .with_service("frontend", "production", 0.10, 0.11, 0.0, 0.0, 100.0, 100.0);

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    let summary = status.summary.as_ref().expect("should have summary");
    assert_eq!(summary.total_scanned, 2);
    assert_eq!(summary.total_affected, 1);
    assert_eq!(status.affected_services.len(), 1);
    assert_eq!(status.affected_services[0].workload, "cartservice");
}

#[tokio::test]
async fn impact_map_no_services_affected() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // All services unchanged
    let prom = MockImpactMapProm::new()
        .with_service("svc-a", "production", 0.05, 0.05, 0.0, 0.0, 100.0, 100.0)
        .with_service("svc-b", "production", 0.10, 0.10, 0.0, 0.0, 200.0, 200.0);

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed); // Completed, not Failed
    let summary = status.summary.as_ref().expect("should have summary");
    assert_eq!(summary.total_scanned, 2);
    assert_eq!(summary.total_affected, 0);
    assert!(status.affected_services.is_empty());
}

#[tokio::test]
async fn impact_map_prometheus_error_fails_gracefully() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );

    reconcile_impact_map(&im, &kube, &FailingImpactMapProm, None)
        .await
        .unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Failed);
    assert!(status
        .message
        .as_deref()
        .unwrap_or("")
        .contains("query failed"));
}

#[tokio::test]
async fn impact_map_service_in_only_one_metric() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // Service only appears in throughput, not latency or error rate
    let mut prom = MockImpactMapProm::new();
    let key = ServiceKey {
        workload: "batch-worker".to_string(),
        namespace: "production".to_string(),
    };
    prom.throughput_baseline.insert(key.clone(), 100.0);
    prom.throughput_during.insert(key, 50.0); // 50% drop

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.affected_services.len(), 1);
    let svc = &status.affected_services[0];
    assert_eq!(svc.workload, "batch-worker");
    assert_eq!(svc.impact_score, 16); // (0+0+50)/3
    assert_eq!(svc.metrics.throughput.impact_score, 50);
    assert_eq!(svc.metrics.latency_p99.impact_score, 0); // no data → 0
    assert_eq!(svc.metrics.error_rate.impact_score, 0); // no data → 0
}

#[tokio::test]
async fn impact_map_zero_baseline_no_division_error() {
    let im = impact_map(0); // min_impact=0 to see all
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // Zero baseline, nonzero during → impact 100 (from calculate_impact)
    // Zero baseline, zero during → impact 0
    let prom = MockImpactMapProm::new().with_service(
        "new-svc",
        "production",
        0.0,
        0.05,
        0.0,
        0.0,
        0.0,
        0.0,
    );

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    let svc = &status.affected_services[0];
    assert_eq!(svc.metrics.latency_p99.impact_score, 100);
    assert_eq!(svc.metrics.error_rate.impact_score, 0); // 0 → 0
    assert_eq!(svc.metrics.throughput.impact_score, 0); // 0 → 0
}

#[tokio::test]
async fn impact_map_empty_prometheus_result() {
    let im = impact_map(5);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    let prom = MockImpactMapProm::new(); // no services at all

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    let summary = status.summary.as_ref().expect("should have summary");
    assert_eq!(summary.total_scanned, 0);
    assert_eq!(summary.total_affected, 0);
}

// ── Error rate scoring tests ──

#[tokio::test]
async fn impact_map_error_rate_zero_baseline_uses_absolute() {
    let im = impact_map(0); // see all
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // Error rate: 0 → 0.0015 (0.15%) — should NOT be impact 100
    // Latency unchanged, throughput unchanged
    let prom = MockImpactMapProm::new().with_service(
        "frontend",
        "production",
        0.10,
        0.10,
        0.0,
        0.0015,
        100.0,
        100.0,
    );

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    let svc = &status.affected_services[0];
    // 0.15% error rate → absolute impact 0 (rounds down from 0.15)
    assert_eq!(svc.metrics.error_rate.impact_score, 0);
    assert_eq!(svc.impact_score, 0); // no significant impact
}

#[tokio::test]
async fn impact_map_error_rate_zero_baseline_high_errors() {
    let im = impact_map(0);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // Error rate: 0 → 0.10 (10%) — significant, should show impact 10
    let prom = MockImpactMapProm::new().with_service(
        "frontend",
        "production",
        0.10,
        0.10,
        0.0,
        0.10,
        100.0,
        100.0,
    );

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    let svc = &status.affected_services[0];
    assert_eq!(svc.metrics.error_rate.impact_score, 10);
}

#[tokio::test]
async fn impact_map_error_rate_nonzero_baseline_uses_relative() {
    let im = impact_map(0);
    let kube = MockImpactMapKube::with_completed_experiment(
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:05:00Z",
    );
    // Error rate: 0.01 (1%) → 0.02 (2%) — 100% relative increase
    let prom = MockImpactMapProm::new().with_service(
        "frontend",
        "production",
        0.10,
        0.10,
        0.01,
        0.02,
        100.0,
        100.0,
    );

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    let svc = &status.affected_services[0];
    // 1% → 2% = 100% relative increase → impact 100
    assert_eq!(svc.metrics.error_rate.impact_score, 100);
}

// ── Direct vs Indirect classification tests ──

struct EdgeChaosKube {
    experiment_status: ChaosExperimentStatus,
    experiment_spec: ChaosExperimentSpec,
    patched: std::sync::Mutex<Option<ChaosImpactMapStatus>>,
}

impl EdgeChaosKube {
    fn new() -> Self {
        Self {
            experiment_status: ChaosExperimentStatus {
                phase: Phase::Succeeded,
                started_at: Some("2026-01-01T10:00:00Z".to_string()),
                completed_at: Some("2026-01-01T10:05:00Z".to_string()),
                ..Default::default()
            },
            experiment_spec: ChaosExperimentSpec {
                scenario: ScenarioType::EdgeDelay,
                duration: 300,
                target_namespace: None,
                target: Some(Target {
                    namespace: Some("production".to_string()),
                    edge: Some(EdgeTarget {
                        source_service: "frontend".to_string(),
                        destination_service: "cartservice".to_string(),
                    }),
                }),
                parameters: Some(serde_json::json!({"latencyMs": 500})),
            },
            patched: std::sync::Mutex::new(None),
        }
    }

    fn patched_status(&self) -> Option<ChaosImpactMapStatus> {
        self.patched.lock().ok().and_then(|g| g.clone())
    }
}

#[async_trait]
impl ImpactMapKubeClient for EdgeChaosKube {
    async fn get_experiment_status(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<ChaosExperimentStatus, OperatorError> {
        Ok(self.experiment_status.clone())
    }

    async fn get_experiment_spec(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<ChaosExperimentSpec, OperatorError> {
        Ok(self.experiment_spec.clone())
    }

    async fn patch_impact_map_status(
        &self,
        _ns: &str,
        _name: &str,
        status: &ChaosImpactMapStatus,
    ) -> Result<(), OperatorError> {
        *self
            .patched
            .lock()
            .map_err(|e| OperatorError::Analysis(e.to_string()))? = Some(status.clone());
        Ok(())
    }
}

struct MockGraph {
    graph: ServiceGraph,
}

impl MockGraph {
    fn online_boutique() -> Self {
        let mut adjacency: HashMap<ServiceKey, Vec<ServiceKey>> = HashMap::new();
        let mut service_to_workload: HashMap<(String, String), ServiceKey> = HashMap::new();

        let ns = "production".to_string();
        let frontend = ServiceKey { workload: "frontend".into(), namespace: ns.clone() };
        let cart = ServiceKey { workload: "cartservice".into(), namespace: ns.clone() };
        let checkout = ServiceKey { workload: "checkout".into(), namespace: ns.clone() };
        let payment = ServiceKey { workload: "paymentservice".into(), namespace: ns.clone() };
        let ad = ServiceKey { workload: "adservice".into(), namespace: ns.clone() };

        adjacency.entry(frontend.clone()).or_default().push(cart.clone());
        adjacency.entry(frontend.clone()).or_default().push(checkout.clone());
        adjacency.entry(frontend.clone()).or_default().push(ad.clone());
        adjacency.entry(checkout.clone()).or_default().push(payment.clone());

        service_to_workload.insert(("cartservice".into(), ns.clone()), cart);
        service_to_workload.insert(("frontend".into(), ns.clone()), frontend);
        service_to_workload.insert(("checkout".into(), ns.clone()), checkout);
        service_to_workload.insert(("paymentservice".into(), ns.clone()), payment);
        service_to_workload.insert(("adservice".into(), ns.clone()), ad);

        Self {
            graph: ServiceGraph { adjacency, service_to_workload },
        }
    }

    fn with_chain() -> Self {
        let mut adjacency: HashMap<ServiceKey, Vec<ServiceKey>> = HashMap::new();
        let mut service_to_workload: HashMap<(String, String), ServiceKey> = HashMap::new();

        let ns = "production".to_string();
        let cart = ServiceKey { workload: "cartservice".into(), namespace: ns.clone() };
        let inventory = ServiceKey { workload: "inventory".into(), namespace: ns.clone() };
        let warehouse = ServiceKey { workload: "warehouse".into(), namespace: ns.clone() };
        let frontend = ServiceKey { workload: "frontend".into(), namespace: ns.clone() };

        adjacency.entry(frontend.clone()).or_default().push(cart.clone());
        adjacency.entry(cart.clone()).or_default().push(inventory.clone());
        adjacency.entry(inventory.clone()).or_default().push(warehouse.clone());

        service_to_workload.insert(("cartservice".into(), ns.clone()), cart);
        service_to_workload.insert(("inventory".into(), ns.clone()), inventory);
        service_to_workload.insert(("warehouse".into(), ns.clone()), warehouse);
        service_to_workload.insert(("frontend".into(), ns.clone()), frontend);

        Self {
            graph: ServiceGraph { adjacency, service_to_workload },
        }
    }
}

#[async_trait]
impl ImpactMapGraphClient for MockGraph {
    async fn query_service_graph(
        &self,
        _namespace_filter: &[String],
    ) -> Result<ServiceGraph, OperatorError> {
        Ok(ServiceGraph {
            adjacency: self.graph.adjacency.clone(),
            service_to_workload: self.graph.service_to_workload.clone(),
        })
    }
}

struct FailingGraph;

#[async_trait]
impl ImpactMapGraphClient for FailingGraph {
    async fn query_service_graph(
        &self,
        _namespace_filter: &[String],
    ) -> Result<ServiceGraph, OperatorError> {
        Err(OperatorError::Prometheus("graph query failed".into()))
    }
}

fn edge_impact_map() -> ChaosImpactMap {
    ChaosImpactMap {
        metadata: ObjectMeta {
            name: Some("test-edge-impact".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: ChaosImpactMapSpec {
            experiment_ref: ExperimentRef {
                name: "test-edge-exp".into(),
                namespace: Some("default".into()),
            },
            prometheus: PrometheusConfig {
                url: "http://prometheus:9090".into(),
                baseline_window: "5m".into(),
            },
            scope: None,
            min_impact: 0, // see all
        },
        status: None,
    }
}

#[tokio::test]
async fn impact_map_edge_chaos_classifies_direct_and_indirect() {
    let im = edge_impact_map();
    let kube = EdgeChaosKube::new();
    let graph = MockGraph::online_boutique();
    // cartservice: target → direct
    // paymentservice: not reachable from cartservice → indirect
    // adservice: not reachable from cartservice → indirect
    // frontend: upstream, not reachable from cartservice → indirect
    let prom = MockImpactMapProm::new()
        .with_service("cartservice", "production", 0.01, 2.5, 0.0, 0.0, 5.0, 3.5)
        .with_service("paymentservice", "production", 0.01, 0.02, 0.0, 0.0, 1.0, 0.8)
        .with_service("adservice", "production", 0.01, 0.01, 0.0, 0.0, 3.0, 2.2)
        .with_service("frontend", "production", 20.0, 22.0, 0.0, 0.0, 5.0, 3.6);

    reconcile_impact_map(&im, &kube, &prom, Some(&graph)).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    assert_eq!(status.phase, AnalysisPhase::Completed);

    for svc in &status.affected_services {
        match svc.workload.as_str() {
            "cartservice" => assert_eq!(svc.impact_type, Some(ImpactType::Direct)),
            "paymentservice" => assert_eq!(svc.impact_type, Some(ImpactType::Indirect)),
            "adservice" => assert_eq!(svc.impact_type, Some(ImpactType::Indirect)),
            "frontend" => assert_eq!(svc.impact_type, Some(ImpactType::Indirect)),
            _ => panic!("unexpected service: {}", svc.workload),
        }
    }
}

#[tokio::test]
async fn impact_map_edge_chaos_transitive_direct() {
    let im = edge_impact_map();
    let kube = EdgeChaosKube::new();
    let graph = MockGraph::with_chain(); // cart → inventory → warehouse
    let prom = MockImpactMapProm::new()
        .with_service("cartservice", "production", 0.01, 2.5, 0.0, 0.0, 5.0, 3.0)
        .with_service("inventory", "production", 0.01, 0.5, 0.0, 0.0, 10.0, 7.0)
        .with_service("warehouse", "production", 0.01, 0.3, 0.0, 0.0, 8.0, 5.0)
        .with_service("frontend", "production", 20.0, 22.0, 0.0, 0.0, 5.0, 3.5);

    reconcile_impact_map(&im, &kube, &prom, Some(&graph)).await.unwrap();

    let status = kube.patched_status().expect("should patch status");
    for svc in &status.affected_services {
        match svc.workload.as_str() {
            "cartservice" => assert_eq!(svc.impact_type, Some(ImpactType::Direct)),
            "inventory" => assert_eq!(svc.impact_type, Some(ImpactType::Direct)),
            "warehouse" => assert_eq!(svc.impact_type, Some(ImpactType::Direct)),
            "frontend" => assert_eq!(svc.impact_type, Some(ImpactType::Indirect)),
            _ => panic!("unexpected service: {}", svc.workload),
        }
    }
}

#[tokio::test]
async fn impact_map_no_graph_client_no_impact_type() {
    let im = edge_impact_map();
    let kube = EdgeChaosKube::new();
    let prom = MockImpactMapProm::new()
        .with_service("cartservice", "production", 0.01, 2.5, 0.0, 0.0, 5.0, 3.5);

    reconcile_impact_map(&im, &kube, &prom, None).await.unwrap();

    let status = kube.patched_status().expect("should patch");
    for svc in &status.affected_services {
        assert_eq!(svc.impact_type, None);
    }
}

#[tokio::test]
async fn impact_map_graph_error_graceful() {
    let im = edge_impact_map();
    let kube = EdgeChaosKube::new();
    let prom = MockImpactMapProm::new()
        .with_service("cartservice", "production", 0.01, 2.5, 0.0, 0.0, 5.0, 3.5);

    reconcile_impact_map(&im, &kube, &prom, Some(&FailingGraph)).await.unwrap();

    let status = kube.patched_status().expect("should patch");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    // Graph failed but reconcile still completed — impact_type stays None
    for svc in &status.affected_services {
        assert_eq!(svc.impact_type, None);
    }
}

#[tokio::test]
async fn impact_map_edge_chaos_uses_target_namespace_field() {
    // Real CRDs use targetNamespace (top-level), not target.namespace
    let im = edge_impact_map();
    let mut kube = EdgeChaosKube::new();
    kube.experiment_spec.target_namespace = Some("production".to_string());
    kube.experiment_spec.target = Some(Target {
        namespace: None, // NOT set — like real CRDs
        edge: Some(EdgeTarget {
            source_service: "frontend".to_string(),
            destination_service: "cartservice".to_string(),
        }),
    });
    let graph = MockGraph::online_boutique();
    let prom = MockImpactMapProm::new()
        .with_service("cartservice", "production", 0.01, 2.5, 0.0, 0.0, 5.0, 3.5)
        .with_service("frontend", "production", 20.0, 22.0, 0.0, 0.0, 5.0, 3.6);

    reconcile_impact_map(&im, &kube, &prom, Some(&graph)).await.unwrap();

    let status = kube.patched_status().expect("should patch");
    let cart = status.affected_services.iter().find(|s| s.workload == "cartservice");
    assert_eq!(cart.unwrap().impact_type, Some(ImpactType::Direct));
    let front = status.affected_services.iter().find(|s| s.workload == "frontend");
    assert_eq!(front.unwrap().impact_type, Some(ImpactType::Indirect));
}

// ── PromQL builder tests ──

#[test]
fn build_namespace_filter_empty() {
    assert_eq!(build_namespace_filter(&[]), "");
}

#[test]
fn build_namespace_filter_single() {
    assert_eq!(
        build_namespace_filter(&["production".to_string()]),
        r#"destination_workload_namespace=~"production""#
    );
}

#[test]
fn build_namespace_filter_multiple() {
    assert_eq!(
        build_namespace_filter(&["production".to_string(), "staging".to_string()]),
        r#"destination_workload_namespace=~"production|staging""#
    );
}

#[test]
fn latency_query_no_filter() {
    let query = build_latency_query("", "5m");
    assert!(query.contains("istio_request_duration_milliseconds_bucket"));
    assert!(query.contains("histogram_quantile(0.99"));
    assert!(query.contains("[5m]"));
    assert!(!query.contains("destination_workload_namespace=~"));
}

#[test]
fn latency_query_with_filter() {
    let ns_filter = build_namespace_filter(&["production".to_string()]);
    let query = build_latency_query(&ns_filter, "300s");
    assert!(query.contains(r#"destination_workload_namespace=~"production""#));
    assert!(query.contains("[300s]"));
}

#[test]
fn error_rate_query_has_response_code_filter() {
    let query = build_error_rate_query("", "5m");
    assert!(query.contains(r#"response_code=~"5..""#));
    assert!(query.contains("istio_requests_total"));
}

#[test]
fn throughput_query_no_filter() {
    let query = build_throughput_query("", "5m");
    assert!(query.contains("istio_requests_total"));
    assert!(query.contains("destination_workload"));
    assert!(!query.contains("response_code"));
}

#[test]
fn rate_window_matches_experiment_duration() {
    let query = build_latency_query("", "300s");
    assert!(query.contains("[300s]"));
    let query = build_throughput_query("", "120s");
    assert!(query.contains("[120s]"));
}

#[test]
fn namespace_scope_in_queries() {
    let im = impact_map_with_scope(vec!["production".to_string(), "staging".to_string()]);
    let ns_filter = im
        .spec
        .scope
        .as_ref()
        .map(|s| build_namespace_filter(&s.namespaces))
        .unwrap_or_default();
    let query = build_latency_query(&ns_filter, "5m");
    assert!(query.contains(r#"destination_workload_namespace=~"production|staging""#));
}
