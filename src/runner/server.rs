use actix_web::{web, App, HttpResponse, HttpServer};
use prometheus::{Encoder, Registry, TextEncoder};

use crate::operator::types::RunnerError;

struct AppState {
    registry: Registry,
}

async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({"status": "ok"}))
}

async fn metrics(data: web::Data<AppState>) -> HttpResponse {
    let encoder = TextEncoder::new();
    let metric_families = data.registry.gather();
    let mut buffer = Vec::new();

    if encoder.encode(&metric_families, &mut buffer).is_err() {
        return HttpResponse::InternalServerError().finish();
    }

    HttpResponse::Ok()
        .content_type(encoder.format_type())
        .body(buffer)
}

pub async fn start_server(port: u16, registry: Registry) -> Result<(), RunnerError> {
    let data = web::Data::new(AppState { registry });

    HttpServer::new(move || {
        App::new()
            .app_data(data.clone())
            .route("/health", web::get().to(health))
            .route("/metrics", web::get().to(metrics))
    })
    .bind(("0.0.0.0", port))
    .map_err(|e| RunnerError::ExecutionFailed(format!("failed to bind: {e}")))?
    .run()
    .await
    .map_err(|e| RunnerError::ExecutionFailed(format!("server error: {e}")))
}
