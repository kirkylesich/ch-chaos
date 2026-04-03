use chimp_chaos::operator::crd::{ChaosExperiment, ChaosExperimentSpec};
use chimp_chaos::operator::job_builder::{build_runner_job, JobBuilderConfig};
use chimp_chaos::operator::types::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

fn make_experiment(name: &str, namespace: &str, scenario: ScenarioType) -> ChaosExperiment {
    ChaosExperiment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            uid: Some("test-uid-1234".to_string()),
            ..Default::default()
        },
        spec: ChaosExperimentSpec {
            scenario,
            duration: 300,
            target_namespace: None,
            target: None,
            parameters: None,
        },
        status: None,
    }
}

#[test]
fn job_name_format() {
    let exp = make_experiment("pod-killer-test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "550e8400-e29b-41d4-a716-446655440000",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    assert_eq!(
        job.metadata.name.as_deref(),
        Some("chaos-runner-550e8400-node01")
    );
}

#[test]
fn job_name_truncated_for_long_eks_node_names() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let long_node = "ip-10-0-34-50.eu-central-1.compute.internal";
    let job = build_runner_job(
        &exp,
        "550e8400-e29b-41d4-a716-446655440000",
        long_node,
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let name = job.metadata.name.as_deref().unwrap();
    assert!(
        name.len() <= 63,
        "job name '{}' is {} chars, must be <= 63",
        name,
        name.len()
    );
    assert!(name.starts_with("chaos-runner-550e8400-"));
}

#[test]
fn job_name_max_length_with_various_node_names() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let node_names = [
        "ip-10-0-34-50.eu-central-1.compute.internal",
        "ip-192-168-100-200.us-west-2.compute.internal",
        "gke-my-cluster-default-pool-abcdef01-xyz1",
        "short",
    ];
    for node in &node_names {
        let job = build_runner_job(
            &exp,
            "abcdef12-3456-7890-abcd-ef1234567890",
            node,
            ScenarioType::PodKiller,
            300,
            None,
            &JobBuilderConfig::default(),
        );
        let name = job.metadata.name.as_deref().unwrap();
        assert!(
            name.len() <= 63,
            "job name '{}' ({} chars) exceeds 63 for node '{}'",
            name,
            name.len(),
            node
        );
    }
}

#[test]
fn job_namespace_matches_experiment() {
    let exp = make_experiment("test", "production", ScenarioType::CpuStress);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::CpuStress,
        60,
        None,
        &JobBuilderConfig::default(),
    );

    assert_eq!(job.metadata.namespace.as_deref(), Some("production"));
}

#[test]
fn job_backoff_limit_is_zero() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let spec = job.spec.as_ref().unwrap();
    assert_eq!(spec.backoff_limit, Some(JOB_BACKOFF_LIMIT));
}

#[test]
fn job_ttl_after_finished() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let spec = job.spec.as_ref().unwrap();
    assert_eq!(
        spec.ttl_seconds_after_finished,
        Some(JOB_TTL_AFTER_FINISHED)
    );
}

#[test]
fn job_owner_references() {
    let exp = make_experiment("pod-killer-test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let owner_refs = job.metadata.owner_references.as_ref().unwrap();
    assert_eq!(owner_refs.len(), 1);
    assert_eq!(owner_refs[0].api_version, "chaos.io/v1");
    assert_eq!(owner_refs[0].kind, "ChaosExperiment");
    assert_eq!(owner_refs[0].name, "pod-killer-test");
    assert_eq!(owner_refs[0].uid, "test-uid-1234");
    assert_eq!(owner_refs[0].controller, Some(true));
    assert_eq!(owner_refs[0].block_owner_deletion, Some(true));
}

#[test]
fn job_labels() {
    let exp = make_experiment("pod-killer-test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let labels = job.metadata.labels.as_ref().unwrap();
    assert_eq!(labels.get("app").unwrap(), "chimp-chaos-runner");
    assert_eq!(labels.get(EXPERIMENT_LABEL).unwrap(), "pod-killer-test");
    assert_eq!(labels.get(SCENARIO_LABEL).unwrap(), "PodKiller");
}

#[test]
fn job_env_vars() {
    let exp = make_experiment("test", "default", ScenarioType::CpuStress);
    let params = r#"{"cores":2,"percent":80}"#;
    let job = build_runner_job(
        &exp,
        "exp-id-123",
        "node01",
        ScenarioType::CpuStress,
        300,
        Some(params),
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    let envs = container.env.as_ref().unwrap();

    let find_env = |name: &str| -> Option<String> {
        envs.iter()
            .find(|e| e.name == name)
            .and_then(|e| e.value.clone())
    };

    assert_eq!(find_env("EXPERIMENT_ID"), Some("exp-id-123".to_string()));
    assert_eq!(find_env("SCENARIO"), Some("CpuStress".to_string()));
    assert_eq!(find_env("DURATION"), Some("300".to_string()));
    assert_eq!(find_env("PARAMETERS"), Some(params.to_string()));
}

#[test]
fn job_env_vars_no_parameters() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "exp-id-123",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    let envs = container.env.as_ref().unwrap();
    assert!(envs.iter().all(|e| e.name != "PARAMETERS"));
}

#[test]
fn job_prometheus_annotations() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let annotations = job
        .spec
        .as_ref()
        .unwrap()
        .template
        .metadata
        .as_ref()
        .unwrap()
        .annotations
        .as_ref()
        .unwrap();
    assert_eq!(annotations.get("prometheus.io/scrape").unwrap(), "true");
    assert_eq!(annotations.get("prometheus.io/port").unwrap(), "9090");
}

#[test]
fn job_node_name() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "target-node-01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
    assert_eq!(pod_spec.node_name.as_deref(), Some("target-node-01"));
}

#[test]
fn job_restart_policy_never() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
    assert_eq!(pod_spec.restart_policy.as_deref(), Some("Never"));
}

#[test]
fn job_service_account() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let pod_spec = job.spec.as_ref().unwrap().template.spec.as_ref().unwrap();
    assert_eq!(
        pod_spec.service_account_name.as_deref(),
        Some("chimp-chaos-runner")
    );
}

#[test]
fn job_privileged_for_cpu_stress() {
    let exp = make_experiment("test", "default", ScenarioType::CpuStress);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::CpuStress,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    let sec_ctx = container.security_context.as_ref().unwrap();
    assert_eq!(sec_ctx.privileged, Some(true));
}

#[test]
fn job_privileged_for_network_delay() {
    let exp = make_experiment("test", "default", ScenarioType::NetworkDelay);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::NetworkDelay,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    let sec_ctx = container.security_context.as_ref().unwrap();
    assert_eq!(sec_ctx.privileged, Some(true));
}

#[test]
fn job_not_privileged_for_pod_killer() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    assert!(container.security_context.is_none());
}

#[test]
fn job_container_args() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    assert_eq!(
        container.args.as_ref().unwrap(),
        &vec!["--mode".to_string(), "runner".to_string()]
    );
}

#[test]
fn job_custom_config() {
    let config = JobBuilderConfig {
        runner_image: "my-registry/chimp-chaos:v2".to_string(),
        metrics_port: 8080,
    };
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &config,
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    assert_eq!(
        container.image.as_deref(),
        Some("my-registry/chimp-chaos:v2")
    );

    let port = &container.ports.as_ref().unwrap()[0];
    assert_eq!(port.container_port, 8080);

    let annotations = job
        .spec
        .as_ref()
        .unwrap()
        .template
        .metadata
        .as_ref()
        .unwrap()
        .annotations
        .as_ref()
        .unwrap();
    assert_eq!(annotations.get("prometheus.io/port").unwrap(), "8080");
}

#[test]
fn job_metrics_port() {
    let exp = make_experiment("test", "default", ScenarioType::PodKiller);
    let job = build_runner_job(
        &exp,
        "abcd1234",
        "node01",
        ScenarioType::PodKiller,
        300,
        None,
        &JobBuilderConfig::default(),
    );

    let container = &job
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap()
        .containers[0];
    let port = &container.ports.as_ref().unwrap()[0];
    assert_eq!(port.container_port, 9090);
    assert_eq!(port.name.as_deref(), Some("metrics"));
}
