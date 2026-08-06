use quick_xml::{XmlVersion, events::Event, name::ResolveResult, reader::NsReader};

use crate::{EpubError, EpubErrorCode, EpubPath, archive::BoundedArchive};

const CONTAINER_PATH: &str = "META-INF/container.xml";
const CONTAINER_NS: &[u8] = b"urn:oasis:names:tc:opendocument:xmlns:container";

pub(crate) fn package_path(archive: &BoundedArchive) -> Result<EpubPath, EpubError> {
    let container_path = EpubPath::new(CONTAINER_PATH)?;
    let xml = archive
        .get(&container_path)
        .ok_or_else(|| error(EpubErrorCode::InvalidContainer))?;
    let mut reader = NsReader::from_reader(xml);
    let mut rootfile = None;
    let mut depth = 0_usize;
    let mut saw_container = false;
    let mut saw_rootfiles = false;
    let mut in_rootfiles = false;
    loop {
        archive.check_processing(depth)?;
        let event = reader.read_resolved_event();
        match event {
            Ok((namespace, Event::Start(element))) => {
                depth = depth.saturating_add(1);
                archive.check_processing(depth)?;
                let local = element.local_name();
                let is_container_ns =
                    matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == CONTAINER_NS);
                if depth == 1 {
                    if !is_container_ns || local.as_ref() != b"container" {
                        return Err(error(EpubErrorCode::InvalidContainer));
                    }
                    saw_container = true;
                } else if depth == 2 && local.as_ref() == b"rootfiles" {
                    if !is_container_ns || !saw_container || saw_rootfiles {
                        return Err(error(EpubErrorCode::InvalidContainer));
                    }
                    saw_rootfiles = true;
                    in_rootfiles = true;
                } else if local.as_ref() == b"container" || local.as_ref() == b"rootfile" {
                    return Err(error(EpubErrorCode::InvalidContainer));
                }
            }
            Ok((ResolveResult::Bound(namespace), Event::Empty(element)))
                if namespace.as_ref() == CONTAINER_NS
                    && element.local_name().as_ref() == b"rootfile"
                    && depth == 2
                    && in_rootfiles =>
            {
                for attribute in element.attributes().with_checks(true) {
                    let attribute =
                        attribute.map_err(|_| error(EpubErrorCode::InvalidContainer))?;
                    if attribute.key.local_name().as_ref() == b"full-path" {
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|_| error(EpubErrorCode::InvalidContainer))?;
                        let path = EpubPath::new(&value)?;
                        if rootfile.replace(path).is_some() {
                            return Err(error(EpubErrorCode::InvalidContainer));
                        }
                    }
                }
            }
            Ok((_, Event::Empty(element)))
                if matches!(
                    element.local_name().as_ref(),
                    b"container" | b"rootfiles" | b"rootfile"
                ) =>
            {
                return Err(error(EpubErrorCode::InvalidContainer));
            }
            Ok((namespace, Event::End(element))) => {
                let local = element.local_name();
                let is_container_ns =
                    matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == CONTAINER_NS);
                if depth == 2 && local.as_ref() == b"rootfiles" {
                    if !is_container_ns || !in_rootfiles {
                        return Err(error(EpubErrorCode::InvalidContainer));
                    }
                    in_rootfiles = false;
                } else if depth == 1 && (!is_container_ns || local.as_ref() != b"container") {
                    return Err(error(EpubErrorCode::InvalidContainer));
                }
                depth = depth.saturating_sub(1);
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error(EpubErrorCode::InvalidContainer)),
        }
    }
    if !saw_container || !saw_rootfiles || depth != 0 || in_rootfiles {
        return Err(error(EpubErrorCode::InvalidContainer));
    }
    let path = rootfile.ok_or_else(|| error(EpubErrorCode::InvalidContainer))?;
    if archive.contains(&path) {
        Ok(path)
    } else {
        Err(error(EpubErrorCode::MissingPackage))
    }
}

fn error(code: EpubErrorCode) -> EpubError {
    EpubError::new(code)
}
