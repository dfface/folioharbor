#![allow(clippy::expect_used)]

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use axum::{
    Router,
    extract::Extension,
    http::{Request, header::HeaderName},
    middleware,
    routing::get,
};
use folioharbor_application::ports::{BlobStore, BlobStoreError, PromotedBlob, PublicationSource};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use folioharbor_http::middleware::telemetry::{
    MetricAttributes, RequestTraceContext, TraceContext, record_live_operational_metrics,
    trace_request,
};
use http_body_util::BodyExt as _;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tower::ServiceExt as _;
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::prelude::*;

struct ChangingFreeSpace(AtomicU64);

#[async_trait]
impl BlobStore for ChangingFreeSpace {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque("unused".to_owned())
    }
    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn append(&self, _: &StorageKey, _: &[u8]) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
    async fn open_publication(
        &self,
        _: &StorageKey,
    ) -> Result<Box<dyn PublicationSource>, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
}

#[tokio::test]
async fn operational_gauge_refresh_reads_current_pool_and_storage_state_each_time() {
    let storage = ChangingFreeSpace(AtomicU64::new(100));
    let first = record_live_operational_metrics(2, 1, "api", &storage).await;
    storage.0.store(40, Ordering::SeqCst);
    let second = record_live_operational_metrics(5, 3, "api", &storage).await;

    assert_eq!(first.pool_open, 2);
    assert_eq!(first.pool_idle, 1);
    assert_eq!(first.free_storage_bytes, Some(100));
    assert_eq!(second.pool_open, 5);
    assert_eq!(second.pool_idle, 3);
    assert_eq!(second.free_storage_bytes, Some(40));
}

#[test]
fn accepts_only_valid_w3c_traceparent_and_never_reuses_malformed_input() {
    let valid = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let parsed = TraceContext::parse(valid).expect("valid traceparent");
    assert_eq!(parsed.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(parsed.as_header_value(), valid);

    for malformed in [
        "Bearer secret",
        "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
        "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
    ] {
        assert!(TraceContext::parse(malformed).is_none());
    }
}

#[test]
fn metric_attributes_reject_identity_and_storage_cardinality() {
    MetricAttributes::try_new([("method", "GET"), ("route", "/health/ready")])
        .expect("bounded labels");

    for key in [
        "email",
        "title",
        "user_id",
        "item_id",
        "blob_hash",
        "path",
        "storage_key",
    ] {
        assert!(MetricAttributes::try_new([(key, "sensitive-or-unbounded")]).is_err());
    }
}

async fn trace_echo(Extension(trace): Extension<RequestTraceContext>) -> String {
    format!("{}|{}", trace.trace_id(), trace.traceparent())
}

async fn exported_request(inbound: Option<&str>) -> (String, String, String, String) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("folioharbor-http-test");
    let subscriber =
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
    let app = Router::new()
        .route("/trace", get(trace_echo))
        .layer(middleware::from_fn(trace_request));
    let mut request = Request::builder().uri("/trace");
    if let Some(value) = inbound {
        request = request.header(HeaderName::from_static("traceparent"), value);
    }
    let response = app
        .oneshot(
            request
                .body(axum::body::Body::empty())
                .expect("trace request"),
        )
        .with_subscriber(subscriber)
        .await
        .expect("trace response");
    let response_traceparent = response
        .headers()
        .get("traceparent")
        .expect("propagated traceparent")
        .to_str()
        .expect("ASCII traceparent")
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("trace body")
        .to_bytes();
    provider.force_flush().expect("flush spans");
    let spans = exporter.get_finished_spans().expect("exported spans");
    let span = spans
        .iter()
        .find(|span| span.name == "http.request")
        .expect("HTTP server span");
    (
        response_traceparent,
        String::from_utf8(body.to_vec()).expect("trace body UTF-8"),
        span.span_context.trace_id().to_string(),
        span.span_context.span_id().to_string(),
    )
}

#[tokio::test]
async fn headerless_request_returns_and_exposes_the_real_exported_sdk_span_context() {
    let (traceparent, body, exported_trace_id, exported_span_id) = exported_request(None).await;
    assert_eq!(&traceparent[3..35], exported_trace_id);
    assert_eq!(&traceparent[36..52], exported_span_id);
    assert_eq!(body, format!("{exported_trace_id}|{traceparent}"));
}

#[tokio::test]
async fn inbound_w3c_parent_creates_a_child_and_response_injects_the_server_span() {
    let inbound = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let (traceparent, body, exported_trace_id, exported_span_id) =
        exported_request(Some(inbound)).await;
    assert_eq!(exported_trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(&traceparent[36..52], exported_span_id);
    assert_ne!(&traceparent[36..52], &inbound[36..52]);
    assert_eq!(body, format!("{exported_trace_id}|{traceparent}"));
}
