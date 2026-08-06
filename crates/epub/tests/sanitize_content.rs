use std::{cell::RefCell, collections::BTreeMap, time::Duration};

use folioharbor_epub::{ContentSanitizer, EpubPath, ResourceResolver, SanitizerLimits};
use proptest::prelude::*;

struct Resolver {
    base: EpubPath,
    resources: BTreeMap<EpubPath, String>,
}

impl ResourceResolver for Resolver {
    fn base(&self) -> &EpubPath {
        &self.base
    }

    fn resolve(&self, reference: &EpubPath) -> Option<String> {
        self.resources.get(reference).cloned()
    }
}

fn resolver() -> Resolver {
    Resolver {
        base: trusted_path("EPUB/text/chapter.xhtml"),
        resources: BTreeMap::from([
            (
                trusted_path("EPUB/text/images/cover.png"),
                "asset_7Q2M9K".into(),
            ),
            (
                trusted_path("EPUB/text/styles/book.css"),
                "asset_42X".into(),
            ),
        ]),
    }
}

fn trusted_path(value: &str) -> EpubPath {
    EpubPath::new(value).unwrap_or_else(|_| std::process::abort())
}

fn sanitize(html: &str) -> folioharbor_epub::SanitizedContent {
    ContentSanitizer::default().transform(html, &resolver())
}

#[test]
fn removes_executable_and_interactive_html() {
    let html = r#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"/></head><body onload="steal()"><script>steal()</script><form><input/></form><iframe src="x"></iframe><object data="x"></object><a href="https://evil.test">offsite</a><img src="//evil.test/a"/><p onclick="x()">Safe text</p></body></html>"#;
    let output = sanitize(html);
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
    let html = r##"<img src="images/cover.png"/><link rel="stylesheet" href="styles/book.css"/><a href="#note">note</a>"##;
    let output = sanitize(html);
    assert!(output.html.contains("resource:asset_7Q2M9K"));
    assert!(output.html.contains("resource:asset_42X"));
    assert!(output.html.contains("href=\"#note\""));
    assert!(!output.html.contains("../"));
}

#[test]
fn keeps_safe_layout_css_and_rejects_unsafe_css() {
    let html = r#"<style>@import 'https://evil.test/a.css'; body { writing-mode: vertical-rl; background: url(javascript:alert(1)); color: #222; } p { background-image: url('images/cover.png'); behavior: url(x); }</style><p style="position:fixed; writing-mode: vertical-rl; background:url(https://evil.test/x)">text</p>"#;
    let output = sanitize(html);
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
    let output = sanitize(html);
    let lower = output.html.to_ascii_lowercase();
    assert!(!lower.contains("attacker.test"));
    assert!(!lower.contains("javascript"));
    assert!(!lower.contains("u/**/rl"));
    assert!(!lower.contains("u\\72l"));
    assert!(lower.contains("color:red"), "{lower}");
    assert!(lower.contains("writing-mode:vertical-rl"), "{lower}");
}

#[test]
fn fails_closed_when_sanitization_limits_are_exceeded() {
    let cases = [
        (
            "input",
            "<p>oversized</p>".to_owned(),
            SanitizerLimits {
                max_input_bytes: 4,
                ..SanitizerLimits::default()
            },
        ),
        (
            "depth",
            format!("{}text{}", "<div>".repeat(2_000), "</div>".repeat(2_000)),
            SanitizerLimits {
                max_dom_depth: 8,
                deadline: Duration::from_secs(30),
                ..SanitizerLimits::default()
            },
        ),
        (
            "nodes",
            format!("<body>{}</body>", "<span>x</span>".repeat(24)),
            SanitizerLimits {
                max_nodes: 8,
                ..SanitizerLimits::default()
            },
        ),
        (
            "output",
            "<p>&amp;&amp;&amp;&amp;</p>".to_owned(),
            SanitizerLimits {
                max_output_bytes: 16,
                ..SanitizerLimits::default()
            },
        ),
        (
            "deadline",
            "<p>expired</p>".to_owned(),
            SanitizerLimits {
                deadline: Duration::ZERO,
                ..SanitizerLimits::default()
            },
        ),
    ];

    for (expected_warning, html, limits) in cases {
        let output = ContentSanitizer::new(limits).transform(&html, &resolver());
        assert!(
            output.html.is_empty(),
            "partial output escaped for {expected_warning}"
        );
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| warning.contains(expected_warning)),
            "missing {expected_warning} warning: {:?}",
            output.warnings
        );
    }
}

struct SpyResolver {
    base: EpubPath,
    calls: RefCell<Vec<EpubPath>>,
}

impl ResourceResolver for SpyResolver {
    fn base(&self) -> &EpubPath {
        &self.base
    }

    fn resolve(&self, reference: &EpubPath) -> Option<String> {
        self.calls.borrow_mut().push(reference.clone());
        Some("asset_safe".into())
    }
}

#[test]
fn resolver_only_receives_canonical_non_traversing_paths() {
    let resolver = SpyResolver {
        base: trusted_path("EPUB/text/chapter.xhtml"),
        calls: RefCell::new(Vec::new()),
    };
    let html = r#"<img src="../../secret"/><img src="safe/../secret"/><img src="%2e%2e/secret"/><img src="%2E%2E%2fsecret"/><img src="images/cover.png#note"/>"#;

    let output = ContentSanitizer::default().transform(html, &resolver);

    assert_eq!(
        resolver.calls.borrow().as_slice(),
        &[trusted_path("EPUB/text/images/cover.png")]
    );
    assert!(output.html.contains("resource:asset_safe#note"));
    assert!(!output.html.contains("secret"));
}

proptest! {
    #[test]
    fn transformed_attributes_never_retain_external_or_script_urls(scheme in "[A-Za-z]{1,12}") {
        let input = format!(r#"<a href="{scheme}://attacker.test/x">x</a><img src="//attacker.test/y"/><div onmouseover="javascript:alert(1)">z</div>"#);
        let output = sanitize(&input);
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
        let output = sanitize(&input).html.to_ascii_lowercase();
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
        let output = sanitize(&input).html.to_ascii_lowercase();
        prop_assert!(!output.contains("attacker.test"));
        prop_assert!(!output.contains("javascript"));
        prop_assert!(!output.contains("expression"));
    }
}
