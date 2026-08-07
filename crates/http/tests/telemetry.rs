#![allow(clippy::expect_used)]

use folioharbor_http::middleware::telemetry::{MetricAttributes, TraceContext};

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
