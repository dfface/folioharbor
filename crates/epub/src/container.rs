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
            Ok((
                ResolveResult::Bound(namespace),
                Event::Empty(element) | Event::Start(element),
            )) if namespace.as_ref() == CONTAINER_NS
                && element.local_name().as_ref() == b"rootfile" =>
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
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return Err(error(EpubErrorCode::InvalidContainer)),
        }
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
