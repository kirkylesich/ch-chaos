use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;

use super::analysis_reconciler::AnalysisPrometheusClient;
use super::types::{EdgeInfo, OperatorError, DEFAULT_GRAPH_LOOKBACK, DEFAULT_GRAPH_MIN_RPS};

// ── Prometheus client trait (for mocking) ──

#[async_trait]
pub trait PrometheusClient: Send + Sync {
    async fn query(&self, promql: &str) -> Result<PrometheusQueryResult, OperatorError>;
}

// ── Prometheus response types ──

#[derive(Debug, Clone, Deserialize)]
pub struct PrometheusQueryResult {
    pub data: PrometheusData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrometheusData {
    pub result: Vec<PrometheusMetric>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrometheusMetric {
    pub metric: BTreeMap<String, String>,
    pub value: (f64, String),
}

impl PrometheusMetric {
    fn rps(&self) -> f64 {
        self.value.1.parse::<f64>().unwrap_or(0.0)
    }

    fn label(&self, key: &str) -> &str {
        self.metric.get(key).map(|s| s.as_str()).unwrap_or("")
    }
}

// ── Real HTTP Prometheus client ──

pub struct HttpPrometheusClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpPrometheusClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[derive(Deserialize)]
struct PrometheusApiResponse {
    status: String,
    data: PrometheusData,
}

#[async_trait]
impl PrometheusClient for HttpPrometheusClient {
    async fn query(&self, promql: &str) -> Result<PrometheusQueryResult, OperatorError> {
        let url = format!("{}/api/v1/query", self.base_url);
        let resp: PrometheusApiResponse = self
            .client
            .get(&url)
            .query(&[("query", promql)])
            .send()
            .await
            .map_err(|e| OperatorError::Prometheus(e.to_string()))?
            .json()
            .await
            .map_err(|e| OperatorError::Prometheus(e.to_string()))?;

        if resp.status != "success" {
            return Err(OperatorError::Prometheus(format!(
                "prometheus returned status: {}",
                resp.status
            )));
        }

        Ok(PrometheusQueryResult { data: resp.data })
    }
}

#[async_trait]
impl AnalysisPrometheusClient for HttpPrometheusClient {
    async fn query_at(&self, promql: &str, time: &str) -> Result<f64, OperatorError> {
        let url = format!("{}/api/v1/query", self.base_url);
        let resp: PrometheusApiResponse = self
            .client
            .get(&url)
            .query(&[("query", promql), ("time", time)])
            .send()
            .await
            .map_err(|e| OperatorError::Prometheus(e.to_string()))?
            .json()
            .await
            .map_err(|e| OperatorError::Prometheus(e.to_string()))?;

        if resp.status != "success" {
            return Err(OperatorError::Prometheus(format!(
                "prometheus returned status: {}",
                resp.status
            )));
        }

        resp.data
            .result
            .first()
            .map(|m| m.value.1.parse::<f64>().unwrap_or(0.0))
            .ok_or_else(|| OperatorError::Prometheus("empty query result".into()))
    }
}

// ── GraphBuilder config ──

pub struct GraphBuilderConfig {
    pub lookback: String,
    pub min_rps: f64,
}

impl Default for GraphBuilderConfig {
    fn default() -> Self {
        Self {
            lookback: DEFAULT_GRAPH_LOOKBACK.to_string(),
            min_rps: DEFAULT_GRAPH_MIN_RPS,
        }
    }
}

// ── GraphBuilder ──

pub struct GraphBuilder<C: PrometheusClient> {
    client: C,
    config: GraphBuilderConfig,
}

impl<C: PrometheusClient> GraphBuilder<C> {
    pub fn new(client: C, config: GraphBuilderConfig) -> Self {
        Self { client, config }
    }

    pub async fn resolve_edge(
        &self,
        source: &str,
        destination: &str,
        namespace: &str,
    ) -> Result<EdgeInfo, OperatorError> {
        let metrics = self.query_edges().await?;
        self.find_edge(&metrics, source, destination, namespace)
    }

    async fn query_edges(&self) -> Result<Vec<PrometheusMetric>, OperatorError> {
        let promql = build_edges_query(&self.config.lookback);
        let result = self.client.query(&promql).await?;
        Ok(result.data.result)
    }

    fn find_edge(
        &self,
        metrics: &[PrometheusMetric],
        source: &str,
        destination: &str,
        namespace: &str,
    ) -> Result<EdgeInfo, OperatorError> {
        let matching = metrics.iter().find(|m| {
            m.label("source_workload") == source
                && m.label("destination_service_name") == destination
                && m.label("source_workload_namespace") == namespace
        });

        match matching {
            Some(m) if m.rps() >= self.config.min_rps => Ok(edge_info_from_metric(m)),
            Some(_) => Err(OperatorError::Validation(
                super::types::ValidationError::EdgeTrafficBelowThreshold,
            )),
            None => Err(OperatorError::Validation(
                super::types::ValidationError::EdgeNotFound(
                    source.to_string(),
                    destination.to_string(),
                ),
            )),
        }
    }
}

fn build_edges_query(lookback: &str) -> String {
    format!(
        r#"sum by (source_workload, source_workload_namespace, destination_workload, destination_workload_namespace, destination_service_name) (rate(istio_requests_total[{lookback}]))"#
    )
}

fn edge_info_from_metric(m: &PrometheusMetric) -> EdgeInfo {
    EdgeInfo {
        source_workload: m.label("source_workload").to_string(),
        source_namespace: m.label("source_workload_namespace").to_string(),
        destination_workload: m.label("destination_workload").to_string(),
        destination_namespace: m.label("destination_workload_namespace").to_string(),
        destination_service: m.label("destination_service_name").to_string(),
        rps: m.rps(),
        source_labels: BTreeMap::new(), // filled later by reconciler from K8s Service
    }
}
