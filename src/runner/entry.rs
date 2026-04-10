use std::time::Instant;

use prometheus::Registry;

use crate::operator::types::{RunnerError, ScenarioType};
use crate::runner::metrics::RunnerMetrics;
use crate::runner::scenarios::cpu_stress::{self, CpuStress, SystemCommandRunner};
use crate::runner::scenarios::network_delay::{self, NetworkDelay};
use crate::runner::scenarios::pod_killer::{self, PodKiller};
use crate::runner::scenarios::Scenario;

pub struct RunnerConfig {
    pub experiment_id: String,
    pub scenario: ScenarioType,
    pub duration: u64,
    pub parameters: serde_json::Value,
    pub metrics_port: u16,
}

impl RunnerConfig {
    pub fn from_env() -> Result<Self, RunnerError> {
        let experiment_id = std::env::var("EXPERIMENT_ID")
            .map_err(|_| RunnerError::InvalidConfig("EXPERIMENT_ID not set".into()))?;
        let scenario_str = std::env::var("SCENARIO")
            .map_err(|_| RunnerError::InvalidConfig("SCENARIO not set".into()))?;
        let scenario: ScenarioType = serde_json::from_value(serde_json::json!(scenario_str))
            .map_err(|_| RunnerError::InvalidConfig(format!("unknown scenario: {scenario_str}")))?;
        let duration: u64 = std::env::var("DURATION")
            .map_err(|_| RunnerError::InvalidConfig("DURATION not set".into()))?
            .parse()
            .map_err(|_| RunnerError::InvalidConfig("DURATION must be a number".into()))?;
        let parameters: serde_json::Value = std::env::var("PARAMETERS")
            .ok()
            .and_then(|p| serde_json::from_str(&p).ok())
            .unwrap_or(serde_json::json!({}));
        let metrics_port: u16 = std::env::var("METRICS_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9090);

        Ok(Self {
            experiment_id,
            scenario,
            duration,
            parameters,
            metrics_port,
        })
    }
}

pub async fn run(config: RunnerConfig) -> anyhow::Result<()> {
    let registry = Registry::new();
    let metrics = RunnerMetrics::new(&registry)
        .map_err(|e| anyhow::anyhow!("failed to create metrics: {e}"))?;

    let scenario_name = config.scenario.to_string();

    // Start metrics server as async task
    let server_registry = registry.clone();
    let port = config.metrics_port;
    tokio::spawn(async move {
        if let Err(e) = crate::runner::server::start_server(port, server_registry).await {
            tracing::error!(%e, "metrics server error");
        }
    });

    metrics.set_active(&config.experiment_id, &scenario_name);

    let start = Instant::now();
    let result = execute_scenario(&config).await;
    let elapsed = start.elapsed().as_secs_f64();

    metrics.observe_duration(elapsed);

    match &result {
        Ok(targets) => {
            metrics.record_success(&scenario_name);
            metrics.set_targets(*targets);
            tracing::info!(targets, elapsed, "scenario completed successfully");
        }
        Err(e) => {
            metrics.record_failure(&scenario_name);
            tracing::error!(%e, elapsed, "scenario failed");
        }
    }

    metrics.set_inactive(&config.experiment_id, &scenario_name);

    // server task stops when main exits

    result.map(|_| ()).map_err(Into::into)
}

async fn execute_scenario(config: &RunnerConfig) -> Result<u32, RunnerError> {
    match config.scenario {
        ScenarioType::PodKiller => {
            let (ns, selector) = pod_killer::parse_config(&config.parameters)?;
            let client = RealPodClient::new().await?;
            let killer = PodKiller::new(client, &ns, &selector);
            killer.execute().await
        }
        ScenarioType::CpuStress => {
            let (cores, percent) = cpu_stress::parse_config(&config.parameters, config.duration)?;
            let stress = CpuStress::new(SystemCommandRunner, cores, percent, config.duration);
            stress.execute().await
        }
        ScenarioType::NetworkDelay => {
            let (iface, delay_ms, jitter_ms) = network_delay::parse_config(&config.parameters)?;
            let delay = NetworkDelay::new(
                SystemCommandRunner,
                &iface,
                delay_ms,
                jitter_ms,
                config.duration,
            );
            delay.execute().await
        }
        ScenarioType::EdgeDelay | ScenarioType::EdgeAbort => Err(RunnerError::InvalidConfig(
            "edge scenarios do not use runner pods".into(),
        )),
    }
}

// ── Real PodClient using kube ──

struct RealPodClient {
    client: kube::Client,
}

impl RealPodClient {
    async fn new() -> Result<Self, RunnerError> {
        let client = kube::Client::try_default()
            .await
            .map_err(|e| RunnerError::ExecutionFailed(format!("kube client: {e}")))?;
        Ok(Self { client })
    }
}

#[async_trait::async_trait]
impl pod_killer::PodClient for RealPodClient {
    async fn delete_pods(&self, namespace: &str, label_selector: &str) -> Result<u32, RunnerError> {
        use kube::api::{Api, DeleteParams, ListParams};

        let api: Api<k8s_openapi::api::core::v1::Pod> =
            Api::namespaced(self.client.clone(), namespace);
        let lp = ListParams::default().labels(label_selector);
        let pods = api.list(&lp).await?;

        let mut deleted = 0u32;
        for pod in &pods.items {
            if let Some(name) = &pod.metadata.name {
                api.delete(name, &DeleteParams::default()).await?;
                deleted += 1;
                tracing::info!(pod = %name, "deleted pod");
            }
        }

        Ok(deleted)
    }
}
