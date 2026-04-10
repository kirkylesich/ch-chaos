use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use chrono::DateTime;

use super::analysis_reconciler::parse_duration_str;
use super::crd::{
    calculate_impact, AffectedService, ChaosExperimentSpec, ChaosExperimentStatus, ChaosImpactMap,
    ChaosImpactMapStatus, ImpactMapSummary, MetricImpact, ServiceMetrics,
};
use super::types::{AnalysisPhase, DegradationDirection, ImpactType, OperatorError};

// ── Domain types ──

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    pub workload: String,
    pub namespace: String,
}

pub struct ServiceGraph {
    pub adjacency: HashMap<ServiceKey, Vec<ServiceKey>>,
    pub service_to_workload: HashMap<(String, String), ServiceKey>,
}

struct TimeWindows {
    baseline_time: String,
    baseline_rate_window: String,
    chaos_time: String,
    chaos_rate_window: String,
}

struct MetricsSnapshot {
    latency: MetricPair,
    error_rate: MetricPair,
    throughput: MetricPair,
}

struct MetricPair {
    baseline: HashMap<ServiceKey, f64>,
    during: HashMap<ServiceKey, f64>,
}

enum ExperimentReadiness {
    Ready { started_at: String, completed_at: String },
    Waiting(String),
    NotFound(String),
}

// ── Traits (IO boundaries) ──

#[async_trait]
pub trait ImpactMapKubeClient: Send + Sync {
    async fn get_experiment_status(&self, ns: &str, name: &str)
        -> Result<ChaosExperimentStatus, OperatorError>;
    async fn get_experiment_spec(&self, ns: &str, name: &str)
        -> Result<ChaosExperimentSpec, OperatorError>;
    async fn patch_impact_map_status(&self, ns: &str, name: &str, status: &ChaosImpactMapStatus)
        -> Result<(), OperatorError>;
}

#[async_trait]
pub trait ImpactMapPrometheusClient: Send + Sync {
    async fn query_vector_at(&self, promql: &str, time: &str)
        -> Result<HashMap<ServiceKey, f64>, OperatorError>;
}

#[async_trait]
pub trait ImpactMapGraphClient: Send + Sync {
    async fn query_service_graph(&self, namespace_filter: &[String])
        -> Result<ServiceGraph, OperatorError>;
}

// ══════════════════════════════════════════
// Reconciler — IO orchestration only
// ══════════════════════════════════════════

pub async fn reconcile_impact_map(
    impact_map: &ChaosImpactMap,
    kube: &dyn ImpactMapKubeClient,
    prom: &dyn ImpactMapPrometheusClient,
    graph: Option<&dyn ImpactMapGraphClient>,
) -> Result<(), OperatorError> {
    let name = impact_map.metadata.name.as_deref().unwrap_or("unknown");
    let ns = impact_map.metadata.namespace.as_deref().unwrap_or("default");
    let exp_ns = impact_map.spec.experiment_ref.namespace.as_deref().unwrap_or(ns);
    let exp_name = &impact_map.spec.experiment_ref.name;

    if is_completed(impact_map) {
        return Ok(());
    }

    // IO: fetch experiment
    let exp_status = match kube.get_experiment_status(exp_ns, exp_name).await {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("cannot get experiment '{exp_name}': {e}");
            return patch_status(kube, ns, name, build_failed_status(&msg)).await;
        }
    };

    // check readiness and calculate time windows
    let time_windows = match check_experiment_readiness(&exp_status) {
        ExperimentReadiness::Waiting(msg) => {
            return patch_status(kube, ns, name, build_pending_status(&msg)).await;
        }
        ExperimentReadiness::NotFound(msg) => {
            return patch_status(kube, ns, name, build_failed_status(&msg)).await;
        }
        ExperimentReadiness::Ready { started_at, completed_at } => {
            calculate_time_windows(&started_at, &completed_at, &impact_map.spec.prometheus.baseline_window)?
        }
    };

    // query Prometheus
    let metrics = match query_all_metrics(prom, &impact_map.spec, &time_windows).await {
        Ok(m) => m,
        Err(msg) => {
            return patch_status(kube, ns, name, build_failed_status(&msg)).await;
        }
    };

    // compute impact scores
    let (total_scanned, mut affected) = compute_affected_services(&metrics, impact_map.spec.min_impact);

    // fetch graph + classify
    if let Some(graph_client) = graph {
        if let Ok(exp_spec) = kube.get_experiment_spec(exp_ns, exp_name).await {
            if let Ok(service_graph) = graph_client.query_service_graph(&[]).await {
                // Pure: classify each service as direct/indirect
                classify_services(&mut affected, &exp_spec, &service_graph, ns);
            }
        }
    }

    // patch result
    patch_status(kube, ns, name, build_completed_status(total_scanned, affected)).await
}

// ══════════════════════════════════════════
// Pure functions — no IO, no side effects
// ══════════════════════════════════════════

// ── Readiness ──

fn is_completed(impact_map: &ChaosImpactMap) -> bool {
    impact_map.status.as_ref()
        .map(|s| s.phase == AnalysisPhase::Completed)
        .unwrap_or(false)
}

fn check_experiment_readiness(status: &ChaosExperimentStatus) -> ExperimentReadiness {
    if !status.phase.is_terminal() {
        return ExperimentReadiness::Waiting(format!(
            "Waiting for experiment to complete (phase: {})", status.phase
        ));
    }
    match (&status.started_at, &status.completed_at) {
        (Some(s), Some(c)) => ExperimentReadiness::Ready {
            started_at: s.clone(),
            completed_at: c.clone(),
        },
        _ => ExperimentReadiness::NotFound("experiment has no startedAt/completedAt".into()),
    }
}

// ── Time windows ──

fn calculate_time_windows(
    started_at: &str,
    completed_at: &str,
    baseline_window: &str,
) -> Result<TimeWindows, OperatorError> {
    let baseline_secs = parse_duration_str(baseline_window).ok_or_else(|| {
        OperatorError::Analysis(format!("invalid baselineWindow: {baseline_window}"))
    })?;
    let started = DateTime::parse_from_rfc3339(started_at)
        .map_err(|e| OperatorError::Analysis(format!("invalid startedAt: {e}")))?;
    let completed = DateTime::parse_from_rfc3339(completed_at)
        .map_err(|e| OperatorError::Analysis(format!("invalid completedAt: {e}")))?;
    let duration_secs = (completed - started).num_seconds().max(1) as u64;

    Ok(TimeWindows {
        baseline_time: started.to_rfc3339(),
        baseline_rate_window: format!("{baseline_secs}s"),
        chaos_time: completed.to_rfc3339(),
        chaos_rate_window: format!("{duration_secs}s"),
    })
}

// ── Impact computation ──

fn compute_affected_services(metrics: &MetricsSnapshot, min_impact: u32) -> (u32, Vec<AffectedService>) {
    let all_services = discover_services(metrics);
    let total_scanned = all_services.len() as u32;

    let mut affected: Vec<AffectedService> = all_services.iter()
        .filter_map(|svc| {
            let lat = score_metric(&metrics.latency, svc, DegradationDirection::Up);
            let err = score_error_rate(&metrics.error_rate, svc);
            let thr = score_metric(&metrics.throughput, svc, DegradationDirection::Down);
            let impact = (lat.impact_score + err.impact_score + thr.impact_score) / 3;

            (impact >= min_impact).then(|| AffectedService {
                workload: svc.workload.clone(),
                namespace: svc.namespace.clone(),
                impact_score: impact,
                impact_type: None,
                metrics: ServiceMetrics { latency_p99: lat, error_rate: err, throughput: thr },
            })
        })
        .collect();

    affected.sort_by(|a, b| b.impact_score.cmp(&a.impact_score));
    (total_scanned, affected)
}

fn discover_services(metrics: &MetricsSnapshot) -> Vec<ServiceKey> {
    let mut set = HashSet::new();
    for map in [
        &metrics.latency.baseline, &metrics.latency.during,
        &metrics.error_rate.baseline, &metrics.error_rate.during,
        &metrics.throughput.baseline, &metrics.throughput.during,
    ] {
        set.extend(map.keys().cloned());
    }
    let mut result: Vec<_> = set.into_iter().collect();
    result.sort_by(|a, b| a.workload.cmp(&b.workload).then(a.namespace.cmp(&b.namespace)));
    result
}

fn score_metric(pair: &MetricPair, svc: &ServiceKey, direction: DegradationDirection) -> MetricImpact {
    let baseline = pair.baseline.get(svc).copied().unwrap_or(0.0);
    let during = pair.during.get(svc).copied().unwrap_or(0.0);
    let (score, _, _) = calculate_impact(baseline, during, direction, 100);
    MetricImpact { baseline, during, impact_score: score }
}

fn score_error_rate(pair: &MetricPair, svc: &ServiceKey) -> MetricImpact {
    let baseline = pair.baseline.get(svc).copied().unwrap_or(0.0);
    let during = pair.during.get(svc).copied().unwrap_or(0.0);
    let impact_score = if baseline == 0.0 {
        ((during * 100.0).round() as u32).min(100)
    } else {
        let (score, _, _) = calculate_impact(baseline, during, DegradationDirection::Up, 100);
        score
    };
    MetricImpact { baseline, during, impact_score }
}

// ── Graph classification ──

fn classify_services(
    affected: &mut [AffectedService],
    exp_spec: &ChaosExperimentSpec,
    graph: &ServiceGraph,
    default_ns: &str,
) {
    if !exp_spec.scenario.is_edge_chaos() {
        return;
    }

    let edge = match exp_spec.target.as_ref().and_then(|t| t.edge.as_ref()) {
        Some(e) => e,
        None => return,
    };

    let target_ns = exp_spec.target_namespace.as_deref()
        .or_else(|| exp_spec.target.as_ref().and_then(|t| t.namespace.as_deref()))
        .unwrap_or(default_ns);

    let chaos_target = resolve_workload_key(
        &edge.destination_service, target_ns, &graph.service_to_workload,
    );

    let reachable = reachable_from(&graph.adjacency, &chaos_target);

    for svc in affected.iter_mut() {
        let key = ServiceKey { workload: svc.workload.clone(), namespace: svc.namespace.clone() };
        svc.impact_type = Some(if reachable.contains(&key) {
            ImpactType::Direct
        } else {
            ImpactType::Indirect
        });
    }
}

fn resolve_workload_key(
    service_name: &str,
    namespace: &str,
    mapping: &HashMap<(String, String), ServiceKey>,
) -> ServiceKey {
    mapping
        .get(&(service_name.to_string(), namespace.to_string()))
        .cloned()
        .unwrap_or(ServiceKey { workload: service_name.to_string(), namespace: namespace.to_string() })
}

fn reachable_from(graph: &HashMap<ServiceKey, Vec<ServiceKey>>, start: &ServiceKey) -> HashSet<ServiceKey> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(start.clone());
    queue.push_back(start.clone());
    while let Some(node) = queue.pop_front() {
        if let Some(neighbors) = graph.get(&node) {
            for neighbor in neighbors {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
    }
    visited
}

// ── Status builders (pure) ──

fn build_completed_status(total_scanned: u32, affected: Vec<AffectedService>) -> ChaosImpactMapStatus {
    let total_affected = affected.len() as u32;
    let highest = affected.first()
        .map(|s| format!("Highest impact: {} ({}/100)", s.workload, s.impact_score));
    let message = format!(
        "{total_affected} of {total_scanned} services affected.{}",
        highest.map(|h| format!(" {h}")).unwrap_or_default()
    );
    ChaosImpactMapStatus {
        phase: AnalysisPhase::Completed,
        summary: Some(ImpactMapSummary { total_scanned, total_affected, message: Some(message) }),
        affected_services: affected,
        message: None,
    }
}

fn build_pending_status(message: &str) -> ChaosImpactMapStatus {
    ChaosImpactMapStatus {
        phase: AnalysisPhase::Pending,
        message: Some(message.to_string()),
        ..Default::default()
    }
}

fn build_failed_status(message: &str) -> ChaosImpactMapStatus {
    ChaosImpactMapStatus {
        phase: AnalysisPhase::Failed,
        message: Some(message.to_string()),
        ..Default::default()
    }
}

// ══════════════════════════════════════════
// IO functions — Prometheus queries, K8s patches
// ══════════════════════════════════════════

async fn patch_status(
    kube: &dyn ImpactMapKubeClient,
    ns: &str,
    name: &str,
    status: ChaosImpactMapStatus,
) -> Result<(), OperatorError> {
    kube.patch_impact_map_status(ns, name, &status).await
}

async fn query_all_metrics(
    prom: &dyn ImpactMapPrometheusClient,
    spec: &super::crd::ChaosImpactMapSpec,
    tw: &TimeWindows,
) -> Result<MetricsSnapshot, String> {
    let ns_filter = spec.scope.as_ref()
        .map(|s| build_namespace_filter(&s.namespaces))
        .unwrap_or_default();

    let latency = query_metric_pair(prom, &ns_filter, tw, build_latency_query)
        .await.map_err(|e| format!("latency query failed: {e}"))?;
    let error_rate = query_metric_pair(prom, &ns_filter, tw, build_error_rate_query)
        .await.map_err(|e| format!("error rate query failed: {e}"))?;
    let throughput = query_metric_pair(prom, &ns_filter, tw, build_throughput_query)
        .await.map_err(|e| format!("throughput query failed: {e}"))?;

    Ok(MetricsSnapshot { latency, error_rate, throughput })
}

async fn query_metric_pair(
    prom: &dyn ImpactMapPrometheusClient,
    ns_filter: &str,
    tw: &TimeWindows,
    build_query: fn(&str, &str) -> String,
) -> Result<MetricPair, OperatorError> {
    let baseline = prom
        .query_vector_at(&build_query(ns_filter, &tw.baseline_rate_window), &tw.baseline_time)
        .await?;
    let during = prom
        .query_vector_at(&build_query(ns_filter, &tw.chaos_rate_window), &tw.chaos_time)
        .await?;
    Ok(MetricPair { baseline, during })
}

// ── PromQL builders (pure) ──

pub fn build_namespace_filter(namespaces: &[String]) -> String {
    if namespaces.is_empty() {
        String::new()
    } else {
        let joined = namespaces.join("|");
        format!(r#"destination_workload_namespace=~"{joined}""#)
    }
}

fn label_filter(ns_filter: &str) -> String {
    if ns_filter.is_empty() { String::new() } else { format!("{{{ns_filter}}}") }
}

fn label_filter_with_extra(ns_filter: &str, extra: &str) -> String {
    if ns_filter.is_empty() { format!("{{{extra}}}") } else { format!("{{{extra},{ns_filter}}}") }
}

pub fn build_latency_query(ns_filter: &str, rate_window: &str) -> String {
    let filter = label_filter(ns_filter);
    format!(r#"histogram_quantile(0.99, sum by (destination_workload, destination_workload_namespace, le) (rate(istio_request_duration_milliseconds_bucket{filter}[{rate_window}])))"#)
}

pub fn build_error_rate_query(ns_filter: &str, rate_window: &str) -> String {
    let error_filter = label_filter_with_extra(ns_filter, r#"response_code=~"5..""#);
    let total_filter = label_filter(ns_filter);
    format!(r#"sum by (destination_workload, destination_workload_namespace) (rate(istio_requests_total{error_filter}[{rate_window}])) / sum by (destination_workload, destination_workload_namespace) (rate(istio_requests_total{total_filter}[{rate_window}]))"#)
}

pub fn build_throughput_query(ns_filter: &str, rate_window: &str) -> String {
    let filter = label_filter(ns_filter);
    format!(r#"sum by (destination_workload, destination_workload_namespace) (rate(istio_requests_total{filter}[{rate_window}]))"#)
}
