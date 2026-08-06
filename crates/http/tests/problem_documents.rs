#![allow(clippy::expect_used)]

use axum::{
    body::Body,
    http::{Request, StatusCode, header::CONTENT_LANGUAGE},
};
use folioharbor_http::problem_document_router;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

const EMITTED_CODES: &[&str] = &[
    "audit-repository-unavailable",
    "authorization-repository-unavailable",
    "blob-store-unavailable",
    "catalog-repository-unavailable",
    "conflict",
    "csrf-failed",
    "download-repository-unavailable",
    "email-verification-required",
    "epub-filename-required",
    "forbidden",
    "internal-error",
    "invalid",
    "invalid-email",
    "invalid-identifier",
    "invalid-invitation",
    "invalid-invitation-role",
    "invalid-json-body",
    "invalid-library-id",
    "invalid-library-settings",
    "invalid-or-expired-password-reset-token",
    "invalid-or-expired-verification-token",
    "invalid-page",
    "invalid-parser-profile",
    "invalid-password",
    "invalid-progress-request",
    "invalid-query",
    "invalid-registration",
    "invalid-role",
    "invalid-session-id",
    "invalid-upload",
    "invalid-upload-id",
    "invalid-user-id",
    "invalid-uuid",
    "invitation-invalid",
    "item-download-forbidden",
    "item-not-found",
    "library-action-forbidden",
    "library-not-found",
    "library-owner-required",
    "library-quota-exceeded",
    "library-repository-unavailable",
    "library-requires-owner",
    "library-service-unavailable",
    "mail-delivery-unavailable",
    "mail-unavailable",
    "malformed-json",
    "manifestation-not-found",
    "membership-not-found",
    "not-found",
    "owner-not-invitable",
    "payload-too-large",
    "personal-library-provisioning-failed",
    "progress-conflict",
    "progress-mutation-mismatch",
    "publication-resource-malformed",
    "publication-resource-unavailable",
    "quota-exceeded",
    "rate-limit-key-invalid",
    "rate-limit-unavailable",
    "rate-limited",
    "reader-catalog-unavailable",
    "reading-repository-unavailable",
    "required",
    "resource-not-found",
    "session-not-found",
    "storage-exhausted",
    "unauthenticated",
    "upload-already-exists",
    "upload-not-found",
    "upload-not-importable",
    "upload-receipt-lease-lost",
    "upload-repository-unavailable",
    "upload-service-unavailable",
    "upload-state-conflict",
    "unsupported-media-type",
    "unsupported-upload-media-type",
];

async fn get(code: &str, language: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri(format!("/{code}"));
    if let Some(language) = language {
        request = request.header("Accept-Language", language);
    }
    problem_document_router()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("problem router response")
}

#[tokio::test]
async fn every_emitted_problem_type_has_a_public_document() {
    for code in EMITTED_CODES {
        let response = get(code, None).await;
        assert_eq!(response.status(), StatusCode::OK, "missing {code}");
        assert_eq!(response.headers()[CONTENT_LANGUAGE], "en");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert!(
            String::from_utf8_lossy(&body).contains(&format!("<code>{code}</code>")),
            "document must identify {code}"
        );
    }
    assert_eq!(
        get("unknown-problem", None).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn language_negotiation_ranks_only_supported_ranges() {
    let cases = [
        ("fr;q=1, zh-CN;q=0.9, en;q=0.8", "zh-CN"),
        ("zh-CN;q=0, en;q=0.8", "en"),
        ("fr;q=1, *;q=0.6, zh-CN;q=0", "en"),
        ("*;q=0.9, zh-CN;q=0.8", "en"),
        ("fr", "en"),
        ("zh;q=0.9, en;q=0.8", "zh-CN"),
        ("zh-TW;q=1, en;q=0.8", "en"),
        ("zh-HK;q=1, en;q=0.8", "en"),
        ("en-US;q=1, zh;q=0.8", "zh-CN"),
        ("zh-CN;q=bogus, en;q=0.8", "en"),
        ("zh-CN;q=0.1234, en;q=0.8", "en"),
        ("zh-CN;q=0.2, zh-CN;q=0.9, en;q=0.8", "zh-CN"),
    ];
    for (accept, expected) in cases {
        let response = get("item-not-found", Some(accept)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_LANGUAGE], expected, "{accept}");
    }
}
