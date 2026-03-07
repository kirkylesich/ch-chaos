use chimp_chaos::operator::types::*;
use uuid::Uuid;

#[test]
fn experiment_id_generates_unique() {
    let id1 = ExperimentId::new();
    let id2 = ExperimentId::new();
    assert_ne!(id1, id2);
}

#[test]
fn experiment_id_display() {
    let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
    let id = ExperimentId(uuid);
    assert_eq!(id.to_string(), "550e8400-e29b-41d4-a716-446655440000");
}

#[test]
fn experiment_duration_rejects_zero() {
    let result = ExperimentDuration::new(0);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ValidationError::InvalidDuration
    ));
}

#[test]
fn experiment_duration_accepts_positive() {
    let dur = ExperimentDuration::new(300).unwrap();
    assert_eq!(dur.as_secs(), 300);
}

#[test]
fn scenario_type_classification() {
    assert!(ScenarioType::PodKiller.is_pod_node_chaos());
    assert!(ScenarioType::CpuStress.is_pod_node_chaos());
    assert!(ScenarioType::NetworkDelay.is_pod_node_chaos());
    assert!(ScenarioType::EdgeDelay.is_edge_chaos());
    assert!(ScenarioType::EdgeAbort.is_edge_chaos());

    assert!(!ScenarioType::PodKiller.is_edge_chaos());
    assert!(!ScenarioType::EdgeDelay.is_pod_node_chaos());
}

#[test]
fn scenario_type_privileged() {
    assert!(!ScenarioType::PodKiller.requires_privileged());
    assert!(ScenarioType::CpuStress.requires_privileged());
    assert!(ScenarioType::NetworkDelay.requires_privileged());
    assert!(!ScenarioType::EdgeDelay.requires_privileged());
    assert!(!ScenarioType::EdgeAbort.requires_privileged());
}

#[test]
fn phase_terminal() {
    assert!(!Phase::Pending.is_terminal());
    assert!(!Phase::Running.is_terminal());
    assert!(Phase::Succeeded.is_terminal());
    assert!(Phase::Failed.is_terminal());
}

#[test]
fn phase_default_is_pending() {
    assert_eq!(Phase::default(), Phase::Pending);
}

#[test]
fn scenario_type_display() {
    assert_eq!(ScenarioType::PodKiller.to_string(), "PodKiller");
    assert_eq!(ScenarioType::CpuStress.to_string(), "CpuStress");
    assert_eq!(ScenarioType::NetworkDelay.to_string(), "NetworkDelay");
    assert_eq!(ScenarioType::EdgeDelay.to_string(), "EdgeDelay");
    assert_eq!(ScenarioType::EdgeAbort.to_string(), "EdgeAbort");
}

#[test]
fn validation_error_messages() {
    assert_eq!(
        ValidationError::InvalidDuration.to_string(),
        "duration must be greater than 0"
    );
    assert_eq!(
        ValidationError::NoTargetNodes.to_string(),
        "no target nodes found"
    );
    assert_eq!(
        ValidationError::MissingEdgeTarget.to_string(),
        "edge target requires sourceService and destinationService"
    );
    assert_eq!(
        ValidationError::SourceServiceNotFound("payment".to_string()).to_string(),
        "cannot resolve workload labels for source service: payment"
    );
    assert_eq!(
        ValidationError::ConflictingVirtualService("ledger.prod.svc.cluster.local".to_string())
            .to_string(),
        "conflicting VirtualService exists for host: ledger.prod.svc.cluster.local"
    );
    assert_eq!(
        ValidationError::EdgeNotFound("payment".to_string(), "ledger".to_string()).to_string(),
        "target edge not found: payment → ledger"
    );
}

#[test]
fn degradation_direction_serde() {
    let up: DegradationDirection = serde_json::from_str(r#""up""#).unwrap();
    assert_eq!(up, DegradationDirection::Up);

    let down: DegradationDirection = serde_json::from_str(r#""down""#).unwrap();
    assert_eq!(down, DegradationDirection::Down);
}

#[test]
fn edge_target_serde() {
    let json = r#"{"sourceService":"payment","destinationService":"ledger"}"#;
    let target: EdgeTarget = serde_json::from_str(json).unwrap();
    assert_eq!(target.source_service, "payment");
    assert_eq!(target.destination_service, "ledger");
}
