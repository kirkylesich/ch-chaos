use std::collections::BTreeMap;
use std::num::NonZeroU64;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ── Newtype wrappers ──

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExperimentId(pub Uuid);

impl JsonSchema for ExperimentId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("ExperimentId")
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as JsonSchema>::json_schema(generator)
    }
}

impl Default for ExperimentId {
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}

impl ExperimentId {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl std::fmt::Display for ExperimentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExperimentDuration(
    #[schemars(schema_with = "non_zero_u64_schema")]
    pub NonZeroU64,
);

fn non_zero_u64_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    <u64 as JsonSchema>::json_schema(generator)
}

impl ExperimentDuration {
    pub fn new(seconds: u64) -> Result<Self, ValidationError> {
        NonZeroU64::new(seconds)
            .map(Self)
            .ok_or(ValidationError::InvalidDuration)
    }

    pub fn as_secs(&self) -> u64 {
        self.0.get()
    }
}

// ── Enums ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ScenarioType {
    // Pod / Node chaos (RunnerJobInjector)
    PodKiller,
    CpuStress,
    NetworkDelay,
    // Edge chaos (IstioEdgeInjector)
    EdgeDelay,
    EdgeAbort,
}

impl ScenarioType {
    pub fn is_edge_chaos(&self) -> bool {
        matches!(self, ScenarioType::EdgeDelay | ScenarioType::EdgeAbort)
    }

    pub fn is_pod_node_chaos(&self) -> bool {
        !self.is_edge_chaos()
    }

    pub fn requires_privileged(&self) -> bool {
        matches!(self, ScenarioType::CpuStress | ScenarioType::NetworkDelay)
    }
}

impl std::fmt::Display for ScenarioType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum Phase {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
}

impl Phase {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Phase::Succeeded | Phase::Failed)
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DegradationDirection {
    #[serde(rename = "up")]
    Up,
    #[serde(rename = "down")]
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AnalysisVerdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum AnalysisPhase {
    #[default]
    Pending,
    Completed,
    Failed,
}

// ── Edge target ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EdgeTarget {
    #[serde(rename = "sourceService")]
    pub source_service: String,
    #[serde(rename = "destinationService")]
    pub destination_service: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Target {
    pub namespace: Option<String>,
    pub edge: Option<EdgeTarget>,
}

// ── Edge info (from GraphBuilder) ──

#[derive(Debug, Clone)]
pub struct EdgeInfo {
    pub source_workload: String,
    pub source_namespace: String,
    pub destination_workload: String,
    pub destination_namespace: String,
    pub destination_service: String,
    pub rps: f64,
    pub source_labels: BTreeMap<String, String>,
}

// ── Errors ──

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("duration must be greater than 0")]
    InvalidDuration,

    #[error("unknown scenario")]
    UnknownScenario,

    #[error("no target nodes found")]
    NoTargetNodes,

    #[error("edge target requires sourceService and destinationService")]
    MissingEdgeTarget,

    #[error("cannot resolve workload labels for source service: {0}")]
    SourceServiceNotFound(String),

    #[error("conflicting VirtualService exists for host: {0}")]
    ConflictingVirtualService(String),

    #[error("target edge not found: {0} → {1}")]
    EdgeNotFound(String, String),

    #[error("edge traffic below threshold")]
    EdgeTrafficBelowThreshold,
}

#[derive(Debug, Error)]
pub enum OperatorError {
    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("prometheus query error: {0}")]
    Prometheus(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("analysis error: {0}")]
    Analysis(String),
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("scenario execution failed: {0}")]
    ExecutionFailed(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("command execution error: {0}")]
    Command(#[from] std::io::Error),
}

// ── Constants ──

pub const FINALIZER_NAME: &str = "chaos.io/cleanup";
pub const MANAGED_BY_LABEL: &str = "chaos.io/managed-by";
pub const MANAGED_BY_VALUE: &str = "chimp-chaos";
pub const EXPERIMENT_LABEL: &str = "chaos.io/experiment";
pub const SCENARIO_LABEL: &str = "chaos.io/scenario";

pub const REQUEUE_RUNNING_SECS: u64 = 5;
pub const REQUEUE_TERMINAL_SECS: u64 = 300;

pub const JOB_BACKOFF_LIMIT: i32 = 0;
pub const JOB_TTL_AFTER_FINISHED: i32 = 300;

pub const DEFAULT_RUNNER_IMAGE: &str = "chimp-chaos:latest";
pub const DEFAULT_METRICS_PORT: i32 = 9090;
pub const DEFAULT_PROMETHEUS_URL: &str = "http://prometheus:9090";
pub const DEFAULT_GRAPH_LOOKBACK: &str = "10m";
pub const DEFAULT_GRAPH_MIN_RPS: f64 = 0.05;
