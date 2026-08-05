use std::collections::BTreeMap;

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
    let mut metadata = Metadata::default();
    let mut manifest = BTreeMap::new();
    let mut order = Vec::new();
    let mut spine_refs = Vec::new();
    let mut warnings = Vec::new();
    let mut metadata_field: Option<(&'static [u8], Vec<u8>)> = None;
    let mut unknown_property: Option<String> = None;
    let mut depth = 0_usize;

    loop {
        archive.check_processing(depth)?;
        let event = reader.read_resolved_event();
        if matches!(&event, Ok((_, Event::Start(_)))) {
            depth = depth.saturating_add(1);
            archive.check_processing(depth)?;
        } else if matches!(&event, Ok((_, Event::End(_)))) {
            depth = depth.saturating_sub(1);
        }
        match event {
            Ok((ResolveResult::Bound(namespace), Event::Start(element)))
                if namespace.as_ref() == DC_NS =>
            {
                let local = element.local_name().as_ref().to_vec();
                if matches!(
                    local.as_slice(),
                    b"title" | b"creator" | b"language" | b"identifier"
                ) {
                    metadata_field = Some((DC_NS, local));
                } else {
                    unknown_property = Some(format!("dc:{}", String::from_utf8_lossy(&local)));
                }
            }
            Ok((ResolveResult::Bound(namespace), Event::Start(element)))
                if namespace.as_ref() == OPF_NS && element.local_name().as_ref() == b"meta" =>
            {
                unknown_property = attribute(&reader, &element, b"property")?;
            }
            Ok((
                ResolveResult::Bound(namespace),
                Event::Empty(element) | Event::Start(element),
            )) if namespace.as_ref() == OPF_NS && element.local_name().as_ref() == b"item" => {
                let (id, item) = parse_manifest_item(archive, package_path, &reader, &element)?;
                if manifest.insert(id.clone(), item).is_some() {
                    return Err(error(EpubErrorCode::InvalidPackage));
                }
                order.push(id);
            }
            Ok((
                ResolveResult::Bound(namespace),
                Event::Empty(element) | Event::Start(element),
            )) if namespace.as_ref() == OPF_NS && element.local_name().as_ref() == b"itemref" => {
                let idref = required_attribute(&reader, &element, b"idref")?;
                let linear = attribute(&reader, &element, b"linear")?.as_deref() != Some("no");
                spine_refs.push((idref, linear));
            }
            Ok((_, Event::Text(text))) => {
                let value = text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|_| error(EpubErrorCode::InvalidPackage))?
                    .trim()
                    .to_owned();
                if !value.is_empty() {
                    if let Some((_, local)) = metadata_field.as_ref() {
                        push_metadata(&mut metadata, local, value);
                    } else if let Some(property) = unknown_property.take() {
                        warnings.push(format!("unknown metadata property: {property}"));
                    }
                }
            }
            Ok((ResolveResult::Bound(namespace), Event::End(_))) if namespace.as_ref() == DC_NS => {
                metadata_field = None;
            }
            Ok((ResolveResult::Bound(namespace), Event::End(element)))
                if namespace.as_ref() == OPF_NS && element.local_name().as_ref() == b"meta" =>
            {
                unknown_property = None;
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error(EpubErrorCode::InvalidPackage)),
        }
    }

    build_publication(
        archive,
        PackageDocument {
            metadata,
            manifest,
            order,
            spine_refs,
            warnings,
        },
    )
}

fn parse_manifest_item(
    archive: &BoundedArchive,
    package_path: &EpubPath,
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<(String, ManifestItem), EpubError> {
    let id = required_attribute(reader, element, b"id")?;
    let raw_href = required_attribute(reader, element, b"href")?;
    let href = EpubPath::resolve_from(package_path.as_str(), &raw_href)
        .map_err(|_| error(EpubErrorCode::InvalidPackage))?;
    if !archive.contains(&strip_fragment(&href)?) {
        return Err(error(EpubErrorCode::InvalidPackage));
    }
    let item = ManifestItem {
        href,
        media_type: required_attribute(reader, element, b"media-type")?,
        properties: attribute(reader, element, b"properties")?.unwrap_or_default(),
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
    for (idref, linear) in document.spine_refs {
        let item = document
            .manifest
            .get(&idref)
            .ok_or_else(|| error(EpubErrorCode::InvalidSpine))?;
        spine.push(SpineItem {
            href: item.href.clone(),
            linear,
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
    let toc = if let Some(nav) = document
        .manifest
        .values()
        .find(|item| has_property(&item.properties, "nav"))
    {
        navigation::parse(
            archive,
            archive
                .get(&strip_fragment(&nav.href)?)
                .ok_or_else(|| error(EpubErrorCode::InvalidNavigation))?,
            &nav.href,
        )?
    } else {
        Vec::new()
    };
    Ok(ParsedPublication {
        metadata: document.metadata,
        spine,
        resources,
        toc,
        cover,
        warnings: document.warnings,
    })
}

fn attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    name: &[u8],
) -> Result<Option<String>, EpubError> {
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| error(EpubErrorCode::InvalidPackage))?;
        if attribute.key.local_name().as_ref() == name {
            return attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|_| error(EpubErrorCode::InvalidPackage));
        }
    }
    Ok(None)
}

fn required_attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    name: &[u8],
) -> Result<String, EpubError> {
    attribute(reader, element, name)?.ok_or_else(|| error(EpubErrorCode::InvalidPackage))
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
