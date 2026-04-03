use async_trait::async_trait;

use super::cpu_stress::CommandRunner;
use super::Scenario;
use crate::operator::types::RunnerError;

pub struct NetworkDelay<R: CommandRunner> {
    runner: R,
    interface: String,
    delay_ms: u32,
    jitter_ms: u32,
    duration_secs: u64,
}

impl<R: CommandRunner> NetworkDelay<R> {
    pub fn new(
        runner: R,
        interface: &str,
        delay_ms: u32,
        jitter_ms: u32,
        duration_secs: u64,
    ) -> Self {
        Self {
            runner,
            interface: interface.to_string(),
            delay_ms,
            jitter_ms,
            duration_secs,
        }
    }
}

#[async_trait]
impl<R: CommandRunner> Scenario for NetworkDelay<R> {
    async fn execute(&self) -> Result<u32, RunnerError> {
        let delay = format!("{}ms", self.delay_ms);
        let jitter = format!("{}ms", self.jitter_ms);

        self.runner
            .run(
                "tc",
                &[
                    "qdisc",
                    "add",
                    "dev",
                    &self.interface,
                    "root",
                    "netem",
                    "delay",
                    &delay,
                    &jitter,
                ],
            )
            .await?;

        tokio::time::sleep(tokio::time::Duration::from_secs(self.duration_secs)).await;

        self.cleanup().await?;
        Ok(1)
    }

    async fn cleanup(&self) -> Result<(), RunnerError> {
        let _ = self
            .runner
            .run("tc", &["qdisc", "del", "dev", &self.interface, "root"])
            .await;
        Ok(())
    }
}

pub fn parse_config(params: &serde_json::Value) -> Result<(String, u32, u32), RunnerError> {
    let interface = params
        .get("interface")
        .and_then(|v| v.as_str())
        .unwrap_or("eth0")
        .to_string();
    let delay_ms = params
        .get("delayMs")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RunnerError::InvalidConfig("delayMs is required".into()))?
        as u32;
    let jitter_ms = params.get("jitterMs").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    Ok((interface, delay_ms, jitter_ms))
}
