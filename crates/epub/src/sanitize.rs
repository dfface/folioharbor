use html5ever::{ParseOpts, parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::{EpubError, EpubPath};

pub trait ResourceResolver {
    fn resolve(&self, path: &EpubPath) -> Option<String>;
}

#[derive(Debug, Eq, PartialEq)]
pub struct SanitizedContent {
    pub html: String,
    pub warnings: Vec<String>,
}

pub struct ContentSanitizer;

impl ContentSanitizer {
    /// Removes active content and rewrites local references to opaque resource identifiers.
    ///
    /// # Errors
    ///
    /// Returns an error when `document_path` is not a safe EPUB-internal path.
    pub fn transform(
        html: &str,
        document_path: &str,
        resolver: &impl ResourceResolver,
    ) -> Result<SanitizedContent, EpubError> {
        let document = EpubPath::new(document_path)?;
        let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);
        let mut output = String::new();
        let mut warnings = Vec::new();
        for child in dom.document.children.borrow().iter() {
            serialize(
                child,
                document.as_str(),
                resolver,
                &mut output,
                &mut warnings,
            )?;
        }
        Ok(SanitizedContent {
            html: output,
            warnings,
        })
    }
}

fn serialize(
    node: &Handle,
    document_path: &str,
    resolver: &impl ResourceResolver,
    output: &mut String,
    warnings: &mut Vec<String>,
) -> Result<(), EpubError> {
    match &node.data {
        NodeData::Document => serialize_children(node, document_path, resolver, output, warnings),
        NodeData::Text { contents } => {
            escape_text(contents.borrow().as_ref(), output);
            Ok(())
        }
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.as_ref().to_ascii_lowercase();
            if is_forbidden_element(&tag) {
                warnings.push(format!("removed element: {tag}"));
                return Ok(());
            }
            if !is_allowed_element(&tag) {
                return serialize_children(node, document_path, resolver, output, warnings);
            }
            output.push('<');
            output.push_str(&tag);
            for attribute in attrs.borrow().iter() {
                let name = attribute.name.local.as_ref().to_ascii_lowercase();
                if name.starts_with("on") || !is_allowed_attribute(&name) {
                    continue;
                }
                let raw = attribute.value.as_ref();
                let value = if matches!(name.as_str(), "href" | "src" | "poster") {
                    sanitize_url(raw, document_path, resolver)
                } else if name == "style" {
                    let css = sanitize_declarations(raw, document_path, resolver);
                    (!css.is_empty()).then_some(css)
                } else {
                    Some(raw.to_owned())
                };
                if let Some(value) = value {
                    output.push(' ');
                    output.push_str(&name);
                    output.push_str("=\"");
                    escape_attribute(&value, output);
                    output.push('"');
                }
            }
            output.push('>');
            if tag == "style" {
                let css = node
                    .children
                    .borrow()
                    .iter()
                    .filter_map(|child| match &child.data {
                        NodeData::Text { contents } => Some(contents.borrow().to_string()),
                        _ => None,
                    })
                    .collect::<String>();
                output.push_str(&sanitize_stylesheet(&css, document_path, resolver));
            } else {
                serialize_children(node, document_path, resolver, output, warnings)?;
            }
            if !is_void_element(&tag) {
                output.push_str("</");
                output.push_str(&tag);
                output.push('>');
            }
            Ok(())
        }
        NodeData::Doctype { .. }
        | NodeData::Comment { .. }
        | NodeData::ProcessingInstruction { .. } => Ok(()),
    }
}

fn serialize_children(
    node: &Handle,
    document_path: &str,
    resolver: &impl ResourceResolver,
    output: &mut String,
    warnings: &mut Vec<String>,
) -> Result<(), EpubError> {
    for child in node.children.borrow().iter() {
        serialize(child, document_path, resolver, output, warnings)?;
    }
    Ok(())
}

fn sanitize_url(
    raw: &str,
    document_path: &str,
    resolver: &impl ResourceResolver,
) -> Option<String> {
    let value = raw.trim();
    if value.starts_with('#') {
        return safe_fragment(value).then(|| value.to_owned());
    }
    if value.starts_with("//") || value.contains('\0') || has_scheme(value) {
        return None;
    }
    let target = EpubPath::resolve_from(document_path, value).ok()?;
    let fragment = target
        .as_str()
        .split_once('#')
        .map(|(_, fragment)| fragment.to_owned());
    let resource_path = EpubPath::new(target.as_str().split('#').next()?).ok()?;
    let opaque = resolver.resolve(&resource_path)?;
    if opaque.is_empty()
        || !opaque
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return None;
    }
    let mut rewritten = format!("resource:{opaque}");
    if let Some(fragment) = fragment.filter(|fragment| safe_fragment(fragment)) {
        rewritten.push('#');
        rewritten.push_str(&fragment);
    }
    Some(rewritten)
}

fn has_scheme(value: &str) -> bool {
    let prefix = value.split(['/', '#', '?']).next().unwrap_or(value);
    prefix.find(':').is_some_and(|colon| {
        let scheme = &prefix[..colon];
        !scheme.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

fn safe_fragment(fragment: &str) -> bool {
    fragment.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '#' | '_' | '-' | '.' | ':')
    })
}

fn sanitize_stylesheet(css: &str, document_path: &str, resolver: &impl ResourceResolver) -> String {
    let mut without_imports = css.to_owned();
    loop {
        let lower = without_imports.to_ascii_lowercase();
        let Some(start) = lower.find("@import") else {
            break;
        };
        let end = without_imports[start..]
            .find(';')
            .map_or(without_imports.len(), |offset| start + offset + 1);
        without_imports.replace_range(start..end, "");
    }
    let mut output = String::new();
    for rule in without_imports.split('}') {
        let Some((selector, declarations)) = rule.split_once('{') else {
            continue;
        };
        if selector.contains('@') || selector.contains('<') || selector.contains('>') {
            continue;
        }
        let declarations = sanitize_declarations(declarations, document_path, resolver);
        if !selector.trim().is_empty() && !declarations.is_empty() {
            output.push_str(selector.trim());
            output.push('{');
            output.push_str(&declarations);
            output.push('}');
        }
    }
    output
}

fn sanitize_declarations(
    css: &str,
    document_path: &str,
    resolver: &impl ResourceResolver,
) -> String {
    let mut output = Vec::new();
    for declaration in css.split(';') {
        let Some((property, raw_value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        if !is_allowed_css_property(&property) {
            continue;
        }
        let mut value = raw_value.trim().to_owned();
        let lower = value.to_ascii_lowercase();
        if lower.contains("expression(")
            || lower.contains("javascript:")
            || lower.contains("-moz-binding")
        {
            continue;
        }
        if lower.contains("url(") {
            let Some(rewritten) = rewrite_single_css_url(&value, document_path, resolver) else {
                continue;
            };
            value = rewritten;
        }
        if !value.is_empty() {
            output.push(format!("{property}:{value}"));
        }
    }
    output.join(";")
}

fn rewrite_single_css_url(
    value: &str,
    document_path: &str,
    resolver: &impl ResourceResolver,
) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let start = lower.find("url(")?;
    if lower[start + 4..].contains("url(") {
        return None;
    }
    let end_relative = value[start + 4..].find(')')?;
    let end = start + 4 + end_relative;
    let raw = value[start + 4..end].trim().trim_matches(['\'', '"']);
    let rewritten = sanitize_url(raw, document_path, resolver)?;
    let mut result = value[..start].to_owned();
    result.push_str("url('");
    result.push_str(&rewritten);
    result.push_str("')");
    result.push_str(&value[end + 1..]);
    Some(result)
}

fn is_forbidden_element(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "noscript"
            | "form"
            | "input"
            | "button"
            | "select"
            | "option"
            | "textarea"
            | "iframe"
            | "frame"
            | "frameset"
            | "object"
            | "embed"
            | "applet"
            | "meta"
            | "base"
            | "canvas"
            | "audio"
            | "video"
            | "source"
            | "track"
    )
}

fn is_allowed_element(tag: &str) -> bool {
    matches!(
        tag,
        "html"
            | "head"
            | "body"
            | "title"
            | "style"
            | "link"
            | "main"
            | "section"
            | "article"
            | "aside"
            | "nav"
            | "header"
            | "footer"
            | "div"
            | "span"
            | "p"
            | "br"
            | "hr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ol"
            | "ul"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "blockquote"
            | "pre"
            | "code"
            | "em"
            | "strong"
            | "b"
            | "i"
            | "u"
            | "s"
            | "small"
            | "sub"
            | "sup"
            | "ruby"
            | "rt"
            | "rp"
            | "a"
            | "img"
            | "figure"
            | "figcaption"
            | "table"
            | "caption"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "colgroup"
            | "col"
    )
}

fn is_allowed_attribute(name: &str) -> bool {
    matches!(
        name,
        "id" | "class"
            | "title"
            | "lang"
            | "dir"
            | "role"
            | "epub:type"
            | "href"
            | "src"
            | "alt"
            | "width"
            | "height"
            | "rel"
            | "colspan"
            | "rowspan"
            | "scope"
            | "style"
    ) || name.starts_with("aria-")
}

fn is_allowed_css_property(property: &str) -> bool {
    matches!(
        property,
        "color"
            | "background-color"
            | "background-image"
            | "font"
            | "font-family"
            | "font-size"
            | "font-style"
            | "font-weight"
            | "line-height"
            | "letter-spacing"
            | "text-align"
            | "text-decoration"
            | "text-indent"
            | "text-transform"
            | "white-space"
            | "word-break"
            | "writing-mode"
            | "direction"
            | "display"
            | "margin"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border"
            | "border-width"
            | "border-style"
            | "border-color"
            | "width"
            | "height"
            | "max-width"
            | "max-height"
            | "vertical-align"
            | "list-style"
            | "list-style-type"
            | "page-break-before"
            | "page-break-after"
            | "break-before"
            | "break-after"
            | "orphans"
            | "widows"
    )
}

fn is_void_element(tag: &str) -> bool {
    matches!(tag, "br" | "hr" | "img" | "link" | "col")
}

fn escape_text(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            _ => output.push(character),
        }
    }
}

fn escape_attribute(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '"' => output.push_str("&quot;"),
            _ => output.push(character),
        }
    }
}
