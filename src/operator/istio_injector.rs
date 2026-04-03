use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::types::{ScenarioType, EXPERIMENT_LABEL, MANAGED_BY_LABEL, MANAGED_BY_VALUE};

// ── Helpers ──

pub fn fqdn(service: &str, namespace: &str) -> String {
    format!("{service}.{namespace}.svc.cluster.local")
}

pub fn virtual_service_name(experiment_id: &str) -> String {
    let prefix_len = 8.min(experiment_id.len());
    format!("chaos-edge-{}", &experiment_id[..prefix_len])
}

// ── VirtualService spec builder ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServiceSpec {
    pub name: String,
    pub namespace: String,
    pub experiment_name: String,
    pub experiment_uid: String,
    pub destination_fqdn: String,
    pub source_labels: BTreeMap<String, String>,
    pub fault: serde_json::Value,
}

pub fn build_edge_delay_fault(latency_ms: u64) -> serde_json::Value {
    json!({
        "delay": {
            "percentage": { "value": 100 },
            "fixedDelay": format!("{latency_ms}ms")
        }
    })
}

pub fn build_edge_abort_fault(percent: u32, http_status: u16) -> serde_json::Value {
    json!({
        "abort": {
            "percentage": { "value": percent },
            "httpStatus": http_status
        }
    })
}

pub fn build_fault_for_scenario(
    scenario: ScenarioType,
    parameters: &serde_json::Value,
) -> Option<serde_json::Value> {
    match scenario {
        ScenarioType::EdgeDelay => {
            let ms = parameters.get("latencyMs")?.as_u64()?;
            Some(build_edge_delay_fault(ms))
        }
        ScenarioType::EdgeAbort => {
            let percent = parameters.get("abortPercent")?.as_u64()? as u32;
            let status = parameters
                .get("abortHttpStatus")
                .and_then(|v| v.as_u64())
                .unwrap_or(503) as u16;
            Some(build_edge_abort_fault(percent, status))
        }
        _ => None,
    }
}

pub fn build_virtual_service_json(spec: &VirtualServiceSpec) -> serde_json::Value {
    let labels = BTreeMap::from([
        (EXPERIMENT_LABEL.to_string(), spec.experiment_name.clone()),
        (MANAGED_BY_LABEL.to_string(), MANAGED_BY_VALUE.to_string()),
    ]);

    json!({
        "apiVersion": "networking.istio.io/v1beta1",
        "kind": "VirtualService",
        "metadata": {
            "name": spec.name,
            "namespace": spec.namespace,
            "labels": labels,
            "ownerReferences": [{
                "apiVersion": "chaos.io/v1",
                "kind": "ChaosExperiment",
                "name": spec.experiment_name,
                "uid": spec.experiment_uid,
                "controller": true,
                "blockOwnerDeletion": true
            }]
        },
        "spec": {
            "hosts": [&spec.destination_fqdn],
            "http": [
                {
                    "match": [{
                        "sourceLabels": spec.source_labels
                    }],
                    "fault": spec.fault,
                    "route": [{
                        "destination": { "host": &spec.destination_fqdn }
                    }]
                },
                {
                    "route": [{
                        "destination": { "host": &spec.destination_fqdn }
                    }]
                }
            ]
        }
    })
}

pub fn is_chaos_managed(vs_labels: &BTreeMap<String, String>) -> bool {
    vs_labels.get(MANAGED_BY_LABEL).map(|v| v.as_str()) == Some(MANAGED_BY_VALUE)
}
