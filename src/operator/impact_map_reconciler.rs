use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use chrono::DateTime;

use super::analysis_reconciler::parse_duration_str;
use super::crd::{
    calculate_impact, AffectedService, ChaosExperimentStatus, ChaosImpactMap, ChaosImpactMapStatus,
    ImpactMapSummary, MetricImpact, ServiceMetrics,
};
use super::types::{AnalysisPhase, DegradationDirection, OperatorError};

// ── Service key ──

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceKey {
    pub workload: String,
    pub namespace: String,
}

// ── Traits for dependency injection ──

#[async_trait]
pub trait ImpactMapKubeClient: Send + Sync {
    async fn get_experiment_status(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<ChaosExperimentStatus, OperatorError>;

    async fn patch_impact_map_status(
        &self,
        ns: &str,
        name: &str,
        status: &ChaosImpactMapStatus,
    ) -> Result<(), OperatorError>;
}

#[async_trait]
pub trait ImpactMapPrometheusClient: Send + Sync {
    async fn query_vector_at(
        &self,
        promql: &str,
        time: &str,
    ) -> Result<HashMap<ServiceKey, f64>, OperatorError>;
}

// ── PromQL query builders ──

pub fn build_namespace_filter(namespaces: &[String]) -> String {
    if namespaces.is_empty() {
        String::new()
    } else {
        let joined = namespaces.join("|");
        format!(r#"destination_workload_namespace=~"{joined}""#)
    }
}

fn label_filter(ns_filter: &str) -> String {
    if ns_filter.is_empty() {
        String::new()
    } else {
        format!("{{{ns_filter}}}")
    }
}

fn label_filter_with_extra(ns_filter: &str, extra: &str) -> String {
    if ns_filter.is_empty() {
        format!("{{{extra}}}")
    } else {
        format!("{{{extra},{ns_filter}}}")
    }
}

pub fn build_latency_query(ns_filter: &str) -> String {
    let filter = label_filter(ns_filter);
    format!(
        r#"histogram_quantile(0.99, sum by (destination_workload, destination_workload_namespace, le) (rate(istio_request_duration_milliseconds_bucket{filter}[5m])))"#
    )
}

pub fn build_error_rate_query(ns_filter: &str) -> String {
    let error_filter = label_filter_with_extra(ns_filter, r#"response_code=~"5..""#);
    let total_filter = label_filter(ns_filter);
    format!(
        r#"sum by (destination_workload, destination_workload_namespace) (rate(istio_requests_total{error_filter}[5m])) / sum by (destination_workload, destination_workload_namespace) (rate(istio_requests_total{total_filter}[5m]))"#
    )
}

pub fn build_throughput_query(ns_filter: &str) -> String {
    let filter = label_filter(ns_filter);
    format!(
        r#"sum by (destination_workload, destination_workload_namespace) (rate(istio_requests_total{filter}[5m]))"#
    )
}

// ── Reconcile ──

pub async fn reconcile_impact_map(
    impact_map: &ChaosImpactMap,
    kube: &dyn ImpactMapKubeClient,
    prom: &dyn ImpactMapPrometheusClient,
) -> Result<(), OperatorError> {
    let name = impact_map.metadata.name.as_deref().unwrap_or("unknown");
    let ns = impact_map
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default");

    let status = impact_map.status.as_ref().cloned().unwrap_or_default();
    if status.phase == AnalysisPhase::Completed {
        return Ok(());
    }

    // 1. Get experiment status
    let exp_ns = impact_map
        .spec
        .experiment_ref
        .namespace
        .as_deref()
        .unwrap_or(ns);
    let exp_name = &impact_map.spec.experiment_ref.name;

    let exp_status = match kube.get_experiment_status(exp_ns, exp_name).await {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("cannot get experiment '{exp_name}': {e}");
            return patch_failed(kube, ns, name, &msg).await;
        }
    };

    if !exp_status.phase.is_terminal() {
        let status = ChaosImpactMapStatus {
            phase: AnalysisPhase::Pending,
            message: Some(format!(
                "Waiting for experiment '{}' to complete (phase: {})",
                exp_name, exp_status.phase
            )),
            ..Default::default()
        };
        return kube.patch_impact_map_status(ns, name, &status).await;
    }

    let started_at = exp_status
        .started_at
        .as_deref()
        .ok_or_else(|| OperatorError::Analysis("experiment has no startedAt".into()))?;
    let completed_at = exp_status
        .completed_at
        .as_deref()
        .ok_or_else(|| OperatorError::Analysis("experiment has no completedAt".into()))?;

    // 2. Parse baseline window
    let baseline_secs = parse_duration_str(&impact_map.spec.prometheus.baseline_window)
        .ok_or_else(|| {
            OperatorError::Analysis(format!(
                "invalid baselineWindow: {}",
                impact_map.spec.prometheus.baseline_window
            ))
        })?;

    // 3. Calculate query timestamps
    let started = DateTime::parse_from_rfc3339(started_at)
        .map_err(|e| OperatorError::Analysis(format!("invalid startedAt: {e}")))?;
    let completed = DateTime::parse_from_rfc3339(completed_at)
        .map_err(|e| OperatorError::Analysis(format!("invalid completedAt: {e}")))?;

    let baseline_time = started - chrono::Duration::seconds(baseline_secs as i64 / 2);
    let chaos_time = started + (completed - started) / 2;

    let baseline_rfc3339 = baseline_time.to_rfc3339();
    let chaos_rfc3339 = chaos_time.to_rfc3339();

    // 4. Build queries with namespace filter
    let ns_filter = impact_map
        .spec
        .scope
        .as_ref()
        .map(|s| build_namespace_filter(&s.namespaces))
        .unwrap_or_default();

    let latency_query = build_latency_query(&ns_filter);
    let error_query = build_error_rate_query(&ns_filter);
    let throughput_query = build_throughput_query(&ns_filter);

    // 5. Execute 6 queries (3 metrics x 2 time points)
    let latency_baseline = match prom
        .query_vector_at(&latency_query, &baseline_rfc3339)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return patch_failed(
                kube,
                ns,
                name,
                &format!("latency baseline query failed: {e}"),
            )
            .await
        }
    };
    let latency_during = match prom.query_vector_at(&latency_query, &chaos_rfc3339).await {
        Ok(v) => v,
        Err(e) => {
            return patch_failed(kube, ns, name, &format!("latency chaos query failed: {e}")).await
        }
    };
    let error_baseline = match prom.query_vector_at(&error_query, &baseline_rfc3339).await {
        Ok(v) => v,
        Err(e) => {
            return patch_failed(
                kube,
                ns,
                name,
                &format!("error rate baseline query failed: {e}"),
            )
            .await
        }
    };
    let error_during = match prom.query_vector_at(&error_query, &chaos_rfc3339).await {
        Ok(v) => v,
        Err(e) => {
            return patch_failed(
                kube,
                ns,
                name,
                &format!("error rate chaos query failed: {e}"),
            )
            .await
        }
    };
    let throughput_baseline = match prom
        .query_vector_at(&throughput_query, &baseline_rfc3339)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return patch_failed(
                kube,
                ns,
                name,
                &format!("throughput baseline query failed: {e}"),
            )
            .await
        }
    };
    let throughput_during = match prom
        .query_vector_at(&throughput_query, &chaos_rfc3339)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return patch_failed(
                kube,
                ns,
                name,
                &format!("throughput chaos query failed: {e}"),
            )
            .await
        }
    };

    // 6. Discover all services (union of all keys)
    let all_services = collect_all_services(&[
        &latency_baseline,
        &latency_during,
        &error_baseline,
        &error_during,
        &throughput_baseline,
        &throughput_during,
    ]);

    let total_scanned = all_services.len() as u32;

    // 7. Compute per-service impact
    let mut affected: Vec<AffectedService> = Vec::new();

    for svc in &all_services {
        let lat_b = get_value(&latency_baseline, svc);
        let lat_d = get_value(&latency_during, svc);
        let err_b = get_value(&error_baseline, svc);
        let err_d = get_value(&error_during, svc);
        let thr_b = get_value(&throughput_baseline, svc);
        let thr_d = get_value(&throughput_during, svc);

        let (lat_score, _, _) = calculate_impact(lat_b, lat_d, DegradationDirection::Up, 100);
        let err_score = error_rate_impact(err_b, err_d);
        let (thr_score, _, _) = calculate_impact(thr_b, thr_d, DegradationDirection::Down, 100);

        let max_impact = lat_score.max(err_score).max(thr_score);

        if max_impact >= impact_map.spec.min_impact {
            affected.push(AffectedService {
                workload: svc.workload.clone(),
                namespace: svc.namespace.clone(),
                max_impact,
                metrics: ServiceMetrics {
                    latency_p99: MetricImpact {
                        baseline: lat_b,
                        during: lat_d,
                        impact_score: lat_score,
                    },
                    error_rate: MetricImpact {
                        baseline: err_b,
                        during: err_d,
                        impact_score: err_score,
                    },
                    throughput: MetricImpact {
                        baseline: thr_b,
                        during: thr_d,
                        impact_score: thr_score,
                    },
                },
            });
        }
    }

    // 8. Sort by max_impact descending
    affected.sort_by(|a, b| b.max_impact.cmp(&a.max_impact));

    let total_affected = affected.len() as u32;
    let highest = affected
        .first()
        .map(|s| format!("Highest impact: {} ({}/100)", s.workload, s.max_impact));
    let summary_msg = format!(
        "{total_affected} of {total_scanned} services affected.{}",
        highest.map(|h| format!(" {h}")).unwrap_or_default()
    );

    let result_status = ChaosImpactMapStatus {
        phase: AnalysisPhase::Completed,
        summary: Some(ImpactMapSummary {
            total_scanned,
            total_affected,
            message: Some(summary_msg),
        }),
        affected_services: affected,
        message: None,
    };

    kube.patch_impact_map_status(ns, name, &result_status).await
}

// ── Helpers ──

fn collect_all_services(maps: &[&HashMap<ServiceKey, f64>]) -> Vec<ServiceKey> {
    let mut set = HashSet::new();
    for map in maps {
        for key in map.keys() {
            set.insert(key.clone());
        }
    }
    let mut result: Vec<ServiceKey> = set.into_iter().collect();
    result.sort_by(|a, b| {
        a.workload
            .cmp(&b.workload)
            .then(a.namespace.cmp(&b.namespace))
    });
    result
}

/// Error rate impact: uses absolute difference (in percentage points) when baseline is zero.
/// This avoids the "0 → 0.15% = 100% impact" problem — 0.15% error rate is not catastrophic.
/// When baseline > 0, falls back to standard relative percentage calculation.
fn error_rate_impact(baseline: f64, during: f64) -> u32 {
    if baseline == 0.0 {
        // Absolute: error rate is already a ratio (0.0 to 1.0), convert to percentage points
        // 0.001 (0.1%) → impact 0, 0.05 (5%) → impact 5, 1.0 (100%) → impact 100
        let pct = during * 100.0;
        (pct.round() as u32).min(100)
    } else {
        let (score, _, _) = calculate_impact(baseline, during, DegradationDirection::Up, 100);
        score
    }
}

fn get_value(map: &HashMap<ServiceKey, f64>, key: &ServiceKey) -> f64 {
    map.get(key).copied().unwrap_or(0.0)
}

async fn patch_failed(
    kube: &dyn ImpactMapKubeClient,
    ns: &str,
    name: &str,
    message: &str,
) -> Result<(), OperatorError> {
    let status = ChaosImpactMapStatus {
        phase: AnalysisPhase::Failed,
        message: Some(message.to_string()),
        ..Default::default()
    };
    kube.patch_impact_map_status(ns, name, &status).await
}
