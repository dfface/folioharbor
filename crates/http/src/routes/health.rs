use std::sync::Arc;

use axum::{Json, Router, http::StatusCode, response::IntoResponse as _, routing::get};
use folioharbor_application::operations::{HealthStatus, OperationsApi};
use serde::Serialize;

#[derive(Clone)]
struct HealthState {
    operations: Arc<dyn OperationsApi>,
}

#[derive(Serialize)]
struct HealthBody {
    status: &'static str,
}

pub(super) fn router(operations: Arc<dyn OperationsApi>) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(HealthState { operations })
}

async fn live() -> Json<HealthBody> {
    Json(HealthBody { status: "live" })
}

async fn ready(
    axum::extract::State(state): axum::extract::State<HealthState>,
) -> axum::response::Response {
    let (status, label) = match state.operations.readiness().await {
        HealthStatus::Ready => (StatusCode::OK, "ready"),
        HealthStatus::BootstrapRequired => (StatusCode::SERVICE_UNAVAILABLE, "bootstrap_required"),
        HealthStatus::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    (status, Json(HealthBody { status: label })).into_response()
}
