use std::collections::BTreeMap;

use folioharbor_epub::{ContentSanitizer, ResourceResolver};
use proptest::prelude::*;

struct Resolver(BTreeMap<String, String>);

impl ResourceResolver for Resolver {
    fn resolve(&self, reference: &str) -> Option<String> {
        self.0.get(reference).cloned()
    }
}

fn resolver() -> Resolver {
    Resolver(BTreeMap::from([
        ("../images/cover.png".into(), "asset_7Q2M9K".into()),
        ("../styles/book.css".into(), "asset_42X".into()),
    ]))
}

#[test]
fn removes_executable_and_interactive_html() {
    let html = r#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"/></head><body onload="steal()"><script>steal()</script><form><input/></form><iframe src="x"></iframe><object data="x"></object><a href="https://evil.test">offsite</a><img src="//evil.test/a"/><p onclick="x()">Safe text</p></body></html>"#;
    let output = ContentSanitizer::transform(html, &resolver());
    let lower = output.html.to_ascii_lowercase();
    for forbidden in [
        "<script",
        "<form",
        "<input",
        "<iframe",
        "<object",
        "http-equiv",
        "onload",
        "onclick",
        "https://",
        "//evil.test",
    ] {
        assert!(!lower.contains(forbidden), "found {forbidden} in {lower}");
    }
    assert!(output.html.contains("Safe text"));
}

#[test]
fn rewrites_internal_urls_to_opaque_resource_identifiers() {
    let html = r##"<img src="../images/cover.png"/><link rel="stylesheet" href="../styles/book.css"/><a href="#note">note</a>"##;
    let output = ContentSanitizer::transform(html, &resolver());
    assert!(output.html.contains("resource:asset_7Q2M9K"));
    assert!(output.html.contains("resource:asset_42X"));
    assert!(output.html.contains("href=\"#note\""));
    assert!(!output.html.contains("../"));
}

#[test]
fn keeps_safe_layout_css_and_rejects_unsafe_css() {
    let html = r#"<style>@import 'https://evil.test/a.css'; body { writing-mode: vertical-rl; background: url(javascript:alert(1)); color: #222; } p { background-image: url('../images/cover.png'); behavior: url(x); }</style><p style="position:fixed; writing-mode: vertical-rl; background:url(https://evil.test/x)">text</p>"#;
    let output = ContentSanitizer::transform(html, &resolver());
    let lower = output.html.to_ascii_lowercase();
    assert!(!lower.contains("@import"));
    assert!(!lower.contains("javascript:"));
    assert!(!lower.contains("https://"));
    assert!(!lower.contains("behavior"));
    assert!(!lower.contains("position:fixed"));
    assert!(lower.contains("writing-mode"));
    assert!(lower.contains("color"));
    assert!(lower.contains("resource:asset_7q2m9k"));
}

#[test]
fn css_tokenization_rejects_comment_escape_and_case_obfuscation() {
    let html = r"<style>
        p { background-image: u/**/rl(https://attacker.test/a); color: red; }
        i { background-image: u\72l(//attacker.test/b); writing-mode: vertical-rl; }
        b { background-image: URL(jAvAsCrIpT:alert(1)); }
    </style>";
    let output = ContentSanitizer::transform(html, &resolver());
    let lower = output.html.to_ascii_lowercase();
    assert!(!lower.contains("attacker.test"));
    assert!(!lower.contains("javascript"));
    assert!(!lower.contains("u/**/rl"));
    assert!(!lower.contains("u\\72l"));
    assert!(lower.contains("color:red"), "{lower}");
    assert!(lower.contains("writing-mode:vertical-rl"), "{lower}");
}

proptest! {
    #[test]
    fn transformed_attributes_never_retain_external_or_script_urls(scheme in "[A-Za-z]{1,12}") {
        let input = format!(r#"<a href="{scheme}://attacker.test/x">x</a><img src="//attacker.test/y"/><div onmouseover="javascript:alert(1)">z</div>"#);
        let output = ContentSanitizer::transform(&input, &resolver());
        let lower = output.html.to_ascii_lowercase();
        prop_assert!(!lower.contains("attacker.test"));
        prop_assert!(!lower.contains("javascript:"));
        prop_assert!(!lower.contains("onmouseover"));
    }

    #[test]
    fn transformed_output_never_keeps_generated_executable_markup(
        tag in prop::sample::select(vec!["script", "form", "iframe", "object", "embed", "input"]),
        event in prop::sample::select(vec!["onclick", "onload", "onerror", "onfocus"]),
    ) {
        let input = format!(r#"<{tag} {event}="alert(1)"><p>payload</p></{tag}><div {event}="x">safe</div>"#);
        let output = ContentSanitizer::transform(&input, &resolver()).html.to_ascii_lowercase();
        let opening_tag = format!("<{tag}");
        prop_assert!(!output.contains(&opening_tag));
        prop_assert!(!output.contains(event));
    }

    #[test]
    fn transformed_css_never_keeps_obfuscated_external_urls(
        payload in prop::sample::select(vec![
            "url(https://attacker.test/x)",
            "URL(//attacker.test/x)",
            "u/**/rl(https://attacker.test/x)",
            "u\\72l(javascript:alert(1))",
            "expression(alert(1))",
        ]),
    ) {
        let input = format!("<style>p{{background-image:{payload};color:red}}</style>");
        let output = ContentSanitizer::transform(&input, &resolver()).html.to_ascii_lowercase();
        prop_assert!(!output.contains("attacker.test"));
        prop_assert!(!output.contains("javascript"));
        prop_assert!(!output.contains("expression"));
    }
}
