use async_trait::async_trait;
use tokio::process::Command;

use super::Scenario;
use crate::operator::types::RunnerError;

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[&str]) -> Result<(), RunnerError>;
}

pub struct SystemCommandRunner;

#[async_trait]
impl CommandRunner for SystemCommandRunner {
    async fn run(&self, program: &str, args: &[&str]) -> Result<(), RunnerError> {
        let status = Command::new(program).args(args).status().await?;
        if status.success() {
            Ok(())
        } else {
            Err(RunnerError::ExecutionFailed(format!(
                "{} exited with {}",
                program,
                status.code().unwrap_or(-1)
            )))
        }
    }
}

pub struct CpuStress<R: CommandRunner> {
    runner: R,
    cores: u32,
    percent: u32,
    duration_secs: u64,
}

impl<R: CommandRunner> CpuStress<R> {
    pub fn new(runner: R, cores: u32, percent: u32, duration_secs: u64) -> Self {
        Self {
            runner,
            cores,
            percent,
            duration_secs,
        }
    }
}

#[async_trait]
impl<R: CommandRunner> Scenario for CpuStress<R> {
    async fn execute(&self) -> Result<u32, RunnerError> {
        let cores_str = self.cores.to_string();
        let percent_str = self.percent.to_string();
        let timeout_str = format!("{}s", self.duration_secs);

        self.runner
            .run(
                "stress-ng",
                &[
                    "--cpu",
                    &cores_str,
                    "--cpu-load",
                    &percent_str,
                    "--timeout",
                    &timeout_str,
                ],
            )
            .await?;

        Ok(self.cores)
    }

    async fn cleanup(&self) -> Result<(), RunnerError> {
        Ok(())
    }
}

pub fn parse_config(
    params: &serde_json::Value,
    duration_secs: u64,
) -> Result<(u32, u32), RunnerError> {
    let cores = params
        .get("cores")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as u32;
    let percent = params
        .get("percent")
        .and_then(|v| v.as_u64())
        .unwrap_or(80) as u32;

    if cores == 0 {
        return Err(RunnerError::InvalidConfig("cores must be > 0".into()));
    }
    if percent > 100 {
        return Err(RunnerError::InvalidConfig(
            "percent must be 0-100".into(),
        ));
    }
    let _ = duration_secs;

    Ok((cores, percent))
}
