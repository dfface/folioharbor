#![allow(clippy::expect_used)]

use std::{collections::BTreeSet, fs};

#[test]
fn openapi_parses_covers_registered_auth_routes_and_reuses_problem_details() {
    let source = fs::read_to_string(format!(
        "{}/../../openapi/folioharbor-v1.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("OpenAPI document");
    let document: serde_yaml::Value = serde_yaml::from_str(&source).expect("valid OpenAPI YAML");
    let paths = document["paths"].as_mapping().expect("paths");
    let actual = paths
        .keys()
        .filter_map(serde_yaml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let expected = [
        "/api/v1/auth/register",
        "/api/v1/auth/verify-email",
        "/api/v1/auth/login",
        "/api/v1/auth/logout",
        "/api/v1/auth/forgot-password",
        "/api/v1/auth/reset-password",
        "/api/v1/auth/session",
        "/api/v1/auth/sessions",
        "/api/v1/auth/sessions/{session_id}/revoke",
    ];
    for path in expected {
        assert!(actual.contains(path), "missing {path}");
    }
    let rendered = serde_yaml::to_string(&document).expect("render");
    assert!(rendered.contains("cookieAuth"));
    assert!(rendered.contains("X-CSRF-Token"));
    assert!(rendered.contains("#/components/schemas/ProblemDetails"));
    for (_, item) in paths {
        for (_, operation) in item.as_mapping().expect("path item") {
            if let Some(responses) = operation
                .get("responses")
                .and_then(serde_yaml::Value::as_mapping)
            {
                for (status, response) in responses {
                    let status = status.as_str().unwrap_or_default();
                    if status.starts_with('4') || status.starts_with('5') {
                        let rendered = serde_yaml::to_string(response).expect("response");
                        let is_problem = rendered.contains("#/components/schemas/ProblemDetails")
                            || (rendered.contains("#/components/responses/Problem")
                                && rendered.contains("Problem"));
                        assert!(
                            is_problem,
                            "error {status} does not reference ProblemDetails"
                        );
                    }
                }
            }
        }
    }
}
