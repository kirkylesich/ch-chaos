pub mod cpu_stress;
pub mod network_delay;
pub mod pod_killer;

use async_trait::async_trait;

use crate::operator::types::RunnerError;

#[async_trait]
pub trait Scenario: Send + Sync {
    async fn execute(&self) -> Result<u32, RunnerError>;
    async fn cleanup(&self) -> Result<(), RunnerError>;
}
