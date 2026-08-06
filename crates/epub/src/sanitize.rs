use cssparser::{Delimiter, Parser, ParserInput, ToCss, Token};
use html5ever::{ParseOpts, parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub trait ResourceResolver {
    fn resolve(&self, reference: &str) -> Option<String>;
}

#[derive(Debug, Eq, PartialEq)]
pub struct SanitizedContent {
    pub html: String,
    pub warnings: Vec<String>,
}

pub struct ContentSanitizer;

impl ContentSanitizer {
    /// Removes active content and rewrites local references to opaque resource identifiers.
    #[must_use]
    pub fn transform(html: &str, resolver: &impl ResourceResolver) -> SanitizedContent {
        let dom = parse_document(RcDom::default(), ParseOpts::default()).one(html);
        let mut output = String::new();
        let mut warnings = Vec::new();
        for child in dom.document.children.borrow().iter() {
            serialize(child, resolver, &mut output, &mut warnings);
        }
        SanitizedContent {
            html: output,
            warnings,
        }
    }
}

fn serialize(
    node: &Handle,
    resolver: &impl ResourceResolver,
    output: &mut String,
    warnings: &mut Vec<String>,
) {
    match &node.data {
        NodeData::Document => serialize_children(node, resolver, output, warnings),
        NodeData::Text { contents } => {
            escape_text(contents.borrow().as_ref(), output);
        }
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.as_ref().to_ascii_lowercase();
            if is_forbidden_element(&tag) {
                warnings.push(format!("removed element: {tag}"));
                return;
            }
            if !is_allowed_element(&tag) {
                serialize_children(node, resolver, output, warnings);
                return;
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
                    sanitize_url(raw, resolver)
                } else if name == "style" {
                    let css = sanitize_declarations(raw, resolver);
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
                output.push_str(&sanitize_stylesheet(&css, resolver));
            } else {
                serialize_children(node, resolver, output, warnings);
            }
            if !is_void_element(&tag) {
                output.push_str("</");
                output.push_str(&tag);
                output.push('>');
            }
        }
        NodeData::Doctype { .. }
        | NodeData::Comment { .. }
        | NodeData::ProcessingInstruction { .. } => {}
    }
}

fn serialize_children(
    node: &Handle,
    resolver: &impl ResourceResolver,
    output: &mut String,
    warnings: &mut Vec<String>,
) {
    for child in node.children.borrow().iter() {
        serialize(child, resolver, output, warnings);
    }
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
    if reference.is_empty() || reference.starts_with('/') || reference.contains(['\\', '\0']) {
        return None;
    }
    let opaque = resolver.resolve(reference)?;
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
