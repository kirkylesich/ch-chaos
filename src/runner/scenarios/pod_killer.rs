use async_trait::async_trait;

use super::Scenario;
use crate::operator::types::RunnerError;

#[async_trait]
pub trait PodClient: Send + Sync {
    async fn delete_pods(&self, namespace: &str, label_selector: &str) -> Result<u32, RunnerError>;
}

pub struct PodKiller<C: PodClient> {
    client: C,
    namespace: String,
    label_selector: String,
}

impl<C: PodClient> PodKiller<C> {
    pub fn new(client: C, namespace: &str, label_selector: &str) -> Self {
        Self {
            client,
            namespace: namespace.to_string(),
            label_selector: label_selector.to_string(),
        }
    }
}

#[async_trait]
impl<C: PodClient> Scenario for PodKiller<C> {
    async fn execute(&self) -> Result<u32, RunnerError> {
        self.client
            .delete_pods(&self.namespace, &self.label_selector)
            .await
    }

    async fn cleanup(&self) -> Result<(), RunnerError> {
        Ok(())
    }
}

pub fn parse_config(params: &serde_json::Value) -> Result<(String, String), RunnerError> {
    let namespace = params
        .get("namespace")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    let label_selector = params
        .get("labelSelector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RunnerError::InvalidConfig("labelSelector is required".into()))?;
    Ok((namespace.to_string(), label_selector.to_string()))
}
