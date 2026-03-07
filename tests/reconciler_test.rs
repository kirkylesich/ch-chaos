use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{Job, JobStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use chimp_chaos::operator::crd::{ChaosExperiment, ChaosExperimentSpec, ChaosExperimentStatus};
use chimp_chaos::operator::reconciler::*;
use chimp_chaos::operator::types::*;

// ── Call tracking ──

#[derive(Debug, Clone, PartialEq)]
enum Call {
    AddFinalizer(String, String),
    RemoveFinalizer(String, String),
    PatchStatus(String, String, Phase),
    CreateJob(String),
    ListJobs(String, String),
    DeleteJob(String, String),
    ListTargetNodes(String),
    GetServiceSelector(String, String),
    CreateVirtualService(String),
    ListVirtualServicesForHost(String, String),
    DeleteVirtualService(String, String),
}

// ── Mock KubeClient ──

struct MockKube {
    calls: Mutex<Vec<Call>>,
    target_nodes: Vec<String>,
    service_selector: Result<BTreeMap<String, String>, OperatorError>,
    existing_vs: Vec<VirtualServiceInfo>,
    jobs: Vec<Job>,
}

impl MockKube {
    fn new() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            target_nodes: vec!["node-1".to_string()],
            service_selector: Ok(BTreeMap::from([("app".into(), "payment".into())])),
            existing_vs: vec![],
            jobs: vec![],
        }
    }

    fn with_no_nodes() -> Self {
        Self {
            target_nodes: vec![],
            ..Self::new()
        }
    }

    fn with_jobs(jobs: Vec<Job>) -> Self {
        Self {
            jobs,
            ..Self::new()
        }
    }

    fn with_conflicting_vs() -> Self {
        Self {
            existing_vs: vec![VirtualServiceInfo {
                name: "existing-vs".to_string(),
                labels: BTreeMap::from([("app".into(), "other".into())]),
            }],
            ..Self::new()
        }
    }

    fn with_empty_selector() -> Self {
        Self {
            service_selector: Ok(BTreeMap::new()),
            ..Self::new()
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn has_call(&self, call: &Call) -> bool {
        self.calls().contains(call)
    }
}

#[async_trait]
impl KubeClient for MockKube {
    async fn create_job(&self, ns: &str, job: &Job) -> Result<(), OperatorError> {
        let name = job.metadata.name.clone().unwrap_or_default();
        self.calls.lock().unwrap().push(Call::CreateJob(name));
        let _ = ns;
        Ok(())
    }

    async fn list_jobs(&self, ns: &str, label_selector: &str) -> Result<Vec<Job>, OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::ListJobs(ns.into(), label_selector.into()));
        Ok(self.jobs.clone())
    }

    async fn delete_job(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::DeleteJob(ns.into(), name.into()));
        Ok(())
    }

    async fn list_target_nodes(&self, ns: &str) -> Result<Vec<String>, OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::ListTargetNodes(ns.into()));
        Ok(self.target_nodes.clone())
    }

    async fn get_service_selector(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<BTreeMap<String, String>, OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::GetServiceSelector(ns.into(), name.into()));
        match &self.service_selector {
            Ok(m) => Ok(m.clone()),
            Err(_) => Err(OperatorError::Prometheus("not found".into())),
        }
    }

    async fn create_virtual_service(
        &self,
        ns: &str,
        _vs_json: &serde_json::Value,
    ) -> Result<(), OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::CreateVirtualService(ns.into()));
        Ok(())
    }

    async fn list_virtual_services_for_host(
        &self,
        ns: &str,
        host: &str,
    ) -> Result<Vec<VirtualServiceInfo>, OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::ListVirtualServicesForHost(ns.into(), host.into()));
        Ok(self.existing_vs.clone())
    }

    async fn delete_virtual_service(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::DeleteVirtualService(ns.into(), name.into()));
        Ok(())
    }

    async fn patch_experiment_status(
        &self,
        ns: &str,
        name: &str,
        status: &ChaosExperimentStatus,
    ) -> Result<(), OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::PatchStatus(ns.into(), name.into(), status.phase));
        Ok(())
    }

    async fn add_finalizer(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::AddFinalizer(ns.into(), name.into()));
        Ok(())
    }

    async fn remove_finalizer(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::RemoveFinalizer(ns.into(), name.into()));
        Ok(())
    }
}

// ── Mock EdgeResolver ──

struct MockEdgeResolver {
    result: Result<EdgeInfo, OperatorError>,
}

impl MockEdgeResolver {
    fn ok() -> Self {
        Self {
            result: Ok(EdgeInfo {
                source_workload: "payment".into(),
                source_namespace: "production".into(),
                destination_workload: "ledger".into(),
                destination_namespace: "production".into(),
                destination_service: "ledger".into(),
                rps: 12.5,
                source_labels: BTreeMap::new(),
            }),
        }
    }

    fn not_found() -> Self {
        Self {
            result: Err(OperatorError::Validation(ValidationError::EdgeNotFound(
                "payment".into(),
                "ledger".into(),
            ))),
        }
    }
}

#[async_trait]
impl EdgeResolver for MockEdgeResolver {
    async fn resolve_edge(
        &self,
        _source: &str,
        _dest: &str,
        _ns: &str,
    ) -> Result<EdgeInfo, OperatorError> {
        match &self.result {
            Ok(e) => Ok(e.clone()),
            Err(_) => Err(OperatorError::Validation(ValidationError::EdgeNotFound(
                "payment".into(),
                "ledger".into(),
            ))),
        }
    }
}

// ── Test helpers ──

fn pod_experiment(phase: Phase) -> ChaosExperiment {
    pod_experiment_with(phase, true)
}

fn pod_experiment_with(phase: Phase, has_finalizer: bool) -> ChaosExperiment {
    let finalizers = if has_finalizer {
        Some(vec![FINALIZER_NAME.to_string()])
    } else {
        None
    };

    ChaosExperiment {
        metadata: ObjectMeta {
            name: Some("test-exp".into()),
            namespace: Some("default".into()),
            finalizers,
            ..Default::default()
        },
        spec: ChaosExperimentSpec {
            scenario: ScenarioType::CpuStress,
            duration: 300,
            target_namespace: Some("production".into()),
            target: None,
            parameters: Some(serde_json::json!({"cores": 2})),
        },
        status: Some(ChaosExperimentStatus {
            phase,
            started_at: Some("2020-01-01T00:00:00Z".to_string()),
            experiment_id: Some("abc12345".to_string()),
            runner_jobs: vec!["chaos-runner-abc12345-node-1".to_string()],
            ..Default::default()
        }),
    }
}

fn edge_experiment(phase: Phase) -> ChaosExperiment {
    ChaosExperiment {
        metadata: ObjectMeta {
            name: Some("edge-exp".into()),
            namespace: Some("default".into()),
            uid: Some("uid-1234".into()),
            finalizers: Some(vec![FINALIZER_NAME.to_string()]),
            ..Default::default()
        },
        spec: ChaosExperimentSpec {
            scenario: ScenarioType::EdgeDelay,
            duration: 600,
            target_namespace: Some("production".into()),
            target: Some(Target {
                namespace: Some("production".into()),
                edge: Some(EdgeTarget {
                    source_service: "payment".into(),
                    destination_service: "ledger".into(),
                }),
            }),
            parameters: Some(serde_json::json!({"latencyMs": 200})),
        },
        status: Some(ChaosExperimentStatus {
            phase,
            started_at: Some("2020-01-01T00:00:00Z".to_string()),
            experiment_id: Some("edge12345".to_string()),
            ..Default::default()
        }),
    }
}

fn deleted_experiment() -> ChaosExperiment {
    let mut exp = pod_experiment(Phase::Running);
    exp.metadata.deletion_timestamp = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::now(),
    ));
    exp
}

fn default_config() -> ReconcilerConfig {
    ReconcilerConfig::default()
}

fn job_with_status(succeeded: Option<i32>, failed: Option<i32>) -> Job {
    Job {
        metadata: ObjectMeta::default(),
        spec: None,
        status: Some(JobStatus {
            succeeded,
            failed,
            ..Default::default()
        }),
    }
}

// ── Pure function tests ──

#[test]
fn needs_finalizer_true_when_no_finalizers() {
    let exp = pod_experiment_with(Phase::Pending, false);
    assert!(needs_finalizer(&exp));
}

#[test]
fn needs_finalizer_false_when_present() {
    let exp = pod_experiment(Phase::Pending);
    assert!(!needs_finalizer(&exp));
}

#[test]
fn is_being_deleted_true() {
    let exp = deleted_experiment();
    assert!(is_being_deleted(&exp));
}

#[test]
fn is_being_deleted_false() {
    let exp = pod_experiment(Phase::Running);
    assert!(!is_being_deleted(&exp));
}

#[test]
fn target_namespace_from_spec() {
    let exp = pod_experiment(Phase::Pending);
    assert_eq!(target_namespace(&exp), "production");
}

#[test]
fn target_namespace_defaults_to_metadata() {
    let mut exp = pod_experiment(Phase::Pending);
    exp.spec.target_namespace = None;
    assert_eq!(target_namespace(&exp), "default");
}

#[test]
fn requeue_running_is_5s() {
    assert_eq!(requeue_duration(Phase::Running), Duration::from_secs(5));
}

#[test]
fn requeue_pending_is_5s() {
    assert_eq!(requeue_duration(Phase::Pending), Duration::from_secs(5));
}

#[test]
fn requeue_terminal_is_300s() {
    assert_eq!(requeue_duration(Phase::Succeeded), Duration::from_secs(300));
    assert_eq!(requeue_duration(Phase::Failed), Duration::from_secs(300));
}

#[test]
fn duration_elapsed_past() {
    assert!(is_duration_elapsed("2020-01-01T00:00:00Z", 300));
}

#[test]
fn duration_elapsed_future() {
    assert!(!is_duration_elapsed("2099-01-01T00:00:00Z", 300));
}

#[test]
fn duration_elapsed_invalid_date() {
    assert!(!is_duration_elapsed("not-a-date", 300));
}

#[test]
fn validate_ok_pod_chaos() {
    let spec = ChaosExperimentSpec {
        scenario: ScenarioType::CpuStress,
        duration: 300,
        target_namespace: None,
        target: None,
        parameters: None,
    };
    assert!(validate_experiment(&spec).is_ok());
}

#[test]
fn validate_zero_duration() {
    let spec = ChaosExperimentSpec {
        scenario: ScenarioType::CpuStress,
        duration: 0,
        target_namespace: None,
        target: None,
        parameters: None,
    };
    assert!(matches!(
        validate_experiment(&spec),
        Err(ValidationError::InvalidDuration)
    ));
}

#[test]
fn validate_edge_missing_target() {
    let spec = ChaosExperimentSpec {
        scenario: ScenarioType::EdgeDelay,
        duration: 300,
        target_namespace: None,
        target: None,
        parameters: None,
    };
    assert!(matches!(
        validate_experiment(&spec),
        Err(ValidationError::MissingEdgeTarget)
    ));
}

#[test]
fn validate_edge_empty_source() {
    let spec = ChaosExperimentSpec {
        scenario: ScenarioType::EdgeDelay,
        duration: 300,
        target_namespace: None,
        target: Some(Target {
            namespace: None,
            edge: Some(EdgeTarget {
                source_service: "".into(),
                destination_service: "ledger".into(),
            }),
        }),
        parameters: None,
    };
    assert!(matches!(
        validate_experiment(&spec),
        Err(ValidationError::MissingEdgeTarget)
    ));
}

#[test]
fn validate_edge_ok() {
    let spec = ChaosExperimentSpec {
        scenario: ScenarioType::EdgeDelay,
        duration: 300,
        target_namespace: None,
        target: Some(Target {
            namespace: None,
            edge: Some(EdgeTarget {
                source_service: "payment".into(),
                destination_service: "ledger".into(),
            }),
        }),
        parameters: None,
    };
    assert!(validate_experiment(&spec).is_ok());
}

#[test]
fn all_jobs_succeeded_true() {
    let jobs = vec![job_with_status(Some(1), None), job_with_status(Some(1), None)];
    assert!(all_jobs_succeeded(&jobs));
}

#[test]
fn all_jobs_succeeded_false_empty() {
    assert!(!all_jobs_succeeded(&[]));
}

#[test]
fn all_jobs_succeeded_false_mixed() {
    let jobs = vec![job_with_status(Some(1), None), job_with_status(None, None)];
    assert!(!all_jobs_succeeded(&jobs));
}

#[test]
fn any_job_failed_true() {
    let jobs = vec![job_with_status(None, Some(1))];
    assert!(any_job_failed(&jobs));
}

#[test]
fn any_job_failed_false() {
    let jobs = vec![job_with_status(Some(1), None)];
    assert!(!any_job_failed(&jobs));
}

// ── Reconcile flow tests ──

#[tokio::test]
async fn reconcile_adds_finalizer_if_missing() {
    let exp = pod_experiment_with(Phase::Pending, false);
    let kube = MockKube::new();
    let config = default_config();

    let result = reconcile(&exp, &kube, None, &config).await.unwrap();

    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(0)));
    assert!(kube.has_call(&Call::AddFinalizer("default".into(), "test-exp".into())));
}

#[tokio::test]
async fn reconcile_handles_deletion() {
    let exp = deleted_experiment();
    let kube = MockKube::new();
    let config = default_config();

    let result = reconcile(&exp, &kube, None, &config).await.unwrap();

    assert_eq!(result, ReconcileResult::Done);
    assert!(kube.has_call(&Call::DeleteJob(
        "default".into(),
        "chaos-runner-abc12345-node-1".into()
    )));
    assert!(kube.has_call(&Call::RemoveFinalizer("default".into(), "test-exp".into())));
}

#[tokio::test]
async fn reconcile_pending_pod_creates_jobs() {
    let exp = pod_experiment(Phase::Pending);
    let kube = MockKube::new();
    let config = default_config();

    let result = reconcile(&exp, &kube, None, &config).await.unwrap();

    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(5)));
    assert!(kube.has_call(&Call::ListTargetNodes("production".into())));
    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::CreateJob(_))));
    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Running))));
}

#[tokio::test]
async fn reconcile_pending_pod_no_nodes_fails() {
    let exp = pod_experiment(Phase::Pending);
    let kube = MockKube::with_no_nodes();
    let config = default_config();

    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Failed))));
}

#[tokio::test]
async fn reconcile_pending_invalid_duration_fails() {
    let mut exp = pod_experiment(Phase::Pending);
    exp.spec.duration = 0;
    let kube = MockKube::new();
    let config = default_config();

    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Failed))));
}

#[tokio::test]
async fn reconcile_pending_edge_creates_vs() {
    let exp = edge_experiment(Phase::Pending);
    let kube = MockKube::new();
    let resolver = MockEdgeResolver::ok();
    let config = default_config();

    let result = reconcile(&exp, &kube, Some(&resolver), &config)
        .await
        .unwrap();

    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(5)));
    assert!(kube.has_call(&Call::GetServiceSelector(
        "production".into(),
        "payment".into()
    )));
    assert!(kube.has_call(&Call::CreateVirtualService("production".into())));
    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Running))));
}

#[tokio::test]
async fn reconcile_pending_edge_conflict_fails() {
    let exp = edge_experiment(Phase::Pending);
    let kube = MockKube::with_conflicting_vs();
    let resolver = MockEdgeResolver::ok();
    let config = default_config();

    reconcile(&exp, &kube, Some(&resolver), &config)
        .await
        .unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Failed))));
}

#[tokio::test]
async fn reconcile_pending_edge_no_selector_fails() {
    let exp = edge_experiment(Phase::Pending);
    let kube = MockKube::with_empty_selector();
    let resolver = MockEdgeResolver::ok();
    let config = default_config();

    reconcile(&exp, &kube, Some(&resolver), &config)
        .await
        .unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Failed))));
}

#[tokio::test]
async fn reconcile_pending_edge_not_found_fails() {
    let exp = edge_experiment(Phase::Pending);
    let kube = MockKube::new();
    let resolver = MockEdgeResolver::not_found();
    let config = default_config();

    reconcile(&exp, &kube, Some(&resolver), &config)
        .await
        .unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Failed))));
}

#[tokio::test]
async fn reconcile_running_pod_all_succeeded() {
    let exp = pod_experiment(Phase::Running);
    let kube = MockKube::with_jobs(vec![job_with_status(Some(1), None)]);
    let config = default_config();

    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Succeeded))));
}

#[tokio::test]
async fn reconcile_running_pod_job_failed() {
    let exp = pod_experiment(Phase::Running);
    let kube = MockKube::with_jobs(vec![job_with_status(None, Some(1))]);
    let config = default_config();

    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Failed))));
}

#[tokio::test]
async fn reconcile_running_edge_duration_elapsed() {
    let exp = edge_experiment(Phase::Running);
    let kube = MockKube::new();
    let config = default_config();

    // started_at is 2020, so duration is definitely elapsed
    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Succeeded))));
}

#[tokio::test]
async fn reconcile_terminal_does_cleanup() {
    let exp = pod_experiment(Phase::Succeeded);
    let kube = MockKube::new();
    let config = default_config();

    let result = reconcile(&exp, &kube, None, &config).await.unwrap();

    assert_eq!(result, ReconcileResult::Requeue(Duration::from_secs(300)));
    assert!(kube.has_call(&Call::DeleteJob(
        "default".into(),
        "chaos-runner-abc12345-node-1".into()
    )));
    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::PatchStatus(_, _, Phase::Succeeded))));
}

#[tokio::test]
async fn reconcile_terminal_skips_cleanup_if_done() {
    let mut exp = pod_experiment(Phase::Succeeded);
    if let Some(status) = exp.status.as_mut() {
        status.cleanup_done = true;
    }
    let kube = MockKube::new();
    let config = default_config();

    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(!kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::DeleteJob(_, _))));
}

#[tokio::test]
async fn reconcile_deletion_edge_deletes_vs() {
    let mut exp = edge_experiment(Phase::Running);
    exp.metadata.deletion_timestamp = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        k8s_openapi::jiff::Timestamp::now(),
    ));
    let kube = MockKube::new();
    let config = default_config();

    reconcile(&exp, &kube, None, &config).await.unwrap();

    assert!(kube
        .calls()
        .iter()
        .any(|c| matches!(c, Call::DeleteVirtualService(_, _))));
    assert!(kube.has_call(&Call::RemoveFinalizer("default".into(), "edge-exp".into())));
}
