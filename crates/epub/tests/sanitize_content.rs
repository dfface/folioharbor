use std::collections::BTreeMap;

use folioharbor_epub::{ContentSanitizer, EpubPath, ResourceResolver};
use proptest::prelude::*;

struct Resolver(BTreeMap<String, String>);

impl ResourceResolver for Resolver {
    fn resolve(&self, path: &EpubPath) -> Option<String> {
        self.0.get(path.as_str()).cloned()
    }
}

fn resolver() -> Resolver {
    Resolver(BTreeMap::from([
        ("EPUB/images/cover.png".into(), "asset_7Q2M9K".into()),
        ("EPUB/styles/book.css".into(), "asset_42X".into()),
    ]))
}

#[test]
fn removes_executable_and_interactive_html() -> anyhow::Result<()> {
    let html = r#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"/></head><body onload="steal()"><script>steal()</script><form><input/></form><iframe src="x"></iframe><object data="x"></object><a href="https://evil.test">offsite</a><img src="//evil.test/a"/><p onclick="x()">Safe text</p></body></html>"#;
    let output = ContentSanitizer::transform(html, "EPUB/text/chapter.xhtml", &resolver())?;
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
    Ok(())
}

#[test]
fn rewrites_internal_urls_to_opaque_resource_identifiers() -> anyhow::Result<()> {
    let html = r##"<img src="../images/cover.png"/><link rel="stylesheet" href="../styles/book.css"/><a href="#note">note</a>"##;
    let output = ContentSanitizer::transform(html, "EPUB/text/chapter.xhtml", &resolver())?;
    assert!(output.html.contains("resource:asset_7Q2M9K"));
    assert!(output.html.contains("resource:asset_42X"));
    assert!(output.html.contains("href=\"#note\""));
    assert!(!output.html.contains("../"));
    Ok(())
}

#[test]
fn keeps_safe_layout_css_and_rejects_unsafe_css() -> anyhow::Result<()> {
    let html = r#"<style>@import 'https://evil.test/a.css'; body { writing-mode: vertical-rl; background: url(javascript:alert(1)); color: #222; } p { background-image: url('../images/cover.png'); behavior: url(x); }</style><p style="position:fixed; writing-mode: vertical-rl; background:url(https://evil.test/x)">text</p>"#;
    let output = ContentSanitizer::transform(html, "EPUB/text/chapter.xhtml", &resolver())?;
    let lower = output.html.to_ascii_lowercase();
    assert!(!lower.contains("@import"));
    assert!(!lower.contains("javascript:"));
    assert!(!lower.contains("https://"));
    assert!(!lower.contains("behavior"));
    assert!(!lower.contains("position:fixed"));
    assert!(lower.contains("writing-mode"));
    assert!(lower.contains("color"));
    assert!(lower.contains("resource:asset_7q2m9k"));
    Ok(())
}

proptest! {
    #[test]
    fn transformed_attributes_never_retain_external_or_script_urls(scheme in "[A-Za-z]{1,12}") {
        let input = format!(r#"<a href="{scheme}://attacker.test/x">x</a><img src="//attacker.test/y"/><div onmouseover="javascript:alert(1)">z</div>"#);
        let output = ContentSanitizer::transform(&input, "EPUB/chapter.xhtml", &resolver())
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        let lower = output.html.to_ascii_lowercase();
        prop_assert!(!lower.contains("attacker.test"));
        prop_assert!(!lower.contains("javascript:"));
        prop_assert!(!lower.contains("onmouseover"));
    }
}
