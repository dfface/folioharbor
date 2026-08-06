use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::Read,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use folioharbor_application::{
    ports::{
        BlobStore, PublicationResourceReader, ReaderResource, ResourceReadRequest,
        ResourceReaderError,
    },
    reader::ResourceId,
};
use folioharbor_domain::id::{BlobId, PublicationPackageId};

use crate::{ContentSanitizer, EpubPath, ResourceResolver, SanitizerLimits};

const SANITIZER_VERSION: &str = "sanitizer-v1";

#[derive(Clone, Copy, Debug)]
pub struct ResourceCacheLimits {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub max_resource_bytes: usize,
}

impl Default for ResourceCacheLimits {
    fn default() -> Self {
        Self {
            max_entries: 64,
            max_bytes: 32 * 1024 * 1024,
            max_resource_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    blob: BlobId,
    package: PublicationPackageId,
    resource: String,
    sanitizer: &'static str,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<CacheKey, Vec<u8>>,
    order: VecDeque<CacheKey>,
    bytes: usize,
}

pub struct EpubResourceReader {
    blobs: Arc<dyn BlobStore>,
    limits: ResourceCacheLimits,
    cache: Mutex<Cache>,
}

impl EpubResourceReader {
    #[must_use]
    pub fn new(blobs: Arc<dyn BlobStore>, limits: ResourceCacheLimits) -> Self {
        Self {
            blobs,
            limits,
            cache: Mutex::new(Cache::default()),
        }
    }

    fn cached(&self, key: &CacheKey) -> Result<Option<Vec<u8>>, ResourceReaderError> {
        self.cache
            .lock()
            .map_err(|_| ResourceReaderError::Unavailable)
            .map(|cache| cache.entries.get(key).cloned())
    }

    fn insert(&self, key: CacheKey, bytes: Vec<u8>) -> Result<(), ResourceReaderError> {
        if self.limits.max_entries == 0 || bytes.len() > self.limits.max_bytes {
            return Ok(());
        }
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ResourceReaderError::Unavailable)?;
        while cache.entries.len() >= self.limits.max_entries
            || cache.bytes.saturating_add(bytes.len()) > self.limits.max_bytes
        {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            if let Some(removed) = cache.entries.remove(&oldest) {
                cache.bytes = cache.bytes.saturating_sub(removed.len());
            }
        }
        cache.bytes = cache.bytes.saturating_add(bytes.len());
        cache.order.push_back(key.clone());
        cache.entries.insert(key, bytes);
        Ok(())
    }
}

#[async_trait]
impl PublicationResourceReader for EpubResourceReader {
    async fn read(&self, request: ResourceReadRequest) -> Result<Vec<u8>, ResourceReaderError> {
        if !allowed_media_type(&request.media_type) {
            return Err(ResourceReaderError::Malformed);
        }
        EpubPath::new(&request.normalized_href).map_err(|_| ResourceReaderError::Malformed)?;
        let key = CacheKey {
            blob: request.blob_id,
            package: request.package_id,
            resource: request.normalized_href.clone(),
            sanitizer: SANITIZER_VERSION,
        };
        if let Some(bytes) = self.cached(&key)? {
            return Ok(bytes);
        }
        let source = self
            .blobs
            .open_publication(&request.storage_key)
            .await
            .map_err(|_| ResourceReaderError::Unavailable)?;
        let limit = self.limits.max_resource_bytes;
        let href = request.normalized_href.clone();
        let bytes = tokio::task::spawn_blocking(move || read_entry(source, &href, limit))
            .await
            .map_err(|_| ResourceReaderError::Unavailable)??;
        let transformed = transform(bytes, &request)?;
        if transformed.len() > limit {
            return Err(ResourceReaderError::Malformed);
        }
        self.insert(key, transformed.clone())?;
        Ok(transformed)
    }
}

fn read_entry(
    mut source: Box<dyn folioharbor_application::ports::PublicationSource>,
    href: &str,
    limit: usize,
) -> Result<Vec<u8>, ResourceReaderError> {
    let mut archive =
        zip::ZipArchive::new(&mut source).map_err(|_| ResourceReaderError::Malformed)?;
    let mut file = archive
        .by_name(href)
        .map_err(|_| ResourceReaderError::Malformed)?;
    if file.encrypted()
        || usize::try_from(file.size())
            .ok()
            .is_none_or(|size| size > limit)
    {
        return Err(ResourceReaderError::Malformed);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(limit));
    file.by_ref()
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ResourceReaderError::Malformed)?;
    if bytes.len() > limit {
        return Err(ResourceReaderError::Malformed);
    }
    Ok(bytes)
}

fn transform(
    bytes: Vec<u8>,
    request: &ResourceReadRequest,
) -> Result<Vec<u8>, ResourceReaderError> {
    if matches!(
        request.media_type.as_str(),
        "application/xhtml+xml" | "text/html"
    ) {
        let html = std::str::from_utf8(&bytes).map_err(|_| ResourceReaderError::Malformed)?;
        let resolver = OpaqueResolver::new(request)?;
        let sanitized = ContentSanitizer::transform(html, &resolver);
        if sanitized.html.is_empty() && !html.is_empty() {
            return Err(ResourceReaderError::Malformed);
        }
        return Ok(sanitized.html.into_bytes());
    }
    if request.media_type == "text/css" {
        let css = std::str::from_utf8(&bytes).map_err(|_| ResourceReaderError::Malformed)?;
        let resolver = OpaqueResolver::new(request)?;
        let sanitized =
            ContentSanitizer::transform_stylesheet(css, &resolver, SanitizerLimits::default());
        if sanitized.html.is_empty() && !css.is_empty() {
            return Err(ResourceReaderError::Malformed);
        }
        return Ok(sanitized.html.into_bytes());
    }
    Ok(bytes)
}

struct OpaqueResolver {
    base: EpubPath,
    resources: BTreeMap<EpubPath, String>,
}

impl OpaqueResolver {
    fn new(request: &ResourceReadRequest) -> Result<Self, ResourceReaderError> {
        let base =
            EpubPath::new(&request.normalized_href).map_err(|_| ResourceReaderError::Malformed)?;
        let resources = request
            .resources
            .iter()
            .map(|resource: &ReaderResource| {
                EpubPath::new(&resource.normalized_href).map(|path| {
                    (
                        path,
                        ResourceId::for_resource(request.package_id, &resource.normalized_href)
                            .as_str()
                            .to_owned(),
                    )
                })
            })
            .collect::<Result<_, _>>()
            .map_err(|_| ResourceReaderError::Malformed)?;
        Ok(Self { base, resources })
    }
}

impl ResourceResolver for OpaqueResolver {
    fn base(&self) -> &EpubPath {
        &self.base
    }
    fn resolve(&self, reference: &EpubPath) -> Option<String> {
        self.resources.get(reference).cloned()
    }
}

fn allowed_media_type(value: &str) -> bool {
    matches!(
        value,
        "application/xhtml+xml"
            | "text/html"
            | "text/css"
            | "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "font/woff"
            | "font/woff2"
            | "application/font-woff"
    )
}
