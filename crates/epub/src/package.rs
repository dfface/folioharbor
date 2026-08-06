use std::collections::{BTreeMap, BTreeSet};

use quick_xml::{
    XmlVersion,
    events::{BytesStart, Event},
    name::ResolveResult,
    reader::NsReader,
};

use crate::{EpubError, EpubErrorCode, EpubPath, archive::BoundedArchive, navigation};

const OPF_NS: &[u8] = b"http://www.idpf.org/2007/opf";
const DC_NS: &[u8] = b"http://purl.org/dc/elements/1.1/";

#[derive(Debug, Default, Eq, PartialEq)]
pub struct Metadata {
    pub titles: Vec<String>,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub identifiers: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PublicationResource {
    pub id: String,
    pub href: EpubPath,
    pub media_type: String,
}

#[derive(Debug, Eq, PartialEq)]
pub struct SpineItem {
    pub href: EpubPath,
    pub linear: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct TocEntry {
    pub label: String,
    pub href: EpubPath,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ParsedPublication {
    pub metadata: Metadata,
    pub spine: Vec<SpineItem>,
    pub resources: Vec<PublicationResource>,
    pub toc: Vec<TocEntry>,
    pub cover: Option<EpubPath>,
    pub warnings: Vec<String>,
}

struct ManifestItem {
    href: EpubPath,
    media_type: String,
    properties: String,
    fallback: Option<String>,
}

struct PackageDocument {
    metadata: Metadata,
    manifest: BTreeMap<String, ManifestItem>,
    order: Vec<String>,
    spine_refs: Vec<(String, bool)>,
    warnings: Vec<String>,
}

pub(crate) fn parse(
    archive: &BoundedArchive,
    package_path: &EpubPath,
) -> Result<ParsedPublication, EpubError> {
    let xml = archive
        .get(package_path)
        .ok_or_else(|| error(EpubErrorCode::MissingPackage))?;
    let mut reader = NsReader::from_reader(xml);
    let mut state = PackageState::new(archive, package_path);

    loop {
        archive.check_processing(state.depth)?;
        match reader.read_resolved_event() {
            Ok((namespace, Event::Start(element))) => state.start(&namespace, &element)?,
            Ok((namespace, Event::Empty(element))) => state.empty(&namespace, &element)?,
            Ok((_, Event::Text(text))) => state.text(&text)?,
            Ok((namespace, Event::End(element))) => state.end(&namespace, &element)?,
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error(EpubErrorCode::InvalidPackage)),
        }
    }
    build_publication(archive, state.finish()?)
}

struct PackageState<'a> {
    archive: &'a BoundedArchive,
    package_path: &'a EpubPath,
    document: PackageDocument,
    metadata_field: Option<Vec<u8>>,
    unknown_property: Option<String>,
    depth: usize,
    saw_package: bool,
    seen_sections: BTreeSet<PackageSection>,
    section: Option<PackageSection>,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum PackageSection {
    Metadata,
    Manifest,
    Spine,
}

impl<'a> PackageState<'a> {
    fn new(archive: &'a BoundedArchive, package_path: &'a EpubPath) -> Self {
        Self {
            archive,
            package_path,
            document: PackageDocument {
                metadata: Metadata::default(),
                manifest: BTreeMap::new(),
                order: Vec::new(),
                spine_refs: Vec::new(),
                warnings: Vec::new(),
            },
            metadata_field: None,
            unknown_property: None,
            depth: 0,
            saw_package: false,
            seen_sections: BTreeSet::new(),
            section: None,
        }
    }

    fn start(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        self.depth = self.depth.saturating_add(1);
        self.archive.check_processing(self.depth)?;
        let local = element.local_name().as_ref().to_vec();
        let is_opf = is_namespace(namespace, OPF_NS);
        let is_dc = is_namespace(namespace, DC_NS);
        if self.depth == 1 {
            let version = attribute(element, b"version")?;
            if !is_opf
                || local.as_slice() != b"package"
                || !version
                    .as_deref()
                    .is_some_and(|value| value.starts_with("3."))
            {
                return Err(invalid_package());
            }
            self.saw_package = true;
        } else if self.depth == 2 && is_opf {
            self.start_section(&local)?;
        } else if self.depth == 3 && self.section == Some(PackageSection::Metadata) && is_dc {
            if matches!(
                local.as_slice(),
                b"title" | b"creator" | b"language" | b"identifier"
            ) {
                self.metadata_field = Some(local);
            } else {
                self.unknown_property = Some(format!("dc:{}", String::from_utf8_lossy(&local)));
            }
        } else if self.depth == 3
            && self.section == Some(PackageSection::Metadata)
            && is_opf
            && local.as_slice() == b"meta"
        {
            self.unknown_property = attribute(element, b"property")?;
        } else if self.depth == 3
            && self.section == Some(PackageSection::Manifest)
            && is_opf
            && local.as_slice() == b"item"
        {
            self.add_manifest_item(element)?;
        } else if self.depth == 3
            && self.section == Some(PackageSection::Spine)
            && is_opf
            && local.as_slice() == b"itemref"
        {
            self.add_spine_ref(element)?;
        } else if is_structural(&local) {
            return Err(invalid_package());
        }
        Ok(())
    }

    fn start_section(&mut self, local: &[u8]) -> Result<(), EpubError> {
        let section = match local {
            b"metadata" => Some(PackageSection::Metadata),
            b"manifest" => Some(PackageSection::Manifest),
            b"spine" => Some(PackageSection::Spine),
            b"package" => return Err(invalid_package()),
            _ => None,
        };
        if let Some(section) = section {
            if !self.seen_sections.insert(section) {
                return Err(invalid_package());
            }
            self.section = Some(section);
        }
        Ok(())
    }

    fn empty(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &BytesStart<'_>,
    ) -> Result<(), EpubError> {
        let local = element.local_name();
        if is_namespace(namespace, OPF_NS) && self.depth == 2 {
            if local.as_ref() == b"item" && self.section == Some(PackageSection::Manifest) {
                return self.add_manifest_item(element);
            }
            if local.as_ref() == b"itemref" && self.section == Some(PackageSection::Spine) {
                return self.add_spine_ref(element);
            }
        }
        if is_structural(local.as_ref()) {
            return Err(invalid_package());
        }
        Ok(())
    }

    fn add_manifest_item(&mut self, element: &BytesStart<'_>) -> Result<(), EpubError> {
        let (id, item) = parse_manifest_item(self.archive, self.package_path, element)?;
        if self.document.manifest.insert(id.clone(), item).is_some() {
            return Err(invalid_package());
        }
        self.document.order.push(id);
        Ok(())
    }

    fn add_spine_ref(&mut self, element: &BytesStart<'_>) -> Result<(), EpubError> {
        let idref = required_attribute(element, b"idref")?;
        let linear = attribute(element, b"linear")?.as_deref() != Some("no");
        self.document.spine_refs.push((idref, linear));
        Ok(())
    }

    fn text(&mut self, text: &quick_xml::events::BytesText<'_>) -> Result<(), EpubError> {
        let value = text
            .xml_content(XmlVersion::Implicit1_0)
            .map_err(|_| invalid_package())?
            .trim()
            .to_owned();
        if !value.is_empty() {
            if let Some(local) = self.metadata_field.as_ref() {
                push_metadata(&mut self.document.metadata, local, value);
            } else if let Some(property) = self.unknown_property.take() {
                self.document
                    .warnings
                    .push(format!("unknown metadata property: {property}"));
            }
        }
        Ok(())
    }

    fn end(
        &mut self,
        namespace: &ResolveResult<'_>,
        element: &quick_xml::events::BytesEnd<'_>,
    ) -> Result<(), EpubError> {
        let local = element.local_name();
        let is_opf = is_namespace(namespace, OPF_NS);
        if is_namespace(namespace, DC_NS) {
            self.metadata_field = None;
        }
        if is_opf && local.as_ref() == b"meta" {
            self.unknown_property = None;
        }
        if self.depth == 2
            && is_opf
            && matches!(local.as_ref(), b"metadata" | b"manifest" | b"spine")
        {
            self.section = None;
        }
        if self.depth == 1 && (!is_opf || local.as_ref() != b"package") {
            return Err(invalid_package());
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    fn finish(self) -> Result<PackageDocument, EpubError> {
        if !self.saw_package
            || self.seen_sections.len() != 3
            || self.depth != 0
            || self.section.is_some()
        {
            return Err(invalid_package());
        }
        Ok(self.document)
    }
}

fn is_namespace(namespace: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == expected)
}

fn is_structural(local: &[u8]) -> bool {
    matches!(
        local,
        b"package" | b"metadata" | b"manifest" | b"spine" | b"item" | b"itemref"
    )
}

fn invalid_package() -> EpubError {
    error(EpubErrorCode::InvalidPackage)
}

fn parse_manifest_item(
    archive: &BoundedArchive,
    package_path: &EpubPath,
    element: &BytesStart<'_>,
) -> Result<(String, ManifestItem), EpubError> {
    let id = required_attribute(element, b"id")?;
    let raw_href = required_attribute(element, b"href")?;
    let href = EpubPath::resolve_from(package_path.as_str(), &raw_href)
        .map_err(|_| error(EpubErrorCode::InvalidPackage))?;
    if !archive.contains(&strip_fragment(&href)?) {
        return Err(error(EpubErrorCode::InvalidPackage));
    }
    let item = ManifestItem {
        href,
        media_type: required_attribute(element, b"media-type")?,
        properties: attribute(element, b"properties")?.unwrap_or_default(),
        fallback: attribute(element, b"fallback")?,
    };
    Ok((id, item))
}

fn push_metadata(metadata: &mut Metadata, local: &[u8], value: String) {
    match local {
        b"title" => metadata.titles.push(value),
        b"creator" => metadata.authors.push(value),
        b"language" => metadata.languages.push(value),
        b"identifier" => metadata.identifiers.push(value),
        _ => {}
    }
}

fn build_publication(
    archive: &BoundedArchive,
    document: PackageDocument,
) -> Result<ParsedPublication, EpubError> {
    if document.metadata.titles.is_empty() || document.spine_refs.is_empty() {
        return Err(error(EpubErrorCode::InvalidPackage));
    }
    let mut spine = Vec::with_capacity(document.spine_refs.len());
    for (idref, linear) in &document.spine_refs {
        let item = readable_manifest_item(&document.manifest, idref)?;
        spine.push(SpineItem {
            href: item.href.clone(),
            linear: *linear,
        });
    }
    let resources = document
        .order
        .iter()
        .map(|id| {
            let item = document
                .manifest
                .get(id)
                .ok_or_else(|| error(EpubErrorCode::InvalidPackage))?;
            Ok(PublicationResource {
                id: id.clone(),
                href: item.href.clone(),
                media_type: item.media_type.clone(),
            })
        })
        .collect::<Result<Vec<_>, EpubError>>()?;
    let cover = document
        .manifest
        .values()
        .find(|item| has_property(&item.properties, "cover-image"))
        .map(|item| item.href.clone());
    let mut navigation_items = document
        .manifest
        .values()
        .filter(|item| has_property(&item.properties, "nav"));
    let nav = navigation_items
        .next()
        .filter(|item| item.media_type == "application/xhtml+xml")
        .ok_or_else(|| error(EpubErrorCode::InvalidNavigation))?;
    if navigation_items.next().is_some() {
        return Err(error(EpubErrorCode::InvalidNavigation));
    }
    let toc = navigation::parse(
        archive,
        archive
            .get(&strip_fragment(&nav.href)?)
            .ok_or_else(|| error(EpubErrorCode::InvalidNavigation))?,
        &nav.href,
    )?;
    Ok(ParsedPublication {
        metadata: document.metadata,
        spine,
        resources,
        toc,
        cover,
        warnings: document.warnings,
    })
}

fn readable_manifest_item<'a>(
    manifest: &'a BTreeMap<String, ManifestItem>,
    idref: &str,
) -> Result<&'a ManifestItem, EpubError> {
    let mut current = idref;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current.to_owned()) {
            return Err(error(EpubErrorCode::InvalidSpine));
        }
        let item = manifest
            .get(current)
            .ok_or_else(|| error(EpubErrorCode::InvalidSpine))?;
        if matches!(
            item.media_type.as_str(),
            "application/xhtml+xml" | "image/svg+xml"
        ) {
            return Ok(item);
        }
        current = item
            .fallback
            .as_deref()
            .ok_or_else(|| error(EpubErrorCode::InvalidSpine))?;
    }
}

fn attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>, EpubError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| error(EpubErrorCode::InvalidPackage))?;
        if attribute.key.local_name().as_ref() == name {
            return attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|_| error(EpubErrorCode::InvalidPackage));
        }
    }
    Ok(None)
}

fn required_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<String, EpubError> {
    attribute(element, name)?.ok_or_else(|| error(EpubErrorCode::InvalidPackage))
}

fn has_property(properties: &str, expected: &str) -> bool {
    properties
        .split_ascii_whitespace()
        .any(|property| property == expected)
}

fn strip_fragment(path: &EpubPath) -> Result<EpubPath, EpubError> {
    EpubPath::new(path.as_str().split('#').next().unwrap_or(path.as_str()))
}

fn error(code: EpubErrorCode) -> EpubError {
    EpubError::new(code)
}
