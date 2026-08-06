#![allow(clippy::expect_used)]

use std::fs;

#[test]
fn progress_openapi_contract_exposes_versioned_etag_and_rfc9457_conflict() {
    let source = fs::read_to_string(format!(
        "{}/../../openapi/folioharbor-v1.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("OpenAPI");
    let document: serde_yaml::Value = serde_yaml::from_str(&source).expect("valid YAML");
    let operation = &document["paths"]["/api/v1/manifestations/{manifestation_id}/progress"];
    assert_eq!(operation["get"]["operationId"], "getReadingProgress");
    assert_eq!(operation["put"]["operationId"], "updateReadingProgress");
    assert_eq!(operation["put"]["parameters"][2]["name"], "If-Match");
    assert!(
        operation["put"]["parameters"][2]["required"]
            .as_bool()
            .is_some_and(|v| v)
    );
    assert_eq!(
        operation["put"]["responses"]["409"]["$ref"],
        "#/components/responses/ProgressConflict"
    );
    assert_eq!(
        document["components"]["schemas"]["Locator"]["extensions"]["version"],
        serde_yaml::Value::Null
    );
    assert_eq!(
        document["components"]["schemas"]["Locator"]["properties"]["extensions"]["properties"]["version"]
            ["const"],
        1
    );
    let conflict = &document["components"]["responses"]["ProgressConflict"]["content"]["application/problem+json"]
        ["schema"];
    assert_eq!(
        conflict["anyOf"][0]["$ref"],
        "#/components/schemas/ProgressConflictProblem"
    );
    assert_eq!(
        conflict["anyOf"][1]["$ref"],
        "#/components/schemas/ProblemDetails"
    );
    let global = &document["components"]["schemas"]["ConflictGlobalReadingProgress"];
    assert_eq!(global["properties"]["version"]["minimum"], 0);
    assert_eq!(global["properties"]["locator"]["oneOf"][1]["type"], "null");
}
