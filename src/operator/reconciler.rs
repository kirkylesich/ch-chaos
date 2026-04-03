use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use k8s_openapi::api::batch::v1::Job;

use super::crd::{ChaosExperiment, ChaosExperimentSpec, ChaosExperimentStatus};
use super::istio_injector::{
    build_fault_for_scenario, build_virtual_service_json, fqdn, is_chaos_managed,
    virtual_service_name, VirtualServiceSpec,
};
use super::job_builder::{build_runner_job, JobBuilderConfig};
use super::types::*;

// ── Traits for dependency injection ──

#[async_trait]
pub trait KubeClient: Send + Sync {
    async fn create_job(&self, ns: &str, job: &Job) -> Result<(), OperatorError>;
    async fn list_jobs(&self, ns: &str, label_selector: &str) -> Result<Vec<Job>, OperatorError>;
    async fn delete_job(&self, ns: &str, name: &str) -> Result<(), OperatorError>;
    async fn list_target_nodes(&self, ns: &str) -> Result<Vec<String>, OperatorError>;
    async fn get_service_selector(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<BTreeMap<String, String>, OperatorError>;
    async fn create_virtual_service(
        &self,
        ns: &str,
        vs_json: &serde_json::Value,
    ) -> Result<(), OperatorError>;
    async fn list_virtual_services_for_host(
        &self,
        ns: &str,
        host: &str,
    ) -> Result<Vec<VirtualServiceInfo>, OperatorError>;
    async fn delete_virtual_service(&self, ns: &str, name: &str) -> Result<(), OperatorError>;
    async fn patch_experiment_status(
        &self,
        ns: &str,
        name: &str,
        status: &ChaosExperimentStatus,
    ) -> Result<(), OperatorError>;
    async fn add_finalizer(&self, ns: &str, name: &str) -> Result<(), OperatorError>;
    async fn remove_finalizer(&self, ns: &str, name: &str) -> Result<(), OperatorError>;
}

#[async_trait]
pub trait EdgeResolver: Send + Sync {
    async fn resolve_edge(
        &self,
        source: &str,
        destination: &str,
        namespace: &str,
    ) -> Result<EdgeInfo, OperatorError>;
}

#[derive(Clone)]
pub struct VirtualServiceInfo {
    pub name: String,
    pub labels: BTreeMap<String, String>,
}

// ── Config ──

#[derive(Default)]
pub struct ReconcilerConfig {
    pub job_builder: JobBuilderConfig,
}

// ── Result type ──

#[derive(Debug, PartialEq)]
pub enum ReconcileResult {
    Requeue(Duration),
    Done,
}

// ── Pure helper functions ──

pub fn needs_finalizer(experiment: &ChaosExperiment) -> bool {
    experiment
        .metadata
        .finalizers
        .as_ref()
        .map(|f| !f.contains(&FINALIZER_NAME.to_string()))
        .unwrap_or(true)
}

pub fn is_being_deleted(experiment: &ChaosExperiment) -> bool {
    experiment.metadata.deletion_timestamp.is_some()
}

pub fn target_namespace(experiment: &ChaosExperiment) -> &str {
    experiment.spec.target_namespace.as_deref().unwrap_or(
        experiment
            .metadata
            .namespace
            .as_deref()
            .unwrap_or("default"),
    )
}

pub fn requeue_duration(phase: Phase) -> Duration {
    if phase.is_terminal() {
        Duration::from_secs(REQUEUE_TERMINAL_SECS)
    } else {
        Duration::from_secs(REQUEUE_RUNNING_SECS)
    }
}

pub fn is_duration_elapsed(started_at: &str, duration_secs: u64) -> bool {
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return false;
    };
    let elapsed = Utc::now().signed_duration_since(start.with_timezone(&Utc));
    elapsed.num_seconds() >= duration_secs as i64
}

pub fn validate_experiment(spec: &ChaosExperimentSpec) -> Result<(), ValidationError> {
    if spec.duration == 0 {
        return Err(ValidationError::InvalidDuration);
    }
    if spec.scenario.is_edge_chaos() {
        let edge = spec
            .target
            .as_ref()
            .and_then(|t| t.edge.as_ref())
            .ok_or(ValidationError::MissingEdgeTarget)?;
        if edge.source_service.is_empty() || edge.destination_service.is_empty() {
            return Err(ValidationError::MissingEdgeTarget);
        }
    }
    Ok(())
}

pub fn all_jobs_succeeded(jobs: &[Job]) -> bool {
    !jobs.is_empty()
        && jobs
            .iter()
            .all(|j| j.status.as_ref().and_then(|s| s.succeeded).unwrap_or(0) > 0)
}

pub fn any_job_failed(jobs: &[Job]) -> bool {
    jobs.iter()
        .any(|j| j.status.as_ref().and_then(|s| s.failed).unwrap_or(0) > 0)
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn exp_name(experiment: &ChaosExperiment) -> &str {
    experiment.metadata.name.as_deref().unwrap_or("unknown")
}

fn exp_namespace(experiment: &ChaosExperiment) -> &str {
    experiment
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default")
}

// ── Reconcile orchestrator ──

pub async fn reconcile(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
    edge_resolver: Option<&dyn EdgeResolver>,
    config: &ReconcilerConfig,
) -> Result<ReconcileResult, OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);

    if needs_finalizer(experiment) {
        kube.add_finalizer(ns, name).await?;
        return Ok(ReconcileResult::Requeue(Duration::from_secs(0)));
    }

    if is_being_deleted(experiment) {
        handle_deletion(experiment, kube).await?;
        return Ok(ReconcileResult::Done);
    }

    let status = experiment.status.as_ref().cloned().unwrap_or_default();

    match status.phase {
        Phase::Pending => {
            handle_pending(experiment, kube, edge_resolver, config).await?;
            Ok(ReconcileResult::Requeue(requeue_duration(Phase::Running)))
        }
        Phase::Running => {
            handle_running(experiment, kube).await?;
            Ok(ReconcileResult::Requeue(requeue_duration(Phase::Running)))
        }
        Phase::Succeeded | Phase::Failed => {
            handle_terminal(experiment, kube).await?;
            Ok(ReconcileResult::Requeue(requeue_duration(status.phase)))
        }
    }
}

// ── Deletion ──

async fn handle_deletion(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
) -> Result<(), OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);
    let status = experiment.status.as_ref().cloned().unwrap_or_default();

    for job_name in &status.runner_jobs {
        let _ = kube.delete_job(ns, job_name).await;
    }

    if experiment.spec.scenario.is_edge_chaos() {
        if let Some(eid) = &status.experiment_id {
            let vs_name = virtual_service_name(eid);
            let _ = kube
                .delete_virtual_service(target_namespace(experiment), &vs_name)
                .await;
        }
    }

    kube.remove_finalizer(ns, name).await
}

// ── Pending ──

async fn handle_pending(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
    edge_resolver: Option<&dyn EdgeResolver>,
    config: &ReconcilerConfig,
) -> Result<(), OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);

    if let Err(e) = validate_experiment(&experiment.spec) {
        return patch_failed(kube, ns, name, &e.to_string()).await;
    }

    let eid = ExperimentId::new().to_string();

    if experiment.spec.scenario.is_edge_chaos() {
        handle_pending_edge(experiment, kube, edge_resolver, &eid).await
    } else {
        handle_pending_pod(experiment, kube, &eid, config).await
    }
}

async fn handle_pending_pod(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
    experiment_id: &str,
    config: &ReconcilerConfig,
) -> Result<(), OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);
    let target_ns = target_namespace(experiment);

    let nodes = kube.list_target_nodes(target_ns).await?;
    if nodes.is_empty() {
        return patch_failed(kube, ns, name, &ValidationError::NoTargetNodes.to_string()).await;
    }

    let params_json = {
        let mut params = experiment
            .spec
            .parameters
            .clone()
            .unwrap_or(serde_json::json!({}));
        if let Some(obj) = params.as_object_mut() {
            obj.entry("namespace")
                .or_insert_with(|| serde_json::json!(target_ns));
        }
        Some(params.to_string())
    };
    let mut job_names = Vec::new();

    for node in &nodes {
        let job = build_runner_job(
            experiment,
            experiment_id,
            node,
            experiment.spec.scenario,
            experiment.spec.duration,
            params_json.as_deref(),
            &config.job_builder,
        );
        let job_name = job.metadata.name.clone().unwrap_or_default();
        kube.create_job(ns, &job).await?;
        job_names.push(job_name);
    }

    let status = ChaosExperimentStatus {
        phase: Phase::Running,
        message: Some(format!(
            "Running on {} nodes for {}s",
            nodes.len(),
            experiment.spec.duration
        )),
        started_at: Some(now_rfc3339()),
        experiment_id: Some(experiment_id.to_string()),
        runner_jobs: job_names,
        ..Default::default()
    };
    kube.patch_experiment_status(ns, name, &status).await
}

async fn handle_pending_edge(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
    edge_resolver: Option<&dyn EdgeResolver>,
    experiment_id: &str,
) -> Result<(), OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);
    let target_ns = target_namespace(experiment);

    let edge_target = experiment
        .spec
        .target
        .as_ref()
        .and_then(|t| t.edge.as_ref())
        .ok_or(OperatorError::Validation(
            ValidationError::MissingEdgeTarget,
        ))?;

    // 1. Resolve source workload labels
    let source_labels = match kube
        .get_service_selector(target_ns, &edge_target.source_service)
        .await
    {
        Ok(labels) if !labels.is_empty() => labels,
        _ => {
            let msg = ValidationError::SourceServiceNotFound(edge_target.source_service.clone())
                .to_string();
            return patch_failed(kube, ns, name, &msg).await;
        }
    };

    // 2. Check VirtualService conflicts
    let dest_fqdn = fqdn(&edge_target.destination_service, target_ns);
    let existing_vs = kube
        .list_virtual_services_for_host(target_ns, &dest_fqdn)
        .await?;

    for vs in &existing_vs {
        if !is_chaos_managed(&vs.labels) {
            let msg = ValidationError::ConflictingVirtualService(dest_fqdn.clone()).to_string();
            return patch_failed(kube, ns, name, &msg).await;
        }
    }

    // 3. Resolve edge in observed graph
    if let Some(resolver) = edge_resolver {
        if let Err(e) = resolver
            .resolve_edge(
                &edge_target.source_service,
                &edge_target.destination_service,
                target_ns,
            )
            .await
        {
            return patch_failed(kube, ns, name, &e.to_string()).await;
        }
    }

    // 4. Build fault
    let params = experiment
        .spec
        .parameters
        .as_ref()
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let fault = match build_fault_for_scenario(experiment.spec.scenario, &params) {
        Some(f) => f,
        None => {
            return patch_failed(kube, ns, name, "cannot build fault for scenario").await;
        }
    };

    // 5. Create VirtualService
    let vs_spec = VirtualServiceSpec {
        name: virtual_service_name(experiment_id),
        namespace: target_ns.to_string(),
        experiment_name: name.to_string(),
        experiment_uid: experiment.metadata.uid.clone().unwrap_or_default(),
        destination_fqdn: dest_fqdn,
        source_labels,
        fault,
    };
    let vs_json = build_virtual_service_json(&vs_spec);
    kube.create_virtual_service(target_ns, &vs_json).await?;

    let status = ChaosExperimentStatus {
        phase: Phase::Running,
        message: Some(format!(
            "Edge chaos {} → {} for {}s",
            edge_target.source_service, edge_target.destination_service, experiment.spec.duration
        )),
        started_at: Some(now_rfc3339()),
        experiment_id: Some(experiment_id.to_string()),
        ..Default::default()
    };
    kube.patch_experiment_status(ns, name, &status).await
}

// ── Running ──

async fn handle_running(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
) -> Result<(), OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);
    let status = experiment.status.as_ref().cloned().unwrap_or_default();

    if experiment.spec.scenario.is_edge_chaos() {
        handle_running_edge(name, ns, &status, experiment.spec.duration, kube).await
    } else {
        handle_running_pod(name, ns, &status, experiment.spec.duration, kube).await
    }
}

async fn handle_running_edge(
    name: &str,
    ns: &str,
    status: &ChaosExperimentStatus,
    duration_secs: u64,
    kube: &dyn KubeClient,
) -> Result<(), OperatorError> {
    if let Some(started_at) = &status.started_at {
        if is_duration_elapsed(started_at, duration_secs) {
            let new_status = ChaosExperimentStatus {
                phase: Phase::Succeeded,
                message: Some("Edge chaos duration elapsed".to_string()),
                completed_at: Some(now_rfc3339()),
                ..status.clone()
            };
            kube.patch_experiment_status(ns, name, &new_status).await?;
        }
    }
    Ok(())
}

async fn handle_running_pod(
    name: &str,
    ns: &str,
    status: &ChaosExperimentStatus,
    duration_secs: u64,
    kube: &dyn KubeClient,
) -> Result<(), OperatorError> {
    let label_selector = format!("{}={}", EXPERIMENT_LABEL, name);
    let jobs = kube.list_jobs(ns, &label_selector).await?;

    if any_job_failed(&jobs) {
        let new_status = ChaosExperimentStatus {
            phase: Phase::Failed,
            message: Some("One or more runner jobs failed".to_string()),
            completed_at: Some(now_rfc3339()),
            ..status.clone()
        };
        kube.patch_experiment_status(ns, name, &new_status).await?;
    } else if all_jobs_succeeded(&jobs) {
        let new_status = ChaosExperimentStatus {
            phase: Phase::Succeeded,
            message: Some("All runner jobs completed".to_string()),
            completed_at: Some(now_rfc3339()),
            ..status.clone()
        };
        kube.patch_experiment_status(ns, name, &new_status).await?;
    } else if let Some(started_at) = &status.started_at {
        if is_duration_elapsed(started_at, duration_secs) {
            let new_status = ChaosExperimentStatus {
                phase: Phase::Succeeded,
                message: Some("Duration elapsed".to_string()),
                completed_at: Some(now_rfc3339()),
                ..status.clone()
            };
            kube.patch_experiment_status(ns, name, &new_status).await?;
        }
    }
    Ok(())
}

// ── Terminal ──

async fn handle_terminal(
    experiment: &ChaosExperiment,
    kube: &dyn KubeClient,
) -> Result<(), OperatorError> {
    let name = exp_name(experiment);
    let ns = exp_namespace(experiment);
    let status = experiment.status.as_ref().cloned().unwrap_or_default();

    if status.cleanup_done {
        return Ok(());
    }

    for job_name in &status.runner_jobs {
        let _ = kube.delete_job(ns, job_name).await;
    }

    if experiment.spec.scenario.is_edge_chaos() {
        if let Some(eid) = &status.experiment_id {
            let vs_name = virtual_service_name(eid);
            let _ = kube
                .delete_virtual_service(target_namespace(experiment), &vs_name)
                .await;
        }
    }

    let new_status = ChaosExperimentStatus {
        cleanup_done: true,
        ..status
    };
    kube.patch_experiment_status(ns, name, &new_status).await
}

// ── Shared helper ──

async fn patch_failed(
    kube: &dyn KubeClient,
    ns: &str,
    name: &str,
    message: &str,
) -> Result<(), OperatorError> {
    let status = ChaosExperimentStatus {
        phase: Phase::Failed,
        message: Some(message.to_string()),
        ..Default::default()
    };
    kube.patch_experiment_status(ns, name, &status).await
}
