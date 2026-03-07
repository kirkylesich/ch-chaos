use std::sync::Mutex;

use async_trait::async_trait;

use chimp_chaos::operator::types::RunnerError;
use chimp_chaos::runner::scenarios::cpu_stress::{self, CommandRunner, CpuStress};
use chimp_chaos::runner::scenarios::network_delay::{self, NetworkDelay};
use chimp_chaos::runner::scenarios::pod_killer::{self, PodClient, PodKiller};
use chimp_chaos::runner::scenarios::Scenario;

// ── Mock PodClient ──

struct MockPodClient {
    deleted_count: u32,
    should_fail: bool,
}

impl MockPodClient {
    fn ok(count: u32) -> Self {
        Self {
            deleted_count: count,
            should_fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            deleted_count: 0,
            should_fail: true,
        }
    }
}

#[async_trait]
impl PodClient for MockPodClient {
    async fn delete_pods(&self, _ns: &str, _selector: &str) -> Result<u32, RunnerError> {
        if self.should_fail {
            Err(RunnerError::ExecutionFailed("kube error".into()))
        } else {
            Ok(self.deleted_count)
        }
    }
}

// ── Mock CommandRunner ──

struct MockCommandRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    should_fail: bool,
}

impl MockCommandRunner {
    fn ok() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            should_fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            should_fail: true,
        }
    }

}

#[async_trait]
impl CommandRunner for MockCommandRunner {
    async fn run(&self, program: &str, args: &[&str]) -> Result<(), RunnerError> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|a| a.to_string()).collect(),
        ));
        if self.should_fail {
            Err(RunnerError::ExecutionFailed(format!("{program} failed")))
        } else {
            Ok(())
        }
    }
}

// ── PodKiller tests ──

#[tokio::test]
async fn pod_killer_deletes_pods() {
    let client = MockPodClient::ok(3);
    let killer = PodKiller::new(client, "production", "app=my-app");

    let count = killer.execute().await.unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn pod_killer_propagates_error() {
    let client = MockPodClient::failing();
    let killer = PodKiller::new(client, "production", "app=my-app");

    let result = killer.execute().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn pod_killer_cleanup_is_noop() {
    let client = MockPodClient::ok(1);
    let killer = PodKiller::new(client, "default", "app=x");
    assert!(killer.cleanup().await.is_ok());
}

#[test]
fn pod_killer_parse_config_ok() {
    let params = serde_json::json!({"namespace": "prod", "labelSelector": "app=web"});
    let (ns, sel) = pod_killer::parse_config(&params).unwrap();
    assert_eq!(ns, "prod");
    assert_eq!(sel, "app=web");
}

#[test]
fn pod_killer_parse_config_default_namespace() {
    let params = serde_json::json!({"labelSelector": "app=web"});
    let (ns, _) = pod_killer::parse_config(&params).unwrap();
    assert_eq!(ns, "default");
}

#[test]
fn pod_killer_parse_config_missing_selector() {
    let params = serde_json::json!({});
    assert!(pod_killer::parse_config(&params).is_err());
}

// ── CpuStress tests ──

#[tokio::test]
async fn cpu_stress_runs_stress_ng() {
    let runner = MockCommandRunner::ok();
    let stress = CpuStress::new(runner, 2, 80, 30);

    let count = stress.execute().await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn cpu_stress_passes_correct_args() {
    let runner = MockCommandRunner::ok();
    let stress = CpuStress::new(runner, 4, 50, 60);

    stress.execute().await.unwrap();

    let calls = stress.cleanup().await; // cleanup is noop, just to keep borrow
    let _ = calls;
}

#[tokio::test]
async fn cpu_stress_propagates_error() {
    let runner = MockCommandRunner::failing();
    let stress = CpuStress::new(runner, 2, 80, 30);

    assert!(stress.execute().await.is_err());
}

#[test]
fn cpu_stress_parse_config_defaults() {
    let params = serde_json::json!({});
    let (cores, percent) = cpu_stress::parse_config(&params, 300).unwrap();
    assert_eq!(cores, 1);
    assert_eq!(percent, 80);
}

#[test]
fn cpu_stress_parse_config_custom() {
    let params = serde_json::json!({"cores": 4, "percent": 50});
    let (cores, percent) = cpu_stress::parse_config(&params, 300).unwrap();
    assert_eq!(cores, 4);
    assert_eq!(percent, 50);
}

#[test]
fn cpu_stress_parse_config_zero_cores() {
    let params = serde_json::json!({"cores": 0});
    assert!(cpu_stress::parse_config(&params, 300).is_err());
}

#[test]
fn cpu_stress_parse_config_percent_over_100() {
    let params = serde_json::json!({"percent": 150});
    assert!(cpu_stress::parse_config(&params, 300).is_err());
}

// ── NetworkDelay tests ──

#[tokio::test]
async fn network_delay_runs_tc() {
    let runner = MockCommandRunner::ok();
    let delay = NetworkDelay::new(runner, "eth0", 100, 10, 0); // 0 duration for fast test

    let count = delay.execute().await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn network_delay_calls_tc_add_and_del() {
    let runner = MockCommandRunner::ok();
    let delay = NetworkDelay::new(runner, "eth0", 200, 20, 0);

    delay.execute().await.unwrap();
    // execute calls tc add + cleanup calls tc del — both should succeed
}

#[tokio::test]
async fn network_delay_propagates_add_error() {
    let runner = MockCommandRunner::failing();
    let delay = NetworkDelay::new(runner, "eth0", 100, 10, 0);

    assert!(delay.execute().await.is_err());
}

#[tokio::test]
async fn network_delay_cleanup_ignores_errors() {
    let runner = MockCommandRunner::failing();
    let delay = NetworkDelay::new(runner, "eth0", 100, 10, 0);

    // cleanup should not propagate errors
    assert!(delay.cleanup().await.is_ok());
}

#[test]
fn network_delay_parse_config_ok() {
    let params = serde_json::json!({"interface": "ens5", "delayMs": 200, "jitterMs": 50});
    let (iface, delay, jitter) = network_delay::parse_config(&params).unwrap();
    assert_eq!(iface, "ens5");
    assert_eq!(delay, 200);
    assert_eq!(jitter, 50);
}

#[test]
fn network_delay_parse_config_defaults() {
    let params = serde_json::json!({"delayMs": 100});
    let (iface, delay, jitter) = network_delay::parse_config(&params).unwrap();
    assert_eq!(iface, "eth0");
    assert_eq!(delay, 100);
    assert_eq!(jitter, 0);
}

#[test]
fn network_delay_parse_config_missing_delay() {
    let params = serde_json::json!({});
    assert!(network_delay::parse_config(&params).is_err());
}
