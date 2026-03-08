use chimp_chaos::operator::crd::*;
use chimp_chaos::operator::types::*;
use kube::CustomResourceExt;

#[test]
fn crd_generates_valid_experiment_schema() {
    let crd = ChaosExperiment::crd();
    assert_eq!(
        crd.metadata.name.as_deref(),
        Some("chaosexperiments.chaos.io")
    );
}

#[test]
fn crd_parameters_field_has_type() {
    // K8s rejects OpenAPI schemas where object properties have no 'type'.
    // serde_json::Value without a custom schema generates a typeless field.
    let crd = ChaosExperiment::crd();
    let yaml = serde_yaml::to_string(&crd).unwrap();
    let doc: serde_json::Value = serde_yaml::from_str(&yaml).unwrap();

    let parameters_schema = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]
        ["properties"]["spec"]["properties"]["parameters"];

    assert_eq!(
        parameters_schema.get("type").and_then(|t| t.as_str()),
        Some("object"),
        "parameters field must have type 'object' for K8s CRD validation"
    );
}

#[test]
fn crd_all_spec_properties_have_type() {
    // Ensure every property in the CRD spec has a 'type' field,
    // which is required by Kubernetes structural schema validation.
    let crd = ChaosExperiment::crd();
    let yaml = serde_yaml::to_string(&crd).unwrap();
    let doc: serde_json::Value = serde_yaml::from_str(&yaml).unwrap();

    let props = doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]
        ["properties"]["spec"]["properties"]
        .as_object()
        .expect("spec properties should be an object");

    for (name, schema) in props {
        assert!(
            schema.get("type").is_some() || schema.get("$ref").is_some() || schema.get("allOf").is_some(),
            "spec property '{}' has no 'type' — K8s will reject this CRD",
            name,
        );
    }
}

#[test]
fn crd_generates_valid_analysis_schema() {
    let crd = ChaosAnalysis::crd();
    assert_eq!(
        crd.metadata.name.as_deref(),
        Some("chaosanalysises.chaos.io")
    );
}

#[test]
fn experiment_spec_serde_pod_killer() {
    let json = r#"{
        "scenario": "PodKiller",
        "duration": 300,
        "targetNamespace": "production",
        "parameters": {"gracePeriod": 30}
    }"#;
    let spec: ChaosExperimentSpec = serde_json::from_str(json).unwrap();
    assert_eq!(spec.scenario, ScenarioType::PodKiller);
    assert_eq!(spec.duration, 300);
    assert_eq!(spec.target_namespace.as_deref(), Some("production"));
    assert!(spec.parameters.is_some());
}

#[test]
fn experiment_spec_serde_edge_delay() {
    let json = r#"{
        "scenario": "EdgeDelay",
        "duration": 600,
        "target": {
            "namespace": "production",
            "edge": {
                "sourceService": "payment",
                "destinationService": "ledger"
            }
        },
        "parameters": {"latencyMs": 200}
    }"#;
    let spec: ChaosExperimentSpec = serde_json::from_str(json).unwrap();
    assert_eq!(spec.scenario, ScenarioType::EdgeDelay);
    let target = spec.target.as_ref().unwrap();
    let edge = target.edge.as_ref().unwrap();
    assert_eq!(edge.source_service, "payment");
    assert_eq!(edge.destination_service, "ledger");
}

#[test]
fn experiment_status_defaults() {
    let status = ChaosExperimentStatus::default();
    assert_eq!(status.phase, Phase::Pending);
    assert!(status.message.is_none());
    assert!(status.started_at.is_none());
    assert!(status.completed_at.is_none());
    assert!(status.experiment_id.is_none());
    assert!(status.runner_jobs.is_empty());
    assert!(!status.cleanup_done);
}

#[test]
fn analysis_spec_serde() {
    let json = r#"{
        "experimentRef": {"name": "pod-killer-test", "namespace": "default"},
        "prometheus": {"url": "http://prometheus:9090", "baselineWindow": "30m"},
        "query": "histogram_quantile(0.99, rate(http_request_duration_seconds_bucket[5m]))",
        "degradationDirection": "up",
        "successCriteria": {"maxImpact": 30}
    }"#;
    let spec: ChaosAnalysisSpec = serde_json::from_str(json).unwrap();
    assert_eq!(spec.experiment_ref.name, "pod-killer-test");
    assert_eq!(spec.degradation_direction, DegradationDirection::Up);
    assert_eq!(spec.success_criteria.max_impact, 30);
}

// ── Impact scoring tests ──

#[test]
fn impact_no_change_up() {
    let (score, verdict, _) = calculate_impact(0.10, 0.10, DegradationDirection::Up, 30);
    assert_eq!(score, 0);
    assert_eq!(verdict, AnalysisVerdict::Pass);
}

#[test]
fn impact_50_percent_up_fails() {
    let (score, verdict, _) = calculate_impact(0.10, 0.15, DegradationDirection::Up, 30);
    assert_eq!(score, 50);
    assert_eq!(verdict, AnalysisVerdict::Fail);
}

#[test]
fn impact_30_percent_up_passes() {
    let (score, verdict, _) = calculate_impact(0.10, 0.13, DegradationDirection::Up, 30);
    assert_eq!(score, 30);
    assert_eq!(verdict, AnalysisVerdict::Pass);
}

#[test]
fn impact_20_percent_down_passes() {
    let (score, verdict, _) = calculate_impact(1000.0, 800.0, DegradationDirection::Down, 30);
    assert_eq!(score, 20);
    assert_eq!(verdict, AnalysisVerdict::Pass);
}

#[test]
fn impact_80_percent_down_fails() {
    let (score, verdict, _) = calculate_impact(1000.0, 200.0, DegradationDirection::Down, 30);
    assert_eq!(score, 80);
    assert_eq!(verdict, AnalysisVerdict::Fail);
}

#[test]
fn impact_clamped_to_100() {
    let (score, _, _) = calculate_impact(0.10, 0.50, DegradationDirection::Up, 30);
    assert_eq!(score, 100);
}

#[test]
fn impact_no_negative_degradation() {
    let (score, verdict, _) = calculate_impact(0.10, 0.05, DegradationDirection::Up, 30);
    assert_eq!(score, 0);
    assert_eq!(verdict, AnalysisVerdict::Pass);
}

#[test]
fn impact_zero_baseline() {
    let (score, _, _) = calculate_impact(0.0, 0.0, DegradationDirection::Up, 30);
    assert_eq!(score, 0);

    let (score, _, _) = calculate_impact(0.0, 1.0, DegradationDirection::Up, 30);
    assert_eq!(score, 100);
}
