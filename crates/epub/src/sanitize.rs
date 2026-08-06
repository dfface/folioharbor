use std::time::{Duration, Instant};

use cssparser::{Delimiter, Parser, ParserInput, ToCss, Token};
use html5ever::{ParseOpts, parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::EpubPath;

pub trait ResourceResolver {
    /// Returns the trusted EPUB document path used as the base for relative references.
    fn base(&self) -> &EpubPath;

    /// Resolves a validated, canonical, fragment-free EPUB path to an opaque identifier.
    fn resolve(&self, reference: &EpubPath) -> Option<String>;
}

#[derive(Debug, Eq, PartialEq)]
pub struct SanitizedContent {
    pub html: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct SanitizerLimits {
    /// Maximum UTF-8 input size checked before HTML parsing.
    pub max_input_bytes: usize,
    /// Maximum parsed DOM depth, including HTML parser-inserted elements.
    pub max_dom_depth: usize,
    /// Maximum number of parsed DOM nodes.
    pub max_nodes: usize,
    /// Maximum serialized output size after escaping and URL rewriting.
    pub max_output_bytes: usize,
    /// Maximum wall-clock duration for parsing and bounded traversal.
    pub deadline: Duration,
}

impl Default for SanitizerLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 8 * 1024 * 1024,
            max_dom_depth: 256,
            max_nodes: 100_000,
            max_output_bytes: 16 * 1024 * 1024,
            deadline: Duration::from_secs(2),
        }
    }
}

pub struct ContentSanitizer {
    limits: SanitizerLimits,
}

impl Default for ContentSanitizer {
    fn default() -> Self {
        Self::new(SanitizerLimits::default())
    }
}

impl ContentSanitizer {
    #[must_use]
    pub const fn new(limits: SanitizerLimits) -> Self {
        Self { limits }
    }

    /// Removes active content and rewrites local references to opaque resource identifiers.
    ///
    /// Limit or deadline failures are fail-closed: `html` is empty and `warnings` contains a
    /// stable reason. Partial serialized content is never returned.
    #[must_use]
    pub fn transform(&self, html: &str, resolver: &impl ResourceResolver) -> SanitizedContent {
        let started = Instant::now();
        self.transform_with_expiry(html, resolver, || expired(started, self.limits.deadline))
    }

    fn transform_with_expiry(
        &self,
        html: &str,
        resolver: &impl ResourceResolver,
        mut is_expired: impl FnMut() -> bool,
    ) -> SanitizedContent {
        if html.len() > self.limits.max_input_bytes {
            return failed(SanitizeFailure::Input);
        }
        if is_expired() {
            return failed(SanitizeFailure::Deadline);
        }
        let dom = match parse_incrementally(html, &mut is_expired) {
            Ok(dom) => dom,
            Err(reason) => return failed(reason),
        };
        let result = validate_dom(&dom.document, self.limits, &mut is_expired).and_then(|()| {
            serialize_bounded(&dom.document, resolver, self.limits, &mut is_expired)
        });
        dismantle_dom(&dom.document);
        match result {
            Ok(content) => content,
            Err(reason) => failed(reason),
        }
    }
}

fn parse_incrementally(
    html: &str,
    is_expired: &mut impl FnMut() -> bool,
) -> Result<RcDom, SanitizeFailure> {
    const PARSER_CHUNK_BYTES: usize = 4 * 1024;

    let mut parser = parse_document(RcDom::default(), ParseOpts::default());
    let mut offset = 0_usize;
    while offset < html.len() {
        let mut end = offset.saturating_add(PARSER_CHUNK_BYTES).min(html.len());
        while !html.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        parser.process(html[offset..end].into());
        offset = end;
        if is_expired() {
            let dom = parser.finish();
            dismantle_dom(&dom.document);
            return Err(SanitizeFailure::Deadline);
        }
    }

    let dom = parser.finish();
    if is_expired() {
        dismantle_dom(&dom.document);
        Err(SanitizeFailure::Deadline)
    } else {
        Ok(dom)
    }
}

#[derive(Clone, Copy)]
enum SanitizeFailure {
    Input,
    Depth,
    Nodes,
    Output,
    Deadline,
}

fn failed(reason: SanitizeFailure) -> SanitizedContent {
    let reason = match reason {
        SanitizeFailure::Input => "input limit exceeded",
        SanitizeFailure::Depth => "depth limit exceeded",
        SanitizeFailure::Nodes => "nodes limit exceeded",
        SanitizeFailure::Output => "output limit exceeded",
        SanitizeFailure::Deadline => "deadline exceeded",
    };
    SanitizedContent {
        html: String::new(),
        warnings: vec![format!("sanitization failed: {reason}")],
    }
}

enum Work {
    Enter(Handle, usize),
    Exit(String),
}

struct BoundedOutput {
    value: String,
    max_bytes: usize,
}

impl BoundedOutput {
    fn new(max_bytes: usize) -> Self {
        Self {
            value: String::new(),
            max_bytes,
        }
    }

    fn push_str(&mut self, value: &str) -> Result<(), SanitizeFailure> {
        if self
            .value
            .len()
            .checked_add(value.len())
            .is_none_or(|length| length > self.max_bytes)
        {
            return Err(SanitizeFailure::Output);
        }
        self.value.push_str(value);
        Ok(())
    }

    fn push(&mut self, value: char) -> Result<(), SanitizeFailure> {
        let mut encoded = [0_u8; 4];
        self.push_str(value.encode_utf8(&mut encoded))
    }
}

fn validate_dom(
    document: &Handle,
    limits: SanitizerLimits,
    is_expired: &mut impl FnMut() -> bool,
) -> Result<(), SanitizeFailure> {
    let mut nodes = 0_usize;
    let mut work = vec![(document.clone(), 0_usize)];
    while let Some((node, depth)) = work.pop() {
        if is_expired() {
            return Err(SanitizeFailure::Deadline);
        }
        nodes = nodes.saturating_add(1);
        if nodes > limits.max_nodes {
            return Err(SanitizeFailure::Nodes);
        }
        if depth > limits.max_dom_depth {
            return Err(SanitizeFailure::Depth);
        }
        work.extend(
            node.children
                .borrow()
                .iter()
                .map(|child| (child.clone(), depth.saturating_add(1))),
        );
    }
    Ok(())
}

fn serialize_bounded(
    document: &Handle,
    resolver: &impl ResourceResolver,
    limits: SanitizerLimits,
    is_expired: &mut impl FnMut() -> bool,
) -> Result<SanitizedContent, SanitizeFailure> {
    let mut work = document
        .children
        .borrow()
        .iter()
        .rev()
        .map(|child| Work::Enter(child.clone(), 1))
        .collect::<Vec<_>>();
    let mut output = BoundedOutput::new(limits.max_output_bytes);
    let mut warnings = Vec::new();
    let mut nodes = 0_usize;
    while let Some(item) = work.pop() {
        if is_expired() {
            return Err(SanitizeFailure::Deadline);
        }
        match item {
            Work::Exit(tag) => {
                output.push_str("</")?;
                output.push_str(&tag)?;
                output.push('>')?;
            }
            Work::Enter(node, depth) => {
                nodes = nodes.saturating_add(1);
                if nodes > limits.max_nodes {
                    return Err(SanitizeFailure::Nodes);
                }
                if depth > limits.max_dom_depth {
                    return Err(SanitizeFailure::Depth);
                }
                serialize_node(
                    &node,
                    depth,
                    resolver,
                    &mut work,
                    &mut output,
                    &mut warnings,
                )?;
            }
        }
    }
    Ok(SanitizedContent {
        html: output.value,
        warnings,
    })
}

fn serialize_node(
    node: &Handle,
    depth: usize,
    resolver: &impl ResourceResolver,
    work: &mut Vec<Work>,
    output: &mut BoundedOutput,
    warnings: &mut Vec<String>,
) -> Result<(), SanitizeFailure> {
    match &node.data {
        NodeData::Document => push_children(node, depth, work),
        NodeData::Text { contents } => escape_text(contents.borrow().as_ref(), output)?,
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.as_ref().to_ascii_lowercase();
            if is_forbidden_element(&tag) {
                warnings.push(format!("removed element: {tag}"));
            } else if !is_allowed_element(&tag) {
                push_children(node, depth, work);
            } else {
                serialize_element(node, &tag, attrs, depth, resolver, work, output)?;
            }
        }
        NodeData::Doctype { .. }
        | NodeData::Comment { .. }
        | NodeData::ProcessingInstruction { .. } => {}
    }
    Ok(())
}

fn serialize_element(
    node: &Handle,
    tag: &str,
    attrs: &std::cell::RefCell<Vec<html5ever::Attribute>>,
    depth: usize,
    resolver: &impl ResourceResolver,
    work: &mut Vec<Work>,
    output: &mut BoundedOutput,
) -> Result<(), SanitizeFailure> {
    output.push('<')?;
    output.push_str(tag)?;
    for attribute in attrs.borrow().iter() {
        let name = attribute.name.local.as_ref().to_ascii_lowercase();
        if name.starts_with("on") || !is_allowed_attribute(&name) {
            continue;
        }
        let raw = attribute.value.as_ref();
        let value = if matches!(name.as_str(), "href" | "src" | "poster") {
            sanitize_url(raw, resolver)
        } else if name == "style" {
            let css = sanitize_declarations(raw, resolver);
            (!css.is_empty()).then_some(css)
        } else {
            Some(raw.to_owned())
        };
        if let Some(value) = value {
            output.push(' ')?;
            output.push_str(&name)?;
            output.push_str("=\"")?;
            escape_attribute(&value, output)?;
            output.push('"')?;
        }
    }
    output.push('>')?;
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
        output.push_str(&sanitize_stylesheet(&css, resolver))?;
        work.push(Work::Exit(tag.to_owned()));
    } else {
        if !is_void_element(tag) {
            work.push(Work::Exit(tag.to_owned()));
        }
        push_children(node, depth, work);
    }
    Ok(())
}

fn push_children(node: &Handle, depth: usize, work: &mut Vec<Work>) {
    work.extend(
        node.children
            .borrow()
            .iter()
            .rev()
            .map(|child| Work::Enter(child.clone(), depth.saturating_add(1))),
    );
}

fn dismantle_dom(document: &Handle) {
    let mut nodes = vec![document.clone()];
    while let Some(node) = nodes.pop() {
        nodes.extend(std::mem::take(&mut *node.children.borrow_mut()));
    }
}

fn expired(started: Instant, deadline: Duration) -> bool {
    started.elapsed() >= deadline
}

fn sanitize_url(raw: &str, resolver: &impl ResourceResolver) -> Option<String> {
    let value = raw.trim();
    if value.starts_with('#') {
        return safe_fragment(value).then(|| value.to_owned());
    }
    if value.starts_with("//") || value.contains('\0') || has_scheme(value) {
        return None;
    }
    let (reference, fragment) = value
        .split_once('#')
        .map_or((value, None), |(reference, fragment)| {
            (reference, Some(fragment))
        });
    if reference.is_empty()
        || reference.starts_with('/')
        || reference.contains(['\\', '\0'])
        || has_encoded_or_normalized_traversal(reference)
    {
        return None;
    }
    let canonical = EpubPath::resolve_from(resolver.base().as_str(), reference).ok()?;
    let opaque = resolver.resolve(&canonical)?;
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
        rewritten.push_str(fragment);
    }
    Some(rewritten)
}

fn has_encoded_or_normalized_traversal(reference: &str) -> bool {
    let Some(decoded) = percent_decode_for_validation(reference) else {
        return true;
    };
    decoded.starts_with('/')
        || decoded.contains(['\\', '\0'])
        || decoded
            .split('/')
            .any(|component| matches!(component, "." | ".."))
}

fn percent_decode_for_validation(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
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

fn sanitize_stylesheet(css: &str, resolver: &impl ResourceResolver) -> String {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut output = String::new();
    let mut selector = String::new();
    let mut discard_rule = false;
    while let Ok(token) = parser.next().cloned() {
        match token {
            Token::AtKeyword(_) => {
                selector.clear();
                discard_rule = true;
            }
            Token::Semicolon => {
                selector.clear();
                discard_rule = false;
            }
            Token::CurlyBracketBlock => {
                let declarations = parser
                    .parse_nested_block(|nested| {
                        Ok::<_, cssparser::ParseError<'_, ()>>(sanitize_declarations_parser(
                            nested, resolver,
                        ))
                    })
                    .unwrap_or_default();
                if !discard_rule && safe_selector(&selector) && !declarations.is_empty() {
                    output.push_str(selector.trim());
                    output.push('{');
                    output.push_str(&declarations);
                    output.push('}');
                }
                selector.clear();
                discard_rule = false;
            }
            _ if !discard_rule && !token.is_parse_error() => {
                selector.push_str(&token.to_css_string());
            }
            _ => {}
        }
    }
    output
}

fn sanitize_declarations(css: &str, resolver: &impl ResourceResolver) -> String {
    let mut input = ParserInput::new(css);
    sanitize_declarations_parser(&mut Parser::new(&mut input), resolver)
}

fn sanitize_declarations_parser(
    parser: &mut Parser<'_, '_>,
    resolver: &impl ResourceResolver,
) -> String {
    let mut output = Vec::new();
    while let Ok(token) = parser.next().cloned() {
        let Token::Ident(property) = token else {
            skip_declaration(parser);
            continue;
        };
        if parser.expect_colon().is_err() {
            skip_declaration(parser);
            continue;
        }
        let property = property.to_ascii_lowercase();
        if !is_allowed_css_property(&property) {
            skip_declaration(parser);
            continue;
        }
        let value = parser
            .parse_until_after(Delimiter::Semicolon, |declaration| {
                Ok::<_, cssparser::ParseError<'_, ()>>(parse_css_value(declaration, resolver))
            })
            .ok()
            .flatten();
        if let Some(value) = value {
            output.push(format!("{property}:{value}"));
        }
    }
    output.join(";")
}

fn parse_css_value(
    parser: &mut Parser<'_, '_>,
    resolver: &impl ResourceResolver,
) -> Option<String> {
    let mut value = String::new();
    while let Ok(token) = parser.next().cloned() {
        match token {
            Token::Semicolon => break,
            Token::UnquotedUrl(url) => append_css_url(&mut value, &url, resolver)?,
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                let raw = parser
                    .parse_nested_block(|nested| {
                        let token = nested.next()?.clone();
                        let raw = match token {
                            Token::QuotedString(value) | Token::Ident(value) => value.to_string(),
                            _ => return Err(nested.new_custom_error::<(), ()>(())),
                        };
                        nested.expect_exhausted()?;
                        Ok(raw)
                    })
                    .ok()?;
                append_css_url(&mut value, &raw, resolver)?;
            }
            Token::Function(name) if safe_css_function(&name) => {
                let nested = parser
                    .parse_nested_block(|nested| {
                        parse_css_value(nested, resolver)
                            .ok_or_else(|| nested.new_custom_error::<(), ()>(()))
                    })
                    .ok()?;
                value.push_str(&name.to_ascii_lowercase());
                value.push('(');
                value.push_str(&nested);
                value.push(')');
            }
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                consume_nested_block(parser);
                return None;
            }
            Token::AtKeyword(_)
            | Token::BadUrl(_)
            | Token::BadString(_)
            | Token::CloseParenthesis
            | Token::CloseSquareBracket
            | Token::CloseCurlyBracket => return None,
            _ => {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(&token.to_css_string());
            }
        }
    }
    (!value.is_empty()).then_some(value)
}

fn consume_nested_block(parser: &mut Parser<'_, '_>) {
    let _ = parser.parse_nested_block(|nested| {
        while nested.next().is_ok() {}
        Ok::<(), cssparser::ParseError<'_, ()>>(())
    });
}

fn append_css_url(output: &mut String, raw: &str, resolver: &impl ResourceResolver) -> Option<()> {
    let rewritten = sanitize_url(raw, resolver)?;
    output.push_str("url('");
    output.push_str(&rewritten);
    output.push_str("')");
    Some(())
}

fn skip_declaration(parser: &mut Parser<'_, '_>) {
    while let Ok(token) = parser.next() {
        if matches!(token, Token::Semicolon) {
            break;
        }
    }
}

fn safe_selector(selector: &str) -> bool {
    let selector = selector.trim();
    !selector.is_empty()
        && !selector.contains(['@', '<', '>'])
        && !selector.to_ascii_lowercase().contains("url(")
}

fn safe_css_function(name: &str) -> bool {
    ["rgb", "rgba", "hsl", "hsla", "calc", "min", "max", "clamp"]
        .iter()
        .any(|safe| name.eq_ignore_ascii_case(safe))
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

fn escape_text(value: &str, output: &mut BoundedOutput) -> Result<(), SanitizeFailure> {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;")?,
            '<' => output.push_str("&lt;")?,
            '>' => output.push_str("&gt;")?,
            _ => output.push(character)?,
        }
    }
    Ok(())
}

fn escape_attribute(value: &str, output: &mut BoundedOutput) -> Result<(), SanitizeFailure> {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;")?,
            '<' => output.push_str("&lt;")?,
            '"' => output.push_str("&quot;")?,
            _ => output.push(character)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyResolver {
        base: EpubPath,
    }

    impl ResourceResolver for EmptyResolver {
        fn base(&self) -> &EpubPath {
            &self.base
        }

        fn resolve(&self, _reference: &EpubPath) -> Option<String> {
            None
        }
    }

    #[test]
    fn fails_closed_when_deadline_expires_between_parser_chunks() {
        let resolver = EmptyResolver {
            base: EpubPath::new("EPUB/chapter.xhtml").unwrap_or_else(|_| std::process::abort()),
        };
        let html = format!("<p>{}</p>", "text".repeat(4_096));
        let mut checks = 0_usize;

        let output = ContentSanitizer::default().transform_with_expiry(&html, &resolver, || {
            checks = checks.saturating_add(1);
            checks >= 2
        });

        assert!(output.html.is_empty());
        assert_eq!(output.warnings, ["sanitization failed: deadline exceeded"]);
        assert_eq!(checks, 2, "expiry must occur after one parser chunk");
    }
}
