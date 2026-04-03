use std::sync::Arc;

use futures::StreamExt;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher;
use kube::{Api, Client};
use tokio::time::Duration;

use super::analysis_reconciler;
use super::crd::{ChaosAnalysis, ChaosExperiment, ChaosImpactMap};
use super::graph_builder::{GraphBuilder, GraphBuilderConfig, HttpPrometheusClient};
use super::impact_map_reconciler;
use super::kube_client::RealKubeClient;
use super::reconciler::{self, EdgeResolver, ReconcileResult, ReconcilerConfig};
use super::types::OperatorError;

use async_trait::async_trait;

// ── Experiment controller context ──

pub struct OperatorContext {
    kube_client: RealKubeClient,
    edge_resolver: RealEdgeResolver,
    config: ReconcilerConfig,
}

struct RealEdgeResolver {
    graph_builder: GraphBuilder<HttpPrometheusClient>,
}

#[async_trait]
impl EdgeResolver for RealEdgeResolver {
    async fn resolve_edge(
        &self,
        source: &str,
        destination: &str,
        namespace: &str,
    ) -> Result<super::types::EdgeInfo, OperatorError> {
        self.graph_builder
            .resolve_edge(source, destination, namespace)
            .await
    }
}

async fn reconcile_handler(
    experiment: Arc<ChaosExperiment>,
    ctx: Arc<OperatorContext>,
) -> Result<Action, OperatorError> {
    let result = reconciler::reconcile(
        &experiment,
        &ctx.kube_client,
        Some(&ctx.edge_resolver as &dyn EdgeResolver),
        &ctx.config,
    )
    .await?;

    match result {
        ReconcileResult::Requeue(d) => Ok(Action::requeue(d)),
        ReconcileResult::Done => Ok(Action::await_change()),
    }
}

fn error_policy(
    _experiment: Arc<ChaosExperiment>,
    error: &OperatorError,
    _ctx: Arc<OperatorContext>,
) -> Action {
    tracing::error!(%error, "reconcile error");
    Action::requeue(Duration::from_secs(30))
}

// ── Analysis controller context ──

struct AnalysisContext {
    kube_client: RealKubeClient,
    prom_client: HttpPrometheusClient,
}

async fn reconcile_analysis_handler(
    analysis: Arc<ChaosAnalysis>,
    ctx: Arc<AnalysisContext>,
) -> Result<Action, OperatorError> {
    analysis_reconciler::reconcile_analysis(&analysis, &ctx.kube_client, &ctx.prom_client).await?;
    Ok(Action::requeue(Duration::from_secs(300)))
}

fn analysis_error_policy(
    _analysis: Arc<ChaosAnalysis>,
    error: &OperatorError,
    _ctx: Arc<AnalysisContext>,
) -> Action {
    tracing::error!(%error, "analysis reconcile error");
    Action::requeue(Duration::from_secs(30))
}

// ── ImpactMap controller context ──

struct ImpactMapContext {
    kube_client: RealKubeClient,
    prom_client: HttpPrometheusClient,
}

async fn reconcile_impact_map_handler(
    impact_map: Arc<ChaosImpactMap>,
    ctx: Arc<ImpactMapContext>,
) -> Result<Action, OperatorError> {
    impact_map_reconciler::reconcile_impact_map(&impact_map, &ctx.kube_client, &ctx.prom_client)
        .await?;
    Ok(Action::requeue(Duration::from_secs(300)))
}

fn impact_map_error_policy(
    _impact_map: Arc<ChaosImpactMap>,
    error: &OperatorError,
    _ctx: Arc<ImpactMapContext>,
) -> Action {
    tracing::error!(%error, "impact map reconcile error");
    Action::requeue(Duration::from_secs(30))
}

// ── Run all controllers ──

pub async fn run(client: Client, prometheus_url: &str) -> anyhow::Result<()> {
    let experiments: Api<ChaosExperiment> = Api::all(client.clone());
    let analyses: Api<ChaosAnalysis> = Api::all(client.clone());
    let impact_maps: Api<ChaosImpactMap> = Api::all(client.clone());

    let graph_config = GraphBuilderConfig::default();
    let prom_client = HttpPrometheusClient::new(prometheus_url);
    let graph_builder = GraphBuilder::new(prom_client, graph_config);

    let mut config = ReconcilerConfig::default();
    if let Ok(image) = std::env::var("RUNNER_IMAGE") {
        config.job_builder.runner_image = image;
    }

    let exp_ctx = Arc::new(OperatorContext {
        kube_client: RealKubeClient::new(client.clone()),
        edge_resolver: RealEdgeResolver { graph_builder },
        config,
    });

    let analysis_ctx = Arc::new(AnalysisContext {
        kube_client: RealKubeClient::new(client.clone()),
        prom_client: HttpPrometheusClient::new(prometheus_url),
    });

    tracing::info!("starting ChaosExperiment controller");
    let exp_controller = Controller::new(experiments, watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile_handler, error_policy, exp_ctx)
        .for_each(|res| async move {
            match res {
                Ok(o) => tracing::debug!(?o, "experiment reconciled"),
                Err(e) => tracing::error!(%e, "experiment reconcile failed"),
            }
        });

    tracing::info!("starting ChaosAnalysis controller");
    let analysis_controller = Controller::new(analyses, watcher::Config::default())
        .shutdown_on_signal()
        .run(
            reconcile_analysis_handler,
            analysis_error_policy,
            analysis_ctx,
        )
        .for_each(|res| async move {
            match res {
                Ok(o) => tracing::debug!(?o, "analysis reconciled"),
                Err(e) => tracing::error!(%e, "analysis reconcile failed"),
            }
        });

    let impact_map_ctx = Arc::new(ImpactMapContext {
        kube_client: RealKubeClient::new(client.clone()),
        prom_client: HttpPrometheusClient::new(prometheus_url),
    });

    tracing::info!("starting ChaosImpactMap controller");
    let impact_map_controller = Controller::new(impact_maps, watcher::Config::default())
        .shutdown_on_signal()
        .run(
            reconcile_impact_map_handler,
            impact_map_error_policy,
            impact_map_ctx,
        )
        .for_each(|res| async move {
            match res {
                Ok(o) => tracing::debug!(?o, "impact map reconciled"),
                Err(e) => tracing::error!(%e, "impact map reconcile failed"),
            }
        });

    tokio::join!(exp_controller, analysis_controller, impact_map_controller);

    Ok(())
}
