#![allow(clippy::expect_used)]

use std::time::Duration;

use folioharbor_application::error::{AppError, FieldViolation};
use folioharbor_domain::id::{ErrorId, RequestId};
use folioharbor_http::problem::{PROBLEM_CONTENT_TYPE, ProblemContext, ProblemDetails};
use url::Url;

#[test]
fn quota_conflict_has_stable_problem_shape() {
    let problem = ProblemDetails::from_app_error(
        &AppError::Conflict {
            code: "quota_exceeded",
        },
        &ProblemContext::example("01JREQ"),
    );
    assert_eq!(problem.status, 409);
    assert_eq!(problem.code, "quota_exceeded");
    assert_eq!(
        problem.type_uri.as_str(),
        "https://library.example/problems/quota-exceeded"
    );
    assert_eq!(problem.request_id, "01JREQ");
}

#[test]
fn elapsed_item_recovery_window_has_its_documented_public_type() {
    let problem = ProblemDetails::from_app_error(
        &AppError::Conflict {
            code: "item_recovery_window_elapsed",
        },
        &ProblemContext::example("01JREQ"),
    );

    assert_eq!(problem.status, 409);
    assert_eq!(problem.code, "item_recovery_window_elapsed");
    assert_eq!(
        problem.type_uri.as_str(),
        "https://library.example/problems/item-recovery-window-elapsed"
    );
}

#[test]
fn every_application_error_has_a_stable_status_and_code() {
    let errors = [
        (AppError::Unauthenticated, 401, "unauthenticated"),
        (AppError::Forbidden { code: "forbidden" }, 403, "forbidden"),
        (AppError::NotFound { code: "not_found" }, 404, "not_found"),
        (AppError::Conflict { code: "conflict" }, 409, "conflict"),
        (
            AppError::Invalid {
                code: "invalid",
                fields: vec![FieldViolation {
                    field: "email",
                    code: "invalid_format",
                }],
            },
            422,
            "invalid",
        ),
        (AppError::PayloadTooLarge, 413, "payload_too_large"),
        (
            AppError::RateLimited {
                retry_after: Duration::from_secs(30),
            },
            429,
            "rate_limited",
        ),
        (AppError::StorageExhausted, 507, "storage_exhausted"),
        (
            AppError::DependencyUnavailable {
                code: "mail_unavailable",
            },
            503,
            "mail_unavailable",
        ),
        (
            AppError::Internal {
                error_id: ErrorId::new(),
            },
            500,
            "internal_error",
        ),
    ];

    for (error, status, code) in errors {
        let problem = ProblemDetails::from_app_error(&error, &ProblemContext::example("01JREQ"));
        assert_eq!(problem.status, status);
        assert_eq!(problem.code, code);
        assert!(!problem.title.is_empty());
    }
}

#[test]
fn serialized_problems_do_not_leak_internal_error_ids() {
    let problem = ProblemDetails::from_app_error(
        &AppError::Internal {
            error_id: ErrorId::new(),
        },
        &ProblemContext::example("01JREQ"),
    );
    let body = problem.to_json().expect("problem serialization");

    assert_eq!(PROBLEM_CONTENT_TYPE, "application/problem+json");
    assert!(!body.contains("error_id"));
    assert!(body.contains("\"code\":\"internal_error\""));
}

#[test]
fn production_context_builds_types_below_the_public_base_url() {
    let base = Url::parse("https://books.example/base/").expect("valid URL");
    let request_id = RequestId::new();
    let problem = ProblemDetails::from_app_error(
        &AppError::NotFound {
            code: "item_not_found",
        },
        &ProblemContext::new(&base, request_id),
    );

    assert_eq!(
        problem.type_uri.as_str(),
        "https://books.example/base/problems/item-not-found"
    );
    assert_eq!(problem.request_id, request_id.as_ulid().to_string());
}
