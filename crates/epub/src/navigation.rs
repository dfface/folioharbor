use quick_xml::{
    XmlVersion,
    events::{BytesStart, Event},
    name::{NamespaceResolver, ResolveResult},
    reader::NsReader,
};

use crate::{EpubError, EpubErrorCode, EpubPath, archive::BoundedArchive, package::TocEntry};

const XHTML_NS: &[u8] = b"http://www.w3.org/1999/xhtml";
const EPUB_NS: &[u8] = b"http://www.idpf.org/2007/ops";

pub(crate) fn parse(
    archive: &BoundedArchive,
    xml: &[u8],
    nav_path: &EpubPath,
) -> Result<Vec<TocEntry>, EpubError> {
    let mut reader = NsReader::from_reader(xml);
    let mut state = NavigationState::new(archive, nav_path);

    loop {
        archive.check_processing(state.depth)?;
        match reader.read_resolved_event() {
            Ok((namespace, Event::Start(element))) => {
                let is_xhtml = is_namespace(&namespace, XHTML_NS);
                let element = element.into_owned();
                let resolver = reader.resolver().clone();
                state.start(&resolver, is_xhtml, &element)?;
            }
            Ok((namespace, Event::Empty(element))) => {
                let is_xhtml = is_namespace(&namespace, XHTML_NS);
                let element = element.into_owned();
                let resolver = reader.resolver().clone();
                state.empty(&resolver, is_xhtml, &element)?;
            }
            Ok((_, Event::Text(text))) => state.text(&text)?,
            Ok((namespace, Event::End(element))) => {
                state.end(is_namespace(&namespace, XHTML_NS), &element)?;
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error()),
        }
    }

    state.finish()
}

struct NavigationState<'a> {
    archive: &'a BoundedArchive,
    nav_path: &'a EpubPath,
    saw_html: bool,
    saw_toc: bool,
    toc_depth: Option<usize>,
    current_anchor: Option<(usize, EpubPath, String)>,
    toc: Vec<TocEntry>,
    depth: usize,
}

impl<'a> NavigationState<'a> {
    fn new(archive: &'a BoundedArchive, nav_path: &'a EpubPath) -> Self {
        Self {
            archive,
            nav_path,
            saw_html: false,
            saw_toc: false,
            toc_depth: None,
            current_anchor: None,
            toc: Vec::new(),
            depth: 0,
        }
    }

    fn start(
        &mut self,
        resolver: &NamespaceResolver,
        is_xhtml: bool,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        self.depth = self.depth.saturating_add(1);
        self.archive.check_processing(self.depth)?;
        let local = element.local_name();
        if self.depth == 1 {
            if !is_xhtml || local.as_ref() != b"html" {
                return Err(error());
            }
            self.saw_html = true;
        } else if is_xhtml && local.as_ref() == b"html" {
            return Err(error());
        }

        if is_xhtml && local.as_ref() == b"nav" && has_toc_type(resolver, element)? {
            if self.saw_toc || self.toc_depth.is_some() {
                return Err(error());
            }
            self.saw_toc = true;
            self.toc_depth = Some(self.depth);
        } else if is_xhtml && local.as_ref() == b"a" && self.toc_depth.is_some() {
            if self.current_anchor.is_some() {
                return Err(error());
            }
            let href = required_href(element)?;
            let href =
                EpubPath::resolve_from(self.nav_path.as_str(), &href).map_err(|_| error())?;
            if !self.archive.contains(&strip_fragment(&href)?) {
                return Err(error());
            }
            self.current_anchor = Some((self.depth, href, String::new()));
        }
        Ok(())
    }

    fn empty(
        &mut self,
        resolver: &NamespaceResolver,
        is_xhtml: bool,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        if is_xhtml && element.local_name().as_ref() == b"nav" && has_toc_type(resolver, element)? {
            if self.saw_toc {
                return Err(error());
            }
            self.saw_toc = true;
        }
        Ok(())
    }

    fn text(&mut self, text: &quick_xml::events::BytesText<'_>) -> Result<(), EpubError> {
        if let Some((_, _, label)) = self.current_anchor.as_mut() {
            label.push_str(
                &text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|_| error())?,
            );
        }
        Ok(())
    }

    fn end(
        &mut self,
        is_xhtml: bool,
        element: &quick_xml::events::BytesEnd<'_>,
    ) -> Result<(), EpubError> {
        let local = element.local_name();
        if is_xhtml
            && local.as_ref() == b"a"
            && self
                .current_anchor
                .as_ref()
                .is_some_and(|(anchor_depth, _, _)| *anchor_depth == self.depth)
        {
            let (_, href, label) = self.current_anchor.take().ok_or_else(error)?;
            let label = label.trim();
            if label.is_empty() {
                return Err(error());
            }
            self.toc.push(TocEntry {
                label: label.to_owned(),
                href,
            });
        }
        if is_xhtml
            && local.as_ref() == b"nav"
            && self
                .toc_depth
                .is_some_and(|nav_depth| nav_depth == self.depth)
        {
            self.toc_depth = None;
        }
        if self.depth == 1 && (!is_xhtml || local.as_ref() != b"html") {
            return Err(error());
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn finish(self) -> Result<Vec<TocEntry>, EpubError> {
        if !self.saw_html
            || !self.saw_toc
            || self.toc.is_empty()
            || self.depth != 0
            || self.toc_depth.is_some()
        {
            return Err(error());
        }
        Ok(self.toc)
    }
}

fn is_namespace(namespace: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == expected)
}

fn has_toc_type(resolver: &NamespaceResolver, element: &BytesStart<'_>) -> Result<bool, EpubError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| error())?;
        if matches!(resolver.resolve_attribute(attribute.key), (ResolveResult::Bound(ns), local) if ns.as_ref() == EPUB_NS && local.as_ref() == b"type")
        {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|_| error())?;
            return Ok(value.split_ascii_whitespace().any(|item| item == "toc"));
        }
    }
    Ok(false)
}

fn required_href(element: &BytesStart<'_>) -> Result<String, EpubError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| error())?;
        if attribute.key.local_name().as_ref() == b"href" {
            return attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map(std::borrow::Cow::into_owned)
                .map_err(|_| error());
        }
    }
    Err(error())
}

fn strip_fragment(path: &EpubPath) -> Result<EpubPath, EpubError> {
    EpubPath::new(path.as_str().split('#').next().unwrap_or(path.as_str())).map_err(|_| error())
}

fn error() -> EpubError {
    EpubError::new(EpubErrorCode::InvalidNavigation)
}
