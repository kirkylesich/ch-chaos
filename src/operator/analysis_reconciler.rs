use async_trait::async_trait;
use chrono::DateTime;

use super::crd::{calculate_impact, ChaosAnalysis, ChaosAnalysisStatus, ChaosExperimentStatus};
use super::types::{AnalysisPhase, OperatorError};

// ── Traits for dependency injection ──

#[async_trait]
pub trait AnalysisKubeClient: Send + Sync {
    async fn get_experiment_status(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<ChaosExperimentStatus, OperatorError>;

    async fn patch_analysis_status(
        &self,
        ns: &str,
        name: &str,
        status: &ChaosAnalysisStatus,
    ) -> Result<(), OperatorError>;
}

#[async_trait]
pub trait AnalysisPrometheusClient: Send + Sync {
    /// Execute a PromQL query at a specific RFC3339 timestamp, return scalar value.
    async fn query_at(&self, promql: &str, time: &str) -> Result<f64, OperatorError>;
}

// ── Reconcile ──

pub async fn reconcile_analysis(
    analysis: &ChaosAnalysis,
    kube: &dyn AnalysisKubeClient,
    prom: &dyn AnalysisPrometheusClient,
) -> Result<(), OperatorError> {
    let name = analysis.metadata.name.as_deref().unwrap_or("unknown");
    let ns = analysis.metadata.namespace.as_deref().unwrap_or("default");

    let status = analysis.status.as_ref().cloned().unwrap_or_default();
    if status.phase == AnalysisPhase::Completed {
        return Ok(());
    }

    // 1. Get experiment status
    let exp_ns = analysis
        .spec
        .experiment_ref
        .namespace
        .as_deref()
        .unwrap_or(ns);
    let exp_name = &analysis.spec.experiment_ref.name;

    let exp_status = match kube.get_experiment_status(exp_ns, exp_name).await {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("cannot get experiment '{}': {}", exp_name, e);
            return patch_failed(kube, ns, name, &msg).await;
        }
    };

    if !exp_status.phase.is_terminal() {
        let status = ChaosAnalysisStatus {
            phase: AnalysisPhase::Pending,
            message: Some(format!(
                "Waiting for experiment '{}' to complete (phase: {})",
                exp_name, exp_status.phase
            )),
            ..Default::default()
        };
        return kube.patch_analysis_status(ns, name, &status).await;
    }

    let started_at = exp_status.started_at.as_deref().ok_or_else(|| {
        OperatorError::Analysis("experiment has no startedAt".into())
    })?;
    let completed_at = exp_status.completed_at.as_deref().ok_or_else(|| {
        OperatorError::Analysis("experiment has no completedAt".into())
    })?;

    // 2. Parse baseline window
    let baseline_secs = parse_duration_str(&analysis.spec.prometheus.baseline_window)
        .ok_or_else(|| {
            OperatorError::Analysis(format!(
                "invalid baselineWindow: {}",
                analysis.spec.prometheus.baseline_window
            ))
        })?;

    // 3. Calculate query timestamps
    let started = DateTime::parse_from_rfc3339(started_at)
        .map_err(|e| OperatorError::Analysis(format!("invalid startedAt: {e}")))?;
    let completed = DateTime::parse_from_rfc3339(completed_at)
        .map_err(|e| OperatorError::Analysis(format!("invalid completedAt: {e}")))?;

    // Baseline: midpoint of [startedAt - baselineWindow, startedAt]
    let baseline_time = started - chrono::Duration::seconds(baseline_secs as i64 / 2);
    // Chaos: midpoint of [startedAt, completedAt]
    let chaos_time = started + (completed - started) / 2;

    // 4. Query Prometheus
    let baseline_value = match prom
        .query_at(&analysis.spec.query, &baseline_time.to_rfc3339())
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("baseline query failed: {e}");
            return patch_failed(kube, ns, name, &msg).await;
        }
    };

    let during_value = match prom
        .query_at(&analysis.spec.query, &chaos_time.to_rfc3339())
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("chaos window query failed: {e}");
            return patch_failed(kube, ns, name, &msg).await;
        }
    };

    // 5. Calculate impact
    let (impact_score, verdict, degradation_percent) = calculate_impact(
        baseline_value,
        during_value,
        analysis.spec.degradation_direction,
        analysis.spec.success_criteria.max_impact,
    );

    // 6. Update status
    let result_status = ChaosAnalysisStatus {
        phase: AnalysisPhase::Completed,
        verdict: Some(verdict),
        impact_score: Some(impact_score),
        baseline_value: Some(baseline_value),
        during_value: Some(during_value),
        degradation_percent: Some(degradation_percent),
        message: Some(format!(
            "Impact {}/100. baseline={:.4} -> during={:.4} ({:.1}%), threshold={}",
            impact_score,
            baseline_value,
            during_value,
            degradation_percent,
            analysis.spec.success_criteria.max_impact
        )),
    };

    kube.patch_analysis_status(ns, name, &result_status).await
}

async fn patch_failed(
    kube: &dyn AnalysisKubeClient,
    ns: &str,
    name: &str,
    message: &str,
) -> Result<(), OperatorError> {
    let status = ChaosAnalysisStatus {
        phase: AnalysisPhase::Failed,
        message: Some(message.to_string()),
        ..Default::default()
    };
    kube.patch_analysis_status(ns, name, &status).await
}

// ── Duration parsing ──

pub fn parse_duration_str(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('m') {
        num.parse::<u64>().ok().map(|v| v * 60)
    } else if let Some(num) = s.strip_suffix('h') {
        num.parse::<u64>().ok().map(|v| v * 3600)
    } else if let Some(num) = s.strip_suffix('s') {
        num.parse::<u64>().ok()
    } else {
        s.parse::<u64>().ok()
    }
}
