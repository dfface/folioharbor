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
            Ok((_, Event::CData(cdata))) => state.cdata(&cdata)?,
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
    label: Option<Label>,
    toc: Vec<(usize, TocEntry)>,
    next_order: usize,
    depth: usize,
}

struct NavPoint {
    depth: usize,
    order: usize,
    label: Option<String>,
    href: Option<EpubPath>,
}

struct Label {
    nav_label_depth: usize,
    text_depth: Option<usize>,
    saw_text: bool,
    value: String,
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
            label: None,
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
        if self.label.is_some() && !is_ncx {
            return Err(error());
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
            b"text" if self.label.is_some() || !self.nav_points.is_empty() => self.start_text(),
            b"text" => Ok(()),
            b"content" => self.set_target(element),
            _ => Ok(()),
        }
    }

    fn empty(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        if self.label.is_some() {
            return Err(error());
        }
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
        if self.nav_map_depth.is_none() || self.label.is_some() {
            return Err(error());
        }
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        self.nav_points.push(NavPoint {
            depth: self.depth,
            order,
            label: None,
            href: None,
        });
        Ok(())
    }

    fn start_label(&mut self) -> Result<(), EpubError> {
        let point = self.nav_points.last().ok_or_else(error)?;
        if self.label.is_some()
            || point.label.is_some()
            || self.depth != point.depth.saturating_add(1)
        {
            return Err(error());
        }
        self.label = Some(Label {
            nav_label_depth: self.depth,
            text_depth: None,
            saw_text: false,
            value: String::new(),
        });
        Ok(())
    }

    fn start_text(&mut self) -> Result<(), EpubError> {
        let label = self.label.as_mut().ok_or_else(error)?;
        if label.saw_text
            || label.text_depth.is_some()
            || self.depth != label.nav_label_depth.saturating_add(1)
        {
            return Err(error());
        }
        label.saw_text = true;
        label.text_depth = Some(self.depth);
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
        let value = text
            .xml_content(XmlVersion::Implicit1_0)
            .map_err(|_| error())?;
        self.text_value(&value)
    }

    fn cdata(&mut self, cdata: &quick_xml::events::BytesCData<'_>) -> Result<(), EpubError> {
        let value = cdata.decode().map_err(|_| error())?;
        self.text_value(&value)
    }

    fn text_value(&mut self, value: &str) -> Result<(), EpubError> {
        if let Some(label) = self.label.as_mut() {
            if label.text_depth == Some(self.depth) {
                label.value.push_str(value);
            } else if !value.trim().is_empty() {
                return Err(error());
            }
        } else if !self.nav_points.is_empty()
            && !value.trim().is_empty()
        {
            return Err(error());
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
        if is_ncx && local.as_ref() == b"text" {
            self.end_text()?;
        } else if is_ncx && local.as_ref() == b"navLabel" {
            self.end_label()?;
        } else if is_ncx && local.as_ref() == b"navPoint" {
            let point = self
                .nav_points
                .pop()
                .filter(|point| point.depth == self.depth)
                .ok_or_else(error)?;
            let label = point.label.as_deref().ok_or_else(error)?.trim();
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

    fn end_text(&mut self) -> Result<(), EpubError> {
        let Some(label) = self.label.as_mut() else {
            return self.nav_points.is_empty().then_some(()).ok_or_else(error);
        };
        if label.text_depth != Some(self.depth) {
            return Err(error());
        }
        label.text_depth = None;
        Ok(())
    }

    fn end_label(&mut self) -> Result<(), EpubError> {
        let label = self.label.take().ok_or_else(error)?;
        if label.nav_label_depth != self.depth || label.text_depth.is_some() || !label.saw_text {
            return Err(error());
        }
        let point = self.nav_points.last_mut().ok_or_else(error)?;
        if point.label.replace(label.value).is_some() {
            return Err(error());
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<TocEntry>, EpubError> {
        if !self.saw_ncx
            || !self.saw_nav_map
            || self.depth != 0
            || self.nav_map_depth.is_some()
            || self.label.is_some()
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
