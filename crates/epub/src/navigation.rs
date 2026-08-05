use quick_xml::{XmlVersion, events::Event, name::ResolveResult, reader::NsReader};

use crate::{EpubError, EpubErrorCode, EpubPath, archive::BoundedArchive, package::TocEntry};

const XHTML_NS: &[u8] = b"http://www.w3.org/1999/xhtml";
const EPUB_NS: &[u8] = b"http://www.idpf.org/2007/ops";

pub(crate) fn parse(
    archive: &BoundedArchive,
    xml: &[u8],
    nav_path: &EpubPath,
) -> Result<Vec<TocEntry>, EpubError> {
    let mut reader = NsReader::from_reader(xml);
    let mut in_toc = false;
    let mut current_href = None;
    let mut current_label = String::new();
    let mut toc = Vec::new();
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
                if namespace.as_ref() == XHTML_NS && element.local_name().as_ref() == b"nav" =>
            {
                in_toc = element.attributes().with_checks(true).filter_map(Result::ok).any(|attribute| {
                    matches!(reader.resolver().resolve_attribute(attribute.key), (ResolveResult::Bound(ns), local) if ns.as_ref() == EPUB_NS && local.as_ref() == b"type")
                        && attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder()).is_ok_and(|value| value.split_ascii_whitespace().any(|item| item == "toc"))
                });
            }
            Ok((ResolveResult::Bound(namespace), Event::Start(element)))
                if in_toc
                    && namespace.as_ref() == XHTML_NS
                    && element.local_name().as_ref() == b"a" =>
            {
                for attribute in element.attributes().with_checks(true) {
                    let attribute = attribute.map_err(|_| error())?;
                    if attribute.key.local_name().as_ref() == b"href" {
                        let href = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|_| error())?;
                        current_href = Some(
                            EpubPath::resolve_from(nav_path.as_str(), &href)
                                .map_err(|_| error())?,
                        );
                    }
                }
                current_label.clear();
            }
            Ok((_, Event::Text(text))) if current_href.is_some() => {
                current_label.push_str(
                    text.xml_content(XmlVersion::Implicit1_0)
                        .map_err(|_| error())?
                        .trim(),
                );
            }
            Ok((ResolveResult::Bound(namespace), Event::End(element)))
                if namespace.as_ref() == XHTML_NS && element.local_name().as_ref() == b"a" =>
            {
                if let Some(href) = current_href.take() {
                    if !current_label.is_empty() {
                        toc.push(TocEntry {
                            label: current_label.clone(),
                            href,
                        });
                    }
                }
            }
            Ok((ResolveResult::Bound(namespace), Event::End(element)))
                if namespace.as_ref() == XHTML_NS && element.local_name().as_ref() == b"nav" =>
            {
                in_toc = false;
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error()),
        }
    }
    Ok(toc)
}

fn error() -> EpubError {
    EpubError::new(EpubErrorCode::InvalidNavigation)
}
