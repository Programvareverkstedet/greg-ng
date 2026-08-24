use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::util::HealthCheckRegistry;

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

pub fn health_routes(health_checks: HealthCheckRegistry) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(health_checks)
}

async fn health(State(health_checks): State<HealthCheckRegistry>) -> Response {
    match health_checks.all_healthy(HEALTH_CHECK_TIMEOUT).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ok" }))).into_response(),
        Err(failed_check) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "error",
                "failed_check": failed_check,
            })),
        )
            .into_response(),
    }
}
