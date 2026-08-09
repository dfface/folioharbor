mod archive;
mod container;
mod error;
mod navigation;
mod ncx;
mod package;
mod resource_reader;
mod sanitize;

use std::{
    io::{Read, Seek},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use folioharbor_application::ports::{
    BlobStore, BlobStoreError, PublicationParser, PublicationParserError,
};
use folioharbor_domain::{
    catalog::{
        CatalogMetadata, CatalogPublication, ParserMetadata,
        PublicationResource as CatalogResource, SpineEntry as CatalogSpine, TocEntry as CatalogToc,
    },
    imports::blob::StorageKey,
};

pub use error::{EpubError, EpubErrorCode};
pub use package::{Metadata, ParsedPublication, PublicationResource, SpineItem, TocEntry};
pub use resource_reader::{
    BlockingWorkHook, CacheMetrics, EpubResourceReader, ResourceCacheLimits,
};
pub use sanitize::{ContentSanitizer, ResourceResolver, SanitizedContent, SanitizerLimits};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EpubPath(String);

impl EpubPath {
    /// Validates and normalizes an EPUB-internal path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is absolute, escaping, or otherwise unsafe.
    pub fn new(path: &str) -> Result<Self, EpubError> {
        archive::normalize_path(path, false)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_normalized(path: String) -> Self {
        Self(path)
    }

    fn resolve_from(base: &str, reference: &str) -> Result<Self, EpubError> {
        archive::resolve_path(base, reference)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParserLimits {
    pub max_entries: usize,
    pub max_central_directory_bytes: u64,
    pub max_total_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
    pub max_path_depth: usize,
    pub max_xml_depth: usize,
    pub max_resource_bytes: u64,
    pub deadline: Duration,
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self {
            max_entries: 4_096,
            max_central_directory_bytes: 16 * 1024 * 1024,
            max_total_uncompressed_bytes: 512 * 1024 * 1024,
            max_compression_ratio: 200,
            max_path_depth: 32,
            max_xml_depth: 64,
            max_resource_bytes: 64 * 1024 * 1024,
            deadline: Duration::from_secs(30),
        }
    }
}

pub struct EpubParser;

impl EpubParser {
    /// Inspects a bounded EPUB archive and returns format-neutral publication data.
    ///
    /// # Errors
    ///
    /// Returns a stable EPUB error when the archive exceeds limits or is malformed.
    pub fn inspect<R: Read + Seek>(
        source: &mut R,
        limits: ParserLimits,
    ) -> Result<ParsedPublication, EpubError> {
        let archive = archive::BoundedArchive::read(source, limits)?;
        let package_path = container::package_path(&archive)?;
        package::parse(&archive, &package_path)
    }
}

pub struct EpubPublicationParser {
    blobs: Arc<dyn BlobStore>,
    limits: ParserLimits,
}

impl EpubPublicationParser {
    #[must_use]
    pub fn new(blobs: Arc<dyn BlobStore>, limits: ParserLimits) -> Self {
        Self { blobs, limits }
    }
}

#[async_trait]
impl PublicationParser for EpubPublicationParser {
    fn profile_version(&self) -> &'static str {
        "epub-v2"
    }

    async fn parse(&self, key: &StorageKey) -> Result<CatalogPublication, PublicationParserError> {
        let mut source = self
            .blobs
            .open_publication(key)
            .await
            .map_err(|error| map_storage_error(&error))?;
        let parsed = EpubParser::inspect(&mut source, self.limits)
            .map_err(|_| PublicationParserError::Malformed)?;
        map_publication(parsed)
    }
}

fn map_storage_error(error: &BlobStoreError) -> PublicationParserError {
    match error {
        BlobStoreError::InsufficientCapacity => PublicationParserError::Capacity,
        BlobStoreError::InvalidKey
        | BlobStoreError::IdentityMismatch
        | BlobStoreError::InvalidRange => PublicationParserError::Configuration,
        BlobStoreError::Io(_) => PublicationParserError::Unavailable,
    }
}

fn map_publication(
    parsed: ParsedPublication,
) -> Result<CatalogPublication, PublicationParserError> {
    let metadata = CatalogMetadata::from_parser(&ParserMetadata {
        titles: parsed.metadata.titles,
        authors: parsed.metadata.authors,
        languages: parsed.metadata.languages,
        identifiers: parsed.metadata.identifiers,
    })
    .map_err(|_| PublicationParserError::Malformed)?;
    let resources = parsed
        .resources
        .into_iter()
        .map(|resource| CatalogResource::new(resource.href.as_str(), resource.media_type))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PublicationParserError::Malformed)?;
    let spine = parsed
        .spine
        .into_iter()
        .map(|item| CatalogSpine::new(item.href.as_str(), item.linear))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PublicationParserError::Malformed)?;
    let toc = parsed
        .toc
        .into_iter()
        .map(|entry| CatalogToc::new(entry.label, entry.href.as_str()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PublicationParserError::Malformed)?;
    CatalogPublication::from_parser(
        metadata,
        resources,
        spine,
        toc,
        parsed.cover.map(|path| path.as_str().to_owned()),
    )
    .map_err(|_| PublicationParserError::Malformed)
}
