use std::collections::BTreeMap;

use chimp_chaos::operator::istio_injector::*;
use chimp_chaos::operator::types::ScenarioType;

// ── FQDN ──

#[test]
fn fqdn_format() {
    assert_eq!(
        fqdn("ledger", "production"),
        "ledger.production.svc.cluster.local"
    );
}

#[test]
fn fqdn_default_namespace() {
    assert_eq!(
        fqdn("api", "default"),
        "api.default.svc.cluster.local"
    );
}

// ── VirtualService name ──

#[test]
fn vs_name_format() {
    assert_eq!(
        virtual_service_name("550e8400-e29b-41d4-a716-446655440000"),
        "chaos-edge-550e8400"
    );
}

#[test]
fn vs_name_short_id() {
    assert_eq!(virtual_service_name("abc"), "chaos-edge-abc");
}

// ── Fault builders ──

#[test]
fn delay_fault_200ms() {
    let fault = build_edge_delay_fault(200);
    assert_eq!(fault["delay"]["fixedDelay"], "200ms");
    assert_eq!(fault["delay"]["percentage"]["value"], 100);
}

#[test]
fn abort_fault_50_percent_503() {
    let fault = build_edge_abort_fault(50, 503);
    assert_eq!(fault["abort"]["percentage"]["value"], 50);
    assert_eq!(fault["abort"]["httpStatus"], 503);
}

#[test]
fn build_fault_edge_delay_from_params() {
    let params = serde_json::json!({"latencyMs": 200});
    let fault = build_fault_for_scenario(ScenarioType::EdgeDelay, &params).unwrap();
    assert_eq!(fault["delay"]["fixedDelay"], "200ms");
}

#[test]
fn build_fault_edge_abort_from_params() {
    let params = serde_json::json!({"abortPercent": 50, "abortHttpStatus": 503});
    let fault = build_fault_for_scenario(ScenarioType::EdgeAbort, &params).unwrap();
    assert_eq!(fault["abort"]["percentage"]["value"], 50);
    assert_eq!(fault["abort"]["httpStatus"], 503);
}

#[test]
fn build_fault_edge_abort_default_status() {
    let params = serde_json::json!({"abortPercent": 30});
    let fault = build_fault_for_scenario(ScenarioType::EdgeAbort, &params).unwrap();
    assert_eq!(fault["abort"]["httpStatus"], 503);
}

#[test]
fn build_fault_returns_none_for_pod_chaos() {
    let params = serde_json::json!({"gracePeriod": 30});
    assert!(build_fault_for_scenario(ScenarioType::PodKiller, &params).is_none());
    assert!(build_fault_for_scenario(ScenarioType::CpuStress, &params).is_none());
    assert!(build_fault_for_scenario(ScenarioType::NetworkDelay, &params).is_none());
}

// ── VirtualService JSON ──

#[test]
fn vs_json_has_fqdn_hosts() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    assert_eq!(vs["spec"]["hosts"][0], "ledger.production.svc.cluster.local");
}

#[test]
fn vs_json_has_owner_references() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    let owner = &vs["metadata"]["ownerReferences"][0];
    assert_eq!(owner["apiVersion"], "chaos.io/v1");
    assert_eq!(owner["kind"], "ChaosExperiment");
    assert_eq!(owner["name"], "payment-ledger-delay");
    assert_eq!(owner["controller"], true);
}

#[test]
fn vs_json_has_managed_by_label() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    assert_eq!(vs["metadata"]["labels"]["chaos.io/managed-by"], "chimp-chaos");
}

#[test]
fn vs_json_has_experiment_label() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    assert_eq!(
        vs["metadata"]["labels"]["chaos.io/experiment"],
        "payment-ledger-delay"
    );
}

#[test]
fn vs_json_has_source_labels_match() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    let source_labels = &vs["spec"]["http"][0]["match"][0]["sourceLabels"];
    assert_eq!(source_labels["app"], "payment");
}

#[test]
fn vs_json_has_fault_in_first_route() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    assert!(vs["spec"]["http"][0]["fault"].is_object());
}

#[test]
fn vs_json_has_default_route_without_fault() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    let default_route = &vs["spec"]["http"][1];
    assert!(default_route.get("fault").is_none());
    assert_eq!(
        default_route["route"][0]["destination"]["host"],
        "ledger.production.svc.cluster.local"
    );
}

#[test]
fn vs_json_both_routes_same_destination() {
    let spec = test_vs_spec();
    let vs = build_virtual_service_json(&spec);
    let dst1 = &vs["spec"]["http"][0]["route"][0]["destination"]["host"];
    let dst2 = &vs["spec"]["http"][1]["route"][0]["destination"]["host"];
    assert_eq!(dst1, dst2);
    assert_eq!(dst1, "ledger.production.svc.cluster.local");
}

// ── is_chaos_managed ──

#[test]
fn chaos_managed_true() {
    let labels = BTreeMap::from([
        ("chaos.io/managed-by".into(), "chimp-chaos".into()),
    ]);
    assert!(is_chaos_managed(&labels));
}

#[test]
fn chaos_managed_false_no_label() {
    let labels = BTreeMap::new();
    assert!(!is_chaos_managed(&labels));
}

#[test]
fn chaos_managed_false_wrong_value() {
    let labels = BTreeMap::from([
        ("chaos.io/managed-by".into(), "other-tool".into()),
    ]);
    assert!(!is_chaos_managed(&labels));
}

// ── Helper ──

fn test_vs_spec() -> VirtualServiceSpec {
    VirtualServiceSpec {
        name: "chaos-edge-550e8400".to_string(),
        namespace: "production".to_string(),
        experiment_name: "payment-ledger-delay".to_string(),
        experiment_uid: "uid-1234".to_string(),
        destination_fqdn: "ledger.production.svc.cluster.local".to_string(),
        source_labels: BTreeMap::from([("app".into(), "payment".into())]),
        fault: build_edge_delay_fault(200),
    }
}
