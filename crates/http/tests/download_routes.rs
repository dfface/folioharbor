#![allow(clippy::expect_used)]

#[test]
fn download_contract_exposes_get_head_ranges_and_no_storage_internals() {
    let document: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../../../openapi/folioharbor-v1.yaml"))
            .expect("valid OpenAPI YAML");
    let path = &document["paths"]["/api/v1/items/{item_id}/download"];
    assert!(path.get("get").is_some());
    assert!(path.get("head").is_some());
    let rendered = format!(
        "{}\n{}",
        serde_yaml::to_string(path).expect("render path"),
        serde_yaml::to_string(&document["components"]["responses"]["OriginalEpub"])
            .expect("render response")
    );
    assert!(rendered.contains("Accept-Ranges"));
    assert!(rendered.contains("Content-Range"));
    assert!(rendered.contains("application/epub+zip"));
    let expected_range_pattern = "^[Bb][Yy][Tt][Ee][Ss]=(?:[0-9]+-[0-9]*|-[0-9]+)$";
    let expected_validator_description = "Uses weak comparison; supports wildcard and comma-list members across repeated field-lines";
    for operation in ["get", "head"] {
        let parameters = path[operation]["parameters"]
            .as_sequence()
            .expect("download parameters");
        let range = parameters
            .iter()
            .find(|parameter| parameter["name"] == "Range")
            .expect("Range parameter");
        assert_eq!(range["schema"]["pattern"], expected_range_pattern);
        assert!(
            range["description"]
                .as_str()
                .expect("Range description")
                .contains("one byte range field-line")
        );
        let validator = parameters
            .iter()
            .find(|parameter| parameter["name"] == "If-None-Match")
            .expect("If-None-Match parameter");
        assert_eq!(validator["description"], expected_validator_description);
    }
    for secret in ["storage_key", "storage path", "blob hash", "sha256"] {
        assert!(!rendered.to_ascii_lowercase().contains(secret));
    }
}
