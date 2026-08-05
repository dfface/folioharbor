use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek},
    time::Instant,
};

use crate::{EpubError, EpubErrorCode, EpubPath, ParserLimits};

pub(crate) struct BoundedArchive {
    entries: BTreeMap<EpubPath, Vec<u8>>,
    limits: ParserLimits,
    started: Instant,
}

impl BoundedArchive {
    pub(crate) fn read<R: Read + Seek>(
        source: &mut R,
        limits: ParserLimits,
    ) -> Result<Self, EpubError> {
        let started = Instant::now();
        check_deadline(started, limits)?;
        let mut zip =
            zip::ZipArchive::new(source).map_err(|_| error(EpubErrorCode::InvalidArchive))?;
        if zip.len() > limits.max_entries {
            return Err(error(EpubErrorCode::EntryLimit));
        }

        let mut metadata = Vec::with_capacity(zip.len().min(limits.max_entries));
        let mut total = 0_u64;
        let mut seen = BTreeSet::new();
        for index in 0..zip.len() {
            check_deadline(started, limits)?;
            let file = zip
                .by_index_raw(index)
                .map_err(|_| error(EpubErrorCode::InvalidArchive))?;
            if file.encrypted() {
                return Err(error(EpubErrorCode::EncryptedContent));
            }
            let path = std::str::from_utf8(file.name_raw())
                .map_err(|_| error(EpubErrorCode::UnsafePath))
                .and_then(|name| normalize_path(name, false))?;
            if path.as_str().split('/').count() > limits.max_path_depth {
                return Err(error(EpubErrorCode::PathDepthLimit));
            }
            let size = file.size();
            if size > limits.max_resource_bytes {
                return Err(error(EpubErrorCode::ResourceSizeLimit));
            }
            total = total
                .checked_add(size)
                .ok_or_else(|| error(EpubErrorCode::TotalSizeLimit))?;
            if total > limits.max_total_uncompressed_bytes {
                return Err(error(EpubErrorCode::TotalSizeLimit));
            }
            let compressed = file.compressed_size();
            let ratio_exceeded = compressed == 0
                || compressed
                    .checked_mul(limits.max_compression_ratio)
                    .is_some_and(|allowed_size| size > allowed_size);
            if size > 0 && ratio_exceeded {
                return Err(error(EpubErrorCode::CompressionRatioLimit));
            }
            if !seen.insert(path.clone()) {
                return Err(error(EpubErrorCode::DuplicatePath));
            }
            metadata.push((index, path, size));
        }

        let mut entries = BTreeMap::new();
        for (index, path, size) in metadata {
            check_deadline(started, limits)?;
            let capacity =
                usize::try_from(size).map_err(|_| error(EpubErrorCode::ResourceSizeLimit))?;
            let mut bytes = Vec::with_capacity(capacity);
            let mut file = zip.by_index(index).map_err(|zip_error| match zip_error {
                zip::result::ZipError::UnsupportedArchive(_) => {
                    error(EpubErrorCode::EncryptedContent)
                }
                _ => error(EpubErrorCode::InvalidArchive),
            })?;
            let mut buffer = [0_u8; 8 * 1024];
            loop {
                check_deadline(started, limits)?;
                let read = file
                    .read(&mut buffer)
                    .map_err(|_| error(EpubErrorCode::InvalidArchive))?;
                if read == 0 {
                    break;
                }
                if bytes.len().saturating_add(read) > capacity {
                    return Err(error(EpubErrorCode::InvalidArchive));
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            if u64::try_from(bytes.len()).ok() != Some(size) {
                return Err(error(EpubErrorCode::InvalidArchive));
            }
            entries.insert(path, bytes);
        }
        Ok(Self {
            entries,
            limits,
            started,
        })
    }

    pub(crate) fn get(&self, path: &EpubPath) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }

    pub(crate) fn contains(&self, path: &EpubPath) -> bool {
        self.entries.contains_key(path)
    }

    pub(crate) fn check_processing(&self, xml_depth: usize) -> Result<(), EpubError> {
        check_deadline(self.started, self.limits)?;
        if xml_depth > self.limits.max_xml_depth {
            return Err(error(EpubErrorCode::XmlDepthLimit));
        }
        Ok(())
    }
}

pub(crate) fn normalize_path(path: &str, allow_fragment: bool) -> Result<EpubPath, EpubError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err(error(EpubErrorCode::UnsafePath));
    }
    let (path, fragment) = if allow_fragment {
        path.split_once('#')
            .map_or((path, None), |(path, fragment)| (path, Some(fragment)))
    } else {
        (path, None)
    };
    if path.contains(':') {
        return Err(error(EpubErrorCode::UnsafePath));
    }
    let mut normalized = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if normalized.pop().is_none() {
                    return Err(error(EpubErrorCode::UnsafePath));
                }
            }
            value => normalized.push(value),
        }
    }
    if normalized.is_empty() {
        return Err(error(EpubErrorCode::UnsafePath));
    }
    let mut value = normalized.join("/");
    if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
        value.push('#');
        value.push_str(fragment);
    }
    Ok(EpubPath::from_normalized(value))
}

pub(crate) fn resolve_path(base: &str, reference: &str) -> Result<EpubPath, EpubError> {
    if reference.starts_with('#') {
        let base_without_fragment = base.split('#').next().unwrap_or(base);
        return normalize_path(&format!("{base_without_fragment}{reference}"), true);
    }
    let directory = base.rsplit_once('/').map_or("", |(directory, _)| directory);
    let joined = if directory.is_empty() {
        reference.to_owned()
    } else {
        format!("{directory}/{reference}")
    };
    normalize_path(&joined, true)
}

fn check_deadline(started: Instant, limits: ParserLimits) -> Result<(), EpubError> {
    if started.elapsed() >= limits.deadline {
        Err(error(EpubErrorCode::DeadlineExceeded))
    } else {
        Ok(())
    }
}

fn error(code: EpubErrorCode) -> EpubError {
    EpubError::new(code)
}
