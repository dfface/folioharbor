#![allow(clippy::expect_used)]

use std::{collections::BTreeSet, fs};

use serde_yaml::{Mapping, Value};

fn resolve<'a>(document: &'a Value, value: &'a Value) -> &'a Value {
    let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
        return value;
    };
    reference
        .strip_prefix("#/")
        .expect("local reference")
        .split('/')
        .fold(document, |node, segment| &node[segment])
}

fn operation<'a>(document: &'a Value, path: &str, method: &str) -> &'a Value {
    &document["paths"][path][method]
}

#[test]
fn openapi_auth_operations_have_resolved_bodies_success_examples_and_actual_statuses() {
    let source = fs::read_to_string(format!(
        "{}/../../openapi/folioharbor-v1.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("OpenAPI document");
    let document: Value = serde_yaml::from_str(&source).expect("valid OpenAPI YAML");

    let expected: [(&str, &str, &[&str]); 9] = [
        (
            "/api/v1/auth/register",
            "post",
            &["202", "400", "413", "415", "422", "429"],
        ),
        (
            "/api/v1/auth/verify-email",
            "post",
            &["204", "400", "413", "415", "422", "429"],
        ),
        (
            "/api/v1/auth/login",
            "post",
            &["200", "400", "401", "403", "413", "415", "422", "429"],
        ),
        ("/api/v1/auth/logout", "post", &["204", "401", "403"]),
        (
            "/api/v1/auth/forgot-password",
            "post",
            &["202", "400", "413", "415", "422", "429"],
        ),
        (
            "/api/v1/auth/reset-password",
            "post",
            &["200", "400", "413", "415", "422", "429"],
        ),
        ("/api/v1/auth/session", "get", &["200", "401"]),
        ("/api/v1/auth/sessions", "get", &["200", "401"]),
        (
            "/api/v1/auth/sessions/{session_id}/revoke",
            "post",
            &["204", "400", "401", "403", "404"],
        ),
    ];
    for (path, method, statuses) in expected {
        let operation = operation(&document, path, method);
        let actual = operation["responses"]
            .as_mapping()
            .expect("responses")
            .keys()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual,
            statuses.iter().copied().collect(),
            "{method} {path}"
        );
        for (status, response) in operation["responses"].as_mapping().expect("responses") {
            let status = status.as_str().expect("status");
            let response = resolve(&document, response);
            if status.starts_with('4') || status.starts_with('5') {
                let schema = resolve(
                    &document,
                    &response["content"]["application/problem+json"]["schema"],
                );
                assert_eq!(schema, &document["components"]["schemas"]["ProblemDetails"]);
            } else if status != "204" {
                let media = &response["content"]["application/json"];
                assert!(
                    media.get("schema").is_some(),
                    "missing success schema for {method} {path}"
                );
                assert!(
                    media.get("example").is_some(),
                    "missing success example for {method} {path}"
                );
                let _ = resolve(&document, &media["schema"]);
            }
        }
    }

    let body_schemas = [
        ("/api/v1/auth/register", "RegisterRequest"),
        ("/api/v1/auth/verify-email", "VerifyEmailRequest"),
        ("/api/v1/auth/login", "LoginRequest"),
        ("/api/v1/auth/forgot-password", "ForgotPasswordRequest"),
        ("/api/v1/auth/reset-password", "ResetPasswordRequest"),
    ];
    for (path, schema_name) in body_schemas {
        let body = resolve(
            &document,
            &operation(&document, path, "post")["requestBody"],
        );
        assert!(body["required"].as_bool().is_some_and(|required| required));
        assert_eq!(
            resolve(&document, &body["content"]["application/json"]["schema"]),
            &document["components"]["schemas"][schema_name]
        );
        assert!(body["content"]["application/json"].get("example").is_some());
    }

    let _mapping: &Mapping = document["components"]["securitySchemes"]
        .as_mapping()
        .expect("security schemes");
}

#[test]
fn openapi_uuid_path_parameters_document_correlated_malformed_value_problems() {
    let source = fs::read_to_string(format!(
        "{}/../../openapi/folioharbor-v1.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("OpenAPI document");
    let document: Value = serde_yaml::from_str(&source).expect("valid OpenAPI YAML");

    for (path, item) in document["paths"].as_mapping().expect("paths") {
        for (method, operation) in item.as_mapping().expect("path item") {
            let has_uuid_path = operation["parameters"]
                .as_sequence()
                .into_iter()
                .flatten()
                .map(|parameter| resolve(&document, parameter))
                .any(|parameter| {
                    parameter["in"] == "path" && parameter["schema"]["format"] == "uuid"
                });
            if !has_uuid_path {
                continue;
            }
            let response = resolve(&document, &operation["responses"]["400"]);
            let media = &response["content"]["application/problem+json"];
            assert_eq!(
                resolve(&document, &media["schema"]),
                &document["components"]["schemas"]["ProblemDetails"]
            );
            assert_eq!(media["example"]["status"], 400);
            assert!(matches!(
                media["example"]["code"].as_str(),
                Some("invalid_session_id" | "invalid_identifier")
            ));
            assert!(media["example"]["request_id"].as_str().is_some());
            assert!(
                response["description"]
                    .as_str()
                    .is_some_and(|description| description.contains("malformed UUID")),
                "{} {} must explain malformed UUID semantics",
                method.as_str().unwrap_or_default(),
                path.as_str().unwrap_or_default()
            );
        }
    }
}
