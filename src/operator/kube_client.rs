use std::collections::BTreeMap;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Service;
use kube::api::{Api, DynamicObject, ListParams, Patch, PatchParams, PostParams};
use kube::{Client, ResourceExt};
use serde_json::json;

use super::crd::{ChaosExperiment, ChaosExperimentStatus};
use super::reconciler::{KubeClient, VirtualServiceInfo};
use super::types::{OperatorError, FINALIZER_NAME};

pub struct RealKubeClient {
    client: Client,
}

impl RealKubeClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn vs_api(&self, ns: &str) -> Api<DynamicObject> {
        let ar = kube::api::ApiResource {
            group: "networking.istio.io".into(),
            version: "v1beta1".into(),
            api_version: "networking.istio.io/v1beta1".into(),
            kind: "VirtualService".into(),
            plural: "virtualservices".into(),
        };
        Api::namespaced_with(self.client.clone(), ns, &ar)
    }
}

#[async_trait]
impl KubeClient for RealKubeClient {
    async fn create_job(&self, ns: &str, job: &Job) -> Result<(), OperatorError> {
        let api: Api<Job> = Api::namespaced(self.client.clone(), ns);
        api.create(&PostParams::default(), job).await?;
        Ok(())
    }

    async fn list_jobs(&self, ns: &str, label_selector: &str) -> Result<Vec<Job>, OperatorError> {
        let api: Api<Job> = Api::namespaced(self.client.clone(), ns);
        let lp = ListParams::default().labels(label_selector);
        let list = api.list(&lp).await?;
        Ok(list.items)
    }

    async fn delete_job(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        let api: Api<Job> = Api::namespaced(self.client.clone(), ns);
        api.delete(name, &Default::default()).await?;
        Ok(())
    }

    async fn list_target_nodes(&self, ns: &str) -> Result<Vec<String>, OperatorError> {
        let api: Api<k8s_openapi::api::core::v1::Pod> =
            Api::namespaced(self.client.clone(), ns);
        let lp = ListParams::default();
        let pods = api.list(&lp).await?;

        let mut nodes: Vec<String> = pods
            .items
            .iter()
            .filter_map(|p| p.spec.as_ref()?.node_name.clone())
            .collect();
        nodes.sort();
        nodes.dedup();
        Ok(nodes)
    }

    async fn get_service_selector(
        &self,
        ns: &str,
        name: &str,
    ) -> Result<BTreeMap<String, String>, OperatorError> {
        let api: Api<Service> = Api::namespaced(self.client.clone(), ns);
        let svc = api.get(name).await?;
        Ok(svc
            .spec
            .and_then(|s| s.selector)
            .unwrap_or_default())
    }

    async fn create_virtual_service(
        &self,
        ns: &str,
        vs_json: &serde_json::Value,
    ) -> Result<(), OperatorError> {
        let api = self.vs_api(ns);
        let data: DynamicObject = serde_json::from_value(vs_json.clone())?;
        api.create(&PostParams::default(), &data).await?;
        Ok(())
    }

    async fn list_virtual_services_for_host(
        &self,
        ns: &str,
        _host: &str,
    ) -> Result<Vec<VirtualServiceInfo>, OperatorError> {
        let api = self.vs_api(ns);
        let lp = ListParams::default();
        let list = api.list(&lp).await?;

        Ok(list
            .items
            .into_iter()
            .map(|vs| VirtualServiceInfo {
                name: vs.name_any(),
                labels: vs.labels().clone(),
            })
            .collect())
    }

    async fn delete_virtual_service(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        let api = self.vs_api(ns);
        api.delete(name, &Default::default()).await?;
        Ok(())
    }

    async fn patch_experiment_status(
        &self,
        ns: &str,
        name: &str,
        status: &ChaosExperimentStatus,
    ) -> Result<(), OperatorError> {
        let api: Api<ChaosExperiment> = Api::namespaced(self.client.clone(), ns);
        let patch = json!({ "status": status });
        api.patch_status(name, &PatchParams::apply("chimp-chaos"), &Patch::Merge(&patch))
            .await?;
        Ok(())
    }

    async fn add_finalizer(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        let api: Api<ChaosExperiment> = Api::namespaced(self.client.clone(), ns);
        let patch = json!({
            "metadata": {
                "finalizers": [FINALIZER_NAME]
            }
        });
        api.patch(name, &PatchParams::apply("chimp-chaos"), &Patch::Merge(&patch))
            .await?;
        Ok(())
    }

    async fn remove_finalizer(&self, ns: &str, name: &str) -> Result<(), OperatorError> {
        let api: Api<ChaosExperiment> = Api::namespaced(self.client.clone(), ns);
        let patch = json!({
            "metadata": {
                "finalizers": []
            }
        });
        api.patch(name, &PatchParams::apply("chimp-chaos"), &Patch::Merge(&patch))
            .await?;
        Ok(())
    }
}
