use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{
    AnalysisPhase, AnalysisVerdict, DegradationDirection, Phase, ScenarioType, Target,
};

// ── ChaosExperiment CRD ──

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "chaos.io",
    version = "v1",
    kind = "ChaosExperiment",
    namespaced,
    status = "ChaosExperimentStatus",
    shortname = "ce",
    printcolumn = r#"{"name":"Scenario","type":"string","jsonPath":".spec.scenario"}"#,
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Duration","type":"integer","jsonPath":".spec.duration"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct ChaosExperimentSpec {
    pub scenario: ScenarioType,
    pub duration: u64,
    #[serde(rename = "targetNamespace", skip_serializing_if = "Option::is_none")]
    pub target_namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "parameters_schema")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
pub struct ChaosExperimentStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(rename = "startedAt", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(rename = "completedAt", skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(rename = "experimentId", skip_serializing_if = "Option::is_none")]
    pub experiment_id: Option<String>,
    #[serde(rename = "runnerJobs", default, skip_serializing_if = "Vec::is_empty")]
    pub runner_jobs: Vec<String>,
    #[serde(rename = "cleanupDone", default)]
    pub cleanup_done: bool,
}

// ── ChaosAnalysis CRD ──

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "chaos.io",
    version = "v1",
    kind = "ChaosAnalysis",
    namespaced,
    status = "ChaosAnalysisStatus",
    shortname = "ca",
    printcolumn = r#"{"name":"Verdict","type":"string","jsonPath":".status.verdict"}"#,
    printcolumn = r#"{"name":"Impact","type":"integer","jsonPath":".status.impactScore"}"#,
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#
)]
pub struct ChaosAnalysisSpec {
    #[serde(rename = "experimentRef")]
    pub experiment_ref: ExperimentRef,
    pub prometheus: PrometheusConfig,
    pub query: String,
    #[serde(rename = "degradationDirection")]
    pub degradation_direction: DegradationDirection,
    #[serde(rename = "successCriteria")]
    pub success_criteria: SuccessCriteria,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct ExperimentRef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct PrometheusConfig {
    pub url: String,
    #[serde(rename = "baselineWindow")]
    pub baseline_window: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct SuccessCriteria {
    #[serde(rename = "maxImpact")]
    pub max_impact: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
pub struct ChaosAnalysisStatus {
    #[serde(default)]
    pub phase: AnalysisPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<AnalysisVerdict>,
    #[serde(rename = "impactScore", skip_serializing_if = "Option::is_none")]
    pub impact_score: Option<u32>,
    #[serde(rename = "baselineValue", skip_serializing_if = "Option::is_none")]
    pub baseline_value: Option<f64>,
    #[serde(rename = "duringValue", skip_serializing_if = "Option::is_none")]
    pub during_value: Option<f64>,
    #[serde(rename = "degradationPercent", skip_serializing_if = "Option::is_none")]
    pub degradation_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn parameters_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "x-kubernetes-preserve-unknown-fields": true
    })
}

// ── ChaosImpactMap CRD ──

#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "chaos.io",
    version = "v1",
    kind = "ChaosImpactMap",
    namespaced,
    status = "ChaosImpactMapStatus",
    shortname = "cim",
    printcolumn = r#"{"name":"Affected","type":"integer","jsonPath":".status.summary.totalAffected"}"#,
    printcolumn = r#"{"name":"Scanned","type":"integer","jsonPath":".status.summary.totalScanned"}"#,
    printcolumn = r#"{"name":"Phase","type":"string","jsonPath":".status.phase"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct ChaosImpactMapSpec {
    #[serde(rename = "experimentRef")]
    pub experiment_ref: ExperimentRef,
    pub prometheus: PrometheusConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ImpactMapScope>,
    #[serde(rename = "minImpact", default = "default_min_impact")]
    pub min_impact: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct ImpactMapScope {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
pub struct ChaosImpactMapStatus {
    #[serde(default)]
    pub phase: AnalysisPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ImpactMapSummary>,
    #[serde(
        rename = "affectedServices",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub affected_services: Vec<AffectedService>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct ImpactMapSummary {
    #[serde(rename = "totalScanned")]
    pub total_scanned: u32,
    #[serde(rename = "totalAffected")]
    pub total_affected: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct AffectedService {
    pub workload: String,
    pub namespace: String,
    #[serde(rename = "maxImpact")]
    pub max_impact: u32,
    pub metrics: ServiceMetrics,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct ServiceMetrics {
    #[serde(rename = "latencyP99")]
    pub latency_p99: MetricImpact,
    #[serde(rename = "errorRate")]
    pub error_rate: MetricImpact,
    pub throughput: MetricImpact,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
pub struct MetricImpact {
    pub baseline: f64,
    pub during: f64,
    #[serde(rename = "impactScore")]
    pub impact_score: u32,
}

fn default_min_impact() -> u32 {
    5
}

// ── Scoring logic ──

pub fn calculate_impact(
    baseline: f64,
    during: f64,
    direction: DegradationDirection,
    max_impact: u32,
) -> (u32, AnalysisVerdict, f64) {
    let degradation_percent = if baseline == 0.0 {
        if during == 0.0 {
            0.0
        } else {
            100.0
        }
    } else {
        let raw = match direction {
            DegradationDirection::Up => (during - baseline) / baseline * 100.0,
            DegradationDirection::Down => (baseline - during) / baseline * 100.0,
        };
        raw.max(0.0)
    };

    let impact_score = (degradation_percent.round() as u32).min(100);
    let verdict = if impact_score <= max_impact {
        AnalysisVerdict::Pass
    } else {
        AnalysisVerdict::Fail
    };

    (impact_score, verdict, degradation_percent)
}
