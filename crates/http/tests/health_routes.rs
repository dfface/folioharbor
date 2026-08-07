#![allow(clippy::expect_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{body::Body, http::Request};
use folioharbor_application::operations::{HealthStatus, OperationsApi, RegistrationGate};
use folioharbor_http::health_router;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

struct FakeOperations {
    readiness: HealthStatus,
    readiness_calls: AtomicUsize,
}

#[async_trait]
impl OperationsApi for FakeOperations {
    async fn readiness(&self) -> HealthStatus {
        self.readiness_calls.fetch_add(1, Ordering::SeqCst);
        self.readiness
    }

    async fn registration_gate(&self) -> RegistrationGate {
        RegistrationGate::Available
    }
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test]
async fn liveness_depends_only_on_the_process_request_loop() {
    let operations = Arc::new(FakeOperations {
        readiness: HealthStatus::Unavailable,
        readiness_calls: AtomicUsize::new(0),
    });

    let response = health_router(operations.clone())
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({"status": "live"})
    );
    assert_eq!(operations.readiness_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn readiness_exposes_only_safe_aggregate_states() {
    for (status, expected_status, expected_body) in [
        (HealthStatus::Ready, 200, "ready"),
        (HealthStatus::BootstrapRequired, 503, "bootstrap_required"),
        (HealthStatus::Unavailable, 503, "unavailable"),
    ] {
        let operations = Arc::new(FakeOperations {
            readiness: status,
            readiness_calls: AtomicUsize::new(0),
        });
        let response = health_router(operations)
            .oneshot(
                Request::builder()
                    .uri("/health/ready")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status().as_u16(), expected_status);
        assert_eq!(
            response_json(response).await,
            serde_json::json!({"status": expected_body})
        );
    }
}
