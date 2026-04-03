//! End-to-end lifecycle tests using mocks.
//! Tests the full flow: Experiment (Pending → Running → Succeeded) → Analysis (Pending → Completed).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{Job, JobStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use chimp_chaos::operator::analysis_reconciler::*;
use chimp_chaos::operator::crd::*;
use chimp_chaos::operator::reconciler::*;
use chimp_chaos::operator::types::*;

// ══════════════════════════════════════════════════
// Stateful mock that tracks experiment lifecycle
// ══════════════════════════════════════════════════

struct StatefulKube {
    calls: Mutex<Vec<String>>,
    experiment_status: Mutex<ChaosExperimentStatus>,
    analysis_status: Mutex<Option<ChaosAnalysisStatus>>,
    target_nodes: Vec<String>,
    created_jobs: Mutex<Vec<String>>,
    job_results: Mutex<Vec<Job>>,
    created_vs: Mutex<Vec<VirtualServiceInfo>>,
}

impl StatefulKube {
    fn new() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            experiment_status: Mutex::new(ChaosExperimentStatus::default()),
            analysis_status: Mutex::new(None),
            target_nodes: vec!["node-1".to_string(), "node-2".to_string()],
            created_jobs: Mutex::new(vec![]),
            job_results: Mutex::new(vec![]),
            created_vs: Mutex::new(vec![]),
        }
    }

    fn record(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }

    fn get_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn current_phase(&self) -> Phase {
        self.experiment_status.lock().unwrap().phase
    }

    fn current_experiment_status(&self) -> ChaosExperimentStatus {
        self.experiment_status.lock().unwrap().clone()
    }

    fn set_jobs_succeeded(&self) {
        let created = self.created_jobs.lock().unwrap().clone();
        let mut jobs = self.job_results.lock().unwrap();
        *jobs = created
            .iter()
            .map(|name| Job {
                metadata: ObjectMeta {
                    name: Some(name.clone()),
                    ..Default::default()
                },
                spec: None,
                status: Some(JobStatus {
                    succeeded: Some(1),
                    ..Default::default()
                }),
            })
            .collect();
    }

    fn set_jobs_failed(&self) {
        let created = self.created_jobs.lock().unwrap().clone();
        let mut jobs = self.job_results.lock().unwrap();
        *jobs = created
            .iter()
            .map(|name| Job {
                metadata: ObjectMeta {
                    name: Some(name.clone()),
                    ..Default::default()
                },
                spec: None,
                status: Some(JobStatus {
                    failed: Some(1),
                    ..Default::default()
                }),
            })
            .collect();
    }

    fn get_analysis_status(&self) -> Option<ChaosAnalysisStatus> {
        self.analysis_status.lock().unwrap().clone()
    }
}

#[async_trait]
impl KubeClient for StatefulKube {
    async fn create_job(&self, _ns: &str, job: &Job) -> Result<(), OperatorError> {
        let name = job.metadata.name.clone().unwrap_or_default();
        self.record(&format!("create_job:{name}"));
        self.created_jobs.lock().unwrap().push(name);
        Ok(())
    }

    async fn list_jobs(&self, _ns: &str, _label_selector: &str) -> Result<Vec<Job>, OperatorError> {
        self.record("list_jobs");
        Ok(self.job_results.lock().unwrap().clone())
    }

    async fn delete_job(&self, _ns: &str, name: &str) -> Result<(), OperatorError> {
        self.record(&format!("delete_job:{name}"));
        Ok(())
    }

    async fn list_target_nodes(&self, _ns: &str) -> Result<Vec<String>, OperatorError> {
        self.record("list_target_nodes");
        Ok(self.target_nodes.clone())
    }

    async fn get_service_selector(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<BTreeMap<String, String>, OperatorError> {
        self.record("get_service_selector");
        Ok(BTreeMap::from([("app".into(), "payment".into())]))
    }

    async fn create_virtual_service(
        &self,
        _ns: &str,
        vs_json: &serde_json::Value,
    ) -> Result<(), OperatorError> {
        self.record("create_virtual_service");
        let name = vs_json
            .pointer("/metadata/name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let labels: BTreeMap<String, String> = vs_json
            .pointer("/metadata/labels")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        self.created_vs
            .lock()
            .unwrap()
            .push(VirtualServiceInfo { name, labels });
        Ok(())
    }

    async fn list_virtual_services_for_host(
        &self,
        _ns: &str,
        _host: &str,
    ) -> Result<Vec<VirtualServiceInfo>, OperatorError> {
        self.record("list_virtual_services_for_host");
        Ok(self.created_vs.lock().unwrap().clone())
    }

    async fn delete_virtual_service(&self, _ns: &str, name: &str) -> Result<(), OperatorError> {
        self.record(&format!("delete_virtual_service:{name}"));
        self.created_vs.lock().unwrap().retain(|vs| vs.name != name);
        Ok(())
    }

    async fn patch_experiment_status(
        &self,
        _ns: &str,
        _name: &str,
        status: &ChaosExperimentStatus,
    ) -> Result<(), OperatorError> {
        self.record(&format!("patch_status:{}", status.phase));
        *self.experiment_status.lock().unwrap() = status.clone();
        Ok(())
    }

    async fn add_finalizer(&self, _ns: &str, _name: &str) -> Result<(), OperatorError> {
        self.record("add_finalizer");
        Ok(())
    }

    async fn remove_finalizer(&self, _ns: &str, _name: &str) -> Result<(), OperatorError> {
        self.record("remove_finalizer");
        Ok(())
    }
}

#[async_trait]
impl AnalysisKubeClient for StatefulKube {
    async fn get_experiment_status(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<ChaosExperimentStatus, OperatorError> {
        Ok(self.current_experiment_status())
    }

    async fn patch_analysis_status(
        &self,
        _ns: &str,
        _name: &str,
        status: &ChaosAnalysisStatus,
    ) -> Result<(), OperatorError> {
        self.record(&format!("patch_analysis:{:?}", status.phase));
        *self.analysis_status.lock().unwrap() = Some(status.clone());
        Ok(())
    }
}

// ══════════════════════════════════════════════════
// Mock Prometheus for analysis
// ══════════════════════════════════════════════════

struct MockProm {
    baseline: f64,
    during: f64,
    call_count: Mutex<u32>,
}

impl MockProm {
    fn new(baseline: f64, during: f64) -> Self {
        Self {
            baseline,
            during,
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl AnalysisPrometheusClient for MockProm {
    async fn query_at(&self, _promql: &str, _time: &str) -> Result<f64, OperatorError> {
        let mut count = self
            .call_count
            .lock()
            .map_err(|e| OperatorError::Analysis(e.to_string()))?;
        *count += 1;
        // First call = baseline, second call = during
        if *count == 1 {
            Ok(self.baseline)
        } else {
            Ok(self.during)
        }
    }
}

// ══════════════════════════════════════════════════
// Test helpers
// ══════════════════════════════════════════════════

fn pod_experiment_no_finalizer() -> ChaosExperiment {
    ChaosExperiment {
        metadata: ObjectMeta {
            name: Some("e2e-pod-killer".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: ChaosExperimentSpec {
            scenario: ScenarioType::PodKiller,
            duration: 60,
            target_namespace: Some("demo".into()),
            target: None,
            parameters: Some(serde_json::json!({"labelSelector": "app=cartservice"})),
        },
        status: None,
    }
}

fn with_finalizer(mut exp: ChaosExperiment) -> ChaosExperiment {
    exp.metadata.finalizers = Some(vec![FINALIZER_NAME.to_string()]);
    exp
}

fn with_status(mut exp: ChaosExperiment, status: ChaosExperimentStatus) -> ChaosExperiment {
    exp.metadata.finalizers = Some(vec![FINALIZER_NAME.to_string()]);
    exp.status = Some(status);
    exp
}

fn with_deletion(mut exp: ChaosExperiment) -> ChaosExperiment {
    exp.metadata.deletion_timestamp = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::now(),
    ));
    exp
}

fn analysis_for_experiment(direction: DegradationDirection, max_impact: u32) -> ChaosAnalysis {
    ChaosAnalysis {
        metadata: ObjectMeta {
            name: Some("e2e-analysis".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        spec: ChaosAnalysisSpec {
            experiment_ref: ExperimentRef {
                name: "e2e-pod-killer".into(),
                namespace: Some("default".into()),
            },
            prometheus: PrometheusConfig {
                url: "http://prometheus:9090".into(),
                baseline_window: "5m".into(),
            },
            query: "sum(rate(istio_requests_total[1m]))".into(),
            degradation_direction: direction,
            success_criteria: SuccessCriteria { max_impact },
        },
        status: None,
    }
}

fn edge_experiment_no_finalizer() -> ChaosExperiment {
    ChaosExperiment {
        metadata: ObjectMeta {
            name: Some("e2e-edge-delay".into()),
            namespace: Some("default".into()),
            uid: Some("uid-edge".into()),
            ..Default::default()
        },
        spec: ChaosExperimentSpec {
            scenario: ScenarioType::EdgeDelay,
            duration: 120,
            target_namespace: Some("demo".into()),
            target: Some(Target {
                namespace: Some("demo".into()),
                edge: Some(EdgeTarget {
                    source_service: "frontend".into(),
                    destination_service: "cartservice".into(),
                }),
            }),
            parameters: Some(serde_json::json!({"latencyMs": 500})),
        },
        status: None,
    }
}

fn default_config() -> ReconcilerConfig {
    ReconcilerConfig::default()
}

// ══════════════════════════════════════════════════
// E2E lifecycle: Pod Chaos
// ══════════════════════════════════════════════════

#[tokio::test]
async fn e2e_pod_chaos_full_lifecycle() {
    let kube = StatefulKube::new();
    let config = default_config();

    // ── Phase 1: Pending (no finalizer) → adds finalizer, requeues immediately ──
    let exp = pod_experiment_no_finalizer();
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(0)));
    assert!(kube.get_calls().contains(&"add_finalizer".to_string()));

    // ── Phase 2: Pending (with finalizer) → creates jobs, transitions to Running ──
    let exp = with_finalizer(pod_experiment_no_finalizer());
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(5)));
    assert_eq!(kube.current_phase(), Phase::Running);

    let status = kube.current_experiment_status();
    assert!(status.started_at.is_some());
    assert!(status.experiment_id.is_some());
    assert_eq!(status.runner_jobs.len(), 2, "should create job per node");
    assert!(status.message.as_deref().unwrap_or("").contains("2 nodes"));

    // ── Phase 3: Running, jobs not done yet → stays Running ──
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(5)));
    // Phase should remain Running (no jobs returned = not done)

    // ── Phase 4: Running, all jobs succeeded → transitions to Succeeded ──
    kube.set_jobs_succeeded();
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(5)));
    assert_eq!(kube.current_phase(), Phase::Succeeded);
    assert!(kube.current_experiment_status().completed_at.is_some());

    // ── Phase 5: Succeeded → cleanup (delete jobs, mark cleanup_done) ──
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(5)));
    assert!(kube.current_experiment_status().cleanup_done);
    assert!(kube
        .get_calls()
        .iter()
        .any(|c| c.starts_with("delete_job:")));

    // ── Phase 6: Succeeded + cleanup_done → done (no more requeue) ──
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    let calls_before = kube.get_calls().len();
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Done);
    let calls_after = kube.get_calls().len();
    // Only list_jobs call, no more delete_job or patch_status
    assert!(
        calls_after - calls_before <= 1,
        "should be near no-op after cleanup"
    );
}

#[tokio::test]
async fn e2e_pod_chaos_job_failure() {
    let kube = StatefulKube::new();
    let config = default_config();

    // Pending → Running
    let exp = with_finalizer(pod_experiment_no_finalizer());
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Running);

    // Running + failed jobs → Failed
    kube.set_jobs_failed();
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Failed);
    assert!(kube
        .current_experiment_status()
        .message
        .as_deref()
        .unwrap_or("")
        .contains("failed"));
}

#[tokio::test]
async fn e2e_pod_chaos_deletion_during_running() {
    let kube = StatefulKube::new();
    let config = default_config();

    // Pending → Running
    let exp = with_finalizer(pod_experiment_no_finalizer());
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Running);

    // User deletes CR while Running → cleanup + remove finalizer
    let exp = with_deletion(with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    ));
    let result = reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(result, ReconcileResult::Done);
    assert!(kube.get_calls().contains(&"remove_finalizer".to_string()));
    assert!(kube
        .get_calls()
        .iter()
        .any(|c| c.starts_with("delete_job:")));
}

// ══════════════════════════════════════════════════
// E2E lifecycle: Edge Chaos
// ══════════════════════════════════════════════════

struct MockEdge;

#[async_trait]
impl EdgeResolver for MockEdge {
    async fn resolve_edge(
        &self,
        _source: &str,
        _dest: &str,
        _ns: &str,
    ) -> Result<EdgeInfo, OperatorError> {
        Ok(EdgeInfo {
            source_workload: "frontend".into(),
            source_namespace: "demo".into(),
            destination_workload: "cartservice".into(),
            destination_namespace: "demo".into(),
            destination_service: "cartservice".into(),
            rps: 50.0,
            source_labels: BTreeMap::new(),
        })
    }
}

#[tokio::test]
async fn e2e_edge_chaos_full_lifecycle() {
    let kube = StatefulKube::new();
    let config = default_config();
    let edge = MockEdge;

    // ── Pending (no finalizer) → add finalizer ──
    let exp = edge_experiment_no_finalizer();
    reconcile(&exp, &kube, Some(&edge), &config).await.unwrap();
    assert!(kube.get_calls().contains(&"add_finalizer".to_string()));

    // ── Pending → creates VirtualService, transitions to Running ──
    let exp = with_finalizer(edge_experiment_no_finalizer());
    reconcile(&exp, &kube, Some(&edge), &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Running);
    assert!(kube
        .get_calls()
        .contains(&"create_virtual_service".to_string()));
    assert!(kube
        .get_calls()
        .contains(&"get_service_selector".to_string()));

    // ── Running, duration elapsed (override started_at to past) → Succeeded ──
    let mut status = kube.current_experiment_status();
    status.started_at = Some("2020-01-01T00:00:00Z".to_string());
    let exp = with_status(edge_experiment_no_finalizer(), status);
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Succeeded);

    // ── Succeeded → cleanup (delete VirtualService) ──
    let mut exp = with_status(
        edge_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    exp.spec.scenario = ScenarioType::EdgeDelay; // ensure edge cleanup path
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert!(kube
        .get_calls()
        .iter()
        .any(|c| c.starts_with("delete_virtual_service:")));
    assert!(kube.current_experiment_status().cleanup_done);
}

// ══════════════════════════════════════════════════
// E2E: Experiment → Analysis (full pipeline)
// ══════════════════════════════════════════════════

#[tokio::test]
async fn e2e_experiment_then_analysis_pass() {
    let kube = StatefulKube::new();
    let config = default_config();

    // ── Run full experiment lifecycle ──
    let exp = with_finalizer(pod_experiment_no_finalizer());
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Running);

    kube.set_jobs_succeeded();
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Succeeded);

    // ── Now run analysis against the completed experiment ──
    // Success rate: baseline=0.99, during=0.95 → 4% drop, direction=down, threshold=30 → Pass
    let prom = MockProm::new(0.99, 0.95);
    let analysis = analysis_for_experiment(DegradationDirection::Down, 30);

    reconcile_analysis(&analysis, &kube, &prom).await.unwrap();

    let status = kube
        .get_analysis_status()
        .expect("analysis should be patched");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Pass));
    assert!(status.impact_score.unwrap_or(100) <= 30);
    assert!(status.baseline_value.is_some());
    assert!(status.during_value.is_some());
    assert!(status.message.is_some());
}

#[tokio::test]
async fn e2e_experiment_then_analysis_fail() {
    let kube = StatefulKube::new();
    let config = default_config();

    // ── Run experiment to Succeeded ──
    let exp = with_finalizer(pod_experiment_no_finalizer());
    reconcile(&exp, &kube, None, &config).await.unwrap();
    kube.set_jobs_succeeded();
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Succeeded);

    // ── Analysis: latency spiked 80% → Fail ──
    let prom = MockProm::new(0.10, 0.18);
    let analysis = analysis_for_experiment(DegradationDirection::Up, 30);

    reconcile_analysis(&analysis, &kube, &prom).await.unwrap();

    let status = kube
        .get_analysis_status()
        .expect("analysis should be patched");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Fail));
    assert_eq!(status.impact_score, Some(80));
}

#[tokio::test]
async fn e2e_analysis_waits_for_running_experiment() {
    let kube = StatefulKube::new();
    let config = default_config();

    // ── Start experiment but don't complete it ──
    let exp = with_finalizer(pod_experiment_no_finalizer());
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Running);

    // ── Analysis sees Running experiment → Pending ──
    let prom = MockProm::new(1.0, 1.0);
    let analysis = analysis_for_experiment(DegradationDirection::Down, 30);

    reconcile_analysis(&analysis, &kube, &prom).await.unwrap();

    let status = kube
        .get_analysis_status()
        .expect("analysis should be patched");
    assert_eq!(status.phase, AnalysisPhase::Pending);
    assert!(status.message.as_deref().unwrap_or("").contains("Waiting"));

    // ── Complete the experiment ──
    kube.set_jobs_succeeded();
    let exp = with_status(
        pod_experiment_no_finalizer(),
        kube.current_experiment_status(),
    );
    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Succeeded);

    // ── Re-reconcile analysis → now completes (new prom so call count resets) ──
    let prom2 = MockProm::new(1.0, 1.0);
    reconcile_analysis(&analysis, &kube, &prom2).await.unwrap();

    let status = kube
        .get_analysis_status()
        .expect("analysis should be patched");
    assert_eq!(status.phase, AnalysisPhase::Completed);
    assert_eq!(status.verdict, Some(AnalysisVerdict::Pass));
}

#[tokio::test]
async fn e2e_validation_failure_zero_duration() {
    let kube = StatefulKube::new();
    let config = default_config();

    let mut exp = with_finalizer(pod_experiment_no_finalizer());
    exp.spec.duration = 0;

    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Failed);
    assert!(kube
        .current_experiment_status()
        .message
        .as_deref()
        .unwrap_or("")
        .contains("duration"));
}

#[tokio::test]
async fn e2e_validation_failure_edge_missing_target() {
    let kube = StatefulKube::new();
    let config = default_config();

    let mut exp = with_finalizer(edge_experiment_no_finalizer());
    exp.spec.target = None;

    reconcile(&exp, &kube, None, &config).await.unwrap();
    assert_eq!(kube.current_phase(), Phase::Failed);
}
