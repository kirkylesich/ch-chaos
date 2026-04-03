use std::collections::BTreeMap;

use async_trait::async_trait;
use chimp_chaos::operator::graph_builder::*;
use chimp_chaos::operator::types::OperatorError;

// ── Mock Prometheus client ──

struct MockPrometheus {
    metrics: Vec<PrometheusMetric>,
}

impl MockPrometheus {
    fn with_edges(edges: Vec<(&str, &str, &str, f64)>) -> Self {
        let metrics = edges
            .into_iter()
            .map(|(src, dst, ns, rps)| PrometheusMetric {
                metric: BTreeMap::from([
                    ("source_workload".into(), src.into()),
                    ("source_workload_namespace".into(), ns.into()),
                    ("destination_workload".into(), dst.into()),
                    ("destination_workload_namespace".into(), ns.into()),
                    ("destination_service_name".into(), dst.into()),
                ]),
                value: (0.0, rps.to_string()),
            })
            .collect();
        Self { metrics }
    }

    fn empty() -> Self {
        Self { metrics: vec![] }
    }
}

#[async_trait]
impl PrometheusClient for MockPrometheus {
    async fn query(&self, _promql: &str) -> Result<PrometheusQueryResult, OperatorError> {
        Ok(PrometheusQueryResult {
            data: PrometheusData {
                result: self.metrics.clone(),
            },
        })
    }
}

struct FailingPrometheus;

#[async_trait]
impl PrometheusClient for FailingPrometheus {
    async fn query(&self, _promql: &str) -> Result<PrometheusQueryResult, OperatorError> {
        Err(OperatorError::Prometheus("connection refused".into()))
    }
}

fn default_config() -> GraphBuilderConfig {
    GraphBuilderConfig::default()
}

// ── Tests ──

#[tokio::test]
async fn finds_existing_edge() {
    let client = MockPrometheus::with_edges(vec![("payment", "ledger", "production", 12.5)]);
    let gb = GraphBuilder::new(client, default_config());

    let edge = gb
        .resolve_edge("payment", "ledger", "production")
        .await
        .unwrap();
    assert_eq!(edge.source_workload, "payment");
    assert_eq!(edge.destination_service, "ledger");
    assert!((edge.rps - 12.5).abs() < 0.01);
}

#[tokio::test]
async fn finds_edge_among_many() {
    let client = MockPrometheus::with_edges(vec![
        ("frontend", "api", "production", 450.0),
        ("api", "payment", "production", 45.0),
        ("payment", "ledger", "production", 12.5),
    ]);
    let gb = GraphBuilder::new(client, default_config());

    let edge = gb
        .resolve_edge("payment", "ledger", "production")
        .await
        .unwrap();
    assert_eq!(edge.source_workload, "payment");
    assert_eq!(edge.destination_service, "ledger");
}

#[tokio::test]
async fn edge_not_found() {
    let client = MockPrometheus::with_edges(vec![("frontend", "api", "production", 450.0)]);
    let gb = GraphBuilder::new(client, default_config());

    let result = gb.resolve_edge("payment", "ledger", "production").await;
    assert!(result.is_err());

    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("target edge not found"));
}

#[tokio::test]
async fn edge_not_found_empty_graph() {
    let client = MockPrometheus::empty();
    let gb = GraphBuilder::new(client, default_config());

    let result = gb.resolve_edge("payment", "ledger", "production").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn edge_traffic_below_threshold() {
    let client = MockPrometheus::with_edges(vec![
        ("payment", "ledger", "production", 0.01), // below 0.05
    ]);
    let gb = GraphBuilder::new(client, default_config());

    let result = gb.resolve_edge("payment", "ledger", "production").await;
    assert!(result.is_err());

    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("below threshold"));
}

#[tokio::test]
async fn edge_at_exact_threshold_passes() {
    let client = MockPrometheus::with_edges(vec![("payment", "ledger", "production", 0.05)]);
    let gb = GraphBuilder::new(client, default_config());

    let edge = gb
        .resolve_edge("payment", "ledger", "production")
        .await
        .unwrap();
    assert!((edge.rps - 0.05).abs() < 0.001);
}

#[tokio::test]
async fn wrong_namespace_not_matched() {
    let client = MockPrometheus::with_edges(vec![("payment", "ledger", "staging", 12.5)]);
    let gb = GraphBuilder::new(client, default_config());

    let result = gb.resolve_edge("payment", "ledger", "production").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn prometheus_error_propagated() {
    let gb = GraphBuilder::new(FailingPrometheus, default_config());

    let result = gb.resolve_edge("payment", "ledger", "production").await;
    assert!(result.is_err());

    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("connection refused"));
}

#[tokio::test]
async fn custom_config_min_rps() {
    let client = MockPrometheus::with_edges(vec![("payment", "ledger", "production", 0.5)]);
    let config = GraphBuilderConfig {
        lookback: "5m".to_string(),
        min_rps: 1.0,
    };
    let gb = GraphBuilder::new(client, config);

    // 0.5 rps < 1.0 min_rps → should fail
    let result = gb.resolve_edge("payment", "ledger", "production").await;
    assert!(result.is_err());
}
