use std::collections::BTreeMap;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, PodSpec, PodTemplateSpec, SecurityContext,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};

use super::crd::ChaosExperiment;
use super::types::{
    ScenarioType, DEFAULT_METRICS_PORT, DEFAULT_RUNNER_IMAGE, EXPERIMENT_LABEL, JOB_BACKOFF_LIMIT,
    JOB_TTL_AFTER_FINISHED, SCENARIO_LABEL,
};

pub struct JobBuilderConfig {
    pub runner_image: String,
    pub metrics_port: i32,
}

impl Default for JobBuilderConfig {
    fn default() -> Self {
        Self {
            runner_image: DEFAULT_RUNNER_IMAGE.to_string(),
            metrics_port: DEFAULT_METRICS_PORT,
        }
    }
}

pub fn build_runner_job(
    experiment: &ChaosExperiment,
    experiment_id: &str,
    node_name: &str,
    scenario: ScenarioType,
    duration: u64,
    parameters_json: Option<&str>,
    config: &JobBuilderConfig,
) -> Job {
    let exp_name = experiment.metadata.name.as_deref().unwrap_or("unknown");
    let namespace = experiment
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default");
    let job_name = make_job_name(experiment_id, node_name);

    Job {
        metadata: ObjectMeta {
            name: Some(job_name),
            namespace: Some(namespace.to_string()),
            labels: Some(job_labels(exp_name, scenario)),
            owner_references: Some(vec![owner_ref(experiment)]),
            ..Default::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(JOB_BACKOFF_LIMIT),
            ttl_seconds_after_finished: Some(JOB_TTL_AFTER_FINISHED),
            template: pod_template(
                node_name,
                scenario,
                experiment_id,
                duration,
                parameters_json,
                config,
            ),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn make_job_name(experiment_id: &str, node_name: &str) -> String {
    let id_prefix = &experiment_id[..8.min(experiment_id.len())];
    // "chaos-runner-" = 13 chars, "-" = 1 char, id_prefix = 8 chars → 22 chars for node
    let max_node_len = 63 - 13 - 1 - id_prefix.len();
    let node_short = &node_name[..max_node_len.min(node_name.len())];
    format!("chaos-runner-{}-{}", id_prefix, node_short)
}

fn job_labels(exp_name: &str, scenario: ScenarioType) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app".to_string(), "chimp-chaos-runner".to_string()),
        (EXPERIMENT_LABEL.to_string(), exp_name.to_string()),
        (SCENARIO_LABEL.to_string(), scenario.to_string()),
    ])
}

fn owner_ref(experiment: &ChaosExperiment) -> OwnerReference {
    let exp_name = experiment.metadata.name.as_deref().unwrap_or("unknown");
    let uid = experiment.metadata.uid.clone().unwrap_or_default();

    OwnerReference {
        api_version: "chaos.io/v1".to_string(),
        kind: "ChaosExperiment".to_string(),
        name: exp_name.to_string(),
        uid,
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}

fn env_var(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.to_string(),
        value: Some(value.to_string()),
        ..Default::default()
    }
}

fn runner_env_vars(
    experiment_id: &str,
    scenario: ScenarioType,
    duration: u64,
    parameters_json: Option<&str>,
) -> Vec<EnvVar> {
    let mut vars = vec![
        env_var("EXPERIMENT_ID", experiment_id),
        env_var("SCENARIO", &scenario.to_string()),
        env_var("DURATION", &duration.to_string()),
    ];
    if let Some(params) = parameters_json {
        vars.push(env_var("PARAMETERS", params));
    }
    vars
}

fn runner_container(
    scenario: ScenarioType,
    experiment_id: &str,
    duration: u64,
    parameters_json: Option<&str>,
    config: &JobBuilderConfig,
) -> Container {
    let security_context = scenario.requires_privileged().then(|| SecurityContext {
        privileged: Some(true),
        ..Default::default()
    });

    Container {
        name: "chaos-runner".to_string(),
        image: Some(config.runner_image.clone()),
        args: Some(vec!["--mode".to_string(), "runner".to_string()]),
        env: Some(runner_env_vars(
            experiment_id,
            scenario,
            duration,
            parameters_json,
        )),
        ports: Some(vec![ContainerPort {
            container_port: config.metrics_port,
            name: Some("metrics".to_string()),
            ..Default::default()
        }]),
        security_context,
        ..Default::default()
    }
}

fn prometheus_annotations(metrics_port: i32) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("prometheus.io/scrape".to_string(), "true".to_string()),
        ("prometheus.io/port".to_string(), metrics_port.to_string()),
    ])
}

fn pod_template(
    node_name: &str,
    scenario: ScenarioType,
    experiment_id: &str,
    duration: u64,
    parameters_json: Option<&str>,
    config: &JobBuilderConfig,
) -> PodTemplateSpec {
    PodTemplateSpec {
        metadata: Some(ObjectMeta {
            labels: Some(BTreeMap::from([(
                "app".to_string(),
                "chimp-chaos-runner".to_string(),
            )])),
            annotations: Some(prometheus_annotations(config.metrics_port)),
            ..Default::default()
        }),
        spec: Some(PodSpec {
            containers: vec![runner_container(
                scenario,
                experiment_id,
                duration,
                parameters_json,
                config,
            )],
            node_name: Some(node_name.to_string()),
            restart_policy: Some("Never".to_string()),
            service_account_name: Some("chimp-chaos-runner".to_string()),
            ..Default::default()
        }),
    }
}
