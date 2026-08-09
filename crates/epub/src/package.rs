use std::collections::{BTreeMap, BTreeSet};

use quick_xml::{
    XmlVersion,
    events::{BytesStart, Event},
    name::ResolveResult,
    reader::NsReader,
};

use crate::{EpubError, EpubErrorCode, EpubPath, archive::BoundedArchive, navigation, ncx};

const OPF_NS: &[u8] = b"http://www.idpf.org/2007/opf";
const DC_NS: &[u8] = b"http://purl.org/dc/elements/1.1/";
const MAX_WARNINGS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageVersion {
    Epub2,
    Epub3,
}

impl PackageVersion {
    fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            Some("2.0" | "2.0.1") => Some(Self::Epub2),
            Some("3.0" | "3.1" | "3.2" | "3.3") => Some(Self::Epub3),
            _ => None,
        }
    }
}

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
    version: PackageVersion,
    metadata: Metadata,
    manifest: BTreeMap<String, ManifestItem>,
    order: Vec<String>,
    spine_refs: Vec<(String, bool)>,
    ncx_id: Option<String>,
    epub_two_cover_id: Option<String>,
    guide_cover: Option<EpubPath>,
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
    Guide,
}

impl<'a> PackageState<'a> {
    fn new(archive: &'a BoundedArchive, package_path: &'a EpubPath) -> Self {
        Self {
            archive,
            package_path,
            document: PackageDocument {
                version: PackageVersion::Epub3,
                metadata: Metadata::default(),
                manifest: BTreeMap::new(),
                order: Vec::new(),
                spine_refs: Vec::new(),
                ncx_id: None,
                epub_two_cover_id: None,
                guide_cover: None,
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
            let version = PackageVersion::parse(version.as_deref());
            if !is_opf || local.as_slice() != b"package" || version.is_none() {
                return Err(invalid_package());
            }
            self.saw_package = true;
            self.document.version = version.expect("checked above");
        } else if self.depth == 2 && is_opf {
            self.start_section(&local, element)?;
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
            self.add_metadata_meta(element)?;
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
        } else if self.depth == 3
            && self.section == Some(PackageSection::Guide)
            && is_opf
            && local.as_slice() == b"reference"
        {
            self.add_guide_reference(element)?;
        } else if is_structural(&local) {
            return Err(invalid_package());
        }
        Ok(())
    }

    fn start_section(&mut self, local: &[u8], element: &BytesStart<'_>) -> Result<(), EpubError> {
        let section = match local {
            b"metadata" => Some(PackageSection::Metadata),
            b"manifest" => Some(PackageSection::Manifest),
            b"spine" => Some(PackageSection::Spine),
            b"guide" => Some(PackageSection::Guide),
            b"package" => return Err(invalid_package()),
            _ => None,
        };
        if let Some(section) = section {
            if section != PackageSection::Guide && !self.seen_sections.insert(section) {
                return Err(invalid_package());
            }
            if section == PackageSection::Guide && self.section == Some(PackageSection::Guide) {
                return Err(invalid_package());
            }
            if section == PackageSection::Spine {
                self.document.ncx_id = attribute(element, b"toc")?;
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
            if local.as_ref() == b"meta" && self.section == Some(PackageSection::Metadata) {
                return self.add_metadata_meta(element);
            }
            if local.as_ref() == b"reference" && self.section == Some(PackageSection::Guide) {
                return self.add_guide_reference(element);
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

    fn add_metadata_meta(&mut self, element: &BytesStart<'_>) -> Result<(), EpubError> {
        if self.document.version == PackageVersion::Epub2
            && attribute(element, b"name")?.as_deref() == Some("cover")
        {
            let cover_id = required_attribute(element, b"content")?;
            if self.document.epub_two_cover_id.replace(cover_id).is_some() {
                return Err(invalid_package());
            }
        } else {
            self.unknown_property = attribute(element, b"property")?;
        }
        Ok(())
    }

    fn add_guide_reference(&mut self, element: &BytesStart<'_>) -> Result<(), EpubError> {
        if !attribute(element, b"type")?
            .as_deref()
            .is_some_and(|value| has_property(value, "cover"))
        {
            return Ok(());
        }
        let raw_href = required_attribute(element, b"href")?;
        let href = EpubPath::resolve_from(self.package_path.as_str(), &raw_href)
            .map_err(|_| invalid_package())?;
        if !self.archive.contains(&strip_fragment(&href)?) {
            return Err(invalid_package());
        }
        if self.document.guide_cover.replace(href).is_some() {
            return Err(invalid_package());
        }
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
                push_warning(
                    &mut self.document.warnings,
                    format!("unknown metadata property: {property}"),
                );
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
            && matches!(
                local.as_ref(),
                b"metadata" | b"manifest" | b"spine" | b"guide"
            )
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
        b"package"
            | b"metadata"
            | b"manifest"
            | b"spine"
            | b"guide"
            | b"item"
            | b"itemref"
            | b"reference"
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
    mut document: PackageDocument,
) -> Result<ParsedPublication, EpubError> {
    if document.metadata.titles.is_empty() || document.spine_refs.is_empty() {
        return Err(error(EpubErrorCode::InvalidPackage));
    }
    let mut spine = Vec::with_capacity(document.spine_refs.len());
    let mut readable_spine_ids = BTreeSet::new();
    for (idref, linear) in &document.spine_refs {
        let (id, item) = readable_manifest_item(document.version, &document.manifest, idref)?;
        readable_spine_ids.insert(id);
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
                media_type: catalog_media_type(document.version, id, item, &readable_spine_ids),
            })
        })
        .collect::<Result<Vec<_>, EpubError>>()?;
    let guide_cover = document
        .guide_cover
        .as_ref()
        .filter(|cover| manifest_contains_href(&document.manifest, cover))
        .cloned();
    let cover = document
        .epub_two_cover_id
        .as_deref()
        .and_then(|id| document.manifest.get(id))
        .map(|item| item.href.clone())
        .or_else(|| {
            document
                .manifest
                .values()
                .find(|item| has_property(&item.properties, "cover-image"))
                .map(|item| item.href.clone())
        })
        .or(guide_cover);
    let (toc, warning) = select_toc(archive, &document, &spine)?;
    if let Some(warning) = warning {
        push_compatibility_warning(&mut document.warnings, warning);
    }
    Ok(ParsedPublication {
        metadata: document.metadata,
        spine,
        resources,
        toc,
        cover,
        warnings: document.warnings,
    })
}

fn catalog_media_type(
    version: PackageVersion,
    id: &str,
    item: &ManifestItem,
    readable_spine_ids: &BTreeSet<String>,
) -> String {
    if version == PackageVersion::Epub2
        && item.media_type == "text/html"
        && readable_spine_ids.contains(id)
    {
        "application/xhtml+xml".to_owned()
    } else {
        item.media_type.clone()
    }
}

enum NavigationDocument<'a> {
    Ncx(&'a ManifestItem),
    Nav(&'a ManifestItem),
}

fn select_toc(
    archive: &BoundedArchive,
    document: &PackageDocument,
    spine: &[SpineItem],
) -> Result<(Vec<TocEntry>, Option<String>), EpubError> {
    if let Some(preferred) = preferred_navigation(document)? {
        return Ok((parse_navigation_document(archive, &preferred)?, None));
    }

    let alternatives = navigation_alternatives(document);
    match alternatives.as_slice() {
        [] => Ok((
            spine
                .iter()
                .map(|item| TocEntry {
                    label: item.href.as_str().to_owned(),
                    href: item.href.clone(),
                })
                .collect(),
            Some(
                "navigation missing; generated table of contents from readable spine items".into(),
            ),
        )),
        [alternative] => {
            let label = match alternative {
                NavigationDocument::Ncx(_) => "NCX",
                NavigationDocument::Nav(_) => "navigation document",
            };
            Ok((
                parse_navigation_document(archive, alternative)?,
                Some(format!(
                    "preferred navigation missing; used fallback {label}"
                )),
            ))
        }
        _ => Err(error(EpubErrorCode::InvalidNavigation)),
    }
}

fn preferred_navigation(
    document: &PackageDocument,
) -> Result<Option<NavigationDocument<'_>>, EpubError> {
    match document.version {
        PackageVersion::Epub2 => {
            let Some(ncx_id) = document.ncx_id.as_deref() else {
                return Ok(None);
            };
            let ncx = document
                .manifest
                .get(ncx_id)
                .filter(|item| is_ncx(item))
                .ok_or_else(|| error(EpubErrorCode::InvalidNavigation))?;
            Ok(Some(NavigationDocument::Ncx(ncx)))
        }
        PackageVersion::Epub3 => {
            let navigation_items = document
                .manifest
                .values()
                .filter(|item| has_property(&item.properties, "nav"))
                .collect::<Vec<_>>();
            if navigation_items.len() > 1 {
                return Err(error(EpubErrorCode::InvalidNavigation));
            }
            let Some(navigation_item) = navigation_items.into_iter().next() else {
                return Ok(None);
            };
            if !is_nav(navigation_item) {
                return Err(error(EpubErrorCode::InvalidNavigation));
            }
            Ok(Some(NavigationDocument::Nav(navigation_item)))
        }
    }
}

fn navigation_alternatives(document: &PackageDocument) -> Vec<NavigationDocument<'_>> {
    document
        .manifest
        .values()
        .filter_map(|item| {
            if is_ncx(item) {
                Some(NavigationDocument::Ncx(item))
            } else if is_nav(item) {
                Some(NavigationDocument::Nav(item))
            } else {
                None
            }
        })
        .collect()
}

fn is_ncx(item: &ManifestItem) -> bool {
    item.media_type == "application/x-dtbncx+xml"
}

fn is_nav(item: &ManifestItem) -> bool {
    item.media_type == "application/xhtml+xml" && has_property(&item.properties, "nav")
}

fn parse_navigation_document(
    archive: &BoundedArchive,
    navigation_document: &NavigationDocument<'_>,
) -> Result<Vec<TocEntry>, EpubError> {
    match navigation_document {
        NavigationDocument::Ncx(item) => ncx::parse(
            archive,
            archive
                .get(&strip_fragment(&item.href)?)
                .ok_or_else(|| error(EpubErrorCode::InvalidNavigation))?,
            &item.href,
        ),
        NavigationDocument::Nav(item) => navigation::parse(
            archive,
            archive
                .get(&strip_fragment(&item.href)?)
                .ok_or_else(|| error(EpubErrorCode::InvalidNavigation))?,
            &item.href,
        ),
    }
}

fn readable_manifest_item<'a>(
    version: PackageVersion,
    manifest: &'a BTreeMap<String, ManifestItem>,
    idref: &str,
) -> Result<(String, &'a ManifestItem), EpubError> {
    let mut current = idref;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current.to_owned()) {
            return Err(error(EpubErrorCode::InvalidSpine));
        }
        let item = manifest
            .get(current)
            .ok_or_else(|| error(EpubErrorCode::InvalidSpine))?;
        if is_readable_spine_item(version, item) {
            return Ok((current.to_owned(), item));
        }
        current = item
            .fallback
            .as_deref()
            .ok_or_else(|| error(EpubErrorCode::InvalidSpine))?;
    }
}

fn is_readable_spine_item(version: PackageVersion, item: &ManifestItem) -> bool {
    matches!(
        item.media_type.as_str(),
        "application/xhtml+xml" | "image/svg+xml"
    ) || (version == PackageVersion::Epub2 && item.media_type == "text/html")
}

fn manifest_contains_href(manifest: &BTreeMap<String, ManifestItem>, href: &EpubPath) -> bool {
    let Ok(href) = strip_fragment(href) else {
        return false;
    };
    manifest
        .values()
        .any(|item| strip_fragment(&item.href).is_ok_and(|manifest_href| manifest_href == href))
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(warning);
    }
}

fn push_compatibility_warning(warnings: &mut Vec<String>, warning: String) {
    if warnings.len() >= MAX_WARNINGS {
        warnings.truncate(MAX_WARNINGS - 1);
    }
    warnings.insert(0, warning);
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
