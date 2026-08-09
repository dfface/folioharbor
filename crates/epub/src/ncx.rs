use quick_xml::{
    XmlVersion,
    events::{BytesStart, Event},
    name::ResolveResult,
    reader::NsReader,
};

use crate::{EpubError, EpubErrorCode, EpubPath, archive::BoundedArchive, package::TocEntry};

const NCX_NS: &[u8] = b"http://www.daisy.org/z3986/2005/ncx/";

pub(crate) fn parse(
    archive: &BoundedArchive,
    xml: &[u8],
    ncx_path: &EpubPath,
) -> Result<Vec<TocEntry>, EpubError> {
    let mut reader = NsReader::from_reader(xml);
    let mut state = NcxState::new(archive, ncx_path);

    loop {
        archive.check_processing(state.depth)?;
        match reader.read_resolved_event() {
            Ok((namespace, Event::Start(element))) => state.start(&namespace, &element)?,
            Ok((namespace, Event::Empty(element))) => state.empty(&namespace, &element)?,
            Ok((_, Event::Text(text))) => state.text(&text)?,
            Ok((namespace, Event::End(element))) => state.end(&namespace, &element)?,
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error()),
        }
    }

    state.finish()
}

struct NcxState<'a> {
    archive: &'a BoundedArchive,
    ncx_path: &'a EpubPath,
    saw_ncx: bool,
    saw_nav_map: bool,
    nav_map_depth: Option<usize>,
    nav_points: Vec<NavPoint>,
    label_depth: Option<usize>,
    toc: Vec<(usize, TocEntry)>,
    next_order: usize,
    depth: usize,
}

struct NavPoint {
    depth: usize,
    order: usize,
    label: String,
    href: Option<EpubPath>,
}

impl<'a> NcxState<'a> {
    fn new(archive: &'a BoundedArchive, ncx_path: &'a EpubPath) -> Self {
        Self {
            archive,
            ncx_path,
            saw_ncx: false,
            saw_nav_map: false,
            nav_map_depth: None,
            nav_points: Vec::new(),
            label_depth: None,
            toc: Vec::new(),
            next_order: 0,
            depth: 0,
        }
    }

    fn start(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        self.depth = self.depth.saturating_add(1);
        self.archive.check_processing(self.depth)?;
        let local = element.local_name();
        let is_ncx = is_namespace(namespace, NCX_NS);

        if self.depth == 1 {
            if !is_ncx || local.as_ref() != b"ncx" || self.saw_ncx {
                return Err(error());
            }
            self.saw_ncx = true;
            return Ok(());
        }
        if !is_ncx {
            return Ok(());
        }
        match local.as_ref() {
            b"ncx" => Err(error()),
            b"navMap" => {
                if self.depth != 2 || self.saw_nav_map {
                    return Err(error());
                }
                self.saw_nav_map = true;
                self.nav_map_depth = Some(self.depth);
                Ok(())
            }
            b"navPoint" => self.start_nav_point(),
            b"navLabel" => self.start_label(),
            b"content" => self.set_target(element),
            _ => Ok(()),
        }
    }

    fn empty(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        if !is_namespace(namespace, NCX_NS) {
            return Ok(());
        }
        match element.local_name().as_ref() {
            b"content" => self.set_target(element),
            b"ncx" | b"navMap" | b"navPoint" | b"navLabel" => Err(error()),
            _ => Ok(()),
        }
    }

    fn start_nav_point(&mut self) -> Result<(), EpubError> {
        if self.nav_map_depth.is_none() || self.label_depth.is_some() {
            return Err(error());
        }
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        self.nav_points.push(NavPoint {
            depth: self.depth,
            order,
            label: String::new(),
            href: None,
        });
        Ok(())
    }

    fn start_label(&mut self) -> Result<(), EpubError> {
        let point = self.nav_points.last().ok_or_else(error)?;
        if self.label_depth.is_some() || self.depth != point.depth.saturating_add(1) {
            return Err(error());
        }
        self.label_depth = Some(self.depth);
        Ok(())
    }

    fn set_target(&mut self, element: &BytesStart<'_>) -> Result<(), EpubError> {
        let point = self.nav_points.last_mut().ok_or_else(error)?;
        if point.href.is_some() {
            return Err(error());
        }
        let source = required_attribute(element, b"src")?;
        let href = EpubPath::resolve_from(self.ncx_path.as_str(), &source).map_err(|_| error())?;
        if !self.archive.contains(&strip_fragment(&href)?) {
            return Err(error());
        }
        point.href = Some(href);
        Ok(())
    }

    fn text(&mut self, text: &quick_xml::events::BytesText<'_>) -> Result<(), EpubError> {
        if self.label_depth.is_some() {
            let point = self.nav_points.last_mut().ok_or_else(error)?;
            point.label.push_str(
                &text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|_| error())?,
            );
        }
        Ok(())
    }

    fn end(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &quick_xml::events::BytesEnd<'_>,
    ) -> Result<(), EpubError> {
        let local = element.local_name();
        let is_ncx = is_namespace(namespace, NCX_NS);
        if is_ncx && local.as_ref() == b"navLabel" {
            if self.label_depth != Some(self.depth) {
                return Err(error());
            }
            self.label_depth = None;
        } else if is_ncx && local.as_ref() == b"navPoint" {
            let point = self
                .nav_points
                .pop()
                .filter(|point| point.depth == self.depth)
                .ok_or_else(error)?;
            let label = point.label.trim();
            let href = point.href.ok_or_else(error)?;
            if label.is_empty() {
                return Err(error());
            }
            self.toc.push((
                point.order,
                TocEntry {
                    label: label.to_owned(),
                    href,
                },
            ));
        } else if is_ncx && local.as_ref() == b"navMap" {
            if self.nav_map_depth != Some(self.depth) || !self.nav_points.is_empty() {
                return Err(error());
            }
            self.nav_map_depth = None;
        }
        if self.depth == 1 && (!is_ncx || local.as_ref() != b"ncx") {
            return Err(error());
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<TocEntry>, EpubError> {
        if !self.saw_ncx
            || !self.saw_nav_map
            || self.depth != 0
            || self.nav_map_depth.is_some()
            || self.label_depth.is_some()
            || !self.nav_points.is_empty()
            || self.toc.is_empty()
        {
            return Err(error());
        }
        self.toc.sort_by_key(|(order, _)| *order);
        Ok(self.toc.into_iter().map(|(_, entry)| entry).collect())
    }
}

fn is_namespace(namespace: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == expected)
}

fn required_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<String, EpubError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| error())?;
        if attribute.key.local_name().as_ref() == name {
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
