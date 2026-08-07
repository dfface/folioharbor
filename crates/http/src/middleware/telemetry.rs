use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request},
    http::{HeaderValue, header::HeaderName},
    middleware::Next,
    response::Response,
};
use folioharbor_application::config::ObservabilitySettings;
use folioharbor_domain::id::RequestId;
use opentelemetry::{KeyValue, propagation::Extractor, trace::TracerProvider as _};
use opentelemetry_otlp::WithExportConfig as _;
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider, trace::SdkTracerProvider};
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::prelude::*;

const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");
const ALLOWED_METRIC_KEYS: &[&str] = &[
    "method",
    "route",
    "status_class",
    "job_kind",
    "outcome",
    "pool",
    "state",
    "retry_kind",
    "service",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    header: String,
    trace_id: String,
}

impl TraceContext {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.len() != 55 {
            return None;
        }
        let bytes = value.as_bytes();
        if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
            return None;
        }
        let version = &value[0..2];
        let trace_id = &value[3..35];
        let parent_id = &value[36..52];
        let flags = &value[53..55];
        if version == "ff"
            || ![version, trace_id, parent_id, flags]
                .into_iter()
                .all(is_lower_hex)
            || trace_id.bytes().all(|byte| byte == b'0')
            || parent_id.bytes().all(|byte| byte == b'0')
        {
            return None;
        }
        Some(Self {
            header: value.to_owned(),
            trace_id: trace_id.to_owned(),
        })
    }

    #[must_use]
    pub fn generate() -> Self {
        let trace_id = uuid::Uuid::now_v7().simple().to_string();
        let parent_uuid = uuid::Uuid::now_v7().simple().to_string();
        let parent_id = &parent_uuid[..16];
        Self {
            header: format!("00-{trace_id}-{parent_id}-01"),
            trace_id,
        }
    }

    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    #[must_use]
    pub fn as_header_value(&self) -> &str {
        &self.header
    }
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug)]
pub struct MetricAttributes(Vec<KeyValue>);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("metric attributes must use the bounded label allowlist")]
pub struct MetricAttributeError;

impl MetricAttributes {
    /// Builds a bounded metric attribute set.
    ///
    /// # Errors
    ///
    /// Rejects identity, content, storage, path, or unknown high-cardinality keys.
    pub fn try_new<I, K, V>(attributes: I) -> Result<Self, MetricAttributeError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        attributes
            .into_iter()
            .map(|(key, value)| {
                let key = key.as_ref();
                if !ALLOWED_METRIC_KEYS.contains(&key) {
                    return Err(MetricAttributeError);
                }
                Ok(KeyValue::new(key.to_owned(), value.into()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    fn as_slice(&self) -> &[KeyValue] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TelemetryMetrics;

impl TelemetryMetrics {
    pub fn record_request(
        self,
        latency_seconds: f64,
        is_error: bool,
        attributes: &MetricAttributes,
    ) {
        let meter = opentelemetry::global::meter("folioharbor.http");
        meter
            .f64_histogram("folioharbor.http.request.duration")
            .with_unit("s")
            .build()
            .record(latency_seconds, attributes.as_slice());
        if is_error {
            meter
                .u64_counter("folioharbor.http.request.errors")
                .build()
                .add(1, attributes.as_slice());
        }
    }

    pub fn record_queue_depth(self, depth: u64, attributes: &MetricAttributes) {
        opentelemetry::global::meter("folioharbor.worker")
            .u64_gauge("folioharbor.jobs.queue.depth")
            .build()
            .record(depth, attributes.as_slice());
    }

    pub fn record_retry(self, attributes: &MetricAttributes) {
        opentelemetry::global::meter("folioharbor.worker")
            .u64_counter("folioharbor.jobs.retries")
            .build()
            .add(1, attributes.as_slice());
    }

    pub fn record_upload_bytes(self, bytes: u64, attributes: &MetricAttributes) {
        opentelemetry::global::meter("folioharbor.http")
            .u64_histogram("folioharbor.upload.bytes")
            .with_unit("By")
            .build()
            .record(bytes, attributes.as_slice());
    }

    pub fn record_free_storage(self, bytes: u64) {
        opentelemetry::global::meter("folioharbor.storage")
            .u64_gauge("folioharbor.storage.free")
            .with_unit("By")
            .build()
            .record(bytes, &[]);
    }

    pub fn record_pool_state(self, connections: u64, attributes: &MetricAttributes) {
        opentelemetry::global::meter("folioharbor.database")
            .u64_gauge("folioharbor.database.pool.connections")
            .build()
            .record(connections, attributes.as_slice());
    }

    pub fn record_job(self, latency_seconds: f64, is_error: bool, attributes: &MetricAttributes) {
        let meter = opentelemetry::global::meter("folioharbor.worker");
        meter
            .f64_histogram("folioharbor.jobs.duration")
            .with_unit("s")
            .build()
            .record(latency_seconds, attributes.as_slice());
        if is_error {
            meter
                .u64_counter("folioharbor.jobs.errors")
                .build()
                .add(1, attributes.as_slice());
        }
    }
}

pub struct TelemetryGuard {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("observability initialization failed")]
pub struct TelemetryInitError;

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let _ = self.tracer_provider.shutdown();
        let _ = self.meter_provider.shutdown();
    }
}

/// Installs JSON tracing and optional OTLP trace/metric exporters for one process.
///
/// # Errors
///
/// Returns an error when exporter construction, filter parsing, or global subscriber setup fails.
pub fn init_observability(
    service_name: &'static str,
    settings: &ObservabilitySettings,
) -> Result<TelemetryGuard, TelemetryInitError> {
    let resource = Resource::builder_empty()
        .with_attribute(KeyValue::new("service.name", service_name))
        .build();
    let (tracer_provider, meter_provider) = if let Some(endpoint) = settings.otlp_endpoint.as_ref()
    {
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint.as_str())
            .build()
            .map_err(|_| TelemetryInitError)?;
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint.as_str())
            .build()
            .map_err(|_| TelemetryInitError)?;
        (
            SdkTracerProvider::builder()
                .with_batch_exporter(span_exporter)
                .with_resource(resource.clone())
                .build(),
            SdkMeterProvider::builder()
                .with_periodic_exporter(metric_exporter)
                .with_resource(resource)
                .build(),
        )
    } else {
        (
            SdkTracerProvider::builder()
                .with_resource(resource.clone())
                .build(),
            SdkMeterProvider::builder().with_resource(resource).build(),
        )
    };
    opentelemetry::global::set_tracer_provider(tracer_provider.clone());
    opentelemetry::global::set_meter_provider(meter_provider.clone());
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );
    let tracer = tracer_provider.tracer(service_name);
    let filter = tracing_subscriber::EnvFilter::try_new(&settings.log_filter)
        .map_err(|_| TelemetryInitError)?;
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(true),
        )
        .try_init()
        .map_err(|_| TelemetryInitError)?;
    Ok(TelemetryGuard {
        tracer_provider,
        meter_provider,
    })
}

pub async fn trace_request(mut request: Request, next: Next) -> Response {
    let trace = request
        .headers()
        .get(&TRACEPARENT)
        .and_then(|value| value.to_str().ok())
        .and_then(TraceContext::parse)
        .unwrap_or_else(TraceContext::generate);
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::new);
    request.extensions_mut().insert(request_id);
    let request_id = request_id.as_ulid().to_string();
    request.extensions_mut().insert(trace.clone());
    let parent_context = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(request.headers()))
    });
    let span = tracing::info_span!(
        "http.request",
        request_id = %request_id,
        trace_id = %trace.trace_id(),
        method = %method,
        route = %route,
    );
    span.set_parent(parent_context);
    let started = Instant::now();
    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(trace.as_header_value()) {
        response.headers_mut().insert(TRACEPARENT, value);
    }
    let status = response.status();
    let status_class = format!("{}xx", status.as_u16() / 100);
    if let Ok(attributes) = MetricAttributes::try_new([
        ("method", method),
        ("route", route),
        ("status_class", status_class),
    ]) {
        TelemetryMetrics.record_request(
            started.elapsed().as_secs_f64(),
            status.is_server_error(),
            &attributes,
        );
    }
    response
}

struct HeaderExtractor<'a>(&'a axum::http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}
