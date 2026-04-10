use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use prometheus::{Encoder, Registry, TextEncoder};
use tokio::net::TcpListener;

use crate::operator::types::RunnerError;

pub struct AppState {
    pub registry: Registry,
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "{\"status\":\"ok\"}")
}

async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = state.registry.gather();
    let mut buffer = Vec::new();

    if encoder.encode(&metric_families, &mut buffer).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Vec::new()).into_response();
    }

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, encoder.format_type())],
        buffer,
    )
        .into_response()
}

pub async fn start_server(port: u16, registry: Registry) -> Result<(), RunnerError> {
    let state = Arc::new(AppState { registry });

    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .with_state(state);

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| RunnerError::ExecutionFailed(format!("failed to bind: {e}")))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| RunnerError::ExecutionFailed(format!("server error: {e}")))
}
